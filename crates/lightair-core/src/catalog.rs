//! The effect catalog (docs/plans/effects-revamp.md): every effect a source can have, with its
//! category, description and controls, and how the stack becomes libobs filters.
//!
//! Three families share one model:
//! - **Face and background effects** (`studio.*`) are separate effects in the stack, but the
//!   engine runs one `lightair_camera_studio` filter per source with all of them merged
//!   ([`lower`]), so they share one face tracker and one segmenter.
//! - **Shader effects** (`fx.*`) run on the `lightair_fx` filter, one shader each; their settings
//!   are the shader's uniforms.
//! - **libobs filters** keep their own ids and settings.

use serde_json::{Map, Value, json};

use crate::model::{Effect, Id};

/// The Lightair Effect Stack filter all face and background effects run on.
pub const STUDIO_FILTER: &str = "lightair_camera_studio";
/// The data-driven shader filter `fx.*` effects run on.
pub const FX_FILTER: &str = "lightair_fx";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Face,
    Background,
    Colour,
    Stylize,
    Blur,
    Distort,
    Keying,
    /// Screen recording: the drawn pointer, and later zooms.
    Screen,
    // audio
    Voice,
    Dynamics,
    Tone,
    Space,
    Utility,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Face,
        Category::Background,
        Category::Colour,
        Category::Stylize,
        Category::Blur,
        Category::Distort,
        Category::Keying,
        Category::Screen,
    ];

    /// The audio effects' categories, in catalog order.
    pub const AUDIO: [Category; 5] =
        [Category::Voice, Category::Dynamics, Category::Tone, Category::Space, Category::Utility];

    pub fn name(self) -> &'static str {
        match self {
            Category::Voice => "Voice",
            Category::Dynamics => "Dynamics",
            Category::Tone => "EQ & tone",
            Category::Space => "Space",
            Category::Utility => "Utility",
            Category::Face => "Face",
            Category::Background => "Background",
            Category::Colour => "Colour",
            Category::Stylize => "Stylize",
            Category::Blur => "Blur & sharpen",
            Category::Distort => "Distort & motion",
            Category::Keying => "Keying & utility",
            Category::Screen => "Screen",
        }
    }
}

/// What an effect needs in the picture to do anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    Face,
    Person,
}

/// How a slider's value reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Of the larger end of the range: "50%", bipolar "+20%".
    Percent,
    /// The number itself.
    Number,
    Px,
    Ms,
    /// Stored in radians, shown in degrees.
    Radians,
    /// Stored and shown in degrees.
    Degrees,
    Db,
    Hz,
    /// "4:1"
    Ratio,
}

/// A fixed choice's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pick {
    Int(i64),
    Str(&'static str),
}

impl Pick {
    pub fn json(self) -> Value {
        match self {
            Pick::Int(i) => json!(i),
            Pick::Str(s) => json!(s),
        }
    }

    pub fn matches(self, v: &Value) -> bool {
        match self {
            Pick::Int(i) => v.as_f64() == Some(i as f64),
            Pick::Str(s) => v.as_str() == Some(s),
        }
    }
}

/// Preset collections shown as tile grids (their names come from the module's data files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presets {
    Makeup,
    Stickers,
    Light,
    Grade,
    Ambience,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub default: f64,
    pub unit: Unit,
    /// Stored as an integer (Lightair Effect Stack's settings are whole numbers).
    pub int: bool,
    /// Moves on a logarithmic scale (frequencies).
    pub log: bool,
}

impl Slider {
    /// Centred on "unchanged" (0) with room both ways.
    pub fn bipolar(&self) -> bool {
        self.min < 0.0 && self.max > 0.0
    }

    pub fn value(&self, settings: &Value) -> f64 {
        settings.get(self.key).and_then(Value::as_f64).unwrap_or(self.default)
    }

    pub fn json(&self, v: f64) -> Value {
        let v = v.clamp(self.min, self.max);
        if self.int { json!(v.round() as i64) } else { json!(((v / self.step).round() * self.step * 1e6).round() / 1e6) }
    }

    pub fn display(&self, v: f64) -> String {
        let sign = if self.bipolar() && v > 0.0 { "+" } else { "" };
        match self.unit {
            Unit::Percent => {
                // a 0..1 (or 0..0.95) control is a plain fraction; others, of their larger end
                let of = if self.max <= 1.0 { 1.0 } else { self.max.abs().max(self.min.abs()) };
                format!("{sign}{:.0}%", v / of * 100.0)
            }
            Unit::Number if self.step >= 1.0 || self.int => format!("{sign}{v:.0}"),
            Unit::Number => format!("{sign}{v:.2}"),
            Unit::Px => format!("{v:.0} px"),
            Unit::Ms => format!("{v:.0} ms"),
            Unit::Radians => format!("{sign}{:.0}°", v.to_degrees()),
            Unit::Degrees => format!("{sign}{v:.0}°"),
            Unit::Db if v <= -144.0 => "−∞ dB".into(),
            Unit::Db => format!("{sign}{v:.1} dB"),
            Unit::Hz if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
            Unit::Hz => format!("{v:.0} Hz"),
            Unit::Ratio => format!("{v:.1}:1"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    /// Starts a group of controls.
    Heading(&'static str),
    Slider(Slider),
    Choice { key: &'static str, label: &'static str, options: &'static [(&'static str, Pick)], default: Pick },
    /// An OBS colour (0xAABBGGRR).
    Colour { key: &'static str, label: &'static str, default: u32 },
    Presets { key: &'static str, label: &'static str, set: Presets, default: &'static str },
    File { key: &'static str, label: &'static str, extensions: &'static [&'static str] },
    Switch { key: &'static str, label: &'static str, default: bool },
    /// Another mixer track (a ducker's source), stored as its id.
    Track { key: &'static str, label: &'static str },
}

impl Control {
    pub fn key(&self) -> Option<&'static str> {
        match self {
            Control::Heading(_) => None,
            Control::Slider(s) => Some(s.key),
            Control::Choice { key, .. }
            | Control::Colour { key, .. }
            | Control::Presets { key, .. }
            | Control::File { key, .. }
            | Control::Switch { key, .. }
            | Control::Track { key, .. } => Some(key),
        }
    }

    pub fn default_json(&self) -> Option<Value> {
        Some(match self {
            Control::Heading(_) => return None,
            Control::Slider(s) => s.json(s.default),
            Control::Choice { default, .. } => default.json(),
            Control::Colour { default, .. } => json!(default),
            Control::Presets { default, .. } => json!(default),
            Control::File { .. } | Control::Track { .. } => json!(""),
            Control::Switch { default, .. } => json!(default),
        })
    }

    /// The value that leaves the picture alone (for face effects that aren't in the stack).
    fn neutral_json(&self) -> Option<Value> {
        Some(match self {
            Control::Slider(s) => s.json(0.0f64.clamp(s.min, s.max)),
            Control::Presets { set: Presets::Makeup, .. } => json!("None"),
            Control::Presets { .. } => json!(""),
            _ => return self.default_json(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectDef {
    pub kind: &'static str,
    pub name: &'static str,
    pub category: Category,
    /// One line for the catalog.
    pub description: &'static str,
    /// A lucide icon name.
    pub icon: &'static str,
    pub needs: Needs,
    pub controls: &'static [Control],
    /// Settings the controls don't cover.
    pub fixed: &'static [(&'static str, Pick)],
}

impl EffectDef {
    pub fn is_studio(&self) -> bool {
        self.kind.starts_with("studio.")
    }

    /// The settings a new effect starts with.
    pub fn defaults(&self) -> Map<String, Value> {
        let mut m = Map::new();
        for (k, v) in self.fixed {
            m.insert(k.to_string(), v.json());
        }
        for c in self.controls {
            if let (Some(k), Some(v)) = (c.key(), c.default_json()) {
                m.insert(k.to_string(), v);
            }
        }
        m
    }

    /// A new effect of this kind.
    pub fn create(&self) -> Effect {
        Effect::new(self.name, self.kind, Value::Object(self.defaults()))
    }

    pub fn sliders(&self) -> impl Iterator<Item = &'static Slider> {
        self.controls.iter().filter_map(|c| if let Control::Slider(s) = c { Some(s) } else { None })
    }

    /// A one-line summary of what the effect is doing: its preset, then the sliders away
    /// from neutral ("Soft Box · Intensity 60%").
    pub fn summary(&self, settings: &Value) -> String {
        if self.kind == "audio.eq" {
            return eq_summary(self, settings);
        }
        if self.kind == PLUGIN_KIND {
            let vendor = settings.get("vendor").and_then(Value::as_str).unwrap_or("");
            let format = plugin_format_name(settings.get("format").and_then(Value::as_str).unwrap_or(""));
            return if vendor.is_empty() { format.into() } else { format!("{vendor} · {format}") };
        }
        let mut parts = Vec::new();
        for c in self.controls {
            match c {
                Control::Presets { key, default, .. } => {
                    let name = settings.get(*key).and_then(Value::as_str).unwrap_or(default);
                    if !name.is_empty() && name != "None" {
                        parts.push(name.to_string());
                    }
                }
                Control::Choice { key, options, default, .. } if parts.is_empty() => {
                    let v = settings.get(*key);
                    let pick = options.iter().find(|(_, p)| v.is_some_and(|v| p.matches(v)));
                    let (label, _) = pick.or_else(|| options.iter().find(|(_, p)| p == default)).copied().unwrap_or(("", *default));
                    parts.push(label.to_string());
                }
                Control::Slider(s) => {
                    let v = s.value(settings);
                    if v.abs() > 1e-6 && parts.len() < 3 {
                        parts.push(format!("{} {}", s.label, s.display(v)));
                    }
                }
                _ => {}
            }
        }
        parts.join(" · ")
    }
}

/// The EQ's bands that are on: "High pass 80 Hz · Bell 3.0 kHz +3.0 dB".
fn eq_summary(def: &EffectDef, settings: &Value) -> String {
    let get = |key: String| {
        settings.get(&key).cloned().or_else(|| def.controls.iter().find(|c| c.key() == Some(&key)).and_then(|c| c.default_json()))
    };
    let slider = |key: &str| def.sliders().find(|s| s.key == key).copied();
    let mut parts = Vec::new();
    for n in 1..=8 {
        if get(format!("b{n}_on")).and_then(|v| v.as_bool()) != Some(true) {
            continue;
        }
        let kind = get(format!("b{n}_type")).and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let label = EQ_TYPES.iter().find(|(_, p)| p.matches(&json!(kind))).map_or("Band", |(l, _)| *l);
        let freq_key = format!("b{n}_freq");
        let freq = slider(&freq_key).map(|s| s.display(get(freq_key.clone()).and_then(|v| v.as_f64()).unwrap_or(s.default)));
        let gain = get(format!("b{n}_gain")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mut part = format!("{label} {}", freq.unwrap_or_default());
        if matches!(kind.as_str(), "bell" | "ls" | "hs") && gain.abs() > 0.05 {
            part.push_str(&format!(" {gain:+.1} dB"));
        }
        parts.push(part);
    }
    if parts.is_empty() { "Flat".into() } else { parts.join(" · ") }
}

const fn s(key: &'static str, label: &'static str, default: f64) -> Control {
    // Lightair Effect Stack: 0..100
    Control::Slider(Slider { key, label, min: 0.0, max: 100.0, step: 1.0, default, unit: Unit::Percent, int: true, log: false })
}

const fn sb(key: &'static str, label: &'static str) -> Control {
    // Lightair Effect Stack, bipolar: -100..100
    Control::Slider(Slider { key, label, min: -100.0, max: 100.0, step: 1.0, default: 0.0, unit: Unit::Percent, int: true, log: false })
}

const fn f(key: &'static str, label: &'static str, min: f64, max: f64, default: f64, unit: Unit) -> Control {
    // Shader and libobs values: floats
    Control::Slider(Slider { key, label, min, max, step: (max - min) / 100.0, default, unit, int: false, log: false })
}

const fn amount(default: f64) -> Control {
    f("amount", "Amount", 0.0, 1.0, default, Unit::Percent)
}

const fn colour(key: &'static str, label: &'static str, default: u32) -> Control {
    Control::Colour { key, label, default }
}

const FEATHER: Control = s("bg_feather", "Edge softness", 30.0);
const SHIFT: Control = sb("bg_shift", "Edge shift");
/// Which model finds the person, and how hard it looks (Apple Vision's levels).
const SEGMENTATION: Control = Control::Heading("Segmentation");
/// With macOS's own camera effect too: Portrait blurs, Backgrounds replaces, and for Background
/// removal Backgrounds paints green (Lightair sets its picture) and Lightair Effect Stack keys it out.
pub const BG_MODEL_SYSTEM: i64 = 2;
const MODEL_OR_SYSTEM: Control = Control::Choice {
    key: "bg_model",
    label: "Model",
    options: &[("Vision", Pick::Int(0)), ("MediaPipe", Pick::Int(1)), ("macOS", Pick::Int(BG_MODEL_SYSTEM))],
    default: Pick::Int(0),
};
/// Background removal on macOS's own effect: the colour macOS paints behind the person, to key
/// out (blue when the person wears green).
const KEY_COLOUR: Control = Control::Choice {
    key: "bg_key",
    label: "Key colour",
    options: &[("Green", Pick::Int(0)), ("Blue", Pick::Int(1))],
    default: Pick::Int(0),
};
const QUALITY: Control = Control::Choice {
    key: "bg_quality",
    label: "Quality",
    options: &[("Fast", Pick::Int(0)), ("Balanced", Pick::Int(1)), ("Accurate", Pick::Int(2))],
    default: Pick::Int(1),
};

/// The drawn pointer (docs/plans/screen-studio.md, "Cursor"): the engine's own filter.
pub const POINTER_KIND: &str = "lightair_pointer";
/// Its kind before the drawn pointer was called a pointer (still read).
pub const OLD_POINTER_KIND: &str = "lightair_cursor";

/// Every effect, in catalog order.
pub const CATALOG: &[EffectDef] = &[
    // ---------------------------------------------------------------- Screen
    EffectDef {
        kind: POINTER_KIND,
        name: "Drawn pointer",
        category: Category::Screen,
        description: "Lightair draws the pointer: bigger, smooth, with clicks",
        icon: "mouse-pointer-2",
        needs: Nothing,
        controls: &[
            Control::Choice {
                key: "style",
                label: "Look",
                options: &[("Pointer", Pick::Int(0)), ("Dot", Pick::Int(1)), ("Ring", Pick::Int(2))],
                default: Pick::Int(0),
            },
            f("size", "Size", 0.5, 4.0, 1.5, Unit::Number),
            f("smoothing", "Smoothing", 0.0, 1.0, 0.5, Unit::Percent),
            Control::Switch { key: "tilt", label: "Tilt while moving", default: true },
            f("idle_s", "Hide when still (s)", 0.0, 10.0, 3.0, Unit::Number),
            Control::Choice {
                key: "clicks",
                label: "Clicks",
                options: &[("None", Pick::Int(0)), ("Circle", Pick::Int(1)), ("Ripple", Pick::Int(2))],
                default: Pick::Int(2),
            },
            Control::Switch { key: "arrow_only", label: "Always the arrow", default: false },
            colour("color", "Colour", 0xFFFF_9000),
        ],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Face
    EffectDef {
        kind: "studio.beauty",
        name: "Beauty",
        category: Category::Face,
        description: "Skin, face shape and makeup in one",
        icon: "sparkles",
        needs: Needs::Face,
        controls: &[
            Control::Heading("Skin"),
            s("smooth", "Smooth", 50.0),
            s("foundation", "Foundation", 20.0),
            sb("skin_tone", "Skin tone"),
            s("redness", "Redness", 0.0),
            s("shadows", "Dark circles", 0.0),
            s("teeth", "Brighten teeth", 0.0),
            s("eyelight", "Bright eyes", 30.0),
            s("contrast", "Clarity", 0.0),
            Control::Switch { key: "all_skin", label: "Whole body skin", default: true },
            Control::Heading("Face"),
            s("face_natural", "Natural", 0.0),
            s("face_slim", "Slim", 0.0),
            s("face_v", "V-shape", 0.0),
            s("face_narrow", "Narrow", 0.0),
            s("face_small", "Small", 0.0),
            s("cheekbone", "Cheekbones", 0.0),
            s("jawbone", "Jawbone", 0.0),
            sb("chin", "Chin"),
            sb("forehead", "Forehead"),
            s("head", "Head size", 0.0),
            Control::Heading("Eyes"),
            s("eye_size", "Enlarge", 0.0),
            sb("eye_spacing", "Distance"),
            sb("eye_position", "Position"),
            sb("eye_tilt", "Tilt"),
            s("eye_inner", "Inner corner", 0.0),
            sb("eye_outer", "Outer corner"),
            s("eye_lower_lid", "Lower lid", 0.0),
            s("eye_pupil", "Iris size", 0.0),
            Control::Heading("Nose"),
            s("nose_narrow", "Narrow", 0.0),
            sb("nose_length", "Lift"),
            s("nose_bridge", "Bridge", 0.0),
            s("nose_tip", "Tip", 0.0),
            Control::Heading("Mouth"),
            sb("mouth_size", "Size"),
            sb("mouth_position", "Position"),
            s("mouth_smile", "Smile", 0.0),
            Control::Heading("Brows"),
            sb("brow_thickness", "Thickness"),
            sb("brow_position", "Position"),
            Control::Heading("Makeup"),
            Control::Presets { key: "makeup_preset", label: "Look", set: Presets::Makeup, default: "None" },
            s("makeup_amt", "Intensity", 70.0),
            colour("mk_lip_color", "Lip colour", 0xFF8080D9),
            s("mk_lip_opacity", "Lips", 0.0),
            s("mk_lip_gloss", "Gloss", 0.0),
            colour("mk_blush_color", "Blush colour", 0xFFA5A5F5),
            s("mk_blush_opacity", "Blush", 0.0),
            Control::Choice {
                key: "mk_blush_style",
                label: "Placement",
                options: &[("Apples", Pick::Int(0)), ("Lifted", Pick::Int(1)), ("Sun-kissed", Pick::Int(2))],
                default: Pick::Int(0),
            },
            colour("mk_shadow_color", "Shadow colour", 0xFF8090C8),
            s("mk_shadow_opacity", "Eyeshadow", 0.0),
            Control::Choice {
                key: "mk_liner_style",
                label: "Eyeliner",
                options: &[
                    ("None", Pick::Int(0)),
                    ("Classic", Pick::Int(1)),
                    ("Winged", Pick::Int(2)),
                    ("Cat eye", Pick::Int(3)),
                    ("Smoky", Pick::Int(4)),
                ],
                default: Pick::Int(0),
            },
            s("mk_contour", "Contour", 0.0),
            s("mk_highlight", "Highlight", 0.0),
            s("mk_brow", "Brow tint", 0.0),
            s("filter_amt", "Look colour", 70.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "studio.stickers",
        name: "Face stickers",
        category: Category::Face,
        description: "Glasses, ears, crowns and more that follow your face",
        icon: "sticker",
        needs: Needs::Face,
        controls: &[Control::Presets {
            key: "effect_preset",
            label: "Sticker",
            set: Presets::Stickers,
            default: "Heart Eyes",
        }],
        fixed: &[],
    },
    EffectDef {
        kind: "studio.light",
        name: "Relight",
        category: Category::Face,
        description: "Studio lighting on your face and body",
        icon: "lamp",
        needs: Needs::Face,
        controls: &[
            Control::Presets { key: "light_preset", label: "Light", set: Presets::Light, default: "Soft Box" },
            s("light_amt", "Intensity", 60.0),
        ],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Background
    EffectDef {
        kind: "studio.bg_blur",
        name: "Background blur",
        category: Category::Background,
        description: "Blur everything behind you",
        icon: "aperture",
        needs: Needs::Person,
        controls: &[s("bg_blur", "Amount", 60.0), FEATHER, SHIFT, SEGMENTATION, MODEL_OR_SYSTEM, QUALITY],
        fixed: &[("bg_mode", Pick::Int(1))],
    },
    EffectDef {
        kind: "studio.bg_remove",
        name: "Background removal",
        category: Category::Background,
        description: "Remove the background; the objects below show through",
        icon: "eraser",
        needs: Needs::Person,
        controls: &[FEATHER, SHIFT, SEGMENTATION, MODEL_OR_SYSTEM, KEY_COLOUR, QUALITY],
        fixed: &[("bg_mode", Pick::Int(2))],
    },
    EffectDef {
        kind: "studio.bg_replace",
        name: "Background replace",
        category: Category::Background,
        description: "Put a colour or a picture behind you",
        icon: "image",
        needs: Needs::Person,
        controls: &[
            Control::Choice {
                key: "bg_mode",
                label: "Show",
                options: &[("Colour", Pick::Int(3)), ("Image", Pick::Int(4))],
                default: Pick::Int(3),
            },
            colour("bg_color", "Colour", 0xFF1A1A1A),
            Control::File {
                key: "bg_image",
                label: "Image",
                extensions: &["png", "jpg", "jpeg", "gif", "bmp", "webp"],
            },
            FEATHER,
            SHIFT,
            s("bg_wrap", "Light wrap", 30.0),
            SEGMENTATION,
            MODEL_OR_SYSTEM,
            QUALITY,
        ],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Colour
    EffectDef {
        kind: "studio.grade",
        name: "Colour grade",
        category: Category::Colour,
        description: "Film stocks and moods, tuned for skin",
        icon: "film",
        needs: Needs::Nothing,
        controls: &[
            Control::Presets { key: "filter_preset", label: "Grade", set: Presets::Grade, default: "Portra" },
            s("filter_preset_amt", "Intensity", 80.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "color_filter_v2",
        name: "Colour correction",
        category: Category::Colour,
        description: "Brightness, contrast, saturation, gamma and hue",
        icon: "sun",
        needs: Needs::Nothing,
        controls: &[
            f("brightness", "Brightness", -1.0, 1.0, 0.0, Unit::Percent),
            f("contrast", "Contrast", -4.0, 4.0, 0.0, Unit::Number),
            f("saturation", "Saturation", -1.0, 5.0, 0.0, Unit::Number),
            f("gamma", "Gamma", -3.0, 3.0, 0.0, Unit::Number),
            Control::Slider(Slider {
                key: "hue_shift",
                label: "Hue",
                min: -180.0,
                max: 180.0,
                step: 1.0,
                default: 0.0,
                unit: Unit::Degrees,
                int: false,
                log: false,
            }),
            f("opacity", "Opacity", 0.0, 1.0, 1.0, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "clut_filter",
        name: "LUT",
        category: Category::Colour,
        description: "Apply a .cube or .png look-up table",
        icon: "swatch-book",
        needs: Nothing,
        controls: &[
            Control::File { key: "image_path", label: "LUT file", extensions: &["cube", "png"] },
            f("clut_amount", "Amount", 0.0, 1.0, 1.0, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.mono",
        name: "Black & white",
        category: Category::Colour,
        description: "Monochrome, optionally tinted",
        icon: "contrast",
        needs: Nothing,
        controls: &[amount(1.0), colour("tint_color", "Tint", 0xFFA8DBFF), f("tint", "Tint amount", 0.0, 1.0, 0.0, Unit::Percent)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.duotone",
        name: "Duotone",
        category: Category::Colour,
        description: "Two colours from shadows to highlights",
        icon: "blend",
        needs: Nothing,
        controls: &[colour("shadow_color", "Shadows", 0xFF520D1C), colour("highlight_color", "Highlights", 0xFF6B9EFF), amount(1.0)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.posterize",
        name: "Posterize",
        category: Category::Colour,
        description: "Fewer colour levels, flat and graphic",
        icon: "layers",
        needs: Nothing,
        controls: &[Control::Slider(Slider {
            key: "levels",
            label: "Levels",
            min: 2.0,
            max: 16.0,
            step: 1.0,
            default: 5.0,
            unit: Unit::Number,
            int: false,
            log: false,
        })],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.invert",
        name: "Invert",
        category: Category::Colour,
        description: "A negative of the picture",
        icon: "eclipse",
        needs: Nothing,
        controls: &[amount(1.0)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.vignette",
        name: "Vignette",
        category: Category::Colour,
        description: "Darker edges draw the eye in",
        icon: "circle-dot",
        needs: Nothing,
        controls: &[
            amount(0.5),
            f("vignette_size", "Size", 0.0, 1.0, 0.55, Unit::Percent),
            f("softness", "Softness", 0.0, 1.0, 0.5, Unit::Percent),
            f("roundness", "Roundness", 0.0, 1.0, 1.0, Unit::Percent),
        ],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Stylize
    EffectDef {
        kind: "fx.grain",
        name: "Film grain",
        category: Category::Stylize,
        description: "Moving grain, strongest in the midtones",
        icon: "sparkle",
        needs: Nothing,
        controls: &[
            amount(0.3),
            f("grain_size", "Size", 0.5, 4.0, 1.5, Unit::Number),
            Control::Switch { key: "colored", label: "Colour grain", default: false },
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.vhs",
        name: "VHS",
        category: Category::Stylize,
        description: "Worn tape: jitter, tracking noise and colour bleed",
        icon: "cassette-tape",
        needs: Nothing,
        controls: &[
            amount(0.6),
            f("tracking", "Tracking noise", 0.0, 1.0, 0.3, Unit::Percent),
            f("bleed", "Colour bleed", 0.0, 1.0, 0.5, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.scanlines",
        name: "CRT scanlines",
        category: Category::Stylize,
        description: "Scanlines on a curved old screen",
        icon: "monitor",
        needs: Nothing,
        controls: &[
            amount(0.5),
            f("line_size", "Line size", 1.0, 8.0, 3.0, Unit::Px),
            f("curvature", "Curvature", 0.0, 1.0, 0.2, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.halftone",
        name: "Halftone",
        category: Category::Stylize,
        description: "Printed dots, like a comic or newspaper",
        icon: "grip",
        needs: Nothing,
        controls: &[
            f("dot_size", "Dot size", 3.0, 32.0, 8.0, Unit::Px),
            f("angle", "Angle", 0.0, std::f64::consts::FRAC_PI_2, 0.4, Unit::Radians),
            Control::Switch { key: "colored", label: "Colour", default: false },
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.pixelate",
        name: "Pixelate",
        category: Category::Stylize,
        description: "Big square pixels",
        icon: "grid-3x3",
        needs: Nothing,
        controls: &[f("block_size", "Block size", 2.0, 64.0, 16.0, Unit::Px)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.edges",
        name: "Neon edges",
        category: Category::Stylize,
        description: "Glowing outlines",
        icon: "zap",
        needs: Nothing,
        controls: &[amount(1.0), colour("glow_color", "Glow", 0xFFE6FF1A), f("keep", "Keep image", 0.0, 1.0, 0.2, Unit::Percent)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.sketch",
        name: "Sketch",
        category: Category::Stylize,
        description: "Pencil lines on paper",
        icon: "pencil",
        needs: Nothing,
        controls: &[amount(1.0), f("weight", "Line weight", 0.5, 3.0, 1.0, Unit::Number)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.chroma",
        name: "Chromatic aberration",
        category: Category::Stylize,
        description: "Colour fringes towards the edges",
        icon: "rainbow",
        needs: Nothing,
        controls: &[amount(0.5), f("angle", "Angle", 0.0, std::f64::consts::TAU, 0.0, Unit::Radians)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.glow",
        name: "Glow",
        category: Category::Stylize,
        description: "Bright parts bloom softly",
        icon: "sun-medium",
        needs: Nothing,
        controls: &[
            amount(0.6),
            f("threshold", "Threshold", 0.0, 1.0, 0.6, Unit::Percent),
            f("glow_size", "Size", 0.25, 3.0, 1.0, Unit::Number),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.emboss",
        name: "Emboss",
        category: Category::Stylize,
        description: "A raised, stamped relief",
        icon: "stamp",
        needs: Nothing,
        controls: &[amount(1.0), f("angle", "Light angle", 0.0, std::f64::consts::TAU, 0.8, Unit::Radians)],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Blur & sharpen
    EffectDef {
        kind: "fx.blur",
        name: "Blur",
        category: Category::Blur,
        description: "Soft gaussian blur",
        icon: "droplet",
        needs: Nothing,
        controls: &[amount(0.5)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.tiltshift",
        name: "Tilt-shift",
        category: Category::Blur,
        description: "A sharp band; a miniature world",
        icon: "rows-3",
        needs: Nothing,
        controls: &[
            amount(0.7),
            f("focus", "Focus position", 0.0, 1.0, 0.5, Unit::Percent),
            f("focus_size", "Focus size", 0.0, 1.0, 0.2, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "sharpness_filter_v2",
        name: "Sharpen",
        category: Category::Blur,
        description: "Crisper detail",
        icon: "triangle",
        needs: Nothing,
        controls: &[f("sharpness", "Amount", 0.0, 1.0, 0.08, Unit::Percent)],
        fixed: &[],
    },
    // ---------------------------------------------------------------- Distort & motion
    EffectDef {
        kind: "studio.motion",
        name: "Camera motion",
        category: Category::Distort,
        description: "Handheld sway, beat pulses, glitches",
        icon: "vibrate",
        needs: Nothing,
        controls: &[
            Control::Presets { key: "motion_preset", label: "Motion", set: Presets::Motion, default: "Handheld" },
            s("motion_amt", "Intensity", 50.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "studio.ambience",
        name: "Ambience",
        category: Category::Distort,
        description: "Sparkles, snow, bokeh, light leaks and more",
        icon: "snowflake",
        needs: Nothing,
        controls: &[
            Control::Presets { key: "atmo_preset", label: "Particles", set: Presets::Ambience, default: "Sparkles" },
            s("atmo_amt", "Intensity", 70.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.mirror",
        name: "Mirror",
        category: Category::Distort,
        description: "One half reflected onto the other",
        icon: "flip-horizontal-2",
        needs: Nothing,
        controls: &[Control::Choice {
            key: "axis",
            label: "Reflect",
            options: &[
                ("Left → right", Pick::Int(0)),
                ("Right → left", Pick::Int(1)),
                ("Top → bottom", Pick::Int(2)),
                ("Bottom → top", Pick::Int(3)),
            ],
            default: Pick::Int(0),
        }],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.kaleido",
        name: "Kaleidoscope",
        category: Category::Distort,
        description: "Mirrored slices around the centre",
        icon: "flower",
        needs: Nothing,
        controls: &[
            Control::Slider(Slider {
                key: "segments",
                label: "Segments",
                min: 2.0,
                max: 16.0,
                step: 1.0,
                default: 6.0,
                unit: Unit::Number,
                int: false,
                log: false,
            }),
            f("rotation", "Rotation", 0.0, std::f64::consts::TAU, 0.0, Unit::Radians),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.bulge",
        name: "Bulge",
        category: Category::Distort,
        description: "Swell or pinch the middle",
        icon: "circle",
        needs: Nothing,
        controls: &[f("amount", "Amount", -1.0, 1.0, 0.5, Unit::Percent), f("radius", "Size", 0.1, 1.0, 0.5, Unit::Percent)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.swirl",
        name: "Swirl",
        category: Category::Distort,
        description: "A twist around the centre",
        icon: "tornado",
        needs: Nothing,
        controls: &[f("amount", "Amount", -1.0, 1.0, 0.5, Unit::Percent), f("radius", "Size", 0.1, 1.0, 0.5, Unit::Percent)],
        fixed: &[],
    },
    EffectDef {
        kind: "fx.wave",
        name: "Wave",
        category: Category::Distort,
        description: "A rippling, underwater picture",
        icon: "waves-horizontal",
        needs: Nothing,
        controls: &[
            amount(0.3),
            f("frequency", "Frequency", 0.0, 1.0, 0.5, Unit::Percent),
            f("speed", "Speed", 0.0, 1.0, 0.5, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "scroll_filter",
        name: "Scroll",
        category: Category::Distort,
        description: "Endless scrolling, like a ticker",
        icon: "move-horizontal",
        needs: Nothing,
        controls: &[
            Control::Slider(Slider {
                key: "speed_x",
                label: "Horizontal",
                min: -500.0,
                max: 500.0,
                step: 10.0,
                default: 50.0,
                unit: Unit::Number,
                int: false,
                log: false,
            }),
            Control::Slider(Slider {
                key: "speed_y",
                label: "Vertical",
                min: -500.0,
                max: 500.0,
                step: 10.0,
                default: 0.0,
                unit: Unit::Number,
                int: false,
                log: false,
            }),
        ],
        fixed: &[("loop", Pick::Int(1))],
    },
    // ---------------------------------------------------------------- Keying & utility
    EffectDef {
        kind: "chroma_key_filter_v2",
        name: "Green screen",
        category: Category::Keying,
        description: "Key out a green, blue, magenta or any colour screen",
        icon: "square-dashed",
        needs: Nothing,
        controls: &[
            Control::Choice {
                key: "key_color_type",
                label: "Screen",
                options: &[
                    ("Green", Pick::Str("green")),
                    ("Blue", Pick::Str("blue")),
                    ("Magenta", Pick::Str("magenta")),
                    ("Custom", Pick::Str("custom")),
                ],
                default: Pick::Str("green"),
            },
            colour("key_color", "Key colour", 0xFF00FF00),
            Control::Slider(Slider {
                key: "similarity",
                label: "Similarity",
                min: 1.0,
                max: 1000.0,
                step: 1.0,
                default: 400.0,
                unit: Unit::Number,
                int: true,
                log: false,
            }),
            Control::Slider(Slider {
                key: "smoothness",
                label: "Smoothness",
                min: 1.0,
                max: 1000.0,
                step: 1.0,
                default: 80.0,
                unit: Unit::Number,
                int: true,
                log: false,
            }),
            Control::Slider(Slider {
                key: "spill",
                label: "Spill",
                min: 1.0,
                max: 1000.0,
                step: 1.0,
                default: 100.0,
                unit: Unit::Number,
                int: true,
                log: false,
            }),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "color_key_filter_v2",
        name: "Colour key",
        category: Category::Keying,
        description: "Key out any one colour",
        icon: "pipette",
        needs: Nothing,
        controls: &[
            colour("key_color", "Colour", 0xFF00FF00),
            Control::Slider(Slider {
                key: "similarity",
                label: "Similarity",
                min: 1.0,
                max: 1000.0,
                step: 1.0,
                default: 80.0,
                unit: Unit::Number,
                int: true,
                log: false,
            }),
            Control::Slider(Slider {
                key: "smoothness",
                label: "Smoothness",
                min: 1.0,
                max: 1000.0,
                step: 1.0,
                default: 50.0,
                unit: Unit::Number,
                int: true,
                log: false,
            }),
        ],
        fixed: &[("key_color_type", Pick::Str("custom"))],
    },
    EffectDef {
        kind: "luma_key_filter_v2",
        name: "Luma key",
        category: Category::Keying,
        description: "Remove darks or lights",
        icon: "contrast",
        needs: Nothing,
        controls: &[
            f("luma_min", "Remove darks", 0.0, 1.0, 0.0, Unit::Percent),
            f("luma_max", "Keep up to", 0.0, 1.0, 1.0, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "gpu_delay",
        name: "Delay",
        category: Category::Keying,
        description: "Hold the picture back to line up with audio",
        icon: "timer",
        needs: Nothing,
        controls: &[Control::Slider(Slider {
            key: "delay_ms",
            label: "Delay",
            min: 0.0,
            max: 500.0,
            step: 10.0,
            default: 0.0,
            unit: Unit::Ms,
            int: true,
            log: false,
        })],
        fixed: &[],
    },
];

use Needs::Nothing;

pub fn def(kind: &str) -> Option<&'static EffectDef> {
    let kind = if kind == OLD_POINTER_KIND { POINTER_KIND } else { kind };
    CATALOG.iter().chain(AUDIO).chain(std::iter::once(&PLUGIN)).find(|d| d.kind == kind)
}

/// The kind of a hosted plugin insert.
pub const PLUGIN_KIND: &str = "audio.plugin";

/// A hosted plugin (Audio Unit, VST3). Not a catalog tile: the sheet lists the installed
/// plugins. Settings: "format" ("au", "vst3"), "id" (the format's identifier), "name", "vendor",
/// and "state" (the plugin's own state, base64) once it has been edited.
pub static PLUGIN: EffectDef = EffectDef {
    kind: PLUGIN_KIND,
    name: "Plugin",
    category: Category::Utility,
    description: "An Audio Unit or VST3 plugin",
    icon: "plug",
    needs: Nothing,
    controls: &[],
    fixed: &[],
};

/// A plugin insert for an installed plugin.
pub fn plugin_effect(format: &str, id: &str, name: &str, vendor: &str) -> Effect {
    Effect::new(
        name,
        PLUGIN_KIND,
        serde_json::json!({ "format": format, "id": id, "name": name, "vendor": vendor }),
    )
}

/// What a plugin format is called.
pub fn plugin_format_name(format: &str) -> &'static str {
    match format {
        "au" => "Audio Unit",
        "vst3" => "VST3",
        _ => "Plugin",
    }
}

pub fn audio_in_category(category: Category) -> impl Iterator<Item = &'static EffectDef> {
    AUDIO.iter().filter(move |d| d.category == category)
}

// ------------------------------------------------------------------ audio

const fn a(key: &'static str, label: &'static str, min: f64, max: f64, default: f64, unit: Unit) -> Control {
    Control::Slider(Slider { key, label, min, max, step: (max - min) / 200.0, default, unit, int: false, log: false })
}

const fn hz(key: &'static str, label: &'static str, min: f64, max: f64, default: f64) -> Control {
    Control::Slider(Slider { key, label, min, max, step: 1.0, default, unit: Unit::Hz, int: false, log: true })
}

const fn ms(key: &'static str, label: &'static str, min: f64, max: f64, default: f64) -> Control {
    Control::Slider(Slider { key, label, min, max, step: 0.1, default, unit: Unit::Ms, int: false, log: true })
}

const fn on(key: &'static str, label: &'static str, default: bool) -> Control {
    Control::Switch { key, label, default }
}

/// An EQ band's shapes, as stored in "b<n>_type".
pub const EQ_TYPES: &[(&str, Pick)] = &[
    ("High pass", Pick::Str("hp")),
    ("Low shelf", Pick::Str("ls")),
    ("Bell", Pick::Str("bell")),
    ("Notch", Pick::Str("notch")),
    ("High shelf", Pick::Str("hs")),
    ("Low pass", Pick::Str("lp")),
];

macro_rules! eq_band {
    ($n:literal, $kind:literal, $freq:literal) => {
        [
            Control::Heading(concat!("Band ", $n)),
            Control::Switch { key: concat!("b", $n, "_on"), label: "On", default: false },
            Control::Choice { key: concat!("b", $n, "_type"), label: "Shape", options: EQ_TYPES, default: Pick::Str($kind) },
            Control::Choice {
                key: concat!("b", $n, "_slope"),
                label: "Slope",
                options: &[("12", Pick::Int(12)), ("24", Pick::Int(24)), ("36", Pick::Int(36)), ("48", Pick::Int(48))],
                default: Pick::Int(24),
            },
            hz(concat!("b", $n, "_freq"), "Frequency", 20.0, 20_000.0, $freq),
            a(concat!("b", $n, "_gain"), "Gain", -24.0, 24.0, 0.0, Unit::Db),
            Control::Slider(Slider {
                key: concat!("b", $n, "_q"),
                label: "Q",
                min: 0.1,
                max: 10.0,
                step: 0.01,
                default: 0.9,
                unit: Unit::Number,
                int: false,
                log: true,
            }),
        ]
    };
}

const EQ_CONTROLS: [Control; 57] = {
    let bands = [
        eq_band!("1", "hp", 80.0),
        eq_band!("2", "ls", 150.0),
        eq_band!("3", "bell", 300.0),
        eq_band!("4", "bell", 1000.0),
        eq_band!("5", "bell", 3000.0),
        eq_band!("6", "bell", 6000.0),
        eq_band!("7", "hs", 10_000.0),
        eq_band!("8", "lp", 18_000.0),
    ];
    let mut out = [Control::Heading(""); 57];
    let mut i = 0;
    while i < 56 {
        out[i] = bands[i / 7][i % 7];
        i += 1;
    }
    out[56] = a("output_db", "Output", -24.0, 24.0, 0.0, Unit::Db);
    out
};

/// Every built-in audio effect (docs/plans/audio-engine.md), in catalog order.
pub const AUDIO: &[EffectDef] = &[
    EffectDef {
        kind: "audio.voice",
        name: "Voice Clarity",
        category: Category::Voice,
        description: "One knob for a clean, present voice",
        icon: "mic-vocal",
        needs: Nothing,
        controls: &[
            a("amount", "Amount", 0.0, 1.0, 0.5, Unit::Percent),
            Control::Choice {
                key: "character",
                label: "Character",
                options: &[("Clear", Pick::Int(0)), ("Podcast", Pick::Int(1))],
                default: Pick::Int(0),
            },
            Control::Heading("Stages"),
            on("noise", "Noise suppression", true),
            on("gate", "Expander", true),
            on("deess", "De-esser", true),
            on("eq", "EQ", true),
            on("compress", "Compressor", true),
            on("limit", "Limiter", true),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.noise",
        name: "Noise suppression",
        category: Category::Voice,
        description: "Removes fans, hum, keyboards and room noise (neural)",
        icon: "audio-waveform",
        needs: Nothing,
        controls: &[
            a("strength", "Strength", 0.0, 1.0, 1.0, Unit::Percent),
            a("vad_threshold", "Voice gate", 0.0, 0.95, 0.0, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.deesser",
        name: "De-esser",
        category: Category::Voice,
        description: "Tames harsh s and sh sounds",
        icon: "wind",
        needs: Nothing,
        controls: &[
            hz("freq", "Frequency", 2000.0, 12_000.0, 6000.0),
            a("threshold_db", "Threshold", -60.0, 0.0, -30.0, Unit::Db),
            a("range_db", "Range", 0.0, 24.0, 8.0, Unit::Db),
            on("split", "Only the highs", true),
            on("listen", "Listen to what it takes", false),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.compressor",
        name: "Compressor",
        category: Category::Dynamics,
        description: "Evens out loud and quiet",
        icon: "chevrons-down-up",
        needs: Nothing,
        controls: &[
            a("threshold_db", "Threshold", -60.0, 0.0, -18.0, Unit::Db),
            a("ratio", "Ratio", 1.0, 20.0, 3.0, Unit::Ratio),
            ms("attack_ms", "Attack", 0.1, 200.0, 10.0),
            ms("release_ms", "Release", 10.0, 2000.0, 120.0),
            a("makeup_db", "Makeup", -12.0, 24.0, 0.0, Unit::Db),
            on("auto_makeup", "Auto makeup", true),
            Control::Heading("More"),
            a("knee_db", "Knee", 0.0, 24.0, 6.0, Unit::Db),
            a("mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
            hz("sidechain_hpf", "Detector high-pass", 20.0, 400.0, 80.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.gate",
        name: "Noise gate",
        category: Category::Dynamics,
        description: "Silences the gaps between words",
        icon: "door-closed",
        needs: Nothing,
        controls: &[
            a("threshold_db", "Threshold", -80.0, 0.0, -45.0, Unit::Db),
            a("range_db", "Range", 0.0, 80.0, 40.0, Unit::Db),
            ms("attack_ms", "Attack", 0.1, 50.0, 1.0),
            ms("hold_ms", "Hold", 1.0, 500.0, 80.0),
            ms("release_ms", "Release", 5.0, 1000.0, 150.0),
            Control::Heading("More"),
            a("ratio", "Ratio", 1.0, 20.0, 10.0, Unit::Ratio),
            a("hysteresis_db", "Hysteresis", 0.0, 12.0, 4.0, Unit::Db),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.limiter",
        name: "Limiter",
        category: Category::Dynamics,
        description: "A ceiling nothing gets past",
        icon: "arrow-up-to-line",
        needs: Nothing,
        controls: &[
            a("ceiling_db", "Ceiling", -12.0, 0.0, -1.0, Unit::Db),
            a("input_db", "Input", 0.0, 24.0, 0.0, Unit::Db),
            ms("release_ms", "Release", 1.0, 500.0, 80.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.ducker",
        name: "Ducking",
        category: Category::Dynamics,
        description: "Lowers this when another track is active (music under the mic)",
        icon: "arrow-down-wide-narrow",
        needs: Nothing,
        controls: &[
            Control::Track { key: "source", label: "When this plays" },
            a("amount_db", "Amount", 0.0, 40.0, 12.0, Unit::Db),
            a("threshold_db", "Threshold", -60.0, 0.0, -40.0, Unit::Db),
            ms("attack_ms", "Attack", 1.0, 500.0, 30.0),
            ms("hold_ms", "Hold", 1.0, 2000.0, 300.0),
            ms("release_ms", "Release", 10.0, 3000.0, 600.0),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.eq",
        name: "EQ",
        category: Category::Tone,
        description: "Eight bands: cut rumble, warm up, add presence",
        icon: "sliders-vertical",
        needs: Nothing,
        controls: &EQ_CONTROLS,
        fixed: &[],
    },
    EffectDef {
        kind: "audio.reverb",
        name: "Reverb",
        category: Category::Space,
        description: "A room, a plate, a hall",
        icon: "waves-horizontal",
        needs: Nothing,
        controls: &[
            a("size", "Size", 0.0, 1.0, 0.5, Unit::Percent),
            a("damping", "Damping", 0.0, 1.0, 0.5, Unit::Percent),
            a("pre_delay_ms", "Pre-delay", 0.0, 200.0, 15.0, Unit::Ms),
            a("width", "Width", 0.0, 1.0, 1.0, Unit::Percent),
            a("mix", "Mix", 0.0, 1.0, 0.25, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.delay",
        name: "Delay",
        category: Category::Space,
        description: "Echoes, slapback, ping-pong",
        icon: "repeat",
        needs: Nothing,
        controls: &[
            ms("time_ms", "Time", 1.0, 2000.0, 350.0),
            a("feedback", "Feedback", 0.0, 0.95, 0.35, Unit::Percent),
            hz("tone_hz", "Tone", 500.0, 18_000.0, 6000.0),
            on("ping_pong", "Ping-pong", false),
            a("mix", "Mix", 0.0, 1.0, 0.25, Unit::Percent),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.gain",
        name: "Gain",
        category: Category::Utility,
        description: "Level, polarity, mono, swap left and right",
        icon: "volume-2",
        needs: Nothing,
        controls: &[
            a("db", "Gain", -30.0, 30.0, 0.0, Unit::Db),
            on("invert", "Invert polarity", false),
            on("mono", "Mono", false),
            on("swap", "Swap left and right", false),
        ],
        fixed: &[],
    },
    EffectDef {
        kind: "audio.channel",
        name: "Stereo tools",
        category: Category::Utility,
        description: "Width, and mono bass",
        icon: "move-horizontal",
        needs: Nothing,
        controls: &[
            a("width", "Width", 0.0, 2.0, 1.0, Unit::Number),
            a("bass_mono_hz", "Mono below (0: off)", 0.0, 300.0, 0.0, Unit::Hz),
        ],
        fixed: &[],
    },
];

pub fn in_category(category: Category) -> impl Iterator<Item = &'static EffectDef> {
    CATALOG.iter().filter(move |d| d.category == category)
}

/// Catalog entries whose name, description or category match a search.
pub fn search(query: &str) -> impl Iterator<Item = &'static EffectDef> + '_ {
    let q = query.trim().to_lowercase();
    CATALOG.iter().filter(move |d| {
        q.is_empty()
            || d.name.to_lowercase().contains(&q)
            || d.description.to_lowercase().contains(&q)
            || d.category.name().to_lowercase().contains(&q)
    })
}

// ------------------------------------------------------------------ lowering

/// The id of the merged Lightair Effect Stack filter in a source's chain: one per source and stable,
/// so face effects coming and going update it instead of rebuilding it (and its tracker).
pub const STUDIO_ID: Id = Id(uuid::Uuid::from_u128(0x6c69_6768_7461_6972_2d73_7475_6469_6f00));

/// The libobs filters for a source's effect stack:
/// - `studio.*` effects merge into one Lightair Effect Stack filter at the first one's place (each
///   effect turned off, or not in the stack, leaves its part of the picture alone);
/// - `fx.<name>` become `lightair_fx` with `fx` set;
/// - everything else (libobs filters, and a whole Lightair Effect Stack from older workspaces) passes
///   through.
pub fn lower(effects: &[Effect]) -> Vec<Effect> {
    let mut out = Vec::with_capacity(effects.len());
    let mut studio_at = None;
    for e in effects {
        if e.kind.starts_with("studio.") {
            if studio_at.is_none() {
                studio_at = Some(out.len());
                out.push(Effect {
                    id: STUDIO_ID,
                    name: "Effects".into(),
                    kind: STUDIO_FILTER.into(),
                    enabled: false,
                    settings: Value::Null,
                });
            }
        } else if let Some(name) = e.kind.strip_prefix("fx.") {
            let mut settings = match &e.settings {
                Value::Object(m) => m.clone(),
                _ => Map::new(),
            };
            settings.insert("fx".into(), json!(name));
            out.push(Effect { settings: Value::Object(settings), kind: FX_FILTER.into(), ..e.clone() });
        } else if e.kind == OLD_POINTER_KIND {
            out.push(Effect { kind: POINTER_KIND.into(), ..e.clone() });
        } else {
            out.push(e.clone());
        }
    }
    if let Some(i) = studio_at {
        out[i].enabled = effects.iter().any(|e| e.enabled && e.kind.starts_with("studio."));
        out[i].settings = Value::Object(studio_settings(effects));
    }
    out
}

/// Lightair Effect Stack's settings for a stack's face effects.
pub fn studio_settings(effects: &[Effect]) -> Map<String, Value> {
    let mut m = Map::new();
    for d in CATALOG.iter().filter(|d| d.is_studio()) {
        for (k, _) in d.fixed {
            m.insert(k.to_string(), json!(0));
        }
        for c in d.controls {
            if let (Some(k), Some(v)) = (c.key(), c.neutral_json()) {
                m.insert(k.to_string(), v);
            }
        }
    }
    m.insert("bg_mode".into(), json!(0)); // no background effect: the background as it is
    for k in ["enhance_on", "effects_on", "atmo_on", "filters_on", "light_on", "sync_tracking"] {
        m.insert(k.into(), json!(true));
    }
    let mut makeup_tweaked = false;
    for e in effects.iter().filter(|e| e.enabled) {
        let Some(d) = def(&e.kind).filter(|d| d.is_studio()) else { continue };
        m.extend(d.defaults());
        if let Value::Object(own) = &e.settings {
            for (k, v) in own {
                m.insert(k.clone(), v.clone());
                makeup_tweaked |= k.starts_with("mk_") || k.starts_with("fl_");
            }
        }
    }
    // The filter applies a makeup look's values when the look changes; values the effect sets
    // itself (picked from the look, then tweaked) win, so mark the look as applied.
    if makeup_tweaked && let Some(look) = m.get("makeup_preset").cloned() {
        m.insert("makeup_applied".into(), look);
    }
    m
}

/// Makeup values for a look from the module's `makeup/presets.json` entry (as the filter's
/// own apply would set them): the effect stores these so each can be tweaked.
pub fn makeup_values(preset: &Value) -> Map<String, Value> {
    let pct = |o: &Value, k: &str| json!((o.get(k).and_then(Value::as_f64).unwrap_or(0.0) * 100.0).round() as i64);
    let colour = |o: &Value, k: &str| {
        let hex = o.get(k).and_then(Value::as_str).unwrap_or("#FFFFFF").trim_start_matches('#');
        let rgb = u32::from_str_radix(hex, 16).unwrap_or(0xFFFFFF);
        let (r, g, b) = ((rgb >> 16) & 0xFF, (rgb >> 8) & 0xFF, rgb & 0xFF);
        json!(0xFF00_0000u32 | (b << 16) | (g << 8) | r)
    };
    let fl = &preset["filter"];
    let mk = &preset["makeup"];
    let mut m = Map::new();
    for k in ["exposure", "contrast", "saturation", "warmth", "tint", "fade"] {
        m.insert(format!("fl_{k}"), pct(fl, k));
    }
    m.insert("fl_lut".into(), json!(fl.get("lut").and_then(Value::as_str).unwrap_or("")));
    for k in ["lip_color", "blush_color", "shadow_color"] {
        m.insert(format!("mk_{k}"), colour(mk, k));
    }
    for k in ["lip_opacity", "lip_gloss", "blush_opacity", "shadow_opacity", "contour", "highlight", "brow"] {
        m.insert(format!("mk_{k}"), pct(mk, k));
    }
    let styles = ["none", "classic", "winged", "cat", "smoky"];
    let liner = mk.get("liner_style").and_then(Value::as_str).unwrap_or("none");
    m.insert("mk_liner_style".into(), json!(styles.iter().position(|s| *s == liner).unwrap_or(0)));
    m.insert("mk_blush_style".into(), json!(mk.get("blush_style").and_then(Value::as_i64).unwrap_or(0)));
    m
}

/// Splits an older workspace's single Lightair Effect Stack effect into the face effects it was doing.
/// Effects that would do nothing are left out; returns `None` when there is nothing to split.
pub fn split_camera_studio(e: &Effect) -> Option<Vec<Effect>> {
    if e.kind != STUDIO_FILTER {
        return None;
    }
    let s = e.settings.as_object().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for d in CATALOG.iter().filter(|d| d.is_studio()) {
        let mut own = Map::new();
        for c in d.controls {
            if let Some(k) = c.key()
                && let Some(v) = s.get(k)
            {
                own.insert(k.into(), v.clone());
            }
        }
        // Keep it when it would show something: a chosen preset, or a slider off neutral.
        let active = d.controls.iter().any(|c| match c {
            Control::Presets { key, .. } => own.get(*key).and_then(Value::as_str).is_some_and(|v| !v.is_empty() && v != "None"),
            Control::Slider(sl) if d.kind == "studio.beauty" && !sl.key.starts_with("mk_") && sl.key != "makeup_amt" && sl.key != "filter_amt" => {
                own.get(sl.key).and_then(Value::as_f64).is_some_and(|v| v != 0.0)
            }
            _ => false,
        }) || (d.kind == "studio.bg_blur" && s.get("bg_mode").and_then(Value::as_i64) == Some(1))
            || (d.kind == "studio.bg_remove" && s.get("bg_mode").and_then(Value::as_i64) == Some(2))
            || (d.kind == "studio.bg_replace" && matches!(s.get("bg_mode").and_then(Value::as_i64), Some(3 | 4)));
        if !active {
            continue;
        }
        if d.kind == "studio.beauty" {
            for (k, v) in &s {
                if k.starts_with("mk_") || k.starts_with("fl_") {
                    own.insert(k.clone(), v.clone());
                }
            }
        }
        // Fill what the old effect left to the filter's defaults, so nothing changes; makeup
        // values only when it had its own (otherwise the filter applies the chosen look).
        let mine_makeup = own.keys().any(|k| k.starts_with("mk_") || k.starts_with("fl_"));
        let mut settings = d.defaults();
        if !mine_makeup {
            settings.retain(|k, _| !k.starts_with("mk_") && !k.starts_with("fl_"));
        }
        settings.extend(own);
        let mut new = Effect::new(d.name, d.kind, Value::Object(settings));
        new.enabled = e.enabled;
        out.push(new);
    }
    Some(out)
}

/// An older track effect (a libobs audio filter) as the audio engine's equivalent, keeping its
/// id, name and switch; `None` when it already is one, or has no equivalent.
pub fn migrate_audio_effect(e: &Effect) -> Option<Effect> {
    let s = &e.settings;
    let n = |k: &str, d: f64| s.get(k).and_then(Value::as_f64).unwrap_or(d);
    let (kind, settings) = match e.kind.as_str() {
        "noise_suppress_filter_v2" | "noise_suppress_filter" => ("audio.noise", json!({"strength": 1.0})),
        "noise_gate_filter" => {
            let open = n("open_threshold", -26.0);
            (
                "audio.gate",
                json!({
                    "threshold_db": open,
                    "hysteresis_db": (open - n("close_threshold", -32.0)).max(0.0),
                    "attack_ms": n("attack_time", 25.0),
                    "hold_ms": n("hold_time", 200.0),
                    "release_ms": n("release_time", 150.0),
                }),
            )
        }
        "compressor_filter" => (
            "audio.compressor",
            json!({
                "threshold_db": n("threshold", -18.0),
                "ratio": n("ratio", 10.0),
                "attack_ms": n("attack_time", 6.0),
                "release_ms": n("release_time", 60.0),
                "makeup_db": n("output_gain", 0.0),
                "auto_makeup": false,
            }),
        ),
        "limiter_filter" => {
            ("audio.limiter", json!({"ceiling_db": n("threshold", -6.0), "release_ms": n("release_time", 60.0)}))
        }
        "gain_filter" => ("audio.gain", json!({"db": n("db", 0.0)})),
        _ => return None,
    };
    Some(Effect { kind: kind.into(), settings, ..e.clone() })
}

/// Commands that turn tracks' libobs audio filters into the audio engine's effects. Run once the
/// engine plays the mixer (until then the libobs filters are what process the sound).
pub fn migrate_audio(workspace: &crate::model::Workspace) -> Vec<crate::command::Command> {
    use crate::command::Command;
    let mut out = Vec::new();
    for t in &workspace.tracks {
        if t.effects.iter().any(|e| migrate_audio_effect(e).is_some()) {
            let mut track = (**t).clone();
            for e in &mut track.effects {
                if let Some(new) = migrate_audio_effect(e) {
                    *e = new;
                }
            }
            out.push(Command::UpdateTrack { track });
        }
    }
    out
}

/// Commands that bring an older workspace's effects up to date: each source's single Camera
/// Studio effect becomes the face effects it was doing, in its place.
pub fn migrate(workspace: &crate::model::Workspace) -> Vec<crate::command::Command> {
    use crate::command::Command;
    let mut out = Vec::new();
    let mut sources: Vec<_> = workspace.sources.values().collect();
    sources.sort_by_key(|i| i.id.0);
    for source in sources {
        for (index, e) in source.effects.iter().enumerate().rev() {
            let Some(parts) = split_camera_studio(e) else { continue };
            let mut commands = vec![Command::RemoveSourceEffect { source: source.id, effect: e.id }];
            for (n, effect) in parts.into_iter().enumerate() {
                commands.push(Command::AddSourceEffect { source: source.id, effect, index: Some(index + n) });
            }
            out.push(Command::Batch { label: "Update effects".into(), commands });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: &str, settings: Value) -> Effect {
        Effect::new(kind, kind, settings)
    }

    #[test]
    fn every_entry_is_well_formed() {
        let mut kinds = std::collections::HashSet::new();
        for d in CATALOG.iter().chain(AUDIO) {
            assert!(kinds.insert(d.kind), "{} listed twice", d.kind);
            assert!(!d.description.is_empty() && !d.controls.is_empty(), "{}", d.kind);
            let mut keys = std::collections::HashSet::new();
            for c in d.controls {
                if let Some(k) = c.key() {
                    assert!(keys.insert(k), "{}: {k} twice", d.kind);
                }
                if let Control::Slider(s) = c {
                    assert!(s.min < s.max && (s.min..=s.max).contains(&s.default), "{}: {}", d.kind, s.key);
                }
            }
        }
        for c in Category::ALL {
            assert!(in_category(c).count() > 0, "{} is empty", c.name());
        }
        for c in Category::AUDIO {
            assert!(audio_in_category(c).count() > 0, "{} is empty", c.name());
        }
    }

    #[test]
    fn face_effects_merge_into_one_camera_studio() {
        let skin = effect("studio.beauty", json!({"smooth": 80}));
        let cc = effect("color_filter_v2", json!({"saturation": 0.5}));
        let mut light = effect("studio.light", json!({"light_preset": "Rembrandt"}));
        let mono = effect("fx.mono", json!({"amount": 0.5}));
        let lowered = lower(&[skin.clone(), cc.clone(), light.clone(), mono.clone()]);
        let kinds: Vec<_> = lowered.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, [STUDIO_FILTER, "color_filter_v2", FX_FILTER]);
        let studio = &lowered[0].settings;
        assert_eq!(studio["smooth"], 80);
        assert_eq!(studio["foundation"], 20, "Beauty's own defaults");
        assert_eq!(studio["light_preset"], "Rembrandt");
        assert_eq!(studio["light_amt"], 60);
        assert_eq!(studio["face_slim"], 0);
        assert_eq!(studio["mk_lip_opacity"], 0, "no makeup until a look is chosen");
        assert_eq!(studio["makeup_preset"], "None");
        assert_eq!(studio["bg_mode"], 0);
        assert_eq!(lowered[2].settings["fx"], "mono");
        assert_eq!(lowered[2].id, mono.id);

        // Turning one off leaves its part alone; the filter stays (same id) while one is on.
        light.enabled = false;
        let lowered = lower(&[skin.clone(), light.clone()]);
        assert_eq!(lowered[0].id, STUDIO_ID);
        assert!(lowered[0].enabled);
        assert_eq!(lowered[0].settings["light_preset"], "");
        let mut skin_off = skin;
        skin_off.enabled = false;
        assert!(!lower(&[skin_off, light])[0].enabled, "all off: the filter is bypassed");
    }

    #[test]
    fn makeup_tweaks_survive_the_look() {
        let plain = studio_settings(&[effect("studio.beauty", json!({"makeup_preset": "Berry"}))]);
        assert!(plain.get("makeup_applied").is_none(), "the filter applies the look");
        let tweaked =
            studio_settings(&[effect("studio.beauty", json!({"makeup_preset": "Berry", "mk_lip_opacity": 90}))]);
        assert_eq!(tweaked["makeup_applied"], "Berry");
        assert_eq!(tweaked["mk_lip_opacity"], 90);
    }

    #[test]
    fn makeup_values_follow_the_filter() {
        let p = json!({"name": "X", "filter": {"warmth": 0.05}, "makeup": {"lip_color": "#D98080", "lip_opacity": 0.3, "liner_style": "winged", "blush_style": 1}});
        let m = makeup_values(&p);
        assert_eq!(m["fl_warmth"], 5);
        assert_eq!(m["mk_lip_color"], 0xFF8080D9u32, "0xAABBGGRR");
        assert_eq!(m["mk_lip_opacity"], 30);
        assert_eq!(m["mk_liner_style"], 2);
        assert_eq!(m["mk_blush_style"], 1);
    }

    #[test]
    fn an_old_camera_studio_splits_into_face_effects() {
        let old = Effect::new(
            "Effects",
            STUDIO_FILTER,
            json!({"smooth": 55, "eye_size": 20, "light_preset": "Soft Box", "light_amt": 35, "filter_preset": "", "bg_mode": 1}),
        );
        let parts = split_camera_studio(&old).unwrap();
        let kinds: Vec<_> = parts.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["studio.beauty", "studio.light", "studio.bg_blur"]);
        assert_eq!(parts[0].settings["smooth"], 55);
        assert_eq!(parts[0].settings["eye_size"], 20);
        assert_eq!(parts[0].settings["foundation"], 20, "left to the default before: still the default");
        assert!(parts[0].settings.get("mk_lip_opacity").is_none(), "the filter applies the look's makeup");
        assert_eq!(parts[1].settings["light_amt"], 35);
        // The merged filter does what the old one did.
        let merged = studio_settings(&parts);
        assert_eq!(merged["smooth"], 55);
        assert_eq!(merged["eye_size"], 20);
        assert_eq!(merged["bg_mode"], 1);
    }

    #[test]
    fn sliders_read_in_human_units() {
        let skin = def("studio.beauty").unwrap();
        let tone = skin.sliders().find(|s| s.key == "skin_tone").unwrap();
        assert!(tone.bipolar());
        assert_eq!(tone.display(20.0), "+20%");
        let angle = def("fx.halftone").unwrap().sliders().find(|s| s.key == "angle").unwrap();
        assert_eq!(angle.display(std::f64::consts::FRAC_PI_4), "45°");
        assert_eq!(skin.summary(&Value::Object(skin.defaults())), "Smooth 50% · Foundation 20% · Bright eyes 30%");
        assert_eq!(def("studio.light").unwrap().summary(&json!({"light_preset": "Neon", "light_amt": 40})), "Neon · Intensity 40%");
        assert_eq!(search("blur").count(), 4, "Background blur, Blur, and the Blur & sharpen category");
    }
}
