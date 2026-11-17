# Lightair vs OBS: streamer benchmark

November 2026. Lightair (release builds) against OBS Studio 33.0.0, vanilla. Harness:
[tools/bench](../../tools/bench/README.md).

## Third round: energy, and nothing off air

Measured the same afternoon, Lightair and OBS alternately, two runs each (`quick`, 20 s):

| | Lightair | OBS 33 |
|---|---|---|
| CPU energy, Gameplay (app + helpers) | **169 mW** | 1057 mW |
| Instructions retired, Gameplay | **1.07 G/s** | 3.5 G/s |
| CPU energy, Desktop (display captured) | **297 mW** | 518 mW |
| Instructions retired, Desktop | **1.40 G/s** | 1.86 G/s |
| Window server CPU, Gameplay / Desktop | 40 / 41 % | 36 / 38 % |
| GPU busy, Gameplay / Desktop | **16 / 28 %** | 28 / 31 % |
| Memory | **~980 MB** | ~1265 MB |

"CPU %" (time on a core) said Lightair used 5 points more than OBS; it doesn't. It counts time
on any core at any clock: Lightair's remaining work is light enough that macOS runs it on the
efficiency cores at low clocks, where it takes longer and costs a sixth of the energy. The
harness now reports the kernel's own estimate (`power_mw`, `proc_pid_rusage`) and instructions
retired (`instr_g`) next to CPU %, and those are what to compare. (CPU only: the GPU, the video
decoder and the window server aren't in it.)

What changed since the second round:

- **Video files are decoded by the hardware decoder straight into textures** (Lightair's own
  media source for MP4 and QuickTime files): 1.1 W less than OBS's media source on Gameplay.
- **Nothing off air costs anything.** OBS's screen capture and camera capture run from the
  moment they're made, shown or not; Lightair now makes them when something shows them and lets
  them go a second after nothing does. Web pages close off air. A workspace with a display
  capture in five scenes had kept the window server at 97 % (50 % with nothing running) and the
  GPU at 64 %, the whole Mac sluggish; now 60 % and 45 %.
- **Desktop and app audio come from Core Audio process taps**, not ScreenCaptureKit, whose
  stream had the window server compose the display for audio alone.
- The benchmark's display capture had captured nothing in OBS (an empty display UUID): fixed,
  and the Desktop scene is now compared too.

## Result

Second round (`ec16c73`), both apps measured the same morning, with the Mac in use (so every
number sits a little apart from the overnight round's below; compare within a round):

| | OBS 33 (vanilla) | Lightair |
|---|---|---|
| Startup to ready (median of 3) | 1.09 s | **1.00 s** |
| Memory 3 s after launch (all processes) | 1207 MB | **1138 MB** |
| CPU, idle on Gameplay (app + helpers) | **55.3 %** | 57.5 % |
| CPU, recording while switching scenes | **57.4 %** | 58.0 % |
| GPU busy, idle | 26.5 % | **25.4 %** |
| GPU busy, recording | **26.9 %** | 28.3 % |
| WindowServer CPU (macOS compositor), idle | 41.4 % | **36.9 %** |
| WindowServer CPU, recording | 44.0 % | **34.2 %** |
| Render time per frame (libobs), idle / recording | 1.34 / 3.57 ms | **0.90 / 3.14 ms** |
| Memory while recording | 1369 MB | **1268 MB** |
| Frames skipped (render / output) | 0 / 0 | 0 / 0 |

Lightair's UI now costs the compositor less than OBS's: the window redraws about 3 times a
second while the show plays, the preview and the meters moving on their own (below).

### First round (overnight, `1ab6b6a`)

| | OBS 33 (vanilla) | Lightair before | Lightair after |
|---|---|---|---|
| Startup to ready (median of 5) | 1.13 s | 0.96 s | **0.99 s** |
| Memory 3 s after launch (all processes) | 1204 MB | 1207 MB | **1128 MB** |
| CPU, idle on Gameplay (app + helpers) | 54.2 % | 103.8 % | **56.2 %** |
| CPU, recording while switching scenes | 55.8 % | 112.7 % | **54.3 %** |
| GPU busy, idle | 31.7 % | 71.4 % | **29.8 %** |
| GPU busy, recording | 32.3 % | 68.3 % | **30.7 %** |
| Render time per frame (libobs), idle | 1.71 ms | 4.83 ms | **0.91 ms** |
| Render time per frame, recording | 3.79 ms | 5.35 ms | **2.77 ms** |
| Memory while recording | 1381 MB | 1352 MB | **1268 MB** |
| Frames skipped (render / output) | 0 / 0 | 0 / 0 | 0 / 0 |
| WindowServer CPU (macOS compositor) | 45 % | 47 % | 55 % |

CPU is percent of one core, summed over the app and its helper processes (Chromium), from
the processes' own CPU time, sampled every second for 60 s. GPU is the GPU's "Device
Utilization" (ioreg). Both recordings came out 1920×1080 at 60 fps with every frame
(3616 frames in 60.27 s for OBS, 3617 in 60.28 s for Lightair).

After the first round Lightair matched OBS on CPU and GPU, started faster and used less
memory, but cost the macOS compositor 10 points more: GPUI redrew the whole window for each
preview frame where OBS's Qt window updates only its preview. The second round fixed that.

## Machine and scenario

MacBook Pro, Apple M4 Pro (14 cores), 24 GB, built-in 3024×1964 display at 120 Hz, macOS 27.
Each app ran alone, frontmost, display awake.

Six scenes a streamer would have, built in both apps by the same script through the same
obs-websocket requests (OBS's own server; Lightair's compatible one):

- **Starting Soon**: background image, dark tint, logo, title text, alert overlay (browser),
  music.
- **Gameplay** (on air while idle): 1080p60 gameplay video with sound, 720p30 webcam with a
  frame, logo, title, chat (browser, 400×800), alert overlay (browser, 1920×1080, animated),
  mic, music at −20 dB.
- **Desktop**: display capture of the main display with the same webcam, overlays and audio.
- **Just Chatting**, **BRB**, **Ending**.

Canvas 1920×1080 at 60 fps. Recording: Apple VideoToolbox H.264 (hardware), 12 Mbps average,
2 s keyframes, B-frames, AAC 192 kbps, MP4, the same in both. Browser sources at 30 fps.
The webcam, gameplay, mic and music are generated media files (ffmpeg) so both apps see
identical input; the display capture is real.

**Vanilla OBS**: OBS on macOS has no portable mode, so its settings folder was swapped for a
clean one for the OBS runs (the user's own, with five third-party plugins, renamed and put
back afterwards, byte-for-byte). OBS's log confirms only its bundled modules loaded. Clean
profile: 1080p60, Advanced output with the recording settings above, OpenGL renderer, no
Desktop Audio or Mic/Aux devices (neither app captured a real microphone).

Measured: startup (launch to window, and to the scene collection loaded, answering over the
API) ×5 each; then after 20 s settling, 60 s idle with Gameplay on air; then 60 s recording
while switching scenes every 10 s (Just Chatting, Desktop, BRB, Gameplay, Starting Soon).

## What was wrong, and what changed

In order of effect, measured along the way (idle CPU / GPU):

1. **Off-air sources ran** (104 % → 56 % with the rest; −16 points CPU, −29 GPU on its own).
   A mixer track kept its source active "for its sound", so every video decoded, every web
   page painted and the display capture captured whether its scene was on air or not. Now, as
   in OBS, a source with a picture runs only while a scene showing it is on air; sound-only
   sources (microphones, desktop and app audio) stay on like OBS's global audio sources, and
   off-air web pages are hidden (Chromium stops painting them), as OBS's browser source does.
   This uncovered a Move-transition bug the always-on tracks had hidden (libobs counted a
   video through a fading item one way and back another, and the video stopped on landing):
   fixed, with a test over Cut, Fade and Move.
2. **The window redrew everything at 120 Hz** (104 % → 77 %). The root view, the canvas, the
   mixer and the outputs panel each asked for every display refresh, and in GPUI asking marks
   the view and all its parents dirty, so the whole window re-laid out and repainted 120
   times a second. Now the canvas redraws its own panel only when the preview has a new
   frame, meters move at 30 fps in the same redraws as the canvas, clocks twice a second.
3. **The preview stalled the render thread** (render time 3.2 → 0.8 ms). Each frame it blitted
   the program into an IOSurface, waited for the GPU (`glFinish`), then converted it with a
   VideoToolbox transfer, synchronously. The conversion to NV12 is now a libobs shader, the
   copy is a GPU blit, and a GPU fence hands the frame over when it's done, without waiting.
   (It also fixed a colour slip: the old path encoded BT.709 where the UI decodes BT.601.)
4. **Browser frames went through the CPU** (−2 points). Chromium painted into memory, Lightair
   copied each frame, libobs copied it again and uploaded it. Now on macOS Chromium shares
   its frames as IOSurfaces, drawn by libobs as they are, as OBS's browser source does.
5. **Startup crash, now and then** (stability). Chromium's library loaded lazily, after
   libobs's threads were running; on macOS its allocator takes over the process's memory zone
   as it loads, and a free racing that takeover crashed the app (three crash reports that
   day, the earliest before this work). It now loads first thing in `main`: 20 launches of the
   benchmark workspace in a row, no crash.

Second round:

6. **The preview drawn by Core Animation** (GPU −24 points against GPUI drawing it, same build).
   GPUI redraws its whole window for any change, so every preview frame cost a full redraw.
   The window is now transparent with a view of Lightair's own under GPUI's: it paints the
   window's background and the canvas well, and each canvas's picture is a layer the preview
   hands its frames to straight from libobs's graphics thread. GPUI leaves the well unpainted;
   selection, guides, menus and anything over the canvas draw on top as before. The rest of
   the window is pixel-for-pixel what it was.
7. **The meters drawn by Core Animation too.** Each meter is native layers under the strips'
   holes, moved 60 times a second by a meter thread straight from the audio levels: fluid,
   and free for the UI. They gained a peak hold (held 1.5 s, then falling at 12 dB/s, as on
   consoles), and the peak readout shows the held peak, changing at most four times a second
   instead of every frame.
8. **Chromium's pump runs when Chromium asks** (CEF's `on_schedule_message_pump_work`), at
   least every 33 ms, instead of every 8 ms.

Nothing is throttled: the preview runs at the canvas's rate and the meters at 60 fps.

## What's left

- Not measured: a real camera and microphone (generated files stood in for them), streaming
  (no destination), long sessions, other Macs.
- The native drawing is macOS only; Windows still draws the preview and meters with GPUI
  (`LIGHTAIR_GPUI_PREVIEW=1` does that on macOS too, for comparison).

## libobs's Metal renderer

libobs 31+ has an experimental Metal renderer next to OpenGL. Both apps were measured on each,
back to back, the same afternoon (same harness, same scenario):

| | OBS OpenGL | OBS Metal | Lightair OpenGL | Lightair Metal |
|---|---|---|---|---|
| CPU, idle | 54.6 % | 57.4 % | 57.5 % | 60.3 % |
| CPU, recording | 47.7 % | 54.9 % | 58.3 % | 57.6 % |
| GPU busy, idle / recording | 26.9 / 28.6 % | 33.4 / 31.1 % | 29.3 / 32.7 % | 32.2 / 36.0 % |
| Render time, idle / recording | 1.09 / 4.21 ms | 0.70 / 2.19 ms | 0.88 / 2.95 ms | 0.84 / 2.96 ms |
| VideoToolbox services (system), idle | 0 % | 10 % | 0 % | 0 % |
| Frames skipped | 0 | 0 | 0 | 0 |

- **OBS** spends less time per frame rendering on Metal but more CPU and GPU overall, and its
  Metal build keeps macOS's VideoToolbox decoder and encoder services busy (~10 % more CPU).
- **Lightair** is about the same on both: the work OpenGL made expensive (the preview's copy,
  stalls) is already done outside libobs (the preview and meters are Core Animation layers).
- So the renderer isn't where the remaining cost is; Metal is worth having for the future
  (Apple deprecated OpenGL; HDR), not for speed today.

Lightair runs on Metal with `LIGHTAIR_GRAPHICS=metal`: the preview's frames are BGRA, copied
by libobs into IOSurfaces (on OpenGL they're converted to NV12), and the views GPUI draws (the
program monitor, display thumbnails, the Library player) get an NV12 copy made by VideoToolbox,
once per frame and only when one of them is showing. All 20 preview tests pass on both
renderers. Lightair Effect Stack's green key didn't compile on Metal: libobs's Metal shader converter
breaks a call to a function with no arguments that reads a uniform (`f()` becomes
`f(, uniforms)`), so the helper takes its value as an argument now.

## Hardware decoding of media files

Tried: media sources decoding on the video decoder (`hw_decode`, off by default in OBS too).
Measured on the Gameplay scene (two videos, 1080p60 and 720p30), 30 s each, on and off alternately:

| | Software (before) | Hardware |
|---|---|---|
| CPU, app + helpers | 55.1–57.9 % | 58.0–62.9 % |
| CPU, main process | 46.9–48.9 % | 41.4–43.8 % |
| GPU busy | 24.1–25.7 % | 26.1–34.5 % |
| Render time per frame (libobs) | 0.91–0.93 ms | 2.23–2.60 ms |

The decoding leaves the main process (−6 points) but the total goes up: libobs's media source
copies each decoded frame back from the decoder into memory and uploads it again, so the work
moves rather than disappears, and the frames arrive slower to draw. Not a win on this Mac
(Apple Silicon decodes 1080p H.264 in software cheaply), so it stays off by default, as in OBS;
a source can still turn it on. It may pay off for 4K or HEVC, or on weaker machines.

## Reproduce

```sh
cargo xtask bundle --release
python3 tools/bench/bench.py prepare
```

then the steps in [tools/bench/README.md](../../tools/bench/README.md).
