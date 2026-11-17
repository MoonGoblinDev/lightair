# Not done yet

What's left, as of 17 November 2026. The live-integrations plan
([plans/live-integrations.md](plans/live-integrations.md)) says what's built; this is the rest,
with what's known about each. Keep it current: strike items as they land.

## Asked for, being worked on

1. ~~**A real colour picker for every colour field.**~~ Done (shell/colour_field.rs). Many fields offer only a few swatches
   (text, colour objects: `object_props.rs` `PALETTE`/`swatches`; widget fields of type
   `colorpicker` are a text box with a preview: `shell/widget_props.rs`). Every colour field
   should open a proper picker (hue/saturation, alpha where it applies, hex input, the swatches
   as presets, an eyedropper: `eyedropper.rs` exists).
2. ~~**Edit text objects on the canvas.**~~ Done (shell/canvas_text.rs; the field shows the code font, not the object's). Double-click a text object to type in place (today the
   text is edited in the Inspector's field only: `object_props.rs` `TextEdit`).
3. ~~**Sign-in that finishes on its own.**~~ Done. The sign-in window (`lightair_live::accounts::sign_in`,
   Outputs › a destination › Sending) should close itself as soon as the platform's sign-in
   cookie appears (poll `accounts::signed_in` while it's open), and the row update.
4. ~~**A Stream manager panel.**~~ Done in part (stream_panel.rs: live, title, category, viewers, uptime for Twitch, YouTube, Kick; Open dashboard signed in). Title and category edits: Twitch through the dashboard's own GraphQL mutation (Helix doesn't take the site's session), Kick through its site's request; both untested on a real channel (Edit in the Stream panel). YouTube Studio's edit asks for Google's anti-bot attestation, so YouTube stays its dashboard. ~~Stream health per destination~~ (done: bitrate, frames dropped, reconnecting, in each row while streaming). Left: more platforms' details. One place per destination for what's live: title, category or
   game, tags, viewers, uptime, stream health; change title and category per platform (through
   the signed-in pages, no app approval: docs/plans/live-integrations.md, 2.2 and 2.10).

## Chat and events

- **Facebook, Instagram, X, LinkedIn** readers and sending: through signed-in hidden pages.
  Needs a signed-in session to build against (selectors / their own requests are unknown until
  then).
- ~~**Huya**~~ (done: huya.rs, TARS; chat, gifts, nobles; read a live room with
  `LIGHTAIR_LIVE_CHECK=huya:<room>`). ~~**Niconico**~~ (done: niconico.rs, NDGR; read a
  live program's comments with `LIGHTAIR_LIVE_CHECK=niconico:lv… cargo test -p lightair-live
  live_check`).
- ~~**SOOP** subscriptions~~ (done: new, renewed, gifted; also ad, video and sub-broadcast
  balloons, Quick View, OGQ and mission gifts; untested on a live stream). ~~**YouTube** raids~~
  (done: its raid banner; YouTube gives no viewer count), ~~**Bilibili** emoticons~~ (done:
  stickers and `[name]` emoticons drawn as images; untested on a live room).
- **YouTube Private streams**: readable only signed in as the owner; Unlisted works from the
  stream's link. Could find the owner's own live (Private/Unlisted) through the signed-in
  session.
- ~~**Reading rules updated without a release**~~ (built: rules.rs; a per-platform off switch and
  the send/TikTok selectors read through it, signed with Ed25519). Not live yet: needs a place
  to publish (`ADDRESS` in rules.rs) and a key (`cargo xtask rules-key`, then paste the public
  key into `PUBLIC_KEY`); `cargo xtask rules-sign rules.json` signs each update. More of the
  readers' addresses and fields could move into it.
- **TwitCasting** reader is untested on a live stream.
- **Sending** is verified only for Twitch's login and join (`LIGHTAIR_SELFTEST=twitch-login`);
  YouTube, Kick and TikTok sending (typing into their pages) are untested with a real session.
  Google may refuse sign-in in an embedded browser.
- ~~**Import a sign-in from Chrome / Brave / Edge / Arc**~~ (done: Outputs › Sending › "Use my
  browser's"; handing cookies to Lightair's browser checked with `LIGHTAIR_SELFTEST=cookie-set`,
  reading a real browser's untested here since it asks for the Keychain). Native Touch ID
  passkeys can't be used (Apple's restricted browser entitlement).
- Chat panel: ~~tabs per platform, reply to a message~~ (done); ~~moderation~~ (built: delete,
  10-minute timeout, ban with a second click; Twitch by typing `/delete`, `/timeout`, `/ban`
  into its chat box in a signed-in hidden page, Kick through its site's requests from one;
  untested on a real chat. YouTube: bans and timeouts through the official sign-in; deleting a
  message there isn't possible, its API numbers messages differently.) Twitch's API (Helix) answers 404 to the site's
  session; the Stream panel's Twitch edit now uses the dashboard's GraphQL mutation.

## Widgets

- ~~Featured message, prediction, emote combo~~ (done: Chat › a message › Feature; Prediction
  bets Lightair points with `!bet 1 100`, winners split the pot; Emote combo). Countdown: the
  Timer object covers it.
- ~~A widget gallery~~ (done: Add object › Widgets, by what they're for, each with what it does).
  ~~Streamlabs custom-widget shim~~ (done: sl-shim.js, chosen when the widget's
  script listens on `document`; untested with a real Streamlabs widget).
- ~~Multi-line text fields in the widget Inspector~~ (done: field type `textarea`; the social
  rotator takes one a line).

## Automations

- Triggers: ~~stream/recording start and stop, app start, a web address per automation~~ (done);
  ~~hotkeys~~ (done, untested by pressing); ~~MIDI, game controllers, file changes~~ (done:
  triggers.rs; files tested end to end, MIDI and controllers without a device here);
  ~~voice phrases~~ (done on macOS: Apple's speech recognition in the bundled `lightair-speech`
  helper, on the Mac where the language allows; untested by speaking: it asks for the
  microphone and speech recognition the first time. Windows left).
- ~~Platform steps~~ (built on the official APIs: change the title or category on any platform
  signed in officially; on Twitch ads, polls, predictions, raids, shoutouts, clips, markers;
  untested until an app is registered). Rewards management left.
- ~~A block editor beyond the current list~~ (done: If's conditions, Then and Otherwise, and
  Random's choices edited in place, nested to any depth; a step moves between lists with its
  move button).
- ~~Streamer.bot import~~ (done: Automations › New › Import from Streamer.bot, from a copied
  export; all 141 public exports found import, C# kept as comments to port, older If/Else and
  some OBS steps listed as not brought over). ~~Twitch channel-point redemptions~~ (built:
  EventSub over WebSocket with the official sign-in; trigger "Channel-points redemptions",
  `{{ reward }}`; untested until an app is registered).

## Integrations

- **Official sign-in** (built: docs/official-apis.md; oauth.rs, official.rs): YouTube (title,
  description, category, chat, bans; finds Private and Unlisted streams), Twitch (title,
  category, chat, delete, bans), Kick (title, category, chat, bans), each used first where
  signed in; X (goes live with your title when you stream and ends it after; sends chat; X chat
  is to be read from X's own page, signed in). Needs Lightair's apps registered (client ids
  in the build); untested until then.
- ~~Philips Hue~~ (done: Settings › Connections › Philips Hue, automation step "Philips Hue
  light"; tested against a stand-in bridge only). ~~A Lightair Stream Deck plugin~~ (done:
  streamdeck/, Settings › Remote API › Stream Deck; tested with a stand-in Stream Deck against a
  real Lightair, not yet on a real Stream Deck). ~~Bring-your-own official API credentials~~
  (done: Settings › Connections › Official sign-in › Use your own app). ~~Spotify's own API~~
  (done: official sign-in; Now playing then shows any of your devices, untested until an app is
  registered).
- ~~**The relay**~~ (dropped: Lightair runs no server). Ko-fi is read from its own stream alerts
  page (Settings › Connections › Ko-fi › Paste stream alerts link; untested with a real Ko-fi
  account).
- Pulsoid and Elgato Key Lights are untested without a token / a light; VTube Studio was tested
  against a stand-in server only.

## Windows

Built, tested (317 tests) and benchmarked on Windows 11 on 17 November 2026
([plans/windows.md](plans/windows.md) §6 says what that took; numbers in
[benchmarks/lightair-vs-obs-windows.md](benchmarks/lightair-vs-obs-windows.md)). Left:

- **An installer and signing**: `cargo xtask dist` makes the portable app and its zip; an MSIX or
  Inno Setup installer, Authenticode signing (a certificate). ~~The updater~~ (built: updater.rs,
  for the portable zip too; untested on Windows).
- ~~The virtual camera~~ (Lightair's own on Windows 11, OBS's elsewhere), ~~"Now playing"~~,
  ~~"Use my browser's" sign-in~~ and ~~the Editor player without read back~~: done
  (plans/windows.md §6). Left there: Google's app-bound cookies (sign in in Lightair's window
  instead); Lightair Virtual Camera in Teams/Zoom/a browser (seen through Windows' Frame Server with ffmpeg;
  needs `regsvr32 lightair_camera.dll` as administrator once, or the menu's "Install Lightair
  Virtual Camera…"); the camera at sizes
  other than 1080p (a canvas of another shape is stretched to 16:9); an installer that registers
  it.
- Windows Studio Effects from Lightair itself (the camera's extended controls): needs a Copilot+
  PC to try on; Settings › Camera works meanwhile.
- The zoom badge by the pointer; the system pointer's picture; pointer and follow-zoom on a second
  monitor (`display_frame`); virtual cameras labelled as such in Add object.
- Under the full parallel run, now and then: `media_on_air` (an off-air video still Playing),
  `stream_sharing::identical_destinations_share_an_encoder`, `sequence::proxies_and_filmstrips`
  (ffmpeg's hardware H.264 encoder shared by tests at once); each passes alone.
- NVENC needs a newer NVIDIA driver than 560 for OBS 33's encoder (OBS says so too); Lightair
  falls back to Quick Sync or x264 as OBS does. ~~Worth a note in the Encoders choice~~ (done).

## Known issues

- ~~**`cargo xtask dist` sometimes leaves an invalid signature**~~ (found: the bad app was what
  a run that failed part way left behind, its Chromium helpers signed alone and their bundles not
  yet sealed; it was then installed without checking dist's exit. Now a failed run moves its
  app to `Lightair (failed).app`, and codesign retries when Apple's timestamp service has a
  moment's trouble, the likely cause of the failure. `cargo xtask bundle` signs only the outer
  app, so `codesign --verify --deep` on a bundle (not dist) build always reports the helpers.)
- **`media_native::a_rotated_video_shows_upright`** (lightair-preview) fails now and then when
  the whole suite runs in parallel; passes alone. So does
  `stream_sharing::identical_destinations_share_an_encoder` and `audio::track_levels_follow_the_fader`
  (lightair-engine), and `media_native::a_local_video_plays_through_lightairs_own_source`
  (lightair-preview): timing under load.
- Media source render time +1 ms (deferred earlier).
- ~~Notarization~~ (done: release.env's `LIGHTAIR_NOTARY_PROFILE`; the release workflow
  notarizes on GitHub). ~~Updates~~ (built: Settings › About, signed manifests on GitHub
  Releases; docs/releasing.md).
