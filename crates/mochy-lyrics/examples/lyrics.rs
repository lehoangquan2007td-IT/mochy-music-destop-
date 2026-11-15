//! Live provider check.
//!
//! ```text
//! cargo run -p mochy-lyrics --example lyrics -- "<title>" "<artist>" <duration_secs> [provider_id|all] [video_id]
//! ```
//!
//! - with a provider id (`LRCLIB`, `BetterLyrics`, …): prints that provider's result;
//! - without one (or `all`): asks every ported provider (including ones disabled by default)
//!   one by one, prints a summary table, then runs the real [`mochy_lyrics::fetch`] with the
//!   default config and prints the winner.

use std::time::{Duration, Instant};

use mochy_core::models::TrackMeta;
use mochy_lyrics::{
    FetchConfig, Lyrics, ProviderId, SyncKind, fetch, fetch_raw_from_provider, parse_lrc,
};

fn summary(l: &Lyrics) -> String {
    let words = l.lines.iter().filter(|x| !x.words.is_empty()).count();
    let agents: std::collections::BTreeSet<&str> =
        l.lines.iter().filter_map(|x| x.agent.as_deref()).collect();
    let bg = l.lines.iter().filter(|x| x.bg).count();
    format!(
        "{:?}, {} lines ({} word-timed, {} bg, agents {:?})",
        l.sync,
        l.lines.len(),
        words,
        bg,
        agents
    )
}

fn print_lines(l: &Lyrics, n: usize) {
    for line in l.lines.iter().take(n) {
        let tag = match (&line.agent, line.bg) {
            (_, true) => " {bg}".to_string(),
            (Some(a), _) => format!(" {{{a}}}"),
            _ => String::new(),
        };
        println!(
            "    [{:7.2}]{tag} {}  ({} words)",
            line.time,
            line.text,
            line.words.len()
        );
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!(
            "usage: lyrics \"<title>\" \"<artist>\" <duration_secs> [provider_id|all] [video_id]"
        );
        std::process::exit(2);
    }
    let meta = TrackMeta {
        title: args[0].clone(),
        artists: args[1].split(", ").map(str::to_string).collect(),
        album: None,
        duration_secs: args[2].parse().ok(),
        video_id: args.get(4).cloned(),
    };
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    let which = args.get(3).map(String::as_str).unwrap_or("all");
    let providers: Vec<ProviderId> = if which.eq_ignore_ascii_case("all") {
        ProviderId::all()
            .iter()
            .copied()
            .filter(|p| {
                !matches!(
                    p,
                    ProviderId::YouTubeSubtitle
                        | ProviderId::YouTubeMusic
                        | ProviderId::YouTubeCaptions
                )
            })
            .collect()
    } else {
        match ProviderId::from_id(which) {
            Some(p) => vec![p],
            None => {
                eprintln!("unknown provider {which}");
                std::process::exit(2);
            }
        }
    };

    println!(
        "== {} — {} ({}s) video={:?}",
        meta.title,
        meta.artists.join(", "),
        args[2],
        meta.video_id
    );
    for p in &providers {
        let t0 = Instant::now();
        let res = tokio::time::timeout(
            Duration::from_secs(35),
            fetch_raw_from_provider(&meta, *p, &http),
        )
        .await;
        let dt = t0.elapsed().as_secs_f32();
        let enabled = if p.implemented() {
            ""
        } else {
            " [disabled by default]"
        };
        match res {
            Err(_) => println!("{:<14} TIMEOUT ({dt:.1}s){enabled}", p.id()),
            Ok(Err(e)) => println!("{:<14} ERROR {e} ({dt:.1}s){enabled}", p.id()),
            Ok(Ok(None)) => println!("{:<14} none ({dt:.1}s){enabled}", p.id()),
            Ok(Ok(Some(raw))) => match Lyrics::from_lrc(p.display_name(), &raw) {
                Some(l) => {
                    println!("{:<14} OK {} ({dt:.1}s){enabled}", p.id(), summary(&l));
                    if providers.len() == 1 {
                        print_lines(&l, 12);
                        println!(
                            "--- raw (first 600 chars) ---\n{}",
                            raw.chars().take(600).collect::<String>()
                        );
                    } else {
                        print_lines(&l, 2);
                    }
                }
                None => {
                    let (_, sync) = parse_lrc(&raw);
                    println!("{:<14} unparseable ({sync:?}) ({dt:.1}s){enabled}", p.id());
                }
            },
        }
    }

    if providers.len() > 1 {
        let t0 = Instant::now();
        match fetch(&meta, &FetchConfig::default(), &http).await {
            Some(l) => {
                println!(
                    "== fetch() winner: {} — {} ({:.1}s)",
                    l.provider,
                    summary(&l),
                    t0.elapsed().as_secs_f32()
                );
                print_lines(&l, 4);
                assert!(l.sync != SyncKind::None);
            }
            None => println!("== fetch(): no lyrics ({:.1}s)", t0.elapsed().as_secs_f32()),
        }
    }
}
