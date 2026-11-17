"""Per-thread CPU of a running app on Windows (where the time goes, by thread name).

  python tools/bench/threads.py PID [SECONDS]

Thread names come from SetThreadDescription (Rust's std names its threads so; libobs's and
Chromium's threads are named too). Prints each thread's share of one core over the interval.
"""
import ctypes, sys, time
from ctypes import wintypes

import psutil

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
k32.OpenThread.restype = wintypes.HANDLE
k32.GetThreadDescription.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_wchar_p)]
k32.QueryThreadCycleTime.argtypes = [wintypes.HANDLE, ctypes.POINTER(ctypes.c_uint64)]
THREAD_QUERY_LIMITED_INFORMATION = 0x0800


def name(tid):
    h = k32.OpenThread(THREAD_QUERY_LIMITED_INFORMATION, False, tid)
    if not h:
        return "?"
    p = ctypes.c_wchar_p()
    out = "?"
    if k32.GetThreadDescription(h, ctypes.byref(p)) >= 0 and p.value:
        out = p.value
        ctypes.windll.kernel32.LocalFree(p)
    k32.CloseHandle(h)
    return out


def cycles(tid):
    h = k32.OpenThread(THREAD_QUERY_LIMITED_INFORMATION, False, tid)
    if not h:
        return 0
    c = ctypes.c_uint64()
    k32.QueryThreadCycleTime(h, ctypes.byref(c))
    k32.CloseHandle(h)
    return c.value


def snapshot(proc):
    return {t.id: (t.user_time + t.system_time, cycles(t.id)) for t in proc.threads()}


def main(pid, seconds=10.0):
    proc = psutil.Process(pid)
    a = snapshot(proc)
    time.sleep(seconds)
    b = snapshot(proc)
    rows = []
    for tid, (cpu, cyc) in b.items():
        cpu0, cyc0 = a.get(tid, (0.0, 0))
        rows.append(((cpu - cpu0) / seconds * 100, (cyc - cyc0) / seconds / 1e6, tid, name(tid)))
    rows.sort(reverse=True)
    groups = {}
    for pct, mc, tid, n in rows:
        key = n.split("#")[0].strip() or "?"
        g = groups.setdefault(key, [0.0, 0.0, 0])
        g[0] += pct
        g[1] += mc
        g[2] += 1
    total = sum(r[0] for r in rows)
    print(f"total {total:.1f}% of a core, {sum(r[1] for r in rows):.0f} Mcycles/s, {len(rows)} threads")
    for k, (pct, mc, n) in sorted(groups.items(), key=lambda kv: -kv[1][0])[:25]:
        if pct >= 0.05 or mc > 5:
            print(f"{pct:6.2f}%  {mc:8.0f} Mc/s  x{n:<3} {k}")


if __name__ == "__main__":
    main(int(sys.argv[1]), float(sys.argv[2]) if len(sys.argv) > 2 else 10.0)
