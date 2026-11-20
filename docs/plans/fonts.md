# Fonts: the whole Google Fonts library, on every Lightair app

Status (20 November 2026): **planned, nothing built.** The desktop docs already promise "system
and Google Fonts" for text layers, the chat box and widgets (docs/02 LY-10, docs/07, docs/12),
but today `lightair-render` only loads the machine's own fonts (`FontSystem::new()` in
`generate.rs`), the desktop's text layer is libobs's `text_ft2_source_v2`, which picks fonts by
name from the system's installed fonts, and the mobile apps (Lightair Live, Lightair Editor,
built in private repos on this repo's shared crates) have no fonts at all: on iOS, text doesn't
draw until the renderer is handed fonts explicitly.

This plan solves fonts once, in a new shared crate, `lightair-fonts`, so that a project made on
any device shows the same fonts on every other one, and every font in it is licensed for what
creators do with it: bundle it in an app, put it in a stream, export it in a video.

## 1. Principles

- **One library everywhere.** The desktop, Lightair Live and Lightair Editor offer the same
  fonts, from the same catalogue, with the same names, resolved by the same code.
- **Projects name fonts; they never point at files.** A project stores family, weight, style and
  axis values. Opening it on another device fetches what's missing.
- **Only fonts we're allowed to ship.** Every catalogue font's licence allows bundling,
  redistribution and commercial use, and travels with the font.
- **Original files, unmodified.** No subsetting, no format conversion, so reserved font names
  never come into it (§2).
- **No server of our own.** Catalogue and files come from free, public sources (§4), as the rest
  of Lightair needs no server (docs/plans/live-integrations.md, 2.9).
- **Works offline.** A bundled set covers the first project and every interface; downloaded
  fonts are cached for good.
- **Output never waits on the network silently.** A preview may show a fallback while a font
  downloads, and says so; an export or a stream never uses a fallback without saying so.

## 2. Licences

The catalogue is **Google Fonts** (about 1,800 families, from the `google/fonts` repository). Its
families come under three licences, all of which allow what Lightair needs:

| Licence | Share | Bundle in an app | Use in streams and exported video | Obligations |
|---|---|---|---|---|
| SIL Open Font License 1.1 | Nearly all | Yes, commercially too | Yes | Can't be sold on its own; the licence text accompanies the font; a modified font with a Reserved Font Name must be renamed (subsetting and format conversion count as modifying) |
| Apache 2.0 | A few (the Roboto family, some older ones) | Yes | Yes | Licence and notice accompany the font |
| Ubuntu Font Licence 1.0 | The Ubuntu family | Yes | Yes | Licence accompanies the font; modified versions renamed |

Text drawn into pixels (a frame of a stream, an exported video) doesn't redistribute the font, so
output is never restricted.

How Lightair keeps to them:

- Bundled and downloaded files are the upstream originals, byte for byte (verified by hash, §4),
  so Reserved Font Names never apply.
- Each font's licence file is stored next to it in the cache; bundled fonts' licences ship in the
  app; **About › Licences** lists every font in use (bundled, downloaded, in this project) with
  its licence and copyright.
- The catalogue generator (§4) includes a family only if its licence directory in `google/fonts`
  is `ofl`, `apache` or `ufl`, and records which.

Other sources were considered and left out: Fontshare (its licence forbids redistribution, so we
can't serve or bundle its fonts), Font Squirrel (licences vary font by font), and the open
foundries (The League of Moveable Type, Open Foundry, Adobe's Source families, Noto), which are
OFL and mostly already in Google Fonts.

**System fonts** (SF Pro, Segoe UI, the Android system font) are not offered for text that ends
up in a frame: their licences differ by platform (Apple's SF licence is narrow) and a project
using them would look different on another device. They stay for the interface. Roboto is in the
catalogue, so it remains available as a font for content.

**Emoji** are drawn with **Noto Color Emoji** (OFL), bundled, so they look the same on every
device and are licensed for output. Apple Color Emoji isn't licensed for rendering into other
people's videos and exists only on Apple devices.

## 3. Three tiers

| Tier | What | Where it lives |
|---|---|---|
| **Bundled** | About 40 families plus Noto Color Emoji, chosen for interfaces, overlays and captions (below). Variable fonts where the family has one | In each app, read-only |
| **Catalogue** | Every other Google Fonts family, downloaded on first use: picked in the font picker, or needed by a project being opened | The shared cache (§5) |
| **Imported** | The person's own `.ttf` / `.otf` / `.ttc` files, with a one-line note: "You're responsible for this font's licence" | The project (copied into it, so it travels with exports and sync) |

**The bundled set** (decided; adjusted after measuring, within a 25 MB budget including emoji):

| Kind | Families |
|---|---|
| Interface | Inter, JetBrains Mono |
| Sans | Roboto, Open Sans, Montserrat, Poppins, Lato, Nunito, Raleway, Work Sans, DM Sans, Manrope, Space Grotesk, Rubik |
| Bold and condensed display | Oswald, Bebas Neue, Anton, Barlow Condensed, Archivo Black, Bungee, Righteous, Bangers, Alfa Slab One, Russo One |
| Rounded | Fredoka, Baloo 2, Quicksand, Comfortaa |
| Serif | Playfair Display, Merriweather, Lora, DM Serif Display, Abril Fatface |
| Handwriting and script | Pacifico, Caveat, Permanent Marker, Dancing Script, Lobster, Satisfy |
| Pixel and retro | Press Start 2P, VT323, Silkscreen |
| Mono | Space Mono, Roboto Mono |
| Emoji | Noto Color Emoji |

The emoji font's format (CBDT bitmaps or COLRv1) is the one `lightair-render`'s rasteriser
(swash, through cosmic-text) draws correctly at overlay sizes; checked in phase 1.

**Other scripts.** The bundled families cover Latin, Greek and Cyrillic between them. Every other
script falls back to the matching **Noto** family from the catalogue (Noto Sans JP, KR, SC, TC,
Arabic, Hebrew, Devanagari, Thai…), downloaded when first needed. The Chinese, Japanese and Korean
families are large (roughly 10–20 MB per weight), so they are never bundled and only the weights
in use are fetched.

## 4. The catalogue and where files come from

**The catalogue** is a JSON file, `fonts-catalog.json`, generated by `cargo xtask fonts-catalog`
from `google/fonts` at a pinned commit:

| Field | From |
|---|---|
| `family`, `category` (sans, serif, display, handwriting, mono), `designer`, `added` | Each family's `METADATA.pb` |
| `licence` (`ofl`, `apache`, `ufl`), `copyright` | The family's directory and `METADATA.pb` |
| `files`: weight, style, axes (tag, min, max, default), file name, size, SHA-256 | `METADATA.pb` and the files themselves |
| `scripts` (subsets) | `METADATA.pb` |
| `url` per file: Google's own font server (`fonts.gstatic.com`) | The Google Fonts Developer API, called at build time with a key that never leaves the build machine |
| `mirror` per file: `cdn.jsdelivr.net/gh/google/fonts@<commit>/<licence>/<family>/<file>` | Derived from the pinned commit |
| `popularity` rank | The Developer API |

Compressed, it is expected to be a few hundred kilobytes; measured in phase 1.

**Where it lives.**

- Each app release **ships the catalogue it was built with**, so the picker works offline and on
  first launch.
- The same file is **published from this repo as a static file on GitHub Pages** (free for a
  public repo), regenerated when the pin moves. Apps check it at most once a week and when a file
  link fails, with an `ETag` so an unchanged catalogue costs one small request.

**Where files come from.**

1. **Google's font server**, through the `url` in the catalogue: fast, and Google's to keep
   running. A link can go stale when Google updates a font; then:
2. **jsDelivr**, serving the same file from `google/fonts` at the pinned commit (`mirror`).
   jsDelivr limits the size of what it serves from GitHub, so phase 1 checks it against this
   repository's largest families; families it can't serve rely on Google's link alone.
3. Every download is **checked against the catalogue's SHA-256** before it enters the cache; a
   mismatch is discarded and the next source tried.

**Privacy.** A download is a request from the device to Google (or jsDelivr), which sees its IP
address. The privacy policy and Settings say so; **Settings › Fonts › Download fonts** can be
turned off, leaving the bundled set, imported fonts and what's already cached.

## 5. The `lightair-fonts` crate

A new shared crate in this repo, used by the desktop, Lightair Live and Lightair Editor.

| Part | Does |
|---|---|
| `Catalog` | Loads the shipped catalogue, applies a newer published one, searches it (name, category, script, popularity) |
| `FontRef` | What a project stores: `family`, `weight`, `style` (normal, italic), `axes` (tag → value), `source` (`bundled`, `catalog`, `imported { sha256, name }`) |
| `Resolver` | `FontRef` → the file and face that draws it; the nearest weight or axis value when the exact one isn't there |
| `Fallback` | Per-script chains: the requested family → Inter (Latin, Greek, Cyrillic) → the script's Noto family → Noto Color Emoji |
| `Cache` | Downloaded files and their licences, keyed by SHA-256; an index of what's present; never evicts a font a saved project uses, evicts others least-recently-used past a size limit (default 1 GB) |
| `Fetcher` (trait) | Downloads a URL to a file. The desktop implements it with its HTTP client (ureq); the mobile apps with the platform's (`URLSession`, OkHttp) so they inherit the system's proxy, VPN and background-transfer rules |
| `FontSet` | What `lightair-render` is given to draw with: the faces a document needs, loaded into a `fontdb::Database` |

**Cache locations.**

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/Lightair/Fonts` |
| Windows | `%LOCALAPPDATA%\Lightair\Fonts` |
| iOS | The App Group container shared by Lightair Live and Lightair Editor (`group.app.lightair.shared`), so both apps share one copy |
| Android | Each app's own files; Lightair Editor reads Lightair Live's cache through its signature-protected provider when both are installed |

**`lightair-render`** stops calling `FontSystem::new()` (which reads only the machine's fonts) and
draws with the `FontSet` its caller gives it. This is the change Lightair Editor already needs to
draw any text on iOS (its `docs/public-repo-changes.md`, change 4), and it makes the desktop's
own renders the same on every machine.

## 6. Missing fonts

| Where | While the font downloads | If it can't be downloaded |
|---|---|---|
| Editing (canvas, Editor preview) | The fallback draws; the layer or clip shows a small "Downloading font" badge; the frame redraws when it arrives | The badge becomes "Font unavailable · Choose another"; the project keeps the reference |
| Export | Waits for the download, with the reason shown in the export sheet | Asks before exporting with the fallback, naming the font |
| Going live (desktop, Lightair Live's Studio) | The Go Live review lists "Fonts downloading (2)" as a check | The check says which layers will use a fallback; going live isn't blocked |
| Opening a project | All missing fonts start downloading at once, smallest first | Listed in the project's missing-items view (docs/02 SH-12) |

## 7. The font picker

The same picker on every app (a popover on the desktop, a sheet on phones, a popover on tablets):

- Search by name; categories (Sans, Serif, Display, Handwriting, Mono); a script filter.
- **Used in this project**, **Favourites**, **Recent**, then the whole catalogue by popularity.
- Each family previewed in the layer's own text (or its name when there's none), with a download
  mark for families not yet on the device.
- Weight and style, and sliders for variable axes (weight, width, slant, and others the family
  has).
- **Import font…** (§3), and **About this font** (designer, licence, copyright).

## 8. Desktop specifics

- **Text layer.** libobs's `text_ft2_source_v2` chooses fonts by face and style name from the
  fonts the operating system knows, so downloaded fonts in Lightair's cache aren't visible to it.
  **Decision: replace it with a Lightair text source drawn by `lightair-render`** (cosmic-text,
  per-run styling), which LY-10's rich text needs anyway, with the same `FontSet` as every other
  app. Existing projects' text layers migrate once (face, style, size, colours, outline). The
  rejected alternative was registering cached fonts with the operating system, which is invisible
  to `text_ft2`'s own font scan on some platforms and installs fonts system-wide on others.
- **Widgets in CEF.** Widget pages get `@font-face` rules pointing at the cached files, served by
  the local widget server, so HTML widgets and native layers use the same files.
- **Timers** (`timers.rs`, today `text_ft2_source_v2` too) move with the text layer.

## 9. Phases

1. **The crate and the catalogue:**
   - `lightair-fonts` (catalogue, `FontRef`, resolver, fallback, cache, `Fetcher` trait);
   - `cargo xtask fonts-catalog` and the published catalogue;
   - the bundled set measured and packed; the emoji format checked;
   - `lightair-render` drawing with a caller's `FontSet`;
   - jsDelivr checked against the largest families.

   *Done when:* a test renders the same text in the same pixels from a bundled font, a downloaded
   font and an imported one, on macOS and on the iOS simulator; a catalogue built from a pinned
   `google/fonts` commit lists only `ofl`, `apache` and `ufl` families.
2. **Lightair Editor:** text clips and captions use the picker and the cache; projects store
   `FontRef`s; missing fonts behave as §6.

   *Done when:* a desktop Editor project with catalogue fonts opens on an iPhone, the fonts
   download, and its export matches the desktop's frame for frame.
3. **Lightair Live:** Studio's text, widgets and alerts; the Go Live check; the shared cache with
   the Editor.

   *Done when:* a scene made on the desktop with catalogue fonts goes live from a phone with the
   same fonts.
4. **Desktop:** the Lightair text source replaces `text_ft2` (with migration); timers; CEF
   widgets' `@font-face`; the picker in the Inspector; About › Licences.

   *Done when:* every desktop text surface draws from `lightair-fonts`, and an OBS-imported scene
   collection's text layers keep their look.

## 10. Decisions

Settled:

- **Google Fonts is the catalogue**, all three of its licences, original files only (§2).
- **About 40 bundled families plus Noto Color Emoji**, within 25 MB (§3).
- **The person's own font files can be imported**, with a licence note, and travel with the
  project (§3).
- **Files from Google's server first, jsDelivr second, both hash-checked; the catalogue published
  on GitHub Pages from this repo**; no server of our own (§4).
- **No system fonts in content**; Noto Color Emoji for emoji (§2).
- **Order:** the crate, then Lightair Editor, then Lightair Live, then the desktop (§9).
- **The desktop text layer becomes a Lightair source drawn by `lightair-render`** (§8).

To confirm while building:

- The bundled list against its measured size (phase 1).
- The emoji font's format (phase 1).
- Whether jsDelivr serves every family (phase 1); families it can't serve rely on Google's link.
- The cache's default size limit (1 GB).
