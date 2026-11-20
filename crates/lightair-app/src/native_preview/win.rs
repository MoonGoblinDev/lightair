//! The canvas preview drawn by DirectComposition, under the window's UI (Windows).
//!
//! GPUI shows its window through a DirectComposition swap chain with alpha; the patched GPUI
//! (third_party/gpui-pre-windows) puts an "underlay" visual below it. Here the underlay holds the
//! window's background, the canvas well, and in the well one visual per canvas showing a swap
//! chain that libobs's graphics thread presents the preview to (`lightair_preview::Layer`). GPUI
//! leaves the well transparent, so the picture shows through, and a new frame redraws nothing
//! of the UI: the counterpart of the macOS Core Animation preview.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{Bounds, Hsla, Pixels, Window};
use lightair_model::Id;
use lightair_preview::Layer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, ID3D11Texture2D};
use windows::Win32::Graphics::DirectComposition::{
    DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR, DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
    DCOMPOSITION_BORDER_MODE_HARD, IDCompositionDevice, IDCompositionSurface, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM};
use windows::core::Interface;
use windows_numerics::Matrix3x2;

/// The well and each canvas's picture, as last placed.
type Placed = (Option<Bounds<Pixels>>, Vec<(Id, Bounds<Pixels>)>);

/// A colour filling its visual: a 1×1 surface stretched by the visual's transform.
struct Solid {
    visual: IDCompositionVisual,
    _surface: IDCompositionSurface,
}

impl Solid {
    fn new(device: &IDCompositionDevice, colour: Hsla) -> windows::core::Result<Self> {
        let c = colour.to_rgb();
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        Self::rgb(device, (q(c.r) << 16) | (q(c.g) << 8) | q(c.b))
    }

    /// 0xRRGGBB. UI thread (it writes the pixel with GPUI's device context).
    fn rgb(device: &IDCompositionDevice, rgb: u32) -> windows::core::Result<Self> {
        unsafe {
            let visual = device.CreateVisual()?;
            let surface = surface(device, 1, 1, &[(rgb & 0xFF) as u8, ((rgb >> 8) & 0xFF) as u8, (rgb >> 16) as u8, 255], false)?;
            visual.SetContent(&surface)?;
            visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR)?;
            visual.SetBorderMode(DCOMPOSITION_BORDER_MODE_HARD)?;
            Ok(Self { visual, _surface: surface })
        }
    }
}

/// A composition surface holding `bgra` (`w`×`h`, premultiplied when `alpha`). UI thread: it
/// writes through GPUI's device context.
fn surface(device: &IDCompositionDevice, w: u32, h: u32, bgra: &[u8], alpha: bool) -> windows::core::Result<IDCompositionSurface> {
    unsafe {
        let mode = if alpha { DXGI_ALPHA_MODE_PREMULTIPLIED } else { DXGI_ALPHA_MODE_IGNORE };
        let surface = device.CreateSurface(w, h, DXGI_FORMAT_B8G8R8A8_UNORM, mode)?;
        let mut at = POINT::default();
        let tex: ID3D11Texture2D = surface.BeginDraw(None, &mut at)?;
        let (x, y) = (at.x as u32, at.y as u32);
        // the surface may be a part of a larger texture: only its own pixels are written
        let place = D3D11_BOX { left: x, top: y, front: 0, right: x + w, bottom: y + h, back: 1 };
        let context = tex.GetDevice()?.GetImmediateContext()?;
        context.UpdateSubresource(&tex, 0, Some(&place), bgra.as_ptr().cast(), w * 4, w * h * 4);
        drop(tex);
        surface.EndDraw()?;
        Ok(surface)
    }
}

fn transform(visual: &IDCompositionVisual, sx: f32, sy: f32, x: f32, y: f32) {
    let m = Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: x, M32: y };
    unsafe {
        let _ = visual.SetTransform2(&m);
    }
}

fn clip(visual: &IDCompositionVisual, w: f32, h: f32) {
    let r = D2D_RECT_F { left: 0.0, top: 0.0, right: w.max(0.0), bottom: h.max(0.0) };
    unsafe {
        let _ = visual.SetClip2(&r);
    }
}

struct Picture {
    visual: IDCompositionVisual,
    layer: Arc<Layer>,
    /// The swap chain's size its transform was worked out for.
    sized: (u32, u32),
}

/// The visuals under the UI: the background, the well (clipping its pictures) and each canvas's
/// picture.
pub struct NativePreview {
    hwnd: isize,
    generation: u64,
    device: IDCompositionDevice,
    underlay: IDCompositionVisual,
    _background: Solid,
    well: IDCompositionVisual,
    well_fill: Solid,
    pictures: HashMap<Id, Picture>,
    last: Placed,
    visible: bool,
    scale: f32,
    /// The mixer's meters: their container (between the background and the well), what the
    /// meter thread draws (its lock is every composition change's, both threads), whether the
    /// thread runs and the mixer's tab is shown.
    meters_root: IDCompositionVisual,
    meters: Arc<parking_lot::Mutex<Meters>>,
    meter_thread: bool,
    meters_active: bool,
    /// The Editor's player (its frames presented by its renderer), and whether the Editor shows.
    player: Option<Player>,
    player_shown: bool,
}

/// The Editor's player under the UI: a container clipping to the player, black behind, and the
/// visual the renderer presents into.
struct Player {
    root: IDCompositionVisual,
    back: Solid,
    picture: IDCompositionVisual,
    placed: Option<(Bounds<Pixels>, (u32, u32))>,
}

impl NativePreview {
    /// Fills the window's underlay (GPUI's window must be transparent), painting `background`
    /// everywhere and `well` where the canvas well is.
    pub fn attach(window: &Window, background: Hsla, well: Hsla) -> Option<Self> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return None };
        let hwnd = h.hwnd.get();
        let (device, underlay, generation) = gpui_windows::underlay::get(hwnd)?;
        // SAFETY: `get` added a reference to each for us.
        let (device, underlay) = unsafe { (IDCompositionDevice::from_raw(device), IDCompositionVisual::from_raw(underlay)) };
        let made = (|| -> windows::core::Result<_> {
            let bg = Solid::new(&device, background)?;
            // larger than any window; the window clips it
            transform(&bg.visual, 32768.0, 32768.0, 0.0, 0.0);
            let well_visual = unsafe { device.CreateVisual()? };
            let fill = Solid::new(&device, well)?;
            // the meters under the well: a strip the canvas grows over is covered by it
            let meters = unsafe { device.CreateVisual()? };
            unsafe {
                underlay.RemoveAllVisuals()?;
                underlay.AddVisual(&bg.visual, false, None)?;
                underlay.AddVisual(&meters, true, &bg.visual)?;
                well_visual.AddVisual(&fill.visual, false, None)?;
                underlay.AddVisual(&well_visual, true, &meters)?;
            }
            clip(&well_visual, 0.0, 0.0);
            clip(&meters, 0.0, 0.0);
            unsafe { device.Commit()? };
            Ok((bg, well_visual, fill, meters))
        })();
        let (bg, well_visual, fill, meters_root) = match made {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("the preview under the UI couldn't be made: {e}");
                return None;
            }
        };
        Some(Self {
            hwnd,
            generation,
            device,
            underlay,
            _background: bg,
            well: well_visual,
            well_fill: fill,
            pictures: HashMap::new(),
            last: (None, vec![]),
            visible: true,
            scale: window.scale_factor(),
            meters_root,
            meters: Default::default(),
            meter_thread: false,
            meters_active: true,
            player: None,
            player_shown: false,
        })
    }

    /// Whether GPUI still shows these visuals (it remakes its composition after a lost device).
    pub fn current(&self) -> bool {
        gpui_windows::underlay::generation(self.hwnd) == self.generation
    }

    /// The layer a canvas's preview presents its frames to (made the first time), for
    /// `Preview::set_layer`; null if it couldn't be made.
    pub fn picture(&mut self, canvas: Id) -> *mut std::ffi::c_void {
        if let Some(p) = self.pictures.get(&canvas) {
            return Arc::as_ptr(&p.layer) as *mut _;
        }
        let Some(layer) = Layer::new() else { return std::ptr::null_mut() };
        let layer = Arc::new(layer);
        let _comp = self.meters.lock();
        let made = (|| -> windows::core::Result<IDCompositionVisual> {
            unsafe {
                let visual = self.device.CreateVisual()?;
                let swap = layer.swap_chain();
                let swap = windows::core::IUnknown::from_raw_borrowed(&swap);
                visual.SetContent(swap)?;
                visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR)?;
                // hidden until placed
                transform(&visual, 0.0, 0.0, 0.0, 0.0);
                self.well.AddVisual(&visual, true, &self.well_fill.visual)?;
                self.device.Commit()?;
                Ok(visual)
            }
        })();
        match made {
            Ok(visual) => {
                let ptr = Arc::as_ptr(&layer) as *mut _;
                self.pictures.insert(canvas, Picture { visual, layer, sized: (0, 0) });
                ptr
            }
            Err(e) => {
                tracing::warn!("canvas picture visual: {e}");
                std::ptr::null_mut()
            }
        }
    }

    /// Where the well is (window points), and each shown canvas's picture in it; `None`: not
    /// on screen (another place, the Library, Editor).
    pub fn place(&mut self, well: Option<Bounds<Pixels>>, pictures: &[(Id, Bounds<Pixels>)]) {
        let resized = self.pictures.values().any(|p| p.layer.size() != p.sized);
        if !resized && self.last.0 == well && self.last.1 == pictures {
            return;
        }
        self.last = (well, pictures.to_vec());
        self.apply();
    }

    fn apply(&mut self) {
        let _comp = self.meters.lock();
        let s = self.scale;
        let px = |v: Pixels| f32::from(v) * s;
        match self.last.0.filter(|_| self.visible) {
            None => clip(&self.well, 0.0, 0.0),
            Some(w) => {
                let (wx, wy) = (px(w.origin.x).round(), px(w.origin.y).round());
                let (ww, wh) = (px(w.size.width).round(), px(w.size.height).round());
                transform(&self.well, 1.0, 1.0, wx, wy);
                clip(&self.well, ww, wh);
                transform(&self.well_fill.visual, ww, wh, 0.0, 0.0);
                for (id, p) in &mut self.pictures {
                    p.sized = p.layer.size();
                    match self.last.1.iter().find(|(c, _)| c == id) {
                        Some((_, b)) => {
                            let (sw, sh) = p.sized;
                            let (x, y) = ((px(b.origin.x) - wx).round(), (px(b.origin.y) - wy).round());
                            let (bw, bh) = (px(b.size.width).round(), px(b.size.height).round());
                            // frames come at this size from now on (`refit` follows)
                            p.layer.show_at(bw as u32, bh as u32);
                            transform(&p.visual, bw / sw.max(1) as f32, bh / sh.max(1) as f32, x, y);
                        }
                        None => transform(&p.visual, 0.0, 0.0, 0.0, 0.0),
                    }
                }
            }
        }
        unsafe {
            let _ = self.device.Commit();
        }
    }

    /// Places the pictures again when a swap chain changed size since (libobs's thread resizes
    /// it to the size it's shown at, a frame after it's asked): called on a timer, cheap. Shows
    /// the Editor player's new swap chain (the renderer made one: it shows once committed).
    pub fn refit(&mut self) {
        if self.pictures.values().any(|p| p.layer.size() != p.sized) {
            self.apply();
        }
        if lightair_render::compose::VISUAL_CHANGED.swap(false, std::sync::atomic::Ordering::AcqRel) {
            let _comp = self.meters.lock();
            unsafe {
                let _ = self.device.Commit();
            }
        }
    }

    /// The visual the Editor's renderer presents its frames into (`IDCompositionVisual*`,
    /// borrowed: it lives as long as this does), made the first time.
    pub fn player_visual(&mut self) -> *mut std::ffi::c_void {
        if self.player.is_none() {
            let made = (|| -> windows::core::Result<Player> {
                let root = unsafe { self.device.CreateVisual()? };
                let back = Solid::rgb(&self.device, 0x000000)?;
                let picture = unsafe { self.device.CreateVisual()? };
                unsafe {
                    root.AddVisual(&back.visual, false, None)?;
                    root.AddVisual(&picture, false, None)?;
                    picture.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR)?;
                    // over everything else under the UI (in the Editor nothing else shows)
                    self.underlay.AddVisual(&root, false, None)?;
                }
                clip(&root, 0.0, 0.0);
                Ok(Player { root, back, picture, placed: None })
            })();
            match made {
                Ok(p) => {
                    let _comp = self.meters.lock();
                    unsafe {
                        let _ = self.device.Commit();
                    }
                    self.player = Some(p);
                }
                Err(e) => {
                    tracing::warn!("the player's visual: {e}");
                    return std::ptr::null_mut();
                }
            }
        }
        self.player.as_ref().map_or(std::ptr::null_mut(), |p| p.picture.as_raw())
    }

    /// Where the Editor's player is (window points) and the size of the frames presented into
    /// it (fitted in, as the player fits its picture); `None`: not shown.
    pub fn place_player(&mut self, place: Option<(Bounds<Pixels>, (u32, u32))>) {
        let s = self.scale;
        let Some(p) = self.player.as_mut() else { return };
        let place = place.filter(|_| self.player_shown);
        if p.placed == place {
            return;
        }
        p.placed = place;
        let _comp = self.meters.lock();
        match place {
            None => clip(&p.root, 0.0, 0.0),
            Some((b, (fw, fh))) => {
                let (x, y) = ((f32::from(b.origin.x) * s).round(), (f32::from(b.origin.y) * s).round());
                let (w, h) = ((f32::from(b.size.width) * s).round(), (f32::from(b.size.height) * s).round());
                transform(&p.root, 1.0, 1.0, x, y);
                clip(&p.root, w, h);
                transform(&p.back.visual, w, h, 0.0, 0.0);
                // the frame fitted in, centred
                let k = (w / fw.max(1) as f32).min(h / fh.max(1) as f32);
                let (pw, ph) = (fw as f32 * k, fh as f32 * k);
                transform(&p.picture, k, k, ((w - pw) / 2.0).round(), ((h - ph) / 2.0).round());
            }
        }
        unsafe {
            let _ = self.device.Commit();
        }
    }

    /// The Editor is the place shown (its player may show) or not.
    pub fn show_player(&mut self, shown: bool) {
        if self.player_shown != shown {
            self.player_shown = shown;
            if !shown {
                self.place_player(None);
            }
        }
    }

    /// The canvas is on screen (the Live place) or not; placed again as it last was.
    pub fn set_visible(&mut self, visible: bool) {
        if visible != self.visible {
            self.visible = visible;
            self.apply();
            self.apply_meters();
        }
    }

    /// A canvas gone: its visual and swap chain go too.
    pub fn forget(&mut self, canvas: Id) {
        if let Some(p) = self.pictures.remove(&canvas) {
            let _comp = self.meters.lock();
            unsafe {
                let _ = self.well.RemoveVisual(&p.visual);
                let _ = self.device.Commit();
            }
        }
    }

    /// Puts each strip's meter where the mixer laid it out (holes GPUI leaves in the mixer), or
    /// takes away those no longer there. UI thread.
    pub fn place_meters(&mut self, strips: &[(Id, super::MeterPlace)]) {
        let s = self.scale;
        let mut m = self.meters.lock();
        let gone: Vec<Id> = m.strips.keys().copied().filter(|id| !strips.iter().any(|(k, _)| k == id)).collect();
        for id in gone {
            if let Some(v) = m.strips.remove(&id) {
                unsafe {
                    let _ = self.meters_root.RemoveVisual(&v.clip);
                }
            }
        }
        if m.glyphs.is_none() {
            m.glyphs = Glyphs::new(&self.device, s).ok();
        }
        for (id, place) in strips {
            if m.strips.get(id).is_some_and(|v| v.place == *place) {
                continue;
            }
            let shown = m.strips.get(id).map(|v| v.shown);
            if let Some(old) = m.strips.remove(id) {
                unsafe {
                    let _ = self.meters_root.RemoveVisual(&old.clip);
                }
            }
            match StripVisuals::new(&self.device, place, s, m.glyphs.as_ref()) {
                Ok(mut v) => {
                    // the levels carry over; what's drawn doesn't (these visuals are new)
                    if let Some(shown) = shown {
                        v.shown = Shown { text: None, ..shown };
                    }
                    unsafe {
                        let _ = self.meters_root.AddVisual(&v.clip, false, None);
                    }
                    m.strips.insert(*id, v);
                }
                Err(e) => tracing::warn!("meter visuals: {e}"),
            }
        }
        unsafe {
            let _ = self.device.Commit();
        }
        drop(m);
        if !self.meter_thread && !strips.is_empty() {
            self.meter_thread = true;
            let (meters, device) = (self.meters.clone(), Comp(self.device.clone()));
            let _ = std::thread::Builder::new().name("lightair-meters".into()).spawn(move || meter_thread(meters, device));
        }
    }

    /// The mixer's tab is the one shown (its meters show in the Live place).
    pub fn show_meters(&mut self, active: bool) {
        self.meters_active = active;
        self.apply_meters();
    }

    fn apply_meters(&mut self) {
        let shown = self.meters_active && self.visible;
        let mut m = self.meters.lock();
        if m.shown == shown {
            return;
        }
        m.shown = shown;
        clip(&self.meters_root, if shown { 32768.0 } else { 0.0 }, if shown { 32768.0 } else { 0.0 });
        unsafe {
            let _ = self.device.Commit();
        }
    }
}

impl Drop for NativePreview {
    fn drop(&mut self) {
        // the meter thread sees it's alone and ends
        let mut m = self.meters.lock();
        m.strips.clear();
        unsafe {
            let _ = self.underlay.RemoveAllVisuals();
            let _ = self.device.Commit();
        }
    }
}

// ---------------------------------------------------------------- meters

/// Peak hold (the marker every console leaves at the recent peak): held this long, then it
/// falls this fast.
const HOLD_S: f32 = 1.5;
const HOLD_FALL_DB_S: f32 = 12.0;
/// The peak readout: the held peak, changing at most this often (a number that moved every
/// frame couldn't be read).
const READOUT_S: f32 = 0.25;
/// The meters' colours (ui.rs's tokens): green below −20, yellow −20…−9, red above −9 dBFS.
const OK: u32 = 0x34D399;
const WARN: u32 = 0xFBBF24;
const ERROR: u32 = 0xF87171;
const BAR_BG: u32 = 0x18181B;
const BOX_BG: u32 = 0x0B0B0C;
const TEXT: u32 = 0xA9A9B1;
/// What the readout writes: digits, the point, the minus and "inf".
const GLYPHS: &str = "-.0123456789inf";
/// The readout's longest text ("-60.0").
const READOUT_CHARS: usize = 5;

fn meter_frac(db: f32) -> f32 {
    if !db.is_finite() { 0.0 } else { ((db + 60.0) / 60.0).clamp(0.0, 1.0) }
}

/// The composition device, used by the meter thread too (under the meters' lock).
struct Comp(IDCompositionDevice);
// SAFETY: DirectComposition objects are free-threaded; every use from two threads is under one
// lock (`Meters`).
unsafe impl Send for Comp {}

/// The readout's characters as small surfaces, in the readout's colour and in red (made once,
/// UI thread): the meter thread spells a number by swapping them, drawing nothing itself.
struct Glyphs {
    /// Per character: its surface in TEXT and in ERROR.
    surfaces: HashMap<char, [IDCompositionSurface; 2]>,
    /// Every glyph's box (pixels): one advance for all, like a fixed-width face.
    size: (f32, f32),
    /// How far apart they sit (the box less the padding the text renderer leaves round a glyph).
    advance: f32,
}

impl Glyphs {
    fn new(device: &IDCompositionDevice, scale: f32) -> windows::core::Result<Self> {
        let mut surfaces = HashMap::new();
        let mut size = (0.0f32, 0.0f32);
        let mut rasters = Vec::new();
        for c in GLYPHS.chars() {
            let mut both = Vec::new();
            for colour in [TEXT, ERROR] {
                let style = lightair_render::sequence::TextStyle {
                    text: c.to_string(),
                    size: 10.0 * scale,
                    color: colour,
                    bold: false,
                    align: 0,
                    background: None,
                    outline: None,
                    width: 64.0 * scale,
                    font: None,
                };
                let r = lightair_render::generate::text(&style);
                size = (size.0.max(r.size.0 as f32), size.1.max(r.size.1 as f32));
                both.push(r);
            }
            rasters.push((c, both));
        }
        // each glyph centred in the common box
        let (bw, bh) = (size.0.ceil() as u32, size.1.ceil() as u32);
        for (c, both) in rasters {
            let mut made = Vec::new();
            for r in both {
                let mut bgra = vec![0u8; (bw * bh * 4) as usize];
                let (ox, oy) = ((bw - r.size.0.min(bw)) / 2, (bh - r.size.1.min(bh)) / 2);
                for y in 0..r.size.1.min(bh) {
                    for x in 0..r.size.0.min(bw) {
                        let s = ((y * r.size.0 + x) * 4) as usize;
                        let d = (((y + oy) * bw + x + ox) * 4) as usize;
                        // RGBA (premultiplied) to BGRA
                        bgra[d] = r.rgba[s + 2];
                        bgra[d + 1] = r.rgba[s + 1];
                        bgra[d + 2] = r.rgba[s];
                        bgra[d + 3] = r.rgba[s + 3];
                    }
                }
                made.push(surface(device, bw, bh, &bgra, true)?);
            }
            let [a, b]: [IDCompositionSurface; 2] = made.try_into().map_err(|_| windows::core::Error::empty())?;
            surfaces.insert(c, [a, b]);
        }
        let advance = (bw as f32 - 2.0 * (10.0 * scale * 0.12)).max(1.0).ceil();
        Ok(Self { surfaces, size: (bw as f32, bh as f32), advance })
    }
}

/// A strip's meter as displayed.
#[derive(Clone, Copy)]
struct Shown {
    bars: [f32; 2],
    hold: [(f32, std::time::Instant); 2],
    readout: (f32, std::time::Instant),
    text: Option<f32>,
    last: std::time::Instant,
}

/// A strip's meter: its clip, box, readout box, each bar's track, three level segments and
/// peak hold (three, one per colour), and the readout's characters.
struct StripVisuals {
    clip: IDCompositionVisual,
    _solids: Vec<Solid>,
    /// Per bar: the green, yellow and red segments, then the hold in each colour.
    bars: [[IDCompositionVisual; 6]; 2],
    /// Each bar's place in the clip (x, bottom, width, height; pixels).
    bar_place: [(f32, f32, f32, f32); 2],
    chars: Vec<IDCompositionVisual>,
    /// The readout's place in the clip (x, y, w, h; pixels).
    readout: (f32, f32, f32, f32),
    place: super::MeterPlace,
    shown: Shown,
    /// What's drawn: each bar's level and hold top (pixels), and the readout's text.
    drawn: Option<[(f32, f32); 2]>,
}

impl StripVisuals {
    fn new(device: &IDCompositionDevice, place: &super::MeterPlace, s: f32, glyphs: Option<&Glyphs>) -> windows::core::Result<Self> {
        let px = |b: Bounds<Pixels>| {
            (f32::from(b.origin.x) * s, f32::from(b.origin.y) * s, f32::from(b.size.width) * s, f32::from(b.size.height) * s)
        };
        let (cx, cy, cw, ch) = px(place.clip);
        let within = |b: Bounds<Pixels>| {
            let (x, y, w, h) = px(b);
            ((x - cx).round(), (y - cy).round(), w.round(), h.round())
        };
        let clip_v = unsafe { device.CreateVisual()? };
        transform(&clip_v, 1.0, 1.0, cx.round(), cy.round());
        clip(&clip_v, cw.round(), ch.round());
        let mut solids = Vec::new();
        let mut add = |colour: u32, rect: (f32, f32, f32, f32)| -> windows::core::Result<IDCompositionVisual> {
            let solid = Solid::rgb(device, colour)?;
            transform(&solid.visual, rect.2, rect.3, rect.0, rect.1);
            // (with no reference visual, `insertAbove: false` puts it in front of the others)
            unsafe { clip_v.AddVisual(&solid.visual, false, None)? };
            let v = solid.visual.clone();
            solids.push(solid);
            Ok(v)
        };
        add(BOX_BG, within(place.boxed))?;
        let readout = within(place.readout);
        add(BOX_BG, readout)?;
        let mut bars: Vec<[IDCompositionVisual; 6]> = Vec::new();
        let mut bar_place = [(0.0, 0.0, 0.0, 0.0); 2];
        for (c, b) in place.bars.iter().enumerate() {
            let (x, y, w, h) = within(*b);
            add(BAR_BG, (x, y, w, h))?;
            bar_place[c] = (x, y + h, w, h);
            let mut six = Vec::new();
            for colour in [OK, WARN, ERROR, OK, WARN, ERROR] {
                // hidden until the meter thread sizes them
                six.push(add(colour, (0.0, 0.0, 0.0, 0.0))?);
            }
            bars.push(six.try_into().map_err(|_| windows::core::Error::empty())?);
        }
        let mut chars = Vec::new();
        if glyphs.is_some() {
            for _ in 0..READOUT_CHARS {
                let v = unsafe { device.CreateVisual()? };
                unsafe { clip_v.AddVisual(&v, false, None)? };
                chars.push(v);
            }
        }
        let now = std::time::Instant::now();
        let quiet = f32::NEG_INFINITY;
        Ok(Self {
            clip: clip_v,
            _solids: solids,
            bars: bars.try_into().map_err(|_| windows::core::Error::empty())?,
            bar_place,
            chars,
            readout,
            place: *place,
            shown: Shown { bars: [quiet; 2], hold: [(quiet, now); 2], readout: (quiet, now), text: None, last: now },
            drawn: None,
        })
    }

    /// Sizes the bars' segments and hold to the levels shown (when they've changed by a
    /// pixel), and spells the readout when its number changed. Under the meters' lock.
    fn draw(&mut self, glyphs: Option<&Glyphs>) {
        let (g_end, y_end) = (meter_frac(-20.0), meter_frac(-9.0));
        let mut now = [(0.0f32, 0.0f32); 2];
        for (c, n) in now.iter_mut().enumerate() {
            let h = self.bar_place[c].3;
            let held = self.shown.hold[c].0;
            *n = ((h * meter_frac(self.shown.bars[c])).round(), if held > -90.0 { (h * meter_frac(held)).round() } else { -1.0 });
        }
        if self.drawn != Some(now) {
            self.drawn = Some(now);
            for (c, &(level, top)) in now.iter().enumerate() {
                let (x, bottom, w, h) = self.bar_place[c];
                for (k, (from, to)) in [(0.0, g_end), (g_end, y_end), (y_end, 1.0)].into_iter().enumerate() {
                    let (lo, hi) = ((h * from).round(), (h * to).round().min(level));
                    if hi > lo {
                        transform(&self.bars[c][k], w, hi - lo, x, bottom - hi);
                    } else {
                        transform(&self.bars[c][k], 0.0, 0.0, 0.0, 0.0);
                    }
                }
                // the peak hold: a 2-pixel line, coloured by where it is
                let zone = if top / h > y_end { 2 } else if top / h > g_end { 1 } else { 0 };
                for k in 0..3 {
                    if top > 0.0 && k == zone {
                        transform(&self.bars[c][3 + k], w, top.min(2.0), x, bottom - top);
                    } else {
                        transform(&self.bars[c][3 + k], 0.0, 0.0, 0.0, 0.0);
                    }
                }
            }
        }
        let peak = self.shown.readout.0;
        if let Some(g) = glyphs
            && self.shown.text != Some(peak)
        {
            self.shown.text = Some(peak);
            let text = if peak > -90.0 { format!("{peak:.1}") } else { "-inf".into() };
            let red = usize::from(peak > -1.0);
            let (rx, ry, rw, rh) = self.readout;
            let n = text.chars().count().min(self.chars.len());
            let x0 = (rx + (rw - g.advance * n as f32) / 2.0 - (g.size.0 - g.advance) / 2.0).round();
            let y0 = (ry + (rh - g.size.1) / 2.0).round();
            for (i, v) in self.chars.iter().enumerate() {
                let surface = text.chars().nth(i).and_then(|c| g.surfaces.get(&c)).map(|s| &s[red]);
                unsafe {
                    match surface {
                        Some(s) => {
                            let _ = v.SetContent(s);
                            transform(v, 1.0, 1.0, x0 + g.advance * i as f32, y0);
                        }
                        None => {
                            let _ = v.SetContent(None::<&windows::core::IUnknown>);
                        }
                    }
                }
            }
        }
    }
}

/// The meters (shared with the meter thread, which draws them).
#[derive(Default)]
struct Meters {
    strips: HashMap<Id, StripVisuals>,
    glyphs: Option<Glyphs>,
    shown: bool,
}

// SAFETY: its composition objects are used under its lock only (UI and meter threads).
unsafe impl Send for Meters {}

/// Moves every meter at 60 frames a second, straight from the audio levels, without the UI.
fn meter_thread(shared: Arc<parking_lot::Mutex<Meters>>, device: Comp) {
    loop {
        std::thread::sleep(std::time::Duration::from_micros(16_667));
        // the window gone: the thread goes too
        if Arc::strong_count(&shared) == 1 {
            break;
        }
        let mut guard = shared.lock();
        if !guard.shown || guard.strips.is_empty() {
            continue;
        }
        let now = std::time::Instant::now();
        let Meters { strips, glyphs, .. } = &mut *guard;
        let mut changed = false;
        for (id, v) in strips.iter_mut() {
            let levels = lightair_engine::meters::meters().get(*id).map(|c| c.levels());
            let sh = &mut v.shown;
            let dt = now.duration_since(sh.last).as_secs_f32().min(0.1);
            sh.last = now;
            let quiet = f32::NEG_INFINITY;
            for c in 0..2 {
                let target = levels.map_or(quiet, |l| l.peak[c]);
                // the bar: rises at once, falls at 24 dB/s
                let b = &mut sh.bars[c];
                let fallen = if b.is_finite() { *b - 24.0 * dt } else { quiet };
                *b = if target > fallen { target } else { fallen.max(-90.0) };
                // the hold: set by a new peak, kept a while, then falling slowly, never below the bar
                let (level, since) = &mut sh.hold[c];
                if target >= *level {
                    (*level, *since) = (target, now);
                } else if now.duration_since(*since).as_secs_f32() > HOLD_S {
                    *level = (*level - HOLD_FALL_DB_S * dt).max(sh.bars[c]);
                    if *level <= -90.0 {
                        *level = quiet;
                    }
                }
            }
            let held = sh.hold[0].0.max(sh.hold[1].0);
            if now.duration_since(sh.readout.1).as_secs_f32() >= READOUT_S || held > sh.readout.0 {
                sh.readout = (held, now);
            }
            let before = (v.drawn, v.shown.text);
            v.draw(glyphs.as_ref());
            changed |= before != (v.drawn, v.shown.text);
        }
        // nothing moved: nothing to hand to the compositor
        if changed {
            unsafe {
                let _ = device.0.Commit();
            }
        }
    }
}
