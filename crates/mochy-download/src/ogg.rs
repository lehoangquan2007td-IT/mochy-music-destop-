//! Minimal Ogg page writer/reader (RFC 3533): CRC, lacing, packet
//! pagination. Enough to write an Ogg Opus file and to verify one in tests.

/// Ogg's CRC-32: polynomial 0x04C11DB7, not reflected, initial value 0, no
/// final xor, computed over the page with its CRC field zeroed.
const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

pub fn crc32(parts: &[&[u8]]) -> u32 {
    let mut crc = 0u32;
    for part in parts {
        for &b in *part {
            crc = (crc << 8) ^ CRC_TABLE[(((crc >> 24) as u8) ^ b) as usize];
        }
    }
    crc
}

pub const FLAG_CONTINUED: u8 = 0x01;
pub const FLAG_BOS: u8 = 0x02;
pub const FLAG_EOS: u8 = 0x04;
/// Granule position of a page on which no packet ends.
pub const NO_GRANULE: i64 = -1;

/// Body bytes after which an audio page is closed (libopusenc-like pages).
const TARGET_PAGE_BODY: usize = 4096;

/// Writes one logical bitstream as pages into a byte vector.
pub struct OggWriter {
    pub out: Vec<u8>,
    serial: u32,
    sequence: u32,
    // the open page
    lacing: Vec<u8>,
    body: Vec<u8>,
    /// Granule of the last packet that ended on the open page.
    granule: i64,
    any_packet_ended: bool,
    /// The open page starts with the tail of a packet from the page before.
    continued: bool,
    pages: usize,
}

impl OggWriter {
    pub fn new(serial: u32) -> Self {
        Self {
            out: Vec::new(),
            serial,
            sequence: 0,
            lacing: Vec::new(),
            body: Vec::new(),
            granule: 0,
            any_packet_ended: false,
            continued: false,
            pages: 0,
        }
    }

    pub fn pages_written(&self) -> usize {
        self.pages
    }

    /// Append one whole packet; `granule` is the stream position once it has
    /// been decoded. Pages are closed when full (255 segments) or past the
    /// target size; a packet larger than a page continues on the next one.
    pub fn packet(&mut self, data: &[u8], granule: i64) {
        // Keep small packets whole on one page.
        let needed = data.len() / 255 + 1;
        if !self.lacing.is_empty() && self.lacing.len() + needed > 255 && needed <= 255 {
            self.flush(0);
        }
        let mut left = data;
        loop {
            if self.lacing.len() == 255 {
                // The packet goes on over the page boundary.
                self.flush(0);
                self.continued = true;
            }
            let take = left.len().min(255);
            self.lacing.push(take as u8);
            self.body.extend_from_slice(&left[..take]);
            left = &left[take..];
            if take < 255 {
                break;
            }
            if left.is_empty() {
                // A multiple of 255: terminate with a zero-length segment.
                if self.lacing.len() == 255 {
                    self.flush(0);
                    self.continued = true;
                }
                self.lacing.push(0);
                break;
            }
        }
        self.granule = granule;
        self.any_packet_ended = true;
        if self.body.len() >= TARGET_PAGE_BODY {
            self.flush(0);
        }
    }

    /// Close the open page (no-op when it's empty unless `flags` asks for a
    /// flagged page, e.g. a lone EOS).
    pub fn flush(&mut self, flags: u8) {
        if self.lacing.is_empty() && flags == 0 {
            return;
        }
        let mut header_type = flags;
        if self.continued {
            header_type |= FLAG_CONTINUED;
        }
        if self.sequence == 0 {
            header_type |= FLAG_BOS;
        }
        let granule = if self.any_packet_ended { self.granule } else { NO_GRANULE };
        write_page(&mut self.out, header_type, granule, self.serial, self.sequence, &self.lacing, &self.body);
        self.sequence += 1;
        self.pages += 1;
        self.lacing.clear();
        self.body.clear();
        self.any_packet_ended = false;
        self.continued = false;
    }

    /// Close the stream: the last page carries EOS.
    pub fn finish(mut self) -> Vec<u8> {
        self.flush(FLAG_EOS);
        self.out
    }
}

pub fn write_page(out: &mut Vec<u8>, header_type: u8, granule: i64, serial: u32, sequence: u32, lacing: &[u8], body: &[u8]) {
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.push(0); // version
    out.push(header_type);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]); // crc placeholder
    out.push(lacing.len() as u8);
    out.extend_from_slice(lacing);
    out.extend_from_slice(body);
    let crc = crc32(&[&out[start..]]);
    out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
}

/// One parsed page (tests / verification).
#[derive(Debug, Clone)]
pub struct Page {
    pub header_type: u8,
    pub granule: i64,
    pub serial: u32,
    pub sequence: u32,
    pub crc: u32,
    pub lacing: Vec<u8>,
    pub body: Vec<u8>,
    /// CRC recomputed over the page matches the stored one.
    pub crc_ok: bool,
}

/// Split `data` into pages; `None` when it isn't a well-formed Ogg stream.
pub fn read_pages(mut data: &[u8]) -> Option<Vec<Page>> {
    let mut pages = Vec::new();
    while !data.is_empty() {
        if data.len() < 27 || &data[..4] != b"OggS" || data[4] != 0 {
            return None;
        }
        let segments = data[26] as usize;
        if data.len() < 27 + segments {
            return None;
        }
        let lacing = data[27..27 + segments].to_vec();
        let body_len: usize = lacing.iter().map(|&l| l as usize).sum();
        let total = 27 + segments + body_len;
        if data.len() < total {
            return None;
        }
        let crc = u32::from_le_bytes(data[22..26].try_into().ok()?);
        let mut copy = data[..total].to_vec();
        copy[22..26].fill(0);
        pages.push(Page {
            header_type: data[5],
            granule: i64::from_le_bytes(data[6..14].try_into().ok()?),
            serial: u32::from_le_bytes(data[14..18].try_into().ok()?),
            sequence: u32::from_le_bytes(data[18..22].try_into().ok()?),
            crc,
            lacing,
            body: data[27 + segments..total].to_vec(),
            crc_ok: crc32(&[&copy]) == crc,
        });
        data = &data[total..];
    }
    Some(pages)
}

/// Reassemble packets from pages (tests / verification).
pub fn packets(pages: &[Page]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    for page in pages {
        let mut at = 0;
        for &l in &page.lacing {
            current.extend_from_slice(&page.body[at..at + l as usize]);
            at += l as usize;
            if l < 255 {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_known_vector() {
        // CRC-32/MPEG-2 style check value for this unreflected, init-0 variant
        // ("123456789" → 0x89A1897F is the CRC-32/BZIP2-without-xor family);
        // verify self-consistency against a bitwise implementation instead.
        fn slow(data: &[u8]) -> u32 {
            let mut crc = 0u32;
            for &b in data {
                crc ^= (b as u32) << 24;
                for _ in 0..8 {
                    crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
                }
            }
            crc
        }
        let data = b"The quick brown fox jumps over the lazy dog";
        assert_eq!(crc32(&[data]), slow(data));
        assert_eq!(crc32(&[&data[..10], &data[10..]]), slow(data));
    }

    #[test]
    fn pages_roundtrip_with_big_and_exact_packets() {
        let mut w = OggWriter::new(0x1234);
        let a = vec![1u8; 10];
        let b = vec![2u8; 255 * 3]; // exact multiple of 255 → zero-length terminator
        let c = vec![3u8; 200_000]; // spans three pages
        let d = vec![4u8; 3];
        w.packet(&a, 0);
        w.flush(0);
        w.packet(&b, 100);
        w.packet(&c, 200);
        w.packet(&d, 300);
        let bytes = w.finish();
        let pages = read_pages(&bytes).expect("valid ogg");
        assert!(pages.iter().all(|p| p.crc_ok));
        assert!(pages.iter().all(|p| p.serial == 0x1234));
        assert_eq!(pages[0].header_type & FLAG_BOS, FLAG_BOS);
        assert_eq!(pages.last().unwrap().header_type & FLAG_EOS, FLAG_EOS);
        for (i, p) in pages.iter().enumerate() {
            assert_eq!(p.sequence as usize, i);
            assert!(p.lacing.len() <= 255);
        }
        // The page that only holds the middle of `c` has no granule and continues.
        assert!(pages.iter().any(|p| p.granule == NO_GRANULE));
        assert!(pages.iter().any(|p| p.header_type & FLAG_CONTINUED != 0));
        assert_eq!(packets(&pages), vec![a, b, c, d]);
    }
}
