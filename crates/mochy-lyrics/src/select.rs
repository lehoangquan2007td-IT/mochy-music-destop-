//! Candidate grading and winner selection — a port of Android `LyricsHelper.pickBest`
//! (spec §3.3).
//!
//! Score = `script * 10000 + sync * 1000 + duet * 100 + order`, where
//! - **script** (0‥3) says how likely the result is in the song's own language (decided from
//!   the title/artist/album script, YouTube's own lyrics, or the majority of sources);
//! - **sync** is the [`SyncKind`] ordinal (word 3 > line 2 > plain 1), syllable-split word
//!   timings demoted to line;
//! - **duet** is 2 for real two-sided voices, 1 for voice tags on a single voice;
//! - **order** is `provider_count - index` (the Chinese services get `2 * count - index` for a
//!   Chinese song, so they win ties outright).
//!
//! The Chinese services (KuGou, NetEase, QQ Music) and YouTube's auto captions only take part
//! when better sources have nothing (tiered pool, `eligible`).

use std::collections::{HashMap, HashSet};

use mochy_core::models::TrackMeta;

use crate::script::{
    Script, dominant_script, has_distinct_voices, has_voice_tags, is_syllable_split,
    latin_language, lyrics_script,
};
use crate::{LYRICS_NOT_FOUND, ProviderId, SyncKind, parse_lrc};

pub const SCRIPT_WEIGHT: i64 = 10_000;
pub const SYNC_WEIGHT: i64 = 1_000;
pub const DUET_WEIGHT: i64 = 100;

const LATIN_PREFIX: &str = "latin:";

/// One provider's graded answer.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub provider: ProviderId,
    /// Raw payload in the Mochy LRC dialect, exactly as the provider produced it.
    pub raw: String,
    /// Index of the provider in the full configured order.
    pub order: usize,
    pub sync: SyncKind,
    pub script: Script,
    /// Latin-script language when it can be told ("en", "vi", …).
    pub language: Option<&'static str>,
}

impl Candidate {
    /// Grade a payload. `None` when it is blank, the not-found sentinel, or has no lyrics.
    pub fn new(provider: ProviderId, raw: impl Into<String>, order: usize) -> Option<Candidate> {
        let raw = raw.into();
        if raw.trim().is_empty() || raw.trim() == LYRICS_NOT_FOUND {
            return None;
        }
        let (_, sync) = parse_lrc(&raw);
        if sync == SyncKind::None {
            return None;
        }
        Some(Candidate {
            provider,
            script: lyrics_script(&raw),
            language: latin_language(&raw),
            sync,
            raw,
            order,
        })
    }

    /// Real left/right voices.
    pub fn has_duet(&self) -> bool {
        has_distinct_voices(&self.raw)
    }

    fn language_key(&self) -> String {
        if self.script == Script::Latin {
            match self.language {
                Some(l) => format!("{LATIN_PREFIX}{l}"),
                None => Script::Latin.name().to_string(),
            }
        } else {
            self.script.name().to_string()
        }
    }
}

pub(crate) fn sync_ordinal(s: SyncKind) -> i64 {
    match s {
        SyncKind::None => 0,
        SyncKind::Plain => 1,
        SyncKind::Line => 2,
        SyncKind::Word => 3,
    }
}

fn is_chinese_source(p: ProviderId) -> bool {
    matches!(
        p,
        ProviderId::Kugou | ProviderId::NetEase | ProviderId::QQMusic
    )
}

fn is_language_reference(p: ProviderId) -> bool {
    matches!(p, ProviderId::YouTubeMusic | ProviderId::YouTubeSubtitle)
}

fn same_language_family(a: Script, b: Script) -> bool {
    let cjk = |s| matches!(s, Script::Han | Script::Kana);
    cjk(a) && cjk(b)
}

/// Language keys; undetected Latin takes the most common detected Latin language.
fn language_keys(cands: &[&Candidate]) -> Vec<String> {
    let keys: Vec<String> = cands.iter().map(|c| c.language_key()).collect();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for k in keys.iter().filter(|k| k.starts_with(LATIN_PREFIX)) {
        match counts.iter_mut().find(|(key, _)| key == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k.clone(), 1)),
        }
    }
    let mut common: Option<&(String, usize)> = None;
    for c in &counts {
        if common.is_none_or(|best| c.1 > best.1) {
            common = Some(c);
        }
    }
    let common = common.map(|c| c.0.clone());
    keys.into_iter()
        .map(|k| match &common {
            Some(c) if k == Script::Latin.name() => c.clone(),
            _ => k,
        })
        .collect()
}

/// The clearly most common language: ≥ `min_votes`, strictly more than the runner-up.
fn majority_language(keys: &[String], min_votes: usize) -> Option<String> {
    let mut counts: Vec<(&String, usize)> = Vec::new();
    for k in keys.iter().filter(|k| k.as_str() != Script::Unknown.name()) {
        match counts.iter_mut().find(|(key, _)| *key == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    let (top, votes) = counts.first()?;
    let runner_up = counts.get(1).map(|c| c.1).unwrap_or(0);
    (*votes >= min_votes && *votes > runner_up).then(|| (*top).clone())
}

fn is_chinese_song(reference: Option<Script>, cands: &[&Candidate]) -> bool {
    match reference {
        Some(r) => r == Script::Han,
        None => {
            let others: Vec<&Candidate> = cands
                .iter()
                .copied()
                .filter(|c| !is_chinese_source(c.provider))
                .collect();
            !cands.iter().any(|c| c.script == Script::Kana)
                && majority_language(&language_keys(&others), 1).as_deref()
                    == Some(Script::Han.name())
        }
    }
}

fn song_language(reference: Option<Script>, cands: &[&Candidate]) -> Option<String> {
    if let Some(r) = reference {
        return Some(r.name().to_string());
    }
    let others: Vec<&Candidate> = cands
        .iter()
        .copied()
        .filter(|c| !is_chinese_source(c.provider) && c.provider != ProviderId::YouTubeCaptions)
        .collect();
    majority_language(&language_keys(&others), 1)
}

fn candidate_language_key(c: &Candidate, song_language: &str) -> String {
    match c.script {
        Script::Latin => match c.language {
            Some(l) => format!("{LATIN_PREFIX}{l}"),
            None if song_language.starts_with(LATIN_PREFIX)
                || song_language == Script::Latin.name() =>
            {
                song_language.to_string()
            }
            None => Script::Latin.name().to_string(),
        },
        Script::Kana if song_language == Script::Han.name() => song_language.to_string(),
        s => s.name().to_string(),
    }
}

/// Tiered pool: main sources (+ Chinese services for a Chinese song, or their word-timed
/// originals in the song's language), else non-Chinese answers of the Chinese services,
/// else YouTube auto captions.
fn eligible<'a>(
    cands: &[&'a Candidate],
    chinese_song: bool,
    song_language: Option<&str>,
) -> Vec<&'a Candidate> {
    let (chinese, rest): (Vec<&Candidate>, Vec<&Candidate>) = cands
        .iter()
        .copied()
        .partition(|c| is_chinese_source(c.provider));
    let (captions, main): (Vec<&Candidate>, Vec<&Candidate>) = rest
        .into_iter()
        .partition(|c| c.provider == ProviderId::YouTubeCaptions);
    let mut pool = main;
    if chinese_song {
        pool.extend(chinese.iter().copied());
    } else {
        pool.extend(chinese.iter().copied().filter(|c| {
            c.sync == SyncKind::Word
                && c.script != Script::Han
                && song_language.is_some_and(|sl| candidate_language_key(c, sl) == sl)
        }));
    }
    if pool.is_empty() {
        pool = chinese
            .iter()
            .copied()
            .filter(|c| c.script != Script::Han)
            .collect();
    }
    if pool.is_empty() {
        pool = captions;
    }
    pool
}

/// Pick the lyrics most likely to be the song as sung. `provider_count` is the length of the
/// full configured order (the order tiebreak is `provider_count - order`).
pub fn pick_best<'a>(
    meta: &TrackMeta,
    candidates: &'a [Candidate],
    provider_count: usize,
) -> Option<&'a Candidate> {
    let all: Vec<&Candidate> = candidates.iter().collect();
    let meta_text = [
        Some(meta.title.clone()),
        Some(meta.artists.join(" ")),
        meta.album.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    let metadata_script = dominant_script(&meta_text);
    let reference = if metadata_script.is_native() {
        Some(metadata_script)
    } else {
        all.iter()
            .filter(|c| is_language_reference(c.provider))
            .map(|c| c.script)
            .find(|s| s.is_native())
    }
    .map(|s| {
        if s == Script::Han && all.iter().any(|c| c.script == Script::Kana) {
            Script::Kana
        } else {
            s
        }
    });

    let chinese_song = is_chinese_song(reference, &all);
    let song_lang = song_language(reference, &all);
    let pool = eligible(&all, chinese_song, song_lang.as_deref());
    if pool.is_empty() {
        return None;
    }
    let languages = language_keys(&pool);
    let majority = if reference.is_none() {
        majority_language(&languages, 2)
    } else {
        None
    };

    let trusted_native: HashSet<Script> = if reference.is_some() || majority.is_some() {
        HashSet::new()
    } else {
        let mut counts: HashMap<Script, usize> = HashMap::new();
        for c in pool.iter().filter(|c| c.script.is_native()) {
            *counts.entry(c.script).or_default() += 1;
        }
        counts
            .into_iter()
            .filter(|(s, n)| *n >= 2 || matches!(s, Script::Kana | Script::Hangul))
            .map(|(s, _)| s)
            .collect()
    };

    let count = provider_count as i64;
    let mut best: Option<(&Candidate, i64)> = None;
    for (index, c) in pool.iter().enumerate() {
        let script_score = if let Some(r) = reference {
            if c.script == r {
                3
            } else if same_language_family(c.script, r) {
                2
            } else if c.script == Script::Unknown {
                1
            } else {
                0
            }
        } else if let Some(m) = &majority {
            if &languages[index] == m {
                3
            } else if c.script == Script::Unknown {
                1
            } else {
                0
            }
        } else if !trusted_native.is_empty() {
            if trusted_native.contains(&c.script) {
                2
            } else if c.script == Script::Unknown {
                1
            } else {
                0
            }
        } else if c.script.is_native() {
            0
        } else {
            1
        };
        let sync_score = if c.sync == SyncKind::Word && is_syllable_split(&c.raw) {
            sync_ordinal(SyncKind::Line)
        } else {
            sync_ordinal(c.sync)
        };
        let duet_score = if c.has_duet() {
            2
        } else if has_voice_tags(&c.raw) {
            1
        } else {
            0
        };
        let order = c.order as i64;
        let order_score = if chinese_song && is_chinese_source(c.provider) {
            count * 2 - order
        } else {
            count - order
        };
        let score = script_score * SCRIPT_WEIGHT
            + sync_score * SYNC_WEIGHT
            + duet_score * DUET_WEIGHT
            + order_score;
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((c, score));
        }
    }
    best.map(|(c, _)| c)
}
