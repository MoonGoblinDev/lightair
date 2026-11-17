//! The effect catalog's shader effects (`fx.*`, the Live filter's) on a clip's picture: the
//! same effects ported to WGSL (fx.wgsl), each one pass or several, run on the GPU before the
//! picture is composited. Their settings are the .effect files' uniforms, read from those files
//! (names, order, defaults), so the two stay in step.

use std::collections::HashMap;

use crate::sequence::ClipEffect;

const PRELUDE: &str = include_str!("fx_prelude.wgsl");
const EFFECTS: &str = include_str!("fx.wgsl");

macro_rules! effect_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../../../modules/lightair-effect-stack/data/fx/", $name, ".effect")))),*]
    };
}

/// Each effect's .effect file (for its uniforms).
const FILES: &[(&str, &str)] = effect_files!(
    "blur", "bulge", "chroma", "duotone", "edges", "emboss", "glow", "grain", "halftone", "invert", "kaleido", "mirror", "mono", "pixelate",
    "posterize", "scanlines", "sketch", "swirl", "tiltshift", "vhs", "vignette", "wave",
);

/// A uniform of an effect: its name, whether it's a colour, its default.
#[derive(Debug, Clone, PartialEq)]
pub struct Uniform {
    pub name: String,
    pub colour: bool,
    pub default: [f32; 4],
}

/// An effect's uniforms, in order (`u.p[0]`, `u.p[1]`, …).
pub fn uniforms(name: &str) -> Vec<Uniform> {
    let Some((_, file)) = FILES.iter().find(|(n, _)| *n == name) else { return Vec::new() };
    file.lines()
        .filter_map(|l| {
            let l = l.trim().strip_prefix("uniform ")?;
            let (decl, value) = l.split_once('=').map_or((l.trim_end_matches(';'), ""), |(a, b)| (a, b));
            let mut parts = decl.split_whitespace();
            let (ty, name) = (parts.next()?, parts.next()?.trim_end_matches(';'));
            let nums: Vec<f32> = value
                .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            let colour = ty.starts_with("float4") || ty.starts_with("float3");
            let default = if colour {
                [nums.first().copied().unwrap_or(1.0), nums.get(1).copied().unwrap_or(1.0), nums.get(2).copied().unwrap_or(1.0), nums.get(3).copied().unwrap_or(1.0)]
            } else {
                [nums.first().copied().unwrap_or(0.0), 0.0, 0.0, 0.0]
            };
            Some(Uniform { name: name.to_string(), colour, default })
        })
        .collect()
}

/// The effects there are (their names without "fx."), and how many passes each runs.
pub fn passes() -> &'static HashMap<String, usize> {
    static P: std::sync::OnceLock<HashMap<String, usize>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut m: HashMap<String, usize> = HashMap::new();
        for line in EFFECTS.lines() {
            let Some(rest) = line.trim().strip_prefix("fn fs_") else { continue };
            let Some(name_pass) = rest.split('(').next() else { continue };
            if let Some((name, pass)) = name_pass.rsplit_once('_')
                && let Ok(n) = pass.parse::<usize>()
            {
                let e = m.entry(name.to_string()).or_insert(0);
                *e = (*e).max(n + 1);
            }
        }
        m
    })
}

/// A setting as the shader takes it: a number, or a colour (OBS's 0xAABBGGRR).
fn value_of(u: &Uniform, settings: &serde_json::Map<String, serde_json::Value>) -> [f32; 4] {
    match settings.get(&u.name) {
        Some(v) if u.colour => match v.as_u64() {
            Some(c) => [(c & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, ((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 24) & 0xFF) as f32 / 255.0],
            None => u.default,
        },
        Some(v) => match v.as_f64().or_else(|| v.as_bool().map(|b| if b { 1.0 } else { 0.0 })) {
            Some(x) => [x as f32, 0.0, 0.0, 0.0],
            None => u.default,
        },
        None => u.default,
    }
}

/// The effects' pipelines, made as they're first used.
pub struct Fx {
    device: wgpu::Device,
    queue: wgpu::Queue,
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<String, wgpu::RenderPipeline>,
    linear: wgpu::Sampler,
    point: wgpu::Sampler,
    /// Three targets per size, passes going round them (never writing what they read).
    targets: HashMap<(u32, u32), [wgpu::Texture; 3]>,
}

impl Fx {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Fx {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lightair-fx"),
            source: wgpu::ShaderSource::Wgsl(format!("{PRELUDE}\n{EFFECTS}").into()),
        });
        let tex = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let samp = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                tex(1),
                tex(2),
                samp(3),
                samp(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        };
        Fx {
            device: device.clone(),
            queue: queue.clone(),
            module,
            layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            linear: sampler(wgpu::FilterMode::Linear),
            point: sampler(wgpu::FilterMode::Nearest),
            targets: HashMap::new(),
        }
    }

    fn pipeline(&mut self, entry: &str) -> &wgpu::RenderPipeline {
        if !self.pipelines.contains_key(entry) {
            let p = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            });
            self.pipelines.insert(entry.to_string(), p);
        }
        &self.pipelines[entry]
    }

    /// Runs `effects` (those switched on and known) on `input` (`size`) at `time` seconds: the
    /// texture holding the result, or `None` when there was nothing to run.
    pub fn run(&mut self, input: &wgpu::Texture, size: (u32, u32), effects: &[ClipEffect], time: f32) -> Option<wgpu::Texture> {
        let runs: Vec<(String, usize, Vec<[f32; 4]>)> = effects
            .iter()
            .filter(|e| e.on)
            .filter_map(|e| {
                let name = e.kind.strip_prefix("fx.")?.to_string();
                let n = *passes().get(&name)?;
                let values = uniforms(&name).iter().map(|u| value_of(u, &e.settings)).collect();
                Some((name, n, values))
            })
            .collect();
        if runs.is_empty() {
            return None;
        }
        let targets = self.targets.entry(size).or_insert_with(|| {
            let make = || {
                self.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };
            [make(), make(), make()]
        });
        let targets = targets.clone();
        let mut encoder = self.device.create_command_encoder(&Default::default());
        // each effect reads the last one's result as its source
        let mut current: Option<usize> = None;
        for (name, n, values) in runs {
            let source_view = match current {
                None => input.create_view(&Default::default()),
                Some(i) => targets[i].create_view(&Default::default()),
            };
            let mut image = current;
            for pass in 0..n {
                let out = (0..3).find(|i| Some(*i) != image && Some(*i) != current).unwrap_or(0);
                // size, texel; clock; p[8]
                let mut u = [0f32; 40];
                u[0..4].copy_from_slice(&[size.0 as f32, size.1 as f32, 1.0 / size.0 as f32, 1.0 / size.1 as f32]);
                u[4] = time;
                for (k, v) in values.iter().take(8).enumerate() {
                    u[8 + k * 4..12 + k * 4].copy_from_slice(v);
                }
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: (u.len() * 4) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bytes: Vec<u8> = u.iter().flat_map(|v| v.to_le_bytes()).collect();
                self.queue.write_buffer(&buffer, 0, &bytes);
                let image_view = match image {
                    None => input.create_view(&Default::default()),
                    Some(i) => targets[i].create_view(&Default::default()),
                };
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&image_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&source_view) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.linear) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.point) },
                    ],
                });
                let entry = format!("fs_{name}_{pass}");
                let pipeline = self.pipeline(&entry).clone();
                let view = targets[out].create_view(&Default::default());
                {
                    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    rp.set_pipeline(&pipeline);
                    rp.set_bind_group(0, &group, &[]);
                    rp.draw(0..3, 0..1);
                }
                image = Some(out);
            }
            current = image;
        }
        self.queue.submit([encoder.finish()]);
        current.map(|i| targets[i].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_has_its_passes_and_uniforms() {
        for (name, _) in FILES {
            assert!(passes().get(*name).is_some_and(|n| *n >= 1), "{name} has no passes in fx.wgsl");
        }
        assert_eq!(passes()["blur"], 4);
        let mono = uniforms("mono");
        assert_eq!(mono.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["amount", "tint_color", "tint"]);
        assert!(mono[1].colour && (mono[1].default[1] - 0.86).abs() < 1e-6);
        assert_eq!(uniforms("mirror")[0].name, "axis");
        let mut s = serde_json::Map::new();
        s.insert("tint_color".into(), serde_json::json!(0xFF0000FFu32));
        assert_eq!(value_of(&mono[1], &s), [1.0, 0.0, 0.0, 1.0]);
    }
}
