# Lightair's patch to gpui-pre-windows 0.3.12

Vendored from crates.io (`gpui-pre-windows` 0.3.12) and patched in the workspace's
`[patch.crates-io]`.

- **Composition underlay** (`src/underlay.rs`, `DirectComposition` in `src/directx_renderer.rs`):
  the window's composition target's root is a container visual holding an empty "underlay"
  visual below GPUI's swap chain visual. `gpui_windows::underlay::get(hwnd)` hands the underlay
  and its composition device to the app, which shows the canvas preview (presented by libobs's
  own D3D11 device) and the meters there, under the UI, without GPUI redrawing for each frame:
  the Windows counterpart of Lightair's Core Animation preview on macOS
  (`crates/lightair-app/src/native_preview.rs`).
