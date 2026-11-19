//! The widget server: on this machine only (127.0.0.1), every widget's page at
//! `/w/<id>/?key=<key>` (a key per widget, so no other page can open it or listen in), its event
//! feed at `/w/<id>/ws`, the bridge at `/lightair.js`. Lightair's Chromium shows the pages on the
//! canvas; OBS or anything else on this machine can load the same address.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use crossbeam_channel::{Receiver, Sender, unbounded};
use lightair_live::{ChatEvent, Hub};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Error, Message};

use crate::http::{self, Request};
use crate::session::Session;
use crate::{Instance, catalog, fields, page, store};

/// The port asked for first (OBS's remote API is 4455, Lightair's 4455 too); the next free
/// one after it otherwise.
pub const PORT: u16 = 4460;

/// Alerts waiting longer than this are dropped (the alert box wasn't on air).
const STALE: Duration = Duration::from_secs(120);
/// At most this many alerts wait.
const MAX_WAITING: usize = 30;

/// An icon for a platform (its id → SVG), from the app's own logos.
pub type Icons = Box<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

struct Conn {
    id: u64,
    instance: String,
    tx: Sender<String>,
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<(Instant, ChatEvent)>,
    /// The alert showing, until when at the latest (it says when it's done).
    busy_until: Option<Instant>,
}

pub(crate) struct Shared {
    port: u16,
    hub: Hub,
    store_dir: PathBuf,
    instances: Mutex<HashMap<String, Instance>>,
    conns: Mutex<Vec<Conn>>,
    session: Mutex<Session>,
    /// The same totals, per platform (for widgets that listen to some platforms only).
    sessions: Mutex<HashMap<String, Session>>,
    queues: Mutex<HashMap<String, Queue>>,
    icons: Mutex<Option<Icons>>,
    next_conn: AtomicU64,
    /// Files pages may load besides their settings' (sounds automations play).
    files: Mutex<std::collections::HashSet<String>>,
    /// Live data from integrations ("now_playing", …), sent to every page.
    data: Mutex<Map<String, Value>>,
    /// What `/run/<name>` does (the app runs an automation), and its key.
    run: Mutex<Option<(String, RunHook)>>,
    /// Changes a viewer's points for pages (a prediction): (platform, id, name, delta) → balance,
    /// or None when they haven't enough.
    points: Mutex<Option<PointsHook>>,
}

pub type PointsHook = Box<dyn Fn(&str, &str, &str, f64) -> Option<f64> + Send + Sync>;

/// Runs an automation by name or id (the web address's way in).
pub type RunHook = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// The widget server (cheap to clone: one server).
#[derive(Clone)]
pub struct Widgets {
    shared: Arc<Shared>,
}

impl Widgets {
    /// Starts serving (and listening to `hub`); `store_dir` keeps what widgets store.
    pub fn start(hub: Hub, store_dir: PathBuf) -> io::Result<Widgets> {
        let listener = (PORT..PORT + 20)
            .find_map(|p| TcpListener::bind(("127.0.0.1", p)).ok())
            .map_or_else(|| TcpListener::bind(("127.0.0.1", 0)), Ok)?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            port,
            hub: hub.clone(),
            store_dir,
            instances: Mutex::new(HashMap::new()),
            conns: Mutex::new(Vec::new()),
            session: Mutex::new(Session::default()),
            sessions: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            icons: Mutex::new(None),
            next_conn: AtomicU64::new(1),
            files: Mutex::new(Default::default()),
            data: Mutex::new(Map::new()),
            run: Mutex::new(None),
            points: Mutex::new(None),
        });
        tracing::info!(port, "widgets served");
        {
            let shared = shared.clone();
            std::thread::Builder::new().name("lightair-widgets".into()).spawn(move || {
                for stream in listener.incoming().flatten() {
                    let shared = shared.clone();
                    let _ = std::thread::Builder::new().name("lightair-widget-conn".into()).spawn(move || {
                        let _ = serve(stream, &shared);
                    });
                }
            })?;
        }
        // the event bus → every widget
        {
            let (wake, woken) = crossbeam_channel::bounded::<()>(1);
            hub.on_change(move || {
                let _ = wake.try_send(());
            });
            let shared = shared.clone();
            let mut last = hub.history().last().map_or(0, |e| e.id);
            std::thread::Builder::new().name("lightair-widget-events".into()).spawn(move || {
                while woken.recv().is_ok() {
                    for e in shared.hub.since(last) {
                        last = e.id;
                        shared.event(&e);
                    }
                }
            })?;
        }
        // the alert queues
        {
            let shared = shared.clone();
            std::thread::Builder::new().name("lightair-widget-alerts".into()).spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    shared.tick();
                }
            })?;
        }
        Ok(Widgets { shared })
    }

    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// The address a widget is shown at.
    pub fn url(&self, id: &str, key: &str) -> String {
        format!("http://127.0.0.1:{}/w/{id}/?key={key}", self.shared.port)
    }

    /// The widgets the workspace has (their settings); open ones get changed settings at once.
    pub fn set_instances(&self, list: Vec<Instance>) {
        let (mut changed, mut recoded) = (Vec::new(), Vec::new());
        {
            let mut instances = self.shared.instances.lock();
            let mut next = HashMap::new();
            for i in list {
                match instances.get(&i.id) {
                    // its code changed (made its own, edited, put back): the page loads again
                    Some(old) if !code_matches(&old.custom, &i.custom) || old.platforms != i.platforms => recoded.push(i.id.clone()),
                    Some(old) if old.fields == i.fields => {}
                    _ => changed.push(i.id.clone()),
                }
                next.insert(i.id.clone(), i);
            }
            *instances = next;
        }
        for id in changed {
            let Some(values) = self.shared.values(&id) else { continue };
            self.shared.send_to(&id, &json!({"type": "fields", "fields": values}).to_string());
        }
        for id in recoded {
            self.shared.send_to(&id, &json!({"type": "reload"}).to_string());
        }
    }

    /// A widget's button (a field of type "button") was pressed: a built-in widget's changes
    /// its storage here; any other's goes to its page.
    pub fn button(&self, id: &str, name: &str) {
        let kind = self.shared.instances.lock().get(id).map(|i| i.kind.clone()).unwrap_or_default();
        let values = self.shared.values(id).unwrap_or_default();
        let stored = store::load(&self.shared.store_dir, id);
        match catalog::button(&kind, name, &values, &stored) {
            Some(changes) => self.shared.store_many(id, changes),
            None => self.shared.send_to(id, &json!({"type": "button", "name": name}).to_string()),
        }
    }

    /// A test event, as if read from `platform` (it reaches automations too).
    pub fn test(&self, kind: &str, platform: &str) {
        let e = crate::test_events::make(kind, platform);
        let channel = lightair_live::Channel { platform: platform.into(), channel: "test".into() };
        self.shared.hub.inject(channel, e);
    }

    /// Shows an event's alert again.
    pub fn replay(&self, e: &ChatEvent) {
        let mut e = e.clone();
        e.test = true;
        self.shared.event(&e);
    }

    /// Ends the alert showing now (the next one follows).
    pub fn skip(&self) {
        let boxes: Vec<String> = self.shared.instances.lock().values().filter(|i| catalog::queues_alerts(&i.kind)).map(|i| i.id.clone()).collect();
        for id in boxes {
            self.shared.send_to(&id, r#"{"type":"skip"}"#);
        }
    }

    fn alert_boxes(&self) -> Vec<String> {
        self.shared.instances.lock().values().filter(|i| catalog::queues_alerts(&i.kind)).map(|i| i.id.clone()).collect()
    }

    /// Says `text` on stream (through the alert boxes, whose sound is in the mix). False when
    /// there's no alert box on screen to say it.
    pub fn speak(&self, text: &str) -> bool {
        let shown: Vec<String> = self.shared.conns.lock().iter().map(|c| c.instance.clone()).collect();
        let boxes: Vec<String> = self.alert_boxes().into_iter().filter(|b| shown.contains(b)).collect();
        for id in &boxes {
            self.shared.send_to(id, &json!({"type": "speak", "text": text}).to_string());
        }
        !boxes.is_empty()
    }

    /// Plays a sound file (a path on this machine, or an address) on stream, like `speak`.
    pub fn sound(&self, file: &str, volume: f64) -> bool {
        if !file.contains("://") {
            self.shared.files.lock().insert(file.to_string());
        }
        let shown: Vec<String> = self.shared.conns.lock().iter().map(|c| c.instance.clone()).collect();
        let boxes: Vec<String> = self.alert_boxes().into_iter().filter(|b| shown.contains(b)).collect();
        for id in &boxes {
            self.shared.send_to(id, &json!({"type": "sound", "file": file, "volume": volume}).to_string());
        }
        !boxes.is_empty()
    }

    /// Live data for pages (`key`: "now_playing", …): sent when it changes.
    pub fn set_data(&self, key: &str, value: Value) {
        {
            let mut data = self.shared.data.lock();
            if data.get(key) == Some(&value) {
                return;
            }
            data.insert(key.into(), value.clone());
        }
        let m = json!({"type": "data", "key": key, "value": value}).to_string();
        for c in self.shared.conns.lock().iter() {
            let _ = c.tx.send(m.clone());
        }
    }

    pub fn data(&self, key: &str) -> Value {
        self.shared.data.lock().get(key).cloned().unwrap_or(Value::Null)
    }

    /// Lets `/run/<automation>?key=<key>` run automations (`key`: a secret of this computer's).
    pub fn set_run_hook(&self, key: String, hook: RunHook) {
        *self.shared.run.lock() = Some((key, hook));
    }

    /// Lets pages change viewers' points (`lightair.points`).
    pub fn set_points_hook(&self, hook: PointsHook) {
        *self.shared.points.lock() = Some(hook);
    }

    /// The web address that runs `automation` (an id or name).
    pub fn run_url(&self, automation: &str) -> Option<String> {
        let run = self.shared.run.lock();
        let (key, _) = run.as_ref()?;
        Some(format!("http://127.0.0.1:{}/run/{}?key={key}", self.shared.port, percent_encode(automation)))
    }

    /// A widget of `kind` exists (integrations only ask for what one shows).
    pub fn has_kind(&self, kind: &str) -> bool {
        self.shared.instances.lock().values().any(|i| i.kind == kind)
    }

    /// The widgets there are: (id, kind, name of the kind).
    pub fn list(&self) -> Vec<(String, String)> {
        self.shared.instances.lock().values().map(|i| (i.id.clone(), i.kind.clone())).collect()
    }

    /// Alerts waiting (all alert boxes).
    pub fn waiting(&self) -> usize {
        self.shared.queues.lock().values().map(|q| q.waiting.len()).max().unwrap_or(0)
    }

    /// The session's totals.
    pub fn session(&self) -> Value {
        self.shared.session.lock().to_json()
    }

    /// Where platform icons come from.
    pub fn set_icons(&self, icons: Icons) {
        *self.shared.icons.lock() = Some(icons);
    }

    /// What a widget stored (counters, timers).
    pub fn storage(&self, id: &str) -> Map<String, Value> {
        store::load(&self.shared.store_dir, id)
    }

    /// Changes what a widget stored, and tells it.
    pub fn store(&self, id: &str, key: &str, value: Value) {
        let all = store::set(&self.shared.store_dir, id, key, value);
        self.shared.send_to(id, &json!({"type": "storage", "storage": all}).to_string());
    }
}

impl Shared {
    fn values(&self, id: &str) -> Option<Map<String, Value>> {
        let instances = self.instances.lock();
        let i = instances.get(id)?;
        Some(fields::values(&fields::parse(i.schema()), &i.fields))
    }

    fn store_many(&self, id: &str, changes: Vec<(String, Value)>) {
        let mut all = Map::new();
        for (k, v) in changes {
            all = store::set(&self.store_dir, id, &k, v);
        }
        self.send_to(id, &json!({"type": "storage", "storage": all}).to_string());
    }

    fn send_to(&self, instance: &str, text: &str) {
        for c in self.conns.lock().iter().filter(|c| c.instance == instance) {
            let _ = c.tx.send(text.to_string());
        }
    }

    /// A widget's session totals: its platforms' (every platform's, when it listens to all).
    fn session_for(&self, id: &str) -> Value {
        let platforms = self.instances.lock().get(id).map(|i| i.platforms.clone()).unwrap_or_default();
        if platforms.is_empty() {
            return self.session.lock().to_json();
        }
        let sessions = self.sessions.lock();
        Session::merged(platforms.iter().filter_map(|p| sessions.get(p))).to_json()
    }

    fn event(&self, e: &ChatEvent) {
        let counted = self.session.lock().add(e);
        self.sessions.lock().entry(e.platform.clone()).or_default().add(e);
        let event = json!({"type": "event", "event": e}).to_string();
        // each widget that listens to its platform: the event, and its totals when they changed
        let hears: HashMap<String, bool> = self.instances.lock().iter().map(|(id, i)| (id.clone(), i.hears(&e.platform))).collect();
        let mut totals: HashMap<String, String> = HashMap::new();
        for c in self.conns.lock().iter() {
            if !hears.get(&c.instance).copied().unwrap_or(true) {
                continue;
            }
            let _ = c.tx.send(event.clone());
            if counted && e.kind != lightair_live::Kind::Message {
                let m = totals
                    .entry(c.instance.clone())
                    .or_insert_with(|| json!({"type": "session", "session": self.session_for(&c.instance)}).to_string());
                let _ = c.tx.send(m.clone());
            }
        }
        if e.alert_kind().is_none() || e.deleted {
            return;
        }
        // subathon timers count on whether or not they're on screen
        let timers: Vec<String> = self.instances.lock().values().filter(|i| i.kind == "subathon" && i.hears(&e.platform)).map(|i| i.id.clone()).collect();
        for id in timers {
            let values = self.values(&id).unwrap_or_default();
            if let Some(changes) = catalog::subathon_event(&values, &store::load(&self.store_dir, &id), e) {
                self.store_many(&id, changes);
            }
        }
        // the alert boxes' queues
        let alert_boxes: Vec<String> = self.instances.lock().values().filter(|i| catalog::queues_alerts(&i.kind) && i.hears(&e.platform)).map(|i| i.id.clone()).collect();
        let mut queues = self.queues.lock();
        for id in alert_boxes {
            let q = queues.entry(id).or_default();
            q.waiting.push_back((Instant::now(), e.clone()));
            while q.waiting.len() > MAX_WAITING {
                q.waiting.pop_front();
            }
        }
    }

    fn tick(&self) {
        let now = Instant::now();
        let shown: Vec<String> = self.conns.lock().iter().map(|c| c.instance.clone()).collect();
        let mut sends = Vec::new();
        {
            let mut queues = self.queues.lock();
            for (id, q) in queues.iter_mut() {
                q.waiting.retain(|(at, _)| now.duration_since(*at) < STALE);
                if q.busy_until.is_some_and(|t| t > now) || !shown.contains(id) {
                    continue;
                }
                q.busy_until = None;
                if let Some((_, e)) = q.waiting.pop_front() {
                    let secs = self.values(id).and_then(|v| v.get("duration").and_then(Value::as_f64)).unwrap_or(8.0);
                    q.busy_until = Some(now + Duration::from_secs_f64(secs.clamp(1.0, 120.0) + 3.0));
                    sends.push((id.clone(), json!({"type": "alert", "event": e}).to_string()));
                }
            }
        }
        for (id, m) in sends {
            self.send_to(&id, &m);
        }
    }

    fn done(&self, instance: &str) {
        if let Some(q) = self.queues.lock().get_mut(instance) {
            // a short gap between alerts
            q.busy_until = Some(Instant::now() + Duration::from_millis(600));
        }
    }

    fn load_message(&self, id: &str) -> String {
        let history = self.hub.history();
        let instance = self.instances.lock().get(id).cloned();
        let hears = |e: &ChatEvent| instance.as_ref().is_none_or(|i| i.hears(&e.platform));
        let mut recents: Vec<&ChatEvent> = history.iter().rev().filter(|e| !e.deleted && hears(e)).take(100).collect();
        recents.reverse();
        json!({
            "type": "load",
            "id": id,
            "fields": self.values(id).unwrap_or_default(),
            "session": self.session_for(id),
            "recents": recents,
            "storage": store::load(&self.store_dir, id),
            "data": self.data.lock().clone(),
        })
        .to_string()
    }
}

fn query(path: &str, name: &str) -> Option<String> {
    let q = path.split_once('?')?.1;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then(|| percent_decode(v))
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("apng") => "image/apng",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg" | "oga") => "audio/ogg",
        Some("m4a") => "audio/mp4",
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        _ => "application/octet-stream",
    }
}

fn serve(mut stream: TcpStream, shared: &Arc<Shared>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    while let Some(req) = http::read_request(&mut stream)? {
        if !http::local_host(req.header("host")) {
            return http::respond(&mut stream, 403, "text/plain", b"not here", &[]);
        }
        if req.method != "GET" {
            return http::respond(&mut stream, 405, "text/plain", b"GET only", &[]);
        }
        let path = req.path.split('?').next().unwrap_or("/").to_string();
        tracing::debug!(%path, "widget request");
        let no_cache = [("Cache-Control", "no-store")];
        match path.as_str() {
            "/lightair.js" => http::respond(&mut stream, 200, "text/javascript; charset=utf-8", page::BRIDGE.as_bytes(), &no_cache)?,
            "/jquery.js" => http::respond(&mut stream, 200, "text/javascript; charset=utf-8", page::JQUERY.as_bytes(), &[])?,
            "/se-shim.js" => http::respond(&mut stream, 200, "text/javascript; charset=utf-8", page::SE_SHIM.as_bytes(), &no_cache)?,
            "/sl-shim.js" => http::respond(&mut stream, 200, "text/javascript; charset=utf-8", page::SL_SHIM.as_bytes(), &no_cache)?,
            p if p.starts_with("/run/") => {
                let name = percent_decode(&p[5..]);
                let given = query(&req.path, "key").unwrap_or_default();
                let answer = match shared.run.lock().as_ref() {
                    Some((key, hook)) if *key == given => hook(&name).map(|_| "ran".to_string()),
                    _ => Err("not allowed".to_string()),
                };
                match answer {
                    Ok(t) => http::respond(&mut stream, 200, "text/plain", t.as_bytes(), &[])?,
                    Err(e) => http::respond(&mut stream, 403, "text/plain", e.as_bytes(), &[])?,
                }
            }
            p if p.starts_with("/icons/") => {
                let platform = p.trim_start_matches("/icons/").trim_end_matches(".svg");
                let svg = shared.icons.lock().as_ref().and_then(|f| f(platform));
                match svg {
                    Some(svg) => http::respond(&mut stream, 200, "image/svg+xml", &svg, &[("Cache-Control", "max-age=86400")])?,
                    None => http::respond(&mut stream, 404, "text/plain", b"no icon", &[])?,
                }
            }
            p if p.starts_with("/w/") => {
                let rest = &p[3..];
                let (id, sub) = rest.split_once('/').unwrap_or((rest, ""));
                let instance = shared.instances.lock().get(id).cloned();
                let Some(instance) = instance.filter(|i| query(&req.path, "key").as_deref() == Some(i.key.as_str())) else {
                    return http::respond(&mut stream, 404, "text/plain", b"no such widget", &[]);
                };
                match sub {
                    "" | "index.html" => {
                        let html = page::build(&instance);
                        http::respond(&mut stream, 200, "text/html; charset=utf-8", html.as_bytes(), &no_cache)?;
                    }
                    "ws" => {
                        let ws = accept(stream, &req)?;
                        return feed(ws, shared, &instance.id);
                    }
                    "file" => {
                        // only a file one of its settings names
                        let wanted = query(&req.path, "path").unwrap_or_default();
                        let named = shared.values(&instance.id).is_some_and(|v| v.values().any(|x| x.as_str() == Some(wanted.as_str())))
                            || shared.files.lock().contains(&wanted);
                        let path = PathBuf::from(&wanted);
                        match std::fs::read(&path).ok().filter(|_| named && path.is_absolute()) {
                            Some(bytes) => http::respond(&mut stream, 200, content_type(&path), &bytes, &[])?,
                            None => http::respond(&mut stream, 404, "text/plain", b"no such file", &[])?,
                        }
                    }
                    _ => http::respond(&mut stream, 404, "text/plain", b"not found", &[])?,
                }
            }
            _ => http::respond(&mut stream, 404, "text/plain", b"not found", &[])?,
        }
        if !req.keep_alive() {
            break;
        }
    }
    Ok(())
}

fn accept(mut stream: TcpStream, req: &Request) -> io::Result<WebSocket<TcpStream>> {
    let key = req.header("sec-websocket-key").ok_or_else(|| io::Error::other("no Sec-WebSocket-Key"))?;
    let mut sha = sha1_smol::Sha1::new();
    sha.update(key.as_bytes());
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept = base64::engine::general_purpose::STANDARD.encode(sha.digest().bytes());
    let head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.set_read_timeout(Some(Duration::from_millis(50)))?;
    Ok(WebSocket::from_raw_socket(stream, Role::Server, None))
}

fn feed(mut ws: WebSocket<TcpStream>, shared: &Arc<Shared>, instance: &str) -> io::Result<()> {
    let (tx, rx): (Sender<String>, Receiver<String>) = unbounded();
    let conn = shared.next_conn.fetch_add(1, Ordering::Relaxed);
    let _ = ws.send(Message::text(shared.load_message(instance)));
    let me = tx.clone();
    shared.conns.lock().push(Conn { id: conn, instance: instance.to_string(), tx });
    let result = (|| -> io::Result<()> {
        loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    let Ok(m) = serde_json::from_str::<Value>(t.as_str()) else { continue };
                    match m["type"].as_str() {
                        Some("done") => shared.done(instance),
                        Some("points") => {
                            let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                            let delta = m["delta"].as_f64().unwrap_or(0.0);
                            let balance = shared.points.lock().as_ref().and_then(|hook| hook(&s("platform"), &s("id"), &s("name"), delta));
                            let _ = me.send(json!({"type": "points", "ref": m["ref"], "ok": balance.is_some(), "balance": balance}).to_string());
                        }
                        Some("storage.set") => {
                            if let Some(key) = m["key"].as_str() {
                                store::set(&shared.store_dir, instance, key, m["value"].clone());
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) => return Ok(()),
                Ok(_) => {}
                Err(Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(Error::ConnectionClosed | Error::AlreadyClosed) => return Ok(()),
                Err(e) => return Err(io::Error::other(e.to_string())),
            }
            while let Ok(m) = rx.try_recv() {
                ws.send(Message::text(m)).map_err(|e| io::Error::other(e.to_string()))?;
            }
            match ws.flush() {
                Ok(()) => {}
                Err(Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(Error::ConnectionClosed | Error::AlreadyClosed) => return Ok(()),
                Err(e) => return Err(io::Error::other(e.to_string())),
            }
        }
    })();
    shared.conns.lock().retain(|c| c.id != conn);
    result
}

/// Whether two widgets' code is the same (where it's kept aside).
fn code_matches(a: &Option<crate::Custom>, b: &Option<crate::Custom>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.same_code(b),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widgets_get_only_their_platforms_events() {
        let dir = std::env::temp_dir().join(format!("lightair-widget-store-{}", std::process::id()));
        let w = Widgets::start(Hub::new(), dir.clone()).unwrap();
        let every = crate::Instance::new("label", None);
        let mut youtube = crate::Instance::new("label", None);
        youtube.platforms = vec!["youtube".into()];
        w.set_instances(vec![every.clone(), youtube.clone()]);
        // a page open for each
        let (tx_every, rx_every) = crossbeam_channel::unbounded();
        let (tx_youtube, rx_youtube) = crossbeam_channel::unbounded();
        w.shared.conns.lock().push(Conn { id: 1, instance: every.id.clone(), tx: tx_every });
        w.shared.conns.lock().push(Conn { id: 2, instance: youtube.id.clone(), tx: tx_youtube });
        let follow = |platform: &str, name: &str| {
            let mut e = ChatEvent::new(platform, "c", lightair_live::Kind::Follow);
            e.user = Some(lightair_live::User { display_name: name.into(), ..Default::default() });
            e
        };
        w.shared.event(&follow("twitch", "tw"));
        w.shared.event(&follow("youtube", "yt"));
        let got = |rx: &crossbeam_channel::Receiver<String>| -> Vec<Value> { rx.try_iter().map(|m| serde_json::from_str(&m).unwrap()).collect() };
        let (all, yt) = (got(&rx_every), got(&rx_youtube));
        let events = |ms: &[Value]| ms.iter().filter(|m| m["type"] == "event").map(|m| m["event"]["platform"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(events(&all), ["twitch", "youtube"]);
        assert_eq!(events(&yt), ["youtube"]);
        // totals: every platform's, or its own
        let last = |ms: &[Value]| ms.iter().rev().find(|m| m["type"] == "session").unwrap()["session"].clone();
        assert_eq!(last(&all)["counts"]["follow"], 2);
        assert_eq!(last(&yt)["counts"]["follow"], 1);
        assert_eq!(last(&yt)["latest"]["follow"]["name"], "yt");
        // its load: its own totals and recent events
        assert_eq!(serde_json::from_str::<Value>(&w.shared.load_message(&youtube.id)).unwrap()["session"]["counts"]["follow"], 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn queries_decode() {
        assert_eq!(query("/w/a/file?key=k&path=%2FUsers%2Fme%2Fa%20b.png", "path").as_deref(), Some("/Users/me/a b.png"));
        assert_eq!(query("/w/a/?key=abc", "key").as_deref(), Some("abc"));
        assert_eq!(query("/w/a/", "key"), None);
    }
}
