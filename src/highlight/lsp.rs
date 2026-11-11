//! The smallest LSP client the semantic indexer needs: base-protocol
//! framing, requests with ids, replies to server-initiated requests, and
//! orderly shutdown of an owned server process. Protocol traffic uses the
//! server's stdin/stdout only; its stderr is kept apart as a bounded log.

use crate::process::{self, Limits, OwnedChild};
use crate::text::PosEncoding;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{ChildStdin, Command};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Upper bound for one message body.
pub const MAX_MESSAGE_BYTES: usize = 512 << 20;

#[derive(Debug)]
pub enum Incoming {
    Response { id: u64, result: Result<Value, (i64, String)> },
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
    /// The stream ended or broke; nothing follows.
    Closed(String),
}

/// Reads one `Content-Length`-framed JSON message. `Ok(None)` at a clean
/// end of stream; malformed framing or JSON is an error.
pub fn read_frame(r: &mut impl BufRead) -> Result<Option<Value>> {
    let mut len: Option<usize> = None;
    let mut headers = 0;
    loop {
        let mut line = Vec::new();
        let n = r.by_ref().take(4096).read_until(b'\n', &mut line)?;
        if n == 0 {
            if headers == 0 {
                return Ok(None);
            }
            bail!("stream ended inside a message header");
        }
        if !line.ends_with(b"\n") {
            bail!("header line too long");
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        headers += 1;
        if headers > 32 {
            bail!("too many header lines");
        }
        let Some((k, v)) = line.split_once(':') else { bail!("malformed header line {line:?}") };
        if k.trim().eq_ignore_ascii_case("content-length") {
            len = Some(v.trim().parse::<usize>().map_err(|_| anyhow::anyhow!("bad Content-Length {v:?}"))?);
        }
    }
    let Some(len) = len else { bail!("message without Content-Length") };
    if len > MAX_MESSAGE_BYTES {
        bail!("message of {len} bytes exceeds the limit");
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).context("stream ended inside a message body")?;
    Ok(Some(serde_json::from_slice(&body).context("message body is not JSON")?))
}

pub fn classify(v: Value) -> Result<Incoming> {
    let Value::Object(mut o) = v else { bail!("message is not a JSON object") };
    let method = o.get("method").and_then(|m| m.as_str()).map(String::from);
    let id = o.remove("id");
    match (method, id) {
        (Some(method), Some(id)) => Ok(Incoming::Request { id, method, params: o.remove("params").unwrap_or(Value::Null) }),
        (Some(method), None) => Ok(Incoming::Notification { method, params: o.remove("params").unwrap_or(Value::Null) }),
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { bail!("response with an id this client never issued: {id}") };
            if let Some(e) = o.remove("error") {
                let code = e.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
                return Ok(Incoming::Response { id, result: Err((code, msg)) });
            }
            Ok(Incoming::Response { id, result: Ok(o.remove("result").unwrap_or(Value::Null)) })
        }
        (None, None) => bail!("message with neither method nor id"),
    }
}

pub fn write_frame(w: &mut impl Write, v: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

/// `file://` URI of an absolute path (percent-encoding everything outside
/// the unreserved set).
pub fn path_to_uri(p: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// What the server said about semantic tokens when initialised.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub legend: super::Legend,
    pub encoding: PosEncoding,
    pub full: bool,
    pub range: bool,
    pub name: Option<String>,
}

/// Parses an `initialize` result.
pub fn server_info(result: &Value) -> Result<ServerInfo> {
    let caps = result.get("capabilities").context("initialize result has no capabilities")?;
    let Some(st) = caps.get("semanticTokensProvider").filter(|v| v.is_object()) else {
        bail!("the server does not offer semantic tokens");
    };
    let list = |k: &str| -> Result<Vec<String>> {
        st.pointer(&format!("/legend/{k}"))
            .and_then(|v| v.as_array())
            .with_context(|| format!("semantic token legend has no {k}"))?
            .iter()
            .map(|v| v.as_str().map(String::from).context("legend entry is not a string"))
            .collect()
    };
    let legend = super::Legend { types: list("tokenTypes")?, modifiers: list("tokenModifiers")? };
    if legend.types.is_empty() || legend.types.len() > u16::MAX as usize || legend.modifiers.len() > 32 {
        bail!("unusable semantic token legend ({} types, {} modifiers)", legend.types.len(), legend.modifiers.len());
    }
    // Absent means UTF-16 (LSP 3.17).
    let encoding = match caps.get("positionEncoding").and_then(|v| v.as_str()) {
        None => PosEncoding::Utf16,
        Some(s) => super::semantic::parse_encoding(s).with_context(|| format!("unsupported position encoding {s:?}"))?,
    };
    let full = match st.get("full") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Object(_)) => true,
        _ => false,
    };
    Ok(ServerInfo {
        legend,
        encoding,
        full,
        range: st.get("range").is_some_and(|v| v.as_bool() == Some(true) || v.is_object()),
        name: result.pointer("/serverInfo/name").and_then(|v| v.as_str()).map(|n| {
            format!("{n} {}", result.pointer("/serverInfo/version").and_then(|v| v.as_str()).unwrap_or(""))
        }),
    })
}

/// How a server process ended.
#[derive(Clone, Debug, Default)]
pub struct ShutdownReport {
    /// The server answered `shutdown` and exited by itself.
    pub clean: bool,
    pub elapsed: Duration,
    /// Processes of the server's tree still alive afterwards (expected 0).
    pub lingering: usize,
}

pub struct Client {
    child: OwnedChild,
    stdin: Option<ChildStdin>,
    rx: Receiver<Incoming>,
    next_id: u64,
    closed: bool,
}

impl Client {
    /// Starts the server in its own process group with `limits`.
    pub fn start(cmd: &mut Command, limits: &Limits) -> Result<Client> {
        let mut child = process::spawn_piped(cmd, limits)?;
        let stdin = child.take_stdin().context("server stdin")?;
        let stdout = child.take_stdout().context("server stdout")?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new().name("slim-lsp-read".into()).spawn(move || {
            let mut r = BufReader::with_capacity(1 << 16, stdout);
            loop {
                let msg = match read_frame(&mut r) {
                    Ok(Some(v)) => classify(v),
                    Ok(None) => {
                        let _ = tx.send(Incoming::Closed("the server closed its output".into()));
                        return;
                    }
                    Err(e) => Err(e),
                };
                match msg {
                    Ok(m) => {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        // After a framing error the stream cannot be trusted.
                        let _ = tx.send(Incoming::Closed(format!("malformed protocol data: {e:#}")));
                        return;
                    }
                }
            }
        })?;
        Ok(Client { child, stdin: Some(stdin), rx, next_id: 0, closed: false })
    }

    pub fn pid(&self) -> i32 {
        self.child.pid()
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.child.stderr_tail()
    }

    fn send(&mut self, v: Value) -> Result<()> {
        let Some(w) = self.stdin.as_mut() else { bail!("server input is closed") };
        write_frame(w, &v).context("writing to the server")
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        Ok(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    pub fn respond(&mut self, id: &Value, result: Value) -> Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }

    pub fn respond_error(&mut self, id: &Value, code: i64, message: &str) -> Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
    }

    /// Answers a server-initiated request the indexer has no use for:
    /// registrations and refreshes succeed with `null`, configuration
    /// queries get `null` per item, anything else is "method not found".
    pub fn answer_default(&mut self, id: &Value, method: &str, params: &Value) -> Result<()> {
        match method {
            "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create" => self.respond(id, Value::Null),
            m if m.ends_with("/refresh") => self.respond(id, Value::Null),
            "workspace/configuration" => {
                let n = params.get("items").and_then(|i| i.as_array()).map(|a| a.len()).unwrap_or(0);
                self.respond(id, Value::Array(vec![Value::Null; n]))
            }
            "workspace/workspaceFolders" => self.respond(id, Value::Null),
            "window/showMessageRequest" | "window/showDocument" => self.respond(id, Value::Null),
            _ => self.respond_error(id, -32601, "method not supported by this client"),
        }
    }

    /// The next message, or `None` after `timeout`. After `Closed` every
    /// call returns `Closed` again.
    pub fn recv(&mut self, timeout: Duration) -> Option<Incoming> {
        if self.closed {
            return Some(Incoming::Closed("the server is gone".into()));
        }
        match self.rx.recv_timeout(timeout) {
            Ok(m) => {
                if matches!(m, Incoming::Closed(_)) {
                    self.closed = true;
                }
                Some(m)
            }
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Some(Incoming::Closed("the reader stopped".into()))
            }
        }
    }

    /// `initialize` / `initialized`, returning the negotiated facts.
    pub fn initialize(&mut self, root: &Path, timeout: Duration, cancel: &std::sync::atomic::AtomicBool) -> Result<ServerInfo> {
        let id = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "clientInfo": { "name": "slim", "version": env!("CARGO_PKG_VERSION") },
                "rootUri": path_to_uri(root),
                "capabilities": {
                    "general": { "positionEncodings": ["utf-16", "utf-8", "utf-32"] },
                    "textDocument": {
                        "synchronization": { "dynamicRegistration": false },
                        "publishDiagnostics": { "versionSupport": true },
                        "semanticTokens": {
                            "dynamicRegistration": false,
                            "requests": { "full": true, "range": true },
                            "tokenTypes": [], "tokenModifiers": [], "formats": ["relative"],
                            "overlappingTokenSupport": false, "multilineTokenSupport": false
                        }
                    },
                    "workspace": { "semanticTokens": { "refreshSupport": true } }
                }
            }),
        )?;
        let start = Instant::now();
        loop {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                bail!("cancelled");
            }
            if start.elapsed() > timeout {
                bail!("the server did not answer `initialize` within {} s", timeout.as_secs());
            }
            match self.recv(Duration::from_millis(50)) {
                Some(Incoming::Response { id: rid, result }) if rid == id => {
                    let result = result.map_err(|(c, m)| anyhow::anyhow!("initialize failed ({c}): {m}"))?;
                    let info = server_info(&result)?;
                    self.notify("initialized", json!({}))?;
                    return Ok(info);
                }
                Some(Incoming::Request { id, method, params }) => self.answer_default(&id, &method, &params)?,
                Some(Incoming::Closed(why)) => bail!("the server exited during initialisation: {why}. {}", self.stderr_tail().join(" | ")),
                _ => {}
            }
        }
    }

    /// Ends the server: `shutdown` + `exit` when `polite`, a bounded wait,
    /// then SIGTERM to its process group and SIGKILL after 1.5 s. Always
    /// reaps, and reports whether anything of its tree survived.
    pub fn shutdown(mut self, polite: bool) -> ShutdownReport {
        let start = Instant::now();
        let tree = process::descendants(self.child.pid());
        let mut clean = false;
        if polite && !self.closed {
            if let Ok(id) = self.request("shutdown", Value::Null) {
                let deadline = Instant::now() + Duration::from_millis(1500);
                while Instant::now() < deadline {
                    match self.recv(Duration::from_millis(20)) {
                        Some(Incoming::Response { id: rid, .. }) if rid == id => {
                            let _ = self.notify("exit", Value::Null);
                            break;
                        }
                        Some(Incoming::Closed(_)) => break,
                        _ => {}
                    }
                }
                self.stdin = None;
                let deadline = Instant::now() + Duration::from_millis(1000);
                while Instant::now() < deadline {
                    if self.child.exited().is_some() {
                        clean = true;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        self.stdin = None;
        self.child.terminate();
        // A sandbox's inner processes die with its init; give the kernel a
        // moment before declaring anything lingering.
        let mut lingering;
        let deadline = Instant::now() + Duration::from_millis(1000);
        loop {
            lingering = tree.iter().filter(|&&p| process::process_alive(p)).count();
            if lingering == 0 || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        ShutdownReport { clean, elapsed: start.elapsed(), lingering }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    #[test]
    fn frames_are_read_strictly() {
        let mut data = frame(r#"{"jsonrpc":"2.0","id":1,"result":{"a":1}}"#);
        data.extend(b"Content-Type: x\r\ncontent-length: 2\r\n\r\n{}".iter());
        let mut r = std::io::Cursor::new(data);
        assert_eq!(read_frame(&mut r).unwrap().unwrap()["result"]["a"], 1);
        assert_eq!(read_frame(&mut r).unwrap().unwrap(), json!({}));
        assert!(read_frame(&mut r).unwrap().is_none(), "clean end of stream");
        for bad in [
            &b"Content-Length: 5\r\n\r\n{}"[..],       // body shorter than announced
            b"Content-Length: x\r\n\r\n{}",            // not a number
            b"\r\n{}",                                 // no length
            b"garbage without colon\r\n\r\n",          // not a header
            b"Content-Length: 3\r\n\r\n{]}",           // not JSON
            b"Content-Length: 99999999999\r\n\r\n",    // over the limit
            b"Content-Length: 2",                      // ends inside the header
        ] {
            assert!(read_frame(&mut std::io::Cursor::new(bad)).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        let long = format!("X: {}\r\n\r\n", "a".repeat(5000));
        assert!(read_frame(&mut std::io::Cursor::new(long.into_bytes())).is_err());
    }

    #[test]
    fn messages_are_classified() {
        assert!(matches!(classify(json!({"id": 7, "result": null})).unwrap(), Incoming::Response { id: 7, result: Ok(Value::Null) }));
        match classify(json!({"id": 7, "error": {"code": -32801, "message": "gone"}})).unwrap() {
            Incoming::Response { result: Err((c, m)), .. } => assert_eq!((c, m.as_str()), (-32801, "gone")),
            o => panic!("{o:?}"),
        }
        // A server request may reuse a number this client also used: it is a
        // request because it has a method.
        assert!(matches!(classify(json!({"id": 7, "method": "workspace/semanticTokens/refresh"})).unwrap(), Incoming::Request { .. }));
        assert!(matches!(classify(json!({"id": "s1", "method": "client/registerCapability", "params": {}})).unwrap(), Incoming::Request { .. }));
        assert!(matches!(classify(json!({"method": "$/lean/fileProgress", "params": {}})).unwrap(), Incoming::Notification { .. }));
        assert!(classify(json!({"id": "never-issued", "result": 1})).is_err());
        assert!(classify(json!([1, 2])).is_err());
        assert!(classify(json!({})).is_err());
    }

    #[test]
    fn initialize_result_gives_legend_and_encoding() {
        let r = json!({"capabilities": {"semanticTokensProvider": {"full": true, "range": true,
            "legend": {"tokenTypes": ["keyword", "variable"], "tokenModifiers": ["declaration"]}}}});
        let i = server_info(&r).unwrap();
        assert_eq!(i.legend.types, vec!["keyword", "variable"]);
        assert_eq!(i.encoding, PosEncoding::Utf16, "absent means UTF-16");
        assert!(i.full && i.range);
        let r8 = json!({"capabilities": {"positionEncoding": "utf-8", "semanticTokensProvider": {"full": {"delta": true},
            "legend": {"tokenTypes": ["a"], "tokenModifiers": []}}}});
        let i = server_info(&r8).unwrap();
        assert_eq!(i.encoding, PosEncoding::Utf8);
        assert!(i.full && !i.range);
        assert!(server_info(&json!({"capabilities": {}})).is_err());
        assert!(server_info(&json!({"capabilities": {"positionEncoding": "ebcdic", "semanticTokensProvider": {"legend": {"tokenTypes": ["a"], "tokenModifiers": []}}}})).is_err());
        assert!(server_info(&json!({"capabilities": {"semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}}}})).is_err());
    }

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(path_to_uri(Path::new("/a b/Ünï.lean")), "file:///a%20b/%C3%9Cn%C3%AF.lean");
    }
}
