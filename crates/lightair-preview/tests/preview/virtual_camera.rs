//! Lightair's virtual camera on Windows: its output writes the program, as NV12 at the camera's
//! size, into the memory the camera (lightair_camera.dll, in Windows' Frame Server) reads. Here
//! the test plays the camera: it makes the memory, and Lightair isn't asked to add the camera to
//! Windows (that needs the DLL registered, as administrator).

use std::sync::atomic::Ordering;
use std::time::Duration;

use lightair_model::{Command, Document, Layout, Object, Scene, Source, Workspace, diff};
use lightair_engine::{Engine, EngineConfig};
use serde_json::json;
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile};
use windows::core::HSTRING;

#[path = "../../../lightair-camera-win/src/protocol.rs"]
#[allow(dead_code)]
mod protocol;
use protocol as p;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

#[test]
fn the_program_reaches_the_cameras_memory() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    // (this process only: Lightair's output, not added to Windows)
    let name = format!("LightairVirtualCameraTest{}", std::process::id());
    unsafe { std::env::set_var("LIGHTAIR_VIRTUAL_CAMERA_TEST", &name) };

    // the camera's part: its memory (Local, and this test's own: a test can't make Global
    // memory, and the camera's own test may be running)
    let mapping = unsafe {
        CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, p::SIZE as u32, &HSTRING::from(format!(r"Local\{name}"))).unwrap()
    };
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, p::SIZE) };
    let header = unsafe { &mut *(view.Value as *mut p::Header) };
    (header.magic, header.version, header.width, header.height) = (p::MAGIC, p::VERSION, p::WIDTH, p::HEIGHT);

    let engine = Engine::start(EngineConfig { canvas: (640, 360), output: (640, 360), ..Default::default() }).unwrap();
    let mut doc = Document::new(Workspace::new("Virtual camera"));
    let canvas = doc.workspace().primary_canvas().id;
    let red = Source::new("Red", "color_source_v3", json!({"color": 0xFF0000FFu32, "width": 640, "height": 360}));
    let mut scene = Scene::new("Main");
    scene.objects.push(Object::new("Red", red.id, [(canvas, Layout::boxed(0.0, 0.0, 640.0, 360.0))]));
    let scene_id = scene.id;
    for cmd in [Command::AddSource { source: red }, Command::AddScene { scene, index: None }, Command::SetProgram { scene: Some(scene_id) }] {
        apply(&engine, &mut doc, cmd);
    }
    let started = engine.call(|s| s.outputs.start_virtual_camera(None, None));
    assert!(started.is_ok(), "{started:?}");
    let now = || unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
    // no app reading the camera: nothing copied
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(header.seq.load(Ordering::Acquire), 0, "written while nobody reads");
    // an app reading (the camera notes each read): frames, 30 a second at most
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(1500) {
        header.read_ms.store(now(), Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(33));
    }
    let seq = header.seq.load(Ordering::Acquire);
    eprintln!("frames written in 1.5 s: {}", seq / 2);
    assert!((2 * 30..=2 * 48).contains(&seq), "frames written: {}", seq / 2);
    let frame = unsafe { std::slice::from_raw_parts((view.Value as *const u8).add(p::HEADER_SIZE), p::FRAME_SIZE) };
    let (w, h) = (p::WIDTH as usize, p::HEIGHT as usize);
    // red, in BT.601 limited range: Y about 81, Cb about 90, Cr 240
    let y = frame[(h / 2) * w + w / 2];
    let uv = &frame[w * h + (h / 4) * w + w / 2..][..2];
    eprintln!("centre: Y {y}, Cb {}, Cr {}", uv[0], uv[1]);
    assert!((75..88).contains(&y), "Y {y}");
    assert!(uv[1] > 230 && (80..100).contains(&uv[0]), "CbCr {uv:?}");

    engine.call(|s| s.outputs.stop_virtual_camera());
    drop(engine);
    unsafe {
        let _ = UnmapViewOfFile(view);
        let _ = CloseHandle(mapping);
    }
}
