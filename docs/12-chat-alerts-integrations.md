# 12 · Chat, Alerts, Creator Tools & Control

(How it's built, platform by platform, and in what order: [plans/live-integrations](plans/live-integrations.md).)

Streams are conversations. Lightair brings the audience's side into the app natively: chat, events, alerts and widgets are built-in features rendered by Lightair, not browser sources pointed at third-party services. The same section covers how Lightair is controlled from outside: hotkeys, Stream Deck, MIDI, the remote API.

## Unified chat

### Chat panel

- One feed for **Twitch, YouTube, Kick, TikTok, X and Facebook**, each message with platform icon, badges, emotes (native + 7TV/BTTV/FFZ), username colour and timestamp.
- **Per-platform filters** and per-output visibility toggles.
- **Send** to all connected platforms or one (send-target menu beside the input), with emote picker and autocompletion of usernames and emotes.
- **Display options**: font size (S/M/L/XL), alternating backgrounds, compact mode, invert message direction, reverse layout, show/hide timestamps, show deleted messages greyed.
- **Split view**: chat and activity side by side or stacked; pop out to its own window.
- **Highlights**: mentions, first-time chatters, keywords, VIPs, and messages from mods stand out; hover a message to **Highlight on stream** (Chat Highlight widget) or **Pin**.
- **Custom chat dock**: replace the chat panel with any URL (bot dashboards) when a creator prefers another tool.

### Moderation

Where platform APIs allow: delete message, timeout (preset durations), ban/unban, mod/unmod, slow mode, followers-only, subscribers-only, emote-only, shield mode, and a **user card** (account age, follow date, message history in this session, notes). Moderation actions are logged in Activity.

### Chat on stream (Chat layer)

- Styles: bubbles, plain, boxed, minimal; fonts (system or Google Fonts), colours, platform badges on/off.
- Animations: slide in, fade in, pop; hide messages after N seconds.
- Filters: hide commands (`!…`), hide bots and specific accounts, minimum account age, keyword blocklist.
- Event messages inline (follows, subs, raids) with toggles per event type.
- Vertical-friendly presets for the vertical format.

## Activity & events

The **Activity** panel is one timeline of everything that happens during a show:

| Category | Events |
|---|---|
| Audience | Follows, subs (tier, months, message), gift subs, bits/cheers, Super Chats/Stickers, memberships, Kick subs/gifts, TikTok gifts, raids/hosts, channel point redemptions, polls/predictions results |
| Commerce | Merch orders (Fourthwall and similar), donations (Streamlabs/StreamElements tips, Ko-fi) |
| Show | Stream started/stopped, recording, clips saved, markers, scene changes by automation, issues |

Each event can be **replayed as an alert**, **highlighted on stream**, **thanked in chat** (template message), or marked done. Filters by type and platform. The Activity feed is also available as a layer (Activity, Recent Activity).

## Alerts

Native alert engine (no third-party browser source required):

- **Alert types** per event (follow, sub, resub, gift, bits tiers, Super Chat, raid, donation, merch, redemption, custom API event).
- **Variations with conditions**: e.g. bits ≥ 1000 gets a different animation; first-time follower; gift ≥ 10.
- **Content**: image/video/animation (WebM with alpha, Lottie), sound, text template with variables (`{name}`, `{amount}`, `{months}`, `{message}`), text-to-speech (on-device voices, with profanity filter and length limit), duration.
- **Layout**: position and animation in/out, per format.
- **Queue**: alerts never overlap; queue length shown in Activity with skip/pause controls; a "pause alerts" hotkey for serious moments.
- **Test**: send any test event from the Inspector or Activity.

## Widgets

All widgets are Lightair layers (native rendering where possible, Lightair-maintained HTML for richly animated ones), styled with the design system and Google/system fonts, configurable per format, and controllable by hotkey/API.

| Widget | Behaviour |
|---|---|
| **Activity / Recent activity** | Live feed or the last N events, filtered |
| **Combo** | Builds up animation as repeated events arrive |
| **Spotlight** | Carousel of recent supporters |
| **Goals / progress bar** | Followers, subs, bits, donations, custom counter; start/target, reset rules |
| **Wheel spin** | Segments, weights, spin on command/redemption, result to chat |
| **Social handles** | Rotating usernames across platforms |
| **Counter** | Death counter, wins, custom |
| **Chat highlight** | Featured message card |
| **Subathon timer** | Adds time per event type; pause; add time via API/hotkey; caps |
| **Countdown / stopwatch** | See layers in [07](07-scenes-layers-canvas.md) |
| **Confetti** | Fall and pop, emoji confetti, triggered by events/hotkeys |
| **Polls** | Native polls (platform polls where supported, chat-vote otherwise) with live results |
| **Captions** | Live captions from on-device transcription |

Widget configurations export/import as files to share styles.

## Captions

- **On-device speech-to-text** (Whisper-class model, Core ML / DirectML / CPU) on a chosen mic track, multiple languages, with a 30-second test.
- Feeds the **Captions** widget (style, position, max lines, per format), **platform captions** where supported (YouTube CEA-608/708 via the stream), the **recording sidecar** (SRT/VTT export), and **voice-triggered actions** (say "clip that" to save a clip, if enabled).
- Everything is processed locally; no audio leaves the machine.

## Chat commands & bot

Built-in, optional chat bot behaviour using the creator's account or a bot account:

- Built-in commands: `!uptime`, `!socials`, `!scene` (mods), `!clip` (mods/VIPs), `!song` (if a music source is linked).
- **Custom commands** with text responses, variables, cooldowns and permission levels.
- **Command-triggered actions**: any Lightair command (switch scene, show layer, play media, spin wheel) can be bound to a chat command or channel point redemption.
- Timers: periodic messages with minimum chat activity.

## Creator tools

### Overlay builder (AI)

Describe what you want ("a neon lower third with my name and a follower goal bar"); Lightair generates an **editable overlay layer** (HTML/CSS/JS in a sandboxed browser layer) and places it on the canvas.

- Iterate in a side panel with prompts, restore points, and reference images.
- **Test with sample events** (chat, follow, sub, raid, gift, purchase, transcript).
- Generated overlays react to live events through Lightair's event bridge.
- **Bring your own model**: the user connects a provider of their choice (cloud API key or a local model runtime). The feature is off until a provider is configured; the panel states clearly which provider receives the prompt and the reference images.
- Generated overlays are saved into the user library and can be shared as packs.

### Overlay gallery

A built-in gallery of starter overlays and elements (Starting Soon, BRB, alerts, chat widgets, lower thirds, seasonal), each customisable by editing parameters or with the overlay builder.

### LUT tools

Create a LUT from a colour adjustment stack, convert between formats (`.cube`, `.png` strip), preview on any layer, and save to the library.

### Screen draw sessions

Share a link so guests can draw on a Screen Draw layer (moderated: approve guests, clear, lock). Uses a direct WebRTC connection (no Lightair server); no account needed for guests.

## Control

### Hotkeys

- Every command can be bound. The **Hotkeys** settings page lists commands by category with search, enable checkbox, in-app and **global** bindings, conflict detection, and restore defaults.
- Global hotkeys require Input Monitoring (macOS) or work natively (Windows); Lightair explains the permission when the first global hotkey is created.
- Defaults include: screenshot (F2), group (⌘G), ungroup (⌘⇧G), undo/redo, scene 1–9, take (Space in studio mode), clip, marker, mute mic, privacy cut.

### Stream Deck

Official Elgato Stream Deck plugin with dynamic key images (live scene thumbnails, mute state, timers):

| Action | Notes |
|---|---|
| Switch scene / Stage scene / Take | Studio mode aware |
| Mute, cue, volume (dial with level ring on Stream Deck +) | Per track |
| Layer visibility, effect on/off | Per layer, per format |
| Go live, record, pause, clip, marker, screenshot | |
| Replay controls, media controls | Play, pause, seek, restart, next |
| Widget controls | Timer start/stop/add, counter ±, wheel spin, confetti |
| Lightair Effect Stack | Apply look (face thumbnail), background blur toggle, intensity dial |
| Run macro | |
| Privacy cut, mute all | |

Integrations for Loupedeck, Touch Portal and other controllers use the remote API.

### MIDI & OSC

- Learn mode: click a control in Lightair, move a knob/press a pad, done.
- Faders and knobs map to volumes and any numeric parameter with range and curve; buttons map to commands; **motorised fader and LED feedback** for controllers that support it.
- OSC server for lighting desks and show-control software.

### Macros

Sequences of commands with waits, conditions and variables ("Go to BRB: switch scene, mute mic, start 5-minute countdown, pause alerts"). Assign to hotkeys, Stream Deck, chat commands or automations.

### Remote API

- **Lightair protocol**: JSON-RPC 2.0 over WebSocket. Requests mirror Lightair commands (every action in the app); events mirror runtime and document changes; telemetry subscriptions (audio levels, stats) with rate control. Schemas published for typed client generation.
- **obs-websocket v5 compatibility**: the commonly used subset (scenes, inputs, filters, transitions, studio mode, outputs, recording, streaming, replay buffer, media inputs, hotkeys by name), so existing OBS tools keep working.
- **Security**: off by default; localhost only by default; LAN access opt-in; password or per-client tokens with scopes (read-only, control, admin); connections listed in Settings with revoke.
- **Third-party tools** documented: Streamer.bot, Lumia Stream, Mix It Up, Touch Portal, bitfocus Companion.

### Mobile remote

A companion app (and a LAN web remote) shows scenes with thumbnails, a compact mixer, chat, stats and start/stop controls. Pairs by QR code over the remote API with a scoped token. The same companion app provides the **Phone camera** layer ([07](07-scenes-layers-canvas.md#video)).

### Hardware & app integrations

| Integration | What it does |
|---|---|
| BEACN, Elgato Wave Link, GoXLR | Expose mixer channels and mixes as Lightair tracks with level/mute sync |
| Fourthwall and similar stores | Merch order events |
| Streamlabs/StreamElements tips, Ko-fi | Donation events |
| Discord | Rich presence; optional "going live" announcement webhook |
| Spotify / Apple Music (now playing) | Now-playing text layer and `!song` |
