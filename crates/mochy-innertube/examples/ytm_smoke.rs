//! Live smoke test against YouTube Music.
//!
//! ```text
//! cargo run -p mochy-innertube --example ytm_smoke            # everything
//! cargo run -p mochy-innertube --example ytm_smoke -- streams # streams only
//! ```
//!
//! Exercises every public endpoint, then resolves streams for several songs
//! and proves they play: ranged GETs at the start, middle and last bytes with
//! the returned headers must answer 200/206, and the first bytes must be an
//! MP4 `ftyp` box (AAC) or an EBML header (webm/opus).

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use mochy_core::models::{ShelfItem, Song};
use mochy_core::settings::AudioQuality;
use mochy_innertube::{AudioFormat, CodecPreference, StreamResult, YtmClient, search_filters};

fn song_line(s: &Song) -> String {
    let artists: Vec<&str> = s.artists.iter().map(|a| a.name.as_str()).collect();
    format!(
        "{} | {} | album={} | dur={} | thumb={} | video={} | id={}",
        s.title,
        if artists.is_empty() { "<no artist>".to_string() } else { artists.join(", ") },
        s.album_name.as_deref().unwrap_or("-"),
        s.duration_secs.map(|d| format!("{}:{:02}", d as u64 / 60, d as u64 % 60)).unwrap_or_else(|| "?".into()),
        if s.thumbnail_url.is_some() { "yes" } else { "NO" },
        s.is_video,
        s.id,
    )
}

fn item_line(i: &ShelfItem) -> String {
    match i {
        ShelfItem::Song(s) => format!("song: {}", song_line(s)),
        ShelfItem::Album(a) => format!("album: {} ({}) year={:?} thumb={}", a.title, a.id, a.year, a.thumbnail_url.is_some()),
        ShelfItem::Artist(a) => format!("artist: {} ({}) thumb={}", a.name, a.id, a.thumbnail_url.is_some()),
        ShelfItem::Playlist(p) => format!("playlist: {} ({}) songs={} thumb={}", p.name, p.id, p.song_count, p.thumbnail_url.is_some()),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// GET `bytes=start-end` with the stream headers → (status, body).
async fn ranged(client: &YtmClient, s: &StreamResult, start: u64, end: u64) -> Result<(u16, Vec<u8>), String> {
    let mut req = client.http.get(&s.url).header("Range", format!("bytes={start}-{end}"));
    for (k, v) in &s.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok((status, body))
}

/// Ranged GETs at start / middle / end + container magic check.
async fn verify_playable(client: &YtmClient, s: &StreamResult) -> Result<String, String> {
    let len = s.content_length.ok_or("no content length")?;
    let mut report = Vec::new();
    for (label, start) in [("start", 0u64), ("mid", len / 2), ("end", len.saturating_sub(16))] {
        let end = (start + 15).min(len - 1);
        let (status, body) = ranged(client, s, start, end).await?;
        if status != 200 && status != 206 {
            return Err(format!("{label}: HTTP {status}"));
        }
        if label == "start" {
            match s.format {
                AudioFormat::AacM4a => {
                    if body.len() < 8 || &body[4..8] != b"ftyp" {
                        return Err(format!("start: not an MP4 ftyp box: {:02x?}", &body[..body.len().min(8)]));
                    }
                    report.push(format!("start {status} ftyp={}", String::from_utf8_lossy(&body[8..12.min(body.len())])));
                }
                AudioFormat::OpusWebm => {
                    if body.len() < 4 || body[..4] != [0x1A, 0x45, 0xDF, 0xA3] {
                        return Err("start: not an EBML/webm header".into());
                    }
                    report.push(format!("start {status} EBML"));
                }
                _ => report.push(format!("start {status}")),
            }
        } else {
            report.push(format!("{label} {status} ({}B)", body.len()));
        }
    }
    Ok(report.join(", "))
}

fn describe_stream(s: &StreamResult) -> String {
    let exp = s.expires_at_unix.map(|e| format!("{:.1}h", (e.saturating_sub(now())) as f64 / 3600.0)).unwrap_or("?".into());
    let alts: Vec<String> = s
        .alternatives
        .iter()
        .map(|a| format!("{}:{:?}", a.itag.unwrap_or(0), a.format))
        .collect();
    format!(
        "client={} itag={:?} {:?} codecs={} {}kbps {}Hz {}B loudness={:?}dB expires_in={} alts=[{}]",
        s.client,
        s.itag,
        s.format,
        s.codecs.as_deref().unwrap_or("?"),
        s.bitrate_kbps.unwrap_or(0),
        s.sample_rate_hz.unwrap_or(0),
        s.content_length.unwrap_or(0),
        s.loudness_db,
        exp,
        alts.join(" ")
    )
}

async fn endpoints(c: &YtmClient) -> Option<String> {
    let t = Instant::now();
    let mut seed: Option<String> = None;

    match c.search_all("Imagine Dragons Believer").await {
        Ok(r) => {
            println!(
                "[search_all] songs={} videos={} albums={} artists={} playlists={} ({} ms)",
                r.songs.len(), r.videos.len(), r.albums.len(), r.artists.len(), r.playlists.len(), t.elapsed().as_millis()
            );
            for s in r.songs.iter().take(2) {
                println!("    song  {}", song_line(s));
            }
            for s in r.videos.iter().take(2) {
                println!("    video {}", song_line(s));
            }
            if let Some(a) = r.albums.first() {
                println!("    album {} ({}) year={:?} playlist={:?}", a.title, a.id, a.year, a.playlist_id);
            }
            if let Some(a) = r.artists.first() {
                println!("    artist {} ({})", a.name, a.id);
            }
            if let Some(p) = r.playlists.first() {
                println!("    playlist {} ({}) songs={}", p.name, p.id, p.song_count);
            }
        }
        Err(e) => println!("[search_all] ERROR {e}"),
    }

    let songs = c.search_songs("Imagine Dragons Believer", search_filters::SONG).await;
    match &songs {
        Ok(v) => {
            println!("[search_songs] {} results", v.len());
            for s in v.iter().take(3) {
                println!("    {}", song_line(s));
            }
            seed = v.first().map(|s| s.id.clone());
        }
        Err(e) => println!("[search_songs] ERROR {e}"),
    }

    match c.suggestions("son tung").await {
        Ok(v) => println!("[suggestions] {} -> {:?}", v.len(), v),
        Err(e) => println!("[suggestions] ERROR {e}"),
    }

    match c.home().await {
        Ok(shelves) => {
            println!("[home] {} shelves", shelves.len());
            for sh in shelves.iter().take(4) {
                println!("    '{}' ({} items) first: {}", sh.title, sh.items.len(), sh.items.first().map(item_line).unwrap_or_default());
            }
        }
        Err(e) => println!("[home] ERROR {e}"),
    }

    let filtered = |f| c.search_filtered("Imagine Dragons", f);
    if let Ok(r) = filtered(search_filters::ALBUM).await
        && let Some(a) = r.albums.first()
    {
        match c.album(&a.id).await {
            Ok((album, tracks)) => {
                println!(
                    "[album] {} ({}) year={:?} songs={:?} dur={:?}s playlist={:?} thumb={} desc={}",
                    album.title, album.id, album.year, album.song_count, album.duration_secs, album.playlist_id,
                    album.thumbnail_url.is_some(), album.description.is_some()
                );
                for s in tracks.iter().take(2) {
                    println!("    {}", song_line(s));
                }
            }
            Err(e) => println!("[album] ERROR {e}"),
        }
    }

    if let Ok(r) = filtered(search_filters::ARTIST).await
        && let Some(a) = r.artists.first()
    {
        match c.artist(&a.id).await {
            Ok((artist, shelves)) => {
                println!("[artist] {} ({}) channel={:?} thumb={} shelves={}", artist.name, artist.id, artist.channel_id, artist.thumbnail_url.is_some(), shelves.len());
                for sh in &shelves {
                    println!("    '{}' ({} items) first: {}", sh.title, sh.items.len(), sh.items.first().map(item_line).unwrap_or_default());
                }
            }
            Err(e) => println!("[artist] ERROR {e}"),
        }
    }

    if let Ok(r) = filtered(search_filters::FEATURED_PLAYLIST).await
        && let Some(p) = r.playlists.first()
    {
        match c.playlist(&p.id).await {
            Ok((pl, tracks)) => {
                println!("[playlist] {} ({}) count={} tracks={} thumb={}", pl.name, pl.id, pl.song_count, tracks.len(), pl.thumbnail_url.is_some());
                for s in tracks.iter().take(2) {
                    println!("    {}", song_line(s));
                }
            }
            Err(e) => println!("[playlist] ERROR {e}"),
        }
    }
    // Long community playlist (exercises continuations).
    match c.playlist("PLtJCksbabIPyfutcI3Kx9mg6h87p2DXzE").await {
        Ok((pl, tracks)) => println!("[playlist/long] {} count={} tracks={}", pl.name, pl.song_count, tracks.len()),
        Err(e) => println!("[playlist/long] ERROR {e}"),
    }

    if let Some(id) = &seed {
        match c.get_queue(id).await {
            Ok(q) => {
                println!("[get_queue] {} items", q.len());
                for i in q.iter().take(3) {
                    let artists: Vec<&str> = i.artists.iter().map(|a| a.name.as_str()).collect();
                    println!("    {} | {} | {:?}s | thumb={}", i.title, artists.join(", "), i.duration_secs, i.thumbnail_url.is_some());
                }
            }
            Err(e) => println!("[get_queue] ERROR {e}"),
        }
        match c.related(id).await {
            Ok(v) => {
                println!("[related] {} songs", v.len());
                for s in v.iter().take(3) {
                    println!("    {}", song_line(s));
                }
            }
            Err(e) => println!("[related] ERROR {e}"),
        }
        match c.track_meta(id).await {
            Ok(m) => println!("[track_meta] {:?}", m),
            Err(e) => println!("[track_meta] ERROR {e}"),
        }
        match c.lyrics(id).await {
            Ok(l) => println!("[lyrics] {}", l.map(|t| format!("{} chars", t.len())).unwrap_or("none".into())),
            Err(e) => println!("[lyrics] ERROR {e}"),
        }
    }
    seed
}

async fn streams(c: &YtmClient) {
    // (label, video id or search query)
    let mut targets: Vec<(String, String)> = vec![
        ("Believer (song)".into(), "Kx7B-XvmFtE".into()),
        ("Believer (official music video)".into(), "7wtfhZwyrcc".into()),
    ];
    for (label, q) in [
        ("Vietnamese: Son Tung M-TP Lac Troi", "Sơn Tùng M-TP Lạc Trôi"),
        ("Vietnamese: Hoang Thuy Linh", "Hoàng Thùy Linh See Tình"),
        ("The Weeknd Blinding Lights", "The Weeknd Blinding Lights"),
        ("Ed Sheeran Shape of You", "Ed Sheeran Shape of You"),
        ("Queen Bohemian Rhapsody", "Queen Bohemian Rhapsody"),
    ] {
        match c.search_songs(q, search_filters::SONG).await {
            Ok(v) if !v.is_empty() => targets.push((format!("{label} -> {}", v[0].title), v[0].id.clone())),
            Ok(_) => println!("[streams] no search result for {q}"),
            Err(e) => println!("[streams] search {q}: {e}"),
        }
    }
    if let Ok(v) = c.search_songs("Sơn Tùng M-TP Chúng Ta Của Hiện Tại", search_filters::VIDEO).await
        && let Some(s) = v.first()
    {
        targets.push((format!("Vietnamese music video -> {}", s.title), s.id.clone()));
    }

    let mut ok = 0;
    for (label, id) in &targets {
        let t = Instant::now();
        match c.resolve_stream(id, AudioQuality::Auto).await {
            Ok(s) => {
                let ms = t.elapsed().as_millis();
                match verify_playable(c, &s).await {
                    Ok(rep) => {
                        ok += 1;
                        println!("[stream OK] {label} ({id}) {ms} ms\n    {}\n    {rep}", describe_stream(&s));
                    }
                    Err(e) => println!("[stream BAD] {label} ({id}): {e}\n    {}", describe_stream(&s)),
                }
            }
            Err(e) => println!("[stream ERR] {label} ({id}): {e}"),
        }
    }
    println!("[streams] {ok}/{} playable", targets.len());

    // Quality tiers + Opus preference on one song.
    for q in [AudioQuality::Low, AudioQuality::Medium, AudioQuality::High] {
        match c.resolve_stream("Kx7B-XvmFtE", q).await {
            Ok(s) => println!("[quality {q:?}] itag={:?} {}kbps {:?}", s.itag, s.bitrate_kbps.unwrap_or(0), s.codecs),
            Err(e) => println!("[quality {q:?}] ERROR {e}"),
        }
    }
    match c.resolve_stream_with("Kx7B-XvmFtE", AudioQuality::High, CodecPreference::PreferOpus).await {
        Ok(s) => match verify_playable(c, &s).await {
            Ok(rep) => println!("[opus] itag={:?} {:?} {}kbps -> {rep}", s.itag, s.format, s.bitrate_kbps.unwrap_or(0)),
            Err(e) => println!("[opus] BAD {e}"),
        },
        Err(e) => println!("[opus] ERROR {e}"),
    }
    // Opus alternative of an AAC result must also play with the same headers.
    if let Ok(s) = c.resolve_stream("Kx7B-XvmFtE", AudioQuality::Auto).await
        && let Some(alt) = s.alternative(AudioFormat::OpusWebm)
    {
        let alt_res = StreamResult {
            url: alt.url.clone(),
            format: alt.format,
            itag: alt.itag,
            content_length: alt.content_length,
            ..s.clone()
        };
        match verify_playable(c, &alt_res).await {
            Ok(rep) => println!("[opus alternative] itag={:?} -> {rep}", alt.itag),
            Err(e) => println!("[opus alternative] BAD {e}"),
        }
    }
}

/// Each walk client on its own (what happens when VISIONOS is unavailable).
async fn per_client(c: &YtmClient) {
    for client in mochy_innertube::clients::STREAM_WALK {
        let t = Instant::now();
        let r = c
            .resolve_stream_walk("Kx7B-XvmFtE", AudioQuality::Auto, CodecPreference::PreferAac, std::slice::from_ref(client))
            .await;
        match r {
            Ok(s) => match verify_playable(c, &s).await {
                Ok(rep) => println!("[client {} {}] OK itag={:?} ({} ms) {rep}", client.name, client.version, s.itag, t.elapsed().as_millis()),
                Err(e) => println!("[client {} {}] resolved but BAD: {e}", client.name, client.version),
            },
            Err(e) => println!("[client {} {}] {e}", client.name, client.version),
        }
    }
}

#[tokio::main]
async fn main() {
    let only_streams = std::env::args().any(|a| a == "streams");
    let t = Instant::now();
    let c = YtmClient::new().await.expect("client");
    let vd = c.visitor_data();
    println!("[new] visitor_data={}… ({} ms)", &vd[..vd.len().min(16)], t.elapsed().as_millis());
    if std::env::args().any(|a| a == "clients") {
        per_client(&c).await;
        return;
    }
    if !only_streams {
        endpoints(&c).await;
        // Locale: a separate client so the shared one stays en/US.
        let vn = YtmClient::new_offline().expect("client");
        vn.set_visitor_data(&c.visitor_data());
        vn.set_locale("vi", "VN");
        match vn.home().await {
            Ok(sh) => println!("[locale vi/VN] home shelves: {:?}", sh.iter().take(4).map(|s| s.title.as_str()).collect::<Vec<_>>()),
            Err(e) => println!("[locale vi/VN] ERROR {e}"),
        }
    }
    streams(&c).await;
    println!("[done] total {} ms", t.elapsed().as_millis());
}
