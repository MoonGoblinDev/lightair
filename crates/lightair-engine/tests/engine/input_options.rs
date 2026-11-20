//! A source's list choices are read from the source itself (cameras, displays, windows, mics use
//! the same path; a slideshow's list needs no permission).

use lightair_model::{Command, Document, Source, Workspace, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;

#[test]
fn list_choices_come_from_the_source() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let engine = Engine::start(EngineConfig { canvas: (320, 180), output: (320, 180), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Options"));
    let show = Source::new("Slides", "slideshow_v2", json!({}));
    let id = show.id;
    let change = doc.apply(&Command::AddSource { source: show }).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
    let options = engine.call(move |s| s.reconciler.source_options(id, "transition"));
    eprintln!("{options:?}");
    assert!(options.len() >= 3, "slideshow offers its transitions: {options:?}");
    assert!(options.iter().all(|(label, v)| !label.is_empty() && v.is_string()));
    assert!(engine.call(move |s| s.reconciler.source_options(id, "nope")).is_empty());
    drop(engine);
}
