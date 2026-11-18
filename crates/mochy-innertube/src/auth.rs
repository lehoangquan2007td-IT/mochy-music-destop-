//! Signed-in requests (port of the login half of Kotlin `InnerTube.ytClient`).
//!
//! YouTube's web clients authenticate with the browser's cookies plus an
//! `Authorization: SAPISIDHASH <ts>_<sha1("<ts> <SAPISID> <origin>")>` header.
//! The hash is tied to the origin the request goes to, so one made for
//! `music.youtube.com` is refused on `www.youtube.com` and vice versa.
//!
//! Only the web clients (`WEB_REMIX`, `WEB`) ever carry the cookie; stream
//! resolution stays anonymous (the native clients it uses don't log in).

use std::collections::BTreeMap;

use sha1::{Digest, Sha1};

/// The signed-in session handed to [`crate::YtmClient::set_auth`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct YtAuth {
    /// `Cookie` header value for `.youtube.com` (`name=value; name=value`).
    pub cookie: String,
    /// `DATASYNC_ID` from the page config; picks the brand account the
    /// session acts as (`context.user.onBehalfOfUser`).
    pub data_sync_id: Option<String>,
}

impl YtAuth {
    pub fn new(cookie: impl Into<String>, data_sync_id: Option<String>) -> Self {
        Self { cookie: normalize_cookie(&cookie.into()), data_sync_id: data_sync_id.as_deref().and_then(normalize_data_sync_id) }
    }

    /// The SAPISID value the hash is made from (`SAPISID`, or the
    /// `__Secure-3PAPISID` copy when the plain one is missing).
    pub fn sapisid(&self) -> Option<String> {
        let map = parse_cookie(&self.cookie);
        map.get("SAPISID").or_else(|| map.get("__Secure-3PAPISID")).filter(|v| !v.is_empty()).cloned()
    }

    /// Whether the cookie belongs to a signed-in session at all.
    pub fn is_signed_in(&self) -> bool {
        self.sapisid().is_some()
    }

    /// `Authorization` header value for a request to `origin`
    /// (e.g. `https://music.youtube.com`) at unix time `now_secs`.
    pub fn authorization(&self, origin: &str, now_secs: u64) -> Option<String> {
        let sapisid = self.sapisid()?;
        let hash = sapisid_hash(&sapisid, origin, now_secs);
        Some(format!("SAPISIDHASH {now_secs}_{hash}"))
    }

    /// Merge `Set-Cookie` headers from a response into the cookie (Google
    /// rotates `__Secure-*PSIDTS` and friends). Returns `true` when the
    /// cookie changed. Deleted cookies (empty value / expired) are dropped.
    pub fn merge_set_cookies<'a>(&mut self, headers: impl IntoIterator<Item = &'a str>) -> bool {
        let mut map = parse_cookie(&self.cookie);
        let mut changed = false;
        for h in headers {
            let Some((name, value, deleted)) = parse_set_cookie(h) else { continue };
            if deleted {
                changed |= map.remove(&name).is_some();
            } else if map.get(&name) != Some(&value) {
                map.insert(name, value);
                changed = true;
            }
        }
        if changed {
            self.cookie = join_cookie(&map);
        }
        changed
    }
}

/// `"a=1; b=2"` → map (Kotlin `parseCookieString`; tolerant of `;` without
/// a space, stray whitespace and a leading `Cookie:` label).
pub fn parse_cookie(cookie: &str) -> BTreeMap<String, String> {
    let cookie = cookie.trim();
    let cookie = cookie.strip_prefix("Cookie:").or_else(|| cookie.strip_prefix("cookie:")).unwrap_or(cookie);
    cookie
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            let (k, v) = part.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

fn join_cookie(map: &BTreeMap<String, String>) -> String {
    map.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
}

/// Canonical `name=value; …` form of a pasted cookie string.
pub fn normalize_cookie(cookie: &str) -> String {
    join_cookie(&parse_cookie(cookie))
}

/// `(name, value, deleted)` of one `Set-Cookie` header.
fn parse_set_cookie(h: &str) -> Option<(String, String, bool)> {
    let mut parts = h.split(';');
    let (name, value) = parts.next()?.split_once('=')?;
    let name = name.trim().to_string();
    let value = value.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let mut deleted = value.is_empty() || value == "EXPIRED";
    for attr in parts {
        let attr = attr.trim().to_ascii_lowercase();
        if attr == "max-age=0" || attr.starts_with("max-age=-") || attr.contains("expires=thu, 01 jan 1970") || attr.contains("expires=thu, 01-jan-1970")
        {
            deleted = true;
        }
    }
    Some((name, value, deleted))
}

/// Lower-case hex SHA-1 of `"{ts} {sapisid} {origin}"`.
pub fn sapisid_hash(sapisid: &str, origin: &str, ts: u64) -> String {
    let digest = Sha1::digest(format!("{ts} {sapisid} {origin}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `DATASYNC_ID` → the id to act as (App.kt): `"id"` stays, `"id||"` → `id`,
/// `"brand||main"` → `main`. Empty → `None`.
pub fn normalize_data_sync_id(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let id = if !raw.contains("||") {
        raw
    } else if let Some(stripped) = raw.strip_suffix("||") {
        stripped
    } else {
        raw.split_once("||").map(|(_, after)| after).unwrap_or(raw)
    };
    (!id.is_empty()).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_parsing_and_normalizing() {
        let m = parse_cookie("Cookie: SAPISID=abc/def; HSID=x;SSID = y ; bad; =z");
        assert_eq!(m.get("SAPISID").map(String::as_str), Some("abc/def"));
        assert_eq!(m.get("HSID").map(String::as_str), Some("x"));
        assert_eq!(m.get("SSID").map(String::as_str), Some("y"));
        assert_eq!(m.len(), 3);
        assert_eq!(normalize_cookie("b=2;a=1"), "a=1; b=2");
    }

    #[test]
    fn sapisid_hash_matches_reference() {
        let h = sapisid_hash("abc", "https://music.youtube.com", 1_700_000_000);
        assert_eq!(h.len(), 40);
        assert_eq!(h, sapisid_hash("abc", "https://music.youtube.com", 1_700_000_000));
        assert_ne!(h, sapisid_hash("abc", "https://www.youtube.com", 1_700_000_000));
        // The input is exactly "{ts} {sapisid} {origin}": sha1("1 a b").
        let direct: String = Sha1::digest(b"1 a b").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(sapisid_hash("a", "b", 1), direct);
        // SHA-1 itself (FIPS 180 test vector).
        let hex: String = Sha1::digest(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn authorization_header() {
        let a = YtAuth::new("SID=1; SAPISID=secret", None);
        assert!(a.is_signed_in());
        let h = a.authorization("https://music.youtube.com", 42).unwrap();
        assert!(h.starts_with("SAPISIDHASH 42_"));
        assert_eq!(h.len(), "SAPISIDHASH 42_".len() + 40);
        let anon = YtAuth::new("YSC=1; VISITOR_INFO1_LIVE=2", None);
        assert!(!anon.is_signed_in());
        assert!(anon.authorization("https://music.youtube.com", 42).is_none());
        let secure_only = YtAuth::new("__Secure-3PAPISID=s", None);
        assert_eq!(secure_only.sapisid().as_deref(), Some("s"));
    }

    #[test]
    fn data_sync_id_forms() {
        assert_eq!(normalize_data_sync_id("123").as_deref(), Some("123"));
        assert_eq!(normalize_data_sync_id("123||").as_deref(), Some("123"));
        assert_eq!(normalize_data_sync_id("brand||main").as_deref(), Some("main"));
        assert_eq!(normalize_data_sync_id("  "), None);
    }

    #[test]
    fn set_cookie_merge() {
        let mut a = YtAuth::new("SAPISID=s; __Secure-1PSIDTS=old; GONE=1", None);
        let changed = a.merge_set_cookies([
            "__Secure-1PSIDTS=new; Domain=.youtube.com; Path=/; Secure; HttpOnly",
            "GONE=; Max-Age=0",
            "SAPISID=s; Path=/",
        ]);
        assert!(changed);
        let m = parse_cookie(&a.cookie);
        assert_eq!(m.get("__Secure-1PSIDTS").map(String::as_str), Some("new"));
        assert!(!m.contains_key("GONE"));
        assert!(!a.merge_set_cookies(["SAPISID=s; Path=/"]));
    }
}
