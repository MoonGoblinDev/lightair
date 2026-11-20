//! Media playback control for media sources (video files): runtime state, not document state
//! (docs/07-scenes-objects-canvas.md, "Media layer"). Looping and the file live in settings.

use libobs_sys as sys;
use lightair_model::Id;

use crate::reconcile::Reconciler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaState {
    None,
    Playing,
    Opening,
    Buffering,
    Paused,
    Stopped,
    Ended,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaStatus {
    pub state: MediaState,
    pub time_ms: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaAction {
    Play,
    Pause,
    Restart,
    Stop,
    /// Seek to a position in milliseconds.
    Seek(i64),
}

pub(crate) fn state(s: sys::obs_media_state) -> MediaState {
    match s {
        sys::OBS_MEDIA_STATE_PLAYING => MediaState::Playing,
        sys::OBS_MEDIA_STATE_OPENING => MediaState::Opening,
        sys::OBS_MEDIA_STATE_BUFFERING => MediaState::Buffering,
        sys::OBS_MEDIA_STATE_PAUSED => MediaState::Paused,
        sys::OBS_MEDIA_STATE_STOPPED => MediaState::Stopped,
        sys::OBS_MEDIA_STATE_ENDED => MediaState::Ended,
        sys::OBS_MEDIA_STATE_ERROR => MediaState::Error,
        _ => MediaState::None,
    }
}

impl Reconciler {
    /// Playback status of a media source, or `None` if the source isn't a media source.
    pub fn media_status(&self, source: Id) -> Option<MediaStatus> {
        let src = self.source_ptr(source)?;
        let flags = unsafe { sys::obs_source_get_output_flags(src) };
        if flags & sys::OBS_SOURCE_CONTROLLABLE_MEDIA == 0 {
            return None;
        }
        unsafe {
            Some(MediaStatus {
                state: state(sys::obs_source_media_get_state(src)),
                time_ms: sys::obs_source_media_get_time(src),
                duration_ms: sys::obs_source_media_get_duration(src),
            })
        }
    }

    /// Restyles a web page source's page now, with custom CSS not yet in its settings.
    pub fn web_restyle(&self, source: Id, css: String) {
        if let Some(src) = self.source_ptr(source) {
            crate::web::restyle(src, css);
        }
    }

    /// Loads a web page source's page again.
    pub fn web_reload(&self, source: Id) {
        if let Some(src) = self.source_ptr(source) {
            crate::web::reload(src);
        }
    }

    pub fn media_action(&self, source: Id, action: MediaAction) {
        let Some(src) = self.source_ptr(source) else { return };
        unsafe {
            match action {
                MediaAction::Play => sys::obs_source_media_play_pause(src, false),
                MediaAction::Pause => sys::obs_source_media_play_pause(src, true),
                MediaAction::Restart => sys::obs_source_media_restart(src),
                MediaAction::Stop => sys::obs_source_media_stop(src),
                MediaAction::Seek(ms) => sys::obs_source_media_set_time(src, ms.max(0)),
            }
        }
    }
}
