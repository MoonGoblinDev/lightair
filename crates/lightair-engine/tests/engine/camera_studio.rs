//! Lightair Effect Stack loads as an in-tree libobs module and renders without errors.
//! Run `cargo xtask modules` first; the test is skipped when the plugin isn't built.

use std::path::PathBuf;
use std::time::Duration;

use lightair_engine::obs::{Data, ObsScene, ObsSource};
use lightair_engine::{Engine, EngineConfig};

#[test]
fn camera_studio_module_loads_and_renders() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    // macOS: the .plugin bundle; Windows: OBS's layout
    let (dir, module) = if cfg!(windows) {
        (built.join("obs-plugins/64bit"), "lightair-effect-stack.dll")
    } else {
        (built, "lightair-effect-stack.plugin")
    };
    if !dir.join(module).exists() {
        eprintln!("{module} not built; run `cargo xtask modules`");
        return;
    }
    let engine = Engine::start(EngineConfig {
        canvas: (1280, 720),
        output: (1280, 720),
        extra_plugin_dirs: vec![dir],
        ..Default::default()
    })
    .unwrap();
    assert!(
        engine.capabilities.modules.iter().any(|m| m.contains("lightair-effect-stack")),
        "{:?}",
        engine.capabilities.modules
    );
    assert!(engine.capabilities.filters.iter().any(|f| f == "lightair_camera_studio"), "filter registered");

    let rendered = engine.call(|s| {
        let mut colour = Data::new();
        colour.set_int("color", 0xFF8899AAu32 as i64).set_int("width", 1280).set_int("height", 720);
        let src = ObsSource::create("color_source_v3", "Skin-ish", Some(&colour)).unwrap();
        let mut look = Data::new();
        look.set_int("smooth", 60).set_int("foundation", 40);
        let filter = ObsSource::create_private("lightair_camera_studio", "Lightair Effect Stack", Some(&look)).unwrap();
        unsafe { lightair_engine::sys::obs_source_filter_add(src.as_ptr(), filter.as_ptr()) };
        let scene = ObsScene::create("Main").unwrap();
        scene.add(&src);
        s.sources.insert("src".into(), src);
        s.sources.insert("filter".into(), filter);
        s.scenes.insert("main".into(), scene);
        s.set_program("main")
    });
    assert!(rendered);
    // First frames compile the filter's shaders; measure steady state after warm-up.
    std::thread::sleep(Duration::from_millis(1000));
    let warm = engine.call(|s| s.stats());
    std::thread::sleep(Duration::from_millis(1000));
    let stats = engine.call(|s| s.stats());
    eprintln!("with Lightair Effect Stack, steady state: {stats:?} (lagged during warm-up: {})", warm.lagged_frames);
    // The filter must not starve the renderer once warm. Timing means nothing while other
    // tests share the GPU (`cargo xtask test` says so); `cargo test` checks it.
    let shared = std::env::var_os("LIGHTAIR_TESTS_SHARE_GPU").is_some();
    assert!(stats.total_frames - warm.total_frames > if shared { 15 } else { 50 }, "kept rendering");
    if !shared {
        assert!(
            stats.lagged_frames - warm.lagged_frames <= 2,
            "lagged {} after warm-up",
            stats.lagged_frames - warm.lagged_frames
        );
    }
    engine.call(|s| {
        let filter = s.sources.remove("filter").unwrap();
        unsafe { lightair_engine::sys::obs_source_filter_remove(s.sources["src"].as_ptr(), filter.as_ptr()) };
    });
    drop(engine);
}
