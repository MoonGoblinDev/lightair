# What Lightair Live and Lightair Editor need from the desktop and lightair-core

Every desktop and lightair-core item in lightair-live's `docs/12-roadmap.md` (phases 0–4),
`docs/05-protocols.md`, the "Desktop" column of `docs/02-features.md`, and lightair-editor's
`docs/public-repo-changes.md`, each marked done or not. The phone halves are the mobile repos'
own. Items the roadmap puts after phase 4 (looks MR-19, the relay MR-23, the camera over the
internet MC-17, remote control of a phone's Studio MS-26) are listed last and not counted.

✅ done · ⬜ not yet. Where a choice wasn't in the spec, what was chosen is said on the line.

## Phase 0 · Groundwork

1. ✅ `native-tls` → `rustls` in the shared crates (lightair-core `55201b9`)
2. ✅ `lightair-platform` backends by trait (lightair-core `e24f5d4`)
3. ✅ Licence check job in lightair-core (GitHub Actions: cargo deny, no path to libobs/cef/gpui, commit authors)
4. ✅ `chat.send` documented in `docs/remote-api.md`

## Phase 1 · Remote (05 §1–5, §9–10)

5. ✅ TLS port 4456, pinned self-signed identity in the Keychain
6. ✅ `pair.claim`, the consent alert, device keys, `Lightair-Key` sign-in, scopes (control, watch, camera)
7. ✅ `pair.forget`
8. ✅ `pair.list`, `pair.remove` (a control device managing the paired devices)
9. ✅ Pairing by typed code: six words for the secret, the fingerprint shown as four words (05 §2, "Without a camera"); the words are made (consonant-vowel-consonant-vowel, 11 bits each), not a list (lightair-core)
10. ✅ Settings › Remote API › Phones: pair, list, remove
11. ✅ "Not seen for 6 months" on a device unseen for 180 days
12. ✅ `pair.claim` limits: 5 a minute per address, then 15 minutes' lockout
13. ✅ At most 8 paired devices connected at once (05 §10)
14. ✅ mDNS advertisement
15. ✅ `session.hello` with protocol 2 and features
16. ✅ Bounded event queues per client (1,000), meters and health dropped first, `session.resync` (05 §5)
17. ✅ Program preview stream (JPEG, ack pacing)
18. ✅ `scene.thumbnails`
19. ✅ Event `scene.thumbnail` (a scene's picture changed; one per scene per 5 s)
20. ✅ `staging.get` / `enable` / `stage` / `take`, event `staging.state`
21. ✅ `transition` in `staging.get` and on `staging.take`
22. ✅ `outputs.list`, event `output.health`
23. ✅ `started_at` (and `elapsed_ms`) on `stream.state` / `record.state`
24. ✅ `clip.saved` with `id`, `duration_ms`, `formats`, `thumbnail` (MR-08)
25. ✅ `audio.levels` (opt-in, 20 Hz)
26. ✅ `panic.*`, event `panic.state`
27. ✅ `deck.get` / `deck.set`, event `deck.changed`
28. ✅ `chat.history`, `chat.send`, events `chat.message`, `chat.removed`
29. ✅ `chat.moderate` (delete, timeout, ban, as the Chat panel does them; `unban` answers that no platform's path offers it yet; timeouts 600 s unless given)
30. ✅ `activity.history`, event `activity.item`
31. ✅ `activity.replay` (the Alerts widget's replay)
32. ✅ `stream.info.get` / `set`
33. ✅ `stream.categories` (Twitch and Kick by their own search, YouTube's fixed list; lightair-core's `stream_info::categories`; watch scope may call it)
34. ✅ `issues.list`, events `issue.raised` / `issue.cleared`
35. ✅ Fixes for issues (`fixes` offered, `issues.fix` runs them): a failed destination offers try again and stop, a reconnecting one stop; other issues offer none (they need the computer)
36. ✅ Event `source.tally` (MC-10)

## Phase 2 · Camera (05 §6)

37. ✅ Phone camera source kind and its Inspector, pairing from the layer (desktop `405ca28`)
38. ✅ SRT listener, per-layer passphrase (desktop `405ca28`, `…`)
39. ✅ Control channel: configure, control, tally, keyframe, media; state and health shown
40. ✅ The phone's mic as the layer's track; rotation applied; placeholder within a second, recovery by itself
41. ✅ USB: usbmuxd (iPhone) and AOA 2 (Android) (desktop `acac816`, `1455ae1`); tried on an iPhone 15, AOA not yet on a real phone
42. ✅ Decoding straight through VideoToolbox / AudioToolbox on macOS (desktop `e8bc5a2`)
43. ⬜ Decoding straight through Media Foundation on Windows (today: libobs's media source over UDP, the slow path)
44. ✅ Latency measured per frame, camera to screen, shown in the Inspector (desktop `e8bc5a2`, `01a237a`; lightair-core `ce1481d`)
45. ✅ "Lowest latency (needs good light)": the exposure cap switch (`configure.low_latency`; `state.low_latency` says whether the phone took it; the phone half is Live's)

## Phase 3 · Lightair Editor (05 §4.2, §7; Editor docs §5, §10)

46. ✅ lightair-render for the Editor: the `Codec` trait, the compositor on a caller's device with texture inputs, adapter limits, fonts, `add_parts`, the mix in Rust, proxies named by content (lightair-core, see lightair-editor's `public-repo-changes.md`)
47. ✅ `Call.device`, transfers on the TLS port: ranged GET, resumable PUT, HEAD, tokens (lightair-core `68ac576`, `a49496c`)
48. ✅ `takes.*`, `clips.*`, `edits.*`, `transfer.upload` / `status` (desktop `9b140ea`, `fc3a0c3`)
49. ✅ The Library's "Being edited on …" (desktop `4d14da7`)
50. ⬜ A sequence's background fill behind fitted pictures: colour, blur, backdrop (ED-18)
51. ⬜ The first clip effect missing in the player-layer path until the playhead moves

## Phase 4 · Studio

52. ⬜ `lightair-render`'s scene renderer: a whole `lightair-model` scene per frame, shared with the desktop Editor and its golden-frame tests
53. ✅ Project transfer (MS-01): `project.list`, `project.export` (a portable package: assets collected, secrets out; lightair-store `portable`). The package is the workspace's files plus `assets/`, sent file by file over the transfer, not zipped (the phone needs no unzip); a phone opens it, marking layers it can't show (its half)

## After phase 4 (designed, not scheduled; not counted)

Looks `look.list` / `look.set` (MR-19); the relay (MR-23); the camera over the internet (MC-17);
remote control of a phone's Studio (MS-26).

## Count

49 of 53 done.
