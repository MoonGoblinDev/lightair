# Live integrations: accounts, chat, widgets and automation in one app

Status (16 November 2026), built:
- **Chat in**: Twitch (and its follows), YouTube, Kick (and Kicks gifts), TikTok (comments,
  gifts with combos, follows, shares, from the room's own connection), CHZZK (and
  subscriptions), SOOP, DouYu, TwitCasting, SHOWROOM, Bilibili; tips from Streamlabs,
  StreamElements and Ko-fi (its own stream alerts page, opened hidden: no server); history kept
  across restarts.
- **Chat out**: sign in on each platform's own page in a Lightair window (Outputs ›
  Sending); Twitch, YouTube, Kick and TikTok send as you; the Chat panel's send box.
- **Widgets** (crates/lightair-widgets): alert box, chat box, event list, goal, label,
  counter, subathon timer, now playing, emote wall, credits, social rotator, poll, wheel,
  leaderboard, heart rate; StreamElements widgets unchanged; `.lwidget` packages.
- **Automations** (crates/lightair-automation): triggers, conditions, steps (every Lightair
  method, chat, voice, sound, variables, points, branches, JavaScript, web requests, VTube
  Studio, Elgato Key Lights), queues, cooldowns, viewers and points, run history, recipes,
  remote API and MCP.
- **Integrations**: Spotify / Apple Music now playing, VTube Studio, Key Lights, Pulsoid;
  Stream Deck through obs-websocket compatibility.

Left: Facebook, Instagram, X and LinkedIn (signed-in pages: need a signed-in session to
build against), Huya and Niconico readers, SOOP subscriptions and YouTube raids, reading
rules updated without a release, Philips Hue, a Stream Deck plugin of Lightair's own,
Streamer.bot import, bring-your-own official API credentials. It turns
[docs/12](../12-chat-alerts-integrations.md) into a design and goes further: everything a
streamer runs next to OBS today (a chat app, StreamElements or Streamlabs widgets, Streamer.bot,
Speaker.bot, Restream chat) is part of Lightair, so nobody needs a second app. Lightair doesn't
depend on, connect to or require any of them; it can import what people already made for them.
Every part names its macOS and Windows path (docs/plans/windows.md).

**Rule: nothing waits on a platform's approval.** Where an official API needs a review, an
audit, a partner deal or an enterprise contract (YouTube's quota audit, Meta's App Review, X's
gated access, TikTok's partner API, LinkedIn's certification, Bilibili's open platform), Lightair
uses the route that works today without it (section 2.10): the platform's own web pages in a
hidden view logged in as the user, or the endpoints its web client uses. Official APIs are used
where they're self-serve (Twitch, TwitCasting) and offered as an option for people who bring
their own developer credentials.

## 1. What the research found

### Platforms (November 2026)

| Platform | Log in | Stream key / setup by API | Chat in | Chat out | Events | Server needed |
|---|---|---|---|---|---|---|
| **Twitch** | Device code flow, public client (no secret) | Yes (`channel:read:stream_key`, title, category, tags) | EventSub WebSocket `channel.chat.message` | Helix Send Chat Message | Everything: follows, subs, gifts, bits, raids, points, polls, predictions, Hype Train, Power-ups, shared chat | No |
| **YouTube** | Loopback redirect + PKCE (installed app) | Yes: `liveStreams` gives ingest URL and key; `liveBroadcasts` create, bind, go live, category (Aug 2026) | `liveChatMessages.streamList` (gRPC stream, 5 units a call) | `insert`, 20 units | In the chat stream: Super Chat/Sticker, members, milestones, gifts, Jewels, polls (no follows) | No |
| **Kick** | OAuth 2.1 + PKCE, but the code exchange needs the client secret | Yes (`streamkey:read`, `channel:write`) | Webhooks only (a WebSocket is "backlog"); the unofficial Pusher socket is what everyone uses | `chat:write` | Webhooks: follows, subs, gifts, Kicks, rewards, bans | **Yes** (token exchange, webhooks) |
| **TikTok LIVE** | None official (Streamlabs has a partner deal) | Partner only; eligible accounts paste a key | Unofficial webcast socket (TikTokLive, Euler Stream signing) | Unofficial only (session cookie: no) | Gifts, likes, follows, shares (unofficial) | No (a paid signer) |
| **Facebook** | Login for Devices (device code) or web OAuth; App Review | Yes (`/live_videos`) | Server-sent events `live_comments` | Graph comment | Reactions | No |
| **X** | Enterprise, gated form | Yes (`/2/sources`, `/2/broadcasts`; old routes end 1 Jan 2027) | Activity API | Yes | Few | Uncertain |
| **Instagram** | Meta OAuth, professional account | **No** | `live_comments` webhook | Private replies only | None | Yes |
| **LinkedIn** | OAuth, certified partners | Ingest URLs | Not documented | No | None | No |
| **Rumble** | The user pastes their Live Stream API URL | No | Polling JSON | No | Rants, follows | No |
| **Trovo** | Shut down 30 June 2026: dropped | | | | | |

What follows from it:

- **Twitch works entirely from the desktop through its official API** with no approval (a
  registered app, a public client): fully native.
- **YouTube's official API** needs Google's quota audit and OAuth verification to ship (10,000
  units a day shared by every user), so it isn't the route: chat is read from the feed youtube.com's
  own chat page uses, sent from a hidden page logged in as the user, and streams go to the
  channel's reusable stream key (2.10).
- **Kick**: read through its chat socket (what every tool uses), sent through a hidden page; its
  official API (self-serve, but it needs a client secret at the token exchange) is an option.
- **TikTok, Facebook, Instagram, X, LinkedIn**: hidden logged-in pages (2.10).

### Context

- **Twitch's merged-chat rule**: its terms (12 Aug 2026) still forbid showing other platforms'
  combined chat on a Twitch stream, though Twitch said it would stop enforcing it. Lightair's chat
  overlay is per output: the Twitch output gets Twitch chat by default.
- **StreamElements** went to Razer (July 2026) after layoffs; its payments end 31 Dec 2026.
  Creators have reason to want widgets that live on their own Mac, not in a cloud account.
- **Streamer.bot** (1.0.7 stable, 1.1 alpha) is Windows-only (.NET Framework; Mac users need an
  unmaintained Wine wrapper), powerful but hard to learn, and anything beyond the basics is C#
  running with full trust. Its 1.1 adds points, cron, voice control (Whisper) and queue controls.
- **Social Stream Ninja** covers 100+ sites by reading the chat from each site's web page in a
  browser, with APIs where they exist: the widest coverage, the most breakage.

## 2. The parts

```text
                 ┌──────────── Lightair ─────────────────────────────────────────────┐
 Twitch ─WS────▶ │ Accounts ─▶ Connectors ─▶ Event bus ─▶ Chat hub ─▶ Chat panel     │
 YouTube ─gRPC─▶ │ (Keychain)  (one per      (typed,       (history,    Activity panel │
 Facebook ─SSE─▶ │             platform)     JSON Schema)   emotes)      Widgets ──────┼─▶ canvas,
 Kick ──┐        │                               │                                    │   OBS URLs
 Instagram ─┐    │                               ├─▶ Automation (triggers → actions)  │
            │    │   Lightair's own events ──────┘      queues, variables, scripts   │
            ▼    │   (stats, audio, outputs, scenes)    every app command as a step  │
   (no server: everything runs on this computer) └──────────────────────────────────────┘
```

### 2.1 Accounts

- **Connect an account** from Settings › Accounts and from the Outputs panel (a destination
  offers "Log in" next to "Paste a key"). Several accounts per platform; each is a **broadcaster**
  or a **bot** account (the bot sends chat and replies; Twitch's bot badge needs the broadcaster to
  grant `channel:bot`).
- Flows: Twitch device code (a code and a link; no secret in the app); every other platform
  through its own sign-in page in Lightair's browser (a window the user sees, their password
  typed into the platform's page, never into Lightair), whose session the hidden pages then use
  (2.10). Optional: a user's own developer credentials for an official API (a Google Cloud
  project, a Meta app in development mode, a Kick app), used for their own account only.
- Tokens in the Keychain (Windows: Credential Manager), refreshed in the background, never in the
  workspace, never given to scripts or widgets (they call functions that use them).
- Scopes asked per feature, with a plain list ("read chat, send chat as you, see subs…"); adding a
  feature later asks again.

### 2.2 Going live without keys

- A logged-in destination fills its server and key itself where the platform shows them to the
  user: through Twitch's API, and from the platform's own key page in the logged-in session
  elsewhere (YouTube Studio's reusable key, Kick's dashboard, Facebook's persistent key; Instagram,
  TikTok and Niconico make a new key for every stream, read the same way when going live).
  Pasting stays possible everywhere.
- **Stream info** per destination in the Outputs panel and the Go live sheet: title, category
  (search), tags, YouTube privacy, description and thumbnail; templates per show; "update all".
- YouTube: the reusable key starts a broadcast as soon as the stream arrives; title and category
  are set in YouTube Studio, shown in a Lightair window (or through the user's own Google Cloud
  project, if they've made one).
- Health back from the platforms where they say it (YouTube stream health, Twitch's ingest).

### 2.3 Event bus

One typed event model for every platform and for Lightair itself, published as JSON Schema with
TypeScript types generated from it (Streamer.bot's untyped string arguments are its most common
complaint):

```text
Event { id, ts, kind, platform?, account?, channel?, native_id?, user?, data, raw? }
User  { id, login, display_name, color?, avatar?, roles[broadcaster|mod|vip|sub|member|verified],
        badges[{set, version, label, image}], first_time, months? }
kinds: chat.message · chat.deleted · chat.cleared · support.follow · support.sub · support.gift
       · support.cheer · support.superchat · support.membership · support.donation
       · support.kicks · support.tiktok_gift · raid · reward.redeemed · poll.* · prediction.*
       · hype.* · moderation.* · stream.online/offline · platform.info_changed
       · lightair.scene · lightair.output (live, recording, dropped frames, bitrate)
       · lightair.audio (silence, clipping on a track) · lightair.disk · lightair.custom
```

A chat message carries **fragments**, never HTML: text, emote (provider native/7TV/BTTV/FFZ/YouTube,
images at 1×/2×/4×, animated, zero-width), mention, cheermote, link; plus `reply_to`, `paid`
(amount in minor units, currency incl. bits and Kicks, display string, estimated USD),
`membership`, `source` (Twitch shared chat), and `state` (deleted, pinned, held by AutoMod).
Deletions are events that tombstone the stored message.

Lightair's own events are what no outside bot sees first-hand: dropped frames, a silent mic, a
track clipping, disk running out, a scene switched by the user.

### 2.4 Chat

- **Panel**: one feed and per-platform tabs or splits; platform and channel on each row;
  native + 7TV/BTTV/FFZ emotes (live 7TV updates, zero-width, precedence 7TV › FFZ › BTTV ›
  native); reply to a message (goes to its platform); **send to** all, one platform or one
  account, greyed where a platform is read-only; mentions, first-time chatters, keywords and paid
  messages highlighted; pin and **feature on stream**; search; user card (history across platforms,
  notes, optional link of one person's accounts); pop-out window.
- **Moderation** from one menu, mapped per platform (delete, timeout, ban, unban, chat modes,
  AutoMod queue on Twitch), logged in Activity.
- **History** in SQLite per workspace, so overlays and the panel survive restarts.
- **TTS** for chat (filters, max length, per-user voice, skip): see 2.7.
- Translation of incoming messages (on-device where possible) later.
- **Web capture** connector (later): the long tail of sites read from their chat page in
  Lightair's own Chromium, as Social Stream Ninja does; sandboxed and labelled best effort.

### 2.5 Widgets

**Runtime.** Widgets are web pages Lightair runs (it already ships Chromium and draws pages
without copies), each in its own origin (`https://w-<id>.widgets.lightair.localhost`), with a
strict content policy, no `file://`, no navigation or pop-ups, no camera or microphone, a CPU and
frame budget, and **no way to reach the remote API**: the only door into Lightair is an injected,
capability-gated bridge.

```text
lightair.on('load', {fields, session, channels, currency, env:{editor, muted, output}})
lightair.on('event', Event)          // the event bus's model; recents replayed on load
lightair.on('session', Totals)       // latest/top/session totals per kind and platform
lightair.on('fields', patch) · lightair.on('button', name)
lightair.queue.hold(ms) / release()  // the host's alert queue: widgets don't build their own
lightair.storage.get/set/delete(key, {scope: instance|widget|global})
lightair.chat.send({text, targets?, reply_to?})          // only if the manifest asks
lightair.counters · lightair.timers · lightair.media.play(sound)
lightair.sanitize() · lightair.emotes.render(fragments)
```

**Package** (`.lwidget`, a zip): `manifest.json` (id, name, version, author, size, capabilities,
network hosts, queue), `index.html`, `style.css`, `main.js`, `fields.json`, `data.json`,
`assets/`. Field types are a superset of StreamElements' and Lumia's: text, textarea, number,
slider, checkbox, colour with alpha, dropdown, multiselect, font (system and Google), image /
sound / video (one or many), button, hidden; with groups, order and `visibleIf`.

**Editor.** On the canvas like any object: fields in the Inspector, live preview, a **test event
palette** (every event kind, per platform), presets saved as `data.json` snapshots, per-output
visibility, import/export of `.lwidget` files, a gallery later.

**Compatibility.** A StreamElements shim (as Lumia Stream ships): `onWidgetLoad`,
`onEventReceived` (with SE's listener names and payloads), `onSessionUpdate`, `SE_API` (store,
counters, resumeQueue, sanitize, cheerFilter, getOverlayStatus), `{{field}}` substitution,
`widgetDuration`, SE field types mapped; import by pasting the HTML/CSS/JS/FIELDS/DATA tabs or a
folder, with a report of what didn't map. Streamlabs' custom HTML shim later.

**Also outside Lightair**: every widget at a local URL (`http://127.0.0.1:<port>/w/<id>?key=…`, a
key for that widget only) for anyone running OBS too.

**First-party widgets**, built on the same public runtime: chat box, featured message, alert box,
event list, goal, label (latest/top), counter, countdown and **subathon timer**, wheel, poll and
prediction, emote wall/combo, credits, now playing, social rotator, leaderboard.

### 2.6 Automation (Streamer.bot's place)

**Model**: trigger → conditions → actions, in named **queues**.

- **Triggers**: any event (2.3) with filters (platform, role, reward, amount ≥, regex), chat
  commands, timers (interval, random interval, cron, minimum chat activity), hotkeys, Stream Deck,
  MIDI, game controller, voice phrases (the on-device Whisper Lightair has for captions), file
  changes, HTTP/WebSocket calls, another action, app start.
- **Actions**: **every Lightair command and remote API method is a step** with no extra work
  (switch scene, show object, set effect, play media, mute, record, clip, marker, set text…), plus
  logic (if/else, switch, loop, parallel, weighted random, delay, wait for event, run action with
  parameters and results), variables, chat (send, reply, pin, announce), platform (title, category,
  ads, polls, predictions, rewards, raids, shoutouts, clips, markers, moderation), TTS, sounds,
  HTTP, files (allow-listed), integrations (2.8).
- **Queues**: blocking or concurrent, pause/resume, "always run", priority, max pending with a drop
  policy, skip; the alert queue is one, with a panel and a skip button.
- **Variables** in SQLite: `run` (the trigger's data), `global` (kept or not), `user` (per platform
  user, or per linked person), `action`. One template language (`{{ user.display_name }}`,
  `{{ math(...) }}`, filters), no get-before-set; change events; a live inspector.
- **Viewers**: one table (roles, groups, first seen, first words, watch time, notes) and **points**
  (payout rules, multipliers per group, spend on commands and rewards) from the first version.
- **Built in, ready-made**: commands (aliases, cooldowns global and per user, permissions,
  counters), `!so` with a clip, `!lurk`, quotes, timed messages, death counter, giveaways, a
  viewer queue, credits.
- **Editor**: a vertical list of nestable blocks (like Shortcuts) with a typed variable picker
  fed by the event schema, test and simulate buttons, templates ("Follower alert", "Death
  counter", "!so with a clip"), enable switches, undo, and a **run history** showing each step's
  inputs and outputs. The same JSON documents are what scripts, the remote API and **MCP** read and
  write, so an AI agent can build an automation from a sentence.
- **Scripting**: JavaScript in an embedded engine (QuickJS through `rquickjs`; V8 if speed ever
  matters), a script step getting `ctx` and returning outputs or `stop`; the API is the event bus,
  every command, chat, platform functions, variables, viewers, queues, timers, TTS, sound, and
  `fetch` to allowed hosts. WebAssembly components later for compiled plugins.
- **Safety**: no ambient file, process or network access; a pack declares what it needs (hosts,
  paths, chat, moderation, scene control) and the user approves it on import, with the difference
  shown on update; time and memory limits per run; rate limits on chat and moderation; a dry-run
  mode; tokens never visible.
- **Streamer.bot import** (later, best effort): its export format is undocumented and changes
  between versions; simple steps (OBS scene/source/filter, delay, send message, sound, globals,
  if/else, run action, fetch, timeout) can be converted, C# comes in as a disabled block with an
  optional AI translation to JavaScript.

### 2.7 Text to speech

Native voices first (macOS AVSpeechSynthesizer and Personal Voice where allowed; Windows
OneCore/SAPI), then ElevenLabs, Azure and Polly with the user's own keys; per-user voices,
blocklist, length limit, queue and skip, played on a mixer track (so it's mixed, monitored and
recorded like any sound).

### 2.8 Integrations, in order

1. **Donations**: StreamElements and Streamlabs tips (their sockets), Ko-fi (its stream alerts
   page); Fourthwall and Throne only send webhooks, so they'd be read from their own pages too.
2. **Spotify** now playing and `!sr` (adding to the queue needs Premium); Apple Music now playing.
3. **Stream Deck plugin** and **MIDI** (docs/12), both on the remote API.
4. **Discord**: go-live announcement, webhooks.
5. **VTube Studio** (its local WebSocket API).
6. **Lights**: Philips Hue (local bridge), LIFX (LAN), Govee, Elgato Key Light.
7. **Heart rate**: Pulsoid, HypeRate.
8. Crowd Control and the rest when asked for. Voicemeeter, Wave Link and the like aren't needed:
   Lightair's mixer is the mixer.

### 2.9 No server

Lightair runs no server of its own (a relay for webhook-only services was built, then dropped
on 17 November 2026): everything happens on the streamer's computer. Services that can only
send webhooks are reached another way: their own pages, opened in Lightair (Ko-fi's stream
alerts page; X's broadcast page for X chat). What arrives while Lightair is closed isn't
replayed.

### 2.10 Approval-free routes

| Platform | Chat in | Chat out | Events | Key |
|---|---|---|---|---|
| YouTube | The feed youtube.com's chat page polls (`get_live_chat`, no login for public streams), read by a Rust client; the hidden page finds the live video and the feed's start | Typed into the hidden chat page (YouTube's bot checks pass as for a person) | Super Chats and Stickers, memberships (new, milestones, gifts), Jewels, polls, deletions, pins, raids | The channel's reusable key in YouTube Studio |
| Facebook | Hidden Live Producer or video page | Typed into it | Comments, Stars, highlighted comments, replies, viewers | Persistent key in Live Producer |
| Instagram | Hidden live page: its own comment requests, the page as a fallback | Typed into it | Comments, joins, viewers, follows and likes | New per stream: read from Live Producer |
| X | Hidden broadcast page (its WebSocket frames) | Typed into it | Chat | Live Studio source key |
| TikTok | Hidden live page. Built: its comments as the page draws them (no login, video paused). Next: its own signed webcast socket, decoded in Rust from TikTok's protobuf, for gifts and the rest | Typed into it | Chat, gifts with streaks, likes, follows, joins, shares, subs, battles | LIVE Center producer page (new per stream; ~1,000 followers) |
| LinkedIn | Hidden event page | Typed into it | Comments, reactions | The event's custom stream (eligible accounts only) |
| Kick | Its chat socket (Pusher), the chatroom and Kick's current Pusher app found through the hidden page (Kick rotates the app; Cloudflare turns away plain clients). Built | Typed into the hidden page, or the official API | Chat, subs, gifts, bans, pins | Dashboard |
| Rumble | The Live Stream API link the user copies from their account (their own key: no approval), polled | Typed into the hidden page | Chat, rants, follows | Get Streamer Configuration |

How the hidden pages work:

- A page per connected platform, in Lightair's Chromium, logged in as the user, never shown
  unless the user opens it (to log in, or to look). Data is taken from **the page's own network
  traffic** (Chromium's DevTools protocol: WebSocket frames and response bodies, which the page
  has already signed and the platform has already let through), with the page's content as the
  fallback; it survives redesigns better.
- Sending is **typing into the page's chat box**, as the user would: the platform's bot checks,
  sessions and anti-forgery tokens are the page's own.
- Pages kept awake (no background throttling) and watched: no messages for a while on a live
  stream reloads the page.
- **Each platform's reading rules are data, updated without an app release**, with a switch to
  turn a platform's connector off: these routes change when the sites do (YouTube's in October
  2025, TikTok's signing several times), and fixes need to reach people in days.
- Risk is lowest as it's used here: the user's own session, on their own stream, at a person's
  pace. It's highest on LinkedIn and TikTok; Lightair says so where they're connected.

### 2.11 Asian platforms

Streaming (in the Outputs list since 16 November 2026), with where the key is shown in each
platform's own words:

| Country | Platform | Server | Notes |
|---|---|---|---|
| Korea | **CHZZK** (치지직) | `rtmp://global-rtmp.lip2.navercorp.com:8080/relay` | 스튜디오 → 방송 설정; 8 Mbps, 1 s keyframes, no B-frames |
| | **SOOP** (숲, formerly AfreecaTV) | `rtmp://stream.soop.live/app/` | 외부장치 방송 설정; 8 Mbps |
| | PandaTV (팬더티비) | `rtmp://rtmp.pandalive.co.kr/app` | 7.5 Mbps |
| | CIME (씨미) | pasted (Amazon IVS) | new in March 2026 |
| Japan | **Niconico** (ニコニコ生放送) | `rtmp://liveorigin.dlive.nicovideo.jp/live/input` | key new per broadcast |
| | **TwitCasting** (ツイキャス) | pasted | ツールとの連携設定 |
| | 17LIVE (イチナナ) | pasted | vertical; PUSH URL and Push Key |
| | Mirrativ (ミラティブ) | pasted | vertical; 720p30, ~1 Mbps |
| | Whowatch (ふわっち) | `rtmps://ingest.global-contribute.live-video.net:443/app` | no B-frames |
| China | **Bilibili** (哔哩哔哩直播) | four ingests, mainland and overseas | 5,000 followers and real-name for a key; else 直播姬's third-party mode |
| | **Huya** (虎牙) | `rtmp://al.direct.huya.com/huyalive/` | 开通远程推流 |
| | **DouYu** (斗鱼) | pasted (per session) | 开始直播 or 第三方转推 |
| | WeChat Channels (视频号) | pasted (expires in 2 h) | vertical; 500 followers, 10 videos, 3 lives |
| | Xiaohongshu (小红书) | `rtmp://live-push.xhscdn.com/live` | vertical; a 6-digit code per live |

Not stream targets: Douyin and Kuaishou (only their own PC apps, 直播伴侣, which can take
Lightair's virtual camera), Pococha (phone only), SHOWROOM (its key resolves to an address
through its own API: later), and the closed ones (Mildom 2024, Trovo and OPENREC 2026).

Chat, without approval:

| Platform | Read | Send |
|---|---|---|
| CHZZK | Its chat socket (`kr-ss*.chat.naver.com`), no login | Hidden logged-in page (the official Open API is an option) |
| SOOP | Its chat socket (binary protocol), no login for public rooms | Hidden logged-in page |
| Niconico | Its comment server (protobuf stream), no login | The watch socket, logged in through Niconico's own page |
| TwitCasting | Its realtime socket; the official API (self-serve) | Official API (self-serve app) |
| Bilibili | Its danmaku socket (WBI-signed), logged in for real names | Its send endpoint with the logged-in session |
| Huya, DouYu | Their chat sockets, no login | Hidden logged-in page |
| SHOWROOM | Its socket, no login | Later |
| WeChat Channels, Xiaohongshu, 17LIVE, Mirrativ | The platform's own live page, shown in a Lightair panel | The same page |

## 3. Phases

1. **Accounts and going live.** Twitch (device code) and the hidden logged-in pages (2.10) with
   sign-in for every platform; keys filled in from the platforms' own pages; stream info per
   destination. *Done when* a Twitch and a YouTube stream start from a fresh install without
   pasting anything.
2. **Event bus and chat, every platform.** Event model and schema; Twitch EventSub; YouTube's
   chat feed; Kick's socket; the hidden-page connectors (Facebook, Instagram, X, TikTok,
   LinkedIn); CHZZK, SOOP, Niconico, TwitCasting, Bilibili, Huya and DouYu readers; reading rules
   updated without a release; chat panel (feed, tabs, send to, reply, emotes incl.
   7TV/BTTV/FFZ, moderation, history); Activity panel. *Done when* a stream on four platforms has
   its chat and paid events in one feed and replies go to the right place.
3. **Widgets.** Runtime, sandbox, `.lwidget`, editor with fields and test events, local URLs; the
   first-party chat box, alert box, event list, goal, label, counter, subathon timer; the
   StreamElements shim. *Done when* a StreamElements custom widget pasted in runs unchanged and
   every first-party widget is configurable without code.
4. **Automation.** Triggers, actions, queues, variables, viewers and points, commands, timers,
   the block editor with run history, TTS, JavaScript steps with capability manifests; every
   command as a step; MCP authoring. *Done when* the top ten Streamer.bot recipes (alert queue,
   `!so` with a clip, death counter, redemption → scene, timed messages, TTS on donations, points
   and a giveaway, ad-break warning, raid welcome, first-chatter greeting) are built without
   code.
5. **Donations.** Streamlabs, StreamElements and Ko-fi, all on this computer. *Done when* a
   Ko-fi tip arrives as an alert.
6. **Integrations.** Spotify, Stream Deck, VTube Studio, lights, heart rate; SHOWROOM; the
   official APIs for people who bring their own credentials; Streamer.bot import.

Every phase lands on macOS and Windows together (accounts and the Keychain have their Windows
counterparts in `lightair-platform`; the widget runtime and scripting are platform-neutral).

## 4. Decisions to make

- **Updating reading rules**: where they're published from (a signed file Lightair fetches), so
  a site's change is fixed in days.
- **Twitch merged chat**: per-output chat overlays default to that output's platform (the
  compliant default); a merged overlay is a choice.
- **Scripting language**: JavaScript (familiar to every widget author, QuickJS is small and
  embeds cleanly) over Lua or C#.

## Sources

- Twitch: [OAuth flows](https://dev.twitch.tv/docs/authentication/getting-tokens-oauth/) ·
  [EventSub WebSocket](https://dev.twitch.tv/docs/eventsub/handling-websocket-events/) ·
  [chat limits](https://dev.twitch.tv/docs/chat/) · [change log](https://dev.twitch.tv/docs/change-log/) ·
  [shared chat sends](https://discuss.dev.twitch.tv/t/adding-for-source-only-parameter-for-shared-chat-and-a-behavior-update-for-send-chat-message-api-endpoint-may-19-2025/63261)
- YouTube: [native-app OAuth](https://developers.google.com/identity/protocols/oauth2/native-app) ·
  [streamList](https://developers.google.com/youtube/v3/live/docs/liveChatMessages/streamList) ·
  [quota costs](https://developers.google.com/youtube/v3/determine_quota_cost) ·
  [quota audits](https://developers.google.com/youtube/v3/guides/quota_and_compliance_audits) ·
  [revision history](https://developers.google.com/youtube/v3/live/revision_history)
- Kick: [OAuth](https://github.com/KickEngineering/KickDevDocs/blob/main/getting-started/generating-tokens-oauth2-flow.md) ·
  [scopes](https://github.com/KickEngineering/KickDevDocs/blob/main/scopes/scopes.md) ·
  [WebSocket request](https://github.com/KickEngineering/KickDevDocs/issues/20)
- TikTok: [Streamlabs integration](https://support.streamlabs.com/hc/en-us/articles/24730717904795-Streamlabs-Announces-Integration-With-TikTok) ·
  [TikTokLive](https://github.com/isaackogan/TikTokLive)
- Meta: [Live Video API](https://developers.facebook.com/documentation/live-video-api) ·
  [Login for Devices](https://developers.facebook.com/docs/facebook-login/for-devices/) ·
  [Instagram webhooks](https://developers.facebook.com/docs/instagram-platform/webhooks)
- X: [Livestream API](https://docs.x.com/livestream-api/introduction) · LinkedIn:
  [Live video](https://learn.microsoft.com/en-us/linkedin/consumer/integrations/live-video) ·
  Trovo: [shutdown](https://streamscharts.com/news/trovo-shut-down) · Rumble:
  [Live Stream API](https://rumblefaq.groovehq.com/help/how-to-use-rumble-s-live-stream-api)
- How others do it: [OBS custom OAuth](https://www.github.com/obsproject/obs-studio/wiki/Using-Custom-OAuth-Credentials) ·
  [Restream chat API](https://developers.restream.io/chat/getting-started) ·
  [Streamlabs socket API](https://dev.streamlabs.com/docs/socket-api)
- Chat tools: [Social Stream Ninja](https://github.com/steveseguin/social_stream) ·
  [its events](https://socialstream.ninja/docs/event-reference.html) ·
  [Restream Chat](https://support.restream.io/restream-chat/restream-chat) ·
  [Streamlabs Multichat](https://streamlabs.com/multistream/multichat) ·
  [Chatterino third-party emotes](https://wiki.chatterino.com/Third_party_services/) ·
  [Twitch merged-chat policy](https://upstream.so/blog/twitch-allows-unified-chat-simulcasting-rules/)
- Widgets: [StreamElements custom widgets](https://docs.streamelements.com/overlays/custom-widget) ·
  [their events](https://docs.streamelements.com/overlays/custom-widget-events) ·
  [Lumia custom overlays](https://dev.lumiastream.com/docs/custom-overlays/custom-overlays-documentation) ·
  [Razer and StreamElements](https://www.razer.com/newsroom/company-news/streamelements-acquisition)
- Streamer.bot: [1.0.0](https://streamer.bot/blog/releases/v1.0.0) ·
  [changelogs](https://docs.streamer.bot/changelogs) · [actions](https://docs.streamer.bot/guide/actions) ·
  [variables](https://docs.streamer.bot/guide/variables) · [triggers](https://docs.streamer.bot/api/triggers) ·
  [sub-actions](https://docs.streamer.bot/api/sub-actions) · [Kick](https://docs.streamer.bot/guide/platforms/kick) ·
  [macOS](https://docs.streamer.bot/get-started/installation/macos)
