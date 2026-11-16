//! What goes into a downloaded file's own tags (port of Android
//! `ExportTags` / `ExportTagWriter`): title, one joined artist field, album,
//! album artist (the lead artist, so a guest on one song doesn't split the
//! album), year, lyrics (as LRC) and a front-cover JPEG.
//!
//! Ogg Opus gets the vorbis comment packet written by the remux itself
//! ([`vorbis_comments`], cover as `METADATA_BLOCK_PICTURE`); every other
//! container (MP4/M4A, FLAC, MP3, plain Ogg) is tagged through lofty
//! ([`write_with_lofty`]), which is also used to retag a file after a song
//! edit.

use std::path::Path;

use base64::Engine;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportTags {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<i32>,
    pub lyrics: Option<String>,
    pub cover: Option<Cover>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl ExportTags {
    /// Tags for a song the way the Android export builds them: artists
    /// joined with ", ", album artist = first artist when there is an album.
    pub fn for_song(title: &str, artists: &[String], album: Option<&str>, year: Option<i32>) -> Self {
        let album = album.map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
        let artist = Some(artists.join(", ")).filter(|a| !a.trim().is_empty());
        Self {
            title: title.to_string(),
            album_artist: album.as_ref().and_then(|_| artists.first().cloned()),
            artist,
            album,
            year,
            lyrics: None,
            cover: None,
        }
    }
}

fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn be32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// The vorbis comment body Opus and FLAC share (vendor "Mochy").
pub fn vorbis_comments(tags: &ExportTags) -> Vec<u8> {
    let mut fields: Vec<String> = vec![format!("TITLE={}", tags.title)];
    if let Some(a) = &tags.artist {
        fields.push(format!("ARTIST={a}"));
    }
    if let Some(a) = &tags.album {
        fields.push(format!("ALBUM={a}"));
    }
    if let Some(a) = &tags.album_artist {
        fields.push(format!("ALBUMARTIST={a}"));
    }
    if let Some(y) = tags.year {
        fields.push(format!("DATE={y}"));
    }
    if let Some(l) = &tags.lyrics {
        fields.push(format!("LYRICS={l}"));
    }
    if let Some(c) = &tags.cover {
        fields.push(format!("METADATA_BLOCK_PICTURE={}", base64::engine::general_purpose::STANDARD.encode(flac_picture(c))));
    }
    let mut out = Vec::new();
    let vendor = b"Mochy";
    le32(&mut out, vendor.len() as u32);
    out.extend_from_slice(vendor);
    le32(&mut out, fields.len() as u32);
    for f in fields {
        le32(&mut out, f.len() as u32);
        out.extend_from_slice(f.as_bytes());
    }
    out
}

/// FLAC's picture block (what Opus carries base64'd): a front-cover JPEG.
pub fn flac_picture(cover: &Cover) -> Vec<u8> {
    let mut out = Vec::new();
    let mime = b"image/jpeg";
    be32(&mut out, 3); // front cover
    be32(&mut out, mime.len() as u32);
    out.extend_from_slice(mime);
    be32(&mut out, 0); // description
    be32(&mut out, cover.width);
    be32(&mut out, cover.height);
    be32(&mut out, 24);
    be32(&mut out, 0);
    be32(&mut out, cover.jpeg.len() as u32);
    out.extend_from_slice(&cover.jpeg);
    out
}

/// Write `tags` into an existing file with lofty (any container lofty can
/// write). Existing title/artist/album/… are replaced, other items kept;
/// the front cover is replaced when `tags.cover` is set.
pub fn write_with_lofty(path: &Path, tags: &ExportTags) -> Result<(), String> {
    let mut tagged = lofty::read_from_path(path).map_err(|e| format!("read tags: {e}"))?;
    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().ok_or("no writable tag")?;
    tag.set_title(tags.title.clone());
    match &tags.artist {
        Some(a) => tag.set_artist(a.clone()),
        None => tag.remove_artist(),
    }
    match &tags.album {
        Some(a) => tag.set_album(a.clone()),
        None => tag.remove_album(),
    }
    tag.remove_key(ItemKey::AlbumArtist);
    if let Some(a) = &tags.album_artist {
        tag.insert_text(ItemKey::AlbumArtist, a.clone());
    }
    tag.remove_key(ItemKey::RecordingDate);
    tag.remove_key(ItemKey::Year);
    if let Some(y) = tags.year {
        tag.insert_text(ItemKey::RecordingDate, y.to_string());
    }
    if let Some(l) = &tags.lyrics {
        tag.remove_key(ItemKey::Lyrics);
        tag.insert_text(ItemKey::Lyrics, l.clone());
    }
    if let Some(c) = &tags.cover {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(c.jpeg.clone()).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build(),
        );
    }
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| format!("write tags: {e}"))?;
    Ok(())
}

/// What lofty reads back (tests / the song-edit retag check).
#[derive(Debug, Default, PartialEq)]
pub struct ReadBack {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub lyrics: Option<String>,
    pub cover_bytes: Option<usize>,
    pub duration_ms: u128,
}

pub fn read_with_lofty(path: &Path) -> Result<ReadBack, String> {
    let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let duration_ms = tagged.properties().duration().as_millis();
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(ReadBack { duration_ms, ..Default::default() });
    };
    Ok(ReadBack {
        title: tag.title().map(|s| s.to_string()),
        artist: tag.artist().map(|s| s.to_string()),
        album: tag.album().map(|s| s.to_string()),
        album_artist: tag.get_string(ItemKey::AlbumArtist).map(str::to_string),
        year: tag.get_string(ItemKey::RecordingDate).or_else(|| tag.get_string(ItemKey::Year)).map(str::to_string),
        lyrics: tag.get_string(ItemKey::Lyrics).map(str::to_string),
        cover_bytes: tag.pictures().first().map(|p| p.data().len()),
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_song_matches_android_export() {
        let t = ExportTags::for_song("T", &["A".into(), "B".into()], Some("Al"), Some(2020));
        assert_eq!(t.artist.as_deref(), Some("A, B"));
        assert_eq!(t.album_artist.as_deref(), Some("A"));
        let t = ExportTags::for_song("T", &[], Some("  "), None);
        assert_eq!(t.artist, None);
        assert_eq!(t.album, None);
        assert_eq!(t.album_artist, None);
    }

    #[test]
    fn vorbis_comment_layout() {
        let t = ExportTags { title: "Hi".into(), artist: Some("Me".into()), ..Default::default() };
        let v = vorbis_comments(&t);
        assert_eq!(&v[..4], &5u32.to_le_bytes());
        assert_eq!(&v[4..9], b"Mochy");
        assert_eq!(&v[9..13], &2u32.to_le_bytes());
        assert_eq!(&v[13..17], &8u32.to_le_bytes());
        assert_eq!(&v[17..25], b"TITLE=Hi");
    }
}
