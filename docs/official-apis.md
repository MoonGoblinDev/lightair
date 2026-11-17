# Official sign-in: registering Lightair's apps

Where a platform has an official API that a desktop app can use without a paid review,
Lightair supports it, as OBS does: the person signs in in their own browser (Settings ›
Connections › Official sign-in), and Lightair uses the platform's own API for the stream's
title, description and category, sending chat, and moderation. Where nobody signed in that way,
Lightair falls back to the signed-in pages (docs/plans/live-integrations.md, 2.10).

Each platform needs **Lightair's own registered app**, made once by whoever publishes Lightair
(never another app's: OBS's credentials are OBS's). Its ids go into the build:

```sh
LIGHTAIR_YOUTUBE_CLIENT_ID=… LIGHTAIR_YOUTUBE_CLIENT_SECRET=… \
LIGHTAIR_TWITCH_CLIENT_ID=… \
LIGHTAIR_KICK_CLIENT_ID=… LIGHTAIR_KICK_CLIENT_SECRET=… \
cargo xtask dist
```

The same variables set when Lightair runs override the built-in ones (for trying an app out).
Without a rebuild: Settings › Connections › Official sign-in › **Use your own app** takes an
app's client ID (and secret, where the platform wants one) from the clipboard and keeps it in the
Keychain; it's used ahead of the build's.
Tokens are kept in the Keychain ("Lightair connection", `oauth-<platform>`). The code:
`crates/lightair-live/src/oauth.rs` (sign-in) and `official.rs` (the APIs).

## YouTube (Google)

1. [Google Cloud console](https://console.cloud.google.com/): a new project; **APIs & Services ›
   Library › YouTube Data API v3 › Enable**.
2. **OAuth consent screen**: External; app name "Lightair"; the scope
   `https://www.googleapis.com/auth/youtube`. While it's in Testing, add the Google accounts
   that may sign in (up to 100); they see an "unverified app" notice.
3. **Credentials › Create credentials › OAuth client ID › Desktop app**. Its client ID and
   secret are `LIGHTAIR_YOUTUBE_CLIENT_ID` / `_SECRET` (Google's desktop secrets aren't secret:
   every copy carries it). Sign-in comes back to `http://127.0.0.1:<any port>`: nothing to
   register.
4. For everyone: **Publish app**, then Google's verification (the scope is "sensitive", not
   "restricted": a review of the app and its privacy policy, no security assessment).

Quota: 10,000 units a day by default; sending a chat message costs 50, an edit 50, reading the
broadcast 1–3.

## Twitch

1. [dev.twitch.tv/console](https://dev.twitch.tv/console) › Register Your Application: name
   "Lightair", **Client Type: Public**, OAuth Redirect URL `http://localhost` (unused: Lightair
   signs in with Twitch's device code flow), category "Broadcaster Suite".
2. Its Client ID is `LIGHTAIR_TWITCH_CLIENT_ID`. A public client has no secret in the app.

Scopes asked: `channel:manage:broadcast user:write:chat user:read:chat
moderator:manage:banned_users moderator:manage:chat_messages`. No review.

## Kick

1. kick.com › Settings › Developer › Create app: name "Lightair", redirect URL
   **`http://127.0.0.1:47801/callback`** (another port: set `LIGHTAIR_KICK_REDIRECT_PORT`),
   scopes `user:read channel:read channel:write chat:write moderation:ban`.
2. Its Client ID and Secret are `LIGHTAIR_KICK_CLIENT_ID` / `_SECRET` (Kick's token request
   wants the secret even with PKCE).

## X

1. [developer.x.com](https://developer.x.com) › a Project and App; **User authentication
   settings**: OAuth 2.0, type **Native App** (public: no secret in Lightair) or Web App (then
   `LIGHTAIR_X_CLIENT_SECRET`, sent as HTTP Basic), callback **`http://127.0.0.1:47802/callback`**
   (another port: `LIGHTAIR_X_REDIRECT_PORT`).
2. **The Livestream API needs X's approval** for the app (X's docs: ask your X account team to
   turn it on; an Enterprise agreement). Its Client ID is `LIGHTAIR_X_CLIENT_ID`.

Scopes asked: `broadcast.read broadcast.write tweet.read users.read offline.access`. X takes a
title only when a broadcast starts, so the Stream panel's title for X is kept for the next one:
when Lightair starts streaming, it waits for X's source to see the video and goes live on X with
it (`POST /2/broadcasts`), and ends that broadcast when the stream stops. Chat: sending works
(`POST /2/broadcasts/:id/chat`, 140 characters); X delivers chat to a public webhook (its
Activity API), and Lightair runs no server: X chat is read from X's own broadcast page,
signed in, like TikTok's (not built yet: it needs a signed-in X session to build against).

## Spotify

1. [developer.spotify.com/dashboard](https://developer.spotify.com/dashboard) › Create app: name
   "Lightair", redirect URI **`http://127.0.0.1:47803/callback`**, the Web API.
2. Its Client ID is `LIGHTAIR_SPOTIFY_CLIENT_ID` (sign-in uses PKCE: no secret in Lightair).
   In development mode up to 25 Spotify accounts can sign in (add them under User Management);
   more needs Spotify's extended quota.

Scopes asked: `user-read-currently-playing user-read-playback-state`. Now playing then shows
what plays on any of the person's devices, with its artwork and progress; without it, Lightair
asks the Spotify or Music app on this computer.

## Not offered (yet)

- **TikTok, Facebook, Instagram, LinkedIn**: their live APIs need app review or business
  approval (or don't exist for this): the signed-in pages stay.
- **YouTube message deletion**: the reader numbers messages as YouTube's site does, which the
  Data API doesn't know; bans and timeouts work.
