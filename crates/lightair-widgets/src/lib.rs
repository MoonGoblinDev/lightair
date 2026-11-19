//! Lightair's widgets (docs/plans/live-integrations.md, 2.5): overlay pages Lightair serves on
//! this machine and shows on the canvas like any web page, fed by the event bus: chat box,
//! alert box, event list, goal, label, counter, subathon timer, and StreamElements custom widgets
//! imported as they are.
//!
//! A widget lives in its web page object's settings (`"widget": {…}`, see [`Instance`]), so it
//! is saved, undone and copied with the workspace. The app tells the server which widgets exist
//! ([`Widgets::set_instances`]); pages reach Lightair only through the bridge (`/lightair.js`).

pub mod catalog;
pub mod fields;
mod http;
pub mod page;
mod server;
pub mod session;
mod store;
pub mod test_events;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use server::{Icons, PORT, Widgets};

/// The scheme a widget's web page object loads (`lightair-widget://<id>?key=<key>`): resolved to
/// the server's address when the page opens (the port can differ between runs).
pub const SCHEME: &str = "lightair-widget://";

/// A widget: what its web page object's settings keep under `"widget"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    /// A built-in widget's kind ("alert-box", …) or "custom".
    pub kind: String,
    /// Only its own page and feed open with it.
    pub key: String,
    /// The settings changed from their defaults.
    #[serde(default)]
    pub fields: Value,
    /// An imported widget's code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<Custom>,
    /// The platforms it gets events from ("twitch", "youtube", …, the Outputs list's ids):
    /// none listed, every one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<String>,
}

/// An imported widget (StreamElements' custom widget tabs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Custom {
    #[serde(default)]
    pub html: String,
    #[serde(default)]
    pub css: String,
    #[serde(default)]
    pub js: String,
    /// Its `fields.json`, as written (its order is the Inspector's).
    #[serde(default)]
    pub fields: String,
    /// A folder on this computer its files are kept in too ([`write_folder`]): edited there,
    /// they're read back in (the app watches it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

impl Custom {
    fn empty() -> Custom {
        Custom { html: String::new(), css: String::new(), js: String::new(), fields: String::new(), folder: None }
    }

    /// The same code (ignoring where it's kept).
    pub fn same_code(&self, other: &Custom) -> bool {
        (&self.html, &self.css, &self.js, &self.fields) == (&other.html, &other.css, &other.js, &other.fields)
    }
}

/// A built-in widget's code, to change as one's own: its page's HTML, CSS and JS apart, and its
/// fields. (The widget keeps its kind, so what Lightair feeds it stays the same.)
pub fn customize(kind: &str) -> Option<Custom> {
    let b = catalog::by_kind(kind)?;
    let mut page = b.page.to_string();
    let css = take_tags(&mut page, "style").join("\n\n");
    let js = take_tags(&mut page, "script").join("\n\n");
    let body = page
        .find("<body")
        .and_then(|i| page[i..].find('>').map(|j| i + j + 1))
        .map(|start| page[start..page.rfind("</body>").unwrap_or(page.len())].to_string())
        .unwrap_or_default();
    Some(Custom { html: dedent(&body), css: dedent(&css), js: dedent(&js), fields: b.fields.to_string(), folder: None })
}

/// Takes every `<tag …>…</tag>` out of `page`, giving back what each held (a script that loads
/// a file, `<script src=…>`, just goes: the page it's put in loads the bridge itself).
fn take_tags(page: &mut String, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut found = Vec::new();
    while let Some(start) = page.find(&open) {
        let Some(head_end) = page[start..].find('>').map(|i| start + i + 1) else { break };
        let Some(end) = page[head_end..].find(&close).map(|i| head_end + i) else { break };
        if !page[start..head_end].contains("src=") {
            found.push(page[head_end..end].to_string());
        }
        page.replace_range(start..end + close.len(), "");
    }
    found
}

/// Without the indent every line shares, and the blank lines around it.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.trim_matches('\n').lines().collect();
    let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let mut out: Vec<&str> = lines.iter().map(|l| if l.len() >= indent { &l[indent..] } else { l.trim_start() }).collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Writes `c`'s files into `dir` (made if need be): `index.html`, `style.css`, `main.js`,
/// `fields.json`, the names a package uses.
pub fn write_folder(c: &Custom, dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (file, text) in [("index.html", &c.html), ("style.css", &c.css), ("main.js", &c.js), ("fields.json", &c.fields)] {
        let path = dir.join(file);
        // (unchanged files are left alone, so an editor showing them isn't told they changed)
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

impl Instance {
    pub fn from_settings(settings: &Value) -> Option<Instance> {
        serde_json::from_value(settings.get("widget")?.clone()).ok()
    }

    /// The fields' schema.
    pub fn schema(&self) -> &str {
        match &self.custom {
            Some(c) => &c.fields,
            None => catalog::by_kind(&self.kind).map_or("{}", |b| b.fields),
        }
    }

    /// A new widget of `kind`.
    pub fn new(kind: &str, custom: Option<Custom>) -> Instance {
        Instance { id: random_hex(8), kind: kind.into(), key: random_hex(16), fields: Value::Object(Default::default()), custom, platforms: Vec::new() }
    }

    /// Whether it gets events from `platform`.
    pub fn hears(&self, platform: &str) -> bool {
        self.platforms.is_empty() || self.platforms.iter().any(|p| p == platform)
    }

    /// The address its web page object loads.
    pub fn address(&self) -> String {
        format!("{SCHEME}{}?key={}", self.id, self.key)
    }
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `lightair-widget://<id>?key=…` → the server's address on `port`; anything else as it is.
pub fn resolve(url: &str, port: u16) -> String {
    match url.strip_prefix(SCHEME) {
        Some(rest) => {
            let (id, q) = rest.split_once('?').unwrap_or((rest, ""));
            format!("http://127.0.0.1:{port}/w/{id}/?{q}")
        }
        None => url.to_string(),
    }
}

/// A widget from a folder, or a `.lwidget` (or `.zip`) package of the same files.
pub fn import(path: &std::path::Path) -> Result<Custom, String> {
    if path.is_dir() {
        return import_folder(path);
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "that isn't a .lwidget (zip) package".to_string())?;
    let mut c = Custom::empty();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().rsplit('/').next().unwrap_or("").to_ascii_lowercase();
        let mut text = String::new();
        if std::io::Read::read_to_string(&mut f, &mut text).is_err() {
            continue;
        }
        if name.ends_with(".html") || name.ends_with(".htm") {
            c.html = text;
        } else if name.ends_with(".css") {
            c.css = text;
        } else if name.ends_with(".js") {
            c.js = text;
        } else if name.contains("field") && name.ends_with(".json") {
            c.fields = text;
        }
    }
    if c.html.is_empty() && c.js.is_empty() {
        return Err("no HTML or JS in that package".into());
    }
    Ok(c)
}

/// Writes `c` as a `.lwidget` package (its manifest names it `name`, `size`).
pub fn export(c: &Custom, name: &str, size: (u32, u32), path: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let manifest = serde_json::json!({"name": name, "version": "1.0.0", "size": [size.0, size.1], "format": "lightair-widget/1"});
    for (file, text) in [
        ("manifest.json", serde_json::to_string_pretty(&manifest).unwrap_or_default()),
        ("index.html", c.html.clone()),
        ("style.css", c.css.clone()),
        ("main.js", c.js.clone()),
        ("fields.json", c.fields.clone()),
    ] {
        zip.start_file(file, opts).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut zip, text.as_bytes()).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

/// An imported StreamElements widget from its folder's files (`html`, `css`, `js`, `fields`:
/// whichever exist, by extension or name).
pub fn import_folder(dir: &std::path::Path) -> Result<Custom, String> {
    let mut c = Custom::empty();
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        if name.ends_with(".html") || name.ends_with(".htm") {
            c.html = text;
        } else if name.ends_with(".css") {
            c.css = text;
        } else if name.ends_with(".js") {
            c.js = text;
        } else if name.contains("field") && name.ends_with(".json") {
            c.fields = text;
        }
    }
    if c.html.is_empty() && c.js.is_empty() {
        return Err("no HTML or JS file there".into());
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packages_round_trip() {
        let c = Custom { html: "<div>{{title}}</div>".into(), css: "div{}".into(), js: "1".into(), fields: r#"{"title":{"type":"text","value":"x"}}"#.into(), folder: None };
        let path = std::env::temp_dir().join(format!("lightair-{}.lwidget", std::process::id()));
        export(&c, "Test", (400, 200), &path).unwrap();
        assert_eq!(import(&path).unwrap(), c);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn every_built_in_widget_can_be_customized() {
        for b in catalog::BUILTIN {
            let c = customize(b.kind).unwrap();
            assert!(!c.css.is_empty() && !c.js.is_empty(), "{}: its style and script", b.kind);
            assert!(!c.html.contains("<script") && !c.html.contains("<style"), "{}: the HTML holds neither", b.kind);
            assert!(c.js.contains("lightair."), "{}: the script uses the bridge", b.kind);
            assert_eq!(c.fields, b.fields);
            // its page: the bridge once, no StreamElements shim
            let mut i = Instance::new(b.kind, Some(c));
            i.fields = serde_json::json!({});
            let page = page::build(&i);
            assert_eq!(page.matches("/lightair.js").count(), 1, "{}", b.kind);
            assert!(!page.contains("se-shim") && !page.contains("sl-shim"), "{}", b.kind);
        }
    }

    #[test]
    fn folders_round_trip() {
        let c = customize("goal").unwrap();
        let dir = std::env::temp_dir().join(format!("lightair-folder-{}", std::process::id()));
        write_folder(&c, &dir).unwrap();
        assert!(import_folder(&dir).unwrap().same_code(&c));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn addresses_resolve_and_settings_round_trip() {
        let i = Instance::new("alert-box", None);
        let url = resolve(&i.address(), 4460);
        assert_eq!(url, format!("http://127.0.0.1:4460/w/{}/?key={}", i.id, i.key));
        assert_eq!(resolve("https://example.com", 4460), "https://example.com");
        let settings = serde_json::json!({"url": i.address(), "widget": i});
        assert_eq!(Instance::from_settings(&settings), Some(i));
    }
}
