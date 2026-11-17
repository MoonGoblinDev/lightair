//! Import an OBS scene collection (docs/14-data-persistence-settings.md, "Importing from OBS").
//!
//! Lightair runs on libobs, so OBS sources become inputs of the same libobs type with the same
//! settings, and OBS filters become input effects. What needs translating is geometry (OBS
//! items are an alignment point plus scale or bounds; Lightair objects are boxes), global audio
//! devices (they become mixer tracks), and plugins Lightair replaces with built-in features.

use std::collections::{HashMap, HashSet};

use lightair_core::{
    Command, Crop, Direction, Effect, Fit, Canvas, Id, Source, Object, Layout, Workspace, Scene, Track, TransitionKind,
    TransitionSettings,
};
use serde_json::Value;

/// What the import did, in the four groups the docs describe.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportReport {
    /// Converted as is.
    pub converted: Vec<String>,
    /// OBS plugins replaced by a built-in Lightair feature.
    pub replaced: Vec<String>,
    /// Plugin sources/filters kept as plugin objects (they work if the plugin is installed).
    pub kept_plugin: Vec<String>,
    /// Imported with an approximation, or not imported; the user should look.
    pub attention: Vec<String>,
    /// Objects whose box was computed from a guessed source size; `refit` corrects them once
    /// the real size is known.
    pub estimated: Vec<EstimatedObject>,
}

/// A object imported from an OBS item whose source size had to be guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct EstimatedObject {
    pub scene: Id,
    pub object: Id,
    pub source: Id,
    pub canvas: Id,
    /// The layout as imported (a object the user has since changed is left alone).
    pub layout: Layout,
    /// The OBS scene item, for recomputing the box.
    item: Value,
}

impl EstimatedObject {
    /// The object's layout for the source's real pixel size.
    pub fn refit(&self, size: (u32, u32)) -> Layout {
        layout_of(&self.item, (size.0 as f32, size.1 as f32))
    }
}

impl ImportReport {
    pub fn summary(&self) -> String {
        format!(
            "{} converted, {} replaced by built-in features, {} kept as plugins, {} need attention",
            self.converted.len(),
            self.replaced.len(),
            self.kept_plugin.len(),
            self.attention.len()
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not an OBS scene collection: {0}")]
    Invalid(String),
}

/// OBS core source types (other ids are plugin sources).
const CORE_INPUTS: &[&str] = &[
    "color_source",
    "color_source_v2",
    "color_source_v3",
    "image_source",
    "slideshow",
    "slideshow_v2",
    "ffmpeg_source",
    "vlc_source",
    "text_ft2_source",
    "text_ft2_source_v2",
    "screen_capture",
    "display_capture",
    "window_capture",
    "macos-avcapture",
    "macos-avcapture-fast",
    "av_capture_input",
    "av_capture_input_v2",
    "coreaudio_input_capture",
    "coreaudio_output_capture",
    "sck_audio_capture",
    "syphon-input",
    "browser_source",
];

/// OBS core filter types (other ids are plugin filters).
const CORE_FILTERS: &[&str] = &[
    "mask_filter",
    "mask_filter_v2",
    "crop_filter",
    "gain_filter",
    "color_filter",
    "color_filter_v2",
    "scale_filter",
    "scroll_filter",
    "gpu_delay",
    "color_key_filter",
    "color_key_filter_v2",
    "clut_filter",
    "sharpness_filter",
    "sharpness_filter_v2",
    "chroma_key_filter",
    "chroma_key_filter_v2",
    "async_delay_filter",
    "noise_suppress_filter",
    "noise_suppress_filter_v2",
    "invert_polarity_filter",
    "noise_gate_filter",
    "compressor_filter",
    "limiter_filter",
    "expander_filter",
    "upward_compressor_filter",
    "luma_key_filter",
    "luma_key_filter_v2",
    "basic_eq_filter",
    "hdr_tonemap_filter",
    "rtx_greenscreen_filter",
];

/// Filters Lightair replaces with built-in features: (OBS id, Lightair kind or None, what).
const REPLACED_FILTERS: &[(&str, Option<&str>, &str)] = &[
    ("obs_beauty_filter", Some("lightair_camera_studio"), "Dewy → Effects"),
    ("background_removal", None, "Background removal → Effects background (coming)"),
    ("vision_background_removal_filter", None, "Background removal → Effects background (coming)"),
];

/// Sources with no picture: in a scene they become mixer tracks, not objects.
fn is_audio_only(kind: &str) -> bool {
    [
        "coreaudio_input_capture",
        "coreaudio_output_capture",
        "sck_audio_capture",
        "wasapi_input_capture",
        "wasapi_output_capture",
        "wasapi_process_output_capture",
        "pulse_input_capture",
        "pulse_output_capture",
        "alsa_input_capture",
        "jack_output_capture",
    ]
    .contains(&kind)
}

fn volume_db(volume: f64) -> f32 {
    if volume > 0.0 { (20.0 * volume.log10()) as f32 } else { f32::NEG_INFINITY }
}

/// OBS's capture sources become Lightair's portable kinds (the workspace opens on either platform).
fn portable(mut input: Source) -> Source {
    if let Some((kind, settings)) = lightair_core::kinds::from_libobs(&input.kind, &input.settings) {
        input.kind = kind.into();
        input.settings = settings;
    }
    input
}

fn kind_of(v: &Value) -> String {
    v.get("versioned_id").or(v.get("id")).and_then(Value::as_str).unwrap_or("").to_string()
}

fn name_of(v: &Value) -> String {
    v.get("name").and_then(Value::as_str).unwrap_or("").to_string()
}

fn xy(v: &Value, key: &str) -> (f32, f32) {
    let p = &v[key];
    (p["x"].as_f64().unwrap_or(0.0) as f32, p["y"].as_f64().unwrap_or(0.0) as f32)
}

/// Best-known pixel size of a source (OBS doesn't save it): from its settings when they say,
/// otherwise a typical size. `exact` is false when it's a guess.
fn source_size(kind: &str, settings: &Value, canvas: (u32, u32)) -> ((f32, f32), bool) {
    let w = settings["width"].as_f64();
    let h = settings["height"].as_f64();
    match kind {
        k if k.starts_with("color_source") => {
            ((w.unwrap_or(canvas.0 as f64) as f32, h.unwrap_or(canvas.1 as f64) as f32), true)
        }
        "screen_capture" | "display_capture" => ((canvas.0 as f32, canvas.1 as f32), false),
        _ => ((1920.0, 1080.0), false),
    }
}

/// OBS item geometry → Lightair layout box.
fn layout_of(item: &Value, size: (f32, f32)) -> Layout {
    let (px, py) = xy(item, "pos");
    let (sx, sy) = xy(item, "scale");
    let bounds_type = item["bounds_type"].as_i64().unwrap_or(0);
    let (w, h, fit) = match bounds_type {
        0 => (size.0 * sx.abs(), size.1 * sy.abs(), Fit::Stretch),
        _ => {
            let (bw, bh) = xy(item, "bounds");
            let fit = match bounds_type {
                1 => Fit::Stretch,
                3 => Fit::Fill,
                _ => Fit::Fit,
            };
            (bw, bh, fit)
        }
    };
    // OBS alignment flags: LEFT 1, RIGHT 2, TOP 4, BOTTOM 8 (none = centre). `pos` is that point.
    let align = item["align"].as_i64().unwrap_or(5);
    let x = if align & 1 != 0 {
        px
    } else if align & 2 != 0 {
        px - w
    } else {
        px - w / 2.0
    };
    let y = if align & 4 != 0 {
        py
    } else if align & 8 != 0 {
        py - h
    } else {
        py - h / 2.0
    };
    let crop = |k: &str| item[k].as_i64().unwrap_or(0).max(0) as u32;
    Layout {
        x,
        y,
        width: w.max(1.0),
        height: h.max(1.0),
        rotation: item["rot"].as_f64().unwrap_or(0.0) as f32,
        crop: Crop {
            left: crop("crop_left"),
            top: crop("crop_top"),
            right: crop("crop_right"),
            bottom: crop("crop_bottom"),
        },
        fit,
        visible: item["visible"].as_bool().unwrap_or(true),
        opacity: 1.0,
        shape: None,
        // OBS flips with a negative scale.
        flip_h: sx < 0.0,
        flip_v: sy < 0.0,
        tilt_x: 0.0,
        tilt_y: 0.0,
        scale_x: 1.0,
        scale_y: 1.0,
    }
}

fn effects_of(source: &Value, report: &mut ImportReport) -> Vec<Effect> {
    let mut out = Vec::new();
    for f in source["filters"].as_array().into_iter().flatten() {
        let kind = kind_of(f);
        let base = f["id"].as_str().unwrap_or(&kind).to_string();
        let label = format!("{} › {}", name_of(source), name_of(f));
        let enabled = f["enabled"].as_bool().unwrap_or(true);
        if let Some((_, to, what)) = REPLACED_FILTERS.iter().find(|(id, ..)| *id == base) {
            match to {
                Some(kind) => {
                    let mut e = Effect::new(name_of(f), *kind, f["settings"].clone());
                    e.name = "Effects".into();
                    e.enabled = enabled;
                    out.push(e);
                    report.replaced.push(format!("{label}: {what}"));
                }
                None => report.attention.push(format!("{label}: {what}")),
            }
            continue;
        }
        let mut e = Effect::new(name_of(f), kind.clone(), f["settings"].clone());
        e.enabled = enabled;
        out.push(e);
        if CORE_FILTERS.contains(&base.as_str()) || CORE_FILTERS.contains(&kind.as_str()) {
            report.converted.push(format!("filter {label}"));
        } else {
            report.kept_plugin.push(format!("filter {label} ({kind})"));
        }
    }
    out
}

/// Converts an OBS scene collection into commands that add its scenes, inputs and audio tracks
/// to `workspace` as one undo step. The workspace's primary canvas is used for geometry.
pub fn import_obs_collection(json: &str, workspace: &Workspace) -> Result<(Command, ImportReport), ImportError> {
    let root: Value = serde_json::from_str(json).map_err(|e| ImportError::Invalid(e.to_string()))?;
    let sources = root["sources"].as_array().ok_or_else(|| ImportError::Invalid("no sources".into()))?;
    let primary: Canvas = workspace.primary_canvas().clone();
    let canvas = (primary.width, primary.height);
    let mut report = ImportReport::default();
    let mut commands = Vec::new();

    // Sources (every non-scene source), keyed by OBS uuid and name.
    let mut inputs: HashMap<String, (Id, String, Value)> = HashMap::new();
    let mut source_by_uuid: HashMap<String, &Value> = HashMap::new();
    let mut tracked: HashSet<Id> = HashSet::new();
    let mut by_name: HashMap<String, String> = HashMap::new();
    for s in sources {
        let kind = kind_of(s);
        let uuid = s["uuid"].as_str().unwrap_or("").to_string();
        by_name.insert(name_of(s), uuid.clone());
        source_by_uuid.insert(uuid.clone(), s);
        if kind == "scene" {
            continue;
        }
        if kind == "group" {
            report
                .attention
                .push(format!("group {}: groups arrive in a later version; its objects were skipped", name_of(s)));
            continue;
        }
        let mut input = portable(Source::new(name_of(s), kind.clone(), s["settings"].clone()));
        input.effects = effects_of(s, &mut report);
        let base = s["id"].as_str().unwrap_or(&kind);
        if CORE_INPUTS.contains(&base) {
            report.converted.push(format!("input {} ({kind})", input.name));
        } else {
            report.kept_plugin.push(format!("input {} ({kind})", input.name));
        }
        inputs.insert(uuid, (input.id, kind, s["settings"].clone()));
        commands.push(Command::AddSource { source: input });
    }

    // Scenes in OBS order.
    let order: Vec<String> = root["scene_order"]
        .as_array()
        .map(|a| a.iter().map(name_of).collect())
        .unwrap_or_else(|| sources.iter().filter(|s| kind_of(s) == "scene").map(name_of).collect());
    let mut scene_ids: HashMap<String, Id> = HashMap::new();
    let mut tracks = Vec::new();
    let mut taken: HashSet<String> = workspace.scenes.iter().map(|s| s.name.clone()).collect();
    for scene_name in &order {
        let Some(src) = sources.iter().find(|s| kind_of(s) == "scene" && name_of(s) == *scene_name) else { continue };
        // Keep names unique next to the workspace's own scenes.
        let mut name = scene_name.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{scene_name} ({n})");
            n += 1;
        }
        taken.insert(name.clone());
        let mut scene = Scene::new(name);
        let mut estimated = Vec::new();
        for item in src["settings"]["items"].as_array().into_iter().flatten() {
            let uuid = item["source_uuid"]
                .as_str()
                .map(str::to_string)
                .or_else(|| item["name"].as_str().and_then(|n| by_name.get(n).cloned()))
                .unwrap_or_default();
            let Some((input_id, kind, settings)) = inputs.get(&uuid) else {
                report.attention.push(format!(
                    "{scene_name} › {}: nested scenes and groups arrive in a later version; skipped",
                    name_of(item)
                ));
                continue;
            };
            if is_audio_only(kind) {
                if tracked.insert(*input_id) {
                    let src = source_by_uuid[&uuid];
                    let mut track = Track::new(name_of(src), *input_id);
                    track.volume_db = volume_db(src["volume"].as_f64().unwrap_or(1.0));
                    track.muted = src["muted"].as_bool().unwrap_or(false);
                    track.cue = src["monitoring_type"].as_i64().unwrap_or(0) != 0;
                    report.attention.push(format!(
                        "{}: was audio in scene {scene_name}; it's now a mixer track that plays in every scene",
                        track.name
                    ));
                    tracks.push(Command::AddTrack { track, index: None });
                }
                continue;
            }
            let (size, exact) = source_size(kind, settings, canvas);
            let layout = layout_of(item, size);
            let guessed = item["bounds_type"].as_i64().unwrap_or(0) == 0 && !exact;
            if guessed || layout.rotation != 0.0 {
                estimated.push(name_of(item));
            }
            let mut object = Object::new(name_of(item), *input_id, [(primary.id, layout)]);
            if guessed {
                report.estimated.push(EstimatedObject {
                    scene: scene.id,
                    object: object.id,
                    source: *input_id,
                    canvas: primary.id,
                    layout,
                    item: item.clone(),
                });
            }
            object.locked = item["locked"].as_bool().unwrap_or(false);
            scene.objects.push(object);
        }
        if !estimated.is_empty() {
            report.attention.push(format!(
                "{scene_name}: sizes estimated for {} (OBS doesn't save source sizes); corrected once the sources start",
                estimated.join(", ")
            ));
        }
        scene_ids.insert(scene_name.clone(), scene.id);
        report.converted.push(format!("scene {scene_name}"));
        commands.push(Command::AddScene { scene, index: None });
    }

    commands.extend(tracks);

    // Global audio devices become mixer tracks.
    for key in [
        "DesktopAudioDevice1",
        "DesktopAudioDevice2",
        "AuxAudioDevice1",
        "AuxAudioDevice2",
        "AuxAudioDevice3",
        "AuxAudioDevice4",
    ] {
        let s = &root[key];
        if !s.is_object() {
            continue;
        }
        let mut input = portable(Source::new(name_of(s), kind_of(s), s["settings"].clone()));
        input.effects = effects_of(s, &mut report);
        let mut track = Track::new(name_of(s), input.id);
        let volume = s["volume"].as_f64().unwrap_or(1.0);
        track.volume_db = volume_db(volume);
        track.muted = s["muted"].as_bool().unwrap_or(false);
        track.cue = s["monitoring_type"].as_i64().unwrap_or(0) != 0;
        report.converted.push(format!("audio track {}", track.name));
        commands.push(Command::AddSource { source: input });
        commands.push(Command::AddTrack { track: track.clone(), index: None });
    }

    // Transition: OBS saves the current one by name; custom ones are listed with their type.
    if let Some(name) = root["current_transition"].as_str() {
        let saved = root["transitions"].as_array().and_then(|a| a.iter().find(|t| name_of(t) == name));
        let lower = name.to_lowercase();
        let id = saved
            .map(kind_of)
            .unwrap_or_else(|| if lower.contains("cut") { "cut_transition" } else { "fade_transition" }.to_string());
        let settings = saved.map(|t| t["settings"].clone()).unwrap_or(Value::Null);
        let direction = match settings["direction"].as_str() {
            Some("right") => Direction::Right,
            Some("up") => Direction::Up,
            Some("down") => Direction::Down,
            _ => Direction::Left,
        };
        let kind = match id.as_str() {
            "cut_transition" => TransitionKind::Cut,
            "slide_transition" => TransitionKind::Slide,
            "swipe_transition" | "wipe_transition" => TransitionKind::Wipe,
            "fade_to_color_transition" => TransitionKind::Dip,
            "move_transition" => {
                report.replaced.push(format!("transition {name}: Move plugin → built-in Move"));
                TransitionKind::Move
            }
            "fade_transition" => TransitionKind::Fade,
            "obs_stinger_transition" => match settings["path"].as_str().filter(|p| !p.is_empty()) {
                Some(path) => {
                    // OBS times the cut in ms or frames; frames are approximated at 30 fps.
                    let point = settings["transition_point"].as_u64().unwrap_or(0);
                    let cut_ms = if settings["tp_type"].as_i64() == Some(1) { point * 1000 / 30 } else { point };
                    commands.push(Command::SetStinger {
                        stinger: Some(lightair_core::Stinger { path: path.to_string(), cut_ms: cut_ms as u32 }),
                    });
                    report.converted.push(format!("transition {name}: stinger"));
                    TransitionKind::Stinger
                }
                None => TransitionKind::Fade,
            },
            other => {
                report.attention.push(format!("transition {name} ({other}): not built in yet; using Fade"));
                TransitionKind::Fade
            }
        };
        let duration_ms = root["transition_duration"].as_u64().unwrap_or(300) as u32;
        commands.push(Command::SetTransition { settings: TransitionSettings { kind, duration_ms, direction } });
    }

    // What was on air.
    if let Some(program) =
        root["current_program_scene"].as_str().or(root["current_scene"].as_str()).and_then(|n| scene_ids.get(n))
    {
        commands.push(Command::SetProgram { scene: Some(*program) });
    }

    let label = format!("Import {}", root["name"].as_str().unwrap_or("OBS scenes"));
    Ok((Command::Batch { label, commands }, report))
}
