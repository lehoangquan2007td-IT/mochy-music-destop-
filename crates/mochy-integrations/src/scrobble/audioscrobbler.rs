//! One Audioscrobbler 2.0 server (Last.fm, or Libre.fm's copy of its API).
//! Port of `providers/lastfm/Audioscrobbler.kt`: calls are form POSTs signed
//! with `api_sig = md5(sorted "keyvalue" concat + secret)`, `format=json` is
//! sent but not signed.

use std::collections::BTreeMap;
use std::time::Duration;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A song as a scrobbling service wants it; times in seconds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrobbleTrack {
    pub artist: String,
    pub track: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u32>,
}

/// A signed-in account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub key: String,
}

/// What the service said went wrong, by its own error codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrobbleError {
    pub code: i32,
    pub message: String,
}

impl ScrobbleError {
    /// Not one of the service's codes: the server failed or the network did.
    pub const SERVER_ERROR: i32 = -1;
    /// No network / timeout (desktop addition: Kotlin used IOException).
    pub const NETWORK: i32 = -2;
    const AUTH_FAILED: i32 = 4;
    const INVALID_PARAMETERS: i32 = 6;
    const INVALID_RESOURCE: i32 = 7;
    const INVALID_SESSION: i32 = 9;
    const UNAUTHORIZED_TOKEN: i32 = 14;
    const TEMPORARY: [i32; 5] = [Self::SERVER_ERROR, 8, 11, 16, 29];

    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    /// The account took back access or the session is gone.
    pub fn is_signed_out(&self) -> bool {
        self.code == Self::INVALID_SESSION
    }
    /// Turned down for good (bad song data): retrying won't help.
    pub fn is_rejected(&self) -> bool {
        self.code == Self::INVALID_PARAMETERS || self.code == Self::INVALID_RESOURCE
    }
    pub fn is_temporary(&self) -> bool {
        Self::TEMPORARY.contains(&self.code)
    }
    pub fn is_network(&self) -> bool {
        self.code == Self::NETWORK
    }
    pub fn is_wrong_login(&self) -> bool {
        self.code == Self::AUTH_FAILED
    }
    /// auth.getSession before the user approved the token in the browser.
    pub fn is_unauthorized_token(&self) -> bool {
        self.code == Self::UNAUTHORIZED_TOKEN
    }
}

impl std::fmt::Display for ScrobbleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for ScrobbleError {}

/// The most plays one `track.scrobble` request takes.
pub const MAX_BATCH: usize = 50;
const USER_AGENT: &str = "Mochy";

pub fn md5_hex(text: &str) -> String {
    let digest = Md5::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `md5(concat(sorted key+value) + secret)`.
pub fn signature(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut s = String::new();
    for (k, v) in params {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    md5_hex(&s)
}

/// The full form body for `method`: the signed params, `api_sig`, `format=json`.
pub fn signed_form(
    method: &str,
    api_key: &str,
    secret: &str,
    session_key: Option<&str>,
    params: &[(String, String)],
) -> Vec<(String, String)> {
    let mut signed = BTreeMap::new();
    signed.insert("method".to_string(), method.to_string());
    signed.insert("api_key".to_string(), api_key.to_string());
    if let Some(sk) = session_key {
        signed.insert("sk".to_string(), sk.to_string());
    }
    for (k, v) in params {
        signed.insert(k.clone(), v.clone());
    }
    let sig = signature(&signed, secret);
    let mut out: Vec<(String, String)> = signed.into_iter().collect();
    out.push(("api_sig".into(), sig));
    out.push(("format".into(), "json".into()));
    out
}

/// Request params of one track; `index` gives the `[i]` batch suffix.
pub fn track_params(t: &ScrobbleTrack, index: Option<usize>) -> Vec<(String, String)> {
    let suffix = index.map(|i| format!("[{i}]")).unwrap_or_default();
    let mut p = vec![(format!("artist{suffix}"), t.artist.clone()), (format!("track{suffix}"), t.track.clone())];
    if let Some(a) = &t.album {
        p.push((format!("album{suffix}"), a.clone()));
    }
    if let Some(d) = t.duration_seconds {
        p.push((format!("duration{suffix}"), d.to_string()));
    }
    p
}

/// Read the service's error out of a reply body; Ok(json) if none.
pub fn check_reply(status: u16, body: &str) -> Result<Value, ScrobbleError> {
    let json: Option<Value> = serde_json::from_str(body).ok().filter(Value::is_object);
    if let Some(j) = &json
        && let Some(err) = j.get("error")
    {
        let code = err.as_i64().map(|c| c as i32).or_else(|| err.as_str().and_then(|s| s.parse().ok())).unwrap_or(ScrobbleError::SERVER_ERROR);
        let msg = j.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("Error {code}"));
        return Err(ScrobbleError::new(code, msg));
    }
    match json {
        Some(j) if (200..300).contains(&status) => Ok(j),
        _ => Err(ScrobbleError::new(ScrobbleError::SERVER_ERROR, format!("The server answered {status}"))),
    }
}

#[derive(Clone)]
pub struct Audioscrobbler {
    api_root: String,
    auth_root: String,
    api_key: String,
    secret: String,
    /// Libre.fm only takes the old md5 login token on sign-in.
    signs_in_with_token: bool,
    http: reqwest::Client,
}

impl Audioscrobbler {
    pub const LASTFM_ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
    pub const LASTFM_AUTH: &str = "https://www.last.fm/api/auth/";
    pub const LIBREFM_ROOT: &str = "https://libre.fm/2.0/";
    pub const LIBREFM_AUTH: &str = "https://libre.fm/api/auth/";

    pub fn new(api_root: &str, auth_root: &str, api_key: &str, secret: &str, signs_in_with_token: bool) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            api_root: api_root.into(),
            auth_root: auth_root.into(),
            api_key: api_key.trim().into(),
            secret: secret.trim().into(),
            signs_in_with_token,
            http,
        }
    }

    /// False without an API key and secret, which can't sign anyone in.
    pub fn is_configured(&self) -> bool {
        !self.api_key.is_empty() && !self.secret.is_empty()
    }

    async fn call(&self, method: &str, params: Vec<(String, String)>, session_key: Option<&str>) -> Result<Value, ScrobbleError> {
        let form = signed_form(method, &self.api_key, &self.secret, session_key, &params);
        let body = form_urlencoded::Serializer::new(String::new()).extend_pairs(form.iter()).finish();
        let resp = self
            .http
            .post(&self.api_root)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| ScrobbleError::new(ScrobbleError::NETWORK, e.to_string()))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.map_err(|e| ScrobbleError::new(ScrobbleError::NETWORK, e.to_string()))?;
        check_reply(status, &text)
    }

    fn unreadable() -> ScrobbleError {
        ScrobbleError::new(ScrobbleError::SERVER_ERROR, "The answer couldn't be read")
    }

    fn session_of(v: &Value) -> Result<Session, ScrobbleError> {
        let s = v.get("session").ok_or_else(Self::unreadable)?;
        Ok(Session {
            name: s.get("name").and_then(Value::as_str).ok_or_else(Self::unreadable)?.to_string(),
            key: s.get("key").and_then(Value::as_str).ok_or_else(Self::unreadable)?.to_string(),
        })
    }

    // ---- sign-in -------------------------------------------------------------

    /// Desktop flow step 1: a request token the user approves in the browser.
    pub async fn get_token(&self) -> Result<String, ScrobbleError> {
        let v = self.call("auth.getToken", vec![], None).await?;
        v.get("token").and_then(Value::as_str).map(str::to_string).ok_or_else(Self::unreadable)
    }

    /// Desktop flow step 2: the page where the user approves `token`.
    pub fn auth_url(&self, token: &str) -> String {
        format!("{}?api_key={}&token={}", self.auth_root, self.api_key, token)
    }

    /// Desktop flow step 3: trade the approved token for a session.
    pub async fn get_session(&self, token: &str) -> Result<Session, ScrobbleError> {
        let v = self.call("auth.getSession", vec![("token".into(), token.into())], None).await?;
        Self::session_of(&v)
    }

    /// Upstream's own sign-in form: username + password (never stored).
    pub async fn get_mobile_session(&self, username: &str, password: &str) -> Result<Session, ScrobbleError> {
        let login = if self.signs_in_with_token {
            ("authToken".to_string(), md5_hex(&format!("{}{}", username.to_lowercase(), md5_hex(password))))
        } else {
            ("password".to_string(), password.to_string())
        };
        let v = self.call("auth.getMobileSession", vec![("username".into(), username.into()), login], None).await?;
        Self::session_of(&v)
    }

    // ---- scrobbling ------------------------------------------------------------

    pub async fn update_now_playing(&self, session_key: &str, track: &ScrobbleTrack) -> Result<(), ScrobbleError> {
        self.call("track.updateNowPlaying", track_params(track, None), Some(session_key)).await.map(|_| ())
    }

    /// Up to [`MAX_BATCH`] plays, each with the time it started (unix seconds).
    pub async fn scrobble(&self, session_key: &str, plays: &[(ScrobbleTrack, i64)]) -> Result<(), ScrobbleError> {
        assert!(plays.len() <= MAX_BATCH, "at most {MAX_BATCH} scrobbles per request");
        self.call("track.scrobble", scrobble_params(plays), Some(session_key)).await.map(|_| ())
    }

    pub async fn love(&self, session_key: &str, artist: &str, track: &str, love: bool) -> Result<(), ScrobbleError> {
        let method = if love { "track.love" } else { "track.unlove" };
        self.call(method, vec![("artist".into(), artist.into()), ("track".into(), track.into())], Some(session_key)).await.map(|_| ())
    }
}

/// Batch params: `artist[i]`, `track[i]`, … and `timestamp[i]`.
pub fn scrobble_params(plays: &[(ScrobbleTrack, i64)]) -> Vec<(String, String)> {
    let mut p = Vec::new();
    for (i, (t, started)) in plays.iter().enumerate() {
        p.extend(track_params(t, Some(i)));
        p.push((format!("timestamp[{i}]"), started.to_string()));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vectors() {
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
    }

    #[test]
    fn signature_sorts_keys_and_appends_secret() {
        let mut p = BTreeMap::new();
        p.insert("method".to_string(), "auth.getSession".to_string());
        p.insert("api_key".to_string(), "KEY".to_string());
        p.insert("token".to_string(), "TOK".to_string());
        // api_keyKEY method auth.getSession token TOK + SECRET
        assert_eq!(signature(&p, "SECRET"), md5_hex("api_keyKEYmethodauth.getSessiontokenTOKSECRET"));
    }

    #[test]
    fn signed_form_leaves_format_unsigned() {
        let t = ScrobbleTrack { artist: "Band".into(), track: "Song".into(), album: Some("LP".into()), duration_seconds: Some(200) };
        let form = signed_form("track.updateNowPlaying", "K", "S", Some("SK"), &track_params(&t, None));
        let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        assert_eq!(get("format").as_deref(), Some("json"));
        assert_eq!(get("sk").as_deref(), Some("SK"));
        let expected = md5_hex("albumLPapi_keyKartistBandduration200methodtrack.updateNowPlayingskSKtrackSongS");
        assert_eq!(get("api_sig"), Some(expected));
        // format comes after api_sig and isn't part of it.
        assert_eq!(form.last().unwrap().0, "format");
    }

    #[test]
    fn batch_params_are_indexed() {
        let a = ScrobbleTrack { artist: "A".into(), track: "1".into(), album: None, duration_seconds: None };
        let b = ScrobbleTrack { artist: "B".into(), track: "2".into(), album: None, duration_seconds: Some(90) };
        let p = scrobble_params(&[(a, 100), (b, 200)]);
        assert!(p.contains(&("artist[0]".into(), "A".into())));
        assert!(p.contains(&("timestamp[1]".into(), "200".into())));
        assert!(p.contains(&("duration[1]".into(), "90".into())));
        assert!(!p.iter().any(|(k, _)| k == "album[0]"));
    }

    #[test]
    fn replies_and_error_codes() {
        let e = check_reply(403, r#"{"error":9,"message":"Invalid session key"}"#).unwrap_err();
        assert!(e.is_signed_out());
        assert!(check_reply(400, r#"{"error":6,"message":"x"}"#).unwrap_err().is_rejected());
        assert!(check_reply(200, r#"{"error":"16","message":"busy"}"#).unwrap_err().is_temporary());
        assert!(check_reply(403, r#"{"error":4,"message":"bad"}"#).unwrap_err().is_wrong_login());
        assert!(check_reply(502, "<html>").unwrap_err().is_temporary());
        assert!(check_reply(200, r#"{"token":"abc"}"#).is_ok());
        let s = Audioscrobbler::session_of(&serde_json::json!({"session":{"name":"me","key":"k1"}})).unwrap();
        assert_eq!(s, Session { name: "me".into(), key: "k1".into() });
    }

    #[test]
    fn libre_login_token_and_auth_url() {
        // authToken = md5(lower(user) + md5(password))
        assert_eq!(md5_hex(&format!("{}{}", "user", md5_hex("pw"))), md5_hex(&format!("user{}", md5_hex("pw"))));
        let a = Audioscrobbler::new(Audioscrobbler::LASTFM_ROOT, Audioscrobbler::LASTFM_AUTH, " KEY ", "S", false);
        assert_eq!(a.auth_url("T"), "https://www.last.fm/api/auth/?api_key=KEY&token=T");
        assert!(a.is_configured());
        assert!(!Audioscrobbler::new(Audioscrobbler::LIBREFM_ROOT, Audioscrobbler::LIBREFM_AUTH, "", "", true).is_configured());
    }

    #[test]
    fn track_serde_matches_upstream_names() {
        let t = ScrobbleTrack { artist: "A".into(), track: "T".into(), album: None, duration_seconds: Some(5) };
        assert_eq!(serde_json::to_string(&t).unwrap(), r#"{"artist":"A","track":"T","durationSeconds":5}"#);
    }
}
