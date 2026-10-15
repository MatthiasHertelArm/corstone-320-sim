//! The interface between the subsystem and the board around it: the MPS4 I/O, the host side
//! of the UARTs, the display, the camera and the audio.

use cs320_core::Ticks;

/// Pixel format of a frame handed to the display or fetched from the camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit grey.
    Gray8,
    /// 16-bit RGB 5:6:5 (little endian).
    Rgb565,
    /// 24-bit RGB.
    Rgb888,
    /// 32-bit, 8 bits each of A, R, G, B (as in a frame buffer word 0xAARRGGBB).
    Argb8888,
    /// NV12 (Y plane followed by interleaved UV at half resolution).
    Nv12,
    /// NV21.
    Nv21,
    /// YUV 4:2:0 planar.
    Yuv420,
    /// 16-bit BGR 5:6:5.
    Bgr565,
}

impl PixelFormat {
    /// Bits per pixel (averaged for the planar formats).
    pub fn bits(self) -> u32 {
        match self {
            PixelFormat::Gray8 => 8,
            PixelFormat::Rgb565 | PixelFormat::Bgr565 => 16,
            PixelFormat::Rgb888 => 24,
            PixelFormat::Argb8888 => 32,
            PixelFormat::Nv12 | PixelFormat::Nv21 | PixelFormat::Yuv420 => 12,
        }
    }

    /// Size of a frame in bytes.
    pub fn frame_bytes(self, w: u32, h: u32) -> usize {
        ((w as usize * h as usize * self.bits() as usize) + 7) / 8
    }
}

/// A frame of RGBA pixels (8 bits each) for the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the board provides to the subsystem. All methods have defaults so that a test board
/// only implements what it needs.
pub trait BoardIo {
    /// The UART `n` transmitted `byte` (frame completed at `now`).
    fn uart_tx(&mut self, _now: Ticks, _n: usize, _byte: u8) {}

    /// The FPGA I/O LEDs changed (bit n = LED n).
    fn fpga_leds(&mut self, _now: Ticks, _leds: u32) {}

    /// FPGA I/O user buttons (bit 0 = PB1, bit 1 = PB2).
    fn fpga_buttons(&mut self, _now: Ticks) -> u32 {
        0
    }

    /// FPGA I/O user switches (bits 7:0).
    fn fpga_switches(&mut self, _now: Ticks) -> u32 {
        0
    }

    /// FPGA I/O MISC register changed (SPI chip selects).
    fn fpga_misc(&mut self, _now: Ticks, _misc: u32) {}

    /// A CMSDK GPIO port drives new output levels (`value` masked by `enabled`).
    fn gpio_out(&mut self, _now: Ticks, _port: usize, _value: u16, _enabled: u16) {}

    /// Input levels of a CMSDK GPIO port.
    fn gpio_in(&mut self, _now: Ticks, _port: usize) -> u16 {
        0
    }

    /// VIO signal output changed (bit n = vioLED n).
    fn vio_signal_out(&mut self, _now: Ticks, _signal: u32) {}

    /// VIO signal input (bit n = vioBUTTON n).
    fn vio_signal_in(&mut self, _now: Ticks) -> u32 {
        0
    }

    /// VIO value `id` written by the target.
    fn vio_value_out(&mut self, _now: Ticks, _id: usize, _value: i32) {}

    /// VIO value `id` read by the target.
    fn vio_value_in(&mut self, _now: Ticks, _id: usize) -> Option<i32> {
        None
    }

    /// I2C bus `bus`: START with address `addr` (7-bit) and direction; returns the ACK.
    fn i2c_start(&mut self, _now: Ticks, _bus: usize, _addr: u8, _read: bool) -> bool {
        false
    }

    /// I2C bus `bus`: a data byte written by the master; returns the ACK.
    fn i2c_write_byte(&mut self, _now: Ticks, _bus: usize, _byte: u8) -> bool {
        false
    }

    /// I2C bus `bus`: a data byte read by the master (`last` = master NACKs it).
    fn i2c_read_byte(&mut self, _now: Ticks, _bus: usize, _last: bool) -> u8 {
        0xFF
    }

    fn i2c_stop(&mut self, _now: Ticks, _bus: usize) {}

    /// Microphone samples: fills `buf` with 16-bit signed samples at `rate` Hz for `channels`
    /// channels (interleaved), starting at simulated time `now`.
    fn mic_samples(&mut self, _now: Ticks, _rate: u32, _channels: u32, buf: &mut [i16]) {
        buf.fill(0);
    }

    /// Speaker samples (16-bit signed, interleaved) at `rate` Hz from `now`.
    fn speaker_samples(&mut self, _now: Ticks, _rate: u32, _channels: u32, _samples: &[i16]) {}

    /// Camera frame: fills `buf` with a frame of `w` × `h` in `format` for camera `cam`
    /// (0 = VSI4 / CAM, 1 = VSI5); returns false when no frame source is configured.
    fn camera_frame(&mut self, _now: Ticks, _cam: usize, _w: u32, _h: u32, _format: PixelFormat, buf: &mut [u8]) -> bool {
        buf.fill(0);
        false
    }

    /// The display controller `display` (0 = HDLCD, 1 = VSI6, 2 = VSI7) produced a frame.
    fn display_frame(&mut self, _now: Ticks, _display: usize, _frame: Frame) {}

    /// Unix time for the RTC at reset.
    fn rtc_epoch(&mut self) -> u32 {
        0
    }

    /// Next time the board changes an input by itself; `Ticks::MAX` if never.
    fn next_event(&mut self, _now: Ticks) -> Ticks {
        Ticks::MAX
    }
}

/// A board with nothing connected (for tests): collects the UART output.
#[derive(Default)]
pub struct NullBoard {
    pub uart: Vec<Vec<u8>>,
    pub leds: u32,
    pub vio_leds: u32,
    pub frames: usize,
}

impl BoardIo for NullBoard {
    fn uart_tx(&mut self, _now: Ticks, n: usize, byte: u8) {
        if self.uart.len() <= n {
            self.uart.resize_with(n + 1, Vec::new);
        }
        self.uart[n].push(byte);
    }

    fn fpga_leds(&mut self, _now: Ticks, leds: u32) {
        self.leds = leds;
    }

    fn vio_signal_out(&mut self, _now: Ticks, signal: u32) {
        self.vio_leds = signal;
    }

    fn display_frame(&mut self, _now: Ticks, _display: usize, _frame: Frame) {
        self.frames += 1;
    }
}
