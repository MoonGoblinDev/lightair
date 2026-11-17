//! HTTP and WebSocket plumbing the readers share: a browser-like client, and sockets that wake
//! up now and then so a reader can notice it's been stopped.

use std::net::TcpStream;
use std::time::Duration;

use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// What Lightair's requests say they are: the platforms' web pages answer browsers.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier).build())
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// GET as text; an error for anything but 2xx.
pub fn get(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> Result<String, String> {
    let mut req = agent.get(url).header("User-Agent", USER_AGENT).header("Accept-Language", "en-US,en;q=0.9");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut res = req.call().map_err(|e| format!("{url}: {e}"))?;
    let status = res.status();
    let body = res.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    Ok(body)
}

pub fn get_json(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, String> {
    serde_json::from_str(&get(agent, url, headers)?).map_err(|e| format!("{url}: {e}"))
}

pub fn post_json(agent: &ureq::Agent, url: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let mut res = agent
        .post(url)
        .header("User-Agent", USER_AGENT)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("{url}: {e}"))?;
    let status = res.status();
    let text = res.body_mut().read_to_string().map_err(|e| format!("{url}: {e}"))?;
    if !status.is_success() {
        return Err(format!("{url}: HTTP {}", status.as_u16()));
    }
    serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))
}

pub type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// Opens a WebSocket whose reads give up after `tick`, so the caller can look at its stop flag.
pub fn connect(url: &str, tick: Duration) -> Result<Socket, String> {
    connect_with(url, &[], tick)
}

/// `connect`, with headers of its own (a subprotocol, an origin).
pub fn connect_with(url: &str, headers: &[(&'static str, &str)], tick: Duration) -> Result<Socket, String> {
    let mut request = tungstenite::client::IntoClientRequest::into_client_request(url).map_err(|e| e.to_string())?;
    request.headers_mut().insert("User-Agent", USER_AGENT.parse().unwrap());
    for (k, v) in headers {
        request.headers_mut().insert(*k, v.parse().map_err(|_| format!("bad header {k}"))?);
    }
    let (socket, _) = tungstenite::connect(request).map_err(|e| format!("{url}: {e}"))?;
    let tcp = match socket.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::NativeTls(s) => s.get_ref(),
        _ => return Err("unexpected stream".into()),
    };
    tcp.set_read_timeout(Some(tick)).map_err(|e| e.to_string())?;
    Ok(socket)
}

/// The next message, `None` when the read timed out (nothing yet).
pub fn read(socket: &mut Socket) -> Result<Option<Message>, String> {
    match socket.read() {
        Ok(m) => Ok(Some(m)),
        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// `{"a":{"b":…}}` → value at "a.b" (array indexes as numbers).
pub fn at<'a>(v: &'a serde_json::Value, path: &str) -> &'a serde_json::Value {
    path.split('.').fold(v, |v, k| match k.parse::<usize>() {
        Ok(i) => &v[i],
        Err(_) => &v[k],
    })
}

/// A JSON text object's text: `simpleText`, or its `runs` joined.
pub fn yt_text(v: &serde_json::Value) -> String {
    if let Some(s) = v["simpleText"].as_str() {
        return s.to_string();
    }
    v["runs"].as_array().map_or_else(String::new, |runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
}

/// The bytes at `url` (redirects followed; GitHub's release downloads), up to `limit` bytes.
pub fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .timeout_global(Some(Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = agent.get(url).header("User-Agent", USER_AGENT).call().map_err(|e| format!("{url}: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("{url}: HTTP {}", res.status().as_u16()));
    }
    res.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("{url}: {e}"))
}
