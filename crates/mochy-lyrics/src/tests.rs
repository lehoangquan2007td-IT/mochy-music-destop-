//! Unit tests: LRC dialect, round-trip, TTML, active line, scoring.

use mochy_core::models::TrackMeta;

use crate::script::{
    Script, dominant_script, has_distinct_voices, is_syllable_split, latin_language, lyrics_script,
};
use crate::*;

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn texts(lines: &[LyricLine]) -> Vec<&str> {
    lines.iter().map(|l| l.text.as_str()).collect()
}

// ---------------------------------------------------------------------------
// parse_lrc
// ---------------------------------------------------------------------------

#[test]
fn lrc_standard_lines_metadata_and_fractions() {
    let src = "[ti:Song]\n[ar:Artist]\n[offset:+100]\n[by:someone]\n\
               [00:01.50]two digit\n[00:02.250]three digit\n[01:03.07] leading space\n[2:04]no fraction\n[00:05:10]colon fraction";
    let (lines, sync) = parse_lrc(src);
    assert_eq!(sync, SyncKind::Line);
    assert_eq!(
        texts(&lines),
        [
            "two digit",
            "three digit",
            "colon fraction",
            "leading space",
            "no fraction"
        ]
    );
    assert!(approx(lines[0].time, 1.5));
    assert!(approx(lines[1].time, 2.25));
    assert!(approx(lines[2].time, 5.1));
    assert!(approx(lines[3].time, 63.07));
    assert!(approx(lines[4].time, 124.0));
    assert!(
        lines
            .iter()
            .all(|l| l.words.is_empty() && !l.bg && l.agent.is_none())
    );
}

#[test]
fn lrc_multiple_timestamps_are_expanded_and_sorted() {
    let (lines, sync) = parse_lrc("[00:30.00][00:10.00]Chorus\n[00:20.00]Verse");
    assert_eq!(sync, SyncKind::Line);
    assert_eq!(texts(&lines), ["Chorus", "Verse", "Chorus"]);
    assert!(
        approx(lines[0].time, 10.0) && approx(lines[1].time, 20.0) && approx(lines[2].time, 30.0)
    );
}

#[test]
fn lrc_word_timings_on_next_line() {
    let src = "[00:01.00]Hello world\n<Hello:1.0:1.5|world:1.5:2.25>\n[00:03.00]Next line here\n<Next:3.0:3.2|line:3.2:3.5|here:3.5:4.0>";
    let (lines, sync) = parse_lrc(src);
    assert_eq!(sync, SyncKind::Word);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].words,
        vec![
            LyricWord {
                start: 1.0,
                end: 1.5,
                text: "Hello".into()
            },
            LyricWord {
                start: 1.5,
                end: 2.25,
                text: "world".into()
            },
        ]
    );
    assert_eq!(lines[1].words.len(), 3);
}

#[test]
fn lrc_word_text_may_contain_colon() {
    let (lines, _) = parse_lrc("[00:01.00]Ratio 1:2 ok\n<Ratio:1:1.2|1:2:1.2:1.6|ok:1.6:2>");
    assert_eq!(lines[0].words[1].text, "1:2");
    assert!(approx(lines[0].words[1].start, 1.2));
}

#[test]
fn lrc_agent_and_bg_markers() {
    let src = "[00:01.00]{agent:v1}Lead line\n[00:02.00]{agent:v2}Answer line\n[00:02.50]{bg}(ooh ooh)\n[00:03.00]{agent:v1}{bg}tagged bg";
    let (lines, _) = parse_lrc(src);
    assert_eq!(
        texts(&lines),
        ["Lead line", "Answer line", "(ooh ooh)", "tagged bg"]
    );
    assert_eq!(lines[0].agent.as_deref(), Some("v1"));
    assert_eq!(lines[1].agent.as_deref(), Some("v2"));
    assert!(!lines[1].bg);
    assert!(lines[2].bg && lines[2].agent.is_none());
    assert!(lines[3].bg && lines[3].agent.as_deref() == Some("v1"));
    assert!(has_distinct_voices(src));
    assert!(!has_distinct_voices(
        "[00:01.00]{agent:v1}a\n[00:02.00]{agent:v1}b"
    ));
}

#[test]
fn lrc_rich_sync_enhanced() {
    let src = "[ti:x]\n[00:10.00]{agent:v2}<00:10.00>Hello <00:10.50>big <00:11.00>world<00:11.80>\n[00:12.00]<00:12.00>Second <00:12.40>line\n[00:15.000]{bg}<00:15.000>ah <00:15.300>ah";
    let (lines, sync) = parse_lrc(src);
    assert_eq!(sync, SyncKind::Word);
    assert_eq!(texts(&lines), ["Hello big world", "Second line", "ah ah"]);
    assert_eq!(lines[0].agent.as_deref(), Some("v2"));
    let w = &lines[0].words;
    assert_eq!(w.len(), 3);
    assert!(approx(w[0].start, 10.0) && approx(w[0].end, 10.5));
    // trailing end stamp closes the last word
    assert!(approx(w[2].start, 11.0) && approx(w[2].end, 11.8));
    // last word of a line without an end stamp runs to the next line
    assert!(approx(lines[1].words[1].end, 15.0));
    // last word of the last line: +0.5 s
    assert!(lines[2].bg);
    assert!(approx(lines[2].words[1].end, 15.8));
}

#[test]
fn lrc_all_same_timestamp_is_plain() {
    let (lines, sync) = parse_lrc("[00:00.00]one\n[00:00.00]two\n[00:00.00]three");
    assert_eq!(sync, SyncKind::Plain);
    assert_eq!(lines.len(), 3);
}

#[test]
fn lrc_untimed_text_is_plain_and_blank_is_none() {
    let (lines, sync) = parse_lrc("[ti:Title]\nFirst line\n\nSecond line\n");
    assert_eq!(sync, SyncKind::Plain);
    assert_eq!(texts(&lines), ["First line", "Second line"]);
    assert!(lines.iter().all(|l| l.time == 0.0));
    assert_eq!(parse_lrc("   \n "), (vec![], SyncKind::None));
    assert_eq!(parse_lrc(LYRICS_NOT_FOUND), (vec![], SyncKind::None));
    assert_eq!(parse_lrc("[ti:only metadata]").1, SyncKind::None);
}

#[test]
fn lrc_escaped_and_entities() {
    let (lines, _) = parse_lrc("\"[00:01.00]Rock &amp; roll\\n[00:02.00]it&#39;s &#x263A;\"");
    assert_eq!(texts(&lines), ["Rock & roll", "it's \u{263A}"]);
}

#[test]
fn lrc_one_word_timing_line_is_still_line_synced() {
    let src = "[00:01.00]a b\n<a:1:1.5|b:1.5:2>\n[00:02.00]c\n[00:03.00]d\n[00:04.00]e";
    assert_eq!(parse_lrc(src).1, SyncKind::Line);
}

#[test]
fn lrc_syllables_glued_in_text_merge_into_words() {
    // "a gain" timed as two pieces, written "again" in the text
    let src = "[00:01.00]Let down again\n<Let:1:1.2|down:1.2:1.5|a:1.5:1.7|gain:1.7:2.0>";
    let (lines, sync) = parse_lrc(src);
    assert_eq!(sync, SyncKind::Word);
    let w: Vec<&str> = lines[0].words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(w, ["Let", "down", "again"]);
    assert!(approx(lines[0].words[2].start, 1.5) && approx(lines[0].words[2].end, 2.0));
}

#[test]
fn lrc_dropped_spaces_are_restored() {
    let src = "[00:01.00]Iloveyou\n<I:1:1.1|love:1.1:1.3|you:1.3:1.5>\n\
               [00:02.00]Holdmetight\n<Hold:2:2.1|me:2.1:2.3|tight:2.3:2.5>\n\
               [00:03.00]Don'tletgo\n<Don't:3:3.1|let:3.1:3.3|go:3.3:3.5>";
    let (lines, _) = parse_lrc(src);
    assert_eq!(
        texts(&lines),
        ["I love you", "Hold me tight", "Don't let go"]
    );
}

#[test]
fn lrc_cjk_pieces_are_not_merged() {
    let (lines, sync) =
        parse_lrc("[00:01.00]夜に駆ける\n<夜:1:1.2|に:1.2:1.4|駆:1.4:1.6|け:1.6:1.8|る:1.8:2>");
    assert_eq!(sync, SyncKind::Word);
    assert_eq!(lines[0].words.len(), 5);
}

// ---------------------------------------------------------------------------
// lines_to_lrc round-trip
// ---------------------------------------------------------------------------

fn sample_lines() -> Vec<LyricLine> {
    vec![
        LyricLine {
            time: 1.5,
            text: "Hello there world".into(),
            words: vec![
                LyricWord {
                    start: 1.5,
                    end: 1.9,
                    text: "Hello".into(),
                },
                LyricWord {
                    start: 1.9,
                    end: 2.3,
                    text: "there".into(),
                },
                LyricWord {
                    start: 2.3,
                    end: 3.125,
                    text: "world".into(),
                },
            ],
            bg: false,
            agent: Some("v1".into()),
        },
        LyricLine {
            time: 2.0,
            text: "(echo echo)".into(),
            words: vec![
                LyricWord {
                    start: 2.0,
                    end: 2.4,
                    text: "(echo".into(),
                },
                LyricWord {
                    start: 2.4,
                    end: 2.8,
                    text: "echo)".into(),
                },
            ],
            bg: true,
            agent: None,
        },
        LyricLine {
            time: 65.432,
            text: "Second voice, time 1:30".into(),
            words: vec![
                LyricWord {
                    start: 65.432,
                    end: 66.0,
                    text: "Second".into(),
                },
                LyricWord {
                    start: 66.0,
                    end: 66.5,
                    text: "voice,".into(),
                },
                LyricWord {
                    start: 66.5,
                    end: 67.0,
                    text: "time".into(),
                },
                LyricWord {
                    start: 67.0,
                    end: 68.0,
                    text: "1:30".into(),
                },
            ],
            bg: false,
            agent: Some("v2".into()),
        },
        LyricLine {
            time: 6001.0,
            text: "Line-only over 100 minutes".into(),
            ..Default::default()
        },
    ]
}

#[test]
fn round_trip_constructed_lines() {
    let lines = sample_lines();
    let lrc = lines_to_lrc(&lines);
    assert!(
        lrc.starts_with(
            "[00:01.500]{agent:v1}Hello there world\n<Hello:1.5:1.9|there:1.9:2.3|world:2.3:3.125>"
        ),
        "{lrc}"
    );
    assert!(lrc.contains("[00:02.000]{bg}(echo echo)"));
    assert!(lrc.contains("[100:01.000]Line-only over 100 minutes"));
    let (back, sync) = parse_lrc(&lrc);
    assert_eq!(sync, SyncKind::Word);
    assert_eq!(back, lines);
    assert_eq!(lines_to_lrc(&back), lrc);
}

#[test]
fn round_trip_parsed_payloads() {
    for src in [
        "[00:10.00]{agent:v2}<00:10.00>Hello <00:10.50>big <00:11.00>world<00:11.80>\n[00:12.00]{bg}<00:12.00>Second <00:12.40>line",
        "[00:01.00]Let down again\n<Let:1:1.2|down:1.2:1.5|a:1.5:1.7|gain:1.7:2.0>\n[00:03.00]plain line",
        "[00:30.00][00:10.00]Chorus\n[00:20.00]Verse",
        "[00:00.00]one\n[00:00.00]two",
    ] {
        let (lines, sync) = parse_lrc(src);
        let (again, sync2) = parse_lrc(&lines_to_lrc(&lines));
        assert_eq!(again, lines, "round-trip of {src:?}");
        assert_eq!(sync2, sync);
    }
}

#[test]
fn lyrics_from_lrc_and_to_lrc() {
    let l = Lyrics::from_lrc("LrcLib", "First\nSecond").unwrap();
    assert_eq!(l.sync, SyncKind::Plain);
    assert_eq!(l.plain.as_deref(), Some("First\nSecond"));
    assert_eq!(l.to_lrc(), "First\nSecond");
    assert!(Lyrics::from_lrc("x", LYRICS_NOT_FOUND).is_none());
    let src = "[00:01.000]a\n[00:02.000]b";
    let l = Lyrics::from_lrc("x", src).unwrap();
    assert_eq!((l.sync, l.to_lrc().as_str()), (SyncKind::Line, src));
}

// ---------------------------------------------------------------------------
// parse_ttml
// ---------------------------------------------------------------------------

const TTML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word" xml:lang="en">
<head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/></metadata></head>
<body dur="3:10.000"><div begin="0:12.000" end="0:30.000" itunes:songPart="Verse">
<p begin="0:12.000" end="0:14.500" ttm:agent="v1" itunes:key="L1"><span begin="0:12.000" end="0:12.400">Tell</span> <span begin="0:12.400" end="0:12.700">me</span> <span begin="0:12.700" end="0:13.100">some</span><span begin="0:13.100" end="0:13.600">thing,</span> <span begin="0:13.600" end="0:14.500">girl</span><span ttm:role="x-bg" begin="0:14.000" end="0:15.000"><span begin="0:14.000" end="0:14.400">(ooh</span> <span begin="0:14.400" end="0:15.000">yeah)</span></span><span ttm:role="x-translation" xml:lang="fr">Dis-moi</span></p>
<p begin="15.25s" end="17s" ttm:agent="v2"><span begin="15.25s" end="16s">I&apos;m</span> <span begin="16s" end="17s">off</span></p>
<p begin="00:00:18.000" end="00:00:20.000">Untimed words here</p>
<p begin="0:21.000" end="0:22.000" ttm:agent="v1"><span begin="0:21.000" end="0:21.300">夜</span><span begin="0:21.300" end="0:21.600">に</span><span begin="0:21.600" end="0:22.000">駆ける</span></p>
<p end="0:30.000"><span>no begin: skipped</span></p>
</div></body></tt>"#;

#[test]
fn ttml_word_sync_agents_and_background() {
    let lines = parse_ttml(TTML).unwrap();
    assert_eq!(
        texts(&lines),
        [
            "Tell me something, girl",
            "(ooh yeah)",
            "I'm off",
            "Untimed words here",
            "夜に駆ける"
        ]
    );

    let l0 = &lines[0];
    assert!(approx(l0.time, 12.0));
    assert_eq!(l0.agent.as_deref(), Some("v1"));
    assert!(!l0.bg);
    let w: Vec<&str> = l0.words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(
        w,
        ["Tell", "me", "something,", "girl"],
        "syllable spans glued without a space merge"
    );
    assert!(approx(l0.words[2].start, 12.7) && approx(l0.words[2].end, 13.6));

    let bg = &lines[1];
    assert!(bg.bg && bg.agent.is_none());
    assert!(approx(bg.time, 14.0));
    assert_eq!(bg.words.len(), 2);

    let l2 = &lines[2];
    assert_eq!(l2.agent.as_deref(), Some("v2"));
    assert!(approx(l2.time, 15.25) && approx(l2.words[1].end, 17.0));

    assert!(approx(lines[3].time, 18.0) && lines[3].words.is_empty());
    // CJK pieces stay separate words (port deviation, see ttml.rs)
    assert_eq!(lines[4].words.len(), 3);

    // and the LRC it produces is word-synced with real duet voices
    let lrc = lines_to_lrc(&lines);
    let (parsed, sync) = parse_lrc(&lrc);
    assert_eq!(sync, SyncKind::Word);
    assert_eq!(parsed, lines);
    assert!(has_distinct_voices(&lrc));
}

#[test]
fn ttml_time_formats_and_errors() {
    use crate::ttml::parse_time;
    assert!(approx(parse_time("1:02.345"), 62.345));
    assert!(approx(parse_time("62.345s"), 62.345));
    assert!(approx(parse_time("00:01:02.345"), 62.345));
    assert!(approx(parse_time("62.345"), 62.345));
    assert!(approx(parse_time("1500ms"), 1.5));
    assert!(parse_ttml("{\"not\":\"xml\"}").is_err());
    assert!(parse_ttml("<html><p begin=\"1\">x</p></html>").is_err());
    assert_eq!(parse_ttml("<tt><body></body></tt>").unwrap(), vec![]);
}

// ---------------------------------------------------------------------------
// active_line_index
// ---------------------------------------------------------------------------

fn timed(times: &[f64]) -> Vec<LyricLine> {
    times
        .iter()
        .map(|t| LyricLine {
            time: *t,
            text: format!("at {t}"),
            ..Default::default()
        })
        .collect()
}

#[test]
fn active_line_edges() {
    let lines = timed(&[10.0, 20.0, 30.0]);
    assert_eq!(active_line_index(&[], 5.0, 0), None);
    assert_eq!(active_line_index(&lines, 0.0, 0), None, "before first line");
    assert_eq!(
        active_line_index(&lines, 9.6, 0),
        None,
        "just before the lead-in"
    );
    assert_eq!(
        active_line_index(&lines, 9.7, 0),
        None,
        "exactly 300 ms early is not yet active"
    );
    assert_eq!(
        active_line_index(&lines, 9.701, 0),
        Some(0),
        "lead-in: lit 300 ms early"
    );
    assert_eq!(active_line_index(&lines, 15.0, 0), Some(0));
    assert_eq!(
        active_line_index(&lines, 19.8, 0),
        Some(1),
        "next line lights inside its lead-in"
    );
    assert_eq!(active_line_index(&lines, 25.0, 0), Some(1));
    assert_eq!(
        active_line_index(&lines, 400.0, 0),
        Some(2),
        "past the end stays on the last line"
    );
}

#[test]
fn active_line_offset() {
    let lines = timed(&[10.0, 20.0, 30.0]);
    // positive offset shows lines earlier
    assert_eq!(active_line_index(&lines, 9.0, 1000), Some(0));
    assert_eq!(active_line_index(&lines, 18.0, 2000), Some(1));
    // negative offset delays them
    assert_eq!(active_line_index(&lines, 10.5, -1000), None);
    assert_eq!(active_line_index(&lines, 20.5, -1000), Some(0));
    assert_eq!(
        active_line_index(&lines, 21.0, -1000),
        Some(1),
        "20.0 s + 300 ms lead-in"
    );
}

#[test]
fn line_and_lyrics_end() {
    let mut lines = timed(&[10.0, 20.0]);
    assert!(approx(line_end_secs(&lines, 0), 20.0));
    assert!(approx(lyrics_end_secs(&lines), 25.0));
    lines[1].words = vec![LyricWord {
        start: 20.0,
        end: 23.5,
        text: "x".into(),
    }];
    assert!(approx(lyrics_end_secs(&lines), 23.5));
    assert_eq!(lyrics_end_secs(&[]), 0.0);
}

// ---------------------------------------------------------------------------
// Script / language
// ---------------------------------------------------------------------------

#[test]
fn scripts_and_languages() {
    assert_eq!(dominant_script("夜に駆ける"), Script::Kana);
    assert_eq!(dominant_script("晴天"), Script::Han);
    assert_eq!(dominant_script("사랑해 love"), Script::Hangul);
    assert_eq!(dominant_script("Лето"), Script::Cyrillic);
    assert_eq!(dominant_script("123 !!"), Script::Unknown);
    assert_eq!(
        lyrics_script("[00:01.00]{agent:v1}故事的小黄花\n[00:02.00]{agent:v1}从出生那年"),
        Script::Han
    );
    let en = "[00:01.00]I know you want me and you know that I want you\n[00:02.00]Just don't let me go, never";
    assert_eq!(latin_language(en), Some("en"));
    let vi = "[00:01.00]Người theo hương hoa mây mù giăng lối\n[00:02.00]Anh yêu em không là của một người trong mình như này";
    assert_eq!(latin_language(vi), Some("vi"));
    assert_eq!(latin_language("[00:01.00]short"), None);
}

#[test]
fn syllable_split_detection() {
    let split = (0..5)
        .map(|i| format!("[00:0{i}.00]wa nt yo u ba by I lo ve"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(is_syllable_split(&split));
    let normal = (0..5)
        .map(|i| format!("[00:0{i}.00]I really want you baby tonight"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!is_syllable_split(&normal));
}

// ---------------------------------------------------------------------------
// Scoring / winner selection (mocked provider results)
// ---------------------------------------------------------------------------

fn meta(title: &str, artist: &str) -> TrackMeta {
    TrackMeta {
        title: title.into(),
        artists: vec![artist.into()],
        album: None,
        duration_secs: Some(200.0),
        video_id: None,
    }
}

const EN_LINE: &str = "[00:01.00]I know you want me and you know that I want you\n[00:05.00]Just don't let me go now, never let me go\n[00:09.00]You are the one that I want, baby\n[00:13.00]And I can't stop now";
const EN_WORD: &str = "[00:01.00]I know you want me and you know that I want you\n<I:1:1.1|know:1.1:1.3|you:1.3:1.5|want:1.5:1.7|me:1.7:1.9|and:1.9:2|you:2:2.1|know:2.1:2.2|that:2.2:2.3|I:2.3:2.4|want:2.4:2.5|you:2.5:2.6>\n[00:05.00]Just don't let me go now, never let me go\n<Just:5:5.1|don't:5.1:5.2|let:5.2:5.3|me:5.3:5.4|go:5.4:5.5|now,:5.5:5.6|never:5.6:5.7|let:5.7:5.8|me:5.8:5.9|go:5.9:6>";
const ZH_LINE: &str = "[00:01.00]我知道你想要我\n[00:05.00]不要让我走\n[00:09.00]你是我想要的人\n[00:13.00]我停不下来";
const ZH_WORD: &str = "[00:01.00]我知道你想要我\n<我:1:1.1|知:1.1:1.2|道:1.2:1.3|你:1.3:1.4>\n[00:05.00]不要让我走\n<不:5:5.1|要:5.1:5.2|让:5.2:5.3>";

fn cand(p: ProviderId, raw: &str, order: usize) -> Candidate {
    Candidate::new(p, raw, order).expect("valid candidate")
}

fn order_of(p: ProviderId) -> usize {
    ProviderId::all().iter().position(|x| *x == p).unwrap()
}

fn winner(meta: &TrackMeta, cands: &[Candidate]) -> ProviderId {
    pick_best(meta, cands, ProviderId::all().len())
        .expect("a winner")
        .provider
}

#[test]
fn candidate_grading() {
    assert!(Candidate::new(ProviderId::LrcLib, "", 0).is_none());
    assert!(Candidate::new(ProviderId::LrcLib, LYRICS_NOT_FOUND, 0).is_none());
    let c = cand(ProviderId::LrcLib, EN_WORD, 0);
    assert_eq!(
        (c.sync, c.script, c.language),
        (SyncKind::Word, Script::Latin, Some("en"))
    );
    assert!(pick_best(&meta("a", "b"), &[], 13).is_none());
}

#[test]
fn sync_beats_provider_order() {
    let m = meta("Song", "Artist");
    let cands = vec![
        cand(
            ProviderId::BetterLyrics,
            EN_LINE,
            order_of(ProviderId::BetterLyrics),
        ),
        cand(ProviderId::LrcLib, EN_WORD, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
}

#[test]
fn order_breaks_ties() {
    let m = meta("Song", "Artist");
    let cands = vec![
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
        cand(
            ProviderId::YouLyPlus,
            EN_LINE,
            order_of(ProviderId::YouLyPlus),
        ),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::YouLyPlus);
}

#[test]
fn duet_tags_beat_order() {
    let m = meta("Song", "Artist");
    let duet = "[00:01.00]{agent:v1}I know you want me and you know that I want you\n[00:05.00]{agent:v2}Just don't let me go now, never let me go\n[00:09.00]{agent:v1}You are the one that I want, baby";
    let cands = vec![
        cand(
            ProviderId::YouLyPlus,
            EN_LINE,
            order_of(ProviderId::YouLyPlus),
        ),
        cand(ProviderId::LrcLib, duet, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
}

#[test]
fn chinese_translation_loses_for_latin_song() {
    // Chinese services answering an English song in Chinese (even word-timed) are not believed.
    let m = meta("Shape of You", "Ed Sheeran");
    let cands = vec![
        cand(ProviderId::NetEase, ZH_WORD, order_of(ProviderId::NetEase)),
        cand(ProviderId::Kugou, ZH_LINE, order_of(ProviderId::Kugou)),
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
    // ...and their non-Chinese word timing joins in when it's in the song's language
    let cands = vec![
        cand(ProviderId::NetEase, EN_WORD, order_of(ProviderId::NetEase)),
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::NetEase);
}

#[test]
fn chinese_song_prefers_chinese_sources_and_script() {
    let m = meta("晴天", "周杰伦");
    let cands = vec![
        cand(ProviderId::LrcLib, EN_WORD, order_of(ProviderId::LrcLib)), // a translation
        cand(ProviderId::Deezer, ZH_LINE, order_of(ProviderId::Deezer)),
        cand(ProviderId::Kugou, ZH_LINE, order_of(ProviderId::Kugou)),
    ];
    // script beats sync; among same script+sync the Chinese services win for a Chinese song
    assert_eq!(winner(&m, &cands), ProviderId::Kugou);
}

#[test]
fn japanese_title_prefers_kana_over_romaji() {
    let m = meta("夜に駆ける", "YOASOBI");
    let ja = "[00:01.00]沈むように溶けてゆくように\n[00:05.00]二人だけの空が広がる夜に";
    let romaji = "[00:01.00]Shizumu you ni tokete yuku you ni\n<Shizumu:1:1.5|you:1.5:1.7|ni:1.7:2>\n[00:05.00]Futari dake no sora ga hirogaru yoru ni\n<Futari:5:5.5|dake:5.5:6>";
    let cands = vec![
        cand(
            ProviderId::BetterLyrics,
            romaji,
            order_of(ProviderId::BetterLyrics),
        ),
        cand(ProviderId::LrcLib, ja, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
}

#[test]
fn majority_language_wins_without_metadata_evidence() {
    // Latin title; two sources say English, one (better synced) says Indonesian.
    let m = meta("Untitled", "Someone");
    let id_word = "[00:01.00]aku dan kamu yang selalu ada dalam hati\n<aku:1:1.1|dan:1.1:1.2|kamu:1.2:1.3>\n[00:05.00]jangan pergi karena cinta kita tidak akan hilang\n<jangan:5:5.1|pergi:5.1:5.2>";
    let cands = vec![
        cand(
            ProviderId::BetterLyrics,
            id_word,
            order_of(ProviderId::BetterLyrics),
        ),
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
        cand(ProviderId::Deezer, EN_LINE, order_of(ProviderId::Deezer)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
}

#[test]
fn syllable_split_word_sync_is_demoted() {
    let m = meta("Song", "Artist");
    let split = (0..5)
        .map(|i| format!("[00:0{i}.00]wa nt yo u ba by I lo ve\n<wa:{i}:{i}.1|nt:{i}.1:{i}.2|yo:{i}.2:{i}.3|u:{i}.3:{i}.4>"))
        .collect::<Vec<_>>()
        .join("\n");
    let cands = vec![
        cand(
            ProviderId::BetterLyrics,
            &split,
            order_of(ProviderId::BetterLyrics),
        ),
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
    ];
    // demoted to line level, so it only wins on order (BetterLyrics is first)
    assert_eq!(cands[0].sync, SyncKind::Word);
    assert_eq!(winner(&m, &cands), ProviderId::BetterLyrics);
    let cands = vec![
        cand(
            ProviderId::BetterLyrics,
            &split,
            order_of(ProviderId::BetterLyrics),
        ),
        cand(ProviderId::LrcLib, EN_WORD, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
}

#[test]
fn captions_are_last_resort() {
    let m = meta("Song", "Artist");
    let cands = vec![
        cand(
            ProviderId::YouTubeCaptions,
            EN_WORD,
            order_of(ProviderId::YouTubeCaptions),
        ),
        cand(ProviderId::LrcLib, EN_LINE, order_of(ProviderId::LrcLib)),
    ];
    assert_eq!(winner(&m, &cands), ProviderId::LrcLib);
    let only = vec![cand(ProviderId::YouTubeCaptions, EN_WORD, 12)];
    assert_eq!(winner(&m, &only), ProviderId::YouTubeCaptions);
}

#[test]
fn score_weights_are_spec_values() {
    assert_eq!(
        (
            select::SCRIPT_WEIGHT,
            select::SYNC_WEIGHT,
            select::DUET_WEIGHT
        ),
        (10_000, 1_000, 100)
    );
}

// ---------------------------------------------------------------------------
// Provider ids / config / provider-local conversions
// ---------------------------------------------------------------------------

#[test]
fn provider_ids_round_trip() {
    for p in ProviderId::all() {
        assert_eq!(ProviderId::from_id(p.id()), Some(*p));
    }
    assert_eq!(
        ProviderId::from_id("BetterLyrics"),
        Some(ProviderId::BetterLyrics)
    );
    assert_eq!(ProviderId::from_id("Kugou"), Some(ProviderId::Kugou));
    assert_eq!(
        ProviderId::from_id("Apple Music"),
        Some(ProviderId::BetterLyrics)
    );
    assert_eq!(ProviderId::from_id(" lrclib "), Some(ProviderId::LrcLib));
    assert_eq!(ProviderId::from_id("nope"), None);
    assert_eq!(ProviderId::from_id(""), None);
}

#[test]
fn default_config_skips_unported_and_dead() {
    let cfg = FetchConfig::default();
    assert_eq!(cfg.timeout, std::time::Duration::from_secs(35));
    assert!(!cfg.order.contains(&ProviderId::Paxsenix));
    // the YouTube sources are ported now; auto captions stay last
    for p in [ProviderId::YouTubeSubtitle, ProviderId::YouTubeMusic, ProviderId::YouTubeCaptions] {
        assert!(cfg.order.contains(&p));
    }
    assert_eq!(cfg.order.last(), Some(&ProviderId::YouTubeCaptions));
    assert_eq!(cfg.order.first(), Some(&ProviderId::BetterLyrics));
    assert!(cfg.order.contains(&ProviderId::LrcRed));
}

#[test]
fn youlyplus_conversion() {
    use crate::providers::youlyplus::{LyricsElement, LyricsItem, Syllable, convert_to_lrc};
    let syl = |t: &str, ms: f64, bg: bool| Syllable {
        text: Some(t.into()),
        time: Some(ms),
        is_background: Some(bg),
    };
    let items = vec![
        LyricsItem {
            text: None,
            time: Some(1000.0),
            syllabus: Some(vec![
                syl("Hel", 1000.0, false),
                syl("lo ", 1200.0, false),
                syl("world", 1500.0, false),
            ]),
            element: Some(LyricsElement {
                singer: Some("v2".into()),
            }),
        },
        LyricsItem {
            text: None,
            time: Some(3000.0),
            syllabus: Some(vec![syl("夜", 3000.0, false), syl("に", 3200.0, false)]),
            element: None,
        },
        LyricsItem {
            text: Some("line only".into()),
            time: Some(65_432.0),
            syllabus: None,
            element: None,
        },
    ];
    let lrc = convert_to_lrc(&items).unwrap();
    assert_eq!(
        lrc,
        "[00:01.000]{agent:v2}<00:01.000>Hel<00:01.200>lo <00:01.500>world\n[00:03.000]<00:03.000>夜<00:03.200>に\n[01:05.432]line only"
    );
    let (lines, _) = parse_lrc(&lrc);
    assert_eq!(texts(&lines), ["Hello world", "夜に", "line only"]);
    assert_eq!(
        lines[0].words.len(),
        2,
        "syllables glued in the text merge into words"
    );
}

#[test]
fn simpmusic_pick_rules() {
    use crate::providers::simpmusic::{LyricsData, pick};
    let d = |dur: f64, rich: Option<&str>, synced: Option<&str>| LyricsData {
        duration: Some(dur),
        rich_sync_lyrics: rich.map(str::to_string),
        synced_lyrics: synced.map(str::to_string),
        plain_lyrics: Some("plain".into()),
    };
    assert_eq!(
        pick(
            vec![d(250.0, Some("far"), None), d(203.0, None, Some("near"))],
            200
        )
        .as_deref(),
        Some("near")
    );
    assert_eq!(
        pick(vec![d(205.0, Some("rich"), Some("synced"))], 200).as_deref(),
        Some("rich")
    );
    assert_eq!(pick(vec![d(230.0, Some("rich"), None)], 200), None);
    assert_eq!(
        pick(vec![d(230.0, Some(" "), None)], -1).as_deref(),
        Some("plain")
    );
}

#[test]
fn paxsenix_scoring() {
    use crate::providers::paxsenix::{SearchResult, score_results};
    let r = |name: &str, artist: &str, ms: f64| SearchResult {
        id: serde_json::json!(name),
        track_name: Some(name.into()),
        artist_name: Some(artist.into()),
        duration: Some(ms),
        ..Default::default()
    };
    let scored = score_results(
        vec![
            r("Blinding Lights (Remix)", "The Weeknd", 200_000.0),
            r("Blinding Lights", "The Weeknd", 201_000.0),
            r("Other", "X", 400_000.0),
        ],
        "Blinding Lights",
        "The Weeknd",
        200,
    );
    assert_eq!(scored.len(), 2);
    assert_eq!(scored[0].0.track_name.as_deref(), Some("Blinding Lights"));
    assert_eq!(scored[0].1, 230.0);
}
