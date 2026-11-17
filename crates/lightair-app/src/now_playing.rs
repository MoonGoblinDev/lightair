//! What music is playing (docs/plans/live-integrations.md, 2.8): Spotify's or Apple Music's
//! current track, asked of the app itself (AppleScript: no developer account, no sign-in; macOS
//! asks once whether Lightair may). Only while a Now playing widget exists, and only of a music
//! app that's already open (asking never opens it).

use std::time::Duration;

use serde_json::{Value, json};

#[cfg(not(windows))]
const SCRIPT: &str = r#"
if application "Spotify" is running then
  tell application "Spotify"
    if player state is playing then return "spotify" & tab & (name of current track) & tab & (artist of current track) & tab & (album of current track) & tab & (artwork url of current track)
  end tell
end if
if application "Music" is running then
  tell application "Music"
    if player state is playing then return "music" & tab & (name of current track) & tab & (artist of current track) & tab & (album of current track) & tab & ""
  end tell
end if
return ""
"#;

/// The track playing now (`null` when nothing is).
pub fn now() -> Value {
    // Spotify's own API where signed in officially: any device, with progress and artwork
    if let Some(v) = lightair_live::official::spotify_now() {
        return v;
    }
    #[cfg(windows)]
    {
        windows_now().unwrap_or(Value::Null)
    }
    #[cfg(not(windows))]
    macos_now()
}

#[cfg(not(windows))]
fn macos_now() -> Value {
    if !cfg!(target_os = "macos") {
        return Value::Null;
    }
    let Ok(out) = std::process::Command::new("/usr/bin/osascript").arg("-e").arg(SCRIPT).output() else { return Value::Null };
    let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let parts: Vec<&str> = text.split('\t').collect();
    match parts.as_slice() {
        [app, title, artist, album, art, ..] => json!({"app": app, "title": title, "artist": artist, "album": album, "artwork": art}),
        _ => Value::Null,
    }
}

/// Windows: the system's media sessions (what the volume flyout shows: Spotify, a browser tab,
/// Media Player…), the current one if it's playing. The artwork is the session's thumbnail as a
/// data address, read again only when the track changes.
#[cfg(windows)]
fn windows_now() -> Option<Value> {
    use std::cell::RefCell;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::{DataReader, InputStreamOptions};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    thread_local! {
        static STARTED: RefCell<bool> = const { RefCell::new(false) };
        /// The last track's (title, artist) and its artwork.
        static ART: RefCell<(String, String, String)> = const { RefCell::new((String::new(), String::new(), String::new())) };
    }
    STARTED.with(|s| {
        if !*s.borrow() {
            // (this thread only; already started is fine)
            let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
            *s.borrow_mut() = true;
        }
    });
    let manager = Manager::RequestAsync().ok()?.join().ok()?;
    let session = manager.GetCurrentSession().ok()?;
    if session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? != Status::Playing {
        return None;
    }
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().ok()?.to_string();
    if title.is_empty() {
        return None;
    }
    let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
    // "Spotify.exe", "MSEdge", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"
    let source = session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default();
    let app = source.rsplit(['\\', '!']).next().unwrap_or(&source).trim_end_matches(".exe").to_ascii_lowercase();
    let artwork = ART.with(|a| {
        let mut a = a.borrow_mut();
        if a.0 != title || a.1 != artist {
            let read = || -> Option<String> {
                let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
                let size = (stream.Size().ok()? as u32).min(2 << 20);
                let kind = stream.ContentType().map(|t| t.to_string()).unwrap_or_default();
                let reader = DataReader::CreateDataReader(&stream).ok()?;
                reader.SetInputStreamOptions(InputStreamOptions::ReadAhead).ok()?;
                let n = reader.LoadAsync(size).ok()?.join().ok()?;
                let mut bytes = vec![0u8; n as usize];
                reader.ReadBytes(&mut bytes).ok()?;
                let kind = if kind.starts_with("image/") { kind } else { "image/jpeg".into() };
                Some(format!("data:{kind};base64,{}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)))
            };
            *a = (title.clone(), artist.clone(), read().unwrap_or_default());
        }
        a.2.clone()
    });
    Some(json!({"app": app, "title": title, "artist": artist, "album": album, "artwork": artwork}))
}

/// Keeps the widgets' "now_playing" up to date while one shows it.
pub fn start(widgets: lightair_widgets::Widgets) {
    let _ = std::thread::Builder::new().name("lightair-now-playing".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(3));
            if widgets.has_kind("now-playing") {
                widgets.set_data("now_playing", now());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "depends on this computer: something playing; cargo xtask test --machine"]
    fn what_plays_is_read() {
        let now = super::now();
        eprintln!("now playing: {}", serde_json::to_string(&now).unwrap_or_default().chars().take(300).collect::<String>());
        assert!(now.get("title").and_then(|t| t.as_str()).is_some_and(|t| !t.is_empty()), "{now}");
    }
}
