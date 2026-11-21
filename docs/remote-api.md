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
| `chat.send` | `text`, `platform?` (every connected chat when absent) | sends a message to the stream's chat as the signed-in account |
| `commands.execute` | `commands` (Lightair commands, as in the journal), `label?` | |
| `ui.select` | `scene?`, `object?` | selects it in the window |
| `camera.pair` | `source` (a Phone camera layer) | `link` (the pairing code's link, for Lightair Live), `expires_at`; the person still allows the phone in Lightair |
| `camera.status` | `source?` | `cameras: [{source, connected, phone, picture, usb, rotation, received_kbps, latency: {arrived, decoded, drawn: {median_ms, min_ms, max_ms, frames}, clock_offset_us}, state, health, tally}]`, `srt_port` (latency: each stage from the frame's capture on the phone) |
| `camera.control` | `source`, `control` (`zoom`, `torch`, `focus`, `exposure`, `white_balance`, as lightair-live's docs/05 §6) | sent to the layer's phone |
| `takes.list` / `clips.list` | `since?` (a `cursor` from before), `kind?` (take, recording, export) | `items` (or `clips`): `{id, kind, name, created, modified (what \`since\` compares), duration_ms, formats, width, height, fps (a take's screen), has_camera, size_bytes, proxy_bytes, revision, has_proxies, thumbnail {mime, width, height, data} (≤ 320 px wide), markers, missing, checked_out}`, `cursor` |
| `takes.fetch` / `clips.fetch` | `take` (or `clip`), `files?`, `proxies_only?` | `transfer {token, manifest: [{name, size, sha256, path}]}` (the same names and SHA-256 each time, so a new token resumes the same partial files), `revision`: a take's folder's files (its name kept: `….lightair-take/screen.mp4`) and the proxies the desktop made (`proxy-<content>.mp4`) |
| `takes.put_edit` | `take`, `edit` (its `edit.json`), `base_revision` | `revision`, or `{conflict: true, revision, copy}` (the phone's kept beside the take's as `edit (edited on …).json`) |
| `takes.checkout` / `takes.checkin` | `item`, `device_name?` | "Being edited on …" for 7 days without word (`checkouts.json`) |
| `edits.list` | `since?` | `projects: [{id, name, created, modified, revision, size, aspect, fps, duration_ms, media, thumbnail, checked_out}]`, `cursor` |
| `edits.fetch` | `project`, `files?`, `proxies_only?` | `transfer` of `project.json` and its media (`media/<n>-<file>`; a take's whole folder as `media/<n>-<folder>/<file>`; its stems and look), each with `path` (as the document has it) and `medium` (its id), proxies with `proxy_for`; `revision` |
| `edits.put` | `project` (the document), `base_revision?` (none: new here) | `revision`, or `{conflict: true, revision, copy}` (a copy "… (edited on …)") |
| `transfer.upload` | `files: [{name, size, sha256}]`, `kind` (export, project), `project?` | `transfer`; when it's all here, an export is in the Library (`from` the project) and a project in `edits/` with its media in `edits/media/<id>/` (`media/…` paths pointed there); event `transfer.done {token, kind, item}` |
| `transfer.status` | `token` | `{done, kind, files: [{name, size, received}], item?}` |
| `auth.login` | `password` | (when not sent as `Authorization: Bearer`) |
| `events.subscribe` | `events` (names; absent: all) | |

The `takes`, `clips`, `edits` and `transfer` methods are for Lightair Editor on a paired phone or tablet (the phones' TLS port, lightair-live's docs/05 §4.2 and §7): their files move as `GET`/`PUT`/`HEAD https://host:4456/transfer/<token>/<name>`, ranged and resumable, the token (128 bits, 24 hours, for that device while it's paired) the only sign-in; a piece of an upload goes at the offset received so far (`Lightair-Received`, also on a 416), and the last is checked against its SHA-256; an empty file is an empty body without `Content-Range`. Tokens aren't logged. A revision is the start of the stored document's SHA-256; a document sent from an older revision isn't merged but kept as a copy.

Events arrive as `{"method": "event", "params": {"name", "data"}}`: `scene.program`, `scene.list`, `record.state`, `stream.state`, `vcam.state`, `clip.saved`, `audio.mute`, `audio.volume`, `object.visible`, `workspace.changed`, `source.tally` (a Phone camera layer's `source`, `program`, `staged`, `live`, on change).

## obs-websocket

Speaks protocol 5 (JSON, authentication, batches, event subscriptions). In OBS's terms a scene is a scene, an object a scene item (its id a number made from the object's), a source an input, and a track its source's volume and mute. `GetVersion` lists the requests answered; anything else answers 204. Studio mode requests answer 506.

## MCP tools

`get_state`, `screenshot`, `switch_scene`, `create_scene`, `rename_scene`, `remove_scene`, `add_object`, `update_object`, `remove_object`, `move_object`, `get_source`, `list_capture_targets`, `set_audio`, `recording`, `streaming`, `virtual_camera`, `save_clip`, `undo`, `redo`, `get_stats`, `get_workspace`, `run_commands`: each one of the methods above.

## For phones (Lightair Live), on the TLS port

The phones' methods and events are lightair-live's docs/05 §4.2; as built on the desktop:

- `staging.get` (and the `staging.state` event) carry `transition: {kind, duration_ms}`, the one a take would use (the staged scene's own, else the workspace's); `staging.take {transition?: {kind, duration_ms?}}` uses the given one for that take only (kind: cut, fade, move, slide, wipe, dip, stinger).
- `record.state` carries `started_at` (Unix ms) and `elapsed_ms` (paused time left out); `stream.state` carries `started_at` and `destinations` (the ids streaming).
- `clip.saved` carries `path` and, once the Library has the clip (up to 4 s later), its `id`, `duration_ms`, `formats` and `thumbnail {mime, data}` (a 240 px JPEG).
- `scene.thumbnail {scene, version}`: a scene's picture may have changed (the scene on air every 5 s, others when their objects change; one per scene per 5 s at most); phones fetch `scene.thumbnails` for those they show.

- `chat.moderate {message, action: delete|timeout|ban|unban, seconds?, reason?}` → `{done: true}` (`duration_s` is taken for `seconds` too): `message` is the chat message's `id` (or its platform id). Done as the account signed in to that platform in Lightair, the same path as the Chat panel's buttons (Twitch and Kick through their own signed-in pages; YouTube where signed in officially). Timeouts last `seconds` (600 by default, at most two weeks). `unban` answers a state error: no platform's path here offers it yet. Answered on the server's thread.
- `activity.replay {item}`: plays the activity item's alert again (the Alerts widget's replay), by `id` or platform id.
- `stream.categories {platform, query}` → `[{id, name, image?}]`, at most 20: Twitch's and Kick's own category search (Kick's through a kick.com page), YouTube's fixed list filtered by name. A watch-scope device may call it.
- `issues.list` items carry `fixes: [{id, label}]` where there's one to run: a destination that failed offers `retry` (start that destination again) and `stop`; one reconnecting offers `stop`. `issues.fix {issue, fix}` runs it, as the button would.
- `session.hello`'s features add `chat.moderate`, `activity.replay`, `stream.categories` and `issues.fix`.
- `project.list` → `{projects: [{id, name, modified, scenes, formats: [{id, name, width, height}], open}]}`: the workspaces on this computer (the Workspaces folder and the open one's), `modified` in Unix ms, `scenes` a count.
- `project.export {project}` (a paired device) → `{transfer, name, assets, secrets_removed}`: a portable copy of that workspace, made by lightair-store's `portable::export`: as it is now (the journal replayed into it), the workspace files (`workspace.json`, `sources.json`, `audio.json`, `mixer.json`, `outputs.json`, `scenes/<id>.json`) and every file a setting pointed at, collected as `assets/<file>` with the setting now `assets/<file>`. Settings named like secrets (`password`, `token`, `secret`, `stream_key`, `key`, …) and such parameters in web addresses are left out; stream keys are never in workspace files anyway. The transfer is fetched as any other (lightair-live's docs/05 §7); the copy stays in the cache for a day. Feature `project.export`.
- The preview stream's `start` may name a `canvas` (an id from `state.get`'s canvases): that canvas's program is sent instead of the primary one's (MR-05). An unknown id, or none, is the primary canvas.
- `takes.fetch` (and `clips.fetch`): a proxy's manifest entry also carries `proxy_for` (the in-folder name of the video it stands in for, `….lightair-take/screen.mp4`) and `proxy_for_path` (that video's path here), as `edits.fetch`'s do.
- `transfer.upload {kind: "media", project, files}` (ED-21): footage added on a device to a project that came from here. The files land in `edits/media/<project>/` (their `media/` prefix dropped) and `transfer.done` says `{project, files}`. Then `edits.put` with the document naming them `media/<name>` (a medium's file, take folder, look or audio stems): before its revision check, `edits.put` points every `media/…` name at that folder, as kind "project" does.
- **For developers:** a debug build started with `LIGHTAIR_DEV_PAIRING=1` keeps the phones' port open (with the Remote API on), writes a fresh control-scope pairing link to `<workspace>/dev-pairing-link.txt` (and stdout, `LIGHTAIR_DEV_PAIRING <link>`) whenever none is open, and lets control and watch devices on this computer itself (a simulator: loopback or this computer's own address) pair without the alert. It reads nothing from the Keychain (an unsigned debug build can't without asking): the API password is `LIGHTAIR_REMOTE_PASSWORD` or one made for that run, and the phones' TLS identity is kept in `Phones/dev-identity.txt`. Beside the installed app, move its ports: `LIGHTAIR_REMOTE_PORT`, `LIGHTAIR_PHONES_PORT`, `LIGHTAIR_PHONE_SRT_PORT`. Release builds never do any of this; cameras always ask.
