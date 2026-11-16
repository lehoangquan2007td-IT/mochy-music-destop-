//! Offline tests for the explore/browse parsers over trimmed live responses
//! (captured 2026-11-17 with hl=en gl=US from a VN IP, so charts are VN).

use mochy_core::models::ShelfItem;
use mochy_innertube::explore;
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).expect("fixture json")
}

#[test]
fn home_page_has_chips_sections_and_continuation() {
    let page = explore::home_page_response(&fixture("home_chips.json"));
    assert!(page.chips.len() >= 5, "chips: {:?}", page.chips);
    let workout = page.chips.iter().find(|c| c.title == "Workout").expect("Workout chip");
    assert!(workout.params.as_deref().is_some_and(|p| p.starts_with("ggM")));
    assert!(page.continuation.is_some());

    let quick = page.sections.iter().find(|s| s.title == "Quick picks").expect("quick picks");
    assert!(quick.rows, "quick picks are list rows");
    assert!(quick.items.iter().all(|i| matches!(i, ShelfItem::Song(_))));
    assert_eq!(quick.items.len(), quick.subtitles.len());
    let first = &quick.songs()[0];
    assert_eq!(first.id, "DlFXDl_ROAM");
    assert_eq!(first.title, "Die With A Smile");
    assert!(quick.subtitles[0].contains("Lady Gaga"), "{}", quick.subtitles[0]);

    let cards = page.sections.iter().find(|s| !s.rows).expect("a card shelf");
    assert!(!cards.items.is_empty());
    assert!(!cards.subtitles[0].is_empty());
}

#[test]
fn explore_page_splits_shortcuts_moods_and_shelves() {
    let page = explore::explore_response(&fixture("explore.json"));
    let ids: Vec<&str> = page.shortcuts.iter().map(|m| m.endpoint.browse_id.as_str()).collect();
    assert_eq!(ids, ["FEmusic_new_releases", "FEmusic_charts", "FEmusic_moods_and_genres"]);
    assert_eq!(page.shortcuts[1].icon.as_deref(), Some("TRENDING_UP"));

    assert!(page.moods.len() >= 30, "moods: {}", page.moods.len());
    let chill = &page.moods[0];
    assert_eq!(chill.title, "Chill");
    assert_eq!(chill.color, 4288988671);
    assert_eq!(chill.endpoint.browse_id, "FEmusic_moods_and_genres_category");
    assert_eq!(chill.endpoint.params.as_deref(), Some("ggMPOg1uX1JOQWZFeDByc2Jm"));

    let trending = page.sections.iter().find(|s| s.title == "Trending").expect("trending");
    assert!(trending.rows);
    assert!(trending.more.as_ref().is_some_and(|m| m.is_playlist()));
    assert!(trending.items.len() >= 10);
    let videos = page.sections.iter().find(|s| s.title == "New music videos").expect("videos");
    assert_eq!(videos.more.as_ref().map(|m| m.browse_id.as_str()), Some("FEmusic_new_releases_videos"));
    assert!(videos.items.iter().all(|i| matches!(i, ShelfItem::Song(s) if s.is_video)));
}

#[test]
fn moods_and_genres_groups() {
    let groups = explore::moods_and_genres_response(&fixture("moods_and_genres.json"));
    let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, ["Moods & moments", "Genres"]);
    assert!(groups[0].items.len() >= 10);
    assert!(groups[1].items.len() >= 20);
    assert!(groups.iter().flat_map(|g| &g.items).all(|m| m.color != 0 && m.endpoint.params.is_some()));
}

#[test]
fn mood_category_page_has_shelves_with_see_all() {
    let page = explore::browse_page_response(&fixture("mood_category.json"));
    assert_eq!(page.title, "Chill");
    assert!(page.sections.len() >= 3);
    let s = &page.sections[0];
    assert_eq!(s.title, "Coffee shop blends");
    let more = s.more.as_ref().expect("see all");
    assert_eq!(more.browse_id, "FEmusic_moods_and_genres_category");
    assert_eq!(more.params.as_deref(), Some("ggMPOg1uX2dsdVFONkZBREhy"));
    assert!(s.items.iter().all(|i| matches!(i, ShelfItem::Playlist(_))));
    assert!(s.subtitles[0].contains("Playlist"));
}

#[test]
fn see_all_page_has_grid_and_carousels() {
    let page = explore::browse_page_response(&fixture("browse_grid.json"));
    assert_eq!(page.title, "Coffee shop blends");
    let grid = page.sections.iter().find(|s| s.grid).expect("grid");
    assert_eq!(grid.title, "Spotlight");
    assert!(!grid.items.is_empty());
    let community = page.sections.iter().find(|s| s.title == "Community playlists").expect("community");
    match &community.items[0] {
        ShelfItem::Playlist(p) => {
            assert!(!p.id.starts_with("VL"));
            assert!(p.browse_id.as_deref().is_some_and(|b| b.starts_with("VL")));
        }
        other => panic!("expected playlist, got {other:?}"),
    }
}

#[test]
fn charts_page_lists_playlists_and_artists() {
    let page = explore::browse_page_response(&fixture("charts.json"));
    assert_eq!(page.title, "Charts");
    let video = page.sections.iter().find(|s| s.title == "Video charts").expect("video charts");
    assert!(video.items.iter().all(|i| matches!(i, ShelfItem::Playlist(_))));
    let artists = page.sections.iter().find(|s| s.title == "Top artists").expect("top artists");
    assert!(artists.rows);
    assert!(artists.items.iter().all(|i| matches!(i, ShelfItem::Artist(_))));
    assert!(artists.subtitles[0].contains("subscribers"), "{}", artists.subtitles[0]);
}

#[test]
fn new_releases_page() {
    let page = explore::browse_page_response(&fixture("new_releases.json"));
    assert_eq!(page.title, "New releases");
    let s = &page.sections[0];
    assert!(s.more.is_some());
    assert!(!s.items.is_empty());
}

#[test]
fn continuations_of_every_shape() {
    // Section list continuation (home page 2) reuses the shelves parser.
    let home2 = explore::home_page_response(&fixture("home_continuation.json"));
    assert!(home2.chips.is_empty());
    assert!(!home2.sections.is_empty());

    // Grid continuation.
    let grid = json!({"continuationContents": {"gridContinuation": {
        "items": [{"musicTwoRowItemRenderer": {
            "title": {"runs": [{"text": "Some Album"}]},
            "subtitle": {"runs": [{"text": "Album"}, {"text": " • "}, {"text": "2024"}]},
            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}
        }}],
        "continuations": [{"nextContinuationData": {"continuation": "NEXT"}}]
    }}});
    let p = explore::continuation_response(&grid);
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.subtitles, ["Album • 2024"]);
    assert_eq!(p.continuation.as_deref(), Some("NEXT"));

    // appendContinuationItemsAction with a trailing continuation item.
    let append = json!({"onResponseReceivedActions": [{"appendContinuationItemsAction": {"continuationItems": [
        {"musicResponsiveListItemRenderer": {
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Song A", "navigationEndpoint": {"watchEndpoint": {"videoId": "aaaaaaaaaaa"}}}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist A", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCa", "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}}]}}}
            ],
            "playlistItemData": {"videoId": "aaaaaaaaaaa"}
        }},
        {"continuationItemRenderer": {"continuationEndpoint": {"continuationCommand": {"token": "TOKEN2"}}}}
    ]}}]});
    let p = explore::continuation_response(&append);
    assert_eq!(p.items.len(), 1);
    assert_eq!(p.continuation.as_deref(), Some("TOKEN2"));

    // Empty continuation stops paging.
    let empty = json!({"continuationContents": {"gridContinuation": {"items": [], "continuations": [{"nextContinuationData": {"continuation": "X"}}]}}});
    assert!(explore::continuation_response(&empty).continuation.is_none());
}
