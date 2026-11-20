//! Choosing what a screen capture shows (Add object › Display, Window, App). A background
//! thread lists the choices (asking the system takes it a moment, so the sheet doesn't wait).
//! Displays each get a live thumbnail as their capture opens; windows and apps, many more of
//! them, are a plain list (capturing each costs too much).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightair_model::kinds::{APP, DISPLAY, WINDOW};
use lightair_engine::Engine;
use lightair_engine::obs::ObsSource;
use lightair_preview::{Frame, Preview};
use parking_lot::Mutex;
use serde_json::Value;

/// Thumbnails fit in this (pixels).
const THUMB: (u32, u32) = (400, 250);

/// One choice: what it is, what it belongs to or measures, and its setting.
#[derive(Clone)]
pub(super) struct Item {
    pub title: String,
    pub detail: String,
    pub value: Value,
    /// The main display.
    pub main: bool,
}

/// What the lister thread has found so far.
#[derive(Default)]
struct Found {
    listed: bool,
    items: Vec<Item>,
    /// Captures, by item, as they're opened (taken by the picker).
    captures: Vec<Option<ObsSource>>,
}

/// A display's thumbnail.
#[derive(Default)]
pub(super) struct Thumb {
    preview: Option<Preview>,
    capture: Option<ObsSource>,
    /// Its picture's size, once known (the capture's own).
    pub size: Option<(u32, u32)>,
}

impl Thumb {
    /// The live picture, once the capture has one.
    pub fn frame(&self) -> Option<Frame> {
        self.preview.as_ref().filter(|_| self.size.is_some()).and_then(|p| p.latest_frame())
    }
}

pub(super) struct ScreenPick {
    pub mode: i64,
    found: Arc<Mutex<Found>>,
    cancel: Arc<AtomicBool>,
    pub thumbs: Vec<Thumb>,
    pub items: Vec<Item>,
    pub listed: bool,
    engine: Arc<Engine>,
}

/// The portable setting each mode chooses.
pub(super) fn key_of(mode: i64) -> &'static str {
    match mode {
        DISPLAY => "display",
        APP => "app",
        _ => "window",
    }
}

/// `w × h` fitted in [`THUMB`] (even numbers).
fn fit((w, h): (u32, u32)) -> (u32, u32) {
    let s = (THUMB.0 as f32 / w.max(1) as f32).min(THUMB.1 as f32 / h.max(1) as f32).min(1.0);
    (((w as f32 * s) as u32).max(2), ((h as f32 * s) as u32).max(2))
}

impl ScreenPick {
    /// Starts listing `mode`'s choices (DISPLAY, WINDOW or APP) and opening their captures.
    pub fn open(mode: i64, engine: &Arc<Engine>) -> Self {
        let found = Arc::new(Mutex::new(Found::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (f, stop, eng) = (found.clone(), cancel.clone(), engine.clone());
        std::thread::Builder::new()
            .name("lightair-screen-pick".into())
            .spawn(move || {
                let key = key_of(mode);
                let options = lightair_engine::displays::screen_options(mode, key);
                let hide = |v: &Value| {
                    lightair_platform::apps::hide_window(
                        &v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()),
                    )
                };
                let main = lightair_engine::displays::main_display_uuid();
                let items: Vec<Item> = super::source::choices(key, &options, |v| mode != DISPLAY && hide(v))
                    .into_iter()
                    .map(|c| {
                        let primary = options.iter().any(|(label, v)| *v == c.value && label.contains("Primary"));
                        let main =
                            mode == DISPLAY && (primary || (main.is_some() && c.value.as_str() == main.as_deref()));
                        Item { title: c.title, detail: c.detail, value: c.value, main }
                    })
                    .collect();
                {
                    let mut f = f.lock();
                    f.captures = (0..items.len()).map(|_| None).collect();
                    f.items = items.clone();
                    f.listed = true;
                }
                // only displays get thumbnails
                for (i, item) in items.iter().enumerate().filter(|_| mode == DISPLAY) {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let capture = lightair_engine::displays::screen_capture(mode, key, &item.value, &item.title);
                    let mut f = f.lock();
                    if stop.load(Ordering::Relaxed) {
                        // the picker has gone: let it go where libobs's other work happens
                        eng.post(move |_| drop(capture));
                        break;
                    }
                    f.captures[i] = capture;
                }
            })
            .ok();
        Self { mode, found, cancel, thumbs: vec![], items: vec![], listed: false, engine: engine.clone() }
    }

    /// Takes what the lister has found, starts thumbnails for new captures and sizes them to
    /// their pictures. Called each frame while the step shows.
    pub fn tick(&mut self) {
        {
            let mut f = self.found.lock();
            self.listed = f.listed;
            if self.items.len() < f.items.len() {
                self.items = f.items.clone();
            }
            self.thumbs.resize_with(f.items.len(), Thumb::default);
            for (thumb, capture) in self.thumbs.iter_mut().zip(f.captures.iter_mut()) {
                if let Some(c) = capture.take() {
                    let preview = Preview::attach();
                    preview.set_own_size(true);
                    preview.set_size(Some(THUMB));
                    preview.set_source(Some(unsafe { lightair_engine::sys::obs_source_get_ref(c.as_ptr()) }));
                    thumb.preview = Some(preview);
                    thumb.capture = Some(c);
                }
            }
        }
        for thumb in &mut self.thumbs {
            let (Some(c), Some(preview)) = (&thumb.capture, &thumb.preview) else { continue };
            let (w, h) = unsafe {
                (
                    lightair_engine::sys::obs_source_get_width(c.as_ptr()),
                    lightair_engine::sys::obs_source_get_height(c.as_ptr()),
                )
            };
            if w > 0 && h > 0 && thumb.size != Some((w, h)) {
                // drawn in its own shape
                preview.set_size(Some(fit((w, h))));
                thumb.size = Some((w, h));
            }
        }
    }
}

impl Drop for ScreenPick {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let mut captures: Vec<ObsSource> = vec![];
        for thumb in &mut self.thumbs {
            if let Some(p) = thumb.preview.take() {
                p.set_source(None);
            }
            captures.extend(thumb.capture.take());
        }
        captures.extend(self.found.lock().captures.iter_mut().filter_map(Option::take));
        // the captures go on the engine thread, with libobs's other work
        self.engine.post(move |_| drop(captures));
    }
}

/// What adding the choice in `mode` sets: the portable screen settings.
pub(super) fn settings(mode: i64, value: &Value) -> Value {
    let mut s = super::display_settings();
    s["mode"] = mode.into();
    match mode {
        DISPLAY => s["display"] = value.clone(),
        WINDOW => s["window"] = value.clone(),
        _ => s["app"] = value.clone(),
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn thumbnails_keep_the_pictures_shape() {
        assert_eq!(super::fit((1512, 982)), (384, 250));
        assert_eq!(super::fit((800, 1200)), (166, 250));
        assert_eq!(super::fit((100, 50)), (100, 50), "never enlarged");
    }
}
