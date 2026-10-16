//! SBCon I2C (`i2c_sbcon_reg_map.h`): a bit-banged bus with SCL in bit 0 and SDA in bit 1 of
//! the control register (set at 0x000, clear at 0x004, status read at 0x000). The model
//! decodes the bit stream into START, address, data bytes and STOP and asks the board's
//! devices for acknowledges and data.

use crate::env::{narrow, Env};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    /// Receiving a byte from the master (the address after a START, or data).
    Rx(u8),
    /// The master sent a byte; the slave acknowledges in the 9th clock. The flag tells
    /// whether the ACK level has been driven (first falling edge seen).
    RxAck(bool),
    /// Transmitting a byte to the master: bits sampled so far.
    Tx(u8),
    /// The master acknowledges a byte it read (`Some(nack)` once sampled).
    TxAck(Option<bool>),
}

pub struct SbconI2c {
    pub bus: usize,
    /// Levels the master drives (true = released high).
    scl: bool,
    sda: bool,
    /// Level the slave drives on SDA (false = pulled low).
    slave_sda: bool,
    state: State,
    shift: u8,
    started: bool,
    /// The byte being received is the address byte.
    expect_address: bool,
    reading: bool,
    ack: bool,
    tx_byte: u8,
}

impl SbconI2c {
    pub fn new(bus: usize) -> Self {
        SbconI2c {
            bus,
            scl: true,
            sda: true,
            slave_sda: true,
            state: State::Idle,
            shift: 0,
            started: false,
            expect_address: false,
            reading: false,
            ack: false,
            tx_byte: 0,
        }
    }

    pub fn reset(&mut self) {
        self.scl = true;
        self.sda = true;
        self.slave_sda = true;
        self.state = State::Idle;
        self.started = false;
    }

    fn sda_level(&self) -> bool {
        self.sda && self.slave_sda
    }

    pub fn read(&self, off: u32, size: u8) -> u32 {
        let v = match off & !3 {
            0x000 => (self.scl as u32) | ((self.sda_level() as u32) << 1),
            _ => 0,
        };
        narrow(v, off, size)
    }

    pub fn write(&mut self, env: &mut Env, off: u32, _size: u8, v: u32) {
        let (scl, sda) = (v & 1 != 0, v & 2 != 0);
        match off & !3 {
            0x000 => {
                if sda {
                    self.sda_edge(env, true);
                }
                if scl {
                    self.scl_edge(env, true);
                }
            }
            0x004 => {
                if scl {
                    self.scl_edge(env, false);
                }
                if sda {
                    self.sda_edge(env, false);
                }
            }
            _ => {}
        }
    }

    fn sda_edge(&mut self, env: &mut Env, high: bool) {
        if self.sda == high {
            return;
        }
        self.sda = high;
        if !self.scl {
            return;
        }
        if !high {
            // START or repeated START
            self.started = true;
            self.state = State::Rx(0);
            self.shift = 0;
            self.expect_address = true;
            self.slave_sda = true;
        } else if self.started {
            // STOP
            self.started = false;
            self.state = State::Idle;
            self.slave_sda = true;
            env.board.i2c_stop(env.now, self.bus);
        }
    }

    fn scl_edge(&mut self, env: &mut Env, high: bool) {
        if self.scl == high {
            return;
        }
        self.scl = high;
        if !self.started {
            return;
        }
        if high {
            // the slave samples (or the master samples what the slave drives) on the rising edge
            match self.state {
                State::Rx(n) => {
                    self.shift = (self.shift << 1) | (self.sda as u8);
                    if n + 1 == 8 {
                        let byte = self.shift;
                        self.ack = if self.expect_address {
                            self.reading = byte & 1 != 0;
                            env.board.i2c_start(env.now, self.bus, byte >> 1, self.reading)
                        } else {
                            env.board.i2c_write_byte(env.now, self.bus, byte)
                        };
                        self.expect_address = false;
                        self.state = State::RxAck(false);
                    } else {
                        self.state = State::Rx(n + 1);
                    }
                }
                State::Tx(n) => self.state = State::Tx(n + 1),
                State::TxAck(None) => self.state = State::TxAck(Some(self.sda)),
                _ => {}
            }
        } else {
            // the slave changes SDA after the falling edge
            match self.state {
                State::RxAck(false) => {
                    self.slave_sda = !self.ack;
                    self.state = State::RxAck(true);
                }
                State::RxAck(true) => {
                    self.slave_sda = true;
                    if self.reading && self.ack {
                        self.begin_tx(env);
                    } else {
                        self.shift = 0;
                        self.state = State::Rx(0);
                    }
                }
                State::Tx(n) => {
                    if n >= 8 {
                        self.slave_sda = true;
                        self.state = State::TxAck(None);
                    } else {
                        self.slave_sda = (self.tx_byte >> (7 - n)) & 1 != 0;
                    }
                }
                State::TxAck(Some(nack)) => {
                    if nack {
                        self.slave_sda = true;
                        self.state = State::Idle;
                    } else {
                        self.begin_tx(env);
                    }
                }
                _ => {}
            }
        }
    }

    fn begin_tx(&mut self, env: &mut Env) {
        self.tx_byte = env.board.i2c_read_byte(env.now, self.bus, false);
        self.state = State::Tx(0);
        self.slave_sda = (self.tx_byte >> 7) & 1 != 0;
    }
}
