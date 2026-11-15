//! Offline parser tests over trimmed live responses (captured 2026-11-16,
//! hl=en gl=US; see `tests/fixtures/`). They pin the response shapes the
//! parsers depend on so a YouTube layout change shows up as a test failure.

use mochy_core::models::ShelfItem;
use mochy_core::settings::AudioQuality;
use mochy_innertube::{AudioFormat, CodecPreference, parse, parse_sw_js_data};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture json")
}

fn names(artists: &[mochy_core::models::ArtistRef]) -> Vec<&str> {
    artists.iter().map(|a| a.name.as_str()).collect()
}

// --- search ---------------------------------------------------------------

#[test]
fn search_all_summary_layout_is_classified() {
    let r = parse::search_response(&fixture("search_all.json"));
    assert!(r.songs.len() >= 3, "songs: {}", r.songs.len());
    assert!(r.videos.len() >= 2, "videos: {}", r.videos.len());
    assert!(!r.albums.is_empty() && !r.artists.is_empty() && !r.playlists.is_empty());

    let believer = &r.songs[0];
    assert_eq!(believer.id, "Kx7B-XvmFtE");
    assert_eq!(believer.title, "Believer");
    assert_eq!(names(&believer.artists), ["Imagine Dragons"]);
    assert_eq!(believer.artists[0].id.as_deref(), Some("UC0aXrjVxG5pZr99v77wZdPQ"));
    assert!(!believer.is_video);
    assert!(believer.thumbnail_url.is_some());

    // "Song • Imagine Dragons & Kaskade" — the type label is not an artist.
    let remix = r.songs.iter().find(|s| s.id == "Al14Nls61M8").expect("remix");
    assert_eq!(names(&remix.artists), ["Imagine Dragons", "Kaskade"]);

    // Top-result card is the official video (OMV) with its duration.
    let omv = r.videos.iter().find(|s| s.id == "7wtfhZwyrcc").expect("card video");
    assert!(omv.is_video);
    assert_eq!(omv.duration_secs, Some(217.0));
    assert_eq!(names(&omv.artists), ["Imagine Dragons"]);

    // Podcast episodes and user profiles are not music results.
    assert!(!r.videos.iter().chain(&r.songs).any(|s| s.id == "WKeXDRDlqbo"));
    assert!(r.artists.iter().all(|a| a.id == "UC0aXrjVxG5pZr99v77wZdPQ"), "profiles leaked: {:?}", r.artists);

    let evolve = r.albums.iter().find(|a| a.id == "MPREb_q16Gzaa1WK8").expect("album");
    assert_eq!(evolve.title, "Evolve");
    assert_eq!(evolve.year, Some(2017));
    assert_eq!(evolve.playlist_id.as_deref(), Some("OLAK5uy_n7O88V1oc-6l7MW_F53flLA9v2W0yHx1Y"));

    let pl = &r.playlists[0];
    assert!(!pl.id.starts_with("VL"));
    assert_eq!(pl.browse_id.as_deref(), Some(format!("VL{}", pl.id).as_str()));
}

#[test]
fn search_songs_filter_has_album_duration_artists() {
    let r = parse::search_response(&fixture("search_songs.json"));
    assert!(r.songs.len() >= 5);
    for s in &r.songs {
        assert!(!s.artists.is_empty(), "{} has no artists", s.title);
        assert!(s.duration_secs.is_some(), "{} has no duration", s.title);
        assert!(s.album_id.as_deref().is_some_and(|a| a.starts_with("MPRE")), "{} album", s.title);
        assert!(s.thumbnail_url.is_some());
        assert!(!s.is_video);
    }
    let b = &r.songs[0];
    assert_eq!((b.title.as_str(), b.album_name.as_deref(), b.duration_secs), ("Believer", Some("Evolve"), Some(205.0)));
    assert_eq!(names(&b.artists), ["Imagine Dragons"]);
    assert!(parse::search_continuation(&fixture("search_songs.json")).is_some());
}

#[test]
fn search_videos_filter() {
    let r = parse::search_response(&fixture("search_videos.json"));
    assert!(!r.videos.is_empty());
    for v in &r.videos {
        assert!(v.is_video && v.duration_secs.is_some() && !v.artists.is_empty(), "{v:?}");
    }
}

#[test]
fn suggestions_and_items() {
    let j = fixture("suggestions.json");
    let s = parse::suggestions_response(&j);
    assert!(s.len() >= 3);
    assert!(s.iter().any(|q| q == "son tung"), "{s:?}");
    let items = parse::suggestion_items(&j);
    assert!(items.iter().any(|i| matches!(i, ShelfItem::Artist(a) if a.name == "Sơn Tùng M-TP")), "{items:?}");
}

// --- browse pages ---------------------------------------------------------

#[test]
fn home_first_page_and_continuation() {
    let (shelves, cont) = parse::home_response(&fixture("home.json"));
    assert!(!shelves.is_empty());
    assert!(cont.is_some(), "home continuation token");
    let quick = &shelves[0];
    assert!(!quick.title.is_empty());
    for item in &quick.items {
        if let ShelfItem::Song(s) = item {
            assert!(!s.artists.is_empty() && s.thumbnail_url.is_some(), "{s:?}");
        }
    }
    let (more, _) = parse::home_response(&fixture("home_continuation.json"));
    assert!(!more.is_empty(), "continuation page shelves");
    assert!(more.iter().all(|s| !s.items.is_empty()));
}

#[test]
fn album_page_tracks_inherit_album_data() {
    let (album, songs) = parse::album_response(&fixture("album.json"));
    assert_eq!(album.id, "MPREb_q16Gzaa1WK8");
    assert_eq!(album.title, "Evolve");
    assert_eq!(album.year, Some(2017));
    assert_eq!(album.song_count, Some(12));
    assert_eq!(album.duration_secs, Some(43.0 * 60.0));
    assert_eq!(album.playlist_id.as_deref(), Some("OLAK5uy_n7O88V1oc-6l7MW_F53flLA9v2W0yHx1Y"));
    assert!(album.description.as_deref().is_some_and(|d| d.contains("Imagine Dragons")));
    assert!(album.thumbnail_url.as_deref().is_some_and(|u| u.contains("w544-h544")));
    assert_eq!(names(&parse::album_artists(&fixture("album.json"))), ["Imagine Dragons"]);

    assert!(!songs.is_empty());
    let first = &songs[0];
    assert_eq!(first.title, "Next To Me");
    assert_eq!(first.duration_secs, Some(231.0));
    assert_eq!(names(&first.artists), ["Imagine Dragons"]);
    assert_eq!(first.album_id.as_deref(), Some("MPREb_q16Gzaa1WK8"));
    assert_eq!(first.album_name.as_deref(), Some("Evolve"));
    assert_eq!(first.thumbnail_url, album.thumbnail_url);
    assert!(songs.iter().all(|s| s.duration_secs.is_some() && !s.artists.is_empty()));
}

#[test]
fn artist_page_shelves() {
    let (artist, shelves) = parse::artist_response(&fixture("artist.json"));
    assert_eq!(artist.id, "UC0aXrjVxG5pZr99v77wZdPQ");
    assert_eq!(artist.name, "Imagine Dragons");
    assert!(artist.thumbnail_url.is_some());
    let top = shelves.iter().find(|s| s.title == "Top songs").expect("top songs");
    for item in &top.items {
        let ShelfItem::Song(s) = item else { panic!("top songs must be songs") };
        assert!(!s.artists.is_empty() && s.album_name.is_some() && s.thumbnail_url.is_some(), "{s:?}");
    }
    let albums = shelves.iter().find(|s| s.title == "Albums").expect("albums");
    assert!(albums.items.iter().all(|i| matches!(i, ShelfItem::Album(a) if a.year.is_some())));
    assert!(shelves.iter().any(|s| s.items.iter().any(|i| matches!(i, ShelfItem::Artist(_)))), "similar artists");
}

#[test]
fn playlist_page() {
    let (pl, songs) = parse::playlist_response(&fixture("playlist.json"));
    assert_eq!(pl.id, "RDCLAK5uy_mzpBFnAPcGS-4FYm4BzAY-Q3VmvNCQwxY");
    assert_eq!(pl.browse_id.as_deref(), Some("VLRDCLAK5uy_mzpBFnAPcGS-4FYm4BzAY-Q3VmvNCQwxY"));
    assert_eq!(pl.name, "Presenting Imagine Dragons");
    assert_eq!(pl.song_count, 40);
    assert!(pl.thumbnail_url.is_some() && pl.description.is_some());
    assert!(!songs.is_empty());
    for s in &songs {
        assert!(s.duration_secs.is_some() && !s.artists.is_empty() && s.thumbnail_url.is_some(), "{s:?}");
    }
    let enemy = songs.iter().find(|s| s.title.starts_with("Enemy")).expect("multi-artist row");
    assert_eq!(names(&enemy.artists), ["League Of Legends", "Imagine Dragons", "Arcane"]);
    assert!(parse::playlist_continuation_token(&fixture("playlist.json")).is_none());
}

#[test]
fn playlist_continuations() {
    let first = fixture("playlist_long.json");
    let (_, songs) = parse::playlist_response(&first);
    assert!(!songs.is_empty());
    assert!(parse::playlist_continuation_token(&first).is_some());
    let (more, next) = parse::playlist_continuation(&fixture("playlist_continuation.json"));
    assert!(!more.is_empty());
    assert!(next.is_some());
    assert!(more.iter().all(|s| s.duration_secs.is_some()));
}

// --- next / related / lyrics ------------------------------------------------

#[test]
fn next_panel_and_tabs() {
    let j = fixture("next.json");
    let q = parse::queue_from_next(&j);
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].video_id, "Kx7B-XvmFtE");
    // "Imagine Dragons • Evolve • 2017": album and year are not artists.
    assert_eq!(names(&q[0].artists), ["Imagine Dragons"]);
    assert_eq!(q[0].duration_secs, Some(205.0));
    let song = parse::panel_video_to_song(parse::panel_videos(&j)[0]).unwrap();
    assert_eq!((song.album_name.as_deref(), song.year), (Some("Evolve"), Some(2017)));

    let (pid, params) = parse::next_automix_endpoint(&j).expect("automix");
    assert_eq!(pid, "RDAMVMKx7B-XvmFtE");
    assert!(params.is_some());
    assert_eq!(parse::next_related_browse_id(&j).as_deref(), Some("MPTRt_q16Gzaa1WK8-4"));
    assert_eq!(parse::next_lyrics_browse_id(&j).as_deref(), Some("MPLYt_q16Gzaa1WK8-4"));
}

#[test]
fn automix_queue() {
    let q = parse::queue_from_next(&fixture("next_automix.json"));
    assert!(q.len() >= 3);
    for i in &q {
        assert!(!i.artists.is_empty() && i.duration_secs.is_some() && i.thumbnail_url.is_some(), "{i:?}");
        assert!(i.playlist_id.as_deref().is_some_and(|p| p.starts_with("RDAMVM")));
    }
}

#[test]
fn related_songs() {
    let songs = parse::related_response(&fixture("related.json"));
    assert!(songs.len() >= 2);
    assert!(songs.iter().all(|s| !s.is_video && !s.artists.is_empty()));
    assert!(songs.iter().any(|s| s.album_name.is_some()));
}

#[test]
fn ytm_lyrics() {
    let l = parse::lyrics_response(&fixture("lyrics.json")).expect("lyrics");
    assert!(l.starts_with("First things first"));
}

// --- player -----------------------------------------------------------------

#[test]
fn player_format_ranking() {
    let j = fixture("player_visionos.json");
    assert_eq!(parse::playability(&j).0, "OK");
    let auto = parse::audio_candidates(&j, AudioQuality::Auto, CodecPreference::PreferAac);
    assert_eq!(auto[0].itag, Some(140));
    assert_eq!(auto[0].format, AudioFormat::AacM4a);
    assert_eq!(auto[0].codecs.as_deref(), Some("mp4a.40.2"));
    assert_eq!(auto[0].loudness_db, Some(7.46));
    assert_eq!(auto[0].content_length, Some(3_309_731));
    // AAC rungs come before every Opus one; Opus stays available.
    let first_opus = auto.iter().position(|c| c.format == AudioFormat::OpusWebm).expect("opus kept");
    assert!(auto[..first_opus].iter().all(|c| c.format == AudioFormat::AacM4a));
    assert!(auto.iter().all(|c| c.itag.is_some_and(|i| [139, 140, 249, 250, 251].contains(&i))), "no video formats");

    let itag = |q, p| parse::audio_candidates(&j, q, p)[0].itag;
    assert_eq!(itag(AudioQuality::Low, CodecPreference::PreferAac), Some(139));
    assert_eq!(itag(AudioQuality::Medium, CodecPreference::PreferAac), Some(140));
    assert_eq!(itag(AudioQuality::High, CodecPreference::PreferAac), Some(140));
    assert_eq!(itag(AudioQuality::Lossless, CodecPreference::PreferAac), Some(140));
    assert_eq!(itag(AudioQuality::High, CodecPreference::PreferOpus), Some(251));
    // 250 peaks at 74 kbps (> the 70 kbps Low target), so 249 wins.
    assert_eq!(itag(AudioQuality::Low, CodecPreference::PreferOpus), Some(249));
    let aac_only = parse::audio_candidates(&j, AudioQuality::Auto, CodecPreference::AacOnly);
    assert!(aac_only.iter().all(|c| c.format == AudioFormat::AacM4a) && aac_only.len() == 2);

    let best = parse::pick_audio_format(&j).expect("pick");
    assert_eq!(best.itag, Some(140));
    assert_eq!(best.expires_at_unix, Some(1_794_796_661));
    assert_eq!(best.alternatives.len(), 4);
}

#[test]
fn player_ciphered_formats_are_skipped() {
    let j = json!({
        "playabilityStatus": {"status": "OK"},
        "streamingData": {"adaptiveFormats": [
            {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000, "signatureCipher": "s=abc&url=https%3A%2F%2Fx"},
            {"itag": 251, "mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 140000, "url": "https://x.googlevideo.com/videoplayback?expire=5"}
        ]}
    });
    let c = parse::audio_candidates(&j, AudioQuality::Auto, CodecPreference::PreferAac);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].itag, Some(251));
    assert!(parse::audio_candidates(&j, AudioQuality::Auto, CodecPreference::AacOnly).is_empty());
}

#[test]
fn player_auto_dubbed_tracks_are_dropped() {
    let j = json!({"streamingData": {"adaptiveFormats": [
        {"itag": 140, "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 130000, "url": "u1", "audioTrack": {"isAutoDubbed": true}},
        {"itag": 139, "mimeType": "audio/mp4; codecs=\"mp4a.40.5\"", "bitrate": 50000, "url": "u2"}
    ]}});
    let c = parse::audio_candidates(&j, AudioQuality::High, CodecPreference::PreferAac);
    assert_eq!(c.iter().map(|c| c.itag).collect::<Vec<_>>(), [Some(139)]);
}

#[test]
fn playability_bot_check_and_premium() {
    let bot = json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Sign in to confirm you’re not a bot"}});
    assert!(parse::is_bot_check(&bot));
    assert!(!parse::is_premium_only(&bot));
    let prem = json!({"playabilityStatus": {"status": "UNPLAYABLE", "reason": "This video is only available to Music Premium members"}});
    assert!(parse::is_premium_only(&prem));
    assert!(!parse::is_bot_check(&prem));
}

// --- helpers ----------------------------------------------------------------

#[test]
fn duration_and_text_helpers() {
    assert_eq!(parse::parse_duration("3:25"), Some(205.0));
    assert_eq!(parse::parse_duration("1:02:30"), Some(3750.0));
    assert_eq!(parse::parse_duration("2017"), None);
    assert_eq!(parse::parse_duration("3 of 5"), None);
    assert_eq!(parse::parse_long_duration("43 minutes"), Some(2580.0));
    assert_eq!(parse::parse_long_duration("2 hours, 28 minutes"), Some(8880.0));
    assert_eq!(parse::parse_long_duration("40 songs"), None);
    assert_eq!(
        parse::hi_res_thumbnail("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj", 544),
        "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
    );
    assert_eq!(parse::hi_res_thumbnail("https://i.ytimg.com/vi/x/hq.jpg", 544), "https://i.ytimg.com/vi/x/hq.jpg");
    assert_eq!(parse::expire_from_url("https://h/videoplayback?a=1&expire=1794796661&b=2"), Some(1_794_796_661));
    assert_eq!(parse::playlist_id_from_browse("VLPL123"), "PL123");
}

#[test]
fn sw_js_data_visitor() {
    let text = r#")]}'
[["yt.sw.adr",null,[null,"x","CgtBQkNERUZHSElKSyiY%3D",3]]]"#;
    assert_eq!(parse_sw_js_data(text).as_deref(), Some("CgtBQkNERUZHSElKSyiY%3D"));
    assert_eq!(parse_sw_js_data(")]}'\n[]"), None);
}
