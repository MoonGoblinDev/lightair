# Audio engine: a real mixer

Status: built (steps 1–8). The engine plays the mixer: tracks, buses, mixes, sends, master,
the built-in effects, AU and VST3 plugins, multitrack WAV recording, interface channels,
low-latency monitoring, templates and the buffer setting. Not yet: true-peak metering, the pitch
and tuner/analyser effects, CAF files and WAV cue markers, effect presets and A/B, drift
compensation between devices (each device is resampled to the engine rate, not clock-matched),
and a VST3 editor that resizes with its window. Builds [10 · Audio](../10-audio.md) properly; replaces the
"each track is a libobs source with libobs filters, everything in one mix" first cut.

## Why our own engine

libobs mixes audio well enough for a single stream, but the brief (streaming, music recording,
podcasts, anything) needs what it can't do:

| Need | libobs | Lightair audio engine |
|---|---|---|
| Buses, groups, aux returns | none | any number |
| Effects on a bus or the master | per-source filters only | inserts on every strip |
| AUv3 / AU / VST3 plugins | VST2 only (obs-vst), crude UI | hosted, native editors, state saved |
| Multitrack | 6 mixes per output | every track and bus, as WAV/CAF files, plus up to 6 in the video file |
| Monitoring | buffered, ~100+ ms | direct CoreAudio, a few ms (needed for singing and instruments) |
| Device channels | whole device | any channel or pair of a multi-channel interface |
| Mix-minus, VOD mix, headphone mixes | no | mixes with per-source send levels |

So: **Lightair's mixer runs in its own real-time engine** (`lightair-audio`, Rust). libobs keeps
video, capture and encoding: each finished mix is handed to libobs as an audio source in its own
libobs mix, which the existing stream and recording encoders take. Sources only libobs can capture
(application audio, system audio via ScreenCaptureKit, media and browser layers) are pulled into
the engine through libobs audio-capture callbacks, and kept out of libobs's own mixes.

## Model (lightair-model)

```text
 Inputs ─► Tracks ──────────────┐            ┌─► Program (master) ─► Stream, Recording track 1
   mic ch 1    ┌ inserts ┐       ├─► Buses ───┤
   mic ch 2    │ fader   │ route │  (groups,  └─► Mixes (sends) ──► headphones, VOD, mix-minus,
   app audio   │ pan     │──────►│   FX)          per-source levels      music-free stream, ISO
   media layer └ sends ──┘       │
```

| Concept | What it is |
|---|---|
| **Track** | A channel strip: one input (a device and its channel or pair, or a libobs source), trim, polarity, mono/stereo, sync offset, insert effects, fader, pan, mute, solo, and a route (Program or a bus) |
| **Bus** | The same strip, fed by tracks routed to it (a group: "Guests", "Music") or by sends (an FX return: shared reverb). Routes to Program or another bus (no loops) |
| **Program** | The master: inserts (limiter, loudness) and fader. What the stream and the main recording track carry |
| **Mix** | An extra output mix: per-source send levels (pre or post fader), its own inserts and fader. Headphones/monitor, Twitch VOD (Program minus excluded tracks), mix-minus for a remote guest, a music-free stream |
| **Output routing** | Each stream destination takes Program or a Mix; recordings take Program plus chosen mixes as video-file tracks (up to 6), and any tracks and buses as separate lossless files |

## Engine (lightair-audio)

- **Graph**: compiled from the model; 48 kHz, 32-bit float, 256-frame blocks (128 for music).
  Model changes compile a new graph and swap it in lock-free; effect state carries across swaps,
  so a fader move never clicks and an effect never resets.
- **Real-time safe**: no allocation, locks or system calls on the audio thread. Parameters
  arrive by atomics/ring buffers and are smoothed (no zipper noise).
- **Clock**: the engine's own audio thread, paced by the monitor device (or a precise timer
  when there's none). Inputs from other devices are drift-compensated by adaptive resampling.
- **Sync**: input buffers carry their capture timestamps; mixes go to libobs with timestamps, so
  audio stays aligned with video. Per-track sync offset ±2000 ms.
- **Meters**: peak, RMS, true peak, LUFS (M/S/I) per strip, gain reduction per dynamics effect,
  read lock-free by the UI.
- **Offline render** for tests: the same graph rendered without devices, deterministic.

## Built-in effects (all controllable, presets, bypass, A/B)

| Effect | Controls |
|---|---|
| **Gain** | Trim (±30 dB), polarity, mono/stereo/swap |
| **Voice Clarity** | One knob (Low, Moderate, High, Very high), then each stage: noise suppression, gate, de-esser, EQ, compressor, limiter |
| **Noise suppression** | RNNoise (neural, real-time), strength, voice-activity gate threshold |
| **Noise gate / expander** | Threshold, range, ratio (expander), attack, hold, release, look-ahead, sidechain HPF |
| **Compressor** | Threshold, ratio, knee, attack, release, makeup / auto-makeup, mix (parallel), sidechain source and HPF; gain-reduction meter |
| **Limiter** | Ceiling (true peak), release, look-ahead |
| **Parametric EQ** | 8 bands: high/low pass (12–48 dB/oct), shelves, bells, notch; frequency, gain, Q; curve over a live spectrum |
| **De-esser** | Frequency, threshold, range, wide/split band, listen |
| **Ducking** | Sidechain from any track or bus: amount, threshold, attack, hold, release (music under the mic) |
| **Reverb** | Room, plate, hall: size, decay, pre-delay, damping, mix |
| **Delay** | Time (ms or tempo), feedback, filter, ping-pong, mix |
| **Pitch / voice** | Pitch shift and formant: deeper, higher, robot, radio, telephone |
| **Channel tools** | Width, pan law, swap, mono below a frequency |
| **Tuner / analyser** | Display effects: tuner for instruments, spectrum |
| **Plugins** | AUv3 and AU v2 (AVAudioUnit), VST3; scanned, searchable, native editor in its own window, state in the project; crashing or slow plugins are reported, not fatal |

## Inputs and outputs

- **Devices**: every CoreAudio input with its channels; a track takes one channel (mono) or a
  pair (stereo), so an 8-channel interface becomes 8 mics. Sample-rate and device changes handled;
  a lost device keeps the strip and reconnects.
- **libobs sources**: app audio, system audio, media/browser/capture-card audio, through the bridge.
- **Monitoring**: any output device, any mix, low latency; per-track monitor mode (off, cue,
  always, for instruments) and solo-in-cue.
- **Recording**:
  - in the video file: Program + up to 5 mixes as audio tracks (MOV/MKV/MP4);
  - **multitrack audio**: chosen tracks and buses as 24-bit or 32-bit float WAV (or CAF for long
    sessions), sample-aligned, named after the strips, with a start timestamp matching the video;
  - audio-only recording (podcast, music) without video;
  - markers shared with video (WAV cue chunks).
- **Streaming**: Program or a mix per destination; Twitch VOD track.

## Use cases, as templates

| Template | Sets up |
|---|---|
| **Streaming** | Mic (Voice Clarity), Game, Music (ducked under the mic), Alerts; Program limiter at −1 dBTP; VOD mix without Music |
| **Podcast** | One track per mic (interface channels), Voice Clarity each, a Guests bus, Program loudness −16 LUFS; every mic recorded to its own WAV; mix-minus ready for remote guests |
| **Music** | Low-latency mode (128 frames), instrument/vocal tracks with direct monitoring, FX bus with reverb, 24-bit WAV per track, no processing on recorded files unless chosen |
| **Gaming / commentary** | Mic, Game, Chat (Discord) with Chat excluded from the stream if wanted |

## Plan

1. **Engine core**: `lightair-audio` crate: graph, strips, buses, mixes, sends, solo/mute,
   pan law, smoothing, meters (peak, RMS, LUFS), offline render; unit tests on exact signals.
2. **Built-in DSP**: gain, EQ, compressor, gate, limiter, de-esser, ducking, noise suppression,
   reverb, delay, channel tools, Voice Clarity; tests on test tones and noise.
3. **Model**: tracks, buses, mixes, routing, recording/stream mix choices in `lightair-model`,
   commands, migration from today's tracks.
4. **Real-time I/O**: CoreAudio inputs with channel selection, monitor output, clock and drift
   compensation; the libobs bridge both ways; A/V sync test.
5. **Multitrack recording**: WAV/CAF writers per track and bus with the video; audio-only mode.
6. **Mixer UI**: strips with input/channel picker, inserts, sends, fader, pan, meters with GR;
   bus and mix strips; routing; effect cards (the effect stack UI); EQ curve.
7. **Plugins**: AUv3/AU hosting first (native on macOS), then VST3; scanner, editor windows.
8. **Templates** and settings (sample rate, buffer size, monitor device).

Each step: tests, screenshots where visible, commit.
