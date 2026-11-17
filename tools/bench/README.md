# Lightair vs OBS benchmark

The same streamer scenario built and measured in Lightair and in a vanilla OBS Studio, both
driven through the same obs-websocket v5 requests (OBS's own server; Lightair's compatible
one). Results and findings: [docs/benchmarks/lightair-vs-obs.md](../../docs/benchmarks/lightair-vs-obs.md).

Needs: macOS, OBS 33 in /Applications, ffmpeg, a release bundle (`cargo xtask bundle --release`).

```sh
python3 tools/bench/bench.py prepare          # OBS copy, clean OBS settings, media → target/bench
./target/bench/obs-swap.sh in                 # clean OBS settings in place of yours (renamed, kept)
python3 tools/bench/bench.py setup obs        # builds the six scenes in OBS (once)
python3 tools/bench/bench.py startup obs 5
python3 tools/bench/bench.py run obs base
./target/bench/obs-swap.sh out                # your OBS settings back
python3 tools/bench/bench.py setup lightair   # the same scenes in a scratch Lightair workspace
python3 tools/bench/bench.py startup lightair 5
python3 tools/bench/bench.py run lightair base
```

macOS OBS has no portable mode (it ignores `--portable`), so a vanilla OBS means swapping its
settings folder: `obs-swap.sh in` renames yours to `obs-studio.lightair-bench-backup` and puts
the clean one in its place; `out` puts yours back. Lightair runs on port 4466 with its own
workspace (`target/bench/la-ws`), never yours.

- `quick APP TAG`: 30 s idle with Gameplay on air (for iterating on a change); `BENCH_SCENE`
  picks another scene. It reports CPU energy (`power_mw`) and instructions (`instr_g`): compare
  those, not CPU %, which counts time on any core at any clock.
- `profile APP TAG`: macOS `sample` of the main process; `prof.py` and the call graph show
  where the time goes per thread.
- Results land in `target/bench` as JSON (`startup-*.json`, `run-*.json`).

## Windows

`winbench.py` builds and measures the same scenario on Windows, against a portable copy of the
installed OBS (`OBS_DIR`, default `C:\Program Files\obs-studio`) whose settings live beside it, so
your own OBS settings are never touched. Plugins installed for all users
(`C:\ProgramData\obs-studio\plugins`) are moved aside while OBS runs and put back. Results:
[docs/benchmarks/lightair-vs-obs-windows.md](../../docs/benchmarks/lightair-vs-obs-windows.md).

Needs: Python with `psutil` and `pywin32`, ffmpeg, a release build with its runtime beside it
(`cargo build --release -p lightair-app -p lightair-web-helper`, then `cargo xtask dev --release`).

```
python tools/bench/winbench.py prepare          # portable OBS copy, its clean settings, media → target/bench-win
python tools/bench/winbench.py setup obs        # the six scenes in OBS (once)
python tools/bench/winbench.py setup lightair   # the same in a scratch Lightair workspace
python tools/bench/winbench.py run lightair r1  # 60 s idle on Gameplay, then 60 s recording, switching scenes
python tools/bench/winbench.py run obs r1
python tools/bench/winbench.py startup lightair 5
python tools/bench/winbench.py quick lightair t # 30 s idle (BENCH_SCENE picks another scene)
python tools/bench/threads.py PID 10            # where a running app's CPU goes, per thread
```

Alternate the apps between runs (a laptop warms up and clocks down), and compare CPU cycles
(`gcycles`, QueryProcessCycleTime) rather than CPU time: Windows counts CPU time in 15.6 ms
ticks and at whatever clock the core ran. GPU figures are Windows's own GPU Engine counters for
the app's processes. `LIGHTAIR_MEDIA_DECODE=software` has Lightair decode videos as OBS does by
default; `BENCH_ENCODER` picks Lightair's recording encoder (x264 by default, as the OBS profile).
