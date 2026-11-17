//! `lightair mcp`: the remote API's MCP server on stdin/stdout, for agents that start their tools
//! as programs (`claude mcp add lightair -- …/lightair mcp`). Each message goes to the running
//! app's `/mcp` with the password from the Keychain; a tool call while Lightair is closed opens
//! it first.

use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Runs the bridge if that's what was asked for (the exit code).
pub fn main_if_asked() -> Option<i32> {
    (std::env::args().nth(1).as_deref() == Some("mcp")).then(run)
}

fn run() -> i32 {
    let prefs = crate::prefs::load_from(&crate::prefs::path());
    let port: u16 = std::env::var("LIGHTAIR_REMOTE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(prefs.remote.port);
    let password = (prefs.remote.require_password || prefs.remote.lan).then(crate::secrets::remote_password).flatten();
    let stdin = io::stdin();
    let mut out = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match post(port, line.as_bytes(), password.as_deref()) {
            Ok(a) => a,
            Err(e) if refused(&e) => {
                let is_call = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .is_some_and(|m| m["method"] == "tools/call");
                if is_call && prefs.remote.enabled && open_app(port) {
                    post(port, line.as_bytes(), password.as_deref()).unwrap_or_else(|e| error_for(&line, &e.to_string()))
                } else {
                    lightair_remote::mcp::answer_without_app(line.as_bytes())
                }
            }
            Err(e) => error_for(&line, &e.to_string()),
        };
        if let Some(a) = answer
            && writeln!(out, "{a}").and_then(|_| out.flush()).is_err()
        {
            break;
        }
    }
    0
}

fn refused(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset | io::ErrorKind::NotFound)
}

/// A JSON-RPC error for the request on `line` (nothing for a notification).
fn error_for(line: &str, message: &str) -> Option<String> {
    let id = serde_json::from_str::<serde_json::Value>(line).ok()?.get("id")?.clone();
    Some(serde_json::json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": format!("Lightair: {message}")}}).to_string())
}

/// Opens the app this binary belongs to and waits for its remote API.
fn open_app(port: u16) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    if cfg!(windows) {
        // the app itself, on its own (not this bridge's child: it outlives the agent's session)
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
            let started = std::process::Command::new(&exe)
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if started.is_err() {
                return false;
            }
        }
    } else {
        // …/Lightair.app/Contents/MacOS/lightair
        let Some(bundle) = exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app")) else { return false };
        if std::process::Command::new("open").arg("-g").arg(bundle).status().is_err() {
            return false;
        }
    }
    let until = Instant::now() + Duration::from_secs(40);
    while Instant::now() < until {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            // the window answers once it's up
            std::thread::sleep(Duration::from_millis(500));
            return true;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// POSTs one message: the answer's body (`None` for 202 Accepted).
fn post(port: u16, body: &[u8], password: Option<&str>) -> io::Result<Option<String>> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    let auth = password.map(|p| format!("Authorization: Bearer {p}\r\n")).unwrap_or_default();
    write!(
        s,
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").ok_or_else(|| io::Error::other("bad answer"))?;
    let status: u16 = head.get(9..12).and_then(|s| s.parse().ok()).unwrap_or(0);
    match status {
        200 => Ok(Some(body.to_string())),
        202 => Ok(None),
        401 => Err(io::Error::other("the remote API password didn't match (Settings › Remote API)")),
        s => Err(io::Error::other(format!("HTTP {s}"))),
    }
}
