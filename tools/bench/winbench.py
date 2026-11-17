"""Lightair vs OBS benchmark on Windows (docs/benchmarks/lightair-vs-obs-windows.md): the same
streamer scenario as bench.py (macOS), built and measured in both apps through the same
obs-websocket v5 requests. Everything it makes goes in target/bench-win.

  python tools/bench/winbench.py prepare                OBS copy (portable), clean settings, media
  python tools/bench/winbench.py setup obs|lightair     build the scenario (once)
  python tools/bench/winbench.py startup obs|lightair N launch N times: time to window and ready
  python tools/bench/winbench.py run obs|lightair TAG   idle 60 s, then record 60 s cycling scenes
  python tools/bench/winbench.py quick obs|lightair TAG 30 s idle (for iterating; BENCH_SCENE)

OBS runs as a portable copy of the installed one (target/bench-win/obs, its settings beside it),
so the user's own OBS settings are never touched. Plugins installed for all users
(C:\\ProgramData\\obs-studio\\plugins) would load into it too: they're moved aside while OBS runs
(`third_party_plugins_off`) and put back afterwards.

Measured each second over the app's process tree (Chromium's helpers included): CPU (percent of
one core, from the processes' own CPU time, and the cycles they ran, QueryProcessCycleTime), the
GPU's busy time per engine for those processes (Windows's GPU Engine counters, summed over the
3D, copy, video encode and decode engines; and the 3D engine of the whole GPU), memory (private
bytes and working set), and the desktop compositor's CPU and GPU (dwm.exe); libobs's own render
time and skipped frames from GetStats.
"""
import contextlib, ctypes, json, os, shutil, statistics, subprocess, sys, time
from ctypes import wintypes

import psutil
import win32api, win32con, win32cred, win32gui, win32pdh, win32process

TOOLS = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(TOOLS))
B = os.environ.get("LIGHTAIR_BENCH_DIR", os.path.join(ROOT, "target", "bench-win"))
sys.path.insert(0, TOOLS)
from obsws import ObsWs, Rpc  # noqa: E402

M = os.path.join(B, "media")
OBS_INSTALL = os.environ.get("OBS_DIR", r"C:\Program Files\obs-studio")
OBS_ROOT = os.path.join(B, "obs")
OBS_EXE = os.path.join(OBS_ROOT, "bin", "64bit", "obs64.exe")
LIGHTAIR_EXE = os.environ.get("LIGHTAIR_EXE", os.path.join(ROOT, "target", "release", "lightair.exe"))
LA_WS = os.path.join(B, "la-ws")
PORT = {"obs": 4460, "lightair": 4466}
SCENES = ["Starting Soon", "Gameplay", "Desktop", "Just Chatting", "BRB", "Ending"]
SYSTEM_PLUGINS = r"C:\ProgramData\obs-studio\plugins"
SYSTEM_PLUGINS_ASIDE = r"C:\ProgramData\obs-studio\plugins.lightair-bench-aside"


def media(name):
    return os.path.join(M, name)


def file_url(path):
    return "file:///" + path.replace("\\", "/")


def inputs():
    font = {"face": "Arial", "size": 64, "flags": 1, "style": "Bold"}
    big = dict(font, size=120)
    return {
        "Background": ("image_source", {"file": media("background.png")}),
        "Overlay Tint": ("color_source_v3", {"color": 0x60000000, "width": 1920, "height": 1080}),
        "Gameplay Video": ("ffmpeg_source", {"local_file": media("gameplay.mp4"), "looping": True, "is_local_file": True}),
        "Display": ("monitor_capture", {"capture_cursor": True}),
        "Webcam": ("ffmpeg_source", {"local_file": media("webcam.mp4"), "looping": True, "is_local_file": True}),
        "Cam Frame": ("image_source", {"file": media("camframe.png")}),
        "Logo": ("image_source", {"file": media("logo.png")}),
        "Alerts": ("browser_source", {"url": file_url(media("alerts.html")), "width": 1920, "height": 1080,
                                      "fps": 30, "fps_custom": True}),
        "Chat": ("browser_source", {"url": file_url(media("chat.html")), "width": 400, "height": 800,
                                    "fps": 30, "fps_custom": True}),
        "Title": ("text_ft2_source_v2", {"text": "LIVE: Benchmark Stream", "font": font}),
        "Starting Text": ("text_ft2_source_v2", {"text": "Starting Soon", "font": big}),
        "BRB Text": ("text_ft2_source_v2", {"text": "Be Right Back", "font": big}),
        "Ending Text": ("text_ft2_source_v2", {"text": "Thanks for watching", "font": big}),
        "Music": ("ffmpeg_source", {"local_file": media("music.m4a"), "looping": True, "is_local_file": True}),
        "Mic": ("ffmpeg_source", {"local_file": media("mic.m4a"), "looping": True, "is_local_file": True}),
    }


FULL = (0, 0, 1920, 1080)
LAYOUT = {
    "Starting Soon": [("Background", FULL), ("Overlay Tint", FULL), ("Logo", (760, 160, 400, 400)),
                      ("Starting Text", (620, 640)), ("Alerts", FULL), ("Music", None)],
    "Gameplay": [("Gameplay Video", FULL), ("Webcam", (1400, 770, 480, 270)), ("Cam Frame", (1380, 750, 520, 310)),
                 ("Logo", (40, 40, 120, 120)), ("Title", (40, 980)), ("Chat", (1480, 60, 400, 640)),
                 ("Alerts", FULL), ("Mic", None), ("Music", None)],
    "Desktop": [("Display", FULL), ("Webcam", (1400, 770, 480, 270)), ("Cam Frame", (1380, 750, 520, 310)),
                ("Logo", (40, 40, 120, 120)), ("Title", (40, 980)), ("Chat", (1480, 60, 400, 640)),
                ("Alerts", FULL), ("Mic", None), ("Music", None)],
    "Just Chatting": [("Background", FULL), ("Webcam", (80, 140, 1280, 720)), ("Chat", (1420, 140, 440, 800)),
                      ("Title", (80, 900)), ("Alerts", FULL), ("Mic", None), ("Music", None)],
    "BRB": [("Background", FULL), ("Gameplay Video", (1200, 80, 640, 360)), ("Overlay Tint", FULL),
            ("Logo", (760, 160, 400, 400)), ("BRB Text", (600, 640)), ("Music", None)],
    "Ending": [("Background", FULL), ("Logo", (760, 160, 400, 400)), ("Ending Text", (520, 640)), ("Music", None)],
}

# ------------------------------------------------------------------ connecting


def password(app):
    if app == "obs":
        return None
    # keyring's Windows store: a generic credential named "<user>.<service>"
    try:
        c = win32cred.CredRead("password.Lightair remote API", win32cred.CRED_TYPE_GENERIC)
    except Exception:
        return None
    blob = c["CredentialBlob"]
    for enc in ("utf-8", "utf-16-le"):
        try:
            s = blob.decode(enc)
            if s.isprintable():
                return s
        except Exception:
            pass
    return None


def connect(app, wait=90):
    until = time.time() + wait
    last = None
    while time.time() < until:
        try:
            return ObsWs(port=PORT[app], password=password(app), timeout=30)
        except Exception as e:
            last = e
            time.sleep(0.05)
    raise RuntimeError(f"{app}: no websocket ({last})")


def setup(app):
    with app_running(app):
        o = connect(app)
        wait_loaded(o)
        made = set()
        have = {s["sceneName"] for s in o.req("GetSceneList")["scenes"]}
        for scene in SCENES:
            if scene not in have:
                o.req("CreateScene", {"sceneName": scene})
        for scene in SCENES:
            for name, box in LAYOUT[scene]:
                if name not in made:
                    kind, settings = inputs()[name]
                    r = o.req("CreateInput", {"sceneName": scene, "inputName": name, "inputKind": kind, "inputSettings": settings})
                    made.add(name)
                    item = r["sceneItemId"]
                else:
                    item = o.req("CreateSceneItem", {"sceneName": scene, "sourceName": name})["sceneItemId"]
                if box is None:
                    continue
                t = {"positionX": box[0], "positionY": box[1], "alignment": 5}
                if len(box) == 4:
                    t.update({"boundsType": "OBS_BOUNDS_STRETCH", "boundsWidth": box[2], "boundsHeight": box[3], "boundsAlignment": 0})
                o.req("SetSceneItemTransform", {"sceneName": scene, "sceneItemId": item, "sceneItemTransform": t})
        # the display: the first monitor either app lists
        items = o.req("GetInputPropertiesListPropertyItems", {"inputName": "Display", "propertyName": "monitor_id"}, check=False)
        first = next((i["itemValue"] for i in (items.get("propertyItems") or []) if i.get("itemEnabled", True)), None)
        if first:
            o.req("SetInputSettings", {"inputName": "Display", "inputSettings": {"monitor_id": first}})
        o.req("SetInputVolume", {"inputName": "Music", "inputVolumeDb": -20})
        if app == "lightair":
            r = Rpc(port=PORT[app], password=password(app))
            rec = r.call("workspace.get")["outputs"]["recording"]
            rec["folder"] = os.path.join(B, "rec-la")
            # the same encoder as OBS here (x264 veryfast, ABR, 2 s keyframes): NVENC needs a newer driver
            rec["encoder"] = os.environ.get("BENCH_ENCODER", "x264")
            print("lightair recording settings:", json.dumps(rec))
            r.call("commands.execute", {"commands": [{"SetRecordingSettings": {"settings": rec}}]})
        o.req("SetCurrentProgramScene", {"sceneName": "Gameplay"})
        for s in o.req("GetSceneList")["scenes"]:
            if s["sceneName"] not in SCENES:
                o.req("RemoveScene", {"sceneName": s["sceneName"]})
        print(app, "scenes:", [s["sceneName"] for s in o.req("GetSceneList")["scenes"]])
        time.sleep(2)


# ------------------------------------------------------------------ processes


@contextlib.contextmanager
def third_party_plugins_off():
    moved = False
    if os.path.isdir(SYSTEM_PLUGINS) and not os.path.exists(SYSTEM_PLUGINS_ASIDE):
        os.rename(SYSTEM_PLUGINS, SYSTEM_PLUGINS_ASIDE)
        moved = True
    try:
        yield
    finally:
        if moved:
            for _ in range(50):
                try:
                    os.rename(SYSTEM_PLUGINS_ASIDE, SYSTEM_PLUGINS)
                    break
                except OSError:
                    time.sleep(0.2)


def launch(app):
    log = open(os.path.join(B, f"{app}-stdout.log"), "w")
    if app == "obs":
        # a run that had to be ended would have OBS ask about its "crash" first
        shutil.rmtree(os.path.join(OBS_ROOT, "config", "obs-studio", ".sentinel"), ignore_errors=True)
        cmd = [OBS_EXE, "--portable", "--disable-updater", "--disable-shutdown-check", "--disable-missing-files-check"]
        cwd = os.path.dirname(OBS_EXE)
        env = dict(os.environ)
    else:
        cmd = [LIGHTAIR_EXE]
        cwd = os.path.dirname(LIGHTAIR_EXE)
        env = dict(os.environ, LIGHTAIR_PROJECT=LA_WS, LIGHTAIR_REMOTE_PORT=str(PORT["lightair"]),
                   LIGHTAIR_LOG="info,libobs=info", LIGHTAIR_STARTER="plain")
        env["PATH"] = os.path.join(OBS_INSTALL, "bin", "64bit") + os.pathsep + env.get("PATH", "")
    t0 = time.time()
    p = subprocess.Popen(cmd, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT,
                         creationflags=subprocess.CREATE_NEW_PROCESS_GROUP)
    return t0, p


@contextlib.contextmanager
def app_running(app):
    ctx = third_party_plugins_off() if app == "obs" else contextlib.nullcontext()
    with ctx:
        t0, p = launch(app)
        try:
            yield p
        finally:
            quit_app(app, p.pid)


def top_window(pid):
    found = []

    def each(hwnd, _):
        if win32gui.IsWindowVisible(hwnd) and win32gui.GetWindow(hwnd, win32con.GW_OWNER) == 0:
            _, wpid = win32process.GetWindowThreadProcessId(hwnd)
            if wpid == pid and win32gui.GetWindowText(hwnd):
                found.append(hwnd)
        return True

    win32gui.EnumWindows(each, None)
    return found[0] if found else None


def tree(pid):
    try:
        p = psutil.Process(pid)
        return [p] + p.children(recursive=True)
    except psutil.NoSuchProcess:
        return []


def quit_app(app, pid, wait=30):
    t0 = time.time()
    hwnd = top_window(pid)
    if hwnd:
        win32gui.PostMessage(hwnd, win32con.WM_CLOSE, 0, 0)
    procs = tree(pid)
    gone, alive = psutil.wait_procs(procs, timeout=wait)
    for p in alive:
        with contextlib.suppress(Exception):
            p.kill()
    took = time.time() - t0
    time.sleep(2)
    return took


def wait_loaded(o, timeout=60):
    until = time.time() + timeout
    while time.time() < until:
        try:
            o.req("GetSceneList")
            return True
        except Exception:
            time.sleep(0.05)
    return False


def wait_ready(o, timeout=60):
    until = time.time() + timeout
    while time.time() < until:
        try:
            r = o.req("GetSceneList")
            names = {s["sceneName"] for s in r["scenes"]}
            if set(SCENES) <= names and r["currentProgramSceneName"] in SCENES:
                return True
        except Exception:
            pass
        time.sleep(0.02)
    return False


# ------------------------------------------------------------------ measuring

_k32 = ctypes.WinDLL("kernel32", use_last_error=True)
_k32.QueryProcessCycleTime.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_uint64)]
_k32.QueryProcessCycleTime.restype = wintypes.BOOL


def cycles(pids):
    total = 0
    for pid in pids:
        try:
            h = win32api.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        except Exception:
            continue
        c = ctypes.c_uint64()
        if _k32.QueryProcessCycleTime(int(h), ctypes.byref(c)):
            total += c.value
        win32api.CloseHandle(h)
    return total


class Gpu:
    """Windows's GPU Engine counters: per process, per engine type, busy percent."""

    def __init__(self):
        self.q = win32pdh.OpenQuery()
        self.counters = {}
        self.refreshed = 0
        self.refresh()

    def refresh(self):
        try:
            paths = win32pdh.ExpandCounterPath(r"\GPU Engine(*)\Utilization Percentage")
        except Exception:
            paths = []
        for p in paths:
            if p not in self.counters:
                with contextlib.suppress(Exception):
                    self.counters[p] = win32pdh.AddCounter(self.q, p)
        self.refreshed = time.time()

    def collect(self):
        win32pdh.CollectQueryData(self.q)

    def read(self, pids):
        """{engine: percent} summed over `pids`, and the whole GPU's 3D engine per adapter."""
        mine, total3d = {}, {}
        for path, c in list(self.counters.items()):
            try:
                _, v = win32pdh.GetFormattedCounterValue(c, win32pdh.PDH_FMT_DOUBLE)
            except Exception:
                continue
            inst = path[path.index("(") + 1:path.rindex(")")]
            parts = inst.split("_")
            try:
                pid = int(parts[parts.index("pid") + 1])
                eng = inst[inst.index("engtype_") + 8:]
                luid = "_".join(parts[parts.index("luid") + 1:parts.index("luid") + 3])
            except (ValueError, IndexError):
                continue
            if eng == "3D":
                total3d[luid] = total3d.get(luid, 0) + v
            if pid in pids:
                mine[eng] = mine.get(eng, 0) + v
        return mine, total3d


def dwm_pid():
    for p in psutil.process_iter(["name"]):
        if (p.info["name"] or "").lower() == "dwm.exe":
            return p.pid
    return None


def sample(pid, seconds, o, during=None):
    gpu = Gpu()
    dwm = dwm_pid()
    procs = tree(pid)
    prev = {p.pid: sum(p.cpu_times()[:2]) for p in procs}
    prev_cyc = cycles([p.pid for p in procs])
    prev_dwm = sum(psutil.Process(dwm).cpu_times()[:2]) if dwm else 0
    prev_dwm_cyc = cycles([dwm]) if dwm else 0
    gpu.collect()
    prev_t = time.time()
    start = prev_t
    stats0 = o.req("GetStats")
    rows = []
    k = 0
    while time.time() - start < seconds:
        time.sleep(1)
        if during:
            during(k)
        k += 1
        if time.time() - gpu.refreshed > 5:
            gpu.refresh()
        gpu.collect()
        now = time.time()
        dt = now - prev_t
        procs = tree(pid)
        cur = {}
        for p in procs:
            with contextlib.suppress(Exception):
                cur[p.pid] = sum(p.cpu_times()[:2])
        cpu = sum(v - prev.get(p, v) for p, v in cur.items()) / dt * 100
        main = (cur.get(pid, 0) - prev.get(pid, 0)) / dt * 100
        cyc = cycles(list(cur))
        mem_private = mem_ws = 0
        for p in procs:
            with contextlib.suppress(Exception):
                mi = p.memory_info()
                mem_private += mi.private
                mem_ws += mi.wset
        d_cpu = d_cyc = 0
        if dwm:
            with contextlib.suppress(Exception):
                dn = sum(psutil.Process(dwm).cpu_times()[:2])
                d_cpu = (dn - prev_dwm) / dt * 100
                prev_dwm = dn
            dc = cycles([dwm])
            d_cyc = (dc - prev_dwm_cyc) / dt / 1e9
            prev_dwm_cyc = dc
        mine, total3d = gpu.read(set(cur))
        dwm_gpu, _ = gpu.read({dwm}) if dwm else ({}, {})
        row = {"t": now - start, "cpu_tree": cpu, "cpu_main": main, "gcycles": (cyc - prev_cyc) / dt / 1e9,
               "private_mb": mem_private / 1e6, "wset_mb": mem_ws / 1e6, "procs": len(procs),
               "gpu_3d": mine.get("3D", 0), "gpu_copy": mine.get("Copy", 0),
               "gpu_vdec": mine.get("VideoDecode", 0), "gpu_venc": mine.get("VideoEncode", 0),
               "gpu_total3d": max(total3d.values()) if total3d else 0,
               "dwm_cpu": d_cpu, "dwm_gcycles": d_cyc, "dwm_gpu3d": dwm_gpu.get("3D", 0)}
        rows.append(row)
        prev, prev_t, prev_cyc = cur, now, cyc
    stats1 = o.req("GetStats")
    return rows, stats0, stats1


def uss_mb(pid):
    """Memory the app's processes use that no other process shares (pages actually touched):
    private bytes also count what's committed and never used."""
    total = 0
    for p in tree(pid):
        with contextlib.suppress(Exception):
            total += p.memory_full_info().uss
    return total / 1e6


def summarize(rows):
    out = {}
    for k in rows[0]:
        if k == "t":
            continue
        vals = sorted(r[k] for r in rows if r.get(k) is not None)
        if vals:
            out[k] = {"mean": statistics.fmean(vals), "p95": vals[max(0, int(len(vals) * 0.95) - 1)], "max": vals[-1]}
    return out


def brief(sm):
    keys = ("cpu_tree", "cpu_main", "gcycles", "gpu_3d", "gpu_copy", "gpu_vdec", "gpu_venc", "gpu_total3d",
            "private_mb", "wset_mb", "dwm_cpu", "dwm_gpu3d")
    return {k: round(sm[k]["mean"], 2 if k == "gcycles" else 1) for k in keys if k in sm}


def render_stats(s0, s1):
    frames = s1["renderTotalFrames"] - s0["renderTotalFrames"]
    return {"render_ms": round(s1["averageFrameRenderTime"], 2),
            "render_skipped": s1["renderSkippedFrames"] - s0["renderSkippedFrames"],
            "output_skipped": s1["outputSkippedFrames"] - s0["outputSkippedFrames"],
            "frames": frames}


def ffprobe(path):
    r = subprocess.run(["ffprobe", "-v", "error", "-count_packets", "-select_streams", "v:0", "-show_entries",
                        "stream=nb_read_packets,avg_frame_rate,width,height,codec_name:format=duration,size,bit_rate",
                        "-of", "json", path], capture_output=True, text=True)
    return json.loads(r.stdout or "{}")


def startup(app, n):
    results = []
    for i in range(n):
        with (third_party_plugins_off() if app == "obs" else contextlib.nullcontext()):
            t0, p = launch(app)
            t_window = None
            while time.time() - t0 < 60 and t_window is None:
                if top_window(p.pid):
                    t_window = time.time() - t0
                else:
                    time.sleep(0.01)
            o = connect(app)
            t_ws = time.time() - t0
            wait_ready(o)
            t_ready = time.time() - t0
            time.sleep(3)
            mem = sum(pp.memory_info().private for pp in tree(p.pid)) / 1e6
            t_quit = quit_app(app, p.pid)
        results.append({"window_s": t_window, "websocket_s": t_ws, "ready_s": t_ready, "private_mb_at_3s": mem, "quit_s": t_quit})
        print(app, i, json.dumps(results[-1]))
        time.sleep(4)
    summary = {k: statistics.median([r[k] for r in results if r[k] is not None]) for k in results[0]}
    json.dump({"app": app, "runs": results, "median": summary}, open(os.path.join(B, f"startup-{app}.json"), "w"), indent=1)
    print(app, "median", json.dumps(summary))


def run(app, tag):
    rec_dir = os.path.join(B, "rec-obs" if app == "obs" else "rec-la")
    before = set(os.listdir(rec_dir))
    with app_running(app) as p:
        o = connect(app)
        wait_ready(o)
        print(app, "ready; settling")
        time.sleep(20)
        o.req("SetCurrentProgramScene", {"sceneName": "Gameplay"})
        time.sleep(5)
        idle, s0, s1 = sample(p.pid, 60, o)
        idle_uss = uss_mb(p.pid)
        print(app, "idle", json.dumps(brief(summarize(idle))), json.dumps(render_stats(s0, s1)), "uss_mb", round(idle_uss))
        cycle = ["Just Chatting", "Desktop", "BRB", "Gameplay", "Starting Soon", "Gameplay"]

        def switch(k):
            if k % 10 == 9:
                o.req("SetCurrentProgramScene", {"sceneName": cycle[(k // 10) % len(cycle)]})

        o.req("StartRecord")
        rec, r0, r1 = sample(p.pid, 60, o, during=switch)
        rec_uss = uss_mb(p.pid)
        stop = o.req("StopRecord")
        print(app, "recording", json.dumps(brief(summarize(rec))), json.dumps(render_stats(r0, r1)), "uss_mb", round(rec_uss))
        time.sleep(3)
    files = sorted(set(os.listdir(rec_dir)) - before)
    path = stop.get("outputPath") or (os.path.join(rec_dir, files[-1]) if files else None)
    probe = ffprobe(path) if path and os.path.exists(path) else {}
    out = {"app": app, "tag": tag,
           "idle": {"summary": summarize(idle), "render": render_stats(s0, s1), "uss_mb": idle_uss, "rows": idle},
           "recording": {"summary": summarize(rec), "render": render_stats(r0, r1), "uss_mb": rec_uss, "rows": rec,
                         "file": path, "probe": probe}}
    json.dump(out, open(os.path.join(B, f"run-{app}-{tag}.json"), "w"), indent=1)
    print(app, "file", path, json.dumps(probe))


def quick(app, tag, seconds=30):
    with app_running(app) as p:
        o = connect(app)
        wait_ready(o)
        time.sleep(12)
        o.req("SetCurrentProgramScene", {"sceneName": os.environ.get("BENCH_SCENE", "Gameplay")})
        time.sleep(5)
        rows, s0, s1 = sample(p.pid, seconds, o)
        out = {**brief(summarize(rows)), **render_stats(s0, s1)}
        print(app, tag, json.dumps(out))
        json.dump(out, open(os.path.join(B, f"quick-{app}-{tag}.json"), "w"))


# ------------------------------------------------------------------ preparing

MEDIA = [
    ["-f", "lavfi", "-i", "testsrc2=s=1920x1080:r=60,hue=H=t*0.5", "-f", "lavfi", "-i", "sine=f=220:r=48000,volume=0.2",
     "-f", "lavfi", "-i", "sine=f=330:r=48000,volume=0.15", "-filter_complex", "[1][2]amerge=inputs=2[a]", "-map", "0",
     "-map", "[a]", "-t", "60", "-c:v", "libx264", "-preset", "fast", "-b:v", "12M", "-pix_fmt", "yuv420p", "-g", "120",
     "-c:a", "aac", "-b:a", "192k", "gameplay.mp4"],
    ["-f", "lavfi", "-i", "mandelbrot=s=1280x720:r=30", "-t", "60", "-c:v", "libx264", "-preset", "fast", "-b:v", "4M",
     "-pix_fmt", "yuv420p", "-g", "60", "-an", "webcam.mp4"],
    ["-f", "lavfi", "-i", "anoisesrc=c=pink:r=48000:a=0.08,tremolo=f=3:d=0.8", "-t", "60", "-ac", "1", "-c:a", "aac",
     "-b:a", "128k", "mic.m4a"],
    ["-f", "lavfi", "-i", "sine=f=261.6:r=48000,volume=0.1", "-f", "lavfi", "-i", "sine=f=329.6:r=48000,volume=0.1",
     "-f", "lavfi", "-i", "sine=f=392:r=48000,volume=0.1", "-filter_complex", "[0][1][2]amix=inputs=3,apulsator=hz=0.5",
     "-t", "60", "-ac", "2", "-c:a", "aac", "-b:a", "192k", "music.m4a"],
    ["-f", "lavfi", "-i", "gradients=s=1920x1080:c0=0x1a1040:c1=0x0a3050:x0=0:y0=0:x1=1920:y1=1080", "-frames:v", "1",
     "background.png"],
    ["-f", "lavfi", "-i", "color=c=0x00000000:s=400x400,format=rgba,geq=r='255':g='120':b='40':a='if(lt(hypot(X-200,Y-200),180),255,0)'",
     "-frames:v", "1", "logo.png"],
    ["-f", "lavfi", "-i", "color=c=black:s=520x310,format=rgba,geq=r='255':g='255':b='255':a='if(lt(X,20)+gt(X,499)+lt(Y,20)+gt(Y,289),255,0)'",
     "-frames:v", "1", "camframe.png"],
]


def prepare():
    for d in ("media", "rec-obs", "rec-la"):
        os.makedirs(os.path.join(B, d), exist_ok=True)
    for args in MEDIA:
        if not os.path.exists(media(args[-1])):
            subprocess.run(["ffmpeg", "-v", "error", "-y", *args], cwd=M, check=True)
    for page in ("alerts.html", "chat.html"):
        shutil.copy(os.path.join(TOOLS, page), media(page))
    if not os.path.exists(OBS_EXE):
        print("copying OBS (portable) from", OBS_INSTALL)
        for sub in ("bin", "core", "data", "obs-plugins"):
            src = os.path.join(OBS_INSTALL, sub)
            if os.path.isdir(src):
                shutil.copytree(src, os.path.join(OBS_ROOT, sub), dirs_exist_ok=True)
    open(os.path.join(OBS_ROOT, "portable_mode.txt"), "w").close()
    c = os.path.join(OBS_ROOT, "config", "obs-studio")
    for d in ("basic/profiles/Bench", "basic/scenes", "plugin_config/obs-websocket"):
        os.makedirs(os.path.join(c, d), exist_ok=True)
    files = {
        "global.ini": """[General]
Pre31Migrated=true
Pre33.0Migrated=true
MaxLogs=10
EnableAutoUpdates=false
LastVersion=553648129
InfoIncrement=-1

[Video]
Renderer=Direct3D 11
""",
        "user.ini": """[General]
FirstRun=true
ConfirmOnExit=false

[Basic]
Profile=Bench
ProfileDir=Bench
SceneCollection=Bench
SceneCollectionFile=Bench

[BasicWindow]
PreviewEnabled=true
WarnBeforeStoppingRecord=false
SysTrayEnabled=false
""",
        "basic/profiles/Bench/basic.ini": f"""[General]
Name=Bench

[Video]
BaseCX=1920
BaseCY=1080
OutputCX=1920
OutputCY=1080
FPSType=0
FPSCommon=60
ColorFormat=NV12
ColorSpace=709
ColorRange=Partial

[Audio]
SampleRate=48000
ChannelSetup=Stereo

[Output]
Mode=Advanced

[AdvOut]
RecType=Standard
RecFilePath={os.path.join(B, "rec-obs").replace(chr(92), "/")}
RecFormat2=mp4
RecEncoder=obs_x264
RecAudioEncoder=ffmpeg_aac
RecTracks=1
Track1Bitrate=192
""",
        "basic/profiles/Bench/recordEncoder.json": json.dumps({"bitrate": 12000, "rate_control": "ABR", "profile": "high", "preset": "veryfast",
                                                               "keyint_sec": 2}),
        "plugin_config/obs-websocket/config.json": json.dumps({"alerts_enabled": False, "auth_required": False,
                                                               "first_load": False, "server_enabled": True,
                                                               "server_password": "", "server_port": 4460}),
        "basic/scenes/Bench.json": json.dumps({"name": "Bench", "current_scene": "Scene", "current_program_scene": "Scene",
            "scene_order": [{"name": "Scene"}], "sources": [{"id": "scene", "versioned_id": "scene", "name": "Scene",
            "uuid": "2f7c1b7e-0d3a-4b8e-9a51-1c2d3e4f5a6b", "settings": {"items": [], "id_counter": 0}}],
            "transitions": [], "current_transition": "Fade", "transition_duration": 300}),
    }
    for name, text in files.items():
        path = os.path.join(c, name)
        # (the scene collection `setup` built is kept)
        if name.startswith("basic/scenes/") and os.path.exists(path):
            continue
        open(path, "w").write(text)
    print("prepared", B)


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "prepare":
        prepare()
    elif cmd == "setup":
        setup(sys.argv[2])
    elif cmd == "startup":
        startup(sys.argv[2], int(sys.argv[3]))
    elif cmd == "run":
        run(sys.argv[2], sys.argv[3])
    elif cmd == "quick":
        quick(sys.argv[2], sys.argv[3])
