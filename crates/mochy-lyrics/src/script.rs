//! Script / language detection used to grade provider results (Android `LyricsUtils`:
//! `Script`, `dominantScript`, `lyricsScript`, `latinLanguage`, `isSyllableSplit`,
//! `hasVoiceTags`, `hasDistinctVoices`).

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::lrc::parse_timed;

/// Writing systems told apart when deciding whether a provider returned the song or a
/// translation of it. Declaration order matches Android (used for tie-breaking).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Script {
    Latin,
    Han,
    Kana,
    Hangul,
    Cyrillic,
    Arabic,
    Devanagari,
    Thai,
    Greek,
    Hebrew,
    Unknown,
}

impl Script {
    const ALL: [Script; 11] = [
        Script::Latin,
        Script::Han,
        Script::Kana,
        Script::Hangul,
        Script::Cyrillic,
        Script::Arabic,
        Script::Devanagari,
        Script::Thai,
        Script::Greek,
        Script::Hebrew,
        Script::Unknown,
    ];

    /// Android enum name (used as a language key).
    pub fn name(self) -> &'static str {
        match self {
            Script::Latin => "LATIN",
            Script::Han => "HAN",
            Script::Kana => "KANA",
            Script::Hangul => "HANGUL",
            Script::Cyrillic => "CYRILLIC",
            Script::Arabic => "ARABIC",
            Script::Devanagari => "DEVANAGARI",
            Script::Thai => "THAI",
            Script::Greek => "GREEK",
            Script::Hebrew => "HEBREW",
            Script::Unknown => "UNKNOWN",
        }
    }

    fn index(self) -> usize {
        Script::ALL.iter().position(|s| *s == self).unwrap_or(10)
    }

    /// Not Latin and not Unknown.
    pub fn is_native(self) -> bool {
        !matches!(self, Script::Latin | Script::Unknown)
    }
}

/// Script of one character, `None` for non-letters / unclassified letters.
pub fn script_of(ch: char) -> Option<Script> {
    if !ch.is_alphabetic() {
        return None;
    }
    let c = ch as u32;
    Some(match c {
        0x0041..=0x024F => Script::Latin,
        0x0370..=0x03FF => Script::Greek,
        0x0400..=0x052F => Script::Cyrillic,
        0x0590..=0x05FF => Script::Hebrew,
        0x0600..=0x06FF | 0x0750..=0x077F => Script::Arabic,
        0x0900..=0x097F => Script::Devanagari,
        0x0E00..=0x0E7F => Script::Thai,
        0x3040..=0x30FF => Script::Kana,
        0xAC00..=0xD7AF | 0x1100..=0x11FF => Script::Hangul,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han,
        _ => return None,
    })
}

/// Characters of scripts written without spaces between words (Han, kana, Thai) plus CJK
/// punctuation / full-width forms. Two such characters side by side never get a space.
pub fn is_unspaced_char(c: char) -> bool {
    matches!(
        script_of(c),
        Some(Script::Han | Script::Kana | Script::Thai)
    ) || matches!(c as u32, 0x2E80..=0x2FDF | 0x3000..=0x303F | 0x31F0..=0x31FF | 0xFF00..=0xFFEF)
}

/// True when no space belongs between pieces `left` and `right` of one line because the
/// boundary characters are both [`is_unspaced_char`].
pub(crate) fn unspaced_boundary(left: &str, right: &str) -> bool {
    match (
        left.trim_end().chars().last(),
        right.trim_start().chars().next(),
    ) {
        (Some(l), Some(r)) => is_unspaced_char(l) && is_unspaced_char(r),
        _ => false,
    }
}

/// The script most of `text` is written in. Any kana ⇒ Kana; Hangul wins at ≥ 25 %.
pub fn dominant_script(text: &str) -> Script {
    let mut counts = [0usize; 11];
    let mut total = 0;
    for s in text.chars().filter_map(script_of) {
        counts[s.index()] += 1;
        total += 1;
    }
    if total == 0 {
        return Script::Unknown;
    }
    if counts[Script::Kana.index()] > 0 {
        return Script::Kana;
    }
    if counts[Script::Hangul.index()] * 4 >= total {
        return Script::Hangul;
    }
    let mut best = Script::Unknown;
    let mut best_count = 0;
    for s in Script::ALL {
        if counts[s.index()] > best_count {
            best = s;
            best_count = counts[s.index()];
        }
    }
    best
}

static BRACKET_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("regex"));
static ANGLE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").expect("regex"));
static BRACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^}]*\}").expect("regex"));
static MARKUP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[^\]]*\]|<[^>]*>|\{[^}]*\}").expect("regex"));
static NON_WORD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\p{L}'’]+").expect("regex"));
static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));

/// The script of the lyric text only, with timestamps and markup stripped.
///
/// Deviation from Android: `{agent:…}`/`{bg}` tags are stripped too (Android only strips
/// `[…]` and `<…>`, so the Latin letters of "agent" counted against short Han lines).
pub fn lyrics_script(raw: &str) -> Script {
    let s = BRACKET_RE.replace_all(raw, " ");
    let s = ANGLE_RE.replace_all(&s, " ");
    let s = BRACE_RE.replace_all(&s, " ");
    dominant_script(&s)
}

/// Small words that mark a language (Android `LanguageMarkers`, same order).
static LANGUAGE_MARKERS: LazyLock<Vec<(&'static str, HashSet<&'static str>)>> =
    LazyLock::new(|| {
        let raw: [(&str, &[&str]); 13] = [
            (
                "en",
                &[
                    "the", "and", "you", "i", "i'm", "my", "it's", "don't", "your", "that", "what",
                    "with", "this", "be", "just", "can't", "won't", "when", "all", "for", "of",
                    "we", "but", "now", "never", "you're", "i'll", "was", "are", "can", "know",
                    "like", "want", "she", "he",
                ],
            ),
            (
                "id",
                &[
                    "yang", "dan", "aku", "kau", "kamu", "ini", "itu", "tak", "tidak", "dengan",
                    "untuk", "akan", "ada", "dalam", "hati", "saja", "bisa", "tapi", "jangan",
                    "sudah", "lagi", "karena", "selalu", "engkau", "dia", "kita", "cinta", "ku",
                    "mu", "dari", "pun", "pernah", "hanya", "semua", "takkan", "denganmu",
                    "padamu", "diriku", "dirimu",
                ],
            ),
            (
                "es",
                &[
                    "que", "y", "el", "los", "las", "yo", "pero", "porque", "quiero", "eres",
                    "estoy", "cuando", "para", "del", "como", "más", "qué", "nada", "todo", "tú",
                    "mí", "sí", "esta", "está", "contigo", "corazón", "vida", "siempre", "nunca",
                    "puedo",
                ],
            ),
            (
                "pt",
                &[
                    "você",
                    "não",
                    "eu",
                    "meu",
                    "minha",
                    "é",
                    "um",
                    "pra",
                    "tudo",
                    "nao",
                    "voce",
                    "também",
                    "mais",
                    "quando",
                    "sem",
                    "isso",
                    "ela",
                    "ele",
                    "só",
                    "vou",
                    "seu",
                    "sua",
                    "nós",
                    "estou",
                    "coração",
                    "agora",
                    "sempre",
                    "nunca",
                ],
            ),
            (
                "fr",
                &[
                    "je", "les", "et", "est", "pas", "pour", "dans", "moi", "toi", "mon", "c'est",
                    "j'ai", "qui", "sur", "une", "des", "du", "au", "ne", "plus", "tout", "elle",
                    "nous", "vous", "suis", "ça", "avec", "mais",
                ],
            ),
            (
                "de",
                &[
                    "der", "die", "das", "und", "ich", "nicht", "ist", "ein", "eine", "mich",
                    "mir", "dich", "dir", "wir", "mein", "auf", "mit", "wie", "nur", "auch",
                    "sich", "noch", "bin", "bist", "kein", "immer", "zu",
                ],
            ),
            (
                "it",
                &[
                    "il", "che", "non", "sono", "sei", "io", "per", "mio", "mia", "più", "cosa",
                    "ancora", "questo", "della", "nel", "perché", "tutto", "anche", "gli",
                    "voglio", "amore",
                ],
            ),
            (
                "tl",
                &[
                    "ang", "ng", "sa", "ko", "mo", "na", "ka", "ako", "ikaw", "hindi", "lang",
                    "mga", "kita", "ay", "naman", "pa", "sana", "akin", "ito", "iyong", "natin",
                    "tayo", "puso",
                ],
            ),
            (
                "nl",
                &[
                    "het", "een", "ik", "niet", "van", "dat", "mijn", "jij", "wij", "voor", "maar",
                    "zijn", "wat", "wel", "nog", "ook", "geen", "heb", "ben",
                ],
            ),
            (
                "tr",
                &[
                    "bir", "ve", "ben", "sen", "bu", "için", "çok", "gibi", "beni", "seni", "var",
                    "yok", "değil", "olan", "her", "ama", "daha", "şey", "kadar",
                ],
            ),
            (
                "vi",
                &[
                    "anh", "em", "và", "không", "là", "của", "một", "người", "yêu", "có", "những",
                    "cho", "đã", "mình", "trong", "như", "tôi", "thì", "này",
                ],
            ),
            (
                "ja-Latn",
                &[
                    "wa", "ga", "wo", "ni", "mo", "kimi", "boku", "watashi", "anata", "dake",
                    "kara", "made", "kokoro", "demo", "itsumo", "nai", "koto", "toki", "naka",
                    "sora",
                ],
            ),
            (
                "ko-Latn",
                &[
                    "naega",
                    "neol",
                    "neoreul",
                    "nan",
                    "nae",
                    "neon",
                    "geu",
                    "nal",
                    "naneun",
                    "neoui",
                    "uri",
                    "saranghae",
                    "jigeum",
                    "modeun",
                    "ije",
                    "geurae",
                    "eonjena",
                ],
            ),
        ];
        raw.iter()
            .map(|(lang, words)| (*lang, words.iter().copied().collect()))
            .collect()
    });

/// Language of Latin-script lyrics ("en", "id", "vi", "ja-Latn", …) or `None` when it can't be
/// told (other script, fewer than 12 words, no clear winner).
pub fn latin_language(raw: &str) -> Option<&'static str> {
    if lyrics_script(raw) != Script::Latin {
        return None;
    }
    let stripped = MARKUP_RE.replace_all(raw, " ").to_lowercase();
    let tokens: Vec<&str> = NON_WORD_RE
        .split(&stripped)
        .map(|t| t.trim_matches(['\'', '’']))
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.len() < 12 {
        return None;
    }
    let mut ranked: Vec<(&'static str, usize)> = LANGUAGE_MARKERS
        .iter()
        .map(|(lang, markers)| {
            (
                *lang,
                tokens.iter().filter(|t| markers.contains(*t)).count(),
            )
        })
        .collect();
    ranked.sort_by_key(|r| std::cmp::Reverse(r.1)); // stable: keeps marker order on ties
    let (best, hits) = ranked[0];
    let runner_up = ranked.get(1).map(|r| r.1).unwrap_or(0);
    (hits * 10 >= tokens.len() && hits * 2 >= runner_up * 3).then_some(best)
}

/// Word timings smeared across single letters / syllables ("wa nt yo u").
pub fn is_syllable_split(raw: &str) -> bool {
    if raw.trim().is_empty() {
        return false;
    }
    let entries = parse_timed(raw);
    if entries.len() < 4 {
        return false;
    }
    let mut checked = 0;
    let mut flagged = 0;
    for entry in &entries {
        let text = entry.text.trim();
        let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
        if letters.is_empty() {
            continue;
        }
        let latin = letters.iter().filter(|c| (**c as u32) < 0x250).count();
        if (latin as f64) / (letters.len() as f64) < 0.9 {
            continue;
        }
        let tokens: Vec<&str> = WS_RE
            .split(text)
            .filter(|t| t.chars().any(char::is_alphabetic))
            .collect();
        if tokens.len() < 4 {
            continue;
        }
        checked += 1;
        let lengths: Vec<usize> = tokens
            .iter()
            .map(|t| t.chars().filter(|c| c.is_alphabetic()).count())
            .collect();
        let short_ratio = lengths.iter().filter(|l| **l <= 3).count() as f64 / lengths.len() as f64;
        let mean = lengths.iter().sum::<usize>() as f64 / lengths.len() as f64;
        if short_ratio >= 0.8 && mean < 2.8 {
            flagged += 1;
        }
    }
    checked >= 4 && (flagged as f64) / (checked as f64) >= 0.65
}

/// Whether a payload carries `{agent:…}` voice tags.
pub fn has_voice_tags(raw: &str) -> bool {
    raw.contains("{agent:")
}

/// Two or more voices among the lead (non-`{bg}`) lines, untagged lines counting as `v1`.
pub fn has_distinct_voices(raw: &str) -> bool {
    if !has_voice_tags(raw) {
        return false;
    }
    let voices: HashSet<String> = parse_timed(raw)
        .into_iter()
        .filter(|l| !l.bg)
        .map(|l| l.agent.unwrap_or_else(|| "v1".to_string()))
        .collect();
    voices.len() > 1
}
