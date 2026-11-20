//! `LIGHTAIR_STALLS=1` (Windows): finds where the UI thread is when the window stops
//! responding. A heartbeat ticks on the UI thread every 20 ms; when it hasn't for longer than
//! `LIGHTAIR_STALLS_MS` (150 by default), a watcher thread pauses the UI thread, walks its stack,
//! resumes it, and logs the stack ("UI stall"), every 100 ms while the stall lasts. For finding
//! freezes; off unless asked for.

// (the watch is Windows's: elsewhere only `start`, which does nothing)
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
fn now_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

/// Starts it if `LIGHTAIR_STALLS` is set: the heartbeat on this (the UI) thread, the watcher
/// on its own.
pub fn start(cx: &mut gpui_kit::App) {
    if std::env::var_os("LIGHTAIR_STALLS").is_none() {
        return;
    }
    #[cfg(windows)]
    {
        let threshold: u64 = std::env::var("LIGHTAIR_STALLS_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
        let start = Instant::now();
        let ui = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        HEARTBEAT_MS.store(now_ms(start), Ordering::Relaxed);
        cx.spawn(async move |cx| {
            loop {
                HEARTBEAT_MS.store(now_ms(start), Ordering::Relaxed);
                cx.background_executor().timer(Duration::from_millis(20)).await;
            }
        })
        .detach();
        std::thread::Builder::new()
            .name("stall-watch".into())
            .spawn(move || win::watch(ui, start, threshold))
            .ok();
        tracing::info!(threshold_ms = threshold, "watching the UI thread for stalls");
    }
    #[cfg(not(windows))]
    let _ = cx;
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64, GetThreadContext, RtlLookupFunctionEntry, RtlVirtualUnwind, UNW_FLAG_NHANDLER,
    };
    use windows_sys::Win32::System::Threading::{
        OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    const DEPTH: usize = 64;

    /// CONTEXT must be 16-byte aligned.
    #[repr(C, align(16))]
    struct Aligned(CONTEXT);

    pub fn watch(ui: u32, start: Instant, threshold: u64) {
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, ui) };
        if thread.is_null() {
            tracing::warn!("stall watch: couldn't open the UI thread");
            return;
        }
        let mut stall_logged = 0u64;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let beat = HEARTBEAT_MS.load(Ordering::Relaxed);
            let late = now_ms(start).saturating_sub(beat);
            if late < threshold {
                stall_logged = 0;
                continue;
            }
            // every 100 ms of a stall, a sample
            if stall_logged != 0 && late < stall_logged + 100 {
                continue;
            }
            stall_logged = late;
            let mut frames = [0u64; DEPTH];
            let n = unsafe { sample(thread, &mut frames) };
            // (named now, with the UI thread running again: naming allocates)
            let mut text = String::new();
            let mut ours = 0;
            for &ip in &frames[..n] {
                let mut name = None;
                backtrace::resolve(ip as *mut std::ffi::c_void, |s| {
                    if name.is_none() {
                        let file = s.filename().map(|f| f.display().to_string()).unwrap_or_default();
                        let line = s.lineno().unwrap_or(0);
                        name = Some(format!("{} ({}:{})", s.name().map(|n| n.to_string()).unwrap_or_else(|| "?".into()), short(&file), line));
                    }
                });
                let name = name.unwrap_or_else(|| format!("{ip:#x}"));
                // the frames that say where: ours, and the first few of everything
                if name.contains("lightair") || ours < 12 {
                    text.push_str("\n    ");
                    text.push_str(&name);
                    ours += 1;
                }
            }
            tracing::warn!(late_ms = late, "UI stall, the UI thread is at:{text}");
        }
        #[allow(unreachable_code)]
        unsafe {
            CloseHandle(thread);
        }
    }

    fn short(file: &str) -> &str {
        file.rfind("\\src\\").or_else(|| file.rfind("/src/")).map_or(file, |i| &file[i + 1..])
    }

    /// The paused thread's return addresses, innermost first. Nothing here allocates: the
    /// thread may be paused holding the allocator's lock.
    unsafe fn sample(thread: windows_sys::Win32::Foundation::HANDLE, out: &mut [u64; DEPTH]) -> usize {
        if unsafe { SuspendThread(thread) } == u32::MAX {
            return 0;
        }
        let mut ctx: Aligned = unsafe { std::mem::zeroed() };
        ctx.0.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
        let mut n = 0;
        if unsafe { GetThreadContext(thread, &mut ctx.0) } != 0 {
            let c = &mut ctx.0;
            while n < DEPTH && c.Rip != 0 {
                out[n] = c.Rip;
                n += 1;
                let mut base = 0u64;
                let entry = unsafe { RtlLookupFunctionEntry(c.Rip, &mut base, std::ptr::null_mut()) };
                if entry.is_null() {
                    // a leaf: its return address is at the top of the stack
                    if c.Rsp == 0 {
                        break;
                    }
                    c.Rip = unsafe { *(c.Rsp as *const u64) };
                    c.Rsp += 8;
                } else {
                    let mut data = std::ptr::null_mut();
                    let mut frame = 0u64;
                    unsafe { RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, c.Rip, entry, c, &mut data, &mut frame, std::ptr::null_mut()) };
                }
            }
        }
        unsafe { ResumeThread(thread) };
        n
    }
}
