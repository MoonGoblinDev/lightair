//! OBS scene-collection import against a synthetic collection.

use lightair_core::{Command, Document, Fit, Workspace, TransitionKind};
use lightair_store::import_obs_collection;

const FIXTURE: &str = include_str!("../fixtures/obs-collection.json");

fn imported() -> (Workspace, lightair_store::ImportReport) {
    let mut doc = Document::new(Workspace::new("Mine"));
    let (cmd, report) = import_obs_collection(FIXTURE, doc.workspace()).unwrap();
    assert!(matches!(&cmd, Command::Batch { label, .. } if label == "Import Test Show"));
    doc.apply(&cmd).expect("batch applies");
    (doc.workspace().as_ref().clone(), report)
}

#[test]
fn scenes_inputs_and_order() {
    let (p, _) = imported();
    let names: Vec<_> = p.scenes.iter().map(|s| s.name.as_str()).collect();
    assert!(names.ends_with(&["Live", "Break"]), "{names:?}");
    let live = p.scenes.iter().find(|s| s.name == "Live").unwrap();
    assert_eq!(p.program, Some(live.id), "program is OBS's program scene");
    assert_eq!(live.objects.len(), 3);
    let cam = p.sources.values().find(|i| i.name == "Camera").unwrap();
    assert_eq!(cam.kind, lightair_core::kinds::CAMERA, "a portable kind");
    assert_eq!(cam.settings["device"], "cam-1");
    let bg = p.sources.values().find(|i| i.name == "Backdrop").unwrap();
    assert_eq!(bg.kind, "color_source_v3", "versioned id wins");
    // The nested scene item in Break is skipped.
    assert_eq!(p.scenes.iter().find(|s| s.name == "Break").unwrap().objects.len(), 1);
}

#[test]
fn geometry() {
    let (p, _) = imported();
    let f = p.primary_canvas().id;
    let live = p.scenes.iter().find(|s| s.name == "Live").unwrap();
    let bg = &live.objects[0].layouts[&f];
    // Centre-aligned at (960,540), 1920x1080 at half scale.
    assert_eq!((bg.x, bg.y, bg.width, bg.height), (480.0, 270.0, 960.0, 540.0));
    assert_eq!((bg.crop.left, bg.crop.bottom), (10, 20));
    // Bottom-right aligned bounds box ending at (1900,1060).
    let cam = &live.objects[1].layouts[&f];
    assert_eq!((cam.x, cam.y, cam.width, cam.height), (1260.0, 700.0, 640.0, 360.0));
    assert_eq!(cam.fit, Fit::Fit);
    assert!(!cam.visible);
    assert!(p.scenes.iter().find(|s| s.name == "Break").unwrap().objects[0].locked);
}

#[test]
fn effects_audio_and_transition() {
    let (p, report) = imported();
    let cam = p.sources.values().find(|i| i.name == "Camera").unwrap();
    let kinds: Vec<_> = cam.effects.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["lightair_camera_studio", "chroma_key_filter_v2", "shaderfilter"]);
    assert_eq!(cam.effects[0].name, "Effects");
    assert_eq!(cam.effects[0].settings["smooth"], 0.4, "Dewy settings carry over");
    assert!(!cam.effects[1].enabled);

    let desk = p.tracks.iter().find(|t| t.name == "Desktop Audio").unwrap();
    assert!((desk.volume_db + 6.0206).abs() < 0.01, "{}", desk.volume_db);
    assert!(desk.muted);
    let mic = p.tracks.iter().find(|t| t.name == "Mic/Aux").unwrap();
    assert!(mic.cue);
    let mic_source = p.sources.values().find(|i| i.id == mic.source).unwrap();
    assert_eq!(mic_source.effects[0].kind, "noise_suppress_filter_v2");

    assert_eq!(p.transition.kind, TransitionKind::Fade);
    assert_eq!(p.transition.duration_ms, 450);

    eprintln!("{report:#?}");
    assert_eq!(report.replaced.len(), 1, "Dewy → Effects");
    assert!(report.kept_plugin.iter().any(|s| s.contains("spectralizer")));
    assert!(report.kept_plugin.iter().any(|s| s.contains("shaderfilter")));
    assert!(report.attention.iter().any(|s| s.contains("Background removal")));
    assert!(report.attention.iter().any(|s| s.contains("nested")));
    assert!(report.attention.iter().any(|s| s.contains("Viz") && s.contains("estimated")));
}

#[test]
fn names_stay_unique_and_import_is_one_undo_step() {
    let mut doc = Document::new(Workspace::new("Mine"));
    for _ in 0..2 {
        let (cmd, _) = import_obs_collection(FIXTURE, doc.workspace()).unwrap();
        doc.apply(&cmd).unwrap();
    }
    let names: Vec<_> = doc.workspace().scenes.iter().map(|s| s.name.clone()).collect();
    assert!(names.contains(&"Live (2)".to_string()), "{names:?}");
    let before = doc.workspace().scenes.len();
    doc.undo();
    assert_eq!(doc.workspace().scenes.len(), before - 2);
}

#[test]
fn estimated_objects_refit_to_the_real_size() {
    let (cmd, report) = import_obs_collection(FIXTURE, &Workspace::new("Mine")).unwrap();
    drop(cmd);
    // Viz is a plugin source at scale 0.25 with top-left alignment at (100,100); its size was
    // guessed as 1920x1080. The colour backdrop's size is in its settings, so it's exact.
    let names: Vec<_> = report.estimated.iter().map(|e| e.layout.width).collect();
    assert_eq!(names, [480.0], "only Viz was guessed");
    let viz = &report.estimated[0];
    let real = viz.refit((800, 400));
    assert_eq!((real.x, real.y, real.width, real.height), (100.0, 100.0, 200.0, 100.0));
}

#[test]
fn stinger_transition_carries_over() {
    let json = r#"{"name": "S", "sources": [], "current_transition": "Swoosh",
        "transitions": [{"id": "obs_stinger_transition", "name": "Swoosh",
                         "settings": {"path": "/tmp/swoosh.webm", "transition_point": 650, "tp_type": 0}}]}"#;
    let mut doc = Document::new(Workspace::new("Mine"));
    let (cmd, _) = import_obs_collection(json, doc.workspace()).unwrap();
    doc.apply(&cmd).unwrap();
    assert_eq!(doc.workspace().transition.kind, TransitionKind::Stinger);
    let st = doc.workspace().stinger.clone().unwrap();
    assert_eq!((st.path.as_str(), st.cut_ms), ("/tmp/swoosh.webm", 650));
}

#[test]
fn rejects_garbage() {
    assert!(import_obs_collection("{}", &Workspace::new("x")).is_err());
    assert!(import_obs_collection("nope", &Workspace::new("x")).is_err());
}
