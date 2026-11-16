//! What a song is really playing as, on its whole road to the speaker
//! (port of Android `StreamFacts`, `StreamInspector` and `StreamStats`).
//!
//! The controller notes the resolved YouTube stream (itag, codec, bitrate,
//! client…) and the decoder's format; the engine publishes the output
//! device's format; the Audio Lab settings give the DSP chain. [`cards`] turns
//! a [`Snapshot`] into "stats for nerds" cards — only lines with a real value.

use std::sync::Mutex;

use mochy_audio::output_info::{OutputFormat, current_output};
use mochy_core::models::Song;
use mochy_core::settings::{AudioLab, AudioQuality};
use mochy_innertube::StreamResult;

use crate::ui::i18n::tr;

/// YouTube's record of the stream it handed over.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamFacts {
    pub client: String,
    pub itag: Option<u32>,
    pub mime_type: String,
    pub codecs: Option<String>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u32>,
    pub content_length: Option<u64>,
    pub loudness_db: Option<f32>,
    pub expires_at_unix: Option<u64>,
    pub duration_ms: Option<u64>,
}

impl StreamFacts {
    pub fn of(st: &StreamResult) -> Self {
        StreamFacts {
            client: st.client.clone(),
            itag: st.itag,
            mime_type: st.mime_type.clone(),
            codecs: st.codecs.clone(),
            bitrate_kbps: st.bitrate_kbps,
            sample_rate_hz: st.sample_rate_hz,
            channels: st.channels,
            content_length: st.content_length,
            loudness_db: st.loudness_db,
            expires_at_unix: st.expires_at_unix,
            duration_ms: st.approx_duration_ms,
        }
    }
}

/// The decoder's view of the source (engine `Event::TrackInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct DecoderFacts {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: Option<u32>,
}

/// Playback settings that shape the stream (Settings → Playback).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlaybackFacts {
    pub quality: AudioQuality,
    pub youtube_normalization: bool,
}

#[derive(Default)]
struct Registry {
    streams: Vec<(String, StreamFacts)>,
    decoder: Option<(String, DecoderFacts)>,
    lab: Option<AudioLab>,
    playback: PlaybackFacts,
    now: Option<Song>,
    volume: f32,
    /// Changes asked for by pages that only see the Audio Lab settings.
    want_youtube_normalization: Option<bool>,
    want_quality: Option<AudioQuality>,
}

static REG: Mutex<Option<Registry>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    let mut g = REG.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Registry::default))
}

/// A YouTube stream was resolved for `video_id`.
pub fn note_stream(video_id: Option<&str>, st: &StreamResult) {
    let Some(id) = video_id else { return };
    let facts = StreamFacts::of(st);
    with(|r| {
        r.streams.retain(|(v, _)| v != id);
        r.streams.push((id.to_string(), facts));
        if r.streams.len() > 8 {
            r.streams.remove(0);
        }
    });
}

/// The engine started decoding the current song.
pub fn note_decoder(song: Option<&Song>, codec: &str, sample_rate: u32, channels: u16, bits_per_sample: Option<u32>) {
    let Some(song) = song else { return };
    let d = DecoderFacts { codec: codec.to_string(), sample_rate, channels, bits_per_sample };
    with(|r| r.decoder = Some((song.id.clone(), d)));
}

/// The Audio Lab settings the engine was given.
pub fn note_lab(lab: &AudioLab) {
    with(|r| {
        if r.lab.as_ref() != Some(lab) {
            r.lab = Some(lab.clone());
        }
    });
}

pub fn note_playback(p: PlaybackFacts) {
    with(|r| r.playback = p);
}

/// The playing song and volume, once per frame (from `player_styles::begin_frame`).
pub fn note_now(song: Option<&Song>, volume: f32) {
    with(|r| {
        if r.now.as_ref().map(|s| &s.id) != song.map(|s| &s.id) {
            r.now = song.cloned();
        }
        r.volume = volume;
    });
}

pub fn playback_now() -> PlaybackFacts {
    with(|r| {
        let mut p = r.playback.clone();
        if let Some(n) = r.want_youtube_normalization {
            p.youtube_normalization = n;
        }
        if let Some(q) = r.want_quality {
            p.quality = q;
        }
        p
    })
}

pub fn request_youtube_normalization(on: bool) {
    with(|r| r.want_youtube_normalization = Some(on));
}

pub fn request_quality(q: AudioQuality) {
    with(|r| r.want_quality = Some(q));
}

/// Pending (youtube normalization, quality) changes, cleared.
pub fn take_requests() -> (Option<bool>, Option<AudioQuality>) {
    with(|r| (r.want_youtube_normalization.take(), r.want_quality.take()))
}

/// [`snapshot`] of the playing song.
pub fn snapshot_now() -> Snapshot {
    let (song, volume) = with(|r| (r.now.clone(), r.volume));
    snapshot(song.as_ref(), volume)
}

/// Everything known about `song` right now.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub song_id: Option<String>,
    pub local: bool,
    pub duration_secs: Option<f64>,
    pub volume: f32,
    pub stream: Option<StreamFacts>,
    pub decoder: Option<DecoderFacts>,
    pub output: Option<OutputFormat>,
    pub lab: Option<AudioLab>,
    pub playback: PlaybackFacts,
}

pub fn snapshot(song: Option<&Song>, volume: f32) -> Snapshot {
    let output = current_output();
    with(|r| {
        let stream = song
            .and_then(|s| s.video_id.as_deref())
            .and_then(|v| r.streams.iter().rev().find(|(id, _)| id == v))
            .map(|(_, f)| f.clone());
        let decoder = song.and_then(|s| r.decoder.as_ref().filter(|(id, _)| *id == s.id)).map(|(_, d)| d.clone());
        Snapshot {
            song_id: song.map(|s| s.id.clone()),
            local: song.is_some_and(|s| s.local_path.is_some()),
            duration_secs: song.and_then(|s| s.duration_secs),
            volume,
            stream,
            decoder,
            output,
            lab: r.lab.clone(),
            playback: r.playback.clone(),
        }
    })
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

/// One card of stats for nerds: a title and only the lines with a value.
#[derive(Debug, Clone, PartialEq)]
pub struct StatCard {
    pub title: String,
    pub lines: Vec<(String, String)>,
}

/// The badge's name for a format (Android `StreamFacts.codecOf`).
pub fn codec_of(mime: &str, codecs: Option<&str>) -> Option<&'static str> {
    let mime = mime.to_lowercase();
    let mime = mime.split(';').next().unwrap_or("").trim().to_string();
    let codec = codecs.unwrap_or("").to_lowercase();
    Some(if mime == "audio/opus" || codec.starts_with("opus") {
        "OPUS"
    } else if mime == "audio/flac" || codec == "flac" {
        "FLAC"
    } else if codec == "alac" {
        "ALAC"
    } else if codec == "ec+3" {
        "DOLBY ATMOS"
    } else if codec == "ec-3" {
        "E-AC-3"
    } else if codec == "ac-3" {
        "AC-3"
    } else if codec.starts_with("mp4a.40.5") || codec.starts_with("mp4a.40.29") {
        "HE-AAC"
    } else if mime == "audio/aac" || codec.starts_with("mp4a") {
        "AAC"
    } else if mime == "audio/vorbis" || codec == "vorbis" {
        "VORBIS"
    } else if mime == "audio/mpeg" || codec == "mp3" {
        "MP3"
    } else if mime == "audio/wav" || mime == "audio/raw" {
        "PCM"
    } else if mime == "audio/webm" {
        "OPUS"
    } else if mime == "audio/mp4" {
        "AAC"
    } else {
        return None;
    })
}

fn container_of(mime: &str) -> Option<String> {
    let m = mime.to_lowercase();
    let m = m.split(';').next().unwrap_or("").trim();
    Some(match m {
        "" => return None,
        "audio/webm" | "video/webm" => "WebM".into(),
        "audio/mp4" | "video/mp4" => "MP4".into(),
        "audio/flac" => "FLAC".into(),
        "audio/ogg" => "Ogg".into(),
        "audio/mpeg" => "MP3".into(),
        "audio/wav" | "audio/x-wav" => "WAV".into(),
        other => other.into(),
    })
}

/// "44.1 kHz", "48 kHz", "22.05 kHz".
pub fn khz(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else if rate % 100 == 0 {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    } else {
        format!("{:.2} kHz", rate as f64 / 1000.0)
    }
}

fn channel_name(n: u32) -> String {
    match n {
        1 => tr("Mono").into(),
        2 => tr("Stereo").into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => tr("{n} channels").replace("{n}", &n.to_string()),
    }
}

/// ".Opus", ".AAC" — Android's card title style.
fn format_title(codec: &str) -> String {
    if ["FLAC", "OPUS", "VORBIS", "ALAC"].contains(&codec) {
        let lower = codec.to_lowercase();
        let mut c = lower.chars();
        format!(".{}{}", c.next().map(|f| f.to_uppercase().collect::<String>()).unwrap_or_default(), c.as_str())
    } else {
        format!(".{codec}")
    }
}

fn line(label: &str, value: impl Into<String>) -> (String, String) {
    (label.to_string(), value.into())
}

/// The badge text the player shows ("OPUS 160 kbps · 48 kHz"), or `None`.
pub fn badge(s: &Snapshot, with_rate: bool) -> Option<String> {
    let codec = s
        .stream
        .as_ref()
        .and_then(|st| codec_of(&st.mime_type, st.codecs.as_deref()).map(str::to_string))
        .or_else(|| s.decoder.as_ref().map(|d| d.codec.to_uppercase()))?;
    let mut out = codec;
    if let Some(b) = s.stream.as_ref().and_then(|st| st.bitrate_kbps) {
        out.push_str(&format!(" {b} kbps"));
    }
    if with_rate {
        let rate = s.decoder.as_ref().map(|d| d.sample_rate).or(s.stream.as_ref().and_then(|st| st.sample_rate_hz));
        if let Some(r) = rate {
            out.push_str(&format!(" · {}", khz(r)));
        }
        if let Some(b) = s.decoder.as_ref().and_then(|d| d.bits_per_sample) {
            out.push_str(&format!(" {b}-bit"));
        }
    }
    Some(out)
}

/// Format, decoder, resampler, DSP and output cards (Android `buildStatCards`).
pub fn cards(s: &Snapshot) -> Vec<StatCard> {
    let mut out = vec![format_card(s), decoder_card(s), resampler_card(s), dsp_card(s), output_card(s)];
    out.retain(|c| !c.lines.is_empty());
    out
}

fn format_card(s: &Snapshot) -> StatCard {
    let st = s.stream.as_ref();
    let d = s.decoder.as_ref();
    let codec = st
        .and_then(|st| codec_of(&st.mime_type, st.codecs.as_deref()).map(str::to_string))
        .or_else(|| d.map(|d| d.codec.to_uppercase()));
    let lossless = codec.as_deref().is_some_and(|c| ["FLAC", "ALAC", "PCM", "WAV"].contains(&c));
    let mut lines = Vec::new();
    if lossless && let Some(b) = d.and_then(|d| d.bits_per_sample) {
        lines.push(line(tr("Bit depth"), format!("{b}-bit")));
    }
    if let Some(r) = d.map(|d| d.sample_rate).or(st.and_then(|st| st.sample_rate_hz)) {
        lines.push(line(tr("Sample rate"), khz(r)));
    }
    if let Some(b) = st.and_then(|st| st.bitrate_kbps) {
        lines.push(line(tr("Bitrate"), format!("{b} kbps")));
    } else if let (Some(len), Some(dur)) = (st.and_then(|st| st.content_length), s.duration_secs.filter(|d| *d > 0.0)) {
        let kbps = (len as f64 * 8.0 / dur / 1000.0).round();
        lines.push(line(tr("Bitrate"), tr("{v} (average)").replace("{v}", &format!("{kbps} kbps"))));
    }
    if let Some(c) = d.map(|d| d.channels as u32).or(st.and_then(|st| st.channels)) {
        lines.push(line(tr("Channels"), channel_name(c)));
    }
    if let Some(c) = st.and_then(|st| container_of(&st.mime_type)) {
        lines.push(line(tr("Container"), c));
    }
    if let Some(st) = st {
        if let Some(i) = st.itag {
            lines.push(line(tr("Format (itag)"), i.to_string()));
        }
        if let Some(c) = st.codecs.as_deref().filter(|c| !c.is_empty()) {
            lines.push(line(tr("Codec string"), c.to_string()));
        }
        if !st.client.is_empty() {
            lines.push(line(tr("Client"), st.client.clone()));
        }
        if let Some(l) = st.loudness_db {
            lines.push(line(tr("Loudness"), format!("{l:+.1} dB")));
        }
        if let Some(len) = st.content_length {
            lines.push(line(tr("File size"), format!("{:.1} MB", len as f64 / 1_048_576.0)));
        }
    }
    if s.local {
        lines.push(line(tr("Source"), tr("Local file")));
    } else if st.is_some() {
        lines.push(line(tr("Source"), "YouTube Music"));
    }
    if let Some(id) = &s.song_id {
        lines.push(line(tr("Media ID"), id.clone()));
    }
    if s.song_id.is_some() {
        lines.push(line(tr("Volume"), format!("{}%", (s.volume * 100.0).round())));
    }
    StatCard { title: codec.as_deref().map(format_title).unwrap_or_else(|| tr("Format").into()), lines }
}

fn decoder_card(s: &Snapshot) -> StatCard {
    let mut lines = Vec::new();
    if let Some(d) = &s.decoder {
        let name = match d.codec.to_lowercase().as_str() {
            "opus" => "rusty-opus",
            _ => "symphonia",
        };
        lines.push(line(tr("Decoder"), name));
        lines.push(line(tr("Runs on"), tr("Software (CPU)")));
        lines.push(line(tr("Codec"), d.codec.to_uppercase()));
        lines.push(line(tr("Output"), tr("32-bit float PCM")));
        lines.push(line(tr("Output rate"), khz(d.sample_rate)));
        lines.push(line(tr("Channels"), channel_name(d.channels as u32)));
    }
    StatCard { title: tr("Decoder").into(), lines }
}

fn resampler_card(s: &Snapshot) -> StatCard {
    let mut lines = Vec::new();
    let in_rate = s.decoder.as_ref().map(|d| d.sample_rate);
    let out_rate = s.output.as_ref().map(|o| o.sample_rate);
    let (speed, pitch_st, preserve) =
        s.lab.as_ref().map(|l| (l.speed, l.pitch_semitones, l.preserve_pitch)).unwrap_or((1.0, 0.0, false));
    let stretch = preserve && ((speed - 1.0).abs() > 1e-3 || pitch_st.abs() > 1e-3);
    if let (Some(i), Some(o)) = (in_rate, out_rate) {
        let resampling = i != o || (speed - 1.0).abs() > 1e-3;
        let kind = if stretch {
            tr("Time-stretch (WSOLA) + sinc resampler")
        } else if resampling {
            tr("Mochy sinc resampler (Kaiser)")
        } else {
            tr("None — rates match")
        };
        lines.push(line(tr("Type"), kind));
        lines.push(line(
            tr("In → out"),
            if i != o { format!("{} → {}", khz(i), khz(o)) } else { tr("{v} in and out").replace("{v}", &khz(i)) },
        ));
        if i != o {
            lines.push(line(tr("Cut-off"), format!("~{}", khz(i.min(o) / 2))));
        } else if !resampling {
            lines.push(line(tr("Quality"), tr("Untouched")));
        }
    }
    if (speed - 1.0).abs() > 1e-3 {
        lines.push(line(tr("Speed"), format!("{speed:.2}×")));
    }
    if preserve && pitch_st.abs() > 1e-3 {
        lines.push(line(tr("Pitch"), tr("{v} semitones").replace("{v}", &format!("{pitch_st:+.1}"))));
    } else if !preserve && (speed - 1.0).abs() > 1e-3 {
        lines.push(line(tr("Pitch"), tr("Follows speed")));
    }
    StatCard { title: tr("Resampler").into(), lines }
}

/// The active effects in chain order, as names (Android `dspCard`'s chain).
pub fn dsp_chain(lab: &AudioLab) -> Vec<String> {
    let mut chain = Vec::new();
    let eq = &lab.eq;
    if eq.enabled {
        let bands = eq.nodes.iter().filter(|n| n.enabled).count();
        let mut s = tr("EQ ({n} bands)").replace("{n}", &bands.to_string());
        if let Some(c) = eq.active_correction() {
            s.push_str(&format!(" + {}", c.model));
        }
        chain.push(s);
    }
    chain.push(tr("Peak limiter").into());
    if lab.reverb.enabled {
        chain.push(tr("Reverb").into());
    }
    let sp = &lab.spatial;
    if sp.virtual_speakers {
        chain.push(tr("Virtual speakers").into());
    } else if sp.crossfeed_enabled {
        chain.push(tr("Crossfeed").into());
    }
    if sp.mono {
        chain.push(tr("Mono").into());
    }
    if (sp.width - 1.0).abs() > 1e-3 {
        chain.push(tr("Stereo width").into());
    }
    if lab.loudness_enabled {
        chain.push(tr("Loudness").into());
    }
    if lab.volume_curve_enabled {
        chain.push(tr("Volume curve").into());
    }
    chain
}

fn dsp_card(s: &Snapshot) -> StatCard {
    let mut lines = Vec::new();
    if let Some(lab) = &s.lab {
        lines.push(line(tr("Processing"), tr("32-bit float")));
        if let Some(o) = &s.output {
            lines.push(line(tr("Sample rate"), khz(o.sample_rate)));
        }
        lines.push(line(tr("Effects"), dsp_chain(lab).join(", ")));
        if lab.eq.enabled {
            lines.push(line(tr("Preamp"), format!("{:+.1} dB", mochy_core::eq::effective_preamp_db(&lab.eq))));
        }
        let norm = if s.local { lab.normalize_replay_gain } else { s.playback.youtube_normalization };
        if norm {
            lines.push(line(tr("Normalization"), if s.local { tr("ReplayGain") } else { tr("YouTube loudness") }));
        }
    }
    StatCard { title: tr("DSP").into(), lines }
}

fn output_card(s: &Snapshot) -> StatCard {
    let mut lines = Vec::new();
    if let Some(o) = &s.output {
        lines.push(line(tr("Device"), o.device.clone()));
        lines.push(line(tr("Audio API"), o.host.clone()));
        lines.push(line(
            tr("Device format"),
            format!("{} · {} · {}", o.sample_format, khz(o.sample_rate), channel_name(o.channels as u32)),
        ));
        lines.push(line(tr("Mode"), tr("Shared (system mixer)")));
    }
    StatCard { title: tr("Output").into(), lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot {
            song_id: Some("abc".into()),
            local: false,
            duration_secs: Some(200.0),
            volume: 0.8,
            stream: Some(StreamFacts {
                client: "VISIONOS".into(),
                itag: Some(251),
                mime_type: "audio/webm; codecs=\"opus\"".into(),
                codecs: Some("opus".into()),
                bitrate_kbps: Some(160),
                sample_rate_hz: Some(48_000),
                channels: Some(2),
                content_length: Some(3_500_000),
                loudness_db: Some(-2.5),
                ..Default::default()
            }),
            decoder: Some(DecoderFacts { codec: "opus".into(), sample_rate: 48_000, channels: 2, bits_per_sample: None }),
            output: Some(OutputFormat {
                device: "Speakers".into(),
                sample_rate: 44_100,
                channels: 2,
                sample_format: "f32".into(),
                host: "Wasapi".into(),
            }),
            lab: Some(AudioLab::default()),
            playback: PlaybackFacts::default(),
        }
    }

    #[test]
    fn codec_names() {
        assert_eq!(codec_of("audio/webm; codecs=\"opus\"", None), Some("OPUS"));
        assert_eq!(codec_of("audio/mp4", Some("mp4a.40.5")), Some("HE-AAC"));
        assert_eq!(codec_of("audio/mp4", Some("mp4a.40.2")), Some("AAC"));
        assert_eq!(codec_of("audio/mp4", None), Some("AAC"));
        assert_eq!(codec_of("text/plain", None), None);
        assert_eq!(khz(44_100), "44.1 kHz");
        assert_eq!(khz(48_000), "48 kHz");
        assert_eq!(khz(22_050), "22.05 kHz");
        assert_eq!(format_title("OPUS"), ".Opus");
        assert_eq!(format_title("AAC"), ".AAC");
    }

    #[test]
    fn cards_have_only_real_lines() {
        // Values only: labels follow the UI language (a global other tests flip).
        let s = snap();
        let c = cards(&s);
        assert_eq!(c.len(), 5);
        assert_eq!(c[0].title, ".Opus");
        let has = |card: usize, value: &str| c[card].lines.iter().any(|l| l.1 == value);
        assert!(has(0, "160 kbps"));
        assert!(has(0, "251"));
        assert!(has(0, "WebM"));
        assert!(has(0, "VISIONOS"));
        assert!(!c[0].lines.iter().any(|l| l.1.ends_with("-bit"))); // lossy: no bit depth
        assert!(has(2, "48 kHz → 44.1 kHz"));
        assert!(c[3].lines.iter().any(|l| l.1.contains(',') || !l.1.is_empty()));
        assert!(has(4, "Speakers"));
        assert_eq!(badge(&s, true).as_deref(), Some("OPUS 160 kbps · 48 kHz"));
        assert_eq!(badge(&s, false).as_deref(), Some("OPUS 160 kbps"));
        // Nothing known: no cards at all.
        assert!(cards(&Snapshot::default()).is_empty());
    }

    #[test]
    fn chain_lists_active_effects_in_order() {
        let mut lab = AudioLab::default();
        assert_eq!(dsp_chain(&lab).len(), 1); // the limiter is always on
        lab.eq.enabled = true;
        lab.eq.nodes = vec![mochy_core::eq::EqNode::peaking(100.0, 3.0)];
        lab.reverb.enabled = true;
        lab.spatial.crossfeed_enabled = true;
        lab.loudness_enabled = true;
        let chain = dsp_chain(&lab);
        assert_eq!(chain.len(), 5);
        assert!(chain[0].starts_with("EQ"));
    }
}
