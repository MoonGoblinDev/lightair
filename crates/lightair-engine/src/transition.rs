//! The Move transition (docs/07-scenes-objects-canvas.md, "Move transition details").
//!
//! Switching from scene A to scene B builds a temporary scene in each canvas's canvas: B's
//! objects that match a object in A (same source first, then same name) start at A's layout and
//! glide to B's; objects only in B fade in; objects only in A fade out. A libobs tick callback
//! drives the glide every frame; when it finishes, each canvas cuts to B (which now looks
//! identical) and the temporary scenes are released on the engine thread.

use std::time::Instant;

use libobs_sys as sys;
use lightair_model::{Id, Layout, Scene as DocScene};

use crate::obs::{ObsScene, ObsSource};

pub(crate) struct Tween {
    pub item: *mut sys::obs_sceneitem_t,
    /// For a shaped object: the item inside its group and the mask.
    pub masked: Option<crate::reconcile::Masked>,
    pub from: Layout,
    pub to: Layout,
}

pub(crate) struct CanvasMove {
    /// Owned to keep the temporary scene alive until the Move is cleaned up.
    #[allow(dead_code)]
    pub temp: ObsScene,
    pub tweens: Vec<Tween>,
    /// The canvas's transition and the scene to land on.
    pub transition: *mut sys::obs_source_t,
    pub dest: *mut sys::obs_source_t,
    /// Per-item fade transitions and shape masks, kept alive for the animation.
    #[allow(dead_code)]
    pub fades: Vec<ObsSource>,
    #[allow(dead_code)]
    pub masks: Vec<crate::reconcile::Masked>,
}

pub(crate) struct MoveAnim {
    pub canvases: Vec<CanvasMove>,
    pub start: Instant,
    pub duration: f32,
    pub done: bool,
}

unsafe impl Send for MoveAnim {}

/// Which object in `from` a object of `to` continues: the one showing the same source, else the
/// one with the same link (`Object::link`). Never by name: two objects called "Logo" needn't be
/// the same thing.
pub(crate) fn match_object<'a>(from: &'a DocScene, object: &lightair_model::Object) -> Option<&'a lightair_model::Object> {
    let same_source = |l: &&lightair_model::Object| object.source.is_some() && l.source == object.source;
    let same_link = |l: &&lightair_model::Object| object.link.is_some() && l.link == object.link;
    from.objects.iter().find(same_source).or_else(|| from.objects.iter().find(same_link))
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn lerp_layout(a: &Layout, b: &Layout, t: f32) -> Layout {
    let crop = |x: u32, y: u32| lerp(x as f32, y as f32, t).round() as u32;
    Layout {
        x: lerp(a.x, b.x, t),
        y: lerp(a.y, b.y, t),
        width: lerp(a.width, b.width, t),
        height: lerp(a.height, b.height, t),
        rotation: lerp(a.rotation, b.rotation, t),
        tilt_x: lerp(a.tilt_x, b.tilt_x, t),
        tilt_y: lerp(a.tilt_y, b.tilt_y, t),
        crop: lightair_model::Crop {
            left: crop(a.crop.left, b.crop.left),
            top: crop(a.crop.top, b.crop.top),
            right: crop(a.crop.right, b.crop.right),
            bottom: crop(a.crop.bottom, b.crop.bottom),
        },
        ..*b
    }
}

impl MoveAnim {
    /// Advances the animation; returns true when it has finished (and landed on B).
    pub(crate) fn step(&mut self) -> bool {
        if self.done {
            return true;
        }
        let t = (self.start.elapsed().as_secs_f32() / self.duration.max(0.001)).min(1.0);
        let e = ease_in_out(t);
        for f in &self.canvases {
            for tw in &f.tweens {
                unsafe {
                    crate::reconcile::layout_item(tw.item, tw.masked.as_ref(), &lerp_layout(&tw.from, &tw.to, e))
                };
            }
        }
        if t >= 1.0 {
            self.finish();
        }
        self.done
    }

    /// Lands every canvas on B immediately.
    pub(crate) fn finish(&mut self) {
        if self.done {
            return;
        }
        for f in &self.canvases {
            unsafe { set_now(f.transition, f.dest) };
        }
        self.done = true;
    }
}

/// Points a transition at `dest` straight away. libobs lets the old scene go before it takes the
/// new one, so a source in both would stop for an instant and restart (a video caught between its
/// stop and its restart stays stopped): `dest` is kept running across the swap.
pub(crate) unsafe fn set_now(transition: *mut sys::obs_source_t, dest: *mut sys::obs_source_t) {
    unsafe {
        if !dest.is_null() {
            sys::obs_source_inc_active(dest);
        }
        sys::obs_transition_set(transition, dest);
        if !dest.is_null() {
            sys::obs_source_dec_active(dest);
        }
    }
}

/// Fade used for objects entering or leaving during a Move.
pub(crate) fn item_fade(item: *mut sys::obs_sceneitem_t, show: bool, ms: u32, keep: &mut Vec<ObsSource>) {
    if let Some(fade) = ObsSource::create_private("fade_transition", &format!("move-fade-{}", Id::new()), None) {
        unsafe {
            sys::obs_sceneitem_set_transition(item, show, fade.as_ptr());
            sys::obs_sceneitem_set_transition_duration(item, show, ms);
        }
        keep.push(fade);
    }
}
