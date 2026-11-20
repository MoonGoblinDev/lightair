//! Golden frames: one scene drawn by libobs (the desktop's engine, Lightair's filters) and by
//! lightair-render's scene renderer (what Lightair Live's Studio draws with) must look the same:
//! fitted, cropped, flipped, turned, tilted, cut out, see-through, through an effect.
//! Needs the module (`cargo xtask modules`), ffmpeg and a GPU; skipped otherwise.

use std::path::PathBuf;
use std::time::Duration;

use lightair_engine::{Engine, EngineConfig};
use lightair_model::{
    Command, Crop, Document, Effect, Fit, Layout, Object, Scene, Shape, ShapeKind, Source, Workspace, diff,
};
use lightair_preview::{Frame, Preview, frame_rgba};
use lightair_render::scene::{Pictures, SceneRenderer, Shot};
use serde_json::json;

const W: u32 = 320;
const H: u32 = 180;

fn apply(engine: &Engine, doc: &mut Document, cmd: Command) {
    let change = doc.apply(&cmd).unwrap();
    let patches = diff(&change.before, &change.after);
    let workspace = change.after.clone();
    engine.call(move |s| s.reconciler.apply(&workspace, &patches));
}

fn settled(preview: &Preview) -> Frame {
    let n = preview.frames();
    while preview.frames() < n + 4 {
        std::thread::sleep(Duration::from_millis(10));
    }
    preview.latest_frame().unwrap()
}

/// The image sources' pictures, read from their files (as a phone's media would be).
struct Files;

impl Pictures for Files {
    fn picture(&mut self, source: &Source) -> Option<Shot> {
        let path = source.settings["file"].as_str()?;
        let img = image::open(path).ok()?.to_rgba8();
        let size = img.dimensions();
        Some(Shot::rgba(img.into_raw(), size))
    }
}

/// How many sampled places differ, of how many. libobs's frame comes back through NV12 here
/// (colour at half resolution), so brightness is compared per pixel and colour per 2×2 block;
/// and a place matches when the other frame has it within a pixel (or block) of there: an edge
/// a pixel apart isn't a difference.
fn differ(a: &[u8], b: &[u8]) -> (usize, usize, Vec<(u32, u32, [u8; 3], [u8; 3])>) {
    let at = |img: &[u8], x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        [img[i] as f32, img[i + 1] as f32, img[i + 2] as f32]
    };
    let luma = |img: &[u8], x: u32, y: u32| {
        let [r, g, b] = at(img, x, y);
        0.299 * r + 0.587 * g + 0.114 * b
    };
    // a 2×2 block's colour (Cb, Cr)
    let chroma = |img: &[u8], bx: u32, by: u32| {
        let (mut cb, mut cr) = (0.0, 0.0);
        for (x, y) in [(bx * 2, by * 2), (bx * 2 + 1, by * 2), (bx * 2, by * 2 + 1), (bx * 2 + 1, by * 2 + 1)] {
            let [r, g, b] = at(img, x, y);
            let yv = 0.299 * r + 0.587 * g + 0.114 * b;
            cb += (b - yv) * 0.564 / 4.0;
            cr += (r - yv) * 0.713 / 4.0;
        }
        (cb, cr)
    };
    let one_way = |p: &[u8], q: &[u8], x: u32, y: u32| {
        let l = luma(p, x, y);
        let near_luma = (y - 1..=y + 1).any(|v| (x - 1..=x + 1).any(|u| (luma(q, u, v) - l).abs() <= 24.0));
        let (bx, by) = (x / 2, y / 2);
        let c = chroma(p, bx, by);
        let near_colour = (by - 1..=by + 1).any(|v| {
            (bx - 1..=bx + 1).any(|u| {
                let d = chroma(q, u, v);
                (d.0 - c.0).abs() <= 20.0 && (d.1 - c.1).abs() <= 20.0
            })
        });
        near_luma && near_colour
    };
    let (mut bad, mut n, mut worst) = (0, 0, vec![]);
    for y in (4..H - 4).step_by(3) {
        for x in (4..W - 4).step_by(3) {
            n += 1;
            if !(one_way(a, b, x, y) && one_way(b, a, x, y)) {
                bad += 1;
                if worst.len() < 8 {
                    let px = |img: &[u8]| at(img, x, y).map(|v| v as u8);
                    worst.push((x, y, px(a), px(b)));
                }
            }
        }
    }
    (bad, n, worst)
}

fn ffmpeg_image(path: &PathBuf, filter: &str) -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-y", "-f", "lavfi", "-i", filter, "-frames:v", "1"])
        .arg(path)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn a_scene_looks_the_same_through_libobs_and_lightair_render() {
    if lightair_engine::testing::ran_in_own_process() {
        return;
    }
    let _ = tracing_subscriber::fmt().with_env_filter("warn").with_test_writer().try_init();
    let module = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/modules/lightair-effect-stack");
    if !module.join("lightair-effect-stack.plugin").exists() {
        eprintln!("module not built; skipping");
        return;
    }
    let dir = std::env::temp_dir().join(format!("lightair-golden-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (halves, grid, square) = (dir.join("halves.png"), dir.join("grid.png"), dir.join("square.png"));
    if !(ffmpeg_image(&halves, "color=c=0xC03020:s=320x180,drawbox=x=160:y=0:w=160:h=180:color=0x2050C0:t=fill")
        && ffmpeg_image(&grid, "color=c=0x30A060:s=200x120,drawgrid=w=40:h=40:t=14:c=0xF0E040")
        && ffmpeg_image(&square, "color=c=0xE0E0E0:s=100x100,drawbox=x=0:y=0:w=50:h=50:color=0x8040C0:t=fill"))
    {
        eprintln!("ffmpeg not available; skipping");
        return;
    }
    let mut renderer = match SceneRenderer::new((W, H)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("no GPU for lightair-render ({e}); skipping");
            return;
        }
    };
    let engine = Engine::start(EngineConfig {
        canvas: (W, H),
        output: (W, H),
        extra_plugin_dirs: vec![module],
        ..Default::default()
    })
    .unwrap();
    // the workspace's canvas the engine's size (layouts are in its pixels)
    let mut ws = Workspace::new("Golden");
    ws.canvases[0].width = W;
    ws.canvases[0].height = H;
    let mut doc = Document::new(ws);
    let c = doc.workspace().primary_canvas().id;
    let source =
        |name: &str, file: &PathBuf| Source::new(name, "image_source", json!({"file": file.to_string_lossy()}));
    let (bg, tiles, sq) = (source("Halves", &halves), source("Grid", &grid), source("Square", &square));
    let mut sq_faded = source("Square faded", &square);
    sq_faded.effects.push(Effect::new("Colour", "color_filter_v2", json!({"opacity": 0.6, "brightness": 0.1})));
    let object = |s: &Source, layout: Layout| Object::new(s.name.clone(), s.id, [(c, layout)]);

    let mut scene = Scene::new("Main");
    let sid = scene.id;
    // the background, stretched over the canvas
    scene.objects.push(object(&bg, Layout { fit: Fit::Stretch, ..Layout::boxed(0.0, 0.0, 320.0, 180.0) }));
    // a grid fitted into a tall box, cropped and flipped
    scene.objects.push(object(
        &tiles,
        Layout {
            crop: Crop { left: 20, top: 0, right: 0, bottom: 10 },
            flip_h: true,
            ..Layout::boxed(10.0, 10.0, 90.0, 160.0)
        },
    ));
    // the square, turned, in a rounded box with a border
    let mut shape = Shape::new(ShapeKind::Rounded);
    shape.radius = 0.4;
    shape.border = 6.0;
    shape.border_color = 0xFF20D0F0;
    scene.objects.push(object(
        &sq,
        Layout { rotation: 20.0, shape: Some(shape), fit: Fit::Stretch, ..Layout::boxed(120.0, 30.0, 70.0, 70.0) },
    ));
    // the square again: tilted, a circle, filled into a wide box
    scene.objects.push(object(
        &sq,
        Layout {
            tilt_y: 35.0,
            tilt_x: -15.0,
            shape: Some(Shape::new(ShapeKind::Circle)),
            fit: Fit::Fill,
            ..Layout::boxed(205.0, 20.0, 100.0, 80.0)
        },
    ));
    // and see-through, brightened by libobs's colour filter
    scene.objects.push(object(
        &sq_faded,
        Layout { fit: Fit::Stretch, opacity: 0.8, ..Layout::boxed(200.0, 110.0, 110.0, 60.0) },
    ));
    for s in [bg, tiles, sq, sq_faded] {
        apply(&engine, &mut doc, Command::AddSource { source: s });
    }
    apply(&engine, &mut doc, Command::AddScene { scene, index: None });
    let preview = engine.call(|_| Preview::attach());
    std::thread::sleep(Duration::from_millis(500));
    let frame = settled(&preview);
    let (fw, fh, libobs) = frame_rgba(&frame, W as usize).unwrap();
    assert_eq!((fw, fh), (W as usize, H as usize));

    let ours = renderer.frame(doc.workspace(), sid, c, &mut Files, 0.0);
    let save = |name: &str, px: &[u8]| {
        let _ = image::save_buffer(dir.join(name), px, W, H, image::ColorType::Rgba8);
    };
    save("libobs.png", &libobs);
    save("lightair-render.png", &ours);
    let (bad, n, worst) = differ(&libobs, &ours);
    eprintln!("golden: {bad} of {n} samples differ (frames in {})", dir.display());
    for (x, y, p, q) in &worst {
        eprintln!("  ({x}, {y}): libobs {p:?}, lightair-render {q:?}");
    }
    // (edges a pixel apart and libobs's scale filter aside) the same picture
    assert!(bad * 100 <= n, "{bad} of {n} samples differ: {worst:?}");
    drop(preview);
    drop(engine);
    let _ = std::fs::remove_dir_all(dir);
}
