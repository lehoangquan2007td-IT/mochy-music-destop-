//! Container detection from a file's first bytes (port of
//! `DownloadExporter.extensionOf`): read off the bytes rather than the
//! stream's declared format.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    WebM,
    Mp4,
    OggOpus,
    Ogg,
    Flac,
    Mp3,
    Wav,
    Unknown,
}

impl Container {
    /// File extension the container is saved under. WebM is listed for
    /// completeness: Opus in WebM is remuxed to `.opus` before saving.
    pub fn extension(self) -> &'static str {
        match self {
            Container::WebM => "webm",
            Container::Mp4 | Container::Unknown => "m4a",
            Container::OggOpus => "opus",
            Container::Ogg => "ogg",
            Container::Flac => "flac",
            Container::Mp3 => "mp3",
            Container::Wav => "wav",
        }
    }
}

pub fn sniff(head: &[u8]) -> Container {
    let at = |offset: usize, text: &[u8]| head.len() >= offset + text.len() && &head[offset..offset + text.len()] == text;
    if head.len() < 4 {
        return Container::Unknown;
    }
    if at(0, b"fLaC") {
        Container::Flac
    } else if at(0, b"OggS") {
        if head.windows(8).take(64).any(|w| w == b"OpusHead") { Container::OggOpus } else { Container::Ogg }
    } else if at(0, b"ID3") || (head[0] == 0xFF && head[1] & 0xE0 == 0xE0) {
        Container::Mp3
    } else if at(0, b"RIFF") {
        Container::Wav
    } else if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        Container::WebM
    } else if at(4, b"ftyp") {
        Container::Mp4
    } else {
        Container::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_containers() {
        assert_eq!(sniff(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F]), Container::WebM);
        assert_eq!(sniff(b"\0\0\0\x18ftypdash"), Container::Mp4);
        assert_eq!(sniff(b"fLaC\0\0\0\x22"), Container::Flac);
        assert_eq!(sniff(b"ID3\x04\0"), Container::Mp3);
        let mut ogg = b"OggS\0\x02".to_vec();
        ogg.extend([0u8; 22]);
        ogg.extend(b"OpusHead");
        assert_eq!(sniff(&ogg), Container::OggOpus);
        assert_eq!(sniff(b"OggS\0\x02\0\0\x01vorbis"), Container::Ogg);
        assert_eq!(sniff(b"xy"), Container::Unknown);
        assert_eq!(Container::OggOpus.extension(), "opus");
        assert_eq!(Container::Mp4.extension(), "m4a");
    }
}
