//! cpal output: device enumeration, stream construction and the real-time
//! callback. The callback never locks, allocates or blocks: it pops device-
//! rate stereo from a lock-free SPSC ring (rtrb), applies parameter updates
//! received on a second SPSC ring, runs the FX chain, de-clicks pause/seek,
//! taps the result for the spectrum analyser, and maps to the device's
//! channel layout and sample format.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::dsp::chain::ParamMsg;
use crate::dsp::{ChainParams, OutputChain};

/// Shared state between the decode thread, engine thread and callback for
/// one open output stream. Frame counters are in "ring frames".
pub(crate) struct Link {
    pub sample_rate: u32,
    /// Frames pushed into the ring (decoder).
    pub written: AtomicU64,
    /// Frames taken from the ring (callback), incl. ones dropped by a flush.
    pub consumed: AtomicU64,
    /// Producer bumps to request "fade out and drop everything queued".
    pub flush_epoch: AtomicU64,
    /// Callback acknowledges a completed flush.
    pub ack_epoch: AtomicU64,
    pub playing: AtomicBool,
    /// Device output latency in frames (from callback timestamps).
    pub latency_frames: AtomicU64,
    /// Callback invocations (watchdog).
    pub callbacks: AtomicU64,
    /// 0 = fine, 1 = stream must be rebuilt.
    pub error: AtomicU8,
}

impl Link {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Link {
            sample_rate,
            written: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            flush_epoch: AtomicU64::new(0),
            ack_epoch: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            latency_frames: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            error: AtomicU8::new(0),
        }
    }
}

/// Ring capacity in seconds of device-rate stereo.
const RING_SECONDS: f64 = 1.5;
const TAP_CAPACITY: usize = 16_384;
const PARAM_CAPACITY: usize = 256;
const BLOCK_FRAMES: usize = 1024;
const DECLICK_SECONDS: f32 = 0.008;

pub(crate) struct Output {
    _stream: cpal::Stream,
    pub link: Arc<Link>,
    pub params: rtrb::Producer<ParamMsg>,
    pub tap: rtrb::Consumer<f32>,
}

impl Output {
    pub(crate) fn send(&mut self, msg: ParamMsg) {
        let _ = self.params.push(msg);
    }
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| "Unknown device".into())
}

/// Names of the host's output devices.
pub(crate) fn device_names() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = match host.output_devices() {
        Ok(devs) => devs.map(|d| device_name(&d)).collect(),
        Err(_) => Vec::new(),
    };
    names.dedup();
    names
}

/// Open `name` (or the default device). `chain` builds the FX parameters
/// for the device rate. Returns the output plus the ring producer the
/// decoder feeds.
pub(crate) fn open_output(
    name: Option<&str>,
    chain: &dyn Fn(u32) -> ChainParams,
) -> Result<(Output, rtrb::Producer<f32>), String> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => host
            .output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| device_name(d) == n)
            .ok_or_else(|| format!("output device '{n}' not found"))?,
        None => host.default_output_device().ok_or_else(|| "no audio output device found".to_string())?,
    };
    let supported = device.default_output_config().map_err(|e| format!("output config: {e}"))?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels().max(1);
    let config = cpal::StreamConfig { channels, sample_rate, buffer_size: cpal::BufferSize::Default };

    let link = Arc::new(Link::new(sample_rate));
    let ring_len = ((sample_rate as f64 * RING_SECONDS) as usize).max(4096) * 2;
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring_len);
    let (params_tx, params_rx) = rtrb::RingBuffer::<ParamMsg>::new(PARAM_CAPACITY);
    let (tap_tx, tap_rx) = rtrb::RingBuffer::<f32>::new(TAP_CAPACITY);
    let state = CallbackState {
        ring: consumer,
        params: params_rx,
        tap: tap_tx,
        chain: OutputChain::new(sample_rate, &chain(sample_rate)),
        link: link.clone(),
        scratch: vec![0.0; BLOCK_FRAMES * 2],
        mono: vec![0.0; BLOCK_FRAMES],
        declick: 0.0,
        declick_step: 1.0 / (sample_rate as f32 * DECLICK_SECONDS).max(1.0),
        out_channels: channels as usize,
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, state, link.clone()),
        SampleFormat::F64 => build::<f64>(&device, &config, state, link.clone()),
        SampleFormat::I16 => build::<i16>(&device, &config, state, link.clone()),
        SampleFormat::I32 => build::<i32>(&device, &config, state, link.clone()),
        SampleFormat::U16 => build::<u16>(&device, &config, state, link.clone()),
        SampleFormat::U8 => build::<u8>(&device, &config, state, link.clone()),
        SampleFormat::I8 => build::<i8>(&device, &config, state, link.clone()),
        other => return Err(format!("unsupported device sample format {other:?}")),
    }
    .map_err(|e| format!("open stream: {e}"))?;
    stream.play().map_err(|e| format!("start stream: {e}"))?;
    Ok((Output { _stream: stream, link, params: params_tx, tap: tap_rx }, producer))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut state: CallbackState,
    link: Arc<Link>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], info: &cpal::OutputCallbackInfo| state.render(data, info),
        move |err: cpal::Error| {
            use cpal::ErrorKind as K;
            match err.kind() {
                K::Xrun | K::DeviceChanged | K::RealtimeDenied => {}
                _ => link.error.store(1, Ordering::Release),
            }
        },
        None,
    )
}

struct CallbackState {
    ring: rtrb::Consumer<f32>,
    params: rtrb::Consumer<ParamMsg>,
    tap: rtrb::Producer<f32>,
    chain: OutputChain,
    link: Arc<Link>,
    scratch: Vec<f32>,
    mono: Vec<f32>,
    /// De-click gain 0..1 (pause/seek fades).
    declick: f32,
    declick_step: f32,
    out_channels: usize,
}

impl CallbackState {
    fn render<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], info: &cpal::OutputCallbackInfo) {
        let link = &*self.link;
        link.callbacks.fetch_add(1, Ordering::Relaxed);
        let ts = info.timestamp();
        let latency = ts.playback.duration_since(ts.callback);
        link.latency_frames.store((latency.as_secs_f64() * link.sample_rate as f64) as u64, Ordering::Relaxed);
        while let Ok(msg) = self.params.pop() {
            self.chain.apply(msg);
        }

        let dc = self.out_channels.max(1);
        let total = data.len() / dc;
        let mut done = 0;
        while done < total {
            let n = (total - done).min(BLOCK_FRAMES);
            let out = &mut data[done * dc..(done + n) * dc];
            done += n;

            let flush_epoch = link.flush_epoch.load(Ordering::Acquire);
            let flushing = flush_epoch != link.ack_epoch.load(Ordering::Relaxed);
            let playing = link.playing.load(Ordering::Relaxed);
            let target = if playing && !flushing { 1.0 } else { 0.0 };

            if self.declick == 0.0 && target == 0.0 {
                if flushing {
                    let k = self.ring.slots();
                    if let Ok(chunk) = self.ring.read_chunk(k) {
                        chunk.commit_all();
                    }
                    link.consumed.fetch_add((k / 2) as u64, Ordering::AcqRel);
                    self.chain.reset();
                    link.ack_epoch.store(flush_epoch, Ordering::Release);
                }
                out.iter_mut().for_each(|s| *s = T::EQUILIBRIUM);
                continue;
            }

            // how many frames may still be taken (fading out stops consuming at 0)
            let mut take = n.min(self.ring.slots() / 2);
            if target == 0.0 {
                let left = (self.declick / self.declick_step).ceil() as usize;
                take = take.min(left.max(1));
            }
            let buf = &mut self.scratch[..n * 2];
            if take > 0
                && let Ok(chunk) = self.ring.read_chunk(take * 2)
            {
                let (a, b) = chunk.as_slices();
                buf[..a.len()].copy_from_slice(a);
                buf[a.len()..a.len() + b.len()].copy_from_slice(b);
                chunk.commit_all();
            }
            buf[take * 2..].fill(0.0);
            link.consumed.fetch_add(take as u64, Ordering::AcqRel);

            self.chain.process(buf);

            for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                if self.declick != target {
                    self.declick = if target > self.declick {
                        (self.declick + self.declick_step).min(1.0)
                    } else {
                        (self.declick - self.declick_step).max(0.0)
                    };
                }
                if target == 0.0 && i >= take {
                    self.declick = 0.0;
                }
                frame[0] *= self.declick;
                frame[1] *= self.declick;
                self.mono[i] = (frame[0] + frame[1]) * 0.5;
            }
            let _ = self.tap.push_partial_slice(&self.mono[..n]);

            if dc == 1 {
                for (o, m) in out.iter_mut().zip(&self.mono[..n]) {
                    *o = T::from_sample(*m);
                }
            } else {
                for (o, f) in out.chunks_exact_mut(dc).zip(buf.as_chunks::<2>().0) {
                    o[0] = T::from_sample(f[0]);
                    o[1] = T::from_sample(f[1]);
                    for x in &mut o[2..] {
                        *x = T::EQUILIBRIUM;
                    }
                }
            }
        }
    }
}
