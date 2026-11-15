//! Decoding: symphonia formats/codecs (+ pure-Rust Opus for WebM/Ogg Opus),
//! conversion to the engine's internal stereo f32, accurate seeking, and the
//! decode thread that feeds the output ring through the rate stages
//! (time-stretch → resample to the device rate).

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use symphonia::core::audio::{Channels, SampleBuffer};
use symphonia::core::codecs::{CodecParameters, CODEC_TYPE_NULL, CODEC_TYPE_OPUS, Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey};
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use crate::dsp::spatial::{HeadRenderer, SURROUND_GAIN};
use crate::dsp::{SincResampler, TimeStretch, parse_replay_gain_number};
use crate::output::Link;
use crate::{AudioError, Event, TrackSource};

/// ReplayGain tags found in the file.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGainTags {
    pub track_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_gain: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGainTags {
    fn absorb(&mut self, rev: &MetadataRevision) {
        for tag in rev.tags() {
            let key = match tag.std_key {
                Some(StandardTagKey::ReplayGainTrackGain) => "REPLAYGAIN_TRACK_GAIN".to_string(),
                Some(StandardTagKey::ReplayGainTrackPeak) => "REPLAYGAIN_TRACK_PEAK".to_string(),
                Some(StandardTagKey::ReplayGainAlbumGain) => "REPLAYGAIN_ALBUM_GAIN".to_string(),
                Some(StandardTagKey::ReplayGainAlbumPeak) => "REPLAYGAIN_ALBUM_PEAK".to_string(),
                _ => tag.key.to_ascii_uppercase(),
            };
            let Some(v) = parse_replay_gain_number(&tag.value.to_string()) else { continue };
            match key.as_str() {
                "REPLAYGAIN_TRACK_GAIN" => self.track_gain = Some(v),
                "REPLAYGAIN_TRACK_PEAK" => self.track_peak = Some(v),
                "REPLAYGAIN_ALBUM_GAIN" => self.album_gain = Some(v),
                "REPLAYGAIN_ALBUM_PEAK" => self.album_peak = Some(v),
                _ => {}
            }
        }
    }
}

/// What was opened.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: Option<u32>,
    pub duration_secs: Option<f64>,
    pub replay_gain: ReplayGainTags,
}

// ---------------------------------------------------------------------------
// Opus (pure Rust, `rusty-opus`)
// ---------------------------------------------------------------------------

struct OpusState {
    dec: rusty_opus::OpusDecoder,
    channels: usize,
    pre_skip_left: usize,
    gain: f32,
    pcm: Vec<f32>,
    /// Format reader trims (Ogg with gapless) instead of us.
    packet_trims: bool,
    pre_skip: usize,
}

/// (channels, pre_skip, output gain Q7.8 dB, mapping family) from `OpusHead`.
pub(crate) fn parse_opus_head(extra: &[u8]) -> Option<(usize, usize, f32, u8)> {
    if extra.len() < 19 || &extra[..8] != b"OpusHead" {
        return None;
    }
    let channels = extra[9] as usize;
    let pre_skip = u16::from_le_bytes([extra[10], extra[11]]) as usize;
    let gain_q8 = i16::from_le_bytes([extra[16], extra[17]]);
    Some((channels, pre_skip, gain_q8 as f32 / 256.0, extra[18]))
}

impl OpusState {
    fn new(params: &CodecParameters) -> Result<Self, AudioError> {
        let head = params.extra_data.as_deref().and_then(parse_opus_head);
        let channels = head
            .map(|h| h.0)
            .or_else(|| params.channels.map(|c| c.count()))
            .unwrap_or(2);
        if let Some((_, _, _, family)) = head
            && family != 0
            && channels > 2
        {
            return Err(AudioError::UnsupportedFormat(format!("multichannel Opus (mapping family {family})")));
        }
        if !(1..=2).contains(&channels) {
            return Err(AudioError::UnsupportedFormat(format!("Opus with {channels} channels")));
        }
        let dec = rusty_opus::OpusDecoder::new(48_000, channels)
            .map_err(|e| AudioError::Decode(format!("opus init: {e:?}")))?;
        let pre_skip = head.map_or(312, |h| h.1);
        let packet_trims = params.delay.is_some();
        Ok(OpusState {
            dec,
            channels,
            pre_skip_left: if packet_trims { 0 } else { pre_skip },
            gain: 10f32.powf(head.map_or(0.0, |h| h.2) / 20.0),
            pcm: vec![0.0; 5760 * channels],
            packet_trims,
            pre_skip,
        })
    }

    fn reset(&mut self, at_start: bool) {
        if let Ok(d) = rusty_opus::OpusDecoder::new(48_000, self.channels) {
            self.dec = d;
        }
        self.pre_skip_left = if at_start && !self.packet_trims { self.pre_skip } else { 0 };
    }

    /// Decode one packet into interleaved samples (`self.channels`).
    fn decode(&mut self, data: &[u8], trim_start: usize, trim_end: usize, out: &mut Vec<f32>) -> Result<usize, String> {
        let n = self.dec.decode(data, 5760, &mut self.pcm).map_err(|e| format!("{e:?}"))?;
        let ch = self.channels;
        let mut start = if self.packet_trims { trim_start } else { 0 };
        let mut end = n.saturating_sub(if self.packet_trims { trim_end } else { 0 });
        if self.pre_skip_left > 0 {
            let skip = self.pre_skip_left.min(end.saturating_sub(start));
            start += skip;
            self.pre_skip_left -= skip;
        }
        end = end.max(start);
        out.extend(self.pcm[start * ch..end * ch].iter().map(|s| s * self.gain));
        Ok(end - start)
    }
}

enum Dec {
    Sym(Box<dyn Decoder>),
    Opus(Box<OpusState>),
}

// ---------------------------------------------------------------------------
// DecodeSession
// ---------------------------------------------------------------------------

/// Per-channel placement for the stereo fold-down: (azimuth°, gain L, gain R).
fn placement(ch: Channels, has_sides: bool) -> (f32, f32, f32) {
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let rear = if has_sides { 150.0 } else { 110.0 };
    match ch {
        Channels::FRONT_LEFT => (-30.0, 1.0, 0.0),
        Channels::FRONT_RIGHT => (30.0, 0.0, 1.0),
        Channels::FRONT_CENTRE => (0.0, H, H),
        Channels::LFE1 | Channels::LFE2 => (f32::NAN, 0.0, 0.0),
        Channels::REAR_LEFT => (-rear, H, 0.0),
        Channels::REAR_RIGHT => (rear, 0.0, H),
        Channels::FRONT_LEFT_CENTRE => (-15.0, 0.85, 0.35),
        Channels::FRONT_RIGHT_CENTRE => (15.0, 0.35, 0.85),
        Channels::REAR_CENTRE => (180.0, 0.5, 0.5),
        Channels::SIDE_LEFT => (-90.0, H, 0.0),
        Channels::SIDE_RIGHT => (90.0, 0.0, H),
        Channels::FRONT_LEFT_WIDE => (-60.0, 1.0, 0.0),
        Channels::FRONT_RIGHT_WIDE => (60.0, 0.0, 1.0),
        _ => (0.0, 0.5, 0.5),
    }
}

/// Folds N interleaved channels to stereo.
struct Downmix {
    channels: usize,
    gains: Vec<(f32, f32)>,
    azimuths: Vec<f32>,
    head: Option<HeadRenderer>,
    surround: bool,
    sample_rate: u32,
}

impl Downmix {
    fn new(spec_channels: Channels, count: usize, sample_rate: u32, surround: bool) -> Self {
        let has_sides = spec_channels.contains(Channels::SIDE_LEFT);
        let mut gains = Vec::with_capacity(count);
        let mut azimuths = Vec::with_capacity(count);
        let mut it = spec_channels.iter();
        for i in 0..count {
            let (az, l, r) = match it.next() {
                Some(c) => placement(c, has_sides),
                None => (0.0, if i % 2 == 0 { 1.0 } else { 0.0 }, if i % 2 == 1 { 1.0 } else { 0.0 }),
            };
            azimuths.push(az);
            gains.push((l, r));
        }
        // keep the fold-down from clipping (centre + one surround on top of a front)
        let norm = 1.0 / (1.0 + std::f32::consts::FRAC_1_SQRT_2);
        if count > 2 {
            gains.iter_mut().for_each(|g| {
                g.0 *= norm;
                g.1 *= norm;
            });
        }
        let mut d = Downmix { channels: count, gains, azimuths, head: None, surround: false, sample_rate };
        d.set_surround(surround);
        d
    }

    fn set_surround(&mut self, on: bool) {
        self.surround = on && self.channels > 2;
        if self.surround && self.head.is_none() {
            let az: Vec<f32> = self.azimuths.iter().copied().take(crate::dsp::spatial::MAX_SOURCES).collect();
            self.head = Some(HeadRenderer::new(self.sample_rate, &az));
        }
    }

    fn reset(&mut self) {
        if let Some(h) = &mut self.head {
            h.clear();
        }
    }

    /// Append `frames` stereo frames from interleaved `src`.
    fn run(&mut self, src: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        match ch {
            0 => {}
            1 => {
                for &s in src {
                    out.push(s);
                    out.push(s);
                }
            }
            2 => out.extend_from_slice(src),
            _ => {
                if self.surround
                    && let Some(head) = &mut self.head
                {
                    for f in src.chunks_exact(ch) {
                        let (l, r) = head.render(f);
                        out.push(l * SURROUND_GAIN);
                        out.push(r * SURROUND_GAIN);
                    }
                } else {
                    for f in src.chunks_exact(ch) {
                        let (mut l, mut r) = (0.0, 0.0);
                        for (s, g) in f.iter().zip(&self.gains) {
                            l += s * g.0;
                            r += s * g.1;
                        }
                        out.push(l);
                        out.push(r);
                    }
                }
            }
        }
    }
}

/// An opened track: format reader + decoder, producing stereo f32 at the
/// source sample rate.
pub struct DecodeSession {
    format: Box<dyn FormatReader>,
    track_id: u32,
    params: CodecParameters,
    dec: Dec,
    pub info: SourceInfo,
    time_base: Option<TimeBase>,
    sample_buf: Option<SampleBuffer<f32>>,
    sample_spec: Option<symphonia::core::audio::SignalSpec>,
    native: Vec<f32>,
    downmix: Option<Downmix>,
    downmix_key: (u32, usize),
    surround: bool,
    discard_until: Option<f64>,
    errors_in_row: u32,
    seek_mode: SeekMode,
    /// Next output frame time when the packet has no usable timestamp.
    running_secs: f64,
    /// How to reopen the source when the demuxer can't seek (no index).
    origin: Option<(TrackSource, Arc<AtomicBool>)>,
}

fn hint_for(ext_or_mime: Option<&str>) -> Hint {
    let mut h = Hint::new();
    if let Some(s) = ext_or_mime {
        let s = s.to_ascii_lowercase();
        let ext = if s.contains("webm") {
            "webm"
        } else if s.contains("mp4") || s.contains("m4a") || s.contains("aac") {
            "m4a"
        } else if s.contains("mpeg") || s.contains("mp3") {
            "mp3"
        } else if s.contains("ogg") || s.contains("opus") {
            "ogg"
        } else if s.contains("flac") {
            "flac"
        } else if s.contains("wav") {
            "wav"
        } else {
            s.as_str()
        };
        h.with_extension(ext);
    }
    h
}

/// `mime=audio%2Fwebm` in googlevideo URLs.
fn mime_from_url(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.strip_prefix("mime=")).map(|m| m.replace("%2F", "/").replace("%2f", "/"))
}

impl DecodeSession {
    /// Open a local file or remote URL. `cancel` aborts blocked network reads.
    pub fn open(source: &TrackSource, cancel: Arc<AtomicBool>) -> Result<Self, AudioError> {
        let mut s = match source {
            TrackSource::Local { path } => {
                let file = File::open(path)?;
                let ext = Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_owned);
                Self::open_media(Box::new(file), hint_for(ext.as_deref()), SeekMode::Accurate)?
            }
            TrackSource::Remote { url, headers } => {
                let src = crate::http_source::HttpByteSource::open_cancellable(url.clone(), headers.clone(), cancel.clone())
                    .map_err(|e| AudioError::Other(format!("stream: {e}")))?;
                let mime = src.content_type().or_else(|| mime_from_url(url));
                Self::open_media(Box::new(src), hint_for(mime.as_deref()), SeekMode::Coarse)?
            }
        };
        s.origin = Some((source.clone(), cancel));
        Ok(s)
    }

    /// Open any symphonia `MediaSource`.
    pub fn open_media(source: Box<dyn MediaSource>, hint: Hint, seek_mode: SeekMode) -> Result<Self, AudioError> {
        let mss = MediaSourceStream::new(source, Default::default());
        let fmt_opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let mut probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &MetadataOptions::default())
            .map_err(|e| match e {
                SymError::Unsupported(m) => AudioError::UnsupportedFormat(m.to_string()),
                SymError::IoError(io) => AudioError::Io(io),
                other => AudioError::Decode(other.to_string()),
            })?;
        let mut format = probed.format;
        let track = format
            .default_track()
            .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .or_else(|| format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL))
            .ok_or_else(|| AudioError::UnsupportedFormat("no audio track".into()))?
            .clone();
        let params = track.codec_params.clone();
        let dec = Self::make_decoder(&params)?;
        let codec = if params.codec == CODEC_TYPE_OPUS {
            "opus".to_string()
        } else {
            symphonia::default::get_codecs()
                .get_codec(params.codec)
                .map(|d| d.short_name.to_string())
                .unwrap_or_else(|| format!("{}", params.codec))
        };
        let sample_rate = match &dec {
            Dec::Opus(_) => 48_000,
            Dec::Sym(_) => params.sample_rate.unwrap_or(44_100),
        };
        let channels = match &dec {
            Dec::Opus(o) => o.channels as u16,
            Dec::Sym(_) => params.channels.map_or(2, |c| c.count() as u16),
        };
        let time_base = params.time_base.or_else(|| params.sample_rate.map(|sr| TimeBase::new(1, sr)));
        let duration_secs = match (params.n_frames, time_base) {
            (Some(n), Some(tb)) if n > 0 => {
                let t = tb.calc_time(n);
                Some(t.seconds as f64 + t.frac)
            }
            _ => None,
        };
        let mut rg = ReplayGainTags::default();
        if let Some(m) = probed.metadata.get()
            && let Some(rev) = m.current()
        {
            rg.absorb(rev);
        }
        if let Some(rev) = format.metadata().current() {
            rg.absorb(rev);
        }
        let info = SourceInfo {
            codec,
            sample_rate,
            channels,
            bits_per_sample: params.bits_per_sample,
            duration_secs,
            replay_gain: rg,
        };
        Ok(DecodeSession {
            format,
            track_id: track.id,
            params,
            dec,
            info,
            time_base,
            sample_buf: None,
            sample_spec: None,
            native: Vec::new(),
            downmix: None,
            downmix_key: (0, 0),
            surround: false,
            discard_until: None,
            errors_in_row: 0,
            seek_mode,
            running_secs: 0.0,
            origin: None,
        })
    }

    fn make_decoder(params: &CodecParameters) -> Result<Dec, AudioError> {
        if params.codec == CODEC_TYPE_OPUS {
            return Ok(Dec::Opus(Box::new(OpusState::new(params)?)));
        }
        symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .map(Dec::Sym)
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))
    }

    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    /// Render multichannel sources to two ears (head model) instead of folding down.
    pub fn set_surround(&mut self, on: bool) {
        self.surround = on;
        if let Some(d) = &mut self.downmix {
            d.set_surround(on);
        }
    }

    fn packet_secs(&self, ts: u64) -> Option<f64> {
        let tb = self.time_base?;
        let t = tb.calc_time(ts);
        Some(t.seconds as f64 + t.frac)
    }

    /// Decode the next packet, appending stereo interleaved f32 to `out`.
    /// Returns `Ok(false)` at end of stream.
    pub fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool, AudioError> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(SymError::ResetRequired) => {
                    self.dec = Self::make_decoder(&self.params)?;
                    continue;
                }
                Err(SymError::IoError(e)) => return Err(AudioError::Io(e)),
                Err(e) => {
                    self.errors_in_row += 1;
                    if self.errors_in_row > 64 {
                        return Err(AudioError::Decode(e.to_string()));
                    }
                    continue;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let t0 = self.packet_secs(packet.ts());
            self.native.clear();
            let (frames, spec_channels, count) = match &mut self.dec {
                Dec::Opus(o) => {
                    match o.decode(&packet.data, packet.trim_start as usize, packet.trim_end as usize, &mut self.native) {
                        Ok(n) => {
                            let c = o.channels;
                            let layout = if c == 1 { Channels::FRONT_LEFT } else { Channels::FRONT_LEFT | Channels::FRONT_RIGHT };
                            (n, layout, c)
                        }
                        Err(e) => {
                            self.errors_in_row += 1;
                            if self.errors_in_row > 64 {
                                return Err(AudioError::Decode(format!("opus: {e}")));
                            }
                            continue;
                        }
                    }
                }
                Dec::Sym(d) => match d.decode(&packet) {
                    Ok(buf) => {
                        let spec = *buf.spec();
                        let cap = buf.capacity() as u64;
                        let n = buf.frames();
                        if self.sample_buf.as_ref().is_none_or(|b| b.capacity() < n) || self.sample_spec != Some(spec) {
                            self.sample_buf = Some(SampleBuffer::new(cap.max(n as u64), spec));
                            self.sample_spec = Some(spec);
                        }
                        let sb = self.sample_buf.as_mut().expect("sample buffer");
                        sb.copy_interleaved_ref(buf);
                        self.native.extend_from_slice(sb.samples());
                        (n, spec.channels, spec.channels.count())
                    }
                    Err(SymError::DecodeError(e)) => {
                        self.errors_in_row += 1;
                        if self.errors_in_row > 64 {
                            return Err(AudioError::Decode(e.to_string()));
                        }
                        continue;
                    }
                    Err(SymError::ResetRequired) => {
                        self.dec = Self::make_decoder(&self.params)?;
                        continue;
                    }
                    Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                    Err(e) => return Err(AudioError::Decode(e.to_string())),
                },
            };
            self.errors_in_row = 0;
            if count == 0 || frames == 0 {
                continue;
            }
            let sr = self.info.sample_rate as f64;
            let start_secs = t0.unwrap_or(self.running_secs);
            self.running_secs = start_secs + frames as f64 / sr;
            let mut skip = 0usize;
            if let Some(target) = self.discard_until {
                skip = (((target - start_secs) * sr).round().max(0.0) as usize).min(frames);
                if skip < frames {
                    self.discard_until = None;
                }
            }
            if skip >= frames {
                continue;
            }
            let key = (spec_channels.bits(), count);
            if self.downmix.is_none() || self.downmix_key != key {
                self.downmix = Some(Downmix::new(spec_channels, count, self.info.sample_rate, self.surround));
                self.downmix_key = key;
            }
            let dm = self.downmix.as_mut().expect("downmix");
            dm.run(&self.native[skip * count..frames * count], out);
            return Ok(true);
        }
    }

    /// Seek to `secs`; decoding resumes exactly there (pre-target audio is
    /// decoded and dropped). Returns the position actually reached.
    pub fn seek(&mut self, secs: f64) -> Result<f64, AudioError> {
        match self.seek_once(secs) {
            Ok(t) => Ok(t),
            Err(e) => {
                // e.g. WebM without Cues once at EOF: start over and seek forward
                let Some((src, cancel)) = self.origin.clone() else { return Err(e) };
                let mut fresh = Self::open(&src, cancel)?;
                fresh.set_surround(self.surround);
                *self = fresh;
                self.seek_once(secs)
            }
        }
    }

    fn seek_once(&mut self, secs: f64) -> Result<f64, AudioError> {
        let secs = secs.max(0.0);
        let opus = matches!(self.dec, Dec::Opus(_));
        // Opus needs ~80 ms of pre-roll to converge after a jump
        let land = if opus { (secs - 0.08).max(0.0) } else { secs };
        let res = self
            .format
            .seek(self.seek_mode, SeekTo::Time { time: Time::from(land), track_id: Some(self.track_id) });
        let res = match res {
            Err(_) if self.seek_mode == SeekMode::Coarse => {
                self.format.seek(SeekMode::Accurate, SeekTo::Time { time: Time::from(land), track_id: Some(self.track_id) })
            }
            r => r,
        };
        let seeked = res.map_err(|e| AudioError::Decode(format!("seek: {e}")))?;
        match &mut self.dec {
            Dec::Sym(d) => d.reset(),
            Dec::Opus(o) => o.reset(secs <= 0.0),
        }
        if let Some(d) = &mut self.downmix {
            d.reset();
        }
        let actual = self.packet_secs(seeked.actual_ts).unwrap_or(secs);
        if actual > secs + 0.25 {
            // demuxer could only scan forward (e.g. WebM without Cues) and overshot
            return Err(AudioError::Decode(format!("seek overshot: wanted {secs:.3}s, got {actual:.3}s")));
        }
        self.running_secs = actual;
        if self.time_base.is_some() && actual < secs {
            self.discard_until = Some(secs);
            Ok(secs)
        } else {
            self.discard_until = None;
            Ok(actual)
        }
    }
}

// ---------------------------------------------------------------------------
// Rate stages: time-stretch (tempo/pitch) → resample (pitch·rate → device)
// ---------------------------------------------------------------------------

/// Speed/pitch as the Android player applies them (spec §4.7): `tempo` =
/// speed; `pitch` = 2^(semitones/12) when preserving pitch, else = speed
/// (varispeed, "like a record").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateParams {
    pub tempo: f64,
    pub pitch: f64,
}

impl Default for RateParams {
    fn default() -> Self {
        RateParams { tempo: 1.0, pitch: 1.0 }
    }
}

impl RateParams {
    pub fn new(speed: f32, semitones: f32, preserve_pitch: bool) -> Self {
        let tempo = (speed as f64).clamp(0.1, 8.0);
        let pitch = if preserve_pitch { 2f64.powf((semitones as f64).clamp(-24.0, 24.0) / 12.0) } else { tempo };
        RateParams { tempo, pitch }
    }
}

/// Stereo source-rate audio → stereo device-rate audio at `tempo`/`pitch`.
pub struct RatePipeline {
    stretch: TimeStretch,
    resampler: SincResampler,
    params: RateParams,
    tmp: Vec<f32>,
}

impl RatePipeline {
    pub fn new(src_rate: u32, out_rate: u32, params: RateParams) -> Self {
        let mut p = RatePipeline {
            stretch: TimeStretch::new(src_rate, 2),
            resampler: SincResampler::new(src_rate, out_rate),
            params,
            tmp: Vec::new(),
        };
        p.set_params(params);
        p
    }

    pub fn set_params(&mut self, p: RateParams) {
        self.params = p;
        self.stretch.set_tempo(p.tempo / p.pitch);
        self.resampler.set_speed(p.pitch);
    }

    pub fn params(&self) -> RateParams {
        self.params
    }

    pub fn reset(&mut self) {
        self.stretch.reset();
        self.resampler.reset();
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.stretch.is_passthrough() && self.stretch.tempo() == 1.0 {
            self.resampler.process(input, 2, out, false);
            return;
        }
        self.tmp.clear();
        self.stretch.process(input, &mut self.tmp);
        self.resampler.process(&self.tmp, 2, out, false);
    }

    pub fn flush(&mut self, out: &mut Vec<f32>) {
        self.tmp.clear();
        self.stretch.flush(&mut self.tmp);
        self.resampler.process(&self.tmp, 2, out, true);
    }
}

// ---------------------------------------------------------------------------
// Decode thread
// ---------------------------------------------------------------------------

/// Ring index → media time anchor (piecewise linear in `speed`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Marker {
    pub index: u64,
    pub media: f64,
    pub speed: f64,
}

/// Decoder → engine state for one track.
#[derive(Default)]
pub(crate) struct TrackShared {
    pub opened: AtomicBool,
    pub failed: AtomicBool,
    pub eof: AtomicBool,
    pub eof_index: AtomicU64,
    pub seek_done_gen: AtomicU64,
    pub markers: Mutex<VecDeque<Marker>>,
    pub info: Mutex<Option<SourceInfo>>,
}

pub(crate) enum DecCmd {
    Seek { secs: f64, generation: u64 },
    Rate(RateParams),
    Surround(bool),
    Rebind { producer: rtrb::Producer<f32>, link: Arc<Link>, pos: f64, generation: u64 },
    Stop,
}

pub(crate) struct DecoderStart {
    pub source: TrackSource,
    pub start_secs: f64,
    pub rate: RateParams,
    pub surround: bool,
    pub producer: rtrb::Producer<f32>,
    pub link: Arc<Link>,
    pub events: Sender<Event>,
    pub shared: Arc<TrackShared>,
    pub cancel: Arc<AtomicBool>,
    pub ctl: Receiver<DecCmd>,
}

/// How much device-rate audio the decoder keeps queued ahead of the device.
const TARGET_FILL_SECS: f64 = 0.25;

/// Ask the callback to fade out and drop everything queued, and wait.
pub(crate) fn flush_ring(link: &Link, cancel: &AtomicBool) {
    let epoch = link.flush_epoch.fetch_add(1, Ordering::AcqRel) + 1;
    let deadline = Instant::now() + Duration::from_millis(400);
    while link.ack_epoch.load(Ordering::Acquire) < epoch {
        if Instant::now() > deadline || cancel.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn push_marker(shared: &TrackShared, m: Marker, clear: bool) {
    let mut q = shared.markers.lock().unwrap_or_else(|e| e.into_inner());
    if clear {
        q.clear();
    }
    q.push_back(m);
}

struct Feeder {
    producer: rtrb::Producer<f32>,
    link: Arc<Link>,
    pending: Vec<f32>,
    pending_off: usize,
}

impl Feeder {
    fn pending_frames(&self) -> usize {
        (self.pending.len() - self.pending_off) / 2
    }

    /// Push as much pending audio as fits (whole frames only).
    fn pump(&mut self) {
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
            return;
        }
        let free = self.producer.slots() & !1;
        let n = free.min(self.pending.len() - self.pending_off);
        if n == 0 {
            return;
        }
        let (pushed, _) = self.producer.push_partial_slice(&self.pending[self.pending_off..self.pending_off + n]);
        let k = pushed.len();
        self.pending_off += k;
        self.link.written.fetch_add((k / 2) as u64, Ordering::AcqRel);
        if self.pending_off >= self.pending.len() {
            self.pending.clear();
            self.pending_off = 0;
        }
    }

    fn queued_frames(&self) -> u64 {
        let w = self.link.written.load(Ordering::Acquire);
        let c = self.link.consumed.load(Ordering::Acquire);
        w.saturating_sub(c)
    }
}

/// Decode thread body. Returns the ring producer (and its link) so the
/// engine can reuse it for the next track.
pub(crate) fn run_decoder(s: DecoderStart) -> (rtrb::Producer<f32>, Arc<Link>) {
    let DecoderStart { source, start_secs, mut rate, surround, producer, link, events, shared, cancel, ctl } = s;
    let mut ctl = Ctl { rx: ctl, stash: None };
    let mut feeder = Feeder { producer, link, pending: Vec::new(), pending_off: 0 };

    let mut session = match DecodeSession::open(&source, cancel.clone()) {
        Ok(s) => s,
        Err(e) => {
            if !cancel.load(Ordering::Relaxed) {
                let _ = events.send(Event::Error(format!("could not open track: {e}")));
            }
            shared.failed.store(true, Ordering::Release);
            return adopt_rebind(feeder, &mut ctl);
        }
    };
    session.set_surround(surround);
    let info = session.info.clone();
    let _ = events.send(Event::TrackInfo {
        codec: info.codec.clone(),
        sample_rate: info.sample_rate,
        channels: info.channels,
        bits_per_sample: info.bits_per_sample,
    });
    *shared.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(info.clone());

    let mut out_rate = feeder.link.sample_rate;
    let mut pipe = RatePipeline::new(info.sample_rate, out_rate, rate);
    let mut media_start = 0.0;
    if start_secs > 0.0 {
        media_start = session.seek(start_secs).unwrap_or(0.0);
    }
    flush_ring(&feeder.link, &cancel);
    push_marker(
        &shared,
        Marker { index: feeder.link.written.load(Ordering::Acquire), media: media_start, speed: rate.tempo },
        true,
    );
    shared.opened.store(true, Ordering::Release);

    let mut decoded = Vec::<f32>::new();
    let mut eof_pending = false;
    let mut at_eof = false;
    let mut last_marker = Marker { index: feeder.link.written.load(Ordering::Acquire), media: media_start, speed: rate.tempo };

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // --- commands (coalesce seeks) ---
        let mut seek: Option<(f64, u64)> = None;
        let mut stop = false;
        let mut new_rate = None;
        let mut rebind = None;
        while let Some(cmd) = ctl.try_recv() {
            match cmd {
                DecCmd::Seek { secs, generation } => seek = Some((secs, generation)),
                DecCmd::Rate(p) => new_rate = Some(p),
                DecCmd::Surround(on) => session.set_surround(on),
                DecCmd::Rebind { producer, link, pos, generation } => rebind = Some((producer, link, pos, generation)),
                DecCmd::Stop => stop = true,
            }
        }
        if stop {
            if let Some((p, l, _, _)) = rebind {
                feeder.producer = p;
                feeder.link = l;
            }
            break;
        }
        if let Some(p) = new_rate
            && p != rate
        {
            rate = p;
            pipe.set_params(p);
            if rebind.is_none() && seek.is_none() {
                let idx = feeder.link.written.load(Ordering::Acquire) + feeder.pending_frames() as u64;
                let media = last_marker.media + (idx.saturating_sub(last_marker.index)) as f64 / out_rate as f64 * last_marker.speed;
                last_marker = Marker { index: idx, media, speed: p.tempo };
                push_marker(&shared, last_marker, false);
            }
        }
        if let Some((producer, link, pos, generation)) = rebind {
            feeder.producer = producer;
            feeder.link = link;
            feeder.pending.clear();
            feeder.pending_off = 0;
            out_rate = feeder.link.sample_rate;
            pipe = RatePipeline::new(info.sample_rate, out_rate, rate);
            let actual = session.seek(pos).unwrap_or(pos);
            at_eof = false;
            eof_pending = false;
            shared.eof.store(false, Ordering::Release);
            last_marker = Marker { index: feeder.link.written.load(Ordering::Acquire), media: actual, speed: rate.tempo };
            push_marker(&shared, last_marker, true);
            shared.seek_done_gen.fetch_max(generation, Ordering::AcqRel);
        }
        if let Some((secs, generation)) = seek {
            let actual = match session.seek(secs) {
                Ok(a) => a,
                Err(_) => {
                    // unseekable stream: keep playing where we are
                    shared.seek_done_gen.fetch_max(generation, Ordering::AcqRel);
                    continue;
                }
            };
            pipe.reset();
            feeder.pending.clear();
            feeder.pending_off = 0;
            at_eof = false;
            eof_pending = false;
            shared.eof.store(false, Ordering::Release);
            flush_ring(&feeder.link, &cancel);
            last_marker = Marker { index: feeder.link.written.load(Ordering::Acquire), media: actual, speed: rate.tempo };
            push_marker(&shared, last_marker, true);
            shared.seek_done_gen.fetch_max(generation, Ordering::AcqRel);
        }

        // --- feed ---
        feeder.pump();
        if feeder.pending_frames() > 0 {
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(4));
            continue;
        }
        if eof_pending {
            eof_pending = false;
            at_eof = true;
            shared.eof_index.store(feeder.link.written.load(Ordering::Acquire), Ordering::Release);
            shared.eof.store(true, Ordering::Release);
        }
        if at_eof {
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(20));
            continue;
        }
        let target = (out_rate as f64 * TARGET_FILL_SECS) as u64;
        if feeder.queued_frames() >= target {
            wait_cmd(&mut ctl, &mut feeder, Duration::from_millis(5));
            continue;
        }
        decoded.clear();
        match session.decode_next(&mut decoded) {
            Ok(true) => pipe.process(&decoded, &mut feeder.pending),
            Ok(false) => {
                pipe.flush(&mut feeder.pending);
                eof_pending = true;
            }
            Err(e) => {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let _ = events.send(Event::Error(format!("playback failed: {e}")));
                shared.failed.store(true, Ordering::Release);
                break;
            }
        }
    }
    adopt_rebind(feeder, &mut ctl)
}

/// Command receiver with a one-slot stash, so waiting can be interrupted by
/// a command without losing it.
struct Ctl {
    rx: Receiver<DecCmd>,
    stash: Option<DecCmd>,
}

impl Ctl {
    fn try_recv(&mut self) -> Option<DecCmd> {
        self.stash.take().or_else(|| self.rx.try_recv().ok())
    }

    /// Block up to `d` for a command (kept for the next `try_recv`).
    fn wait(&mut self, d: Duration) -> bool {
        if self.stash.is_some() {
            return true;
        }
        match self.rx.recv_timeout(d) {
            Ok(cmd) => {
                self.stash = Some(cmd);
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
            Err(RecvTimeoutError::Disconnected) => {
                self.stash = Some(DecCmd::Stop);
                true
            }
        }
    }
}

/// On exit, a queued `Rebind` still carries the engine's current producer.
fn adopt_rebind(mut feeder: Feeder, ctl: &mut Ctl) -> (rtrb::Producer<f32>, Arc<Link>) {
    while let Some(cmd) = ctl.try_recv() {
        if let DecCmd::Rebind { producer, link, .. } = cmd {
            feeder.producer = producer;
            feeder.link = link;
        }
    }
    (feeder.producer, feeder.link)
}

/// Wait until a command arrives, the ring wants more audio, or `d` passes;
/// keeps pushing pending audio meanwhile.
fn wait_cmd(ctl: &mut Ctl, feeder: &mut Feeder, d: Duration) {
    let until = Instant::now() + d;
    loop {
        feeder.pump();
        let low = feeder.queued_frames() < (feeder.link.sample_rate as f64 * TARGET_FILL_SECS) as u64;
        if feeder.pending_frames() == 0 && low {
            return;
        }
        let now = Instant::now();
        if now >= until {
            return;
        }
        if ctl.wait((until - now).min(Duration::from_millis(2))) {
            return;
        }
    }
}
