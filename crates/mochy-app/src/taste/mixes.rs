//! "Made for you" mixes ("Dành cho bạn"), Spotify style, built from the taste
//! profile and YouTube Music radios:
//!
//! - **Daily Mix 1–3**: the top seeds (`TasteProfile::seeds`) are grouped by
//!   how much their radios overlap (seeds whose radios share artists belong
//!   together), each group's radios are interleaved and ~30 % of the user's
//!   own high-score songs by the group's artists are mixed in.
//! - **Discover for you**: songs from the seeds' radios (and one more hop of
//!   radio) the user never played, liked or saved; max 2 per artist.
//! - **Daypart mix** ("Gu buổi sáng"…): radio of the user's top songs by the
//!   artists they play at this time of day.
//!
//! Generation runs on the tokio runtime (≤ [`MAX_INFLIGHT`] InnerTube calls at
//! once), is cached as JSON in the data dir ([`cache_path`]) and redone when
//! the cache is older than [`STALE_MS`], when there is none yet, or on
//! [`request_refresh`]. The daypart mix alone is redone when the part of the
//! day changes. A new profile version alone does not regenerate anything.
//!
//! The UI only uses [`current`], [`get`], [`generating`], [`request_refresh`];
//! `app.rs` calls [`tick`] every frame (cheap).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use mochy_core::models::Song;
use serde::{Deserialize, Serialize};

use super::{Daypart, TasteProfile};
use crate::controller::title_key;
use crate::services::Services;
use crate::ui::i18n::tr;

/// InnerTube calls in flight at once while generating.
pub const MAX_INFLIGHT: usize = 3;
/// Regenerate the mixes after this long.
pub const STALE_MS: i64 = 24 * 3_600_000;
/// Wait this long before retrying a generation that produced nothing.
const RETRY_MS: i64 = 15 * 60_000;
const CACHE_VERSION: u32 = 1;

const DAILY_MIXES: usize = 3;
const DAILY_SEEDS: usize = 9;
const DAILY_LEN: usize = 40;
const DAILY_MIN: usize = 15;
const DISCOVER_LEN: usize = 30;
const DISCOVER_MIN: usize = 10;
const DISCOVER_PER_ARTIST: usize = 2;
const DISCOVER_HOPS: usize = 3;
const DAYPART_LEN: usize = 40;
const DAYPART_MIN: usize = 15;
const DAYPART_SEEDS: usize = 3;
/// Every `OWN_EVERY`-th song of a daily/daypart mix is one of the user's own.
const OWN_EVERY: usize = 3;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixKind {
    /// Daily Mix `n` (1-based).
    Daily(u8),
    Discover,
    /// Mix for a [`Daypart`] (its `index()`).
    Daypart(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mix {
    /// Stable across regenerations (`daily1`, `discover`, `daypart`): the
    /// route of an open mix page keeps working.
    pub id: String,
    pub kind: MixKind,
    /// Main artists (display names), most important first.
    pub artists: Vec<String>,
    /// More artists than `artists` are in the mix ("… and more").
    pub more: bool,
    pub songs: Arc<Vec<Song>>,
}

impl Mix {
    pub fn title(&self) -> String {
        match self.kind {
            MixKind::Daily(n) => format!("{} {n}", tr("Daily Mix")),
            MixKind::Discover => tr("Discover for you").into(),
            MixKind::Daypart(i) => tr(match i {
                0 => "Morning mix",
                1 => "Afternoon mix",
                2 => "Evening mix",
                _ => "Night mix",
            })
            .into(),
        }
    }

    /// "Sơn Tùng M-TP, Soobin and more".
    pub fn subtitle(&self) -> String {
        let names = self.artists.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
        match (names.is_empty(), self.more || self.artists.len() > 2) {
            (true, _) => tr("New songs picked for your taste").into(),
            (false, true) => format!("{names} {}", tr("and more")),
            (false, false) => names,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MixSet {
    /// Epoch millis of the last full generation.
    pub generated_at: i64,
    /// [`Daypart`] index the daypart mix was made for.
    pub daypart: u8,
    /// Local UTC offset at generation (to tell the daypart later).
    pub utc_offset_secs: i64,
    pub mixes: Vec<Mix>,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct State {
    started: bool,
    loading: bool,
    generating: bool,
    force: bool,
    last_attempt: i64,
    set: Option<Arc<MixSet>>,
}

static STATE: Mutex<State> =
    Mutex::new(State { started: false, loading: false, generating: false, force: false, last_attempt: 0, set: None });

/// The current mixes (cached or freshly generated).
pub fn current() -> Option<Arc<MixSet>> {
    STATE.lock().ok().and_then(|s| s.set.clone())
}

/// One mix by id.
pub fn get(id: &str) -> Option<Mix> {
    current().and_then(|s| s.mixes.iter().find(|m| m.id == id).cloned())
}

/// A generation is running.
pub fn generating() -> bool {
    STATE.lock().map(|s| s.generating || s.loading).unwrap_or(false)
}

/// Regenerate all mixes on the next [`tick`].
pub fn request_refresh() {
    if let Ok(mut s) = STATE.lock() {
        s.force = true;
    }
}

/// What a tick has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Nothing,
    All,
    /// Only the daypart mix (the part of the day changed).
    Daypart,
}

/// Local hour of `now_ms` for a UTC offset.
fn local_hour(now_ms: i64, utc_offset_secs: i64) -> u32 {
    ((now_ms / 1000 + utc_offset_secs).rem_euclid(86_400) / 3600) as u32
}

/// Cache freshness: what to regenerate given the current set.
pub fn need(set: Option<&MixSet>, now: i64, force: bool, last_attempt: i64) -> Need {
    if force {
        return Need::All;
    }
    if last_attempt > 0 && now - last_attempt < RETRY_MS {
        return Need::Nothing;
    }
    let Some(set) = set.filter(|s| !s.mixes.is_empty()) else { return Need::All };
    if now - set.generated_at >= STALE_MS || set.generated_at > now + 3_600_000 {
        return Need::All;
    }
    let part = Daypart::from_hour(local_hour(now, set.utc_offset_secs)).index() as u8;
    if part != set.daypart {
        return Need::Daypart;
    }
    Need::Nothing
}

/// Call every frame: loads the cache once, then (re)generates when needed.
pub fn tick(svc: &Services) {
    let Ok(mut st) = STATE.lock() else { return };
    if !st.started {
        st.started = true;
        st.loading = true;
        let bus = svc.bus.clone();
        svc.spawn(async move {
            let set = load_cache().await;
            if let Ok(mut st) = STATE.lock() {
                st.loading = false;
                if st.set.is_none() {
                    st.set = set.map(Arc::new);
                }
            }
            bus.request_repaint();
        });
        return;
    }
    if st.loading || st.generating {
        return;
    }
    let Some(profile) = super::current() else { return };
    if profile.is_empty() {
        return;
    }
    let now = crate::browse::now_ms();
    let what = need(st.set.as_deref(), now, st.force, st.last_attempt);
    if what == Need::Nothing {
        return;
    }
    st.force = false;
    st.generating = true;
    st.last_attempt = now;
    let prev = st.set.clone();
    drop(st);
    tracing::info!(?what, "generating taste mixes");
    let svc2 = svc.clone();
    svc.spawn(async move {
        let out = generate(&svc2, profile, what, prev).await;
        if let Some(set) = &out {
            tracing::info!(mixes = set.mixes.len(), "taste mixes ready");
            save_cache(set).await;
        }
        if let Ok(mut st) = STATE.lock() {
            st.generating = false;
            if let Some(set) = out {
                st.set = Some(Arc::new(set));
            }
        }
        svc2.bus.request_repaint();
    });
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u32,
    generated_at: i64,
    daypart: u8,
    utc_offset_secs: i64,
    mixes: Vec<CachedMix>,
}

#[derive(Serialize, Deserialize)]
struct CachedMix {
    id: String,
    kind: MixKind,
    artists: Vec<String>,
    more: bool,
    songs: Vec<Song>,
}

/// `<data dir>/taste_mixes.json`.
pub fn cache_path() -> std::path::PathBuf {
    mochy_storage::data_dir().join("taste_mixes.json")
}

fn to_cache(set: &MixSet) -> CacheFile {
    CacheFile {
        version: CACHE_VERSION,
        generated_at: set.generated_at,
        daypart: set.daypart,
        utc_offset_secs: set.utc_offset_secs,
        mixes: set
            .mixes
            .iter()
            .map(|m| CachedMix { id: m.id.clone(), kind: m.kind, artists: m.artists.clone(), more: m.more, songs: m.songs.as_ref().clone() })
            .collect(),
    }
}

fn from_cache(c: CacheFile) -> Option<MixSet> {
    if c.version != CACHE_VERSION || c.mixes.is_empty() {
        return None;
    }
    Some(MixSet {
        generated_at: c.generated_at,
        daypart: c.daypart,
        utc_offset_secs: c.utc_offset_secs,
        mixes: c
            .mixes
            .into_iter()
            .map(|m| Mix { id: m.id, kind: m.kind, artists: m.artists, more: m.more, songs: Arc::new(m.songs) })
            .collect(),
    })
}

async fn load_cache() -> Option<MixSet> {
    let bytes = tokio::fs::read(cache_path()).await.ok()?;
    match serde_json::from_slice::<CacheFile>(&bytes) {
        Ok(c) => from_cache(c),
        Err(e) => {
            tracing::warn!("taste mixes cache unreadable: {e}");
            None
        }
    }
}

async fn save_cache(set: &MixSet) {
    let path = cache_path();
    let json = match serde_json::to_vec(&to_cache(set)) {
        Ok(j) => j,
        Err(e) => return tracing::warn!("taste mixes cache: {e}"),
    };
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let tmp = path.with_extension("json.tmp");
    let r = async {
        tokio::fs::write(&tmp, &json).await?;
        tokio::fs::rename(&tmp, &path).await
    };
    if let Err(e) = r.await {
        tracing::warn!("taste mixes cache write failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Generation (async orchestration)
// ---------------------------------------------------------------------------

/// Songs the user already knows (played, liked, in the library).
#[derive(Debug, Default)]
pub struct Known {
    pub ids: HashSet<String>,
    pub titles: HashSet<String>,
}

impl Known {
    fn add(&mut self, s: &Song) {
        self.ids.insert(s.id.clone());
        if let Some(v) = &s.video_id {
            self.ids.insert(v.clone());
        }
        let t = title_key(&s.title);
        if !t.is_empty() {
            self.titles.insert(t);
        }
    }

    pub fn knows(&self, s: &Song) -> bool {
        self.ids.contains(&s.id) || self.titles.contains(&title_key(&s.title))
    }
}

async fn generate(svc: &Services, profile: Arc<TasteProfile>, what: Need, prev: Option<Arc<MixSet>>) -> Option<MixSet> {
    let now = crate::browse::now_ms();
    let (known, utc_offset_secs) = svc
        .db
        .query(|db| {
            let mut k = Known::default();
            for list in [db.all_songs(), db.liked_songs(), db.recent_history(100_000)] {
                for s in list.unwrap_or_default() {
                    k.add(&s);
                }
            }
            (k, db.local_utc_offset_secs().unwrap_or(0))
        })
        .await
        .ok()?;
    let part = Daypart::from_hour(local_hour(now, utc_offset_secs));
    let daypart_seeds = daypart_seeds(&profile, part, DAYPART_SEEDS);
    let daily_seeds = if what == Need::All { profile.seeds(DAILY_SEEDS) } else { Vec::new() };
    let mut ids: Vec<String> = daily_seeds.iter().chain(&daypart_seeds).filter_map(|s| s.video_id.clone()).collect();
    dedupe_strings(&mut ids);

    let client = if ids.is_empty() { None } else { svc.ytm().await.ok() };
    let radios = match &client {
        Some(c) => fetch_radios(c.clone(), ids).await,
        None => HashMap::new(),
    };
    let day = (now / 86_400_000) as u64;

    let mut mixes = Vec::new();
    let mut used = HashSet::new();
    if what == Need::All {
        mixes.extend(build_daily(&profile, &daily_seeds, &radios, &mut used, day));
        // One more hop of radio from unknown songs for the discover mix.
        let pool = interleave(&radio_lists(&daily_seeds, &radios));
        let hops: Vec<String> = discover_filter(pool, &profile, &known, &used, 1, DISCOVER_HOPS)
            .into_iter()
            .filter_map(|s| s.video_id)
            .collect();
        let hop_radios = match (&client, hops.is_empty()) {
            (Some(c), false) => fetch_radios(c.clone(), hops.clone()).await,
            _ => HashMap::new(),
        };
        let hop_lists: Vec<Vec<Song>> = hops.iter().filter_map(|h| hop_radios.get(h).cloned()).collect();
        if let Some(m) = build_discover(&profile, &daily_seeds, &radios, &hop_lists, &known, &used) {
            mixes.push(m);
        }
    }
    let daypart_mix = build_daypart(&profile, part, &daypart_seeds, &radios, day);

    match what {
        Need::All => {
            mixes.extend(daypart_mix);
            if mixes.is_empty() {
                tracing::warn!("taste mixes: nothing generated (offline or too little history)");
                return None;
            }
            Some(MixSet { generated_at: now, daypart: part.index() as u8, utc_offset_secs, mixes })
        }
        Need::Daypart => {
            let prev = prev?;
            let mut mixes: Vec<Mix> = prev.mixes.iter().filter(|m| !matches!(m.kind, MixKind::Daypart(_))).cloned().collect();
            mixes.extend(daypart_mix);
            if mixes.is_empty() {
                return None;
            }
            Some(MixSet { generated_at: prev.generated_at, daypart: part.index() as u8, utc_offset_secs, mixes })
        }
        Need::Nothing => None,
    }
}

/// Radios of `ids`, ≤ [`MAX_INFLIGHT`] calls at once; failed ones are missing.
async fn fetch_radios(client: Arc<mochy_innertube::YtmClient>, ids: Vec<String>) -> HashMap<String, Vec<Song>> {
    let sem = Arc::new(tokio::sync::Semaphore::new(MAX_INFLIGHT));
    let mut set = tokio::task::JoinSet::new();
    for id in ids {
        let c = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let r = tokio::time::timeout(std::time::Duration::from_secs(45), c.get_queue(&id)).await;
            (id, r)
        });
    }
    let mut out = HashMap::new();
    while let Some(j) = set.join_next().await {
        match j {
            Ok((id, Ok(Ok(items)))) => {
                out.insert(id, items.iter().map(crate::controller::song_from_queue_item).collect());
            }
            Ok((id, Ok(Err(e)))) => tracing::warn!(%id, "mix radio failed: {e}"),
            Ok((id, Err(_))) => tracing::warn!(%id, "mix radio timed out"),
            Err(e) => tracing::warn!("mix radio task: {e}"),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pure building blocks
// ---------------------------------------------------------------------------

fn dedupe_strings(v: &mut Vec<String>) {
    let mut seen = HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

/// Lowercased first artist ("" when unknown).
pub fn artist_key(s: &Song) -> String {
    s.artists.first().map(|a| a.name.trim().to_lowercase()).unwrap_or_default()
}

/// Same song already taken: by id or by "same title" ([`title_key`]).
#[derive(Debug, Default)]
pub struct Dedupe {
    ids: HashSet<String>,
    titles: HashSet<String>,
}

impl Dedupe {
    /// `true` (and remembered) when `s` is new.
    pub fn admit(&mut self, s: &Song) -> bool {
        if self.ids.contains(&s.id) {
            return false;
        }
        let t = title_key(&s.title);
        if !t.is_empty() && self.titles.contains(&t) {
            return false;
        }
        self.ids.insert(s.id.clone());
        if !t.is_empty() {
            self.titles.insert(t);
        }
        true
    }
}

/// Playable, not disliked, not seen yet.
fn clean(profile: &TasteProfile, songs: Vec<Song>, used: &HashSet<String>) -> Vec<Song> {
    let mut d = Dedupe::default();
    songs
        .into_iter()
        .filter(|s| s.video_id.is_some() && !profile.is_disliked(&s.id) && !used.contains(&s.id) && d.admit(s))
        .collect()
}

/// Round-robin: first of each list, then second of each…
pub fn interleave(lists: &[Vec<Song>]) -> Vec<Song> {
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::with_capacity(lists.iter().map(Vec::len).sum());
    for i in 0..longest {
        for l in lists {
            if let Some(s) = l.get(i) {
                out.push(s.clone());
            }
        }
    }
    out
}

/// `own` songs at every `every`-th slot (from the first), `fresh` in between;
/// the leftovers of either list follow.
pub fn blend(fresh: Vec<Song>, own: Vec<Song>, every: usize) -> Vec<Song> {
    let every = every.max(1);
    let mut out = Vec::with_capacity(fresh.len() + own.len());
    let mut f = fresh.into_iter().peekable();
    let mut o = own.into_iter().peekable();
    while f.peek().is_some() || o.peek().is_some() {
        let take_own = out.len() % every == 0;
        let next = if take_own { o.next().or_else(|| f.next()) } else { f.next().or_else(|| o.next()) };
        out.extend(next);
    }
    out
}

/// Jaccard similarity of two artist sets.
pub fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    inter as f32 / (a.len() + b.len() - inter) as f32
}

/// Split seeds (by index, best first) into ≤ `groups` groups of seeds that
/// belong together. `sets[i]` = artists of seed `i`'s radio. The first group
/// is anchored on the best seed, the other anchors are the seeds least like
/// the anchors so far (distinct mixes), then every other seed joins the
/// anchor it is most like (groups stay about the same size).
pub fn group_seeds(sets: &[HashSet<String>], groups: usize) -> Vec<Vec<usize>> {
    let n = sets.len();
    let g = groups.min(n);
    if g == 0 {
        return Vec::new();
    }
    let mut anchors = vec![0usize];
    while anchors.len() < g {
        let next = (0..n)
            .filter(|i| !anchors.contains(i))
            .map(|i| (i, anchors.iter().map(|&a| similarity(&sets[i], &sets[a])).fold(0.0f32, f32::max)))
            // Least like any anchor; ties → the better seed (lower index).
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
            .map(|(i, _)| i);
        match next {
            Some(i) => anchors.push(i),
            None => break,
        }
    }
    let cap = n.div_ceil(anchors.len());
    let mut out: Vec<Vec<usize>> = anchors.iter().map(|&a| vec![a]).collect();
    for i in (0..n).filter(|i| !anchors.contains(i)) {
        let best = (0..out.len())
            .filter(|&k| out[k].len() < cap)
            .map(|k| (k, similarity(&sets[i], &sets[anchors[k]])))
            .max_by(|a, b| a.1.total_cmp(&b.1).then(out[b.0].len().cmp(&out[a.0].len())).then(b.0.cmp(&a.0)))
            .map(|(k, _)| k);
        if let Some(k) = best {
            out[k].push(i);
        }
    }
    out
}

/// All artist names (lowercased) of a seed and its radio.
fn artist_set(seed: &Song, radio: &[Song]) -> HashSet<String> {
    std::iter::once(seed)
        .chain(radio)
        .flat_map(|s| s.artists.iter())
        .map(|a| a.name.trim().to_lowercase())
        .filter(|n| !n.is_empty())
        .collect()
}

fn radio_of<'a>(seed: &Song, radios: &'a HashMap<String, Vec<Song>>) -> Option<&'a Vec<Song>> {
    seed.video_id.as_ref().and_then(|v| radios.get(v)).filter(|r| !r.is_empty())
}

fn radio_lists(seeds: &[Song], radios: &HashMap<String, Vec<Song>>) -> Vec<Vec<Song>> {
    seeds.iter().filter_map(|s| radio_of(s, radios).cloned()).collect()
}

/// Small deterministic shuffle (xorshift) so own songs rotate day to day.
fn day_shuffle<T>(v: &mut [T], seed: u64) {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

/// The user's best songs by `artists` (lowercased), rotated by `day`.
fn own_songs(profile: &TasteProfile, artists: &HashSet<String>, n: usize, used: &HashSet<String>, day: u64) -> Vec<Song> {
    let mut pool: Vec<Song> = profile
        .songs
        .iter()
        .filter(|s| s.score > 0.0 && s.song.video_id.is_some() && !profile.is_disliked(&s.song.id) && !used.contains(&s.song.id))
        .filter(|s| artists.contains(&artist_key(&s.song)))
        .take(n * 2)
        .map(|s| s.song.clone())
        .collect();
    day_shuffle(&mut pool, day);
    pool.truncate(n);
    pool
}

/// Display names of the most common artists of `songs`, seeds' artists first.
fn main_artists(seeds: &[&Song], songs: &[Song], n: usize) -> (Vec<String>, bool) {
    let mut names: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for s in seeds {
        if let Some(a) = s.artists.first().filter(|a| !a.name.trim().is_empty())
            && seen.insert(a.name.trim().to_lowercase())
        {
            names.push(a.name.trim().to_string());
        }
    }
    let mut counts: HashMap<String, (usize, usize, String)> = HashMap::new();
    for (i, s) in songs.iter().enumerate() {
        if let Some(a) = s.artists.first().filter(|a| !a.name.trim().is_empty()) {
            let e = counts.entry(a.name.trim().to_lowercase()).or_insert((0, i, a.name.trim().to_string()));
            e.0 += 1;
        }
    }
    let mut by_count: Vec<(String, (usize, usize, String))> = counts.into_iter().collect();
    by_count.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    let distinct = by_count.len().max(names.len());
    for (k, (_, _, name)) in by_count {
        if seen.insert(k) {
            names.push(name);
        }
    }
    names.truncate(n);
    (names.clone(), distinct > names.len())
}

/// Finish a mix: clean (playable, not disliked, deduped), rerank by taste,
/// blend in `own`, cap at `len`.
fn assemble(profile: &TasteProfile, fresh: Vec<Song>, own: Vec<Song>, len: usize, used: &HashSet<String>) -> Vec<Song> {
    let fresh = super::rerank(profile, clean(profile, fresh, used));
    let mut out = clean(profile, blend(fresh, own, OWN_EVERY), used);
    out.truncate(len);
    out
}

/// Daily Mix 1–3 from the seeds' radios. Marks the songs taken in `used`.
pub fn build_daily(profile: &TasteProfile, seeds: &[Song], radios: &HashMap<String, Vec<Song>>, used: &mut HashSet<String>, day: u64) -> Vec<Mix> {
    let live: Vec<(&Song, &Vec<Song>)> = seeds.iter().filter_map(|s| radio_of(s, radios).map(|r| (s, r))).collect();
    let sets: Vec<HashSet<String>> = live.iter().map(|(s, r)| artist_set(s, r)).collect();
    let mut mixes = Vec::new();
    for group in group_seeds(&sets, DAILY_MIXES) {
        let lists: Vec<Vec<Song>> = group.iter().map(|&i| live[i].1.clone()).collect();
        let group_artists: HashSet<String> = group.iter().flat_map(|&i| sets[i].iter().cloned()).collect();
        let own_n = DAILY_LEN * 3 / 10;
        let own = own_songs(profile, &group_artists, own_n, used, day + mixes.len() as u64);
        let songs = assemble(profile, interleave(&lists), own, DAILY_LEN, used);
        if songs.len() < DAILY_MIN {
            continue;
        }
        used.extend(songs.iter().map(|s| s.id.clone()));
        let seed_refs: Vec<&Song> = group.iter().map(|&i| live[i].0).collect();
        let (artists, more) = main_artists(&seed_refs, &songs, 3);
        let n = mixes.len() + 1;
        mixes.push(Mix { id: format!("daily{n}"), kind: MixKind::Daily(n as u8), artists, more, songs: Arc::new(songs) });
    }
    mixes
}

/// Songs the user never played / liked / saved, ≤ `per_artist` per artist.
pub fn discover_filter(candidates: Vec<Song>, profile: &TasteProfile, known: &Known, used: &HashSet<String>, per_artist: usize, limit: usize) -> Vec<Song> {
    let mut per: HashMap<String, usize> = HashMap::new();
    let mut d = Dedupe::default();
    let mut out = Vec::new();
    for s in candidates {
        if out.len() >= limit {
            break;
        }
        if s.video_id.is_none()
            || profile.is_disliked(&s.id)
            || profile.song_index.contains_key(&s.id)
            || used.contains(&s.id)
            || known.knows(&s)
        {
            continue;
        }
        let a = artist_key(&s);
        let c = per.entry(a).or_insert(0);
        if *c >= per_artist || !d.admit(&s) {
            continue;
        }
        *c += 1;
        out.push(s);
    }
    out
}

/// "Discover for you": unknown songs from the hop radios first (further from
/// what the user plays), then the seeds' radios.
pub fn build_discover(
    profile: &TasteProfile,
    seeds: &[Song],
    radios: &HashMap<String, Vec<Song>>,
    hop_radios: &[Vec<Song>],
    known: &Known,
    used: &HashSet<String>,
) -> Option<Mix> {
    let mut pool = interleave(hop_radios);
    pool.extend(interleave(&radio_lists(seeds, radios)));
    let picked = discover_filter(pool, profile, known, used, DISCOVER_PER_ARTIST, DISCOVER_LEN * 2);
    let mut songs = super::rerank(profile, picked);
    songs.truncate(DISCOVER_LEN);
    if songs.len() < DISCOVER_MIN {
        return None;
    }
    let (artists, more) = main_artists(&[], &songs, 3);
    Some(Mix { id: "discover".into(), kind: MixKind::Discover, artists, more, songs: Arc::new(songs) })
}

/// Seeds of the daypart mix: the user's top song of each top artist of `part`.
pub fn daypart_seeds(profile: &TasteProfile, part: Daypart, n: usize) -> Vec<Song> {
    let mut out = Vec::new();
    for a in profile.by_daypart[part.index()].iter() {
        if out.len() >= n {
            break;
        }
        let key = a.name.trim().to_lowercase();
        if let Some(s) = profile
            .songs
            .iter()
            .find(|s| s.score > 0.0 && s.song.video_id.is_some() && !profile.is_disliked(&s.song.id) && artist_key(&s.song) == key)
        {
            out.push(s.song.clone());
        }
    }
    out
}

pub fn build_daypart(profile: &TasteProfile, part: Daypart, seeds: &[Song], radios: &HashMap<String, Vec<Song>>, day: u64) -> Option<Mix> {
    let lists = radio_lists(seeds, radios);
    if lists.is_empty() {
        return None;
    }
    let artists: HashSet<String> = profile.by_daypart[part.index()].iter().take(6).map(|a| a.name.trim().to_lowercase()).collect();
    let none = HashSet::new();
    let own = own_songs(profile, &artists, DAYPART_LEN * 3 / 10, &none, day.wrapping_add(7));
    let songs = assemble(profile, interleave(&lists), own, DAYPART_LEN, &none);
    if songs.len() < DAYPART_MIN {
        return None;
    }
    let seed_refs: Vec<&Song> = seeds.iter().collect();
    let (names, more) = main_artists(&seed_refs, &songs, 3);
    Some(Mix { id: "daypart".into(), kind: MixKind::Daypart(part.index() as u8), artists: names, more, songs: Arc::new(songs) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::{ArtistTaste, SongTaste};
    use mochy_core::models::ArtistRef;

    fn song(id: &str, title: &str, artist: &str) -> Song {
        Song { artists: vec![ArtistRef::new(artist)], ..Song::remote(id, title) }
    }

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn profile(songs: &[(&str, &str, &str, f32)]) -> TasteProfile {
        let mut p = TasteProfile {
            total_events: 10,
            songs: songs
                .iter()
                .map(|(id, t, a, sc)| SongTaste {
                    song: song(id, t, a),
                    score: *sc,
                    plays: 1,
                    completed: 0,
                    skips: 0,
                    liked: false,
                    last_played: 0,
                })
                .collect(),
            ..Default::default()
        };
        p.rebuild_index();
        p
    }

    #[test]
    fn similarity_is_jaccard() {
        assert_eq!(similarity(&set(&["a", "b"]), &set(&["a", "b"])), 1.0);
        assert_eq!(similarity(&set(&["a"]), &set(&["b"])), 0.0);
        assert!((similarity(&set(&["a", "b"]), &set(&["b", "c"])) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(similarity(&set(&[]), &set(&["b"])), 0.0);
    }

    #[test]
    fn groups_seeds_by_radio_overlap() {
        // Seeds 0, 3, 5 are V-pop; 1, 4 are K-pop; 2, 6 are rock.
        let sets = vec![
            set(&["sơn tùng", "soobin", "min"]),
            set(&["bts", "twice", "iu"]),
            set(&["muse", "queens", "foo"]),
            set(&["soobin", "min", "đen"]),
            set(&["twice", "iu", "blackpink"]),
            set(&["sơn tùng", "đen", "min"]),
            set(&["foo", "muse", "nirvana"]),
        ];
        let g = group_seeds(&sets, 3);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0][0], 0, "first group anchored on the best seed");
        let find = |i: usize| g.iter().position(|grp| grp.contains(&i)).unwrap();
        assert_eq!(find(0), find(3));
        assert_eq!(find(0), find(5));
        assert_eq!(find(1), find(4));
        assert_eq!(find(2), find(6));
        assert_ne!(find(0), find(1));
        assert_ne!(find(1), find(2));
        // Every seed exactly once.
        let mut all: Vec<usize> = g.concat();
        all.sort();
        assert_eq!(all, (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn groups_with_few_seeds() {
        assert!(group_seeds(&[], 3).is_empty());
        assert_eq!(group_seeds(&[set(&["a"])], 3), vec![vec![0]]);
        let g = group_seeds(&[set(&["a"]), set(&["a"]), set(&["a"]), set(&["a"])], 3);
        assert_eq!(g.len(), 3);
        assert!(g.iter().all(|x| x.len() <= 2));
    }

    #[test]
    fn interleaves_round_robin() {
        let a = vec![song("a1", "A1", "x"), song("a2", "A2", "x"), song("a3", "A3", "x")];
        let b = vec![song("b1", "B1", "y")];
        let ids: Vec<String> = interleave(&[a, b]).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["a1", "b1", "a2", "a3"]);
        assert!(interleave(&[]).is_empty());
    }

    #[test]
    fn blends_own_songs_every_third() {
        let fresh: Vec<Song> = (0..6).map(|i| song(&format!("f{i}"), &format!("F{i}"), "x")).collect();
        let own: Vec<Song> = (0..2).map(|i| song(&format!("o{i}"), &format!("O{i}"), "y")).collect();
        let ids: Vec<String> = blend(fresh, own, 3).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["o0", "f0", "f1", "o1", "f2", "f3", "f4", "f5"]);
    }

    #[test]
    fn dedupes_by_id_and_title() {
        let mut d = Dedupe::default();
        assert!(d.admit(&song("a", "Nơi Này Có Anh", "Sơn Tùng")));
        assert!(!d.admit(&song("a", "Other", "x")), "same id");
        assert!(!d.admit(&song("b", "NƠI NÀY CÓ ANH | OFFICIAL MV", "Sơn Tùng")), "same title");
        assert!(d.admit(&song("c", "Chúng Ta Của Hiện Tại", "Sơn Tùng")));
    }

    #[test]
    fn clean_drops_disliked_and_unplayable() {
        let mut p = profile(&[]);
        p.disliked.insert("bad".into());
        let local = Song { video_id: None, ..song("loc", "Local", "x") };
        let out = clean(&p, vec![song("ok", "Ok", "x"), song("bad", "Bad", "x"), local, song("ok", "Ok", "x")], &HashSet::new());
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["ok"]);
    }

    #[test]
    fn discover_keeps_only_unknown_songs() {
        let p = profile(&[("played", "Played", "a", 5.0)]);
        let mut known = Known::default();
        known.add(&song("liked", "Liked Song", "b"));
        let used: HashSet<String> = ["used".to_string()].into();
        let candidates = vec![
            song("played", "Played", "a"),
            song("liked", "Liked Song", "b"),
            song("liked2", "Liked Song (Lofi)", "b"),
            song("used", "Used", "c"),
            song("n1", "New 1", "d"),
            song("n2", "New 2", "d"),
            song("n3", "New 3", "d"),
            song("n4", "New 4", "e"),
        ];
        let out = discover_filter(candidates, &p, &known, &used, 2, 10);
        assert_eq!(out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["n1", "n2", "n4"]);
        let capped = discover_filter(vec![song("x1", "X1", "f"), song("x2", "X2", "g")], &p, &known, &used, 2, 1);
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn daily_mixes_from_radios() {
        let p = profile(&[("s1", "Seed 1", "Alpha", 9.0), ("s2", "Seed 2", "Beta", 8.0), ("own1", "Own 1", "Alpha", 7.0)]);
        let seeds = vec![song("s1", "Seed 1", "Alpha"), song("s2", "Seed 2", "Beta")];
        let mut radios = HashMap::new();
        radios.insert("s1".to_string(), (0..30).map(|i| song(&format!("a{i}"), &format!("A {i}"), "Alpha")).collect::<Vec<_>>());
        radios.insert("s2".to_string(), (0..30).map(|i| song(&format!("b{i}"), &format!("B {i}"), "Beta")).collect::<Vec<_>>());
        let mut used = HashSet::new();
        let mixes = build_daily(&p, &seeds, &radios, &mut used, 1);
        assert_eq!(mixes.len(), 2);
        assert_eq!(mixes[0].id, "daily1");
        assert_eq!(mixes[0].artists[0], "Alpha");
        assert!(mixes[0].songs.iter().any(|s| s.id == "own1"), "own songs mixed in");
        assert!(mixes[1].songs.iter().all(|s| !mixes[0].songs.iter().any(|t| t.id == s.id)), "no song in two mixes");
        assert!(mixes.iter().all(|m| m.songs.len() <= DAILY_LEN));
    }

    #[test]
    fn daypart_seeds_use_daypart_artists() {
        let mut p = profile(&[("x", "X", "Gamma", 5.0), ("y", "Y", "Delta", 4.0), ("z", "Z", "Delta", 3.0)]);
        p.by_daypart[Daypart::Night.index()] = vec![ArtistTaste { name: "Delta".into(), ..Default::default() }];
        let s = daypart_seeds(&p, Daypart::Night, 3);
        assert_eq!(s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["y"]);
        assert!(daypart_seeds(&p, Daypart::Morning, 3).is_empty());
    }

    #[test]
    fn cache_freshness() {
        let now = 1_000 * 86_400_000i64;
        let mix = Mix { id: "daily1".into(), kind: MixKind::Daily(1), artists: vec![], more: false, songs: Arc::new(vec![]) };
        let part = Daypart::from_hour(local_hour(now, 0)).index() as u8;
        let fresh = MixSet { generated_at: now - 3_600_000, daypart: part, utc_offset_secs: 0, mixes: vec![mix.clone()] };
        assert_eq!(need(None, now, false, 0), Need::All);
        assert_eq!(need(Some(&fresh), now, false, 0), Need::Nothing);
        assert_eq!(need(Some(&fresh), now, true, now), Need::All, "refresh action");
        let old = MixSet { generated_at: now - STALE_MS - 1, ..fresh.clone() };
        assert_eq!(need(Some(&old), now, false, 0), Need::All);
        assert_eq!(need(Some(&old), now, false, now - 60_000), Need::Nothing, "backoff after an attempt");
        let other_part = MixSet { daypart: (part + 1) % 4, ..fresh.clone() };
        assert_eq!(need(Some(&other_part), now, false, 0), Need::Daypart);
        let empty = MixSet { mixes: vec![], ..fresh.clone() };
        assert_eq!(need(Some(&empty), now, false, 0), Need::All);
    }

    #[test]
    fn cache_round_trip() {
        let set = MixSet {
            generated_at: 5,
            daypart: 2,
            utc_offset_secs: 25_200,
            mixes: vec![Mix {
                id: "discover".into(),
                kind: MixKind::Discover,
                artists: vec!["A".into()],
                more: true,
                songs: Arc::new(vec![song("v", "V", "A")]),
            }],
        };
        let json = serde_json::to_vec(&to_cache(&set)).unwrap();
        let back = from_cache(serde_json::from_slice(&json).unwrap()).unwrap();
        assert_eq!(back, set);
    }
}
