//! `--smoke`: headless integration run exercising the same services and
//! controller as the GUI. Exit code 0 when playback progressed.

use std::time::{Duration, Instant};

use mochy_audio::Event;

use crate::controller::{CtlCfg, PlayState, PlayerController};
use crate::db::DbWorker;
use crate::services::{AppMsg, Bus, Services};

pub fn run(rt: tokio::runtime::Handle, query: &str, secs: u64) -> i32 {
    tracing::info!(%query, secs, "smoke test starting");
    let (bus, rx) = Bus::new();
    let db = DbWorker::spawn(|r| match r {
        Ok(()) => tracing::info!("smoke: db ok"),
        Err(e) => tracing::error!("smoke: db open failed: {e}"),
    });
    let svc = Services::new(rt.clone(), bus.clone(), db.clone());

    // --- storage ----------------------------------------------------------
    let counts = rt.block_on(db.query(|db| {
        (
            db.liked_songs().map(|v| v.len()),
            db.recent_history(50).map(|v| v.len()),
            db.playlists().map(|v| v.len()),
            db.load_queue().map(|q| q.map(|(i, _)| i.len())),
        )
    }));
    match counts {
        Ok((liked, hist, pls, queue)) => {
            tracing::info!(?liked, ?hist, ?pls, ?queue, "smoke: storage queries")
        }
        Err(e) => tracing::error!("smoke: db worker gone: {e}"),
    }
    match mochy_storage::SettingsStore::open_default().load() {
        Ok(_) => tracing::info!("smoke: settings ok"),
        Err(e) => tracing::warn!("smoke: settings load failed: {e}"),
    }

    // --- audio engine -----------------------------------------------------
    tracing::info!(devices = ?mochy_audio::output_devices(), "smoke: output devices");
    let (etx, erx) = std::sync::mpsc::channel();
    let engine = match mochy_audio::spawn_engine(etx) {
        Ok(e) => Some(e),
        Err(e) => {
            tracing::error!("smoke: engine spawn failed: {e}");
            None
        }
    };
    {
        let bus = bus.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = erx.recv() {
                bus.send(AppMsg::Engine(ev));
            }
        });
    }

    // --- YouTube Music ----------------------------------------------------
    let s2 = svc.clone();
    let q = query.to_string();
    let found = rt.block_on(async move {
        let c = s2.ytm().await?;
        match c.home().await {
            Ok(h) => tracing::info!(shelves = h.len(), items = h.iter().map(|s| s.items.len()).sum::<usize>(), "smoke: home"),
            Err(e) => tracing::error!("smoke: home failed: {e}"),
        }
        match c.suggestions(&q).await {
            Ok(s) => tracing::info!(count = s.len(), first = ?s.first(), "smoke: suggestions"),
            Err(e) => tracing::warn!("smoke: suggestions failed: {e}"),
        }
        let songs = c
            .search_songs(&q, mochy_innertube::search_filters::SONG)
            .await
            .map_err(|e| e.to_string())?;
        tracing::info!(count = songs.len(), "smoke: search songs");
        Ok::<_, String>(songs)
    });
    let songs = match found {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            tracing::error!("smoke: search returned no songs");
            return 2;
        }
        Err(e) => {
            tracing::error!("smoke: YTM failed: {e}");
            return 2;
        }
    };
    let first = songs[0].clone();
    tracing::info!(title = %first.title, artist = %first.artist_name(), id = %first.id, "smoke: playing first result");

    // --- lyrics (guarded) ---------------------------------------------------
    crate::lyrics::spawn_load(&svc, first.clone(), crate::lyrics::provider_order(&Default::default()));

    // --- controller ---------------------------------------------------------
    let cfg = CtlCfg { auto_skip: true, persistent_queue: false, ..Default::default() };
    let mut ctl = PlayerController::new(Some(svc.clone()), engine, cfg);
    ctl.set_volume(0.35);
    // MOCHY_SMOKE_TRANSITION=gapless|crossfade|crossmix: jump near the end and watch the hand-over
    let transition = std::env::var("MOCHY_SMOKE_TRANSITION").ok();
    if let (Some(m), Some(e)) = (&transition, ctl.engine()) {
        crate::controller::apply_transition_settings(e, &crate::controller::smoke_transition_settings(m));
    }
    ctl.play_with_radio(first);

    let deadline = Instant::now() + Duration::from_secs(secs.max(3) + 30);
    let mut play_started: Option<Instant> = None;
    let mut max_pos = 0.0f64;
    let mut last_log = Instant::now();
    let mut last_activity = Instant::now();
    // Scripted transport checks: (secs after start, action). Seek far forward,
    // seek back, then skip to the (prefetched) next track.
    let script: Vec<(u64, &str)> =
        if transition.is_some() { vec![(12, "seekend")] } else { vec![(6, "seek150"), (11, "seek30"), (16, "next")] };
    let mut script_i = 0usize;
    let mut seek_at: Option<(Instant, f64)> = None;
    loop {
        if let Some(t) = play_started
            && script_i < script.len()
            && t.elapsed() >= Duration::from_secs(script[script_i].0)
        {
            match script[script_i].1 {
                "seek150" => {
                    tracing::info!("smoke: seeking to 150 s");
                    ctl.seek(150.0);
                    seek_at = Some((Instant::now(), 150.0));
                }
                "seekend" => {
                    let target = (ctl.duration.unwrap_or(200.0) - 40.0).max(0.0);
                    tracing::info!(target, "smoke: seeking near the end for the hand-over");
                    ctl.seek(target);
                    seek_at = Some((Instant::now(), target));
                }
                "seek30" => {
                    tracing::info!("smoke: seeking back to 30 s");
                    ctl.seek(30.0);
                    seek_at = Some((Instant::now(), 30.0));
                }
                _ => {
                    tracing::info!("smoke: skipping to next track");
                    ctl.next();
                }
            }
            script_i += 1;
        }
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        if let Some(t) = play_started
            && t.elapsed() >= Duration::from_secs(secs)
        {
            break;
        }
        let Ok(msg) = rx.recv_timeout(Duration::from_millis(100)) else {
            ctl.tick();
            if matches!(ctl.state, PlayState::Error(_)) && last_activity.elapsed() > Duration::from_secs(5) {
                break;
            }
            continue;
        };
        last_activity = Instant::now();
        match msg {
            AppMsg::Engine(ev) => {
                match &ev {
                    Event::Progress { pos_secs, .. } => {
                        max_pos = max_pos.max(*pos_secs);
                        if let Some((t0, target)) = seek_at
                            && (*pos_secs - target).abs() < 1.5
                            && *pos_secs > target
                        {
                            tracing::info!(target, ms = t0.elapsed().as_millis() as u64, "smoke: seek landed");
                            seek_at = None;
                        }
                        if last_log.elapsed() > Duration::from_secs(2) {
                            last_log = Instant::now();
                            tracing::info!(pos = format!("{pos_secs:.1}"), dur = ?ctl.duration, "smoke: progress");
                        }
                    }
                    other => tracing::info!(?other, "smoke: engine event"),
                }
                if matches!(ev, Event::Playing) && play_started.is_none() {
                    play_started = Some(Instant::now());
                }
                ctl.on_engine(ev);
            }
            AppMsg::Prepared { generation, uid, result } => {
                match &result {
                    Ok(p) => tracing::info!(format = ?p.format, loudness = ?p.loudness_db, "smoke: stream resolved"),
                    Err(e) => tracing::warn!("smoke: resolve failed: {e}"),
                }
                ctl.on_prepared(generation, uid, result);
            }
            AppMsg::Prefetched { video_id, stream } => {
                tracing::info!(%video_id, "smoke: prefetched");
                ctl.on_prefetched(video_id, stream);
            }
            AppMsg::Radio { seed, result, play_after } => {
                tracing::info!(ok = result.is_ok(), n = result.as_ref().map(|v| v.len()).unwrap_or(0), "smoke: radio");
                ctl.on_radio(seed, result, play_after);
                tracing::info!(queue = ctl.queue.len(), "smoke: queue length");
            }
            AppMsg::Lyrics { state, .. } => tracing::info!(state = ?lyrics_summary(&state), "smoke: lyrics"),
            AppMsg::Toast(t) => tracing::info!("smoke: toast: {t}"),
            _ => {}
        }
    }
    let ok = max_pos > 1.0;
    tracing::info!(max_pos = format!("{max_pos:.1}"), state = ?ctl.state, ok, "smoke: finished");
    if let PlayState::Error(e) = &ctl.state {
        tracing::error!("smoke: final error: {e}");
    }
    ctl.shutdown();
    std::thread::sleep(Duration::from_millis(300));
    if ok { 0 } else { 1 }
}

fn lyrics_summary(s: &crate::ui::lyrics_view::LyricsState) -> String {
    use crate::ui::lyrics_view::LyricsState as L;
    match s {
        L::Idle => "idle".into(),
        L::Loading => "loading".into(),
        L::NotFound => "not found".into(),
        L::Loaded(l) => format!("{} lines from {}", l.lines.len(), l.provider),
        L::Error(e) => format!("error: {e}"),
    }
}
