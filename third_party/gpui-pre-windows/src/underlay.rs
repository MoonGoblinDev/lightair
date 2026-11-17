//! LIGHTAIR-PATCH: a DirectComposition visual under each window's UI.
//!
//! GPUI draws a window into a premultiplied-alpha swap chain shown by DirectComposition. This
//! patch puts that swap chain's visual over an empty "underlay" visual, and publishes the
//! underlay with the composition device it belongs to, so an app can show its own content (a
//! video preview presented from another D3D11 device, say) under the UI: wherever GPUI leaves
//! the window transparent, the underlay shows through, without GPUI redrawing anything.
//!
//! Visuals must be made with the same device as the underlay. The pair is replaced when GPUI
//! recreates its devices (a lost device): `generation` changes, and the old visuals are gone.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::DirectComposition::{IDCompositionDevice, IDCompositionVisual};
use windows::core::Interface;

struct Entry {
    device: IDCompositionDevice,
    visual: IDCompositionVisual,
    generation: u64,
}

// SAFETY: DirectComposition devices and visuals are free-threaded COM objects.
unsafe impl Send for Entry {}

static ENTRIES: Mutex<Option<HashMap<isize, Entry>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn publish(hwnd: HWND, device: &IDCompositionDevice, visual: &IDCompositionVisual) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed);
    let entry = Entry { device: device.clone(), visual: visual.clone(), generation };
    ENTRIES.lock().unwrap().get_or_insert_with(HashMap::new).insert(hwnd.0 as isize, entry);
}

pub(crate) fn withdraw(hwnd: HWND, device: &IDCompositionDevice) {
    let mut entries = ENTRIES.lock().unwrap();
    if let Some(map) = entries.as_mut()
        && map.get(&(hwnd.0 as isize)).is_some_and(|e| e.device == *device)
    {
        map.remove(&(hwnd.0 as isize));
    }
}

/// A window's underlay: `IDCompositionDevice*` and `IDCompositionVisual*`, each with a
/// reference added for the caller (release both), and the generation they belong to.
pub fn get(hwnd: isize) -> Option<(*mut c_void, *mut c_void, u64)> {
    let entries = ENTRIES.lock().unwrap();
    let e = entries.as_ref()?.get(&hwnd)?;
    Some((e.device.clone().into_raw(), e.visual.clone().into_raw(), e.generation))
}

/// The current generation of a window's underlay (0: none), to notice a replacement cheaply.
pub fn generation(hwnd: isize) -> u64 {
    ENTRIES.lock().unwrap().as_ref().and_then(|m| m.get(&hwnd)).map_or(0, |e| e.generation)
}
