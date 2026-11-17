//! Thin owning wrappers over libobs reference-counted objects.
//!
//! These are only used on the engine thread (they are `Send` so they can live in
//! [`crate::EngineState`], which moves into the actor thread once).

use std::ffi::{CStr, CString};
use std::ptr::NonNull;

use libobs_sys as sys;

fn cstr(s: &str) -> CString {
    CString::new(s).expect("string contains NUL")
}

/// An `obs_data_t` settings object.
pub struct Data(NonNull<sys::obs_data_t>);

impl Data {
    pub fn new() -> Self {
        Self(NonNull::new(unsafe { sys::obs_data_create() }).expect("obs_data_create"))
    }
    /// Takes ownership of an `obs_data_t` reference (e.g. from `obs_data_create_from_json`).
    /// Settings from JSON (an object; anything else gives empty settings).
    pub fn from_json(value: &serde_json::Value) -> Self {
        let json = std::ffi::CString::new(value.to_string()).unwrap_or_default();
        Self::from_raw(unsafe { sys::obs_data_create_from_json(json.as_ptr()) }).unwrap_or_default()
    }

    pub fn from_raw(ptr: *mut sys::obs_data_t) -> Option<Self> {
        NonNull::new(ptr).map(Self)
    }
    pub fn set_str(&mut self, key: &str, value: &str) -> &mut Self {
        unsafe { sys::obs_data_set_string(self.as_ptr(), cstr(key).as_ptr(), cstr(value).as_ptr()) };
        self
    }
    pub fn set_int(&mut self, key: &str, value: i64) -> &mut Self {
        unsafe { sys::obs_data_set_int(self.as_ptr(), cstr(key).as_ptr(), value) };
        self
    }
    pub fn set_bool(&mut self, key: &str, value: bool) -> &mut Self {
        unsafe { sys::obs_data_set_bool(self.as_ptr(), cstr(key).as_ptr(), value) };
        self
    }
    pub fn set_f64(&mut self, key: &str, value: f64) -> &mut Self {
        unsafe { sys::obs_data_set_double(self.as_ptr(), cstr(key).as_ptr(), value) };
        self
    }
    pub fn as_ptr(&self) -> *mut sys::obs_data_t {
        self.0.as_ptr()
    }
}

impl Default for Data {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        unsafe { sys::obs_data_release(self.as_ptr()) }
    }
}

/// A strong reference to an `obs_source_t` (source, filter, transition or scene source).
pub struct ObsSource(NonNull<sys::obs_source_t>);
unsafe impl Send for ObsSource {}

impl ObsSource {
    pub fn create(type_id: &str, name: &str, settings: Option<&Data>) -> Option<Self> {
        let ptr = unsafe {
            sys::obs_source_create(
                cstr(type_id).as_ptr(),
                cstr(name).as_ptr(),
                settings.map_or(std::ptr::null_mut(), Data::as_ptr),
                std::ptr::null_mut(),
            )
        };
        NonNull::new(ptr).map(Self)
    }
    /// A private source (filters, helpers): not listed in libobs's public source table.
    pub fn create_private(type_id: &str, name: &str, settings: Option<&Data>) -> Option<Self> {
        let ptr = unsafe {
            sys::obs_source_create_private(
                cstr(type_id).as_ptr(),
                cstr(name).as_ptr(),
                settings.map_or(std::ptr::null_mut(), Data::as_ptr),
            )
        };
        NonNull::new(ptr).map(Self)
    }

    /// Takes an additional reference to a borrowed source pointer.
    ///
    /// # Safety
    /// `ptr` must be null or point to a live `obs_source_t`.
    pub unsafe fn from_borrowed(ptr: *mut sys::obs_source_t) -> Option<Self> {
        NonNull::new(unsafe { sys::obs_source_get_ref(ptr) }).map(Self)
    }
    pub fn as_ptr(&self) -> *mut sys::obs_source_t {
        self.0.as_ptr()
    }
    pub fn width(&self) -> u32 {
        unsafe { sys::obs_source_get_width(self.as_ptr()) }
    }
    pub fn height(&self) -> u32 {
        unsafe { sys::obs_source_get_height(self.as_ptr()) }
    }
    pub fn name(&self) -> String {
        let p = unsafe { sys::obs_source_get_name(self.as_ptr()) };
        if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    }
    pub fn update(&self, settings: &Data) {
        unsafe { sys::obs_source_update(self.as_ptr(), settings.as_ptr()) }
    }
    /// Replaces all settings with `settings` on top of the defaults.
    pub fn reset(&self, settings: &Data) {
        unsafe { sys::obs_source_reset_settings(self.as_ptr(), settings.as_ptr()) }
    }
}

impl Drop for ObsSource {
    fn drop(&mut self) {
        unsafe { sys::obs_source_release(self.as_ptr()) }
    }
}

/// A strong reference to an `obs_scene_t`.
pub struct ObsScene(pub(crate) NonNull<sys::obs_scene_t>);
unsafe impl Send for ObsScene {}

impl ObsScene {
    pub fn create(name: &str) -> Option<Self> {
        NonNull::new(unsafe { sys::obs_scene_create(cstr(name).as_ptr()) }).map(Self)
    }
    /// A scene of Lightair's own, not listed with the show's.
    pub fn create_private(name: &str) -> Option<Self> {
        NonNull::new(unsafe { sys::obs_scene_create_private(cstr(name).as_ptr()) }).map(Self)
    }
    pub fn as_ptr(&self) -> *mut sys::obs_scene_t {
        self.0.as_ptr()
    }
    /// The scene's own source (borrowed; valid while the scene lives).
    pub fn source_ptr(&self) -> *mut sys::obs_source_t {
        unsafe { sys::obs_scene_get_source(self.as_ptr()) }
    }
    /// Adds `source` on top of the scene and returns the scene item (owned by the scene).
    pub fn add(&self, source: &ObsSource) -> *mut sys::obs_sceneitem_t {
        unsafe { sys::obs_scene_add(self.as_ptr(), source.as_ptr()) }
    }

    /// Adds `source` scaled to fit inside the box (x, y, w, h), keeping its aspect ratio.
    pub fn add_fitted(&self, source: &ObsSource, x: f32, y: f32, w: f32, h: f32) -> *mut sys::obs_sceneitem_t {
        let item = self.add(source);
        if !item.is_null() {
            unsafe {
                sys::obs_sceneitem_set_bounds_type(item, sys::OBS_BOUNDS_SCALE_INNER);
                sys::obs_sceneitem_set_bounds_alignment(item, sys::OBS_ALIGN_CENTER);
                sys::obs_sceneitem_set_bounds(item, &vec2(w, h));
                sys::obs_sceneitem_set_pos(item, &vec2(x, y));
            }
        }
        item
    }
}

pub fn vec2(x: f32, y: f32) -> sys::vec2 {
    sys::vec2 {
        __bindgen_anon_1: sys::vec2__bindgen_ty_1 { __bindgen_anon_1: sys::vec2__bindgen_ty_1__bindgen_ty_1 { x, y } },
    }
}

/// Dropping a `Scene` deletes it. Since OBS 33 every scene belongs to a canvas that holds its
/// own reference, so the scene is removed from its canvas (`obs_source_remove`) before our
/// reference is released; otherwise it would live until `obs_shutdown` and crash there.
impl Drop for ObsScene {
    fn drop(&mut self) {
        unsafe {
            sys::obs_source_remove(self.source_ptr());
            sys::obs_scene_release(self.as_ptr());
        }
    }
}

/// An additional libobs canvas (a Lightair canvas other than the primary one). Dropping it
/// removes the canvas.
pub struct ObsCanvas(NonNull<sys::obs_canvas_t>);
unsafe impl Send for ObsCanvas {}

fn canvas_video_info(width: u32, height: u32, fps_num: u32, fps_den: u32) -> sys::obs_video_info {
    let mut ovi = sys::obs_video_info::default();
    unsafe { sys::obs_get_video_info(&mut ovi) };
    ovi.base_width = width;
    ovi.base_height = height;
    ovi.output_width = width;
    ovi.output_height = height;
    ovi.fps_num = fps_num;
    ovi.fps_den = fps_den.max(1);
    ovi
}

impl ObsCanvas {
    pub fn create(name: &str, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Option<Self> {
        let mut ovi = canvas_video_info(width, height, fps_num, fps_den);
        // Sources activate and scenes are referenced, but audio is mixed once, by the main canvas.
        let flags = sys::ACTIVATE | sys::SCENE_REF;
        NonNull::new(unsafe { sys::obs_canvas_create(cstr(name).as_ptr(), &mut ovi, flags) }).map(Self)
    }

    pub fn as_ptr(&self) -> *mut sys::obs_canvas_t {
        self.0.as_ptr()
    }

    pub fn create_scene(&self, name: &str) -> Option<ObsScene> {
        NonNull::new(unsafe { sys::obs_canvas_scene_create(self.as_ptr(), cstr(name).as_ptr()) }).map(ObsScene)
    }

    pub fn reset(&self, width: u32, height: u32, fps_num: u32, fps_den: u32) {
        let mut ovi = canvas_video_info(width, height, fps_num, fps_den);
        unsafe { sys::obs_canvas_reset_video(self.as_ptr(), &mut ovi) };
    }
}

impl Drop for ObsCanvas {
    fn drop(&mut self) {
        unsafe {
            sys::obs_canvas_remove(self.as_ptr());
            sys::obs_canvas_release(self.as_ptr());
        }
    }
}

/// The choices a source instance offers for a list setting, as (label, value), read with its
/// current settings applied (some sources fill their lists only then).
pub(crate) fn source_list_items(src: *mut sys::obs_source_t, property: &str) -> Vec<(String, serde_json::Value)> {
    // a capture run only while shown: its list is its capture's
    #[cfg(target_os = "macos")]
    if let Some(items) = crate::on_screen::with_capture(src, |capture| list_items(capture, property)) {
        return items;
    }
    // a camera that isn't running: the system's cameras
    let id = unsafe { CStr::from_ptr(sys::obs_source_get_id(src)) };
    if id == c"lightair_camera_capture" && property == "device" {
        return crate::kinds::cameras().into_iter().map(|(name, uid)| (name, serde_json::Value::from(uid))).collect();
    }
    list_items(src, property)
}

fn list_items(src: *mut sys::obs_source_t, property: &str) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    unsafe {
        let props = sys::obs_source_properties(src);
        if props.is_null() {
            return out;
        }
        let settings = sys::obs_source_get_settings(src);
        sys::obs_properties_apply_settings(props, settings);
        sys::obs_data_release(settings);
        let p = sys::obs_properties_get(props, cstr(property).as_ptr());
        if !p.is_null() {
            let int = sys::obs_property_list_format(p) == sys::OBS_COMBO_FORMAT_INT;
            let text = |ptr: *const std::ffi::c_char| {
                if ptr.is_null() { None } else { Some(CStr::from_ptr(ptr).to_string_lossy().into_owned()) }
            };
            for i in 0..sys::obs_property_list_item_count(p) {
                // a placeholder ("[Select a display to capture]"), not a choice
                if sys::obs_property_list_item_disabled(p, i) {
                    continue;
                }
                let label = text(sys::obs_property_list_item_name(p, i)).unwrap_or_default();
                let value = if int {
                    serde_json::Value::from(sys::obs_property_list_item_int(p, i))
                } else {
                    match text(sys::obs_property_list_item_string(p, i)) {
                        Some(v) => serde_json::Value::from(v),
                        None => continue,
                    }
                };
                if !label.is_empty() {
                    out.push((label, value));
                }
            }
        }
        sys::obs_properties_destroy(props);
    }
    out
}

/// Items of a list property of a source type, as (label, value) — e.g. the camera devices of
/// `av_capture_source_v2`'s `device` property. Call on the engine thread.
pub fn property_items(source_type: &str, property: &str) -> Vec<(String, String)> {
    property_items_with(source_type, property, &serde_json::Value::Null)
}

/// As [`property_items`], with `settings` applied to the properties first: lists that depend on
/// another choice (a camera's resolutions on its device) fill in for it.
pub fn property_items_with(source_type: &str, property: &str, settings: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let props = sys::obs_get_source_properties(cstr(source_type).as_ptr());
        if props.is_null() {
            return out;
        }
        if settings.is_object() {
            let data = Data::from_json(settings);
            sys::obs_properties_apply_settings(props, data.as_ptr());
        }
        let p = sys::obs_properties_get(props, cstr(property).as_ptr());
        if !p.is_null() {
            for i in 0..sys::obs_property_list_item_count(p) {
                let text = |ptr: *const std::ffi::c_char| {
                    if ptr.is_null() { String::new() } else { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
                };
                let label = text(sys::obs_property_list_item_name(p, i));
                let value = text(sys::obs_property_list_item_string(p, i));
                if !value.is_empty() {
                    out.push((label, value));
                }
            }
        }
        sys::obs_properties_destroy(props);
    }
    out
}
