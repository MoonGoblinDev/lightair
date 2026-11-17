//! What sets automations off from outside Lightair: files other apps save, MIDI controllers and
//! game controllers. Devices are opened only while an automation listens to them.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use lightair_automation::Automations;

/// Starts the watchers (each a thread, idle while nothing listens).
pub fn watch(a: Automations) {
    let files = a.clone();
    let _ = std::thread::Builder::new().name("lightair-trigger-files".into()).spawn(move || watch_files(files));
    let midi = a.clone();
    let _ = std::thread::Builder::new().name("lightair-trigger-midi".into()).spawn(move || watch_midi(midi));
    let _ = std::thread::Builder::new().name("lightair-trigger-controllers".into()).spawn(move || watch_controllers(a));
}

/// The start of a file's text (a counter, a line a bot wrote), trimmed.
fn head(path: &str) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; 4096];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).trim().to_string()
}

fn watch_files(a: Automations) {
    // path → (modified, length), from the last look
    let mut seen: HashMap<String, (SystemTime, u64)> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let paths = a.watched_files();
        seen.retain(|p, _| paths.contains(p));
        for path in paths {
            let expanded = shellexpand(&path);
            let Ok(meta) = std::fs::metadata(&expanded) else { continue };
            let now = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
            match seen.insert(path.clone(), now) {
                Some(before) if before != now => a.file_changed(&path, &head(&expanded)),
                _ => {}
            }
        }
    }
}

/// "~/x" → the home folder's x (Windows: "~\x" too, and the home is USERPROFILE: HOME is
/// rarely set there).
fn shellexpand(path: &str) -> String {
    let rest = path.strip_prefix("~/").or_else(|| if cfg!(windows) { path.strip_prefix("~\\") } else { None });
    let home = std::env::var("HOME").ok().or_else(|| if cfg!(windows) { std::env::var("USERPROFILE").ok() } else { None });
    match (rest, home) {
        (Some(rest), Some(home)) => format!("{home}/{rest}"),
        _ => path.to_string(),
    }
}

/// A MIDI message → (kind, channel 1–16, number, value), for notes pressed and controls moved.
pub fn midi_message(bytes: &[u8]) -> Option<(&'static str, u8, u8, u8)> {
    let (&status, number, value) = (bytes.first()?, *bytes.get(1)?, *bytes.get(2)?);
    let channel = (status & 0x0f) + 1;
    match status & 0xf0 {
        0x90 if value > 0 => Some(("note", channel, number, value)),
        0xb0 => Some(("cc", channel, number, value)),
        _ => None,
    }
}

fn watch_midi(a: Automations) {
    // port name → its open connection
    let mut open: HashMap<String, midir::MidiInputConnection<()>> = HashMap::new();
    loop {
        std::thread::sleep(Duration::from_secs(3));
        if !a.wants("midi") {
            open.clear();
            continue;
        }
        let Ok(probe) = midir::MidiInput::new("Lightair") else { continue };
        let ports = probe.ports();
        let names: Vec<(String, midir::MidiInputPort)> = ports.into_iter().filter_map(|p| Some((probe.port_name(&p).ok()?, p))).collect();
        open.retain(|name, _| names.iter().any(|(n, _)| n == name));
        for (name, port) in names {
            if open.contains_key(&name) {
                continue;
            }
            let Ok(input) = midir::MidiInput::new("Lightair") else { continue };
            let (a, device) = (a.clone(), name.clone());
            match input.connect(&port, "lightair-automations", move |_, bytes, _| {
                if let Some((kind, channel, number, value)) = midi_message(bytes) {
                    a.midi(kind, channel, number, value, &device);
                }
            }, ()) {
                Ok(conn) => {
                    tracing::info!(device = name, "MIDI device listened to");
                    open.insert(name, conn);
                }
                Err(e) => tracing::debug!(%e, "MIDI device"),
            }
        }
    }
}

fn watch_controllers(a: Automations) {
    let mut pads: Option<gilrs::Gilrs> = None;
    loop {
        if !a.wants("controller") {
            pads = None;
            std::thread::sleep(Duration::from_secs(2));
            continue;
        }
        if pads.is_none() {
            pads = gilrs::Gilrs::new().map_err(|e| tracing::debug!(%e, "game controllers")).ok();
        }
        let Some(g) = pads.as_mut() else {
            std::thread::sleep(Duration::from_secs(10));
            continue;
        };
        while let Some(gilrs::Event { id, event, .. }) = g.next_event() {
            if let gilrs::EventType::ButtonPressed(button, _) = event {
                let name = g.gamepad(id).name().to_string();
                a.controller(&format!("{button:?}"), &name);
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn midi_notes_and_controls() {
        assert_eq!(super::midi_message(&[0x99, 36, 100]), Some(("note", 10, 36, 100)));
        // a note-on at velocity 0 is a note-off
        assert_eq!(super::midi_message(&[0x90, 36, 0]), None);
        assert_eq!(super::midi_message(&[0xb1, 7, 64]), Some(("cc", 2, 7, 64)));
        assert_eq!(super::midi_message(&[0xf8]), None);
    }
}
