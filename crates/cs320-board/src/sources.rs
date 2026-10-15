//! Frame sources for the camera and sample sources for the microphone; the speaker sink.

use cs320_core::time::{to_secs, TICKS_PER_SEC};
use cs320_core::Ticks;
use cs320_soc::board::Frame;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A decoded image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba {
    pub fn load(path: &Path) -> Result<Rgba, String> {
        let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?.to_rgba8();
        Ok(Rgba { width: img.width(), height: img.height(), data: img.into_raw() })
    }

    /// Resamples (nearest neighbour) to `w` × `h`.
    pub fn resized(&self, w: u32, h: u32) -> Rgba {
        if w == self.width && h == self.height {
            return self.clone();
        }
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let sy = (y as u64 * self.height as u64 / h.max(1) as u64) as usize;
            for x in 0..w {
                let sx = (x as u64 * self.width as u64 / w.max(1) as u64) as usize;
                let s = (sy * self.width as usize + sx) * 4;
                let d = ((y * w + x) * 4) as usize;
                data[d..d + 4].copy_from_slice(&self.data[s..s + 4]);
            }
        }
        Rgba { width: w, height: h, data }
    }

    pub fn to_frame(&self) -> Frame {
        Frame { width: self.width, height: self.height, rgba: self.data.clone() }
    }
}

/// Where the camera frames come from.
#[derive(Clone, Debug)]
pub enum CameraSource {
    /// No camera connected.
    None,
    /// A moving colour-bar test pattern with a frame counter.
    TestPattern,
    /// One image.
    Image(PathBuf),
    /// All images of a folder (sorted), one per frame.
    Folder(PathBuf),
    /// Frames pushed by the host (webcam thread) into the shared slot.
    Live,
}

impl Default for CameraSource {
    fn default() -> Self {
        CameraSource::TestPattern
    }
}

/// Camera state: the source and its decoded frames.
pub struct Camera {
    pub source: CameraSource,
    frames: Vec<Rgba>,
    index: usize,
    /// Frames from the host (live source), newest last.
    pub live: Arc<Mutex<Option<Rgba>>>,
    last_live: Option<Rgba>,
    pub error: Option<String>,
    /// The most recent frame delivered (for the preview).
    pub preview: Option<Rgba>,
    counter: u64,
}

impl Camera {
    pub fn new(source: CameraSource) -> Camera {
        let mut c = Camera { source: CameraSource::None, frames: Vec::new(), index: 0, live: Arc::new(Mutex::new(None)), last_live: None, error: None, preview: None, counter: 0 };
        c.set_source(source);
        c
    }

    pub fn set_source(&mut self, source: CameraSource) {
        self.frames.clear();
        self.index = 0;
        self.error = None;
        match &source {
            CameraSource::Image(p) => match Rgba::load(p) {
                Ok(i) => self.frames.push(i),
                Err(e) => self.error = Some(e),
            },
            CameraSource::Folder(dir) => {
                let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
                    .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| matches!(p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg" | "bmp"))).collect())
                    .unwrap_or_default();
                paths.sort();
                for p in paths {
                    match Rgba::load(&p) {
                        Ok(i) => self.frames.push(i),
                        Err(e) => log::warn!("{e}"),
                    }
                }
                if self.frames.is_empty() {
                    self.error = Some(format!("{}: no images", dir.display()));
                }
            }
            _ => {}
        }
        self.source = source;
    }

    pub fn connected(&self) -> bool {
        match self.source {
            CameraSource::None => false,
            CameraSource::Image(_) | CameraSource::Folder(_) => !self.frames.is_empty(),
            CameraSource::Live => self.last_live.is_some() || self.live.lock().map(|l| l.is_some()).unwrap_or(false),
            CameraSource::TestPattern => true,
        }
    }

    /// The next frame at `w` × `h`, or `None` without a source.
    pub fn next_frame(&mut self, now: Ticks, w: u32, h: u32) -> Option<Rgba> {
        self.counter += 1;
        let frame = match &self.source {
            CameraSource::None => return None,
            CameraSource::TestPattern => test_pattern(w, h, to_secs(now)),
            CameraSource::Image(_) => self.frames.first()?.resized(w, h),
            CameraSource::Folder(_) => {
                if self.frames.is_empty() {
                    return None;
                }
                let f = self.frames[self.index % self.frames.len()].resized(w, h);
                self.index += 1;
                f
            }
            CameraSource::Live => {
                if let Ok(mut l) = self.live.lock() {
                    if let Some(f) = l.take() {
                        self.last_live = Some(f);
                    }
                }
                self.last_live.as_ref()?.resized(w, h)
            }
        };
        self.preview = Some(frame.clone());
        Some(frame)
    }
}

/// Colour bars with a moving marker and a seconds counter rendered as a bar.
pub fn test_pattern(w: u32, h: u32, t: f64) -> Rgba {
    let mut data = vec![0u8; (w * h * 4) as usize];
    let bars: [[u8; 3]; 8] = [[255, 255, 255], [255, 255, 0], [0, 255, 255], [0, 255, 0], [255, 0, 255], [255, 0, 0], [0, 0, 255], [16, 16, 16]];
    let marker_x = ((t * 0.25).fract() * w as f64) as u32;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let bar = bars[((x * 8) / w.max(1)) as usize];
            let mut c = bar;
            if y > h * 3 / 4 {
                // grey ramp at the bottom
                let g = ((x * 255) / w.max(1)) as u8;
                c = [g, g, g];
            }
            if x.abs_diff(marker_x) < w / 100 + 1 && y < h / 8 {
                c = [0, 0, 0];
            }
            data[i..i + 3].copy_from_slice(&c);
            data[i + 3] = 255;
        }
    }
    Rgba { width: w, height: h, data }
}

/// Where the microphone samples come from.
#[derive(Clone, Debug)]
pub enum MicSource {
    Silence,
    /// A sine tone (Hz, amplitude 0..1).
    Tone(f64, f64),
    /// A WAV file, looped.
    Wav(PathBuf),
    /// Samples pushed by the host (microphone thread).
    Live,
}

impl Default for MicSource {
    fn default() -> Self {
        MicSource::Silence
    }
}

/// Microphone state: a mono source resampled to whatever rate the peripheral asks for.
pub struct Microphone {
    pub source: MicSource,
    wav: Vec<i16>,
    wav_rate: u32,
    pub noise: f64,
    pub error: Option<String>,
    /// Live samples from the host at `live_rate`.
    pub live: Arc<Mutex<std::collections::VecDeque<i16>>>,
    pub live_rate: u32,
    /// Peak level of the last delivered samples (0..1) for the meter.
    pub level: f32,
    seed: u32,
}

impl Microphone {
    pub fn new(source: MicSource) -> Microphone {
        let mut m = Microphone {
            source: MicSource::Silence,
            wav: Vec::new(),
            wav_rate: 16_000,
            noise: 0.0,
            error: None,
            live: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            live_rate: 48_000,
            level: 0.0,
            seed: 0x1234_5678,
        };
        m.set_source(source);
        m
    }

    pub fn set_source(&mut self, source: MicSource) {
        self.error = None;
        self.wav.clear();
        if let MicSource::Wav(p) = &source {
            match hound::WavReader::open(p) {
                Ok(mut r) => {
                    let spec = r.spec();
                    self.wav_rate = spec.sample_rate;
                    let ch = spec.channels.max(1) as usize;
                    let samples: Vec<i16> = match spec.sample_format {
                        hound::SampleFormat::Int => r.samples::<i32>().flatten().map(|s| (s >> (spec.bits_per_sample.saturating_sub(16))) as i16).collect(),
                        hound::SampleFormat::Float => r.samples::<f32>().flatten().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect(),
                    };
                    // mix down to mono
                    self.wav = samples.chunks(ch).map(|c| (c.iter().map(|s| *s as i32).sum::<i32>() / ch as i32) as i16).collect();
                    if self.wav.is_empty() {
                        self.error = Some(format!("{}: no samples", p.display()));
                    }
                }
                Err(e) => self.error = Some(format!("{}: {e}", p.display())),
            }
        }
        self.source = source;
    }

    /// Fills `buf` with interleaved samples of `channels` at `rate` starting at `now`.
    pub fn samples(&mut self, now: Ticks, rate: u32, channels: u32, buf: &mut [i16]) {
        let ch = channels.max(1) as usize;
        let n = buf.len() / ch;
        let t0 = now as f64 / TICKS_PER_SEC as f64;
        let mut peak = 0i32;
        for i in 0..n {
            let t = t0 + i as f64 / rate as f64;
            let mut v: f64 = match &self.source {
                MicSource::Silence => 0.0,
                MicSource::Tone(hz, amp) => (2.0 * std::f64::consts::PI * hz * t).sin() * amp,
                MicSource::Wav(_) => {
                    if self.wav.is_empty() {
                        0.0
                    } else {
                        let pos = ((t * self.wav_rate as f64) as u64 % self.wav.len() as u64) as usize;
                        self.wav[pos] as f64 / 32768.0
                    }
                }
                MicSource::Live => {
                    let mut l = self.live.lock().unwrap();
                    // consume at the ratio of the rates
                    let ratio = self.live_rate as f64 / rate as f64;
                    let take = if ratio >= 1.0 { ratio as usize } else { if (i as f64 * ratio).fract() < ratio { 1 } else { 0 } };
                    let mut s = 0.0;
                    for k in 0..take.max(1) {
                        if let Some(x) = l.pop_front() {
                            if k == 0 {
                                s = x as f64 / 32768.0;
                            }
                        }
                    }
                    s
                }
            };
            if self.noise > 0.0 {
                self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                v += ((self.seed >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 2.0 * self.noise;
            }
            let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
            peak = peak.max((s as i32).abs());
            for c in 0..ch {
                buf[i * ch + c] = s;
            }
        }
        if n > 0 {
            self.level = peak as f32 / 32768.0;
        }
    }
}

/// Speaker sink: the samples the target plays, as (start time, rate, channels, samples)
/// blocks for an audio thread or a scenario check.
#[derive(Default)]
pub struct Speaker {
    pub blocks: Vec<(Ticks, u32, u32, Vec<i16>)>,
    pub level: f32,
}

impl Speaker {
    pub fn push(&mut self, now: Ticks, rate: u32, channels: u32, samples: &[i16]) {
        let peak = samples.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0);
        self.level = peak as f32 / 32768.0;
        self.blocks.push((now, rate, channels, samples.to_vec()));
        if self.blocks.len() > 256 {
            self.blocks.drain(..128);
        }
    }
}
