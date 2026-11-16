//! Just enough Matroska/WebM (EBML) demuxing to pull an audio track's
//! packets out of a YouTube `audio/webm` stream: track info (codec id,
//! codec private, codec delay, sample rate, channels) and every frame of
//! that track with its timestamp, from SimpleBlocks and BlockGroups,
//! with all four lacing modes.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum WebmError {
    #[error("not a WebM/Matroska file")]
    NotWebm,
    #[error("no audio track")]
    NoAudioTrack,
    #[error("malformed WebM: {0}")]
    Malformed(&'static str),
}

const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TIMECODE_SCALE: u32 = 0x2A_D7B1;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_TRACK_ENTRY: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_CODEC_PRIVATE: u32 = 0x63A2;
const ID_CODEC_DELAY: u32 = 0x56AA;
const ID_SEEK_PREROLL: u32 = 0x56BB;
const ID_AUDIO: u32 = 0xE1;
const ID_SAMPLING_FREQUENCY: u32 = 0xB5;
const ID_CHANNELS: u32 = 0x9F;
const ID_CLUSTER: u32 = 0x1F43_B675;
const ID_CLUSTER_TIMECODE: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;
const ID_DISCARD_PADDING: u32 = 0x75A2;

/// Top-level (segment children) ids: an unknown-size cluster ends where one
/// of these starts.
const SEGMENT_CHILDREN: &[u32] = &[
    0x114D_9B74, // SeekHead
    ID_INFO,
    ID_TRACKS,
    0x1C53_BB6B, // Cues
    ID_CLUSTER,
    0x1254_C367, // Tags
    0x1043_A770, // Chapters
    0x1941_A469, // Attachments
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub number: u64,
    pub track_type: u64,
    pub codec_id: String,
    pub codec_private: Vec<u8>,
    pub codec_delay_ns: u64,
    pub seek_preroll_ns: u64,
    pub sample_rate: f64,
    pub channels: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub data: Vec<u8>,
    /// Block timestamp in nanoseconds.
    pub timestamp_ns: i64,
    /// `DiscardPadding` of the block this frame came from (ns), on the last
    /// frame of the block only.
    pub discard_padding_ns: i64,
}

#[derive(Debug, Clone, Default)]
pub struct WebmAudio {
    pub timecode_scale: u64,
    pub track: Track,
    pub frames: Vec<Frame>,
}

/// Read a variable-length integer. `keep_marker` for element ids.
fn vint(data: &[u8], at: usize, keep_marker: bool) -> Option<(u64, usize, bool)> {
    let first = *data.get(at)?;
    if first == 0 {
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || at + len > data.len() {
        return None;
    }
    let mut value: u64 = if keep_marker { first as u64 } else { (first as u64) & ((1u64 << (8 - len)) - 1) };
    let mut all_ones = value == (1u64 << (8 - len)) - 1;
    for &b in &data[at + 1..at + len] {
        value = (value << 8) | b as u64;
        all_ones &= b == 0xFF;
    }
    Some((value, len, all_ones && !keep_marker))
}

struct Element {
    id: u32,
    /// Start of the payload.
    start: usize,
    /// End of the payload (clamped to the data; `unknown` = size not given).
    end: usize,
    unknown: bool,
}

fn element(data: &[u8], at: usize, limit: usize) -> Option<Element> {
    let (id, id_len, _) = vint(data, at, true)?;
    let (size, size_len, unknown) = vint(data, at + id_len, false)?;
    let start = at + id_len + size_len;
    if start > limit {
        return None;
    }
    let end = if unknown { limit } else { start.saturating_add(size as usize).min(limit) };
    Some(Element { id: id as u32, start, end, unknown })
}

fn uint(data: &[u8]) -> u64 {
    data.iter().take(8).fold(0u64, |v, &b| (v << 8) | b as u64)
}

fn sint(data: &[u8]) -> i64 {
    if data.is_empty() {
        return 0;
    }
    let mut v: i64 = if data[0] & 0x80 != 0 { -1 } else { 0 };
    for &b in data.iter().take(8) {
        v = (v << 8) | b as i64;
    }
    v
}

fn float(data: &[u8]) -> f64 {
    match data.len() {
        4 => f32::from_be_bytes(data.try_into().unwrap_or([0; 4])) as f64,
        8 => f64::from_be_bytes(data.try_into().unwrap_or([0; 8])),
        _ => 0.0,
    }
}

/// Demux the first audio track (or the first track when none says audio).
pub fn demux_audio(data: &[u8]) -> Result<WebmAudio, WebmError> {
    let first = element(data, 0, data.len()).ok_or(WebmError::NotWebm)?;
    if first.id != ID_EBML {
        return Err(WebmError::NotWebm);
    }
    let mut at = first.end;
    let segment = loop {
        let el = element(data, at, data.len()).ok_or(WebmError::Malformed("no segment"))?;
        if el.id == ID_SEGMENT {
            break el;
        }
        at = el.end;
    };

    let mut out = WebmAudio { timecode_scale: 1_000_000, ..Default::default() };
    let mut tracks: Vec<Track> = Vec::new();
    // Clusters are walked after the tracks are known (they come first in
    // practice, but don't rely on it).
    let mut clusters: Vec<(usize, usize)> = Vec::new();

    let mut at = segment.start;
    while at < segment.end {
        let Some(el) = element(data, at, segment.end) else { break };
        match el.id {
            ID_INFO => {
                for_children(data, el.start, el.end, |id, payload| {
                    if id == ID_TIMECODE_SCALE {
                        out.timecode_scale = uint(payload).max(1);
                    }
                });
            }
            ID_TRACKS => {
                let mut t_at = el.start;
                while t_at < el.end {
                    let Some(entry) = element(data, t_at, el.end) else { break };
                    if entry.id == ID_TRACK_ENTRY {
                        tracks.push(parse_track(data, entry.start, entry.end));
                    }
                    t_at = entry.end.max(t_at + 1);
                }
            }
            ID_CLUSTER => {
                let end = if el.unknown { unknown_cluster_end(data, el.start, segment.end) } else { el.end };
                clusters.push((el.start, end));
                at = end;
                continue;
            }
            _ => {}
        }
        if el.end <= at {
            break;
        }
        at = el.end;
    }

    let track = tracks
        .iter()
        .find(|t| t.track_type == 2)
        .or_else(|| tracks.first())
        .cloned()
        .ok_or(WebmError::NoAudioTrack)?;
    for (start, end) in clusters {
        read_cluster(data, start, end, &track, out.timecode_scale, &mut out.frames);
    }
    out.track = track;
    Ok(out)
}

fn for_children<'a>(data: &'a [u8], mut at: usize, end: usize, mut f: impl FnMut(u32, &'a [u8])) {
    while at < end {
        let Some(el) = element(data, at, end) else { break };
        f(el.id, &data[el.start..el.end]);
        if el.end <= at {
            break;
        }
        at = el.end;
    }
}

fn parse_track(data: &[u8], start: usize, end: usize) -> Track {
    let mut t = Track::default();
    for_children(data, start, end, |id, p| match id {
        ID_TRACK_NUMBER => t.number = uint(p),
        ID_TRACK_TYPE => t.track_type = uint(p),
        ID_CODEC_ID => t.codec_id = String::from_utf8_lossy(p).trim_end_matches('\0').to_string(),
        ID_CODEC_PRIVATE => t.codec_private = p.to_vec(),
        ID_CODEC_DELAY => t.codec_delay_ns = uint(p),
        ID_SEEK_PREROLL => t.seek_preroll_ns = uint(p),
        ID_AUDIO => for_children(p, 0, p.len(), |aid, ap| match aid {
            ID_SAMPLING_FREQUENCY => t.sample_rate = float(ap),
            ID_CHANNELS => t.channels = uint(ap),
            _ => {}
        }),
        _ => {}
    });
    t
}

fn unknown_cluster_end(data: &[u8], start: usize, limit: usize) -> usize {
    let mut at = start;
    while at < limit {
        let Some(el) = element(data, at, limit) else { return limit };
        if SEGMENT_CHILDREN.contains(&el.id) {
            return at;
        }
        if el.end <= at {
            return limit;
        }
        at = el.end;
    }
    limit
}

fn read_cluster(data: &[u8], start: usize, end: usize, track: &Track, scale: u64, frames: &mut Vec<Frame>) {
    let mut cluster_tc: i64 = 0;
    let mut at = start;
    while at < end {
        let Some(el) = element(data, at, end) else { break };
        match el.id {
            ID_CLUSTER_TIMECODE => cluster_tc = uint(&data[el.start..el.end]) as i64,
            ID_SIMPLE_BLOCK => block(&data[el.start..el.end], track, cluster_tc, scale, 0, frames),
            ID_BLOCK_GROUP => {
                let mut payload: Option<&[u8]> = None;
                let mut discard = 0i64;
                for_children(data, el.start, el.end, |id, p| match id {
                    ID_BLOCK => payload = Some(p),
                    ID_DISCARD_PADDING => discard = sint(p),
                    _ => {}
                });
                if let Some(p) = payload {
                    block(p, track, cluster_tc, scale, discard, frames);
                }
            }
            _ => {}
        }
        if el.end <= at {
            break;
        }
        at = el.end;
    }
}

/// One (Simple)Block: track, relative timecode, flags, then laced frames.
fn block(b: &[u8], track: &Track, cluster_tc: i64, scale: u64, discard_ns: i64, frames: &mut Vec<Frame>) {
    let Some((number, n_len, _)) = vint(b, 0, false) else { return };
    if number != track.number || b.len() < n_len + 3 {
        return;
    }
    let rel = i16::from_be_bytes([b[n_len], b[n_len + 1]]) as i64;
    let flags = b[n_len + 2];
    let body = &b[n_len + 3..];
    let ts = (cluster_tc + rel) * scale as i64;
    let Some(sizes) = lace_sizes(body, (flags >> 1) & 3) else { return };
    let (mut at, sizes) = sizes;
    let count = sizes.len();
    for (i, size) in sizes.into_iter().enumerate() {
        if at + size > body.len() {
            return;
        }
        frames.push(Frame {
            data: body[at..at + size].to_vec(),
            timestamp_ns: ts,
            discard_padding_ns: if i + 1 == count { discard_ns } else { 0 },
        });
        at += size;
    }
}

/// Frame sizes for a block body and where the first frame starts.
fn lace_sizes(body: &[u8], lacing: u8) -> Option<(usize, Vec<usize>)> {
    if lacing == 0 {
        return Some((0, vec![body.len()]));
    }
    let count = *body.first()? as usize + 1;
    let mut at = 1;
    let mut sizes = Vec::with_capacity(count);
    match lacing {
        1 => {
            // Xiph: each size as a run of 255s plus a final byte.
            for _ in 0..count - 1 {
                let mut size = 0usize;
                loop {
                    let v = *body.get(at)?;
                    at += 1;
                    size += v as usize;
                    if v < 255 {
                        break;
                    }
                }
                sizes.push(size);
            }
        }
        3 => {
            // EBML: first size unsigned, then signed differences.
            let (first, len, _) = vint(body, at, false)?;
            at += len;
            sizes.push(first as usize);
            let mut prev = first as i64;
            for _ in 1..count - 1 {
                let (raw, len, _) = vint(body, at, false)?;
                at += len;
                let bias = (1i64 << (7 * len - 1)) - 1;
                let size = prev + (raw as i64 - bias);
                if size < 0 {
                    return None;
                }
                sizes.push(size as usize);
                prev = size;
            }
        }
        2 => {
            let each = (body.len().checked_sub(1)?) / count;
            return Some((1, vec![each; count]));
        }
        _ => return None,
    }
    let used: usize = sizes.iter().sum();
    let last = body.len().checked_sub(at + used)?;
    sizes.push(last);
    Some((at, sizes))
}

// ---------------------------------------------------------------------------
// A tiny EBML writer, used by tests (and handy for fixtures elsewhere).
// ---------------------------------------------------------------------------

#[doc(hidden)]
pub mod writer {
    pub fn id(out: &mut Vec<u8>, id: u32) {
        let bytes = id.to_be_bytes();
        let skip = bytes.iter().position(|&b| b != 0).unwrap_or(3);
        out.extend_from_slice(&bytes[skip..]);
    }

    pub fn size(out: &mut Vec<u8>, size: u64) {
        // Always 8 bytes: simple and valid.
        out.push(0x01);
        out.extend_from_slice(&size.to_be_bytes()[1..]);
    }

    pub fn el(out: &mut Vec<u8>, id_: u32, payload: &[u8]) {
        id(out, id_);
        size(out, payload.len() as u64);
        out.extend_from_slice(payload);
    }

    pub fn uint_el(out: &mut Vec<u8>, id_: u32, v: u64) {
        el(out, id_, &v.to_be_bytes());
    }

    pub fn float_el(out: &mut Vec<u8>, id_: u32, v: f64) {
        el(out, id_, &v.to_be_bytes());
    }

    /// A YouTube-like audio/webm with one Opus track and `packets` split into
    /// clusters of `per_cluster` SimpleBlocks, 20 ms apart.
    pub fn opus_webm(opus_head: &[u8], codec_delay_ns: u64, packets: &[Vec<u8>], per_cluster: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut header = Vec::new();
        el(&mut header, 0x4282, b"webm"); // DocType
        el(&mut out, super::ID_EBML, &header);

        let mut seg = Vec::new();
        let mut info = Vec::new();
        uint_el(&mut info, super::ID_TIMECODE_SCALE, 1_000_000);
        el(&mut seg, super::ID_INFO, &info);

        let mut entry = Vec::new();
        uint_el(&mut entry, super::ID_TRACK_NUMBER, 1);
        uint_el(&mut entry, super::ID_TRACK_TYPE, 2);
        el(&mut entry, super::ID_CODEC_ID, b"A_OPUS");
        el(&mut entry, super::ID_CODEC_PRIVATE, opus_head);
        uint_el(&mut entry, super::ID_CODEC_DELAY, codec_delay_ns);
        uint_el(&mut entry, super::ID_SEEK_PREROLL, 80_000_000);
        let mut audio = Vec::new();
        float_el(&mut audio, super::ID_SAMPLING_FREQUENCY, 48_000.0);
        uint_el(&mut audio, super::ID_CHANNELS, 2);
        el(&mut entry, super::ID_AUDIO, &audio);
        let mut tracks = Vec::new();
        el(&mut tracks, super::ID_TRACK_ENTRY, &entry);
        el(&mut seg, super::ID_TRACKS, &tracks);

        for (c, chunk) in packets.chunks(per_cluster.max(1)).enumerate() {
            let mut cluster = Vec::new();
            let base_ms = (c * per_cluster * 20) as u64;
            uint_el(&mut cluster, super::ID_CLUSTER_TIMECODE, base_ms);
            for (i, p) in chunk.iter().enumerate() {
                let mut block = vec![0x81]; // track 1
                block.extend_from_slice(&((i * 20) as i16).to_be_bytes());
                block.push(0x80); // keyframe, no lacing
                block.extend_from_slice(p);
                el(&mut cluster, super::ID_SIMPLE_BLOCK, &block);
            }
            el(&mut seg, super::ID_CLUSTER, &cluster);
        }
        el(&mut out, super::ID_SEGMENT, &seg);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vint_reads_sizes_and_ids() {
        assert_eq!(vint(&[0x81], 0, false), Some((1, 1, false)));
        assert_eq!(vint(&[0x40, 0x02], 0, false), Some((2, 2, false)));
        assert_eq!(vint(&[0xFF], 0, false), Some((127, 1, true)));
        assert_eq!(vint(&[0x1A, 0x45, 0xDF, 0xA3], 0, true), Some((0x1A45_DFA3, 4, false)));
    }

    #[test]
    fn demuxes_synthetic_opus_webm() {
        let head = b"OpusHead\x01\x02\x38\x01\x80\xbb\x00\x00\x00\x00\x00".to_vec();
        let packets: Vec<Vec<u8>> = (0..25u8).map(|i| vec![0xFC, i, i, i]).collect();
        let webm = writer::opus_webm(&head, 6_500_000, &packets, 10);
        let a = demux_audio(&webm).unwrap();
        assert_eq!(a.track.codec_id, "A_OPUS");
        assert_eq!(a.track.codec_private, head);
        assert_eq!(a.track.channels, 2);
        assert_eq!(a.track.sample_rate, 48_000.0);
        assert_eq!(a.track.codec_delay_ns, 6_500_000);
        assert_eq!(a.frames.len(), 25);
        assert_eq!(a.frames.iter().map(|f| f.data.clone()).collect::<Vec<_>>(), packets);
        assert_eq!(a.frames[11].timestamp_ns, 220_000_000);
    }

    #[test]
    fn lacing_modes() {
        // Xiph: 3 frames of 300, 2, rest(4)
        let mut body = vec![2u8, 255, 45, 2];
        body.extend(std::iter::repeat_n(7u8, 306));
        let (at, sizes) = lace_sizes(&body, 1).unwrap();
        assert_eq!((at, sizes), (4, vec![300, 2, 4]));
        // Fixed: 2 frames of 3
        let (at, sizes) = lace_sizes(&[1, 0, 0, 0, 0, 0, 0], 2).unwrap();
        assert_eq!((at, sizes), (1, vec![3, 3]));
        // EBML: 3 frames 10, 12 (+2), rest
        let mut body = vec![2u8, 0x8A, 0xC1]; // 10, diff +2 (0x41 - 63 = 2)
        body.extend(std::iter::repeat_n(0u8, 10 + 12 + 5));
        let (at, sizes) = lace_sizes(&body, 3).unwrap();
        assert_eq!((at, sizes), (3, vec![10, 12, 5]));
    }

    #[test]
    fn rejects_non_webm() {
        assert_eq!(demux_audio(b"OggS....").unwrap_err(), WebmError::NotWebm);
        assert!(demux_audio(&[]).is_err());
    }
}
