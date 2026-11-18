//! Live check of mixes and (with `YT_COOKIE` set) the signed-in account.
//!
//! `cargo run -p mochy-innertube --example yt_account_smoke`
//! `YT_COOKIE="SAPISID=…; …" cargo run -p mochy-innertube --example yt_account_smoke`

use mochy_innertube::{YtAuth, YtmClient};

#[tokio::main]
async fn main() {
    let c = YtmClient::new().await.expect("client");
    c.set_locale("vi", "VN");
    match c.playlist("RDbTuqSWUYE6E").await {
        Ok((p, songs)) => println!("mix '{}' → {} songs, first: {:?}", p.name, songs.len(), songs.first().map(|s| &s.title)),
        Err(e) => println!("mix failed: {e}"),
    }
    let Ok(cookie) = std::env::var("YT_COOKIE") else {
        println!("YT_COOKIE not set: skipping account checks");
        return;
    };
    c.set_auth(Some(YtAuth::new(cookie, std::env::var("YT_DATASYNC").ok())));
    match c.account_info().await {
        Ok(a) => println!("account: {} {:?} {:?}", a.name, a.email, a.channel_handle),
        Err(e) => println!("account failed: {e}"),
    }
    match c.library_snapshot().await {
        Ok(s) => println!(
            "library: {} liked, {} playlists, {} albums, {} artists, {} history, errors {:?}",
            s.liked_songs.len(), s.playlists.len(), s.albums.len(), s.artists.len(), s.history.len(), s.errors
        ),
        Err(e) => println!("library failed: {e}"),
    }
    match c.youtube_home(12, 4).await {
        Ok(h) => {
            println!("youtube home: {} mixes, {} music videos", h.mixes.len(), h.music_videos.len());
            for m in h.mixes.iter().take(8) {
                println!("  {} — {}", m.playlist_id, m.title);
            }
            if let Some(m) = h.mixes.first() {
                match c.playlist(&m.playlist_id).await {
                    Ok((_, songs)) => println!("  first mix plays {} songs", songs.len()),
                    Err(e) => println!("  first mix failed: {e}"),
                }
            }
        }
        Err(e) => println!("youtube home failed: {e}"),
    }
    match c.home_page(None).await {
        Ok(h) => println!("YTM home: {}", h.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>().join(" | ")),
        Err(e) => println!("YTM home failed: {e}"),
    }
    println!("session expired: {}, cookie rotations: {}", c.session_expired(), c.cookie_rotations());
}
