//! Ranged HTTPS download of a media URL into a file.
//!
//! Like the player's `HttpByteSource`, requests go out in ranged chunks
//! (`Range: bytes=a-b`) because googlevideo throttles or refuses large
//! unranged requests; each chunk is retried with backoff on transient
//! failures and resumes where it stopped. A server that ignores `Range`
//! (plain 200) is streamed from the start. Upstream (Android) always
//! downloads straight from the network, never through the player cache,
//! so two formats can never get glued together — same here.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncWriteExt;

/// Bytes per ranged request.
pub const CHUNK: u64 = 1024 * 1024;
const MAX_ATTEMPTS: u32 = 6;
pub const DEFAULT_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36";

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("cancelled")]
    Cancelled,
    /// 403/410: the URL stopped working (expired or cut off) — re-resolve.
    #[error("stream refused (HTTP {0})")]
    Refused(u16),
    #[error("HTTP {0}")]
    Http(u16),
    #[error("network: {0}")]
    Network(String),
    #[error("disk: {0}")]
    Io(#[from] std::io::Error),
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(4)
        .build()
        .unwrap_or_default()
}

fn content_range_total(v: &str) -> Option<u64> {
    v.rsplit('/').next()?.trim().parse().ok()
}

/// Download `url` into `dest` (created/truncated). `progress(done, total)`
/// is called as bytes arrive. Returns the number of bytes written.
pub async fn fetch_to_file(
    http: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
    dest: &Path,
    expected_len: Option<u64>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, FetchError> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::File::create(dest).await?;
    let mut total = expected_len.filter(|l| *l > 0);
    let mut done: u64 = 0;
    let mut attempts = 0u32;
    progress(0, total);

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        if let Some(t) = total
            && done >= t
        {
            break;
        }
        let end = done + CHUNK - 1;
        let end = total.map(|t| end.min(t - 1)).unwrap_or(end);
        let mut req = http.get(url).header("Range", format!("bytes={done}-{end}"));
        let mut has_ua = false;
        for (k, v) in headers {
            has_ua |= k.eq_ignore_ascii_case("user-agent");
            req = req.header(k.as_str(), v.as_str());
        }
        if !has_ua {
            req = req.header("User-Agent", DEFAULT_UA);
        }

        let result: Result<bool, FetchError> = async {
            let mut resp = req.send().await.map_err(|e| FetchError::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            match status {
                206 => {
                    if let Some(t) = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(content_range_total) {
                        total = Some(t);
                    }
                }
                200 => {
                    // Range ignored: the whole body follows, from byte 0.
                    if done > 0 {
                        file.set_len(0).await?;
                        use tokio::io::AsyncSeekExt;
                        file.seek(std::io::SeekFrom::Start(0)).await?;
                        done = 0;
                    }
                    total = resp.content_length().or(total);
                }
                416 if total.is_none() && done > 0 => return Ok(true), // past the end: finished
                403 | 410 => return Err(FetchError::Refused(status)),
                s if s == 429 || s >= 500 => return Err(FetchError::Network(format!("HTTP {s}"))),
                s => return Err(FetchError::Http(s)),
            }
            let whole_body = status == 200;
            let mut got_any = false;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FetchError::Cancelled);
                }
                match resp.chunk().await {
                    Ok(Some(bytes)) => {
                        file.write_all(&bytes).await?;
                        done += bytes.len() as u64;
                        got_any |= !bytes.is_empty();
                        progress(done, total);
                    }
                    Ok(None) => break,
                    Err(e) => return Err(FetchError::Network(e.to_string())),
                }
            }
            // A short chunk without a known total means the file ended.
            let finished = whole_body || (total.is_none() && (!got_any || done <= end));
            Ok(finished)
        }
        .await;

        match result {
            Ok(true) => break,
            Ok(false) => attempts = 0,
            Err(FetchError::Network(e)) => {
                attempts += 1;
                if attempts >= MAX_ATTEMPTS {
                    return Err(FetchError::Network(e));
                }
                let wait = Duration::from_millis(500 * (1 << (attempts - 1)).min(16));
                tracing_sleep(wait, cancel).await?;
            }
            Err(e) => return Err(e),
        }
    }
    file.flush().await?;
    file.sync_all().await.ok();
    Ok(done)
}

async fn tracing_sleep(d: Duration, cancel: &AtomicBool) -> Result<(), FetchError> {
    let steps = (d.as_millis() / 100).max(1);
    for _ in 0..steps {
        if cancel.load(Ordering::Relaxed) {
            return Err(FetchError::Cancelled);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_range() {
        assert_eq!(content_range_total("bytes 0-1048575/3456789"), Some(3_456_789));
        assert_eq!(content_range_total("bytes 0-10/*"), None);
    }
}
