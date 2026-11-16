//! Auto-sync of *plain* lyrics to a video's captions.
//!
//! Android's `CaptionAligner` finds where on a video each lyric line is sung by reading the
//! video's captions (letter n-grams, a chorus finds every place it's sung, the best forward
//! path wins). Android uses it to move timed lyrics onto a music video; desktop uses the same
//! matching to give untimed lyrics (e.g. YouTube Music's own lyrics tab) line timings: every
//! line found in the captions gets the moment it's sung, the lines in between are spread
//! evenly between their neighbours. Too few lines found ⇒ no result, the lyrics stay plain.
//!
//! (Android's audio-based `LyricsAutoSync`/`VocalActivity` offset finder is not ported: it
//! decodes the whole song a second time.)

use std::collections::{HashMap, HashSet};

use unicode_normalization::UnicodeNormalization;

use crate::{LyricLine, Lyrics, SyncKind};

/// One caption piece timed on the video (Android `CaptionCue`).
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionCue {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

const MAX_GRAM_REPEATS: usize = 60;
const MAX_MS_PER_LETTER: u64 = 250;
const MIN_LINE_SCORE: f32 = 0.34;
const MAX_CANDIDATES: usize = 4;
const MIN_ANCHORS: usize = 3;
const MIN_MATCHED_SHARE: f32 = 0.3;

fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' => {
                depth += 1;
                out.push(' ');
            }
            ']' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            '♪' => out.push(' '),
            _ => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn as_u64(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Timed-text JSON (`fmt=json3`) as cues, split wherever YouTube timed a piece on its own, so
/// auto captions keep their word timing (Android `timedTextToCues`).
pub fn timed_text_to_cues(body: &str) -> Vec<CaptionCue> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else { return Vec::new() };
    let Some(events) = json["events"].as_array() else { return Vec::new() };
    let mut cues = Vec::new();
    for e in events {
        let Some(start) = as_u64(&e["tStartMs"]) else { continue };
        let end = start + as_u64(&e["dDurationMs"]).unwrap_or(0);
        let Some(segs) = e["segs"].as_array() else { continue };
        // (start, text); a seg without its own offset belongs to the piece before it
        let mut pieces: Vec<(u64, String)> = Vec::new();
        for seg in segs {
            let text = seg["utf8"].as_str().unwrap_or("");
            match as_u64(&seg["tOffsetMs"]) {
                Some(off) => pieces.push((start + off, text.to_string())),
                None if pieces.is_empty() => pieces.push((start, text.to_string())),
                None => pieces.last_mut().expect("non-empty").1.push_str(text),
            }
        }
        for (i, (s, raw)) in pieces.iter().enumerate() {
            let text = clean(raw);
            if text.is_empty() {
                continue;
            }
            let piece_end = pieces.get(i + 1).map(|p| p.0).unwrap_or(end);
            cues.push(CaptionCue { start_ms: *s, end_ms: piece_end.max(*s), text });
        }
    }
    cues.sort_by_key(|c| c.start_ms);
    cues
}

/// Lowercase letters and digits only, katakana folded into hiragana (Android `normalize`).
pub fn normalize(text: &str) -> Vec<char> {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .map(|c| if ('ァ'..='ヶ').contains(&c) { char::from_u32(c as u32 - 0x60).unwrap_or(c) } else { c })
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0x3400..=0x4DBF)
}

/// Every caption letter in order, each with the moment it's on screen.
struct Stream {
    text: Vec<char>,
    times: Vec<u64>,
}

impl Stream {
    fn of(cues: &[CaptionCue]) -> Option<Stream> {
        let mut text = Vec::new();
        let mut times = Vec::new();
        for cue in cues {
            let letters = normalize(&cue.text);
            if letters.is_empty() {
                continue;
            }
            // capped, so a caption held long after it's sung doesn't push its letters late
            let per = (cue.end_ms.saturating_sub(cue.start_ms) / letters.len() as u64).min(MAX_MS_PER_LETTER);
            for (k, l) in letters.into_iter().enumerate() {
                text.push(l);
                times.push(cue.start_ms + per * k as u64);
            }
        }
        (text.len() >= 20).then_some(Stream { text, times })
    }

    fn mostly_cjk(&self) -> bool {
        self.text.iter().filter(|c| is_cjk(**c)).count() * 10 >= self.text.len() * 4
    }

    /// When a line found at `pos` starts. A misheard word shifts the match a letter or two, and
    /// a line found a letter early would take the time of the previous caption's last letter
    /// (seconds early across a pause): a pause right after `pos` means the line starts after it.
    fn line_time_at(&self, pos: i64) -> u64 {
        let p = pos.clamp(0, self.times.len() as i64 - 1) as usize;
        for d in 1..=2 {
            if let (Some(&a), Some(&b)) = (self.times.get(p + d - 1), self.times.get(p + d))
                && b.saturating_sub(a) > 1_000
            {
                return b;
            }
        }
        self.times[p]
    }

    /// Where each run of `size` letters appears; the very common ones say nothing.
    fn gram_index(&self, size: usize) -> HashMap<&[char], Vec<i64>> {
        let mut m: HashMap<&[char], Vec<i64>> = HashMap::new();
        if self.text.len() >= size {
            for start in 0..=self.text.len() - size {
                m.entry(&self.text[start..start + size]).or_default().push(start as i64);
            }
        }
        m.retain(|_, v| v.len() <= MAX_GRAM_REPEATS);
        m
    }
}

/// Where in the captions a line could start, with the share of its letter runs found there
/// (Android `candidates`).
fn candidates(line: &[char], gram: usize, index: &HashMap<&[char], Vec<i64>>) -> Vec<(i64, f32)> {
    let grams = line.len() + 1 - gram;
    let bucket = 4.max(line.len() / 4) as i64;
    let mut votes: HashMap<i64, usize> = HashMap::new();
    let mut starts: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut seen = HashSet::new();
    for k in 0..grams {
        let Some(positions) = index.get(&line[k..k + gram]) else { continue };
        seen.clear();
        for &p in positions {
            let start = p - k as i64;
            let center = start.div_euclid(bucket);
            // the neighbouring buckets too: captions and lyrics rarely spell a line the same length
            for b in center - 1..=center + 1 {
                if seen.insert(b) {
                    *votes.entry(b).or_default() += 1;
                }
            }
            starts.entry(center).or_default().push(start);
        }
    }
    let mut ranked: Vec<(i64, usize)> = votes.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut picked = Vec::new();
    let mut taken: Vec<i64> = Vec::new();
    for (b, count) in ranked {
        let score = count as f32 / grams as f32;
        if score < MIN_LINE_SCORE || picked.len() >= MAX_CANDIDATES {
            break;
        }
        if taken.iter().any(|t| (t - b).abs() <= 2) {
            continue;
        }
        let mut near: Vec<i64> = (b - 1..=b + 1).flat_map(|x| starts.get(&x).cloned().unwrap_or_default()).collect();
        if near.is_empty() {
            continue;
        }
        near.sort_unstable();
        taken.push(b);
        picked.push((near[near.len() / 2], score.min(1.0)));
    }
    picked
}

/// The time (ms) each line is sung at: `Some` for lines found in the captions, `None` for the
/// rest. `None` overall when too few lines were found to trust any of it.
pub fn align_plain<S: AsRef<str>>(lines: &[S], cues: &[CaptionCue]) -> Option<Vec<Option<u64>>> {
    let stream = Stream::of(cues)?;
    let gram = if stream.mostly_cjk() { 2 } else { 3 };
    let index = stream.gram_index(gram);
    // (line, position, score)
    let mut nodes: Vec<(usize, i64, f32)> = Vec::new();
    let mut usable = 0usize;
    for (i, l) in lines.iter().enumerate() {
        let text = normalize(l.as_ref());
        if text.len() < gram + 2 {
            continue;
        }
        usable += 1;
        for (pos, score) in candidates(&text, gram, &index) {
            nodes.push((i, pos, score));
        }
    }
    if nodes.is_empty() {
        return None;
    }
    // best path that only ever moves forward, in lines and in the captions
    nodes.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut total: Vec<f32> = nodes.iter().map(|n| n.2).collect();
    let mut prev: Vec<Option<usize>> = vec![None; nodes.len()];
    for i in 0..nodes.len() {
        for j in 0..i {
            if nodes[j].0 < nodes[i].0 && nodes[j].1 < nodes[i].1 && total[j] + nodes[i].2 > total[i] {
                total[i] = total[j] + nodes[i].2;
                prev[i] = Some(j);
            }
        }
    }
    let mut at = (0..nodes.len()).max_by(|a, b| total[*a].total_cmp(&total[*b]));
    let mut out = vec![None; lines.len()];
    let mut anchors = 0usize;
    while let Some(i) = at {
        out[nodes[i].0] = Some(stream.line_time_at(nodes[i].1));
        anchors += 1;
        at = prev[i];
    }
    if anchors < MIN_ANCHORS || (anchors as f32) < usable as f32 * MIN_MATCHED_SHARE {
        return None;
    }
    Some(out)
}

/// Fill the lines between anchors: spread evenly between the anchors around them; before the
/// first and after the last, one average line gap apart.
pub fn fill_times(anchors: &[Option<u64>]) -> Option<Vec<u64>> {
    let known: Vec<(usize, u64)> = anchors.iter().enumerate().filter_map(|(i, t)| t.map(|t| (i, t))).collect();
    let (&(first_i, first_t), &(last_i, last_t)) = (known.first()?, known.last()?);
    let avg = if last_i > first_i { (last_t.saturating_sub(first_t)) / (last_i - first_i) as u64 } else { 3_000 };
    let avg = avg.clamp(800, 8_000);
    let mut out = vec![0u64; anchors.len()];
    for w in known.windows(2) {
        let ((a, ta), (b, tb)) = (w[0], w[1]);
        for i in a..=b {
            out[i] = ta + (tb.saturating_sub(ta)) * (i - a) as u64 / (b - a).max(1) as u64;
        }
    }
    out[first_i] = first_t;
    out[last_i] = last_t;
    for i in (0..first_i).rev() {
        out[i] = out[i + 1].saturating_sub(avg);
    }
    for i in last_i + 1..anchors.len() {
        out[i] = out[i - 1] + avg;
    }
    Some(out)
}

/// Plain lyrics timed from captions, as line-synced lyrics; `None` when they don't line up.
pub fn sync_plain_lyrics(lyrics: &Lyrics, cues: &[CaptionCue]) -> Option<Lyrics> {
    if lyrics.sync != SyncKind::Plain || lyrics.lines.is_empty() {
        return None;
    }
    let texts: Vec<&str> = lyrics.lines.iter().map(|l| l.text.as_str()).collect();
    let times = fill_times(&align_plain(&texts, cues)?)?;
    let lines: Vec<LyricLine> = lyrics
        .lines
        .iter()
        .zip(&times)
        .map(|(l, t)| LyricLine { time: *t as f64 / 1000.0, words: Vec::new(), ..l.clone() })
        .collect();
    Some(Lyrics { provider: lyrics.provider.clone(), sync: SyncKind::Line, lines, plain: None })
}

/// Fetch the video's captions and time `lyrics` (plain) with them.
pub async fn sync_plain_to_video(lyrics: &Lyrics, video_id: &str, ytm: Option<&mochy_innertube::YtmClient>) -> Option<Lyrics> {
    if lyrics.sync != SyncKind::Plain {
        return None;
    }
    let cues = crate::providers::youtube::caption_cues(ytm, video_id).await?;
    sync_plain_lyrics(lyrics, &cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Invented placeholder lines (not real lyrics).
    const LINES: [&str; 6] = [
        "paper boats are sailing down the river",
        "lanterns floating softly in the evening",
        "counting every window on the hill",
        "yellow kites above the quiet harbor",
        "paper boats are sailing down the river",
        "morning comes with bread and coffee",
    ];

    /// Auto-caption-like cues: a couple of words each, lyric i sung at 10 s + 6 s·i,
    /// with a few words misheard.
    fn cues() -> Vec<CaptionCue> {
        let mut out = Vec::new();
        for (i, line) in LINES.iter().enumerate() {
            let words: Vec<&str> = line.split(' ').collect();
            let t0 = 10_000 + 6_000 * i as u64;
            for (k, pair) in words.chunks(2).enumerate() {
                let mut text = pair.join(" ");
                if i == 3 && k == 0 {
                    text = "yellow kids".into(); // misheard
                }
                let s = t0 + 900 * k as u64;
                out.push(CaptionCue { start_ms: s, end_ms: s + 900, text });
            }
        }
        out.insert(0, CaptionCue { start_ms: 1_000, end_ms: 3_000, text: "music".into() });
        out
    }

    #[test]
    fn json3_cues_split_on_offsets() {
        let body = r#"{"events":[
            {"tStartMs":1000,"dDurationMs":2000,"segs":[{"utf8":"paper"},{"utf8":" boats","tOffsetMs":500},{"utf8":"!"}]},
            {"tStartMs":2500,"segs":[{"utf8":"\n"}]},
            {"tStartMs":4000,"dDurationMs":1000,"segs":[{"utf8":"[Music]"}]}
        ]}"#;
        let c = timed_text_to_cues(body);
        assert_eq!(
            c,
            vec![
                CaptionCue { start_ms: 1000, end_ms: 1500, text: "paper".into() },
                CaptionCue { start_ms: 1500, end_ms: 3000, text: "boats!".into() },
            ]
        );
    }

    #[test]
    fn normalize_folds() {
        assert_eq!(normalize("Ｐａｐｅｒ, Boats!").into_iter().collect::<String>(), "paperboats");
        assert_eq!(normalize("カタ かな").into_iter().collect::<String>(), "かたかな");
    }

    #[test]
    fn plain_lines_get_caption_times_in_order() {
        let anchors = align_plain(&LINES, &cues()).expect("aligned");
        for (i, t) in anchors.iter().enumerate() {
            if let Some(t) = t {
                let expect = 10_000 + 6_000 * i as u64;
                assert!(t.abs_diff(expect) < 1_000, "line {i}: {t} vs {expect}");
            }
        }
        // the repeated line is placed at its own (second) occurrence, not the first
        assert!(anchors[4].unwrap() > anchors[2].unwrap());
        let times = fill_times(&anchors).unwrap();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn unrelated_captions_give_nothing() {
        let other = vec![CaptionCue { start_ms: 0, end_ms: 60_000, text: "completely different words spoken here at length about weather".into() }];
        assert!(align_plain(&LINES, &other).is_none());
        assert!(align_plain(&LINES, &[]).is_none());
    }

    #[test]
    fn fill_spreads_between_anchors() {
        let t = fill_times(&[None, Some(10_000), None, None, Some(16_000), None]).unwrap();
        assert_eq!(t, vec![8_000, 10_000, 12_000, 14_000, 16_000, 18_000]);
        assert!(fill_times(&[None, None]).is_none());
    }

    #[test]
    fn sync_plain_lyrics_makes_line_synced() {
        let plain = Lyrics::from_lrc("YouTube Music", &LINES.join("\n")).unwrap();
        assert_eq!(plain.sync, SyncKind::Plain);
        let synced = sync_plain_lyrics(&plain, &cues()).unwrap();
        assert_eq!(synced.sync, SyncKind::Line);
        assert_eq!(synced.lines.len(), LINES.len());
        assert!(synced.lines[0].time > 5.0);
        // already-timed lyrics are left alone
        assert!(sync_plain_lyrics(&synced, &cues()).is_none());
    }
}
