//! WebM-Opus → Ogg Opus remux (port of Android `OggOpusRemux`, which used
//! MediaExtractor + MediaMuxer; here a pure-Rust demux + Ogg writer). The
//! same Opus packets move into a plain `.opus` file — no re-encoding —
//! laid out per RFC 7845: an `OpusHead` page, the `OpusTags` comment
//! header (carrying the song's tags, like `ExportTagWriter.copyOpus`), then
//! audio pages whose granule positions count 48 kHz samples.

use thiserror::Error;

use crate::ogg::OggWriter;
use crate::tags::{ExportTags, vorbis_comments};
use crate::webm::{self, WebmError};

#[derive(Debug, Error, PartialEq)]
pub enum RemuxError {
    #[error(transparent)]
    Webm(#[from] WebmError),
    #[error("the WebM audio track is {0}, not Opus")]
    NotOpus(String),
    #[error("no Opus packets in the file")]
    Empty,
}

/// Samples (at 48 kHz) in one Opus packet, from its TOC byte (RFC 6716 §3.1).
pub fn packet_samples(packet: &[u8]) -> Option<u32> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    let frame = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*packet.get(1)? & 0x3F) as u32,
    };
    let total = frame * frames;
    // RFC 6716: at most 120 ms per packet.
    (total <= 5760).then_some(total)
}

/// An `OpusHead` for a track with no usable CodecPrivate.
pub fn build_opus_head(channels: u8, pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(channels.max(1));
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes());
    h.push(0);
    h
}

/// The `OpusTags` packet: magic + vorbis comments.
pub fn opus_tags_packet(tags: &ExportTags) -> Vec<u8> {
    let mut p = b"OpusTags".to_vec();
    p.extend_from_slice(&vorbis_comments(tags));
    p
}

/// A serial number derived from the content (deterministic output).
fn serial_for(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for &b in data.iter().take(4096) {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Remux a whole WebM file (bytes) into an Ogg Opus file (bytes).
pub fn webm_to_ogg_opus(webm_bytes: &[u8], tags: &ExportTags) -> Result<Vec<u8>, RemuxError> {
    let audio = webm::demux_audio(webm_bytes)?;
    if audio.track.codec_id != "A_OPUS" {
        return Err(RemuxError::NotOpus(audio.track.codec_id));
    }
    if audio.frames.is_empty() {
        return Err(RemuxError::Empty);
    }
    let pre_skip = (audio.track.codec_delay_ns as u128 * 48_000 / 1_000_000_000) as u16;
    let head = if audio.track.codec_private.len() >= 19 && audio.track.codec_private.starts_with(b"OpusHead") {
        audio.track.codec_private.clone()
    } else {
        let rate = if audio.track.sample_rate > 0.0 { audio.track.sample_rate as u32 } else { 48_000 };
        build_opus_head(audio.track.channels.clamp(1, 2) as u8, pre_skip, rate)
    };

    let mut w = OggWriter::new(serial_for(webm_bytes));
    // Page 0: OpusHead alone (BOS).
    w.packet(&head, 0);
    w.flush(0);
    // Comment header on its own page(s); audio starts on a fresh page.
    w.packet(&opus_tags_packet(tags), 0);
    w.flush(0);

    let mut granule: i64 = 0;
    let last = audio.frames.len() - 1;
    for (i, f) in audio.frames.iter().enumerate() {
        if f.data.is_empty() {
            continue;
        }
        granule += packet_samples(&f.data).unwrap_or(960) as i64;
        let mut g = granule;
        if i == last && f.discard_padding_ns > 0 {
            // End trimming: the last page's granule may stop short of the
            // decoded length (RFC 7845 §4.4).
            let trim = (f.discard_padding_ns as i128 * 48_000 / 1_000_000_000) as i64;
            g = (granule - trim).max(pre_skip as i64);
        }
        w.packet(&f.data, g);
    }
    Ok(w.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ogg::{self, FLAG_BOS, FLAG_EOS};
    use crate::tags::Cover;
    use crate::webm::writer;

    fn head() -> Vec<u8> {
        build_opus_head(2, 312, 48_000)
    }

    #[test]
    fn packet_sample_counts() {
        assert_eq!(packet_samples(&[0xFC]), Some(960)); // CELT FB 20 ms, 1 frame
        assert_eq!(packet_samples(&[0xF8 | 1]), Some(1920)); // 2 frames
        assert_eq!(packet_samples(&[0x00]), Some(480)); // SILK NB 10 ms
        assert_eq!(packet_samples(&[0x03, 0x03]), Some(1440)); // 3 × 10 ms
        assert_eq!(packet_samples(&[0x78]), Some(960)); // hybrid FB 20 ms
        assert_eq!(packet_samples(&[]), None);
    }

    #[test]
    fn remux_produces_valid_ogg_opus() {
        let packets: Vec<Vec<u8>> = (0..300u32).map(|i| {
            let mut p = vec![0xFC];
            p.extend(std::iter::repeat_n((i % 251) as u8, 40 + (i as usize % 90)));
            p
        }).collect();
        let webm = writer::opus_webm(&head(), 6_500_000, &packets, 50);
        let tags = ExportTags { title: "Song".into(), artist: Some("Artist".into()), ..Default::default() };
        let ogg_bytes = webm_to_ogg_opus(&webm, &tags).unwrap();

        let pages = ogg::read_pages(&ogg_bytes).expect("well-formed pages");
        assert!(pages.iter().all(|p| p.crc_ok), "every page CRC must verify");
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(p.sequence as usize, i);
            assert_eq!(p.serial, pages[0].serial);
        }
        assert_eq!(pages[0].header_type, FLAG_BOS);
        assert_eq!(pages[0].lacing.len(), 1, "OpusHead alone on the first page");
        assert_eq!(pages[0].granule, 0);
        assert_eq!(pages[1].granule, 0);
        assert!(pages.last().unwrap().header_type & FLAG_EOS != 0);
        // granules grow and end at the total sample count
        let audio_pages = &pages[2..];
        let mut prev = 0;
        for p in audio_pages {
            assert!(p.granule >= prev);
            prev = p.granule;
        }
        assert_eq!(prev, 300 * 960);

        let pk = ogg::packets(&pages);
        assert_eq!(pk[0], head());
        assert!(pk[1].starts_with(b"OpusTags"));
        assert_eq!(&pk[2..], &packets[..]);
    }

    #[test]
    fn big_cover_spans_pages_and_lofty_reads_tags() {
        let packets: Vec<Vec<u8>> = (0..100).map(|_| vec![0xFC, 1, 2, 3, 4, 5, 6, 7]).collect();
        let webm = writer::opus_webm(&head(), 0, &packets, 20);
        let cover = Cover { jpeg: tiny_jpeg(), width: 8, height: 8 };
        let mut big = cover.clone();
        big.jpeg.extend(std::iter::repeat_n(0u8, 150_000)); // forces a multi-page comment header
        let tags = ExportTags {
            title: "Tiêu đề".into(),
            artist: Some("Ca sĩ A, Ca sĩ B".into()),
            album: Some("Album".into()),
            album_artist: Some("Ca sĩ A".into()),
            year: Some(2024),
            lyrics: Some("[00:01.00]line one\n[00:02.00]line two".into()),
            cover: Some(big),
        };
        let ogg_bytes = webm_to_ogg_opus(&webm, &tags).unwrap();
        let pages = ogg::read_pages(&ogg_bytes).unwrap();
        assert!(pages.iter().all(|p| p.crc_ok));
        let pk = ogg::packets(&pages);
        assert_eq!(&pk[2..], &packets[..]);
        // Audio starts on a fresh page: the page holding the end of OpusTags has granule 0.
        let tags_end = pages.iter().position(|p| p.granule == 0 && p.sequence > 0).unwrap();
        assert!(pages[1..tags_end].iter().all(|p| p.granule == ogg::NO_GRANULE));

        let dir = std::env::temp_dir().join(format!("mochy-dl-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remux.opus");
        std::fs::write(&path, &ogg_bytes).unwrap();
        let back = crate::tags::read_with_lofty(&path).unwrap();
        assert_eq!(back.title.as_deref(), Some("Tiêu đề"));
        assert_eq!(back.artist.as_deref(), Some("Ca sĩ A, Ca sĩ B"));
        assert_eq!(back.album.as_deref(), Some("Album"));
        assert_eq!(back.album_artist.as_deref(), Some("Ca sĩ A"));
        assert_eq!(back.year.as_deref(), Some("2024"));
        assert!(back.lyrics.unwrap().contains("line two"));
        assert_eq!(back.cover_bytes, Some(tiny_jpeg().len() + 150_000));
        // 100 × 20 ms minus the 312-sample pre-skip
        assert!(back.duration_ms.abs_diff((100 * 960 - 312) * 1000 / 48_000) <= 1);

        // Retag the same file through lofty (the song-edit path) and read it back.
        let retag = ExportTags { title: "New".into(), artist: Some("X".into()), cover: Some(cover), ..Default::default() };
        crate::tags::write_with_lofty(&path, &retag).unwrap();
        let back = crate::tags::read_with_lofty(&path).unwrap();
        assert_eq!(back.title.as_deref(), Some("New"));
        assert_eq!(back.artist.as_deref(), Some("X"));
        assert_eq!(back.album, None);
        assert_eq!(back.cover_bytes, Some(tiny_jpeg().len()));
        let pages = ogg::read_pages(&std::fs::read(&path).unwrap()).unwrap();
        assert!(pages.iter().all(|p| p.crc_ok));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_opus_tracks() {
        let mut webm = writer::opus_webm(&head(), 0, &[vec![0xFC]], 1);
        let pos = webm.windows(6).position(|w| w == b"A_OPUS").unwrap();
        webm[pos..pos + 6].copy_from_slice(b"A_VORB");
        assert_eq!(webm_to_ogg_opus(&webm, &ExportTags::default()).unwrap_err(), RemuxError::NotOpus("A_VORB".into()));
    }

    fn tiny_jpeg() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30]));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img).unwrap();
        out
    }
}
