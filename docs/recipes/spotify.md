# Recipe: Spotify control

Search, play, pause, skip and set the volume on a Spotify Connect device ("play some Brubeck in the kitchen"). ClawDot only **controls** a player; the audio comes out of the device you name, not out of ClawDot's speaker ([media.md](../architecture/media.md), REQ-MUS-006).

> **Read this first: Spotify's Developer Policy restricts exactly this use.** On 2026-11-09 the Spotify Developer Policy (effective 15 May 2025), Section III "Prohibited applications", item 3, reads "Do not create a voice-enabled SDA that enables a user to control Spotify with their voice" and also bars "any kind of voice assistant that provides voice-control functionality". The page lists exceptions for other items (alarms, replicating a core experience, personal data transfer) and none for this one. An application that turns speech into Spotify Web API calls is, on the plain reading, such an application, even though ClawDot converts the speech to text first. No Spotify-approved route for it was found. This recipe is therefore **documentation of a technically working configuration that the policy, as read, does not permit you to rely on**. Do not ship it, publish an app built on it, or use it commercially. Whether private, personal use by one person with their own developer app is tolerated is not stated in anything read. Treat that as unknown and your own risk, and see "Compliant alternatives" below. Decision for the maintainer: [open-questions.md](../open-questions.md).

## What was checked, and what was not

| Source (read 2026-11-09) | What it says |
|---|---|
| Developer Policy <https://developer.spotify.com/policy>, "Effective as of 15 May, 2025" | III.3 voice control (quoted above). III.10 "Spotify is for personal, non-commercial use". IV.1 music streaming only for subscribers. III.14 no training of ML or AI models on Spotify content. |
| Developer Terms <https://developer.spotify.com/terms>, "Version 10, effective as of 15 May, 2025" | III.1.a licence for "private personal use" on Approved Devices. IV.2.1.1 no use of the Platform or Spotify Content to train a machine learning or AI model. IV.3.1.1 do not store Spotify Content indefinitely. |
| Authorization <https://developer.spotify.com/documentation/web-api/concepts/authorization> | Flows: authorisation code, authorisation code with PKCE, client credentials. **No device-code flow.** PKCE refreshes tokens and needs no client secret. |
| Redirect URIs <https://developer.spotify.com/documentation/web-api/concepts/redirect_uri> | HTTPS, except a loopback IP literal (`http://127.0.0.1:PORT`, `http://[::1]:PORT`). **`localhost` is not allowed.** Enforced on new apps from 9 April 2025; all apps expected to comply by November 2025. |
| Start/resume playback <https://developer.spotify.com/documentation/web-api/reference/start-a-users-playback> | `PUT /v1/me/player/play`, scope `user-modify-playback-state`, "This API only works for users who have Spotify Premium." Optional `device_id`; body `context_uri` or `uris`. |

**Could not verify:** whether Spotify tolerates or has ever approved a personal voice-controlled app; the Spotify-side rules for a "development mode" app (user limits, whether the owner needs Premium, quota review), which were not on any page read; whether the Policy has an FAQ or a later amendment that changes III.3; rate limits; and every live call. Third-party forum answers found say the speech-to-text route is still treated as voice-controlled; they are not Spotify statements and are not relied on.

## Compliant alternatives (recommended)

* **Do not use the Web API for voice.** Control the music with Spotify's own apps (phone, desktop or the speaker's remote), and let ClawDot do everything else.
* **A source that is yours.** Internet radio is built in and needs no account ([media.md](../architecture/media.md)). A self-hosted library through MPD, Navidrome or Music Assistant is controlled the same way this recipe controls Spotify (HTTP tools to the player's own API) with no third-party policy in the way. Write that recipe against the player's documentation; the tool shapes below carry over.
* **Receiver options for a Connect device you press play on yourself:** a phone, a Connect speaker, or `librespot` as a sidecar on a PC or Raspberry Pi (below). They give you a place to play; they do not make voice control permitted.

## If you go ahead anyway: the configuration

Shown so the maintainer can see what was built and tested, not as a recommendation.

### Architecture

```
ClawDot ── HTTP tool, fixed bearer SPOTIFY_PROXY_KEY ──► spotify_token_proxy.py (127.0.0.1:8890) ──► api.spotify.com
                                                          holds the refresh token, refreshes the access token
```

The core reads `auth_env` once at start-up and a Spotify access token lasts an hour, so the core cannot hold it. [`spotify_token_proxy.py`](../../examples/recipes/spotify_token_proxy.py) (Python 3 standard library, outside the core) keeps the refresh token in a 0600 file, refreshes the access token a minute before it expires, and forwards **only** eight calls: current track, devices, search, play, pause, next, previous and volume. It listens on the loopback address only, wants a shared key from ClawDot, never logs headers or tokens, and rewrites the play body (`{"uri": ...}` becomes `context_uri` for albums, artists and playlists, `uris` for a track).

### One-time setup

1. Create an app at the Spotify developer dashboard. Add the redirect URI `http://127.0.0.1:8888/callback` (an IP literal; `localhost` is refused). Copy the **client id**. No client secret is used.
2. Authorise once: `python3 spotify_token_proxy.py login --client-id YOUR_ID`. It prints an address, you sign in as the Premium user, and it stores the refresh token in `~/.config/clawdot/spotify-refresh.json` (mode 0600). This is the PKCE authorisation-code flow; Spotify has no device-code flow. Scopes: `user-read-playback-state`, `user-modify-playback-state`, `user-read-currently-playing`.
3. Run the helper beside ClawDot, with a random key in the environment (a systemd unit with `Environment=` or `EnvironmentFile=` works):
   ```
   SPOTIFY_PROXY_KEY=<at least 16 random characters> python3 spotify_token_proxy.py serve --client-id YOUR_ID
   ```
   Give ClawDot the same `SPOTIFY_PROXY_KEY` in its environment.
4. Find your device ids (`spotify_devices` tool, or the dashboard's console) and put them in the `device_id` lists of [`spotify-web-api.json`](../../examples/recipes/spotify-web-api.json). **The model can only target a device on that list.** Replace the `REPLACE_WITH_...` placeholders and rename them in the descriptions.

### Tiers

| Tool | Tier | Why |
|---|---|---|
| `spotify_now_playing`, `spotify_devices`, `spotify_search` | `read_only` | They read. Names come from outside; the descriptions say never to follow instructions in them. |
| `spotify_play`, `spotify_pause`, `spotify_next`, `spotify_previous`, `spotify_volume` | `reversible` | Undone by the opposite command. After outside text is in the conversation (a search result, a web page) they are held for a yes. |

Nothing here is `costly`: the Web API calls change no account setting. There is no tool to edit playlists or the library.

### Receivers

The Web API does not play audio; it tells a Connect device to. Spotify says the Player API needs Premium.

| Receiver | Notes |
|---|---|
| **Phone or desktop app** | Zero setup. It must be awake and recently active, or Spotify reports "no active device" (a `404`). |
| **A Connect speaker** (Sonos, many AV receivers and smart speakers) | Appears in `spotify_devices` while it is on the network. |
| **`librespot` sidecar** on a PC or Raspberry Pi | `librespot --name Kitchen` makes a headless Connect receiver (MIT licence). Its README says: "Using this code to connect to Spotify's API is probably forbidden by them. Use at your own risk", requires Premium, and warns that accounts created after an apparent cutoff cannot obtain audio keys (`audio key error 0 1`, no known workaround). Do not run it on the Echo Dot's budget: it is a separate program with its own memory and CPU. |
| **TECHO5's built-in Spotify Connect** | The TECHO5 project documents Spotify Connect through `librespot` with a documented upstream key failure ([research note 11](../research/11-open-source-echo-feature-landscape.md)). It carries the same librespot caveats, was read from the project's documents only, and has not been tried on a unit. |

ClawDot's own speaker is a receiver only if one of the above runs on that device (B6 in the [roadmap](../roadmap.md)).

### Failure modes

| Symptom | Cause and what the model says |
|---|---|
| `502 could not refresh the Spotify token; run login again` | The refresh token is missing, revoked or the app was removed. Run `login` again. |
| `401` from the helper | `SPOTIFY_PROXY_KEY` differs between ClawDot and the helper. |
| `404 Player command failed: No active device found` (Spotify's wording, written from memory and not checked) | The named device is asleep or not signed in. Wake it from the Spotify app once. |
| `403 Premium required` | The account is not Premium. |
| `429` | Spotify's rate limit; the model is told and says to try later. |
| `{"uri": "http://..."}` | Passed through untouched and refused by Spotify; the helper forwards only listed calls and does not interpret other bodies. |
| Wrong album plays | Search returned three results; the model chose one. Say the artist and album name. |

### Verify without an account

* `scripts/in-wsl.sh python3 -m unittest discover -s examples/recipes -p "test_*.py"` runs the helper against fake Spotify servers: the key is required, only the eight calls pass, the token is refreshed once and reused, a rotated refresh token is saved, play bodies are rewritten, a missing token is a clear `502`.
* `scripts/cargo.sh test -p clawdot-core --test recipes` checks each tool's request and that an unlisted `device_id` is refused before anything is sent.
* The replay eval `rcp-spotify-play-names-the-device` checks the spoken reply names the device.
* `--dry-run` with the real helper: playback commands are described, not sent; `spotify_search` and `spotify_devices` still run.
