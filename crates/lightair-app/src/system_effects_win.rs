//! The OS's own camera effects on Windows: Windows Studio Effects, where the camera and PC have
//! them (docs/plans/windows.md, phase 3). They need a Copilot+ PC's NPU (or a camera whose
//! driver brings them); where present, Windows applies them inside the camera driver, so the
//! picture OBS captures already has them, and they're switched in Settings › Camera, which the
//! button opens. Lightair doesn't read or switch them itself: that's the camera's extended
//! controls (KSPROPERTY_CAMERACONTROL_EXTENDED_BACKGROUNDSEGMENTATION), which can only be
//! tried on hardware that has them.

/// Opens Settings › Camera.
pub fn open_video_effects() {
    let _ = std::process::Command::new("explorer.exe").arg("ms-settings:camera").spawn();
}

/// Whether background blur is on: not known yet.
pub fn portrait_on() -> Option<bool> {
    None
}

/// The picture behind the person: Windows has no background replacement.
pub fn background_picture() -> Option<String> {
    None
}

pub fn set_background_picture(_: &std::path::Path) -> bool {
    false
}

pub const GREEN: [u8; 3] = [0, 255, 0];
pub const BLUE: [u8; 3] = [0, 0, 255];

pub fn set_backgrounds_on(_: bool) -> bool {
    false
}

pub fn camera_backgrounds(_: &str) -> Option<(bool, bool)> {
    None
}

pub fn set_background_colour(_: [u8; 3]) -> bool {
    false
}

/// Whether background replacement is on: Windows has none.
pub fn backgrounds_on() -> Option<bool> {
    None
}
