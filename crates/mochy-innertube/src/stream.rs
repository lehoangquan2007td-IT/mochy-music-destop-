//! Stream acquisition: player requests, client walk and URL validation
//! (port of `YTPlayerUtils.resolvePlaybackData` without PoToken/JS).

use std::time::{Duration, Instant};

use mochy_core::settings::AudioQuality;
use serde_json::{Value, json};

use crate::clients::{self, ClientSpec};
use crate::{BOT_CHECK_REST, CodecPreference, StreamResult, YtmClient, YtmError, YtmResult, parse};

/// How many candidates of one client response are probed before moving on.
const CANDIDATES_PROBED_PER_CLIENT: usize = 2;

impl YtmClient {
    /// The visitor id `client` should use: its own (minted once, cached) for
    /// `own_visitor_id` clients, else the web one.
    async fn visitor_for(&self, client: &ClientSpec, refresh: bool) -> Option<String> {
        if !client.own_visitor_id {
            let v = self.visitor_data();
            return (!v.is_empty()).then_some(v);
        }
        if !refresh
            && let Some(v) = self.inner.own_visitors.lock().expect("visitors lock").get(client.name)
        {
            return Some(v.clone());
        }
        match self.mint_client_visitor(client).await {
            Ok(v) => {
                self.inner.own_visitors.lock().expect("visitors lock").insert(client.name, v.clone());
                Some(v)
            }
            Err(_) => None,
        }
    }

    /// Raw `player` response for `video_id` as `client`.
    pub async fn player(&self, client: &ClientSpec, video_id: &str) -> YtmResult<Value> {
        self.player_inner(client, video_id, false).await
    }

    async fn player_inner(&self, client: &ClientSpec, video_id: &str, fresh_visitor: bool) -> YtmResult<Value> {
        let visitor = self.visitor_for(client, fresh_visitor).await;
        let mut ctx = self.context(client, visitor.as_deref());
        if client.is_embedded {
            ctx["thirdParty"] = json!({ "embedUrl": format!("https://www.youtube.com/watch?v={video_id}") });
        }
        let body = json!({
            "context": ctx,
            "videoId": video_id,
            "contentCheckOk": true,
            "racyCheckOk": true,
        });
        self.post_api(client, "player", body, visitor.as_deref(), Duration::from_millis(clients::BACKEND_TIMEOUT_MS))
            .await
    }

    /// Ranged GET of one byte at the end of the file (or at 1 MiB when the
    /// length is unknown). 200/206 = served; 416 with unknown length = the
    /// file is just shorter than the probe; 403 = cut off / refused.
    pub async fn validate_stream_url(&self, url: &str, headers: &[(String, String)], content_length: Option<u64>) -> bool {
        let offset = content_length
            .filter(|l| *l > 1)
            .map(|l| l - 1)
            .unwrap_or(clients::UNTOKENED_WINDOW_PROBE_BYTE);
        let mut req = self
            .http
            .get(url)
            .header("Range", format!("bytes={offset}-{offset}"))
            .timeout(Duration::from_secs(10));
        let mut has_ua = false;
        for (k, v) in headers {
            has_ua |= k.eq_ignore_ascii_case("user-agent");
            req = req.header(k.as_str(), v.as_str());
        }
        if !has_ua {
            req = req.header("User-Agent", clients::WEB_UA);
        }
        for attempt in 0..2 {
            match req.try_clone().expect("GET without body is clonable").send().await {
                Ok(resp) => {
                    let code = resp.status().as_u16();
                    return resp.status().is_success() || (code == 416 && content_length.is_none());
                }
                Err(e) if attempt == 0 && (e.is_timeout() || e.is_connect()) => continue,
                Err(_) => return false,
            }
        }
        false
    }

    fn visionos_resting(&self) -> bool {
        let mut rest = self.inner.visionos_resting_until.lock().expect("rest lock");
        match *rest {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                *rest = None;
                false
            }
            None => false,
        }
    }

    /// Like [`YtmClient::resolve_stream`] with an explicit codec preference
    /// (`PreferOpus` for an engine that decodes Opus).
    pub async fn resolve_stream_with(
        &self,
        video_id: &str,
        quality: AudioQuality,
        pref: CodecPreference,
    ) -> YtmResult<StreamResult> {
        self.resolve_stream_walk(video_id, quality, pref, clients::STREAM_WALK).await
    }

    /// Resolve using only the given clients, in order (diagnostics / tests).
    pub async fn resolve_stream_walk(
        &self,
        video_id: &str,
        quality: AudioQuality,
        pref: CodecPreference,
        walk: &[ClientSpec],
    ) -> YtmResult<StreamResult> {
        let mut failures: Vec<String> = Vec::new();
        let mut premium_only = false;
        let mut any_served = false;

        for client in walk {
            if client.login_required || client.use_web_po_tokens {
                failures.push(format!("{}: skipped (needs login/PoToken)", client.name));
                continue;
            }
            if client.own_visitor_id && self.visionos_resting() {
                failures.push(format!("{}: resting after bot check", client.name));
                continue;
            }
            let mut resp = match self.player(client, video_id).await {
                Ok(r) => r,
                Err(YtmError::RateLimited) => return Err(YtmError::RateLimited),
                Err(e) => {
                    failures.push(format!("{}: {e}", client.name));
                    continue;
                }
            };
            // A bot check burns the visitor id, not the client: one fresh id
            // and one more ask; twice in a row ⇒ rest the client 30 min.
            if client.own_visitor_id && parse::is_bot_check(&resp) {
                if let Ok(r) = self.player_inner(client, video_id, true).await {
                    resp = r;
                }
                if parse::is_bot_check(&resp) {
                    *self.inner.visionos_resting_until.lock().expect("rest lock") = Some(Instant::now() + BOT_CHECK_REST);
                }
            }
            let (status, reason) = parse::playability(&resp);
            if status != "OK" {
                premium_only |= parse::is_premium_only(&resp);
                failures.push(if reason.is_empty() {
                    format!("{}: {status}", client.name)
                } else {
                    format!("{}: {status} ({reason})", client.name)
                });
                continue;
            }
            any_served = true;
            let candidates = parse::audio_candidates(&resp, quality, pref);
            if candidates.is_empty() {
                failures.push(format!("{}: no direct audio url", client.name));
                continue;
            }
            let headers = client.stream_headers();
            for i in 0..candidates.len().min(CANDIDATES_PROBED_PER_CLIENT) {
                let c = &candidates[i];
                if self.validate_stream_url(&c.url, &headers, c.content_length).await {
                    let mut rest = candidates.clone();
                    let best = rest.remove(i);
                    let expires = parse::stream_expiry(&resp, &best.url);
                    return Ok(StreamResult::from_candidate(best, rest, headers, client.name.to_string(), expires));
                }
            }
            failures.push(format!("{}: stream cut off", client.name));
        }

        if !any_served && premium_only {
            return Err(YtmError::PremiumOnly(video_id.to_string()));
        }
        Err(YtmError::NoStream(format!("{video_id} [{}]", failures.join(" | "))))
    }
}
