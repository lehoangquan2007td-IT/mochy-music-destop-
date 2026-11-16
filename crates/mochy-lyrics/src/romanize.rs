//! Romanization of lyric lines (Android `LyricsUtils.romanize*` + `LyricsReadings.readSong`).
//!
//! Japanese uses the pure-Rust `kakasi` dictionary (Android: kuromoji tokens + a katakana table),
//! Chinese uses `pinyin` without tones (Android: TinyPinyin, lowercased), Korean, the Cyrillic
//! languages, Hindi and Punjabi are table-driven ports of the Kotlin code.
//!
//! The whole song decides the language ([`song_language`]): a kanji line alone looks Chinese
//! and an English verse alone looks like nothing, so callers romanize every line with the
//! language of the whole body ([`romanize_song`]).

use std::collections::HashMap;
use std::sync::LazyLock;

use pinyin::ToPinyin;
use regex::Regex;

use crate::script::{Script, script_of};

/// A language the readings can be switched on/off for (Android `RomanizeLanguageKeys`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RomanizeLang {
    Japanese,
    Korean,
    Chinese,
    Russian,
    Ukrainian,
    Serbian,
    Bulgarian,
    Belarusian,
    Kyrgyz,
    Macedonian,
    Hindi,
    Punjabi,
}

impl RomanizeLang {
    pub const ALL: [RomanizeLang; 12] = [
        RomanizeLang::Japanese,
        RomanizeLang::Korean,
        RomanizeLang::Chinese,
        RomanizeLang::Russian,
        RomanizeLang::Ukrainian,
        RomanizeLang::Serbian,
        RomanizeLang::Bulgarian,
        RomanizeLang::Belarusian,
        RomanizeLang::Kyrgyz,
        RomanizeLang::Macedonian,
        RomanizeLang::Hindi,
        RomanizeLang::Punjabi,
    ];

    /// Stable id for persistence (settings `disabled` list).
    pub fn id(self) -> &'static str {
        match self {
            RomanizeLang::Japanese => "ja",
            RomanizeLang::Korean => "ko",
            RomanizeLang::Chinese => "zh",
            RomanizeLang::Russian => "ru",
            RomanizeLang::Ukrainian => "uk",
            RomanizeLang::Serbian => "sr",
            RomanizeLang::Bulgarian => "bg",
            RomanizeLang::Belarusian => "be",
            RomanizeLang::Kyrgyz => "ky",
            RomanizeLang::Macedonian => "mk",
            RomanizeLang::Hindi => "hi",
            RomanizeLang::Punjabi => "pa",
        }
    }

    /// English display name.
    pub fn name(self) -> &'static str {
        match self {
            RomanizeLang::Japanese => "Japanese",
            RomanizeLang::Korean => "Korean",
            RomanizeLang::Chinese => "Chinese",
            RomanizeLang::Russian => "Russian",
            RomanizeLang::Ukrainian => "Ukrainian",
            RomanizeLang::Serbian => "Serbian",
            RomanizeLang::Bulgarian => "Bulgarian",
            RomanizeLang::Belarusian => "Belarusian",
            RomanizeLang::Kyrgyz => "Kyrgyz",
            RomanizeLang::Macedonian => "Macedonian",
            RomanizeLang::Hindi => "Hindi",
            RomanizeLang::Punjabi => "Punjabi",
        }
    }

    pub fn from_id(id: &str) -> Option<RomanizeLang> {
        RomanizeLang::ALL.into_iter().find(|l| l.id().eq_ignore_ascii_case(id.trim()))
    }

    fn is_cyrillic(self) -> bool {
        matches!(
            self,
            RomanizeLang::Russian
                | RomanizeLang::Ukrainian
                | RomanizeLang::Serbian
                | RomanizeLang::Bulgarian
                | RomanizeLang::Belarusian
                | RomanizeLang::Kyrgyz
                | RomanizeLang::Macedonian
        )
    }
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// The script the readings are for: kana settles it; otherwise the commonest script that isn't
/// Latin, as long as a tenth of the letters are in it (Android `romanizableScript`).
pub fn romanizable_script(text: &str) -> Script {
    let mut counts: HashMap<Script, usize> = HashMap::new();
    let mut total = 0usize;
    for s in text.chars().filter_map(script_of) {
        *counts.entry(s).or_default() += 1;
        total += 1;
    }
    if total == 0 {
        return Script::Unknown;
    }
    if counts.get(&Script::Kana).copied().unwrap_or(0) > 0 {
        return Script::Kana;
    }
    let best = counts
        .iter()
        .filter(|(s, _)| **s != Script::Latin)
        .max_by_key(|(s, c)| (**c, std::cmp::Reverse(s.name())));
    match best {
        Some((s, c)) if c * 10 >= total => *s,
        _ => Script::Unknown,
    }
}

/// The romanization language of a whole song body (Android `switchFor`).
///
/// Deviation: Gurmukhi letters aren't classified by [`script_of`], so Android never offered a
/// reading for an all-Punjabi song; here a body with Gurmukhi and no other native script is
/// Punjabi.
pub fn song_language(body: &str) -> Option<RomanizeLang> {
    Some(match romanizable_script(body) {
        Script::Kana => RomanizeLang::Japanese,
        Script::Hangul => RomanizeLang::Korean,
        Script::Han => RomanizeLang::Chinese,
        Script::Devanagari => RomanizeLang::Hindi,
        // Deviation: Android asked `isUkrainian` first, and Russian written only with letters the
        // alphabets share ("Я тебя люблю") passed it. Letters only one language has decide first.
        Script::Cyrillic => {
            let has = |set: &str| body.chars().any(|c| set.contains(c));
            if has("ҐґЄєЇї") && is_ukrainian(body) {
                RomanizeLang::Ukrainian
            } else if has("ЂђЋћ") && is_serbian(body) {
                RomanizeLang::Serbian
            } else if has("ЃѓЅѕЌќ") && is_macedonian(body) {
                RomanizeLang::Macedonian
            } else if has("ЈјЉљЊњЏџ") && is_serbian(body) {
                RomanizeLang::Serbian
            } else if has("Ўў") && is_belarusian(body) {
                RomanizeLang::Belarusian
            } else if has("ҢңӨөҮү") && is_kyrgyz(body) {
                RomanizeLang::Kyrgyz
            } else if has("Іі") && is_ukrainian(body) {
                RomanizeLang::Ukrainian
            } else if is_russian(body) || !is_bulgarian(body) {
                RomanizeLang::Russian
            } else {
                RomanizeLang::Bulgarian
            }
        }
        Script::Unknown if is_punjabi(body) => RomanizeLang::Punjabi,
        _ => return None,
    })
}

/// Romanize every line of a song (Android `readSong`). `allowed` is asked once for the song's
/// language; a language that isn't allowed (or a song in nothing romanizable) gives `None` for
/// every line. A reading equal to the line itself is dropped.
pub fn romanize_song<S: AsRef<str>>(lines: &[S], allowed: impl Fn(RomanizeLang) -> bool) -> Vec<Option<String>> {
    let body = lines.iter().map(|l| l.as_ref()).collect::<Vec<_>>().join(" ");
    let Some(lang) = song_language(&body) else {
        return vec![None; lines.len()];
    };
    if !allowed(lang) {
        return vec![None; lines.len()];
    }
    // a Punjabi line inside a song read as another script still gets its own reading
    let punjabi = lang != RomanizeLang::Punjabi && is_punjabi(&body) && allowed(RomanizeLang::Punjabi);
    lines
        .iter()
        .map(|l| {
            let text = l.as_ref();
            if text.trim().is_empty() {
                return None;
            }
            let reading = if punjabi { Some(romanize_punjabi(text)) } else { romanize_line(text, lang) };
            reading.map(|r| r.trim().to_string()).filter(|r| !r.is_empty() && r != text.trim())
        })
        .collect()
}

/// One line in `lang`. Cyrillic re-detects the language per line like Android's
/// `romanizeCyrillic` (and returns `None` when the line isn't one of the supported ones).
pub fn romanize_line(text: &str, lang: RomanizeLang) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    match lang {
        RomanizeLang::Japanese => Some(romanize_japanese(text)),
        RomanizeLang::Korean => Some(romanize_korean(text)),
        RomanizeLang::Chinese => Some(romanize_chinese(text)),
        RomanizeLang::Hindi => Some(romanize_hindi(text)),
        RomanizeLang::Punjabi => Some(romanize_punjabi(text)),
        l if l.is_cyrillic() => romanize_cyrillic(text),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Japanese
// ---------------------------------------------------------------------------

/// Japanese → Hepburn romaji, words separated by spaces, lowercase.
pub fn romanize_japanese(text: &str) -> String {
    let res = kakasi::convert(text);
    let out = fix_sokuon(&res.hiragana, &res.romaji.to_lowercase());
    WS_RE.replace_all(out.trim(), " ").into_owned()
}

fn is_kana(c: char) -> bool {
    ('\u{3041}'..='\u{30FF}').contains(&c)
}

const GEMINABLE: &str = "bcdfghjkmprstwz";

/// kakasi spells a small っ that ends one of its chunks (okurigana after a kanji: 会っ|た) as
/// "tsu" ("atsu ta"), and puts a space before a chunk that starts with one ("waka tta"). The
/// reading (`hiragana`) is right, so the つ/っ in it are lined up with the "tsu"s and doubled
/// consonants of the romaji, in order, and every っ before kana is made a doubled consonant
/// joined to its word ("atta", "wakatta") — Android's `katakanaToRomaji` does the same with the
/// next token's reading. When the two don't line up, the romaji is left as it is.
fn fix_sokuon(hiragana: &str, romaji: &str) -> String {
    let h: Vec<char> = hiragana.chars().collect();
    // true = a sokuon before kana (should double the next consonant)
    let h_events: Vec<bool> = h
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == 'つ' || **c == 'っ')
        .map(|(i, c)| *c == 'っ' && h.get(i + 1).is_some_and(|n| is_kana(*n) && *n != 'ー'))
        .collect();
    if !h_events.contains(&true) {
        return romaji.to_string();
    }
    #[derive(Clone, Copy)]
    enum Ev {
        Tsu(usize),
        Gem(usize),
    }
    let r: Vec<char> = romaji.chars().collect();
    let mut events = Vec::new();
    let mut i = 0;
    while i < r.len() {
        if r[i..].starts_with(&['t', 's', 'u']) {
            events.push(Ev::Tsu(i));
            i += 3;
        } else if r[i..].starts_with(&['t', 'c', 'h']) {
            events.push(Ev::Gem(i));
            i += 3;
        } else if i + 1 < r.len() && r[i] == r[i + 1] && GEMINABLE.contains(r[i]) {
            events.push(Ev::Gem(i));
            i += 2;
        } else {
            i += 1;
        }
    }
    if events.len() != h_events.len() {
        return romaji.to_string();
    }
    // Rebuild: (char index, chars to drop, replacement) edits, applied back to front.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (ev, sokuon) in events.iter().zip(&h_events) {
        if !*sokuon {
            continue;
        }
        match *ev {
            Ev::Tsu(at) => {
                let mut j = at + 3;
                while j < r.len() && r[j] == ' ' {
                    j += 1;
                }
                let Some(&next) = r.get(j) else { continue };
                if !GEMINABLE.contains(next) {
                    continue;
                }
                let doubled = if next == 'c' { 't' } else { next };
                edits.push((at, j - at, doubled.to_string()));
            }
            Ev::Gem(at) => {
                let mut k = at;
                while k > 0 && r[k - 1] == ' ' {
                    k -= 1;
                }
                if k < at && k > 0 && r[k - 1].is_alphabetic() {
                    edits.push((k, at - k, String::new()));
                }
            }
        }
    }
    let mut out = r;
    for (at, drop, rep) in edits.into_iter().rev() {
        out.splice(at..at + drop, rep.chars());
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Korean (Revised Romanization with the Android jongseong context table)
// ---------------------------------------------------------------------------

static CHO: &[(&str, &str)] = &[
    ("ᄀ", "g"), ("ᄁ", "kk"), ("ᄂ", "n"), ("ᄃ", "d"), ("ᄄ", "tt"), ("ᄅ", "r"), ("ᄆ", "m"),
    ("ᄇ", "b"), ("ᄈ", "pp"), ("ᄉ", "s"), ("ᄊ", "ss"), ("ᄋ", ""), ("ᄌ", "j"), ("ᄍ", "jj"),
    ("ᄎ", "ch"), ("ᄏ", "k"), ("ᄐ", "t"), ("ᄑ", "p"), ("ᄒ", "h"),
];

static JUNG: &[(&str, &str)] = &[
    ("ᅡ", "a"), ("ᅢ", "ae"), ("ᅣ", "ya"), ("ᅤ", "yae"), ("ᅥ", "eo"), ("ᅦ", "e"), ("ᅧ", "yeo"),
    ("ᅨ", "ye"), ("ᅩ", "o"), ("ᅪ", "wa"), ("ᅫ", "wae"), ("ᅬ", "oe"), ("ᅭ", "yo"), ("ᅮ", "u"),
    ("ᅯ", "wo"), ("ᅰ", "we"), ("ᅱ", "wi"), ("ᅲ", "yu"), ("ᅳ", "eu"), ("ᅴ", "eui"), ("ᅵ", "i"),
];

#[rustfmt::skip]
static JONG: &[(&str, &str)] = &[
    ("ᆨ", "k"), ("ᆨᄋ", "g"), ("ᆨᄂ", "ngn"), ("ᆨᄅ", "ngn"), ("ᆨᄆ", "ngm"), ("ᆨᄒ", "kh"),
    ("ᆩ", "kk"), ("ᆩᄋ", "kg"), ("ᆩᄂ", "ngn"), ("ᆩᄅ", "ngn"), ("ᆩᄆ", "ngm"), ("ᆩᄒ", "kh"),
    ("ᆪ", "k"), ("ᆪᄋ", "ks"), ("ᆪᄂ", "ngn"), ("ᆪᄅ", "ngn"), ("ᆪᄆ", "ngm"), ("ᆪᄒ", "kch"),
    ("ᆫ", "n"), ("ᆫᄅ", "ll"), ("ᆬ", "n"), ("ᆬᄋ", "nj"), ("ᆬᄂ", "nn"), ("ᆬᄅ", "nn"),
    ("ᆬᄆ", "nm"), ("ᆬㅎ", "nch"), ("ᆭ", "n"), ("ᆭᄋ", "nh"), ("ᆭᄅ", "nn"), ("ᆮ", "t"),
    ("ᆮᄋ", "d"), ("ᆮᄂ", "nn"), ("ᆮᄅ", "nn"), ("ᆮᄆ", "nm"), ("ᆮᄒ", "th"), ("ᆯ", "l"),
    ("ᆯᄋ", "r"), ("ᆯᄂ", "ll"), ("ᆯᄅ", "ll"), ("ᆰ", "k"), ("ᆰᄋ", "lg"), ("ᆰᄂ", "ngn"),
    ("ᆰᄅ", "ngn"), ("ᆰᄆ", "ngm"), ("ᆰᄒ", "lkh"), ("ᆱ", "m"), ("ᆱᄋ", "lm"), ("ᆱᄂ", "mn"),
    ("ᆱᄅ", "mn"), ("ᆱᄆ", "mm"), ("ᆱᄒ", "lmh"), ("ᆲ", "p"), ("ᆲᄋ", "lb"), ("ᆲᄂ", "mn"),
    ("ᆲᄅ", "mn"), ("ᆲᄆ", "mm"), ("ᆲᄒ", "lph"), ("ᆳ", "t"), ("ᆳᄋ", "ls"), ("ᆳᄂ", "nn"),
    ("ᆳᄅ", "nn"), ("ᆳᄆ", "nm"), ("ᆳᄒ", "lsh"), ("ᆴ", "t"), ("ᆴᄋ", "lt"), ("ᆴᄂ", "nn"),
    ("ᆴᄅ", "nn"), ("ᆴᄆ", "nm"), ("ᆴᄒ", "lth"), ("ᆵ", "p"), ("ᆵᄋ", "lp"), ("ᆵᄂ", "mn"),
    ("ᆵᄅ", "mn"), ("ᆵᄆ", "mm"), ("ᆵᄒ", "lph"), ("ᆶ", "l"), ("ᆶᄋ", "lh"), ("ᆶᄂ", "ll"),
    ("ᆶᄅ", "ll"), ("ᆶᄆ", "lm"), ("ᆶᄒ", "lh"), ("ᆷ", "m"), ("ᆷᄅ", "mn"), ("ᆸ", "p"),
    ("ᆸᄋ", "b"), ("ᆸᄂ", "mn"), ("ᆸᄅ", "mn"), ("ᆸᄆ", "mm"), ("ᆸᄒ", "ph"), ("ᆹ", "p"),
    ("ᆹᄋ", "ps"), ("ᆹᄂ", "mn"), ("ᆹᄅ", "mn"), ("ᆹᄆ", "mm"), ("ᆹᄒ", "psh"), ("ᆺ", "t"),
    ("ᆺᄋ", "s"), ("ᆺᄂ", "nn"), ("ᆺᄅ", "nn"), ("ᆺᄆ", "nm"), ("ᆺᄒ", "sh"), ("ᆻ", "t"),
    ("ᆻᄋ", "ss"), ("ᆻᄂ", "tn"), ("ᆻᄅ", "tn"), ("ᆻᄆ", "nm"), ("ᆻᄒ", "th"), ("ᆼ", "ng"),
    ("ᆽ", "t"), ("ᆽᄋ", "j"), ("ᆽᄂ", "nn"), ("ᆽᄅ", "nn"), ("ᆽᄆ", "nm"), ("ᆽᄒ", "ch"),
    ("ᆾ", "t"), ("ᆾᄋ", "ch"), ("ᆾᄂ", "nn"), ("ᆾᄅ", "nn"), ("ᆾᄆ", "nm"), ("ᆾᄒ", "ch"),
    ("ᆿ", "k"), ("ᆿᄋ", "k"), ("ᆿᄂ", "ngn"), ("ᆿᄅ", "ngn"), ("ᆿᄆ", "ngm"), ("ᆿᄒ", "kh"),
    ("ᇀ", "t"), ("ᇀᄋ", "t"), ("ᇀᄂ", "nn"), ("ᇀᄅ", "nn"), ("ᇀᄆ", "nm"), ("ᇀᄒ", "th"),
    ("ᇁ", "p"), ("ᇁᄋ", "p"), ("ᇁᄂ", "mn"), ("ᇁᄅ", "mn"), ("ᇁᄆ", "mm"), ("ᇁᄒ", "ph"),
    ("ᇂ", "t"), ("ᇂᄋ", "h"), ("ᇂᄂ", "nn"), ("ᇂᄅ", "nn"), ("ᇂᄆ", "mm"), ("ᇂᄒ", "t"),
    ("ᇂᄀ", "k"),
];

fn table(t: &'static [(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
    t.iter().copied().collect()
}

static CHO_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| table(CHO));
static JUNG_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| table(JUNG));
static JONG_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| table(JONG));

/// Hangul → Revised Romanization (Android `romanizeKorean`): each syllable decomposed into
/// jamo; a final consonant is spelled by what the next syllable starts with.
pub fn romanize_korean(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut prev_final: Option<char> = None;
    let jong_alone = |f: char, buf: &mut String| {
        let s = f.to_string();
        buf.push_str(JONG_MAP.get(s.as_str()).copied().unwrap_or(s.as_str()));
    };
    for ch in text.chars() {
        if ('\u{AC00}'..='\u{D7A3}').contains(&ch) {
            let idx = ch as u32 - 0xAC00;
            let cho = char::from_u32(0x1100 + idx / (21 * 28)).unwrap_or(ch);
            let jung = char::from_u32(0x1161 + (idx % (21 * 28)) / 28).unwrap_or(ch);
            let jong_idx = idx % 28;
            let jong = (jong_idx != 0).then(|| char::from_u32(0x11A7 + jong_idx)).flatten();
            // A context entry (final + next initial) already spells the initial's sound
            // ("ᆨᄆ" → "ngm"). Deviation: Android appended the initial again ("hangungmmal").
            let mut initial_spelled = false;
            if let Some(f) = prev_final {
                let key = format!("{f}{cho}");
                match JONG_MAP.get(key.as_str()) {
                    Some(v) => {
                        out.push_str(v);
                        initial_spelled = cho != 'ᄋ';
                    }
                    None => jong_alone(f, &mut out),
                }
            }
            let c = cho.to_string();
            let j = jung.to_string();
            if !initial_spelled {
                out.push_str(CHO_MAP.get(c.as_str()).copied().unwrap_or(c.as_str()));
            }
            out.push_str(JUNG_MAP.get(j.as_str()).copied().unwrap_or(j.as_str()));
            prev_final = jong;
        } else {
            if let Some(f) = prev_final.take() {
                jong_alone(f, &mut out);
            }
            out.push(ch);
        }
    }
    if let Some(f) = prev_final {
        jong_alone(f, &mut out);
    }
    out
}

// ---------------------------------------------------------------------------
// Chinese
// ---------------------------------------------------------------------------

static SPACE_BEFORE_ASCII_PUNCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+([,.!?;:])").expect("regex"));
static SPACE_BEFORE_CJK_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([，。！？；：、（）《》〈〉【】『』「」])").expect("regex"));
static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));

/// Han → toneless pinyin, one syllable per character (Android `romanizeChinese`).
pub fn romanize_chinese(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            match ch.to_pinyin() {
                Some(p) => {
                    out.push_str(&p.plain().to_lowercase());
                    out.push(' ');
                }
                None => out.push(ch),
            }
        } else {
            out.push(ch);
        }
    }
    let out = SPACE_BEFORE_ASCII_PUNCT.replace_all(&out, "$1");
    let out = SPACE_BEFORE_CJK_PUNCT.replace_all(&out, "$1");
    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Cyrillic
// ---------------------------------------------------------------------------

#[rustfmt::skip]
static GENERAL_CYRILLIC: &[(char, &str)] = &[
    ('А', "A"), ('Б', "B"), ('В', "V"), ('Г', "G"), ('Ґ', "G"), ('Д', "D"),
    ('Ѓ', "Ǵ"), ('Ђ', "Đ"), ('Е', "E"), ('Ё', "Yo"), ('Є', "Ye"), ('Ж', "Zh"),
    ('З', "Z"), ('Ѕ', "Dz"), ('И', "I"), ('І', "I"), ('Ї', "Yi"), ('Й', "Y"),
    ('Ј', "Y"), ('К', "K"), ('Л', "L"), ('Љ', "Ly"), ('М', "M"), ('Н', "N"),
    ('Њ', "Ny"), ('О', "O"), ('П', "P"), ('Р', "R"), ('С', "S"), ('Т', "T"),
    ('Ћ', "Ć"), ('У', "U"), ('Ў', "Ŭ"), ('Ф', "F"), ('Х', "Kh"), ('Ц', "Ts"),
    ('Ч', "Ch"), ('Џ', "Dž"), ('Ш', "Sh"), ('Щ', "Shch"), ('Ъ', "ʺ"), ('Ы', "Y"),
    ('Ь', "ʹ"), ('Э', "E"), ('Ю', "Yu"), ('Я', "Ya"),
    ('Ѡ', "O"), ('Ѣ', "Ya"), ('Ѥ', "Ye"), ('Ѧ', "Ya"), ('Ѩ', "Ya"),
    ('Ѫ', "U"), ('Ѭ', "Yu"), ('Ѯ', "Ks"), ('Ѱ', "Ps"), ('Ѳ', "F"),
    ('Ѵ', "I"), ('Ѷ', "I"), ('Ғ', "Gh"), ('Ҕ', "G"), ('Җ', "Zh"),
    ('Ҙ', "Dz"), ('Қ', "Q"), ('Ҝ', "K"), ('Ҟ', "K"), ('Ҡ', "K"),
    ('Ң', "Ng"), ('Ҥ', "Ng"), ('Ҧ', "P"), ('Ҩ', "O"), ('Ҫ', "S"),
    ('Ҭ', "T"), ('Ү', "U"), ('Ұ', "U"), ('Ҳ', "Kh"), ('Ҵ', "Ts"),
    ('Ҷ', "Ch"), ('Ҹ', "Ch"), ('Һ', "H"), ('Ҽ', "Ch"), ('Ҿ', "Ch"),
    ('Ќ', "Ḱ"), ('Ө', "Ö"),
    ('а', "a"), ('б', "b"), ('в', "v"), ('г', "g"), ('ґ', "g"), ('д', "d"),
    ('ѓ', "ǵ"), ('ђ', "đ"), ('е', "e"), ('ё', "yo"), ('є', "ye"), ('ж', "zh"),
    ('з', "z"), ('ѕ', "dz"), ('и', "i"), ('і', "i"), ('ї', "yi"), ('й', "y"),
    ('ј', "y"), ('к', "k"), ('л', "l"), ('љ', "ly"), ('м', "m"), ('н', "n"),
    ('њ', "ny"), ('о', "o"), ('п', "p"), ('р', "r"), ('с', "s"), ('т', "t"),
    ('ћ', "ć"), ('у', "u"), ('ў', "ŭ"), ('ф', "f"), ('х', "kh"), ('ц', "ts"),
    ('ч', "ch"), ('џ', "dž"), ('ш', "sh"), ('щ', "shch"), ('ъ', "ʺ"), ('ы', "y"),
    ('ь', "ʹ"), ('э', "e"), ('ю', "yu"), ('я', "ya"),
    ('ѡ', "o"), ('ѣ', "ya"), ('ѥ', "ye"), ('ѧ', "ya"), ('ѩ', "ya"),
    ('ѫ', "u"), ('ѭ', "yu"), ('ѯ', "ks"), ('ѱ', "ps"), ('ѳ', "f"),
    ('ѵ', "i"), ('ѷ', "i"), ('ғ', "gh"), ('ҕ', "g"), ('җ', "zh"),
    ('ҙ', "dz"), ('қ', "q"), ('ҝ', "k"), ('ҟ', "k"), ('ҡ', "k"),
    ('ң', "ng"), ('ҥ', "ng"), ('ҧ', "p"), ('ҩ', "o"), ('ҫ', "s"),
    ('ҭ', "t"), ('ү', "u"), ('ұ', "u"), ('ҳ', "kh"), ('ҵ', "ts"),
    ('ҷ', "ch"), ('ҹ', "ch"), ('һ', "h"), ('ҽ', "ch"), ('ҿ', "ch"),
    ('ќ', "ḱ"), ('ө', "ö"),
];

static UKRAINIAN_MAP: &[(char, &str)] = &[
    ('Г', "H"), ('г', "h"), ('Ґ', "G"), ('ґ', "g"), ('Є', "Ye"), ('є', "ye"),
    ('І', "I"), ('і', "i"), ('Ї', "Yi"), ('ї', "yi"),
];

static SERBIAN_MAP: &[(char, &str)] = &[
    ('Ж', "Ž"), ('Љ', "Lj"), ('Њ', "Nj"), ('Ц', "C"), ('Ч', "Č"), ('Џ', "Dž"), ('Ш', "Š"), ('Х', "H"),
    ('ж', "ž"), ('љ', "lj"), ('њ', "nj"), ('ц', "c"), ('ч', "č"), ('џ', "dž"), ('ш', "š"), ('х', "h"),
];

static BULGARIAN_MAP: &[(char, &str)] = &[
    ('Ж', "Zh"), ('Ц', "Ts"), ('Ч', "Ch"), ('Ш', "Sh"), ('Щ', "Sht"), ('Ъ', "A"), ('Ь', "Y"), ('Ю', "Yu"), ('Я', "Ya"),
    ('ж', "zh"), ('ц', "ts"), ('ч', "ch"), ('ш', "sh"), ('щ', "sht"), ('ъ', "a"), ('ь', "y"), ('ю', "yu"), ('я', "ya"),
];

static BELARUSIAN_MAP: &[(char, &str)] = &[('Г', "H"), ('г', "h"), ('Ў', "W"), ('ў', "w")];

static KYRGYZ_MAP: &[(char, &str)] = &[('Ү', "Ü"), ('ү', "ü"), ('Ы', "Y"), ('ы', "y")];

static MACEDONIAN_MAP: &[(char, &str)] = &[
    ('Ѓ', "Gj"), ('Ѕ', "Dz"), ('И', "I"), ('Ј', "J"), ('Љ', "Lj"), ('Њ', "Nj"), ('Ќ', "Kj"), ('Џ', "Dž"),
    ('Ч', "Č"), ('Ш', "Sh"), ('Ж', "Zh"), ('Ц', "C"), ('Х', "H"),
    ('ѓ', "gj"), ('ѕ', "dz"), ('и', "i"), ('ј', "j"), ('љ', "lj"), ('њ', "nj"), ('ќ', "kj"), ('џ', "dž"),
    ('ч', "č"), ('ш', "sh"), ('ж', "zh"), ('ц', "c"), ('х', "h"),
];

const RUSSIAN_LETTERS: &str = "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдеёжзийклмнопрстуфхцчшщъыьэюя";
const UKRAINIAN_LETTERS: &str = "АБВГҐДЕЄЖЗИІЇЙКЛМНОПРСТУФХЦЧШЩЬЮЯабвгґдеєжзиіїйклмнопрстуфхцчшщьюя";
const SERBIAN_LETTERS: &str = "АБВГДЂЕЖЗИЈКЛЉМНЊОПРСТЋУФХЦЧЏШабвгдђежзијклљмнњопрстћуфхцчџш";
const BULGARIAN_LETTERS: &str = "АБВГДЕЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЬЮЯабвгдежзийклмнопрстуфхцчшщъьюя";
const BELARUSIAN_LETTERS: &str = "АБВГДЕЁЖЗІЙКЛМНОПРСТУЎФХЦЧШЬЮЯЫЭабвгдеёжзійклмнопрстуўфхцчшьюяыэ";
const KYRGYZ_LETTERS: &str = "АБВГДЕЁЖЗИЙКЛМНҢОӨПРСТУҮФХЦЧШЩЪЫЬЭЮЯабвгдеёжзийклмнңоөпрстуүфхцчшщъыьэюя";
const MACEDONIAN_LETTERS: &str = "АБВГДЃЕЖЗЅИЈКЛЉМНЊОПРСТЌУФХЦЧЏШабвгдѓежзѕијклљмнњопрстќуфхцчџш";

fn is_cyr(c: char) -> bool {
    ('\u{0400}'..='\u{04FF}').contains(&c)
}

/// Some letter of `alphabet` and no Cyrillic letter outside it (Android `isRussian` & co).
fn written_in(text: &str, alphabet: &str) -> bool {
    text.chars().any(|c| alphabet.contains(c)) && text.chars().all(|c| alphabet.contains(c) || !is_cyr(c))
}

pub fn is_russian(text: &str) -> bool {
    written_in(text, RUSSIAN_LETTERS)
}
pub fn is_ukrainian(text: &str) -> bool {
    written_in(text, UKRAINIAN_LETTERS)
}
pub fn is_serbian(text: &str) -> bool {
    written_in(text, SERBIAN_LETTERS)
}
pub fn is_bulgarian(text: &str) -> bool {
    written_in(text, BULGARIAN_LETTERS)
}
pub fn is_belarusian(text: &str) -> bool {
    written_in(text, BELARUSIAN_LETTERS)
}
pub fn is_kyrgyz(text: &str) -> bool {
    written_in(text, KYRGYZ_LETTERS)
}
pub fn is_macedonian(text: &str) -> bool {
    written_in(text, MACEDONIAN_LETTERS)
}
pub fn is_punjabi(text: &str) -> bool {
    text.chars().any(|c| ('\u{0A00}'..='\u{0A7F}').contains(&c))
}
pub fn is_hindi(text: &str) -> bool {
    text.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c))
}

fn lookup(map: &[(char, &'static str)], c: char) -> Option<&'static str> {
    map.iter().find(|(k, _)| *k == c).map(|(_, v)| *v)
}

fn general(c: char, out: &mut String) {
    match lookup(GENERAL_CYRILLIC, c) {
        Some(v) => out.push_str(v),
        None => out.push(c),
    }
}

fn is_cyrillic_vowel(c: char) -> bool {
    "АаЕеЄєИиІіЇїОоУуЮюЯяЫыЭэ".contains(c)
}

/// Splits like Kotlin's `((?<=\s|[.,!?;])|(?=\s|[.,!?;]))`: every whitespace / `.,!?;` char is a
/// piece of its own, the runs between them are words.
fn cyrillic_pieces(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() || ".,!?;".contains(c) {
            if start < i {
                out.push(&text[start..i]);
            }
            out.push(&text[i..i + c.len_utf8()]);
            start = i + c.len_utf8();
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn is_separator(piece: &str) -> bool {
    piece.chars().all(|c| c.is_whitespace() || ".,!?;".contains(c))
}

/// Per-word transliteration with a language table over the general one.
fn romanize_with(text: &str, word: impl Fn(&[char], &mut String)) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in cyrillic_pieces(text) {
        if is_separator(piece) {
            out.push_str(piece);
        } else {
            let chars: Vec<char> = piece.chars().collect();
            word(&chars, &mut out);
        }
    }
    out
}

fn romanize_russian(text: &str) -> String {
    romanize_with(text, |w, out| {
        let mut i = 0;
        while i < w.len() {
            // Genitive -ого/-его is said "-ovo/-evo". Deviation: Android matched these three
            // letters anywhere in a word ("много" → "mnovo"); here only as the word's ending.
            if i + 3 == w.len() {
                let tri: String = w[i..i + 3].iter().collect();
                let rep = match tri.as_str() {
                    "ого" => Some("ovo"),
                    "Ого" => Some("Ovo"),
                    "его" => Some("evo"),
                    "Его" => Some("Evo"),
                    _ => None,
                };
                if let Some(r) = rep
                    && i > 0
                {
                    out.push_str(r);
                    i += 3;
                    continue;
                }
            }
            let c = w[i];
            if (c == 'е' || c == 'Е') && i == 0 {
                out.push_str(if c == 'е' { "ye" } else { "Ye" });
            } else {
                general(c, out);
            }
            i += 1;
        }
    })
}

fn romanize_ukrainian(text: &str) -> String {
    romanize_with(text, |w, out| {
        for (i, &c) in w.iter().enumerate() {
            if i > 0 && w[i - 1].is_alphabetic() && !is_cyrillic_vowel(w[i - 1]) {
                let soft = match c {
                    'Ю' => Some("Iu"),
                    'ю' => Some("iu"),
                    'Я' => Some("Ia"),
                    'я' => Some("ia"),
                    _ => None,
                };
                if let Some(s) = soft {
                    out.push_str(s);
                    continue;
                }
            }
            match lookup(UKRAINIAN_MAP, c) {
                Some(v) => out.push_str(v),
                None => general(c, out),
            }
        }
    })
}

fn romanize_table(text: &str, map: &'static [(char, &'static str)], ye_at_start: bool) -> String {
    romanize_with(text, |w, out| {
        for (i, &c) in w.iter().enumerate() {
            if ye_at_start && i == 0 && (c == 'е' || c == 'Е') {
                out.push_str(if c == 'е' { "ye" } else { "Ye" });
                continue;
            }
            match lookup(map, c) {
                Some(v) => out.push_str(v),
                None => general(c, out),
            }
        }
    })
}

/// Cyrillic → Latin, the language told by the letters (Android `romanizeCyrillic`). `None` for
/// text without Cyrillic, a lone "е", or letters no supported alphabet has.
pub fn romanize_cyrillic(text: &str) -> Option<String> {
    let cyr: Vec<char> = text.chars().filter(|c| is_cyr(*c)).collect();
    if cyr.is_empty() || (cyr.len() == 1 && (cyr[0] == 'е' || cyr[0] == 'Е')) {
        return None;
    }
    Some(if is_russian(text) {
        romanize_russian(text)
    } else if is_ukrainian(text) {
        romanize_ukrainian(text)
    } else if is_serbian(text) {
        romanize_table(text, SERBIAN_MAP, false)
    } else if is_bulgarian(text) {
        romanize_table(text, BULGARIAN_MAP, false)
    } else if is_belarusian(text) {
        romanize_table(text, BELARUSIAN_MAP, true)
    } else if is_kyrgyz(text) {
        romanize_table(text, KYRGYZ_MAP, false)
    } else if is_macedonian(text) {
        romanize_table(text, MACEDONIAN_MAP, false)
    } else {
        return None;
    })
}

// ---------------------------------------------------------------------------
// Hindi / Punjabi
// ---------------------------------------------------------------------------

#[rustfmt::skip]
static DEVANAGARI: &[(&str, &str)] = &[
    ("अ", "a"), ("आ", "aa"), ("इ", "i"), ("ई", "ee"), ("उ", "u"), ("ऊ", "oo"),
    ("ऋ", "ri"), ("ए", "e"), ("ऐ", "ai"), ("ओ", "o"), ("औ", "au"),
    ("क", "k"), ("ख", "kh"), ("ग", "g"), ("घ", "gh"), ("ङ", "ng"),
    ("च", "ch"), ("छ", "chh"), ("ज", "j"), ("झ", "jh"), ("ञ", "ny"),
    ("ट", "t"), ("ठ", "th"), ("ड", "d"), ("ढ", "dh"), ("ण", "n"),
    ("त", "t"), ("थ", "th"), ("द", "d"), ("ध", "dh"), ("न", "n"),
    ("प", "p"), ("फ", "ph"), ("ब", "b"), ("भ", "bh"), ("म", "m"),
    ("य", "y"), ("र", "r"), ("ल", "l"), ("व", "v"),
    ("श", "sh"), ("ष", "sh"), ("स", "s"), ("ह", "h"),
    ("क्ष", "ksh"), ("त्र", "tr"), ("ज्ञ", "gy"), ("श्र", "shr"),
    ("ा", "aa"), ("ि", "i"), ("ी", "ee"), ("ु", "u"), ("ू", "oo"),
    ("ृ", "ri"), ("े", "e"), ("ै", "ai"), ("ो", "o"), ("ौ", "au"),
    ("ं", "n"), ("ः", "h"), ("ँ", "n"), ("़", ""), ("्", ""),
    ("०", "0"), ("१", "1"), ("२", "2"), ("३", "3"), ("४", "4"),
    ("५", "5"), ("६", "6"), ("७", "7"), ("८", "8"), ("९", "9"),
    ("ॐ", "Om"), ("ऽ", ""),
    ("क़", "q"), ("ख़", "kh"), ("ग़", "g"), ("ज़", "z"), ("ड़", "r"), ("ढ़", "rh"), ("फ़", "f"), ("य़", "y"),
    ("क\u{093C}", "q"), ("ख\u{093C}", "kh"), ("ग\u{093C}", "g"), ("ज\u{093C}", "z"), ("ड\u{093C}", "r"),
    ("ढ\u{093C}", "rh"), ("फ\u{093C}", "f"), ("य\u{093C}", "y"),
];

#[rustfmt::skip]
static GURMUKHI: &[(&str, &str)] = &[
    ("ੳ", "o"), ("ਅ", "a"), ("ੲ", "e"), ("ਸ", "s"), ("ਹ", "h"),
    ("ਕ", "k"), ("ਖ", "kh"), ("ਗ", "g"), ("ਘ", "gh"), ("ਙ", "ng"),
    ("ਚ", "ch"), ("ਛ", "chh"), ("ਜ", "j"), ("ਝ", "jh"), ("ਞ", "ny"),
    ("ਟ", "t"), ("ਠ", "th"), ("ਡ", "d"), ("ਢ", "dh"), ("ਣ", "n"),
    ("ਤ", "t"), ("ਥ", "th"), ("ਦ", "d"), ("ਧ", "dh"), ("ਨ", "n"),
    ("ਪ", "p"), ("ਫ", "ph"), ("ਬ", "b"), ("ਭ", "bh"), ("ਮ", "m"),
    ("ਯ", "y"), ("ਰ", "r"), ("ਲ", "l"), ("ਵ", "v"), ("ੜ", "r"),
    ("ਸ਼", "sh"), ("ਖ਼", "kh"), ("ਗ਼", "g"), ("ਜ਼", "z"), ("ਫ਼", "f"), ("ਲ਼", "l"),
    ("ਾ", "aa"), ("ਿ", "i"), ("ੀ", "ee"), ("ੁ", "u"), ("ੂ", "oo"),
    ("ੇ", "e"), ("ੈ", "ai"), ("ੋ", "o"), ("ੌ", "au"),
    ("ੰ", "n"), ("ਂ", "n"), ("ੱ", ""), ("੍", ""), ("਼", ""),
    ("ੴ", "Ek Onkar"),
    ("੦", "0"), ("੧", "1"), ("੨", "2"), ("੩", "3"), ("੪", "4"),
    ("੫", "5"), ("੬", "6"), ("੭", "7"), ("੮", "8"), ("੯", "9"),
];

static DEVANAGARI_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| table(DEVANAGARI));
static GURMUKHI_MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| table(GURMUKHI));

/// Two characters looked up first, then one; unknown characters kept.
fn two_then_one(map: &HashMap<&str, &str>, out: &mut String, chars: &[char], i: &mut usize) {
    if *i + 1 < chars.len() {
        let two: String = chars[*i..*i + 2].iter().collect();
        if let Some(v) = map.get(two.as_str()) {
            out.push_str(v);
            *i += 2;
            return;
        }
    }
    let one = chars[*i].to_string();
    out.push_str(map.get(one.as_str()).copied().unwrap_or(one.as_str()));
    *i += 1;
}

/// Devanagari → Latin (Android `romanizeHindi`).
pub fn romanize_hindi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        two_then_one(&DEVANAGARI_MAP, &mut out, &chars, &mut i);
    }
    out
}

/// Gurmukhi → Latin (Android `romanizePunjabi`); the addak (ੱ) doubles the next consonant.
pub fn romanize_punjabi(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\u{0A71}' {
            if let Some(next) = chars.get(i + 1)
                && let Some(m) = GURMUKHI_MAP.get(next.to_string().as_str())
                && let Some(first) = m.chars().next()
            {
                out.push(first);
            }
            i += 1;
            continue;
        }
        two_then_one(&GURMUKHI_MAP, &mut out, &chars, &mut i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_romaji() {
        assert_eq!(romanize_japanese("こんにちは世界"), "konnichiha sekai");
        let r = romanize_japanese("夜に走る");
        assert!(r.starts_with("yoru"), "{r}");
        assert!(r.contains("hashi"), "{r}");
        // katakana loanword + latin kept
        let r = romanize_japanese("ハッピー day");
        assert!(r.starts_with("happii") || r.starts_with("happī") || r.starts_with("happy"), "{r}");
        assert!(r.ends_with("day"), "{r}");
        // small っ at the end of a kakasi chunk doubles the next consonant
        assert_eq!(romanize_japanese("昨日会った"), "kinou atta");
        assert_eq!(romanize_japanese("全部分かった"), "zenbu wakatta");
        assert_eq!(romanize_japanese("ちょっと待って"), "chotto matte");
        // a real つ is left alone
        assert_eq!(romanize_japanese("夏が来た"), "natsu ga kita");
    }

    #[test]
    fn korean_revised_romanization() {
        assert_eq!(romanize_korean("안녕하세요"), "annyeonghaseyo");
        assert_eq!(romanize_korean("사랑해"), "saranghae");
        // final consonant before ᄋ carries over: 음악 → eumak
        assert_eq!(romanize_korean("음악"), "eumak");
        // ㄱ before ㄴ nasalises: 한국말 → hangungmal
        assert_eq!(romanize_korean("한국말"), "hangungmal");
        // non-Hangul text passes through and flushes the final
        assert_eq!(romanize_korean("밤 love"), "bam love");
    }

    #[test]
    fn chinese_pinyin() {
        assert_eq!(romanize_chinese("我爱你"), "wo ai ni");
        assert_eq!(romanize_chinese("你好，世界！"), "ni hao，shi jie！");
    }

    #[test]
    fn cyrillic_languages() {
        assert_eq!(romanize_cyrillic("Привет, мир").as_deref(), Some("Privet, mir"));
        assert_eq!(romanize_cyrillic("его сердце").as_deref(), Some("yego serdtse"));
        assert_eq!(romanize_cyrillic("нового").as_deref(), Some("novovo"));
        // only a word-final -ого is said -ovo
        assert_eq!(romanize_cyrillic("огонь").as_deref(), Some("ogonʹ"));
        // Ukrainian: г → h, ї → yi
        assert_eq!(romanize_cyrillic("Україна").as_deref(), Some("Ukrayina"));
        // Serbian: ђ/ј only exist there
        assert_eq!(romanize_cyrillic("Љубав је").as_deref(), Some("Ljubav ye"));
        assert_eq!(romanize_cyrillic("hello"), None);
        assert_eq!(romanize_cyrillic("е"), None);
    }

    #[test]
    fn hindi_and_punjabi() {
        // table transliteration without the inherent vowel, like Android
        assert_eq!(romanize_hindi("नमस्ते"), "nmste");
        assert_eq!(romanize_hindi("आप"), "aap");
        assert_eq!(romanize_punjabi("ਸਤਿ"), "sti");
        assert!(is_punjabi("ਪੰਜਾਬ"));
    }

    #[test]
    fn whole_song_decides_language() {
        // kanji-only line in a Japanese song is still read as Japanese
        let lines = ["夜に走る", "海のように青い空", "二つの星"];
        assert_eq!(song_language(&lines.join(" ")), Some(RomanizeLang::Japanese));
        let out = romanize_song(&lines, |_| true);
        assert!(out.iter().all(|o| o.is_some()));
        assert!(out[2].as_deref().unwrap().contains("hoshi"), "{:?}", out[2]);

        let ko = ["사랑해 baby", "", "I love you"];
        let out = romanize_song(&ko, |_| true);
        assert_eq!(out[0].as_deref(), Some("saranghae baby"));
        assert_eq!(out[1], None);
        // an English line reads the same → no reading
        assert_eq!(out[2], None);

        // switched off
        let out = romanize_song(&ko, |l| l != RomanizeLang::Korean);
        assert!(out.iter().all(Option::is_none));

        // English song → nothing
        assert_eq!(song_language("hello darkness my old friend"), None);
        assert_eq!(song_language("Я тебя люблю"), Some(RomanizeLang::Russian));
        assert_eq!(song_language("Я тебе кохаю, Україна"), Some(RomanizeLang::Ukrainian));
        assert_eq!(song_language("我爱你中国"), Some(RomanizeLang::Chinese));
    }

    #[test]
    fn lang_ids_roundtrip() {
        for l in RomanizeLang::ALL {
            assert_eq!(RomanizeLang::from_id(l.id()), Some(l));
        }
    }
}
