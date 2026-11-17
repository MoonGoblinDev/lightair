//! Which GPU libobs renders on (Windows).
//!
//! libobs takes an adapter index in DXGI's enumeration order, where the first is usually the
//! GPU driving the display: on a laptop with two GPUs that's the integrated one, slower, and
//! without NVENC. Lightair renders on the GPU Windows ranks highest for performance (the one
//! the user picked in Settings › Display › Graphics wins over it), as OBS does by asking the
//! drivers for the discrete GPU. `LIGHTAIR_GPU_ADAPTER=<index>` picks one by index.

/// libobs's adapter index for the GPU to render on, and its LUID (high, low).
#[cfg(windows)]
pub fn preferred_adapter() -> (u32, Option<(i32, u32)>) {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, IDXGIAdapter1,
        IDXGIFactory1, IDXGIFactory6,
    };
    let luid_of = |a: &IDXGIAdapter1| unsafe { a.GetDesc1().ok() }.map(|d| (d.AdapterLuid, d.Flags));
    let pick = || -> Option<(u32, (i32, u32))> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1().ok()? };
        let mut order = Vec::new();
        for i in 0.. {
            let Ok(a) = (unsafe { factory.EnumAdapters1(i) }) else { break };
            order.push(luid_of(&a)?);
        }
        let index_of = |luid: windows::Win32::Foundation::LUID| {
            order.iter().position(|(l, _)| l.HighPart == luid.HighPart && l.LowPart == luid.LowPart)
        };
        if let Some(i) = std::env::var("LIGHTAIR_GPU_ADAPTER").ok().and_then(|v| v.parse::<usize>().ok())
            && let Some((l, _)) = order.get(i)
        {
            return Some((i as u32, (l.HighPart, l.LowPart)));
        }
        let f6: IDXGIFactory6 = windows::core::Interface::cast(&factory).ok()?;
        for i in 0.. {
            let Ok(a) = (unsafe { f6.EnumAdapterByGpuPreference::<IDXGIAdapter1>(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) })
            else {
                break;
            };
            let (luid, flags) = luid_of(&a)?;
            if flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            return index_of(luid).map(|i| (i as u32, (luid.HighPart, luid.LowPart)));
        }
        None
    };
    match pick() {
        Some((i, luid)) => (i, Some(luid)),
        None => (0, None),
    }
}

#[cfg(not(windows))]
pub fn preferred_adapter() -> (u32, Option<(i32, u32)>) {
    (0, None)
}
