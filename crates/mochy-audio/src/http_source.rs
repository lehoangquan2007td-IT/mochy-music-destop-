//! `HttpByteSource`: blocking HTTPS (rustls) `Read + Seek` + symphonia
//! `MediaSource` over a remote media URL (spec §2.6).
//!
//! The file is fetched in ranged chunks (`Range: bytes=a-b`, 512 KiB each —
//! googlevideo throttles or 403s large unranged requests) by two background
//! workers that prefetch ahead of the read position and keep what they got
//! in memory, so seeking back is free and seeking ahead issues a new ranged
//! fetch for the target chunk first. Data is readable while a chunk is still
//! arriving, so playback starts after the first few KiB. Transient failures
//! (timeouts, resets, 429, 5xx) are retried with backoff and resume where the
//! chunk left off. Servers that ignore `Range` (plain 200) are streamed
//! sequentially into the same cache.
//!
//! Must not be used from inside a tokio runtime (reqwest blocking).

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Bytes per ranged request.
pub const CHUNK_SIZE: u64 = 512 * 1024;
/// How many chunks to prefetch ahead of the reader.
const PREFETCH_AHEAD: u64 = 24;
const WORKERS: usize = 3;
const MAX_CACHE_BYTES: u64 = 384 * 1024 * 1024;
const MAX_ATTEMPTS: u32 = 6;
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);
const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";

#[derive(Default)]
struct Chunk {
    data: Vec<u8>,
    complete: bool,
}

#[derive(Default)]
struct State {
    len: Option<u64>,
    chunks: HashMap<u64, Chunk>,
    inflight: HashSet<u64>,
    want: Option<u64>,
    /// Seek landed near a chunk start: decoders often reach back a little.
    want_prev: Option<u64>,
    read_chunk: u64,
    fatal: Option<String>,
    closed: bool,
    /// `None` until the first response tells us whether ranges work.
    ranged: Option<bool>,
    /// Non-ranged mode: whole body received.
    stream_done: bool,
    cached_bytes: u64,
    content_type: Option<String>,
}

struct Inner {
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::blocking::Client,
    state: Mutex<State>,
    cv: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Remote media as a seekable byte stream.
pub struct HttpByteSource {
    inner: Arc<Inner>,
    pos: u64,
    cancel: Arc<AtomicBool>,
}

enum FetchError {
    Transient(String),
    Fatal(String),
    Closed,
    /// A prefetch gave way to a chunk the reader is blocked on (partial
    /// data is kept and resumed later).
    Preempted,
}

impl HttpByteSource {
    /// Open `url` (sends `headers` on every request) and wait for the first
    /// bytes. Blocks; see [`HttpByteSource::open_cancellable`].
    pub fn new(url: String, headers: Vec<(String, String)>) -> io::Result<Self> {
        Self::open_cancellable(url, headers, Arc::new(AtomicBool::new(false)))
    }

    /// Like [`new`](Self::new) but gives up (and later reads fail) as soon as
    /// `cancel` is set — used by the engine when a track is replaced.
    pub fn open_cancellable(url: String, headers: Vec<(String, String)>, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unsupported URL: {url}")));
        }
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .tcp_nodelay(true)
            .build()
            .map_err(|e| io::Error::other(format!("http client: {e}")))?;
        let inner = Arc::new(Inner {
            url,
            headers,
            client,
            state: Mutex::new(State { want: Some(0), ..Default::default() }),
            cv: Condvar::new(),
        });
        for i in 0..WORKERS {
            let w = inner.clone();
            std::thread::Builder::new()
                .name(format!("mochy-http-{i}"))
                .spawn(move || worker(w))
                .map_err(|e| io::Error::other(format!("spawn: {e}")))?;
        }
        let src = HttpByteSource { inner, pos: 0, cancel };
        // wait for the first bytes (or failure)
        let deadline = Instant::now() + OPEN_TIMEOUT;
        let mut st = src.inner.lock();
        loop {
            if let Some(e) = &st.fatal {
                return Err(io::Error::other(e.clone()));
            }
            let ready = st.chunks.get(&0).is_some_and(|c| !c.data.is_empty() || c.complete);
            if ready && st.ranged.is_some() {
                break;
            }
            if src.cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("cancelled"));
            }
            if Instant::now() > deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for the stream"));
            }
            st = src.inner.cv.wait_timeout(st, Duration::from_millis(50)).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
        drop(st);
        Ok(src)
    }

    /// Total size when known (Content-Range / Content-Length).
    pub fn len(&self) -> Option<u64> {
        self.inner.lock().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// `Content-Type` of the first response (for the probe hint).
    pub fn content_type(&self) -> Option<String> {
        self.inner.lock().content_type.clone()
    }

    /// Flag that aborts blocked reads when set.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Bytes currently cached in memory.
    pub fn cached_bytes(&self) -> u64 {
        self.inner.lock().cached_bytes
    }
}

impl Drop for HttpByteSource {
    fn drop(&mut self) {
        let mut st = self.inner.lock();
        st.closed = true;
        drop(st);
        self.inner.cv.notify_all();
    }
}

impl Read for HttpByteSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let idx = self.pos / CHUNK_SIZE;
        let off = (self.pos % CHUNK_SIZE) as usize;
        let mut st = self.inner.lock();
        loop {
            if st.len.is_some_and(|len| self.pos >= len) {
                return Ok(0);
            }
            if st.read_chunk != idx {
                st.read_chunk = idx;
                self.inner.cv.notify_all();
            }
            if let Some(c) = st.chunks.get(&idx) {
                if c.data.len() > off {
                    let n = buf.len().min(c.data.len() - off);
                    buf[..n].copy_from_slice(&c.data[off..off + n]);
                    self.pos += n as u64;
                    return Ok(n);
                }
                if c.complete {
                    return Ok(0); // short final chunk: end of stream
                }
            }
            if st.ranged == Some(false) && st.stream_done {
                return Ok(0);
            }
            if let Some(e) = &st.fatal {
                return Err(io::Error::other(e.clone()));
            }
            if self.cancel.load(Ordering::Relaxed) {
                return Err(io::Error::other("cancelled"));
            }
            if st.want != Some(idx) {
                st.want = Some(idx);
                self.inner.cv.notify_all();
            }
            st = self.inner.cv.wait_timeout(st, Duration::from_millis(50)).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
        }
    }
}

impl Seek for HttpByteSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(o) => o as i128,
            SeekFrom::Current(o) => self.pos as i128 + o as i128,
            SeekFrom::End(o) => match self.len() {
                Some(len) => len as i128 + o as i128,
                None => return Err(io::Error::new(io::ErrorKind::Unsupported, "length unknown")),
            },
        };
        if new < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before start"));
        }
        self.pos = new as u64;
        let idx = self.pos / CHUNK_SIZE;
        let mut st = self.inner.lock();
        let have = st.chunks.contains_key(&idx);
        if !have && st.want != Some(idx) {
            st.want = Some(idx);
            st.read_chunk = idx;
            st.want_prev = (idx > 0 && self.pos % CHUNK_SIZE < 64 * 1024).then(|| idx - 1);
            drop(st);
            self.inner.cv.notify_all();
        }
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for HttpByteSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.len()
    }
}

// ---------------------------------------------------------------------------
// workers
// ---------------------------------------------------------------------------

fn chunk_count(len: u64) -> u64 {
    len.div_ceil(CHUNK_SIZE)
}

fn have(st: &State, idx: u64) -> bool {
    st.chunks.get(&idx).is_some_and(|c| c.complete)
}

fn pick(st: &State) -> Option<u64> {
    if st.fatal.is_some() || st.ranged == Some(false) {
        return None;
    }
    let Some(len) = st.len else {
        // nothing is known yet: only the first request goes out
        return (!st.inflight.contains(&0) && !have(st, 0) && st.ranged.is_none()).then_some(0);
    };
    let n = chunk_count(len);
    let free = |i: u64| i < n && !have(st, i) && !st.inflight.contains(&i);
    if let Some(w) = st.want.filter(|&w| free(w)) {
        return Some(w);
    }
    if let Some(w) = st.want_prev.filter(|&w| free(w)) {
        return Some(w);
    }
    if st.cached_bytes >= MAX_CACHE_BYTES {
        return None;
    }
    (st.read_chunk..(st.read_chunk + PREFETCH_AHEAD).min(n)).find(|&i| free(i))
}

fn worker(inner: Arc<Inner>) {
    loop {
        let task = {
            let mut st = inner.lock();
            loop {
                if st.closed {
                    return;
                }
                if let Some(t) = pick(&st) {
                    st.inflight.insert(t);
                    break t;
                }
                st = inner.cv.wait_timeout(st, Duration::from_millis(250)).map(|r| r.0).unwrap_or_else(|e| e.into_inner().0);
            }
        };
        let mut attempt = 0;
        let result = loop {
            match fetch_chunk(&inner, task) {
                Ok(()) => break Ok(()),
                Err(FetchError::Closed) | Err(FetchError::Preempted) => break Ok(()),
                Err(FetchError::Fatal(e)) => break Err(e),
                Err(FetchError::Transient(e)) => {
                    attempt += 1;
                    if attempt >= MAX_ATTEMPTS {
                        break Err(e);
                    }
                    let wait = Duration::from_millis(250 << attempt.min(5));
                    let until = Instant::now() + wait;
                    while Instant::now() < until {
                        if inner.lock().closed {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
            }
        };
        let mut st = inner.lock();
        st.inflight.remove(&task);
        if let Err(e) = result {
            st.fatal.get_or_insert(e);
        }
        evict(&mut st);
        drop(st);
        inner.cv.notify_all();
    }
}

/// Keep memory bounded: drop complete chunks farthest from the reader.
fn evict(st: &mut State) {
    while st.cached_bytes > MAX_CACHE_BYTES {
        let rc = st.read_chunk;
        let victim = st
            .chunks
            .iter()
            .filter(|(i, c)| c.complete && !st.inflight.contains(i) && i.abs_diff(rc) > 2)
            .max_by_key(|(i, _)| i.abs_diff(rc))
            .map(|(i, _)| *i);
        match victim {
            Some(v) => {
                if let Some(c) = st.chunks.remove(&v) {
                    st.cached_bytes -= c.data.len() as u64;
                }
            }
            None => break,
        }
    }
}

fn parse_total(content_range: &str) -> Option<u64> {
    // "bytes 0-524287/3871234"
    content_range.rsplit('/').next()?.trim().parse().ok()
}

fn fetch_chunk(inner: &Inner, idx: u64) -> Result<(), FetchError> {
    let start = idx * CHUNK_SIZE;
    let (resume, len) = {
        let st = inner.lock();
        if st.closed {
            return Err(FetchError::Closed);
        }
        (st.chunks.get(&idx).map_or(0, |c| c.data.len() as u64), st.len)
    };
    let end_excl = match len {
        Some(l) => (start + CHUNK_SIZE).min(l),
        None => start + CHUNK_SIZE,
    };
    if start + resume >= end_excl && len.is_some() {
        let mut st = inner.lock();
        st.chunks.entry(idx).or_default().complete = true;
        return Ok(());
    }
    let mut req = inner.client.get(&inner.url);
    let mut has_ua = false;
    for (k, v) in &inner.headers {
        let lk = k.to_ascii_lowercase();
        if lk == "range" || lk == "host" || lk == "content-length" {
            continue;
        }
        has_ua |= lk == "user-agent";
        req = req.header(k.as_str(), v.as_str());
    }
    if !has_ua {
        req = req.header("User-Agent", DEFAULT_UA);
    }
    req = req
        .header("Accept-Encoding", "identity")
        .header("Range", format!("bytes={}-{}", start + resume, end_excl - 1));
    let mut resp = req.send().map_err(|e| FetchError::Transient(format!("request failed: {e}")))?;
    let status = resp.status().as_u16();
    let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let content_type = header("content-type");
    match status {
        206 => {
            let total = header("content-range").as_deref().and_then(parse_total);
            {
                let mut st = inner.lock();
                st.ranged = Some(true);
                if st.len.is_none() {
                    st.len = total;
                }
                if st.content_type.is_none() {
                    st.content_type = content_type;
                }
            }
            inner.cv.notify_all();
            let expect = {
                let st = inner.lock();
                st.len.map_or(CHUNK_SIZE, |l| (start + CHUNK_SIZE).min(l) - start)
            };
            stream_into_chunk(inner, idx, &mut resp, expect)
        }
        200 => {
            // Range ignored: stream the whole body sequentially into the cache.
            let total = header("content-length").and_then(|v| v.parse::<u64>().ok());
            {
                let mut st = inner.lock();
                st.ranged = Some(false);
                if st.len.is_none() {
                    st.len = total;
                }
                if st.content_type.is_none() {
                    st.content_type = content_type;
                }
                // whatever was partially there is re-received from byte 0
                for c in st.chunks.values_mut() {
                    c.data.clear();
                    c.complete = false;
                }
                st.cached_bytes = 0;
            }
            inner.cv.notify_all();
            stream_whole(inner, &mut resp)
        }
        416 => {
            let mut st = inner.lock();
            if st.len.is_none() {
                st.len = header("content-range").as_deref().and_then(parse_total).or(Some(start));
            }
            st.ranged.get_or_insert(true);
            st.chunks.entry(idx).or_default().complete = true;
            Ok(())
        }
        408 | 425 | 429 | 500..=599 => Err(FetchError::Transient(format!("HTTP {status}"))),
        _ => Err(FetchError::Fatal(format!("HTTP {status} for media URL"))),
    }
}

fn stream_into_chunk(inner: &Inner, idx: u64, resp: &mut reqwest::blocking::Response, expect: u64) -> Result<(), FetchError> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match resp.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FetchError::Transient(format!("read failed: {e}"))),
        };
        let mut st = inner.lock();
        if st.closed {
            return Err(FetchError::Closed);
        }
        let len_known = st.len.is_some();
        // the reader is stuck on another chunk nobody is fetching: make room
        let preempt = st.want.is_some_and(|w| w != idx && !have(&st, w) && !st.inflight.contains(&w));
        let c = st.chunks.entry(idx).or_default();
        if n == 0 {
            if (c.data.len() as u64) < expect && len_known {
                return Err(FetchError::Transient("connection closed early".into()));
            }
            c.complete = true;
            drop(st);
            inner.cv.notify_all();
            return Ok(());
        }
        let room = (expect as usize).saturating_sub(c.data.len());
        let take = n.min(room);
        c.data.extend_from_slice(&buf[..take]);
        let full = c.data.len() as u64 >= expect;
        if full {
            c.complete = true;
        }
        st.cached_bytes += take as u64;
        drop(st);
        inner.cv.notify_all();
        if full {
            return Ok(());
        }
        if preempt {
            return Err(FetchError::Preempted);
        }
    }
}

fn stream_whole(inner: &Inner, resp: &mut reqwest::blocking::Response) -> Result<(), FetchError> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut offset: u64 = 0;
    loop {
        let n = match resp.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                // can't resume a non-ranged stream
                return Err(FetchError::Fatal(format!("stream read failed: {e}")));
            }
        };
        let mut st = inner.lock();
        if st.closed {
            return Err(FetchError::Closed);
        }
        if n == 0 {
            if let Some(c) = st.chunks.get_mut(&(offset / CHUNK_SIZE)) {
                c.complete = true;
            }
            st.len = Some(offset);
            st.stream_done = true;
            drop(st);
            inner.cv.notify_all();
            return Ok(());
        }
        let mut data = &buf[..n];
        while !data.is_empty() {
            let idx = offset / CHUNK_SIZE;
            let off = (offset % CHUNK_SIZE) as usize;
            let take = data.len().min(CHUNK_SIZE as usize - off);
            let c = st.chunks.entry(idx).or_default();
            c.data.extend_from_slice(&data[..take]);
            if c.data.len() as u64 == CHUNK_SIZE {
                c.complete = true;
            }
            st.cached_bytes += take as u64;
            offset += take as u64;
            data = &data[take..];
        }
        drop(st);
        inner.cv.notify_all();
    }
}
