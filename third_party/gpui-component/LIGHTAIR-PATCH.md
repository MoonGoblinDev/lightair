# gpui-component 0.9.1, patched for Lightair

A copy of the published crate (without its tests) with four changes: `DockSkin::set_menu_button_visible`
hides the "…" panel menu that tab bars otherwise always draw (`src/dock/mod.rs`,
`src/dock/tab_panel.rs`, marked "Lightair patch"), and `Panel::cached`, which lets a panel ask
the tab group not to draw it as a cached view (the mixer: its cached strips beside a meter
overlay that moves 30 times a second; `src/dock/panel.rs`, `src/dock/tab_panel.rs`), and `Panel::transparent`, which lets a panel's
tab group leave its background unpainted (the canvas, whose preview is drawn under the
window's UI), and a transparent window's root leaves its background unpainted
(`src/root.rs`), and `Slider::fill_from`, a fill from a centre value to the thumb (pan, trim:
`src/slider.rs`), and `DockSkin::set_tab_menu`, a right-click menu on every tab built for its
panel (Lightair's "Open in New Window"), and the drop placeholder for a drop beside a whole
column or row drawn over the dock rather than clipped to the hovered group (`src/dock/mod.rs`,
`src/dock/tab_panel.rs`; the drop itself is gpui-base's, `third_party/gpui-base`). The
workspace uses it through
`[patch.crates-io]` in the root `Cargo.toml`. Drop it once upstream offers the switch.
