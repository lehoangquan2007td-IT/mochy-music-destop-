//! The work of an import or re-sync (Android `MusicImportService` +
//! `ImportSteps`): match tracks on YouTube Music a few at a time, file songs
//! in the library, make or top up playlists, and keep progress for the UI.
//! One run at a time; YouTube-account mirroring is not ported (no account).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mochy_core::models::Song;
use mochy_storage::Db;

use crate::matcher::{SongSearch, TrackMatcher};
use crate::matches::ImportMatches;
use crate::models::{ImportProgress, ImportSource, ImportedTrack, ParsedImport, PlaylistOrigin, ResyncSummary, UnmatchedTrack};
use crate::store;

const MATCH_PARALLELISM: usize = 4;

/// Runs a closure on the database thread.
pub trait DbExec: Clone + Send + Sync + 'static {
    fn exec<R: Send + 'static>(&self, f: impl FnOnce(&Db) -> R + Send + 'static) -> impl Future<Output = Option<R>> + Send;
}

/// Shared, observable state of the importer.
#[derive(Clone, Default)]
pub struct ImportState {
    pub progress: Arc<Mutex<ImportProgress>>,
    cancel: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
}

impl ImportState {
    pub fn snapshot(&self) -> ImportProgress {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    /// Stops after the current songs; everything imported so far stays.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    /// Forgets a finished import's summary so the next one starts clean.
    pub fn clear_finished(&self) {
        if !self.is_running() {
            *self.progress.lock().unwrap() = ImportProgress::default();
        }
    }
    fn update(&self, f: impl FnOnce(&mut ImportProgress)) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    /// Claim the single run slot.
    fn start(&self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::SeqCst);
        true
    }
}

/// How a re-sync reads its playlist again.
#[derive(Debug, Clone, PartialEq)]
pub enum ResyncRead {
    Link(String),
    YouTube(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResyncRequest {
    pub playlist_id: String,
    pub playlist_name: String,
    pub source: ImportSource,
    pub read: ResyncRead,
}

/// Why a playlist can't be re-synced directly.
#[derive(Debug, Clone, PartialEq)]
pub enum ResyncRoute {
    Direct(ResyncRequest),
    /// Only the signed-in account can read it (use the account import again).
    ThroughAccount(ImportSource),
    /// Nothing says where it lives.
    NeedsLink,
}

/// How an imported playlist can be read again (Android `PlaylistResync.routeOf`).
pub fn resync_route(row: &store::ImportedPlaylistRow, name: &str, browse_id: Option<&str>, known_tracks: usize) -> Option<ResyncRoute> {
    let source = ImportSource::from_key(&row.source).filter(|s| s.is_listed())?;
    let follows_youtube = source == ImportSource::YoutubeMusic && (browse_id.is_some() || row.session_id.is_none() && row.remote_id.is_none());
    if follows_youtube {
        return None;
    }
    let req = |read| ResyncRoute::Direct(ResyncRequest { playlist_id: row.playlist_id.clone(), playlist_name: name.to_string(), source, read });
    Some(match &row.remote_url {
        // a link only shows spotify's first 100 songs, so a playlist that long needs the account
        Some(_) if source == ImportSource::Spotify && known_tracks >= crate::link::SPOTIFY_EMBED_LIMIT => ResyncRoute::ThroughAccount(source),
        Some(url) if source != ImportSource::YoutubeMusic || row.remote_id.is_none() => req(ResyncRead::Link(url.clone())),
        _ if source == ImportSource::YoutubeMusic => match &row.remote_id {
            Some(id) => req(ResyncRead::YouTube(id.clone())),
            None => ResyncRoute::NeedsLink,
        },
        _ => match source {
            ImportSource::Spotify | ImportSource::AppleMusic | ImportSource::Tidal => ResyncRoute::ThroughAccount(source),
            _ => ResyncRoute::NeedsLink,
        },
    })
}

pub struct ImportRunner<D: DbExec, S: SongSearch + Clone + 'static> {
    pub db: D,
    pub search: S,
    pub matches: Arc<ImportMatches>,
    pub state: ImportState,
    /// Called after every progress change (UI repaint).
    pub notify: Arc<dyn Fn() + Send + Sync>,
}

impl<D: DbExec, S: SongSearch + Clone + 'static> Clone for ImportRunner<D, S> {
    fn clone(&self) -> Self {
        Self { db: self.db.clone(), search: self.search.clone(), matches: self.matches.clone(), state: self.state.clone(), notify: self.notify.clone() }
    }
}

struct Run {
    source: ImportSource,
    session: Option<i64>,
}

struct Cancelled;

impl<D: DbExec, S: SongSearch + Clone + 'static> ImportRunner<D, S> {
    fn upd(&self, f: impl FnOnce(&mut ImportProgress)) {
        self.state.update(f);
        (self.notify)();
    }

    fn matcher(&self, source: ImportSource) -> Arc<TrackMatcher<S>> {
        Arc::new(TrackMatcher::new(self.search.clone(), Some(self.matches.clone()), source))
    }

    /// Import everything in `parsed`. False if a run is already going.
    pub async fn run_import(&self, parsed: ParsedImport) -> bool {
        if !self.state.start() {
            return false;
        }
        let source = parsed.provider.unwrap_or(ImportSource::Other);
        *self.state.progress.lock().unwrap() =
            ImportProgress { is_running: true, total_tracks: parsed.total_songs(), source: parsed.provider, ..Default::default() };
        (self.notify)();
        let session = self.db.exec(move |db| store::begin_session(db, source)).await.flatten();
        let run = Run { source, session };
        let result = self.import_steps(&run, &parsed).await;
        if let Some(s) = session {
            self.db.exec(move |db| store::delete_session_if_empty(db, s)).await;
        }
        self.matches.flush();
        self.upd(|p| {
            p.is_running = false;
            p.is_done = true;
            p.current_label.clear();
            if result.is_err() {
                p.cancelled = true;
            }
        });
        self.state.running.store(false, Ordering::SeqCst);
        true
    }

    async fn import_steps(&self, run: &Run, parsed: &ParsedImport) -> Result<(), Cancelled> {
        let matcher = self.matcher(run.source);
        if !parsed.liked_songs.is_empty() {
            self.import_liked(run, &matcher, &parsed.liked_songs).await?;
        }
        if !parsed.library_songs.is_empty() {
            let matched = dedupe(self.match_all(&matcher, &parsed.library_songs, None, "Library songs").await?);
            let songs: Vec<Song> = matched.iter().map(|m| m.1.clone()).collect();
            let dates = source_dates(&matched);
            let (src, session) = (run.source, run.session);
            self.db
                .exec(move |db| {
                    store::insert_songs(db, &songs);
                    store::file_into_library(db, &songs.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), src, session, &dates);
                })
                .await;
        }
        for (name, tracks) in &parsed.playlists {
            if self.state.cancelled() {
                return Err(Cancelled);
            }
            self.import_playlist(run, &matcher, name, tracks, parsed.playlist_covers.get(name).cloned(), parsed.playlist_origins.get(name).cloned())
                .await?;
        }
        Ok(())
    }

    async fn import_liked(&self, run: &Run, matcher: &Arc<TrackMatcher<S>>, tracks: &[ImportedTrack]) -> Result<(), Cancelled> {
        let matched = dedupe(self.match_all(matcher, tracks, None, "Liked songs").await?);
        if matched.is_empty() {
            return Ok(());
        }
        let dates = source_dates(&matched);
        let songs: Vec<Song> = matched.iter().map(|m| m.1.clone()).collect();
        let (src, session) = (run.source, run.session);
        let newly = self
            .db
            .exec(move |db| {
                store::insert_songs(db, &songs);
                let ids: Vec<String> = songs.iter().map(|s| s.id.clone()).collect();
                store::file_into_library(db, &ids, src, session, &dates);
                // liked when they were liked on the source, so they sit among existing likes in that order
                let with_dates: Vec<(String, i64)> = ids.iter().map(|id| (id.clone(), dates.get(id).copied().unwrap_or_else(store::now_ms))).collect();
                store::like_imported(db, &with_dates, session)
            })
            .await
            .unwrap_or_default();
        self.upd(|p| p.liked_added += newly.len());
        Ok(())
    }

    async fn import_playlist(
        &self,
        run: &Run,
        matcher: &Arc<TrackMatcher<S>>,
        name: &str,
        tracks: &[ImportedTrack],
        cover: Option<String>,
        origin: Option<PlaylistOrigin>,
    ) -> Result<(), Cancelled> {
        let src = run.source;
        // importing a playlist again tops up the copy made last time instead of duplicating it
        let (n, o) = (name.to_string(), origin.clone());
        let existing = self.db.exec(move |db| store::existing_copy(db, src, &n, o.as_ref())).await.flatten();
        if let Some(existing) = existing {
            let (pid, cover2, t) = (existing.id.clone(), cover.clone(), tracks.to_vec());
            let fresh = self
                .db
                .exec(move |db| {
                    if let Some(c) = cover2 {
                        store::set_cover_if_missing(db, &pid, &c);
                    }
                    store::new_tracks(db, &pid, &t)
                })
                .await
                .unwrap_or_default();
            let already = tracks.len() - fresh.len();
            self.upd(|p| {
                p.processed_tracks += already;
                p.matched_tracks += already;
            });
            self.top_up(run, matcher, &existing.id, &existing.name, &fresh, true).await?;
            let (pid, t, o) = (existing.id.clone(), tracks.to_vec(), origin);
            self.db.exec(move |db| store::remember(db, src, &pid, &t, o.as_ref())).await;
            self.upd(|p| p.playlist_ids.push((existing.name.clone(), existing.id.clone())));
            return Ok(());
        }

        let matched = self.match_all(matcher, tracks, Some(name), name).await?;
        let mut seen = std::collections::HashSet::new();
        let songs: Vec<Song> = matched.into_iter().map(|m| m.1).filter(|s| seen.insert(s.id.clone())).collect();
        // nothing found: don't leave an empty playlist behind (an intentionally empty one has no tracks at all)
        if songs.is_empty() && !tracks.is_empty() {
            return Ok(());
        }
        let (n, session, t, o) = (name.to_string(), run.session, tracks.to_vec(), origin);
        let id = self
            .db
            .exec(move |db| {
                store::insert_songs(db, &songs);
                let id = store::create_imported_playlist(db, &n, cover.as_deref(), src, session, o.as_ref(), &songs)?;
                store::remember(db, src, &id, &t, o.as_ref());
                store::file_into_library(db, &songs.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), src, session, &HashMap::new());
                Some(id)
            })
            .await
            .flatten();
        if let Some(id) = id {
            let n = name.to_string();
            self.upd(|p| {
                p.playlists_created += 1;
                p.playlist_ids.push((n, id));
            });
        }
        Ok(())
    }

    /// Finds tracks and adds them to the end of the playlist, skipping any it has.
    async fn top_up(
        &self,
        run: &Run,
        matcher: &Arc<TrackMatcher<S>>,
        playlist_id: &str,
        name: &str,
        tracks: &[ImportedTrack],
        into_library: bool,
    ) -> Result<Vec<String>, Cancelled> {
        if tracks.is_empty() {
            return Ok(Vec::new());
        }
        let matched = self.match_all(matcher, tracks, Some(name), name).await?;
        let songs: Vec<Song> = matched.into_iter().map(|m| m.1).collect();
        if songs.is_empty() {
            return Ok(Vec::new());
        }
        let (pid, src, session) = (playlist_id.to_string(), run.source, run.session);
        Ok(self
            .db
            .exec(move |db| {
                store::insert_songs(db, &songs);
                let added = store::append_new(db, &pid, &songs);
                if into_library {
                    store::file_into_library(db, &songs.iter().map(|s| s.id.clone()).collect::<Vec<_>>(), src, session, &HashMap::new());
                }
                added
            })
            .await
            .unwrap_or_default())
    }

    /// Matches in parallel (a few searches at a time) and keeps the source order.
    async fn match_all(
        &self,
        matcher: &Arc<TrackMatcher<S>>,
        tracks: &[ImportedTrack],
        playlist: Option<&str>,
        label: &str,
    ) -> Result<Vec<(ImportedTrack, Song)>, Cancelled> {
        let mut out: Vec<Option<(ImportedTrack, Song)>> = vec![None; tracks.len()];
        let mut set = tokio::task::JoinSet::new();
        let mut next = 0usize;
        let mut cancelled = false;
        loop {
            while !cancelled && set.len() < MATCH_PARALLELISM && next < tracks.len() {
                if self.state.cancelled() {
                    cancelled = true;
                    break;
                }
                let (i, t, m) = (next, tracks[next].clone(), matcher.clone());
                set.spawn(async move {
                    let r = m.match_track(&t).await;
                    (i, t, r)
                });
                next += 1;
            }
            let Some(joined) = set.join_next().await else { break };
            let Ok((i, track, result)) = joined else { continue };
            let title = track.title.clone();
            let label = format!("{label} — {title}");
            let pl = playlist.map(str::to_string);
            let matched = result.is_some();
            let unmatched = (!matched).then(|| UnmatchedTrack { track: track.clone(), playlist_name: pl });
            self.upd(|p| {
                p.processed_tracks += 1;
                if matched {
                    p.matched_tracks += 1;
                }
                p.current_label = label;
                if let Some(u) = unmatched {
                    p.unmatched.push(u);
                }
            });
            if let Some(song) = result {
                out[i] = Some((track, song));
            }
        }
        matcher.flush();
        if cancelled {
            return Err(Cancelled);
        }
        Ok(out.into_iter().flatten().collect())
    }

    // ---- unmatched review ---------------------------------------------------------

    /// Search results for an unmatched track, best fit first.
    pub async fn candidates(&self, track: &ImportedTrack) -> Vec<(Song, f64)> {
        TrackMatcher::new(self.search.clone(), None, ImportSource::Other).candidates(track).await
    }

    /// The user picked `song` for an unmatched track: remember the pick for
    /// every later import, and put it where the track belonged.
    pub async fn resolve_unmatched(&self, um: &UnmatchedTrack, song: Song) {
        let source = self.state.snapshot().source.unwrap_or(ImportSource::Other);
        self.matches.pin_track(&um.track, source, &song);
        let playlist_id = um
            .playlist_name
            .as_ref()
            .and_then(|n| self.state.snapshot().playlist_ids.iter().find(|(name, _)| name == n).map(|(_, id)| id.clone()));
        let liked = um.playlist_name.is_none();
        let s = song.clone();
        self.db
            .exec(move |db| {
                store::insert_songs(db, std::slice::from_ref(&s));
                store::file_into_library(db, std::slice::from_ref(&s.id), source, None, &HashMap::new());
                if let Some(pid) = playlist_id {
                    store::append_new(db, &pid, std::slice::from_ref(&s));
                } else if liked {
                    store::like_imported(db, &[(s.id.clone(), store::now_ms())], None);
                }
            })
            .await;
        let t = um.track.clone();
        self.upd(|p| {
            if let Some(i) = p.unmatched.iter().position(|u| u.track == t) {
                p.unmatched.remove(i);
                p.matched_tracks += 1;
            }
        });
    }

    // ---- re-sync ------------------------------------------------------------------------

    /// Re-reads imported playlists and adds songs new on the service to their
    /// end; ones the user removed here stay out. `read` fetches each playlist.
    pub async fn run_resync<R, Fut>(&self, requests: Vec<ResyncRequest>, read: R) -> bool
    where
        R: Fn(ResyncRead) -> Fut,
        Fut: Future<Output = Result<(Vec<ImportedTrack>, PlaylistOrigin, Option<String>), String>>,
    {
        let Some(first) = requests.first() else { return true };
        if !self.state.start() {
            return false;
        }
        *self.state.progress.lock().unwrap() = ImportProgress {
            is_running: true,
            source: Some(first.source),
            resync: Some(ResyncSummary { playlist_name: first.playlist_name.clone(), playlists: requests.len(), ..Default::default() }),
            ..Default::default()
        };
        (self.notify)();
        for req in requests {
            if self.state.cancelled() {
                self.upd(|p| p.cancelled = true);
                break;
            }
            let label = format!("Reading \"{}\" on {}", req.playlist_name, req.source.label());
            let name = req.playlist_name.clone();
            self.upd(|p| {
                p.source = Some(req.source);
                p.current_label = label;
                if let Some(r) = &mut p.resync {
                    r.playlist_name = name;
                }
            });
            match self.resync_one(&req, &read).await {
                Ok((added, note)) => self.upd(|p| {
                    if let Some(r) = &mut p.resync {
                        r.done += 1;
                        r.added += added;
                        r.playlists_changed += usize::from(added > 0);
                        if r.note.is_none() {
                            r.note = note;
                        }
                    }
                }),
                Err(e) => {
                    tracing::warn!("re-sync of {} failed: {e}", req.playlist_name);
                    let n = req.playlist_name.clone();
                    self.upd(|p| {
                        p.error = Some(e);
                        if let Some(r) = &mut p.resync {
                            r.done += 1;
                            r.failed.push(n);
                        }
                    });
                }
            }
        }
        self.matches.flush();
        self.upd(|p| {
            p.is_running = false;
            p.is_done = true;
            p.current_label.clear();
        });
        self.state.running.store(false, Ordering::SeqCst);
        true
    }

    async fn resync_one<R, Fut>(&self, req: &ResyncRequest, read: &R) -> Result<(usize, Option<String>), String>
    where
        R: Fn(ResyncRead) -> Fut,
        Fut: Future<Output = Result<(Vec<ImportedTrack>, PlaylistOrigin, Option<String>), String>>,
    {
        let (tracks, origin, note) = read(req.read.clone()).await?;
        let pid = req.playlist_id.clone();
        let src = req.source;
        let info = self
            .db
            .exec(move |db| {
                let pl = db.playlist_by_id(&pid).ok().flatten()?;
                // new songs go in under the import that brought the playlist, so undoing it takes them too
                let owner = store::imported_playlist(db, &pid).and_then(|r| r.session_id);
                let mut own = None;
                let session = match owner {
                    Some(o) if store::session_exists(db, o) => Some(o),
                    Some(_) => {
                        own = store::begin_session(db, src);
                        own
                    }
                    None => None,
                };
                Some((pl, owner.is_some(), session, own))
            })
            .await
            .flatten();
        let Some((pl, owned, session, own_session)) = info else { return Err("That playlist isn't in your library any more.".into()) };
        let (pid, t) = (pl.id.clone(), tracks.clone());
        let fresh = self.db.exec(move |db| store::new_tracks(db, &pid, &t)).await.unwrap_or_default();
        self.upd(|p| p.total_tracks += fresh.len());
        let run = Run { source: src, session };
        let added = self.top_up(&run, &self.matcher(src), &pl.id, &pl.name, &fresh, owned).await.map_err(|_| "Cancelled".to_string())?;
        let (pid, t, o) = (pl.id.clone(), tracks, origin);
        self.db
            .exec(move |db| {
                store::remember(db, src, &pid, &t, Some(&o));
                if let Some(s) = own_session {
                    store::delete_session_if_empty(db, s);
                }
            })
            .await;
        let (n, id) = (pl.name.clone(), pl.id.clone());
        self.upd(|p| p.playlist_ids.push((n, id)));
        Ok((added.len(), note))
    }
}

fn dedupe(v: Vec<(ImportedTrack, Song)>) -> Vec<(ImportedTrack, Song)> {
    let mut seen = std::collections::HashSet::new();
    v.into_iter().filter(|m| seen.insert(m.1.id.clone())).collect()
}

/// When each song was added on the source so lists keep its order; a song
/// with no date gets one just before the song above it (dateless sources
/// list newest first).
fn source_dates(matched: &[(ImportedTrack, Song)]) -> HashMap<String, i64> {
    let now = store::now_ms();
    let mut dates = HashMap::new();
    let mut previous: Option<i64> = None;
    for (t, s) in matched {
        let d = t.added_at_ms.or(previous.map(|p| p - 1000)).unwrap_or(now);
        dates.insert(s.id.clone(), d);
        previous = Some(d);
    }
    dates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::SearchFilter;
    use mochy_core::models::ArtistRef;

    /// The DB on a dedicated thread, like the app's DbWorker.
    #[derive(Clone)]
    struct TestDb(std::sync::mpsc::Sender<Box<dyn FnOnce(&Db) + Send>>);

    impl TestDb {
        fn new() -> Self {
            let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce(&Db) + Send>>();
            std::thread::spawn(move || {
                let db = Db::open_in_memory().unwrap();
                while let Ok(job) = rx.recv() {
                    job(&db);
                }
            });
            Self(tx)
        }
    }

    impl DbExec for TestDb {
        async fn exec<R: Send + 'static>(&self, f: impl FnOnce(&Db) -> R + Send + 'static) -> Option<R> {
            let (otx, orx) = tokio::sync::oneshot::channel();
            self.0
                .send(Box::new(move |db| {
                    let _ = otx.send(f(db));
                }))
                .ok()?;
            orx.await.ok()
        }
    }

    #[derive(Clone)]
    struct Catalog;

    impl SongSearch for Catalog {
        async fn search(&self, q: &str, f: SearchFilter) -> Result<Vec<Song>, String> {
            if f == SearchFilter::Video || q.contains("Unknown") {
                return Ok(Vec::new());
            }
            let title = q.split(" Band").next().unwrap_or(q).to_string();
            let mut s = Song::remote(format!("id-{}", title.replace(' ', "_")), title);
            s.artists = vec![ArtistRef::new("Band")];
            s.duration_secs = Some(200.0);
            Ok(vec![s])
        }
    }

    fn t(title: &str) -> ImportedTrack {
        ImportedTrack { duration_ms: Some(200_000), ..ImportedTrack::new(title, "Band", None) }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_import_resync_and_review() {
        let db = TestDb::new();
        let runner = ImportRunner { db: db.clone(), search: Catalog, matches: Arc::new(ImportMatches::in_memory()), state: ImportState::default(), notify: Arc::new(|| {}) };
        let parsed = ParsedImport {
            liked_songs: vec![t("Liked One")],
            playlists: vec![("Road Trip".into(), vec![t("A"), t("Unknown Song"), t("B")])],
            provider: Some(ImportSource::Spotify),
            playlist_origins: [("Road Trip".to_string(), PlaylistOrigin { remote_id: Some("r1".into()), remote_url: Some("https://open.spotify.com/playlist/r1".into()) })].into(),
            ..Default::default()
        };
        assert!(runner.run_import(parsed).await);
        let p = runner.state.snapshot();
        assert!(p.is_done && !p.is_running);
        assert_eq!(p.total_tracks, 4);
        assert_eq!(p.processed_tracks, 4);
        assert_eq!(p.matched_tracks, 3);
        assert_eq!(p.liked_added, 1);
        assert_eq!(p.playlists_created, 1);
        assert_eq!(p.unmatched.len(), 1);
        let (_, pid) = p.playlist_ids[0].clone();
        let pid2 = pid.clone();
        let songs = db.exec(move |d| d.playlist_songs(&pid2).unwrap()).await.unwrap();
        // source order kept despite parallel matching
        assert_eq!(songs.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), vec!["A", "B"]);

        // review: pick a song for the unmatched one
        let um = p.unmatched[0].clone();
        let mut pick = Song::remote("picked00001", "Unknown Song");
        pick.artists = vec![ArtistRef::new("Band")];
        runner.resolve_unmatched(&um, pick).await;
        assert!(runner.state.snapshot().unmatched.is_empty());
        let pid2 = pid.clone();
        assert_eq!(db.exec(move |d| d.playlist_songs(&pid2).unwrap().len()).await, Some(3));

        // re-sync: the service now has one more song; only it is added
        let row_pid = pid.clone();
        let (row, name) = db
            .exec(move |d| (store::imported_playlist(d, &row_pid).unwrap(), d.playlist_by_id(&row_pid).unwrap().unwrap().name))
            .await
            .unwrap();
        let Some(ResyncRoute::Direct(req)) = resync_route(&row, &name, None, 3) else { panic!("expected a direct route") };
        assert_eq!(req.read, ResyncRead::Link("https://open.spotify.com/playlist/r1".into()));
        let ok = runner
            .run_resync(vec![req], |_| async { Ok((vec![t("A"), t("B"), t("C")], PlaylistOrigin { remote_id: Some("r1".into()), remote_url: None }, None)) })
            .await;
        assert!(ok);
        let r = runner.state.snapshot().resync.unwrap();
        assert_eq!((r.done, r.added, r.playlists_changed), (1, 1, 1));
        let pid2 = pid.clone();
        assert_eq!(db.exec(move |d| d.playlist_songs(&pid2).unwrap().len()).await, Some(4));

        // importing the same link again tops up instead of duplicating
        let again = ParsedImport {
            playlists: vec![("Road Trip".into(), vec![t("A"), t("D")])],
            provider: Some(ImportSource::Spotify),
            playlist_origins: [("Road Trip".to_string(), PlaylistOrigin { remote_id: Some("r1".into()), remote_url: None })].into(),
            ..Default::default()
        };
        assert!(runner.run_import(again).await);
        let count = db.exec(|d| d.playlists().unwrap().len()).await.unwrap();
        assert_eq!(count, 1);
        let pid2 = pid.clone();
        assert_eq!(db.exec(move |d| d.playlist_songs(&pid2).unwrap().len()).await, Some(5));

        // undo the first import: its playlist goes, the like is taken back
        let undone = db.exec(|d| {
            let s = store::sessions(d);
            store::undo_session(d, s.last().unwrap().id)
        });
        assert!(undone.await.unwrap() >= 1);
        assert_eq!(db.exec(|d| d.playlists().unwrap().len()).await, Some(0));
        assert_eq!(db.exec(|d| d.liked_songs().unwrap().len()).await, Some(0));
    }

    #[test]
    fn routes() {
        let row = |source: &str, sid: Option<i64>, rid: Option<&str>, url: Option<&str>| store::ImportedPlaylistRow {
            playlist_id: "p".into(),
            source: source.into(),
            session_id: sid,
            imported_at: 0,
            remote_id: rid.map(str::to_string),
            remote_url: url.map(str::to_string),
            last_synced_at: None,
        };
        assert!(matches!(resync_route(&row("spotify", Some(1), Some("x"), Some("u")), "n", None, 150), Some(ResyncRoute::ThroughAccount(_))));
        assert!(matches!(resync_route(&row("tidal", Some(1), Some("x"), None), "n", None, 0), Some(ResyncRoute::ThroughAccount(_))));
        assert!(matches!(resync_route(&row("deezer", Some(1), None, None), "n", None, 0), Some(ResyncRoute::NeedsLink)));
        let Some(ResyncRoute::Direct(r)) = resync_route(&row("youtube_music", Some(1), Some("PL1"), Some("u")), "n", None, 0) else { panic!() };
        assert_eq!(r.read, ResyncRead::YouTube("PL1".into()));
        assert_eq!(resync_route(&row("other", Some(1), None, None), "n", None, 0), None);
        assert_eq!(resync_route(&row("youtube_music", None, None, None), "n", None, 0), None);
    }
}
