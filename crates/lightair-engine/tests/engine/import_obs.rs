//! An imported OBS collection reconciles into libobs, including plugin sources and filters
//! that aren't installed (they must not take the engine down). No cameras or microphones here,
//! so the test never triggers a permission prompt.

use lightair_model::{Document, Workspace, diff};
use lightair_engine::{Engine, EngineConfig};

const COLLECTION: &str = r#"{
  "name": "Engine import",
  "current_program_scene": "Main",
  "current_transition": "Cut",
  "scene_order": [{"name": "Main"}, {"name": "Other"}],
  "sources": [
    {"id": "color_source", "versioned_id": "color_source_v3", "name": "Red", "uuid": "red",
     "settings": {"color": 4278190335, "width": 640, "height": 360},
     "filters": [
       {"id": "color_filter", "versioned_id": "color_filter_v2", "name": "Grade", "settings": {"saturation": -0.5}},
       {"id": "not_installed_filter", "versioned_id": "not_installed_filter", "name": "Ghost", "settings": {}}
     ]},
    {"id": "not_installed_source", "versioned_id": "not_installed_source", "name": "Ghost source", "uuid": "ghost", "settings": {"a": 1}},
    {"id": "image_source", "versioned_id": "image_source", "name": "Board", "uuid": "board", "settings": {"file": "BOARD"}},
    {"id": "scene", "name": "Main", "uuid": "main", "settings": {"items": [
      {"name": "Red", "source_uuid": "red", "align": 5, "pos": {"x": 0, "y": 0}, "scale": {"x": 1, "y": 1}, "visible": true},
      {"name": "Ghost source", "source_uuid": "ghost", "align": 5, "pos": {"x": 10, "y": 10}, "scale": {"x": 1, "y": 1}, "visible": true}
    ]}},
    {"id": "scene", "name": "Other", "uuid": "other", "settings": {"items": [
      {"name": "Board", "source_uuid": "board", "align": 5, "pos": {"x": 10, "y": 20}, "scale": {"x": 0.1, "y": 0.1}, "visible": true},
      {"name": "Red", "source_uuid": "red", "align": 0, "bounds_type": 2, "bounds": {"x": 320, "y": 180}, "pos": {"x": 320, "y": 180}, "scale": {"x": 1, "y": 1}, "visible": true}
    ]}}
  ]
}"#;

#[test]
fn imported_collection_reconciles() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Import"));
    let board = concat!(env!("CARGO_MANIFEST_DIR"), "/../../branding/final/board.png");
    // (a Windows path's backslashes would be escapes in the JSON)
    let collection = COLLECTION.replace("BOARD", &board.replace('\\', "/"));
    let (cmd, report) = lightair_store::import_obs_collection(&collection, doc.workspace()).unwrap();
    assert_eq!(report.kept_plugin.len(), 2, "{report:#?}");
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    std::thread::sleep(std::time::Duration::from_millis(200));
    let described = engine.call(|s| s.reconciler.describe());
    eprintln!("{described:#?}");
    let (sources, scenes, items) = engine.call(|s| s.reconciler.counts());
    assert_eq!((sources, scenes), (3, 2), "inputs and scenes exist in libobs");
    assert_eq!(items, 4);

    // The board's size was guessed (1920x1080); the image is 1800x1700. Hold it showing off
    // air, read the real size, and refit.
    let est = report.estimated.iter().find(|e| e.layout.width == 192.0).expect("board was estimated").clone();
    let source = est.source;
    engine.call(move |s| s.reconciler.hold_sources(&[source]));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let size = loop {
        if let Some(size) = engine.call(move |s| s.reconciler.source_size(source)) {
            break size;
        }
        assert!(std::time::Instant::now() < deadline, "image reports its size");
        std::thread::sleep(std::time::Duration::from_millis(30));
    };
    assert_eq!(size, (1800, 1700));
    let fixed = est.refit(size);
    assert_eq!((fixed.x, fixed.y, fixed.width, fixed.height), (10.0, 20.0, 180.0, 170.0));
    engine.call(|s| s.reconciler.release_sources());

    // Undo removes everything it added.
    let change = doc.undo().unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    assert_eq!(engine.call(|s| s.reconciler.counts()), (0, 0, 0));
    drop(engine);
}
