//! Host audio: the speaker (the sample blocks the target played) on the default output
//! device, and the live microphone (the default input device) feeding the board's microphone
//! slot at the device rate. Both need the `audio` feature (cpal); without it the speaker is
//! silent and the live microphone source is unavailable.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Speaker playback: mono samples queued for the output stream, replicated to the device's
/// channels by the stream callback.
pub struct Speaker {
    buf: Arc<Mutex<VecDeque<f32>>>,
    rate: f64,
    pub volume: f32,
    pub muted: bool,
    pub available: bool,
    #[cfg(feature = "audio")]
    _stream: Option<cpal::Stream>,
}

impl Speaker {
    pub fn new() -> Speaker {
        let buf = Arc::new(Mutex::new(VecDeque::new()));
        #[cfg(feature = "audio")]
        let (stream, rate) = match open_output(buf.clone()) {
            Ok((s, r)) => (Some(s), r),
            Err(e) => {
                log::warn!("audio output not available: {e}");
                (None, 48_000.0)
            }
        };
        #[cfg(not(feature = "audio"))]
        let rate = 48_000.0;
        Speaker {
            buf,
            rate,
            volume: 0.5,
            muted: false,
            #[cfg(feature = "audio")]
            available: stream.is_some(),
            #[cfg(not(feature = "audio"))]
            available: false,
            #[cfg(feature = "audio")]
            _stream: stream,
        }
    }

    /// Queues a block the target played: the channels mixed to mono, resampled (nearest
    /// sample) from `rate` to the device rate. The queue keeps at most half a second; older
    /// samples are dropped when the target plays faster than the host.
    pub fn play(&mut self, rate: u32, channels: u32, samples: &[i16]) {
        if !self.available || rate == 0 {
            return;
        }
        let ch = channels.max(1) as usize;
        let n_in = samples.len() / ch;
        if n_in == 0 {
            return;
        }
        let n_out = (n_in as f64 * self.rate / rate as f64).round() as usize;
        let gain = if self.muted { 0.0 } else { self.volume };
        let Ok(mut b) = self.buf.lock() else { return };
        for j in 0..n_out {
            let i = ((j as f64 * rate as f64 / self.rate) as usize).min(n_in - 1);
            let frame = &samples[i * ch..i * ch + ch];
            let v = frame.iter().map(|s| *s as f32).sum::<f32>() / ch as f32 / 32768.0;
            b.push_back(v * gain);
        }
        let max = (self.rate * 0.5) as usize;
        if b.len() > max {
            let n = b.len() - max;
            b.drain(..n);
        }
    }

    /// Drops the queued samples (after a pause, a reset or a speed change).
    pub fn clear(&self) {
        if let Ok(mut b) = self.buf.lock() {
            b.clear();
        }
    }
}

impl Default for Speaker {
    fn default() -> Self {
        Self::new()
    }
}

/// The host microphone feeding the board's live sample slot; dropping it closes the stream.
pub struct LiveMic {
    /// Sample rate of the samples pushed into the slot.
    pub rate: u32,
    #[cfg(feature = "audio")]
    _stream: cpal::Stream,
}

/// Maximum number of queued live samples (two seconds at 48 kHz) when the target does not
/// consume them.
#[cfg(feature = "audio")]
const LIVE_MAX: usize = 96_000;

/// Opens the default input device and pushes its samples (mixed to mono, i16) into `dst`.
#[cfg(feature = "audio")]
pub fn open_live_mic(dst: Arc<Mutex<VecDeque<i16>>>) -> Result<LiveMic, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host.default_input_device().ok_or("no input device")?;
    let sup = dev.default_input_config().map_err(|e| e.to_string())?;
    let channels = sup.channels() as usize;
    let rate = sup.sample_rate();
    let fmt = sup.sample_format();
    let config: cpal::StreamConfig = sup.into();
    let err = |e| log::warn!("audio input stream error: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_input_stream(
            config,
            move |data: &[f32], _| capture(data, channels, &dst, |v| (v.clamp(-1.0, 1.0) * 32767.0) as i16),
            err,
            None,
        ),
        cpal::SampleFormat::I16 => dev.build_input_stream(config, move |data: &[i16], _| capture(data, channels, &dst, |v| v), err, None),
        cpal::SampleFormat::U16 => {
            dev.build_input_stream(config, move |data: &[u16], _| capture(data, channels, &dst, |v| (v as i32 - 32768) as i16), err, None)
        }
        f => return Err(format!("unsupported sample format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(LiveMic { rate, _stream: stream })
}

#[cfg(not(feature = "audio"))]
pub fn open_live_mic(_dst: Arc<Mutex<VecDeque<i16>>>) -> Result<LiveMic, String> {
    Err("built without the audio feature".into())
}

#[cfg(feature = "audio")]
fn capture<T: Copy>(data: &[T], channels: usize, dst: &Arc<Mutex<VecDeque<i16>>>, conv: impl Fn(T) -> i16) {
    let Ok(mut d) = dst.lock() else { return };
    for frame in data.chunks(channels.max(1)) {
        let sum: i32 = frame.iter().map(|s| conv(*s) as i32).sum();
        d.push_back((sum / frame.len().max(1) as i32) as i16);
    }
    if d.len() > LIVE_MAX {
        let n = d.len() - LIVE_MAX;
        d.drain(..n);
    }
}

#[cfg(feature = "audio")]
fn open_output(buf: Arc<Mutex<VecDeque<f32>>>) -> Result<(cpal::Stream, f64), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host.default_output_device().ok_or("no output device")?;
    let sup = dev.default_output_config().map_err(|e| e.to_string())?;
    let channels = sup.channels() as usize;
    let rate = sup.sample_rate() as f64;
    let fmt = sup.sample_format();
    let config: cpal::StreamConfig = sup.into();
    let err = |e| log::warn!("audio stream error: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => dev.build_output_stream(config, move |data: &mut [f32], _| fill(data, channels, &buf, |v| v), err, None),
        cpal::SampleFormat::I16 => dev.build_output_stream(
            config,
            move |data: &mut [i16], _| fill(data, channels, &buf, |v| (v.clamp(-1.0, 1.0) * 32767.0) as i16),
            err,
            None,
        ),
        cpal::SampleFormat::U16 => dev.build_output_stream(
            config,
            move |data: &mut [u16], _| fill(data, channels, &buf, |v| ((v.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16),
            err,
            None,
        ),
        f => return Err(format!("unsupported sample format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

#[cfg(feature = "audio")]
fn fill<T: Copy>(data: &mut [T], channels: usize, buf: &Arc<Mutex<VecDeque<f32>>>, conv: impl Fn(f32) -> T) {
    let Ok(mut b) = buf.lock() else { return };
    for frame in data.chunks_mut(channels.max(1)) {
        let v = b.pop_front().unwrap_or(0.0);
        for s in frame.iter_mut() {
            *s = conv(v);
        }
    }
}
