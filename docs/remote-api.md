# Remote API

Lightair runs one local server (Settings › Remote API) that other apps and AI agents use to control it. Every change they make is an ordinary command: it can be undone, and Undo names who made it ("MCP: Add Text").

- **On by default, for this Mac only**, on port **4455** (where OBS tools look). Turn on *Other devices on your network* to let phones, tablets and other computers connect.
- **Password**: generated on first use and kept in the Keychain (*Lightair remote API*). It's always asked for when other devices may connect; for this Mac only it can be turned off.
- Requests whose `Host` isn't a local name are refused unless LAN access is on, so web pages can't reach the server through DNS rebinding.

| Way in | Address | For |
|---|---|---|
| obs-websocket v5 | `ws://host:4455/` | Stream Deck, Touch Portal, bots and other OBS tools, unchanged |
| Lightair's Stream Deck plugin | JSON-RPC, below | Keys lit with what's on ([streamdeck/](../streamdeck/README.md)); Settings › Remote API › Stream Deck installs it |
| JSON-RPC 2.0 | `ws://host:4455/rpc` | Scripts and apps written for Lightair |
| MCP (Streamable HTTP) | `http://host:4455/mcp` | AI agents (`Authorization: Bearer <password>`) |
| MCP (stdio) | `…/Lightair.app/Contents/MacOS/lightair mcp` | Agents that start tools as programs; reads the password from the Keychain, opens Lightair on the first tool call if it's closed |
| Command line | `…/Lightair.app/Contents/MacOS/lightair call <method> [params]` | One JSON-RPC call from a script (`lightair call scene.switch '{"scene": "Main"}'`), the password read from the Keychain; prints the result as JSON, or the error on stderr (exit 1) |

Connecting Claude Code:

```sh
claude mcp add lightair -- "/Applications/Lightair.app/Contents/MacOS/lightair" mcp
```

## Methods (JSON-RPC)

Scenes, objects, sources and tracks are named by id or by name. Positions are canvas pixels in the primary canvas unless `canvas` is given.

| Method | Parameters | Answer |
|---|---|---|
| `app.version` | | `name`, `version`, `workspace` |
| `state.get` | | program, canvases, scenes with their objects (back to front), sources, tracks, outputs |
| `workspace.get` | | the whole workspace document |
| `stats.get` | | `fps`, `frame_time_ms`, `lagged_frames`, `cpu_percent`, `memory_mb`, `disk_free_mb`, … |
| `scene.switch` | `scene` | puts it on air |
| `scene.create` | `name`, `switch?` | `id` |
| `scene.rename` / `scene.remove` | `scene`, `name` | |
| `object.add` | `kind` (text, image, video, color, browser, display, window, app, camera, phone_camera, timer, microphone, or a libobs id) or `source`; `scene?`, `name?`, `settings?`, `x? y? width? height?` | `object`, `source`, `track` |
| `object.update` | `object`, `scene?`, `name? visible? locked? x? y? width? height? rotation? opacity?`, `settings?` (merged into its source's) | the object |
| `object.remove` | `object`, `scene?` | |
| `object.move` | `object`, `position` (front, back, forward, backward) or `to` | |
| `source.get` / `source.update` / `source.remove` | `source`, `name?`, `settings?`, `replace?` | |
| `audio.set` | `track` (or its source), `volume_db?`, `muted?` | |
| `audio.add` | `kind` (microphone, desktop, app), `device?`, `name?` | |
| `capture.targets` | `kind` (display, window, app, camera, microphone) | `targets`: title and the settings to use |
| `record.start` `stop` `toggle` `pause` `resume` `marker` `split` `status` | | |
| `stream.start` `stop` `toggle` `status` | | |
| `vcam.start` `stop` `toggle` `status` | | |
| `clip.save`, `clip.status` | | |
| `history.undo`, `history.redo` | | |
| `screenshot` | `scene?`, `width?` | `mime`, `data` (base64 PNG) |
| `commands.execute` | `commands` (Lightair commands, as in the journal), `label?` | |
| `ui.select` | `scene?`, `object?` | selects it in the window |
| `camera.pair` | `source` (a Phone camera layer) | `link` (the pairing code's link, for Lightair Live), `expires_at`; the person still allows the phone in Lightair |
| `camera.status` | `source?` | `cameras: [{source, connected, phone, picture, rotation, received_kbps, state, health, tally}]`, `srt_port` |
| `camera.control` | `source`, `control` (`zoom`, `torch`, `focus`, `exposure`, `white_balance`, as lightair-live's docs/05 §6) | sent to the layer's phone |
| `auth.login` | `password` | (when not sent as `Authorization: Bearer`) |
| `events.subscribe` | `events` (names; absent: all) | |

Events arrive as `{"method": "event", "params": {"name", "data"}}`: `scene.program`, `scene.list`, `record.state`, `stream.state`, `vcam.state`, `clip.saved`, `audio.mute`, `audio.volume`, `object.visible`, `workspace.changed`, `source.tally` (a Phone camera layer's `source`, `program`, `staged`, `live`, on change).

## obs-websocket

Speaks protocol 5 (JSON, authentication, batches, event subscriptions). In OBS's terms a scene is a scene, an object a scene item (its id a number made from the object's), a source an input, and a track its source's volume and mute. `GetVersion` lists the requests answered; anything else answers 204. Studio mode requests answer 506.

## MCP tools

`get_state`, `screenshot`, `switch_scene`, `create_scene`, `rename_scene`, `remove_scene`, `add_object`, `update_object`, `remove_object`, `move_object`, `get_source`, `list_capture_targets`, `set_audio`, `recording`, `streaming`, `virtual_camera`, `save_clip`, `undo`, `redo`, `get_stats`, `get_workspace`, `run_commands`: each one of the methods above.
