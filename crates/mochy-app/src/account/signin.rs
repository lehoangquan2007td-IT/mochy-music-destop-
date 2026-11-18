//! Runs the `mochy-signin` helper (Google sign-in window, WebView2) and
//! reads the session it prints. Kept out of process so the player never
//! loads WebView2: a missing `WebView2Loader.dll` or a WebView2 crash only
//! fails the sign-in.

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use super::store::{self, Session};
use super::AccountMsg;
use crate::services::{AppMsg, Bus};

/// How a sign-in attempt ended.
pub enum SignInResult {
    Session(Session),
    /// The window was closed / cancelled.
    Cancelled,
    Failed(String),
}

#[cfg(windows)]
const HELPER: &str = "mochy-signin.exe";
#[cfg(not(windows))]
const HELPER: &str = "mochy-signin";

/// Windows `STATUS_DLL_NOT_FOUND`.
#[cfg(windows)]
const DLL_NOT_FOUND: i32 = 0xC000_0135_u32 as i32;

fn helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(HELPER))
}

/// A running sign-in window.
pub struct Helper {
    child: Arc<Mutex<Option<Child>>>,
}

impl Helper {
    /// Start the window; the result arrives as `AccountMsg::SignIn`.
    pub fn spawn(bus: Bus, fresh: bool, vietnamese: bool) -> Result<Helper, String> {
        let path = helper_path().ok_or("cannot locate the app folder")?;
        if !path.exists() {
            return Err(format!("{HELPER} is missing next to mochy-app"));
        }
        #[cfg(windows)]
        if !path.with_file_name("WebView2Loader.dll").exists() {
            return Err("WebView2Loader.dll is missing next to mochy-signin.exe".into());
        }
        let mut cmd = Command::new(&path);
        cmd.arg("--profile").arg(store::webview_dir()).arg("--lang").arg(if vietnamese { "vi" } else { "en" });
        if fresh {
            cmd.arg("--fresh");
        }
        let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));

        if let Some(err) = stderr {
            std::thread::Builder::new()
                .name("mochy-signin-log".into())
                .spawn(move || {
                    for line in BufReader::new(err).lines().map_while(Result::ok) {
                        tracing::info!(target: "mochy_signin", "{line}");
                    }
                })
                .ok();
        }
        let waiter = child.clone();
        std::thread::Builder::new()
            .name("mochy-signin-wait".into())
            .spawn(move || {
                let mut out = String::new();
                if let Some(mut s) = stdout {
                    let _ = s.read_to_string(&mut out);
                }
                let status = waiter.lock().ok().and_then(|mut g| g.take()).and_then(|mut c| c.wait().ok());
                let code = status.and_then(|s| s.code());
                bus.send(AppMsg::Account(AccountMsg::SignIn(interpret(&out, code))));
            })
            .map_err(|e| e.to_string())?;
        Ok(Helper { child })
    }

    /// Close the window (the waiter then reports `Cancelled`).
    pub fn cancel(&self) {
        if let Ok(mut g) = self.child.lock()
            && let Some(c) = g.as_mut()
        {
            let _ = c.kill();
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Turn the helper's stdout + exit code into a result.
fn interpret(stdout: &str, code: Option<i32>) -> SignInResult {
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{'));
    if let Some(line) = line
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
    {
        if let Some(e) = v["error"].as_str() {
            return SignInResult::Failed(e.to_string());
        }
        if let Some(cookie) = v["cookie"].as_str().filter(|c| !c.is_empty()) {
            let s = |k: &str| v[k].as_str().map(str::to_string).filter(|x| !x.is_empty());
            return SignInResult::Session(Session { cookie: cookie.to_string(), data_sync_id: s("dataSyncId"), visitor_data: s("visitorData") });
        }
    }
    match code {
        #[cfg(windows)]
        Some(DLL_NOT_FOUND) => SignInResult::Failed("WebView2Loader.dll was not found".into()),
        Some(0) | Some(2) | None => SignInResult::Cancelled,
        Some(c) => SignInResult::Failed(format!("the sign-in window exited with code {c}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interprets_helper_output() {
        match interpret("{\"cookie\":\"SAPISID=a\",\"dataSyncId\":\"1||\",\"visitorData\":null}\n", Some(0)) {
            SignInResult::Session(s) => {
                assert_eq!(s.cookie, "SAPISID=a");
                assert_eq!(s.data_sync_id.as_deref(), Some("1||"));
                assert!(s.visitor_data.is_none());
            }
            _ => panic!("expected a session"),
        }
        assert!(matches!(interpret("{\"error\":\"WebView2: boom\"}", Some(1)), SignInResult::Failed(e) if e.contains("boom")));
        assert!(matches!(interpret("", Some(2)), SignInResult::Cancelled));
        assert!(matches!(interpret("", None), SignInResult::Cancelled));
        assert!(matches!(interpret("garbage", Some(7)), SignInResult::Failed(_)));
        #[cfg(windows)]
        assert!(matches!(interpret("", Some(DLL_NOT_FOUND)), SignInResult::Failed(e) if e.contains("WebView2Loader")));
    }
}
