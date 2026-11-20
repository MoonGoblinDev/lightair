//! From the workspace's mixer to the audio engine's graph (docs/plans/audio-engine.md): tracks,
//! buses, mixes and the master, with each effect's settings read through the catalog (missing
//! values take the catalog's defaults).

use lightair_audio as la;
use la::dsp::*;
use lightair_model::catalog::{self, EffectDef};
use lightair_model::{Effect, Id, Workspace, Route, TrackChannels};
use serde_json::Value;

/// The engine's id for a document id.
pub fn node(id: Id) -> la::NodeId {
    id.0.as_u128()
}

/// The cue mix: tracks with Cue on, heard on the monitoring device (headphones).
pub const CUE: la::NodeId = 1;

struct Reader<'a> {
    def: &'static EffectDef,
    s: &'a Value,
}

impl Reader<'_> {
    fn default(&self, key: &str) -> Option<Value> {
        self.def.controls.iter().find(|c| c.key() == Some(key)).and_then(|c| c.default_json())
    }
    fn num(&self, key: &str) -> f32 {
        self.s.get(key).and_then(Value::as_f64).or_else(|| self.default(key).and_then(|v| v.as_f64())).unwrap_or(0.0)
            as f32
    }
    fn flag(&self, key: &str) -> bool {
        self.s.get(key).and_then(Value::as_bool).or_else(|| self.default(key).and_then(|v| v.as_bool())).unwrap_or(false)
    }
    fn text(&self, key: &str) -> String {
        self.s
            .get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| self.default(key).and_then(|v| v.as_str().map(String::from)))
            .unwrap_or_default()
    }
}

fn band(r: &Reader, n: usize) -> Band {
    let k = |f: &str| format!("b{n}_{f}");
    let slope = r.num(&k("slope")).clamp(12.0, 48.0) as u8;
    let kind = match r.text(&k("type")).as_str() {
        "hp" => BandKind::HighPass(slope),
        "lp" => BandKind::LowPass(slope),
        "ls" => BandKind::LowShelf,
        "hs" => BandKind::HighShelf,
        "notch" => BandKind::Notch,
        _ => BandKind::Bell,
    };
    Band { on: r.flag(&k("on")), kind, freq: r.num(&k("freq")), gain_db: r.num(&k("gain")), q: r.num(&k("q")) }
}

/// An effect's engine params; `None` for kinds the engine doesn't run (older libobs filters).
pub fn params(e: &Effect) -> Option<Params> {
    let def = catalog::def(&e.kind)?;
    let r = Reader { def, s: &e.settings };
    Some(match e.kind.as_str() {
        "audio.gain" => Params::Gain(GainParams {
            db: r.num("db"),
            invert: r.flag("invert"),
            mono: r.flag("mono"),
            swap: r.flag("swap"),
        }),
        "audio.eq" => {
            let mut p = EqParams::default();
            for (i, b) in p.bands.iter_mut().enumerate() {
                *b = band(&r, i + 1);
            }
            p.output_db = r.num("output_db");
            Params::Eq(p)
        }
        "audio.compressor" => Params::Compressor(CompressorParams {
            threshold_db: r.num("threshold_db"),
            ratio: r.num("ratio"),
            knee_db: r.num("knee_db"),
            attack_ms: r.num("attack_ms"),
            release_ms: r.num("release_ms"),
            makeup_db: r.num("makeup_db"),
            auto_makeup: r.flag("auto_makeup"),
            mix: r.num("mix"),
            sidechain_hpf: r.num("sidechain_hpf"),
        }),
        "audio.gate" => Params::Gate(GateParams {
            threshold_db: r.num("threshold_db"),
            range_db: r.num("range_db"),
            ratio: r.num("ratio"),
            attack_ms: r.num("attack_ms"),
            hold_ms: r.num("hold_ms"),
            release_ms: r.num("release_ms"),
            hysteresis_db: r.num("hysteresis_db"),
        }),
        "audio.limiter" => Params::Limiter(LimiterParams {
            ceiling_db: r.num("ceiling_db"),
            release_ms: r.num("release_ms"),
            input_db: r.num("input_db"),
        }),
        "audio.deesser" => Params::DeEsser(DeEsserParams {
            freq: r.num("freq"),
            threshold_db: r.num("threshold_db"),
            range_db: r.num("range_db"),
            split: r.flag("split"),
            listen: r.flag("listen"),
        }),
        "audio.ducker" => Params::Ducker(DuckerParams {
            source: r.text("source").parse::<uuid::Uuid>().map(|u| u.as_u128()).unwrap_or(0),
            amount_db: r.num("amount_db"),
            threshold_db: r.num("threshold_db"),
            attack_ms: r.num("attack_ms"),
            hold_ms: r.num("hold_ms"),
            release_ms: r.num("release_ms"),
        }),
        "audio.noise" => Params::Noise(NoiseParams { strength: r.num("strength"), vad_threshold: r.num("vad_threshold") }),
        "audio.reverb" => Params::Reverb(ReverbParams {
            size: r.num("size"),
            damping: r.num("damping"),
            pre_delay_ms: r.num("pre_delay_ms"),
            width: r.num("width"),
            mix: r.num("mix"),
        }),
        "audio.delay" => Params::Delay(DelayParams {
            time_ms: r.num("time_ms"),
            feedback: r.num("feedback"),
            tone_hz: r.num("tone_hz"),
            ping_pong: r.flag("ping_pong"),
            mix: r.num("mix"),
        }),
        "audio.channel" => Params::Channel(ChannelParams { width: r.num("width"), bass_mono_hz: r.num("bass_mono_hz") }),
        "audio.voice" => Params::Voice(VoiceParams {
            amount: r.num("amount"),
            noise: r.flag("noise"),
            gate: r.flag("gate"),
            deess: r.flag("deess"),
            eq: r.flag("eq"),
            compress: r.flag("compress"),
            limit: r.flag("limit"),
            podcast: r.num("character") >= 0.5,
        }),
        lightair_model::catalog::PLUGIN_KIND => Params::Plugin(PluginParams { plugin: crate::plugins::instance(e) }),
        _ => return None,
    })
}

fn inserts(effects: &[Effect]) -> Vec<la::EffectSpec> {
    effects
        .iter()
        .filter_map(|e| params(e).map(|p| la::EffectSpec { id: node(e.id), enabled: e.enabled, params: p }))
        .collect()
}

fn dest(r: Route) -> la::Dest {
    match r {
        Route::Master => la::Dest::Program,
        Route::Bus(b) => la::Dest::Bus(node(b)),
        Route::None => la::Dest::Nowhere,
    }
}

fn sends(sends: &[lightair_model::Send]) -> Vec<la::Send> {
    sends.iter().map(|s| la::Send { mix: node(s.mix), level_db: s.level_db, pre_fader: s.pre_fader }).collect()
}

fn fader(db: f32) -> f32 {
    if db.is_finite() { db } else { -145.0 }
}

/// The engine graph for a workspace's mixer.
pub fn spec(workspace: &Workspace) -> la::GraphSpec {
    let mut strips: Vec<la::Strip> = workspace
        .tracks
        .iter()
        .map(|t| {
            let channels = match t.channels {
                // the input object delivers mono inputs on both channels
                TrackChannels::All => la::Channels::Stereo(0, 1),
                TrackChannels::Mono(c) => la::Channels::Mono(c),
                TrackChannels::Stereo(a, b) => la::Channels::Stereo(a, b),
            };
            la::Strip {
                name: t.name.clone(),
                trim_db: t.trim_db,
                invert: t.invert,
                mono: t.mono,
                inserts: inserts(&t.effects),
                gain_db: fader(t.volume_db),
                pan: t.pan,
                mute: t.muted,
                solo: false,
                dest: dest(t.route),
                sends: {
                    let mut s = sends(&t.sends);
                    if t.cue {
                        s.push(la::Send { mix: CUE, level_db: 0.0, pre_fader: false });
                    }
                    s
                },
                ..la::Strip::track(node(t.id), node(t.source), channels)
            }
        })
        .collect();
    strips.extend(workspace.audio.buses.iter().map(|b| la::Strip {
        name: b.name.clone(),
        trim_db: b.trim_db,
        inserts: inserts(&b.effects),
        gain_db: fader(b.volume_db),
        pan: b.pan,
        mute: b.muted,
        dest: dest(b.route),
        sends: {
            let mut s = sends(&b.sends);
            if b.cue {
                s.push(la::Send { mix: CUE, level_db: 0.0, pre_fader: false });
            }
            s
        },
        ..la::Strip::bus(node(b.id))
    }));
    let master = &workspace.audio.master;
    let mut mixes: Vec<la::Mix> = workspace
        .audio
        .mixes
        .iter()
        .map(|m| la::Mix {
            name: m.name.clone(),
            inserts: inserts(&m.effects),
            gain_db: fader(m.volume_db),
            mute: m.muted,
            to_program: m.to_program,
            pan: m.pan,
            trim_db: m.trim_db,
            to_cue: m.cue,
            ..la::Mix::new(node(m.id))
        })
        .collect();
    let a = &workspace.audio;
    let cued = workspace.tracks.iter().any(|t| t.cue)
        || a.buses.iter().any(|b| b.cue)
        || a.mixes.iter().any(|m| m.cue)
        || master.cue;
    if cued {
        mixes.push(la::Mix { name: "Cue".into(), ..la::Mix::new(CUE) });
    }
    la::GraphSpec {
        strips,
        mixes,
        program: la::Master {
            inserts: inserts(&master.effects),
            gain_db: fader(master.volume_db),
            mute: master.muted,
            trim_db: master.trim_db,
            to_cue: master.cue,
        },
        cue: cued.then_some(CUE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightair_model::{AudioMix, Bus, Source, Send, Track};
    use serde_json::json;

    #[test]
    fn a_workspace_mixer_becomes_the_engine_graph() {
        let mut p = Workspace::new("Show");
        let mic = Source::new("Mic", lightair_model::kinds::MIC, json!({}));
        let bus = Bus::new("Guests");
        let phones = AudioMix::new("Headphones");
        let mut t = Track::new("Host", mic.id);
        t.channels = TrackChannels::Mono(2);
        t.route = Route::Bus(bus.id);
        t.sends = vec![Send { mix: phones.id, level_db: -6.0, pre_fader: true }];
        t.volume_db = f32::NEG_INFINITY;
        t.effects = vec![
            Effect::new("Voice", "audio.voice", json!({"amount": 0.8})),
            Effect::new("Old", "noise_suppress_filter_v2", json!({})),
        ];
        let tid = t.id;
        p.sources.insert(mic.id, mic.into());
        p.tracks.push(t.into());
        p.audio.buses.push(bus.clone());
        p.audio.mixes.push(phones.clone());
        let g = spec(&p);
        let host = g.strips.iter().find(|s| s.id == node(tid)).unwrap();
        assert_eq!(host.role, la::Role::Track { input: node(p.tracks[0].source), channels: la::Channels::Mono(2) });
        assert_eq!(host.dest, la::Dest::Bus(node(bus.id)));
        assert_eq!(host.sends, vec![la::Send { mix: node(phones.id), level_db: -6.0, pre_fader: true }]);
        assert!(host.gain_db < -144.0, "−∞ fader: silence");
        assert_eq!(host.inserts.len(), 1, "the old libobs filter isn't the engine's");
        match &host.inserts[0].params {
            Params::Voice(v) => assert!((v.amount - 0.8).abs() < 1e-6 && v.noise, "set value + defaults"),
            other => panic!("{other:?}"),
        }
        assert!(g.strips.iter().any(|s| s.id == node(bus.id) && s.role == la::Role::Bus));
        assert_eq!(g.mixes.len(), 1);
    }

    #[test]
    fn eq_bands_and_a_ducker_source_read_from_settings() {
        let src = Id::new();
        let eq = Effect::new("EQ", "audio.eq", json!({"b1_on": true, "b1_slope": 48, "b4_on": true, "b4_gain": 3.0}));
        let Some(Params::Eq(p)) = params(&eq) else { panic!() };
        assert_eq!(p.bands[0].kind, BandKind::HighPass(48));
        assert!(p.bands[0].on && (p.bands[0].freq - 80.0).abs() < 1e-3);
        assert_eq!(p.bands[3].kind, BandKind::Bell);
        assert!((p.bands[3].gain_db - 3.0).abs() < 1e-6 && !p.bands[1].on);
        let duck = Effect::new("Duck", "audio.ducker", json!({"source": src.0.to_string()}));
        let Some(Params::Ducker(d)) = params(&duck) else { panic!() };
        assert_eq!(d.source, node(src));
        assert!((d.amount_db - 12.0).abs() < 1e-6);
    }

    #[test]
    fn the_engine_chain_follows_the_workspace_order() {
        use lightair_model::{Command, Document, MixerStrip};
        let mut doc = Document::new(Workspace::new("Order"));
        let mic = Source::new("Mic", "lightair_audio_device", json!({}));
        let mut t = Track::new("Host", mic.id);
        let names = ["audio.gate", "audio.eq", "audio.compressor", "audio.limiter"];
        t.effects = names.iter().map(|k| Effect::new(*k, *k, json!({}))).collect();
        let (tid, ids): (Id, Vec<Id>) = (t.id, t.effects.iter().map(|e| e.id).collect());
        doc.apply(&Command::AddSource { source: mic }).unwrap();
        doc.apply(&Command::AddTrack { track: t, index: None }).unwrap();
        let order = |doc: &Document| -> Vec<&'static str> {
            let g = spec(doc.workspace());
            g.strips.iter().find(|s| s.id == node(tid)).unwrap().inserts.iter().map(|e| e.params.kind()).collect()
        };
        assert_eq!(order(&doc), names);
        // the limiter dragged to the top: removed, put back first (as the UI does)
        let limiter = doc.workspace().track(tid).unwrap().effects[3].clone();
        let strip = MixerStrip::Track(tid);
        doc.apply(&Command::Batch {
            label: "Move".into(),
            commands: vec![
                Command::RemoveStripEffect { strip, effect: ids[3] },
                Command::AddStripEffect { strip, effect: limiter, index: Some(0) },
            ],
        })
        .unwrap();
        assert_eq!(order(&doc), ["audio.limiter", "audio.gate", "audio.eq", "audio.compressor"]);
        let g = spec(doc.workspace());
        let first = &g.strips.iter().find(|s| s.id == node(tid)).unwrap().inserts[0];
        assert_eq!(first.id, node(ids[3]), "the same effect (its id keeps its state in the engine)");
    }
}
