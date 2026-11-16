//! Live end-to-end check of the download pipeline (what the app's download
//! manager runs): resolve a YouTube Music stream, fetch it in ranged chunks,
//! remux WebM-Opus to Ogg Opus (or keep MP4), write tags + cover, name the
//! file with the default template.
//!
//! `cargo run -p mochy-download --example download -- <videoId> [out_dir] [aac]`

use std::sync::atomic::AtomicBool;

use mochy_core::settings::AudioQuality;
use mochy_download::{Container, ExportTags, NameFields, fetch, render_template, sniff, unique_path};
use mochy_innertube::{CodecPreference, YtmClient};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let vid = args.first().cloned().unwrap_or_else(|| "lYBUbBu4W08".into());
    let out = std::path::PathBuf::from(args.get(1).cloned().unwrap_or_else(|| std::env::temp_dir().join("mochy-dl").display().to_string()));
    let pref = if args.get(2).map(|s| s == "aac").unwrap_or(false) { CodecPreference::PreferAac } else { CodecPreference::PreferOpus };

    let client = YtmClient::new().await.expect("ytm client");
    let meta = client.track_meta(&vid).await.ok();
    let title = meta.as_ref().map(|m| m.title.clone()).unwrap_or_else(|| vid.clone());
    let artists: Vec<String> = meta.as_ref().map(|m| m.artists.clone()).unwrap_or_default();
    let album = meta.as_ref().and_then(|m| m.album.clone());
    println!("song: {title} — {}", artists.join(", "));

    let st = client.resolve_stream_with(&vid, AudioQuality::High, pref).await.expect("resolve");
    println!("stream: {:?} {:?} kbps via {} ({} bytes)", st.format, st.bitrate_kbps, st.client, st.content_length.unwrap_or(0));

    let tmp = out.join(format!("{vid}.part"));
    let cancel = AtomicBool::new(false);
    let http = fetch::client();
    let t0 = std::time::Instant::now();
    let mut last = 0u64;
    let n = fetch::fetch_to_file(&http, &st.url, &st.headers, &tmp, st.content_length, &cancel, |d, t| {
        if d - last > 1_000_000 || Some(d) == t {
            last = d;
            println!("  {d} / {t:?}");
        }
    })
    .await
    .expect("fetch");
    println!("fetched {n} bytes in {:.1} s", t0.elapsed().as_secs_f64());

    let bytes = std::fs::read(&tmp).unwrap();
    let mut tags = ExportTags::for_song(&title, &artists, album.as_deref(), None);
    tags.lyrics = Some("[00:01.00]Mochy test line".into());
    // Cover: the video thumbnail, square-cropped.
    if let Ok(r) = http.get(format!("https://i.ytimg.com/vi/{vid}/hqdefault.jpg")).send().await
        && let Ok(b) = r.bytes().await
    {
        tags.cover = mochy_download::cover::prepare_cover(&b);
    }
    let fields = NameFields { title: title.clone(), artist: artists.join(", "), id: vid.clone(), ..Default::default() };
    let base = render_template(mochy_download::naming::DEFAULT_TEMPLATE, &fields);
    let container = sniff::sniff(&bytes[..64.min(bytes.len())]);
    println!("container: {container:?}");
    let dest = if container == Container::WebM {
        let ogg = mochy_download::opus::webm_to_ogg_opus(&bytes, &tags).expect("remux");
        let pages = mochy_download::ogg::read_pages(&ogg).expect("ogg pages");
        println!("remuxed: {} pages, all CRC ok: {}", pages.len(), pages.iter().all(|p| p.crc_ok));
        let dest = unique_path(&out, &base.display().to_string(), "opus", |_| true);
        std::fs::write(&dest, ogg).unwrap();
        dest
    } else {
        let dest = unique_path(&out, &base.display().to_string(), container.extension(), |_| true);
        std::fs::write(&dest, &bytes).unwrap();
        mochy_download::tags::write_with_lofty(&dest, &tags).expect("tag");
        dest
    };
    let _ = std::fs::remove_file(&tmp);
    println!("saved: {}", dest.display());
    println!("tags read back: {:?}", mochy_download::tags::read_with_lofty(&dest));
}
