//! Discord local RPC over IPC: `\\.\pipe\discord-ipc-N` on Windows,
//! `$XDG_RUNTIME_DIR/discord-ipc-N` (and friends) elsewhere.
//!
//! Wire format: every frame is `opcode: u32 LE`, `length: u32 LE`, then
//! `length` bytes of UTF-8 JSON. The client sends a HANDSHAKE
//! (`{"v":1,"client_id":…}`), Discord answers with a `READY` dispatch, after
//! which commands go out as FRAMEs carrying `{cmd, args, nonce}` and each gets a
//! reply with the same nonce.

use std::io;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
pub const OP_CLOSE: u32 = 2;
pub const OP_PING: u32 = 3;
pub const OP_PONG: u32 = 4;

/// Frames bigger than this are treated as a broken stream.
const MAX_FRAME: u32 = 1 << 20;
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum IpcError {
    /// No Discord client is listening on any pipe.
    NotRunning,
    /// Discord refused the application id (close code 4000 and friends).
    InvalidClientId(String),
    /// Discord answered a command with an ERROR event.
    Rejected { code: i64, message: String },
    Io(io::Error),
    Protocol(String),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IpcError::NotRunning => write!(f, "Discord is not running"),
            IpcError::InvalidClientId(m) => write!(f, "Discord refused the application id: {m}"),
            IpcError::Rejected { code, message } => write!(f, "Discord rejected the status ({code}): {message}"),
            IpcError::Io(e) => write!(f, "Discord connection error: {e}"),
            IpcError::Protocol(m) => write!(f, "Discord protocol error: {m}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<io::Error> for IpcError {
    fn from(e: io::Error) -> Self {
        IpcError::Io(e)
    }
}

/// Encode one IPC frame.
pub fn encode_frame(op: u32, payload: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).unwrap_or_else(|_| b"{}".to_vec());
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Split a frame header into `(opcode, length)`.
pub fn decode_header(h: &[u8; 8]) -> (u32, u32) {
    (u32::from_le_bytes([h[0], h[1], h[2], h[3]]), u32::from_le_bytes([h[4], h[5], h[6], h[7]]))
}

/// Decode a complete frame from a buffer (tests / tooling).
pub fn decode_frame(buf: &[u8]) -> Option<(u32, Value)> {
    if buf.len() < 8 {
        return None;
    }
    let (op, len) = decode_header(buf[..8].try_into().ok()?);
    let body = buf.get(8..8 + len as usize)?;
    Some((op, serde_json::from_slice(body).ok()?))
}

/// The handshake payload.
pub fn handshake_payload(client_id: &str) -> Value {
    json!({ "v": 1, "client_id": client_id })
}

/// A `SET_ACTIVITY` command; `activity: None` clears the status.
pub fn set_activity_payload(pid: u32, activity: Option<&Value>, nonce: &str) -> Value {
    let mut args = json!({ "pid": pid });
    if let Some(a) = activity {
        args["activity"] = a.clone();
    }
    json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce })
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// One open connection to the local Discord client.
pub struct IpcClient {
    stream: Box<dyn Stream>,
    nonce: u64,
    /// Discord user name from the READY event, when it said.
    pub user: Option<String>,
}

impl IpcClient {
    /// Open the first pipe Discord listens on and do the handshake.
    pub async fn connect(client_id: &str) -> Result<Self, IpcError> {
        let stream = open_any().await?;
        let mut c = Self { stream, nonce: 0, user: None };
        c.write(OP_HANDSHAKE, &handshake_payload(client_id)).await?;
        let (op, v) = tokio::time::timeout(REPLY_TIMEOUT, c.read()).await.map_err(|_| IpcError::Protocol("no READY".into()))??;
        match op {
            OP_FRAME if v["evt"] == "READY" => {
                c.user = v["data"]["user"]["username"].as_str().map(str::to_string);
                Ok(c)
            }
            OP_CLOSE => Err(IpcError::InvalidClientId(v["message"].as_str().unwrap_or("closed").to_string())),
            _ if v["evt"] == "ERROR" => Err(IpcError::InvalidClientId(v["data"]["message"].as_str().unwrap_or("error").to_string())),
            _ => Err(IpcError::Protocol(format!("unexpected handshake reply op {op}"))),
        }
    }

    async fn write(&mut self, op: u32, payload: &Value) -> Result<(), IpcError> {
        let frame = encode_frame(op, payload);
        self.stream.write_all(&frame).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Read the next frame, answering pings on the way.
    async fn read(&mut self) -> Result<(u32, Value), IpcError> {
        loop {
            let mut h = [0u8; 8];
            self.stream.read_exact(&mut h).await?;
            let (op, len) = decode_header(&h);
            if len > MAX_FRAME {
                return Err(IpcError::Protocol(format!("frame of {len} bytes")));
            }
            let mut body = vec![0u8; len as usize];
            self.stream.read_exact(&mut body).await?;
            let v: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap_or(Value::Null) };
            match op {
                OP_PING => self.write(OP_PONG, &v).await?,
                OP_PONG => {}
                _ => return Ok((op, v)),
            }
        }
    }

    /// Send a command and wait for the reply with the same nonce.
    async fn command(&mut self, payload_for: impl FnOnce(&str) -> Value) -> Result<Value, IpcError> {
        self.nonce += 1;
        let nonce = format!("mochy-{}-{}", std::process::id(), self.nonce);
        let payload = payload_for(&nonce);
        self.write(OP_FRAME, &payload).await?;
        let reply = tokio::time::timeout(REPLY_TIMEOUT, async {
            loop {
                let (op, v) = self.read().await?;
                if op == OP_CLOSE {
                    return Err(IpcError::Io(io::Error::new(io::ErrorKind::ConnectionAborted, "Discord closed the connection")));
                }
                if v["nonce"].as_str() == Some(nonce.as_str()) {
                    return Ok(v);
                }
            }
        })
        .await
        .map_err(|_| IpcError::Protocol("no reply".into()))??;
        if reply["evt"] == "ERROR" {
            return Err(IpcError::Rejected {
                code: reply["data"]["code"].as_i64().unwrap_or(0),
                message: reply["data"]["message"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(reply)
    }

    /// Set (or with `None` clear) the activity for this process.
    pub async fn set_activity(&mut self, activity: Option<&Value>) -> Result<(), IpcError> {
        let pid = std::process::id();
        self.command(|nonce| set_activity_payload(pid, activity, nonce)).await.map(|_| ())
    }

    /// Polite close (best effort).
    pub async fn close(mut self) {
        let _ = self.write(OP_CLOSE, &json!({})).await;
        let _ = self.stream.shutdown().await;
    }
}

#[cfg(windows)]
async fn open_any() -> Result<Box<dyn Stream>, IpcError> {
    use tokio::net::windows::named_pipe::ClientOptions;
    for i in 0..10 {
        let name = format!(r"\\.\pipe\discord-ipc-{i}");
        match ClientOptions::new().open(&name) {
            Ok(c) => return Ok(Box::new(c)),
            // 231 = ERROR_PIPE_BUSY: someone else is mid-connect; one short retry.
            Err(e) if e.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Ok(c) = ClientOptions::new().open(&name) {
                    return Ok(Box::new(c));
                }
            }
            Err(_) => {}
        }
    }
    Err(IpcError::NotRunning)
}

#[cfg(unix)]
async fn open_any() -> Result<Box<dyn Stream>, IpcError> {
    let mut dirs: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(|k| std::env::var_os(k).map(Into::into))
        .collect();
    dirs.push("/tmp".into());
    let subdirs = ["", "app/com.discordapp.Discord", "snap.discord", ".flatpak/com.discordapp.Discord/xdg-run"];
    for dir in &dirs {
        for sub in subdirs {
            for i in 0..10 {
                let path = dir.join(sub).join(format!("discord-ipc-{i}"));
                if let Ok(s) = tokio::net::UnixStream::connect(&path).await {
                    return Ok(Box::new(s));
                }
            }
        }
    }
    Err(IpcError::NotRunning)
}

#[cfg(not(any(windows, unix)))]
async fn open_any() -> Result<Box<dyn Stream>, IpcError> {
    Err(IpcError::NotRunning)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_encoding_matches_the_wire_format() {
        let f = encode_frame(OP_HANDSHAKE, &handshake_payload("123"));
        assert_eq!(&f[..4], &[0, 0, 0, 0]);
        let len = u32::from_le_bytes([f[4], f[5], f[6], f[7]]) as usize;
        assert_eq!(len, f.len() - 8);
        let body: Value = serde_json::from_slice(&f[8..]).unwrap();
        assert_eq!(body, json!({"v": 1, "client_id": "123"}));
        let (op, v) = decode_frame(&f).unwrap();
        assert_eq!(op, 0);
        assert_eq!(v["client_id"], "123");

        let p = set_activity_payload(42, Some(&json!({"details": "x"})), "n1");
        let f = encode_frame(OP_FRAME, &p);
        assert_eq!(&f[..4], &1u32.to_le_bytes());
        let (_, v) = decode_frame(&f).unwrap();
        assert_eq!(v["cmd"], "SET_ACTIVITY");
        assert_eq!(v["args"]["pid"], 42);
        assert_eq!(v["args"]["activity"]["details"], "x");
        assert_eq!(v["nonce"], "n1");
        // Clearing leaves the activity out entirely.
        let clear = set_activity_payload(42, None, "n2");
        assert!(clear["args"].get("activity").is_none());
    }

    #[test]
    fn header_is_little_endian() {
        assert_eq!(decode_header(&[1, 0, 0, 0, 0x10, 0x01, 0, 0]), (1, 0x0110));
        assert!(decode_frame(&[1, 0, 0]).is_none());
        // A length that runs past the buffer is not a frame.
        assert!(decode_frame(&[1, 0, 0, 0, 9, 0, 0, 0, b'{', b'}']).is_none());
    }

    /// With a fake Discord on the other end of an in-memory pipe: handshake,
    /// READY, a ping in between, then a SET_ACTIVITY round trip.
    #[tokio::test]
    async fn handshake_and_set_activity_round_trip() {
        let (client_side, mut server) = tokio::io::duplex(4096);
        let fake = tokio::spawn(async move {
            async fn read(s: &mut tokio::io::DuplexStream) -> (u32, Value) {
                let mut h = [0u8; 8];
                s.read_exact(&mut h).await.unwrap();
                let (op, len) = decode_header(&h);
                let mut b = vec![0u8; len as usize];
                s.read_exact(&mut b).await.unwrap();
                (op, serde_json::from_slice(&b).unwrap())
            }
            let (op, hs) = read(&mut server).await;
            assert_eq!(op, OP_HANDSHAKE);
            assert_eq!(hs["client_id"], "42");
            server
                .write_all(&encode_frame(OP_FRAME, &json!({"cmd":"DISPATCH","evt":"READY","data":{"user":{"username":"mochy"}}})))
                .await
                .unwrap();
            let (op, cmd) = read(&mut server).await;
            assert_eq!(op, OP_FRAME);
            assert_eq!(cmd["cmd"], "SET_ACTIVITY");
            server.write_all(&encode_frame(OP_PING, &json!({}))).await.unwrap();
            let (op, _) = read(&mut server).await;
            assert_eq!(op, OP_PONG);
            let nonce = cmd["nonce"].clone();
            server.write_all(&encode_frame(OP_FRAME, &json!({"cmd":"SET_ACTIVITY","nonce":nonce,"data":{}}))).await.unwrap();
            // Second command gets an ERROR back.
            let (_, cmd2) = read(&mut server).await;
            server
                .write_all(&encode_frame(
                    OP_FRAME,
                    &json!({"cmd":"SET_ACTIVITY","evt":"ERROR","nonce":cmd2["nonce"],"data":{"code":4000,"message":"bad"}}),
                ))
                .await
                .unwrap();
        });
        let mut c = IpcClient { stream: Box::new(client_side), nonce: 0, user: None };
        c.write(OP_HANDSHAKE, &handshake_payload("42")).await.unwrap();
        let (op, ready) = c.read().await.unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(ready["evt"], "READY");
        c.set_activity(Some(&json!({"details":"song"}))).await.unwrap();
        let err = c.set_activity(None).await.unwrap_err();
        assert!(matches!(err, IpcError::Rejected { code: 4000, .. }));
        fake.await.unwrap();
    }
}
