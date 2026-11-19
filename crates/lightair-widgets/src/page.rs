//! A widget's page: a built-in widget's own, or an imported StreamElements widget's HTML, CSS
//! and JS put together with its `{{field}}`s filled in, the bridge and the StreamElements shim.

use serde_json::{Map, Value};

use crate::{Instance, catalog, fields};

pub const BRIDGE: &str = include_str!("../assets/lightair.js");
pub const SE_SHIM: &str = include_str!("../assets/se-shim.js");
pub const SL_SHIM: &str = include_str!("../assets/sl-shim.js");

/// Whether a custom widget was written for Streamlabs (it listens on `document` for `onLoad` or
/// `onEventReceived`) rather than StreamElements (on `window`, `onWidgetLoad`).
pub fn is_streamlabs(js: &str) -> bool {
    let squashed: String = js.chars().filter(|c| !c.is_whitespace()).collect();
    !squashed.contains("onWidgetLoad")
        && ["document.addEventListener('onLoad'", "document.addEventListener(\"onLoad\"", "document.addEventListener('onEventReceived'", "document.addEventListener(\"onEventReceived\""]
            .iter()
            .any(|p| squashed.contains(p))
}
pub const JQUERY: &str = include_str!("../assets/jquery-3.7.1.min.js");

/// `{{name}}` (spaces allowed inside) → the field's value.
pub fn substitute(text: &str, values: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        match values.get(name) {
            Some(Value::String(s)) => out.push_str(s),
            Some(Value::Null) | None if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') => {}
            Some(v) => out.push_str(&v.to_string()),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

pub fn build(i: &Instance) -> String {
    let Some(c) = &i.custom else {
        return catalog::page(&i.kind).unwrap_or("<!doctype html><title>Widget</title>").to_string();
    };
    // a built-in widget made one's own uses the bridge as it always did; an imported one may
    // have been written for StreamElements (with jQuery) or Streamlabs
    let imported = catalog::page(&i.kind).is_none();
    let values = fields::values(&fields::parse(&c.fields), &i.fields);
    format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8">
<style>html, body {{ margin: 0; background: transparent; overflow: hidden; }}</style>
<style>{css}</style>
{jquery}<script src="/lightair.js"></script>
{shim}</head><body>
{html}
<script>{js}</script>
</body></html>"#,
        shim = match (imported, is_streamlabs(&c.js)) {
            (false, _) => String::new(),
            (true, true) => "<script src=\"/sl-shim.js\"></script>\n".into(),
            (true, false) => "<script src=\"/se-shim.js\"></script>\n".into(),
        },
        css = substitute(&c.css, &values),
        jquery = if imported { format!("<script>{JQUERY}</script>\n") } else { String::new() },
        html = substitute(&c.html, &values),
        js = substitute(&c.js, &values),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streamlabs_widgets_are_told_apart() {
        assert!(is_streamlabs("document.addEventListener('onEventReceived', function (obj) { obj.detail })"));
        assert!(is_streamlabs("document.addEventListener( \"onLoad\" , (o) => {})"));
        assert!(!is_streamlabs("window.addEventListener('onEventReceived', function (obj) {}); window.addEventListener('onWidgetLoad', f)"));
    }

    #[test]
    fn fields_fill_in() {
        let mut v = Map::new();
        v.insert("color".into(), Value::String("#ff0".into()));
        v.insert("size".into(), Value::from(24));
        assert_eq!(substitute("a{ color: {{color}}; font-size: {{ size }}px }", &v), "a{ color: #ff0; font-size: 24px }");
        // unknown names are left as they were written when they aren't names at all
        assert_eq!(substitute("x {{ a b }} y", &v), "x {{ a b }} y");
        assert_eq!(substitute("x {{missing}} y", &v), "x  y");
    }
}
