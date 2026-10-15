//! The MPS4 board around the SSE-320 as the simulator presents it: the user LEDs, buttons and
//! switches, the VIO signals and values, the UART host side, the display panels, the camera,
//! the microphone and the speaker, and the I2C devices.

use crate::sources::{Camera, CameraSource, MicSource, Microphone, Rgba, Speaker};
use cs320_core::Ticks;
use cs320_soc::board::{BoardIo, Frame, PixelFormat};
use cs320_soc::vsi::from_rgba;

/// Inputs of the physical world, set by the GUI, scenarios or the debugger.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// FPGA I/O user buttons PB1, PB2.
    pub buttons: [bool; 2],
    /// FPGA I/O user switches.
    pub switches: u8,
    /// VIO buttons (bit n = vioBUTTONn).
    pub vio_buttons: u32,
    /// VIO values provided by the host (None = what the target wrote).
    pub vio_values: Vec<(usize, i32)>,
    /// Input levels of the GPIO ports.
    pub gpio_in: [u16; 4],
}

/// A simple I2C register device: a byte-addressed register file with auto-increment.
#[derive(Clone, Debug)]
pub struct I2cDevice {
    pub addr: u8,
    pub name: &'static str,
    pub regs: Vec<u8>,
    ptr: usize,
    first: bool,
}

impl I2cDevice {
    pub fn new(addr: u8, name: &'static str, size: usize) -> Self {
        I2cDevice { addr, name, regs: vec![0; size], ptr: 0, first: true }
    }
}

pub struct Cs320Board {
    pub inputs: Inputs,
    pub leds: u32,
    pub vio_leds: u32,
    pub vio_values: [i32; 64],
    pub gpio_out: [(u16, u16); 4],
    pub misc: u32,
    /// Bytes the target sent on each UART, drained by the runtime.
    pub uart_out: Vec<Vec<u8>>,
    /// The latest frame of each display (HDLCD, VSI6, VSI7) and the frame counters.
    pub displays: Vec<Option<Frame>>,
    pub frame_counts: Vec<u64>,
    /// Display frames since the last drain (for a GUI that wants every frame).
    pub new_frames: Vec<(usize, Frame)>,
    pub keep_frames: bool,
    pub camera: Camera,
    pub mic: Microphone,
    pub speaker: Speaker,
    pub i2c_devices: Vec<I2cDevice>,
    i2c_active: [Option<usize>; 5],
    cam_buf: Vec<u8>,
    /// Scenario-controlled time at which an input changes (buttons released etc.).
    pub next_input_event: Ticks,
}

impl Default for Cs320Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Cs320Board {
    pub fn new() -> Self {
        let mut codec = I2cDevice::new(0x4A, "audio codec", 0x40);
        codec.regs[0x01] = 0xE3; // chip ID
        let eeprom = I2cDevice::new(0x50, "DDR4 EEPROM", 256);
        Cs320Board {
            inputs: Inputs::default(),
            leds: 0,
            vio_leds: 0,
            vio_values: [0; 64],
            gpio_out: [(0, 0); 4],
            misc: 0,
            uart_out: vec![Vec::new(); 6],
            displays: vec![None, None, None],
            frame_counts: vec![0; 3],
            new_frames: Vec::new(),
            keep_frames: false,
            camera: Camera::new(CameraSource::TestPattern),
            mic: Microphone::new(MicSource::Silence),
            speaker: Speaker::default(),
            i2c_devices: vec![codec, eeprom],
            i2c_active: [None; 5],
            cam_buf: Vec::new(),
            next_input_event: Ticks::MAX,
        }
    }

    /// Takes the latest display frame.
    pub fn take_frame(&mut self, display: usize) -> Option<Frame> {
        self.displays.get_mut(display).and_then(|d| d.take())
    }

    /// The camera preview as the last delivered frame.
    pub fn camera_preview(&self) -> Option<&Rgba> {
        self.camera.preview.as_ref()
    }
}

impl BoardIo for Cs320Board {
    fn uart_tx(&mut self, _now: Ticks, n: usize, byte: u8) {
        if let Some(u) = self.uart_out.get_mut(n) {
            u.push(byte);
        }
    }

    fn fpga_leds(&mut self, _now: Ticks, leds: u32) {
        self.leds = leds;
    }

    fn fpga_buttons(&mut self, _now: Ticks) -> u32 {
        (self.inputs.buttons[0] as u32) | ((self.inputs.buttons[1] as u32) << 1)
    }

    fn fpga_switches(&mut self, _now: Ticks) -> u32 {
        self.inputs.switches as u32
    }

    fn fpga_misc(&mut self, _now: Ticks, misc: u32) {
        self.misc = misc;
    }

    fn gpio_out(&mut self, _now: Ticks, port: usize, value: u16, enabled: u16) {
        if port < 4 {
            self.gpio_out[port] = (value, enabled);
        }
    }

    fn gpio_in(&mut self, _now: Ticks, port: usize) -> u16 {
        self.inputs.gpio_in.get(port).copied().unwrap_or(0)
    }

    fn vio_signal_out(&mut self, _now: Ticks, signal: u32) {
        self.vio_leds = signal;
    }

    fn vio_signal_in(&mut self, _now: Ticks) -> u32 {
        self.inputs.vio_buttons
    }

    fn vio_value_out(&mut self, _now: Ticks, id: usize, value: i32) {
        if id < 64 {
            self.vio_values[id] = value;
        }
    }

    fn vio_value_in(&mut self, _now: Ticks, id: usize) -> Option<i32> {
        self.inputs.vio_values.iter().find(|(i, _)| *i == id).map(|(_, v)| *v)
    }

    fn i2c_start(&mut self, _now: Ticks, bus: usize, addr: u8, read: bool) -> bool {
        let bus = bus.min(4);
        let dev = self.i2c_devices.iter().position(|d| d.addr == addr);
        self.i2c_active[bus] = dev;
        if let Some(i) = dev {
            if !read {
                self.i2c_devices[i].first = true;
            }
            true
        } else {
            false
        }
    }

    fn i2c_write_byte(&mut self, _now: Ticks, bus: usize, byte: u8) -> bool {
        let Some(i) = self.i2c_active[bus.min(4)] else { return false };
        let d = &mut self.i2c_devices[i];
        if d.first {
            d.ptr = byte as usize % d.regs.len();
            d.first = false;
        } else {
            d.regs[d.ptr] = byte;
            d.ptr = (d.ptr + 1) % d.regs.len();
        }
        true
    }

    fn i2c_read_byte(&mut self, _now: Ticks, bus: usize, _last: bool) -> u8 {
        let Some(i) = self.i2c_active[bus.min(4)] else { return 0xFF };
        let d = &mut self.i2c_devices[i];
        let v = d.regs[d.ptr];
        d.ptr = (d.ptr + 1) % d.regs.len();
        v
    }

    fn i2c_stop(&mut self, _now: Ticks, bus: usize) {
        self.i2c_active[bus.min(4)] = None;
    }

    fn mic_samples(&mut self, now: Ticks, rate: u32, channels: u32, buf: &mut [i16]) {
        self.mic.samples(now, rate, channels, buf);
    }

    fn speaker_samples(&mut self, now: Ticks, rate: u32, channels: u32, samples: &[i16]) {
        self.speaker.push(now, rate, channels, samples);
    }

    fn camera_frame(&mut self, now: Ticks, _cam: usize, w: u32, h: u32, format: PixelFormat, buf: &mut [u8]) -> bool {
        match self.camera.next_frame(now, w, h) {
            Some(f) => {
                self.cam_buf.clear();
                from_rgba(&f.data, w, h, format, buf);
                true
            }
            None => {
                buf.fill(0);
                false
            }
        }
    }

    fn display_frame(&mut self, _now: Ticks, display: usize, frame: Frame) {
        if display < self.displays.len() {
            self.frame_counts[display] += 1;
            if self.keep_frames {
                self.new_frames.push((display, frame.clone()));
                if self.new_frames.len() > 8 {
                    self.new_frames.remove(0);
                }
            }
            self.displays[display] = Some(frame);
        }
    }

    fn rtc_epoch(&mut self) -> u32 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
    }

    fn next_event(&mut self, _now: Ticks) -> Ticks {
        self.next_input_event
    }
}
