# Lightair vs OBS on Windows: streamer benchmark

17 November 2026. Lightair (release build, the first Windows run of the app) against OBS Studio
33.0.1 as installed, at its defaults. Harness: [tools/bench](../../tools/bench/README.md#windows)
(`winbench.py`). The macOS round is in [lightair-vs-obs.md](lightair-vs-obs.md).

## Result

Gameplay scene on air (1080p60 video with sound, 720p30 webcam video, chat and alert web pages,
logo, title, mic and music files), 1920×1080 at 60 fps. The final build against OBS, three runs
each, alternating, the same evening; each run 60 s idle then 60 s recording while switching
scenes every 10 s (through Desktop, which captures the display).

| | OBS 33.0.1 | Lightair | |
|---|---|---|---|
| **CPU, idle** (cycles, app and helpers) | 3.00 G/s | **1.82 G/s** | −39 % |
| CPU time, idle (% of one core) | 18.0 % | **9.0 %** | |
| **GPU 3D busy, idle** (the app's processes) | 38.2 % | **13.3 %** | −65 % |
| GPU 3D busy, idle (the whole GPU) | 34.8 % | **11.2 %** | |
| Render time per frame (libobs), idle | **1.23 ms** | 1.63 ms | |
| **CPU, recording** (cycles) | 8.00 G/s | **6.01 G/s** | −25 % |
| CPU time, recording | 106.0 % | **69.3 %** | |
| GPU 3D busy, recording (the whole GPU) | 19.1 % | **12.7 %** | |
| **Frames skipped while recording** (render / output, of ~10,900) | 14 / 9 | 13 / **0** | |
| Memory the app uses alone (USS), idle / recording | **426 / 813 MB** | 592 / 942 MB | +39 / +16 % |
| Desktop compositor (dwm.exe) GPU, idle | **2.7 %** | 5.2 % | |
| Startup: window on screen (median of 5) | 2.09 s | **1.67 s** | |
| Startup: ready over obs-websocket | **2.10 s** | 2.19 s | |
| Quit | 30 s (didn't close when asked) | **0.9 s** | |

Both recordings come out 1920×1080 at 60 fps, x264 veryfast at 12 Mbps (ABR), 2 s keyframes,
AAC 192 kbps, MP4 (~3,610 frames a minute in both).

Where Lightair spends more: memory (the GPU video decoder's surfaces, Lightair's own audio engine
and a newer Chromium in its process) and a little of the compositor's GPU (the preview and meters
are composition visuals it blends; OBS's preview is a window of its own). Its remote API used to
answer 0.6 s after OBS's (2.69 s): the password is now read while libobs starts, and the API
listens before Chromium starts (2.19 s).

### Desktop scene (the display captured)

30 s idle with Desktop on air (display capture, webcam video, overlays), one run each:

| | OBS | Lightair |
|---|---|---|
| CPU | 1.70 G/s | 1.72 G/s |
| GPU 3D busy (the whole GPU) | 33.3 % | **13.4 %** |
| Desktop compositor GPU | 32.3 % | **11.8 %** |

Switching to Desktop (the capture starting) skipped 3–4 frames in both.

### With both on software decoding

Lightair decodes videos on the GPU on Windows (below); OBS leaves that off. Decoding the same
way, the two are even on CPU and Lightair still uses less GPU (3 runs of Lightair, 5 of OBS):

| | OBS | Lightair |
|---|---|---|
| CPU, idle | 2.97 G/s | 3.01 G/s |
| CPU, recording | 8.01 G/s | 7.90 G/s |
| GPU 3D busy, idle | 35.9 % | **21.5 %** |
| Frames skipped while recording (render / output) | 26 / 10 | **15 / 0** |

Two videos decoding in software are two thirds of either app's CPU (`av:h264` threads, 1.8 G
cycles/s of 2.7): the rest of each app costs about the same.

## Machine and scenario

Laptop: Intel Core i5-9300H (4 cores, 8 threads), 16 GB, NVIDIA GeForce GTX 1050 Ti (driver
560.81) with Intel UHD 630 driving the built-in display, Windows 11 23H2. Both apps render on the
NVIDIA GPU (OBS through its `NvOptimusEnablement` export; Lightair picks the GPU Windows ranks
highest for performance) and both draw their UI windows on the Intel GPU's display.

The six scenes and their sources are built in both apps by the same script, through the same
obs-websocket requests (OBS's own server; Lightair's compatible one), as on macOS: Starting Soon,
Gameplay, Desktop (a display capture of the main monitor), Just Chatting, BRB, Ending. The media
are generated files (ffmpeg) so both see identical input.

**Vanilla OBS:** a portable copy of the installed OBS 33.0.1 with clean settings beside it (the
user's own settings untouched); plugins installed for all users moved aside while it ran. Its log
confirms only its own modules loaded.

**Encoder:** NVENC needs a newer NVIDIA driver than this laptop's for OBS 33 (OBS says
"outdated_driver", and so does Lightair, which uses the same libobs module); OBS's Quick Sync test
crashed. So both record with x264, the encoder every PC has.

**Measured** each second over the app's process tree (Chromium's helpers included): CPU cycles
(`QueryProcessCycleTime`; CPU time is counted in 15.6 ms ticks at whatever clock the core ran, so
cycles are what to compare), Windows's GPU Engine counters per process, private bytes, working
set, and the memory no other process shares (USS) at the end of each phase. Runs alternate between
the apps (a laptop warms up and clocks down).

## What made the difference

1. **Videos decode on the GPU's video decoder** (Lightair's default on Windows; OBS's media
   source has it, off). CPU idle 3.0 → 1.8 G cycles/s, recording 7.9 → 6.0. On Apple Silicon this
   didn't pay (software decoding is cheap there, and the frames copied back cost more), so it stays
   off on macOS. It costs memory: the decoder keeps a pool of surfaces (USS +100 MB).
2. **The preview is drawn by DirectComposition under GPUI's UI**, as macOS draws it with Core
   Animation: libobs's graphics thread presents each frame into a swap chain shown by a composition
   visual; no frame is read back to memory, nothing in the UI redraws for it. GPU 3D: OBS 36 %,
   Lightair 13–22 %.
3. **The preview is rendered at the size it's shown** (up to the canvas's own), with libobs's area
   filter: on this laptop each preview frame crosses from the NVIDIA GPU to the Intel GPU that
   drives the screen, and a third of the width is a ninth of the copying. GPU copy engine 7.6 →
   3.4 %, 3D 16 → 13 %, the compositor 6.3 → 5.0 %.
4. **Web pages share their frames as D3D11 textures** opened on libobs's device, rendering on
   libobs's GPU (Chromium is told which), as OBS's browser source does.
5. **The meters are composition visuals moved by a meter thread**: the window doesn't redraw for a
   moving meter.

## Reproduce

```
cargo build --release -p lightair-app -p lightair-web-helper
cargo xtask dev --release
python tools/bench/winbench.py prepare
python tools/bench/winbench.py setup obs
python tools/bench/winbench.py setup lightair
python tools/bench/winbench.py run lightair r1
python tools/bench/winbench.py run obs r1
```

Results land in `target/bench-win` as JSON. `LIGHTAIR_MEDIA_DECODE=software` decodes as OBS does.
