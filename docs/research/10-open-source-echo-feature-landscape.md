# R-10: Open-source Echo and voice-satellite feature landscape (seven projects)

Date: 2026-11-09 · Feeds: roadmap Phase B (B0 trial, B2 ESPHome client, B4 features through the base), [future scope](../future-scope.md), the Dot base decision (TECHO5 preferred) · Refines: [R-08](08-existing-echo-dot-projects.md) (which this note partly corrects, section 3)

Method: read-only, 2026-11-09, through the GitHub API and the vendors' own documentation. No device was touched, nothing was cloned or run, no code was copied. Claims below are marked **Confirmed** (a primary file says it, and no contrary evidence found), **Partly** (true with a material limit), **Not confirmed** (could not find it in primary sources) or **Contradicted**. "Docs" means the project's own documentation, which is a statement of intent by the author; where I read only docs and not the code I say so.

## 1. Provenance of the table, and how much to trust it

The feature table the maintainer pasted is published at <https://huskerminion.github.io/techo5/comparison/> and its data lives in the TECHO5 repository (`docs/comparison/data.json`, `docs/comparison.md`). Facts about it, from that page and file:

- **It is published by the author of TECHO5**, one of the seven projects compared. That is a disclosed conflict of interest. The page says it is not a ranking and that every project was handled the same way.
- Checked **2026-11-08**. A separate reviewer read each project's public GitHub repositories and a second reviewer checked; device rows also use issues and discussions. Forums, Reddit and Discord were not counted. **The author tested nothing.**
- The dagger means "this cell has a note" (a source link and the row's definition), not "unverified". "Not found" means nothing was found in the repos, not that it cannot work. "In progress" means not yet released.
- The row definitions matter more than the names. Example: "Assistant without Home Assistant: Yes" means only "speech-to-text, a chat model and text-to-speech can answer on the device with no HA pipeline"; it does **not** mean self-contained (see 2.1).

I treat the TECHO5 cells with extra scepticism and re-checked every decision-relevant cell against the primary repos. Result: the table is **mostly accurate but compresses important limits into one word**, and in two places the underlying evidence differs from the cell (section 2). I found no sign of favouritism toward TECHO5 in the cells I checked; the errors I found are as often pessimistic about other projects (LibreEcho alarms) as flattering to TECHO5.

## 2. Verification of the claims that matter to us

Sources: TECHO5 = `HuskerMinion/techo5` at release v1.6.33 (README, `docs/setup.md`, `docs/actions.md`, `blueprints/README.md`, `echod/internal/feature/voice/voice.go`) and `HuskerMinion/techo5-dot`; LibreEcho = `aslater3/LibreEcho` (README, release 0.14.5), `aslater3/LibreEcho-UI` (`docs/API.md`, `docs/GPT_LIVE.md`, `docs/voice-history-core.md`, `src/adapter/*`) and `docs/without-home-assistant.md` on its 0.14.6 branch.

| # | Claim | Project | Verdict | Evidence and the limit that matters |
|---|---|---|---|---|
| 1 | Assistant without Home Assistant | TECHO5 | **Partly** (confirmed that HA is not required; not that it is self-contained) | `docs/setup.md` "Voice without Home Assistant": speech goes to **two Wyoming servers you run** (faster-whisper, Piper), the text to **any OpenAI-style chat endpoint** (llama.cpp, Ollama), the reply to Piper. "All three servers are needed." The brain is a model with a **tool set limited to the device's own functions** (timers, alarms, reminders, radio, volume, Music Assistant, calling other devices, calendar, weather, to-do, optional SearXNG web search); five-minute conversation memory; a taint rule (after reading the web or calendar nothing in that request may change state). "What it doesn't do: control the house." This is a real tool-calling loop (the docs describe model-chosen actions), but I did not read the code, and it has no MCP client and no user-defined tools. |
| 1b | (same) | TECHO5 | Not confirmed (resolved: setup no longer forces HA) | R-08 reported issue #40 (setup page only opens from HA). The issue is **still open** (updated 2026-11-09) but first-run now has an **"On its own"** option that skips HA (`docs/setup.md` "First run"). Many per-device settings (second wake word, quiet hours, DND, follow-up time, effects) are still **exposed as HA/ESPHome entities**; without HA they are reached only by the setup page, the signed Remote API (`/api/v1/entities`) or an ESPHome client. |
| 2 | Assistant without Home Assistant | LibreEcho | **Partly** (confirmed, but it is a chat bot with intent matching, not an agent) | README "What you get"; `LibreEcho-UI/src/adapter/llm_provider.h`, `llm_openai.c`, `timer_intent.h`. Speech runs on the device (sherpa) or on Wyoming servers ("Custom" mode); the LLM request is `instructions + transcript` and streamed text back (`/chat/completions`, `stream:true`); there is **no tool-calling in the request**. Timers/stop are caught by a **narrow regex-style intent matcher before the model** ("a model can answer a question but it cannot start a timer"). Built-in weather lookup. So: intents plus chat, not a tool loop. |
| 3 | ChatGPT subscription or local LLM | LibreEcho | **Confirmed** | `docs/API.md` `POST /api/v1/assistant/auth/start|poll`: ChatGPT **device-code login**, tokens in `/data/libreecho/secrets/` mode 0600; provider ids include `openai-codex`; a base-URL setting for an OpenAI-compatible endpoint with an explicit LAN-HTTP allowance (`allow_insecure_http`). The subscription path rides ChatGPT's own backend, which OpenAI has not published as an API for third parties (risk: it can break or violate terms). |
| 3b | (same) | TECHO5 | **Partly** (matches the table) | Local OpenAI-style endpoint works; no ChatGPT sign-in found. Cloud realtime needs an API key. |
| 4 | Realtime speech-to-speech | TECHO5 | **Confirmed**, with limits | `docs/setup.md` "Realtime voice": OpenAI, xAI or Gemini realtime over a **user API key** (or a `wss://` relay), **off by default**, billed per audio minute, daily-minute cap, five-minute session cap, **no memory between turns**, no follow-up listening after a realtime answer. Own tools (timers, alarms, radio, volume); the house only through Home Assistant's MCP server and a long-lived HA token. Audio leaves the device while you speak (disclosed). |
| 4b | (same) | LibreEcho | **Confirmed as "In progress"; not usable yet** | `LibreEcho-UI/docs/GPT_LIVE.md`: transport implemented, validated only against a **mock server**; "the remaining external gate is one real ChatGPT device login and acceptance by the live service"; hidden, not in 0.14 releases. It targets an **undocumented WebSocket** (`/v1/realtime?intent=quicksilver&model=gpt-live-1-codex`) found by reading the Codex binary's strings. Interesting, fragile. |
| 5 | Alarms | TECHO5 | **Confirmed** | `docs/actions.md#set-an-alarm` (ESPHome action `alarm_set`), also model tools in the no-HA assistant; repeat days, snooze; keeps working with HA offline (`README.md#why-techo5`). |
| 5b | Reminders | TECHO5 | **Confirmed** | `docs/actions.md#set-a-reminder`; model tool in the no-HA assistant; HA blueprint "Remind me to ... every Tuesday". |
| 5c | Timers, alarms, reminders | LibreEcho | Timers **Confirmed**; alarms **Partly** (cell says "Not found" but the docs differ); reminders **Not confirmed** | `without-home-assistant.md` lists "Alarms set on the device" as working with HA down, and `API.md` timers have `kind: countdown\|alarm`, but `POST /api/v1/timers` creates countdowns only, and I found no recurrence, snooze or reminder feature. Treat alarms as rudimentary. |
| 6 | "Library of voice-command add-ons" | TECHO5 | **Partly, and misleading for us** | `blueprints/README.md`: the "10 voice blueprints" are **Home Assistant blueprints** (YAML automations triggered by Assist sentences that call `esphome.<name>_alarm_set` etc.). They **need HA**. Without HA the same abilities exist as built-in model tools (row 1). They are not an add-on or plugin system a third party can write for the device (the table's own "Plugin system" row is "Not found" for TECHO5 and LibreEcho). |
| 7 | Music by voice | TECHO5 | **Confirmed**, depends on Music Assistant | `docs/setup.md` "Music"; the no-HA assistant plays from Music Assistant "once one is set up on the device". Also AirPlay, **Spotify Connect** (librespot; docs themselves record a Spotify key refusal "error audio key 0 1" and a Music Assistant workaround), DLNA, Sendspin, Bluetooth audio (phone to Dot, and Dot to a Bluetooth speaker). |
| 7b | Music/radio | LibreEcho | **Confirmed** for radio, **Partly** for voice | Internet radio incl. HLS/AAC, AirPlay, Sendspin, Bluetooth audio in README; voice only chooses radio stations. No Spotify Connect found. |
| 8 | Internet radio | both | **Confirmed** | TECHO5 `docs/overview.md`, LibreEcho README. |
| 9 | Do not disturb / quiet hours | TECHO5 | **Confirmed** (as device entities), scope **Partly** | `docs/setup.md` lists "Quiet hours and Do not disturb" among HA-exposed controls; the definition behind the table is "silences announcements or refuses calls". Whether it also suppresses a non-HA announcement was not checked. LibreEcho: "Not found" holds (a `dnd` LED profile exists, but nothing in source sets it). |
| 10 | Announcements / start-conversation from outside HA | TECHO5 | **Partly** (advertised; untested with a non-HA client) | `echod/internal/feature/voice/voice.go` sets `FeatureAnnounce`, `FeatureStartConversation`, `FeatureTimers` on its ESPHome voice satellite. These are ESPHome native-API features, so any ESPHome **client** may call them (R-08 section 4); I did not test whether `echod` serves a non-HA client, and R-08's B0 trial still owns that. The Remote API (`:8181/api/v1`, HMAC-signed) **cannot** speak text or run actions: it lists, reads and sets entities and streams logs only. |
| 10b | (same) | LibreEcho | **Confirmed**, and **simpler** than ESPHome | `API.md`: `POST /api/v1/audio/announce` `{"text": ...}` speaks text through local TTS and the announcement bus, "the supported path for external local services". Bearer-token auth plus a CSRF header for state changes; plain HTTP on the LAN. This is the most directly usable non-HA "speak" API in the whole set. Also `POST /api/v1/assistant/respond` (text in, spoken answer out) and `POST /api/v1/timers`. It has no "start listening for the reply" call; the reply path is only the ESPHome satellite (advertises announce and start-conversation in `esphomed.c`). |
| 11 | Plugin points | all of the above | **Not found** for TECHO5 and LibreEcho (confirmed) | Table says Not found; I found none either. TECHO5 offers "running something of your own at boot" (a shell script, `docs/setup.md` "More") and SSH; that is how ClawDot would be co-resident (B5). Kiosk Satellite has a real plugin SDK, but it is an Android app. |
| 12 | Voice-turn history | TECHO5, LibreEcho | **Confirmed**, both RAM-only | TECHO5 "Recent voice requests": last 10, in memory, never persisted, follows "Send voice text". LibreEcho `docs/voice-history-core.md`: last 10, RAM-only, privacy-bounded, API `voice_history*`. Matches our privacy default (off or RAM-only). |
| 13 | Settings export/import, copy between devices | TECHO5 | **Confirmed** | `docs/setup.md` section 10: `techo5-settings.json` download/restore; secrets and privacy switches are deliberately excluded; per-setting linked sharing signed with a house word. LibreEcho has an export in the control centre; no copy between devices found (matches table). |
| 14 | Signed OTA with rollback | TECHO5, LibreEcho | **Confirmed** (docs) | TECHO5: A/B signed updates; LibreEcho README "Safe updates": signed complete generation, previous kept until the new one confirms. Not verified by running. |
| 15 | Two wake words, two assistants; nearest device answers; follow-up listening; stop without wake word | TECHO5 | **Confirmed** | `docs/setup.md` "Two wake words, two assistants" (the second is chosen in HA; **both share one HA conversation**, issue #328), "Listening after an answer" (0 to 30 s, up to 10 in a row), "The stop word" (no wake word, only while something is sounding), "The nearest device answers" (off by default). LibreEcho: no custom wake models, one wake word (table matches). |
| 16 | Local LLM "Keeps working when HA is down" | LibreEcho, TECHO5 | **Confirmed** | `without-home-assistant.md` (LibreEcho, checked on two units per the doc, not yet in a release); TECHO5 README. |

### 2.1 What this does to the R-08 conclusion

R-08 summary item 1 said neither Dot base is standalone and "both need Home Assistant for speech recognition, intent and speech output". **That was true of the default mode and of the first-run flow at the time, and is no longer the whole story.**

- **TECHO5**: HA is optional, but a non-HA setup needs **your own Wyoming STT/TTS servers and an OpenAI-style model**, and gives only the device's own abilities. That is a small assistant, not an agent: no MCP, no custom tools, no inbox.
- **LibreEcho**: the same shape, with on-device STT/TTS and a ChatGPT-subscription or local model, but chat plus intents instead of tools.
- Neither has anything like an **agent inbox**, a **bring-your-own agent session with voice replies that resume it**, MCP or user-defined HTTP tools, skills, or a portal. For ClawDot's use case the base images are still best treated as **front ends** (mic, speaker, ring, wake word, audio plumbing) with ClawDot as the brain, exactly as R-08 recommended. What changes is the **fallback story** and the **risk story**: a Dot running TECHO5 with "Voice without Home Assistant" works as a stand-alone helper before ClawDot is installed, and keeps timers, alarms, radio and Bluetooth when ClawDot is down.
- The decision-relevant contradiction is therefore **resolved as "Partly confirmed, narrower than the table's wording"**: it does not change the recommendation to implement the ESPHome voice-satellite client (B2) and keep TECHO5 as the preferred base, but it makes two things more important: (a) test early whether `echod` accepts a non-HA client (B0), and (b) decide which brain wins when a wake word is routed to TECHO5's own assistant and not to ClawDot (see idea "assistant-slot routing", section 4).
- A corrected sentence for R-08, to apply when that file is next edited: "Neither is standalone in the sense ClawDot needs; each can answer simple questions without HA given user-run speech and chat servers (TECHO5) or a ChatGPT login or local model (LibreEcho)."

## 3. The seven projects

Maintenance figures come from the TECHO5 table's own data file and the GitHub API on 2026-11-09 and are indicative only.

- **echo-dot-assist** (`Gamer92000/echo-dot-assist`, MIT, 890 stars, first commit 2026-09-21, last push 2026-11-09). **Keeps Fire OS** and Amazon's own audio front end and wake-word engine; a C daemon (`hassmic`) replaces the Alexa client and speaks ESPHome or Wyoming to HA. Works on Dot 2 (`biscuit`, micro-USB), Dot 3 and Echo 2; Show devices are in beta on main. Heavy on stock-Echo parity (nearest Echo answers, Drop In, firewall to Amazon, Bluetooth). 88 releases, 84 of them pre-releases; unlock instructions warn of two bricked Dot 3 units. Needs HA only; a different wake-word engine than the others (Amazon's), which is both its audio-quality strength and its dependence on Amazon blobs.
- **EchoLocal** (`ygelfand/echolocal`, MIT; HACS companion `echolocal-hacs`; shared library `libcountertop` is **AGPL-3.0**). Keeps Fire OS with Amazon services disabled; a Go daemon (`echod`) and a host CLI installer (macOS, Linux, Windows). Strong management features (turn history, health, wake-word library through HACS). 549 stars, pre-1.0 (0.0.13), several releases a week. HA-centric. Its `echod` is the ancestor of TECHO5's daemon (TECHO5 reuses MIT code from it).
- **EchoMuse** (`wilbowes/EchoMuse`, MIT, 1,429 stars per R-08). Replaces the software with its own Linux userspace (emOS) or runs a Go firmware on rooted Fire OS 5, plus a **separate controller** (HA add-on or Docker) that impersonates ESPHome satellites; browser WebUSB installer; wake-word trainer. 76 releases, firmware v2.19.0 on 2026-11-08, 30+ contributors, Discord. Mature, but needs a controller host (R-08 section 4).
- **Kiosk Satellite** (`jxlarrea/kiosk-satellite`, 2,578 stars, 327 releases, last push 2026-11-09). An **Android app** (kiosk, voice satellite, plugin SDK) for tablets and Portals, plus separate **LineageOS 18.1 images for Echo Dot (`biscuit`), Show 5 and Show 8**. The only one with a real plugin system (SDK repo is Apache-2.0). **App licence is CC BY-NC-ND 4.0** with a plugin exception: we may not fork, modify or redistribute it commercially, so it is a "use as is" option only, and the Dot image repo states no licence. The table marks it, LibreEcho and View Assist as not "no telemetry by default" (not checked by me).
- **LibreEcho** (`aslater3/LibreEcho` and four sibling repos, MIT, 132 stars, started 2026-07, last push 2026-11-05). **Replaces the OS entirely**: Linux 6.1 on the MT8163, C daemons, signed A/B OTA, web control centre with accounts. Dot 2 support is newer and less field-tested than the Echo 2. Install needs a Linux PC with fastboot and an unlock zip the user supplies. 19 stable releases, 132 dev builds. Sends a weekly anonymous count ping (opt-out from 0.14.5) and bundles a **non-commercial wake-word model** (README licence note).
- **TECHO5** (`HuskerMinion/techo5`, `techo5-dot`; MIT, 600 and 238 stars, first commit 2026-09-14/16, last push 2026-11-09). Replaces Fire OS with **Alpine Linux and one Go daemon**; signed A/B OTA; broad device list (Dot 2, Echo 2, Dot 3, Show 5/8, Spot, Skylight, a tablet Panel app). Very high release cadence (`dot-v1.0.60`, 121 releases). Written largely with AI assistance by its own account. Five open issues on the Dot repo but 23 on the main repo. Author also publishes the comparison table.
- **View Assist** (`dinki/View-Assist`, CC BY-NC 4.0; companion app VACA, Apache-2.0). Not a firmware: an **HA integration, dashboards and an Android app** that make a tablet or Echo Show running stock or custom Android into a visual Assist satellite. Needs HA. Irrelevant to a screenless Dot except as a source of ideas.

Platform summary: OS replacement (LibreEcho, TECHO5, EchoMuse's emOS); keep Fire OS and replace the assistant client (echo-dot-assist, EchoLocal, EchoMuse's firmware mode); Android app or Android image (Kiosk Satellite, View Assist). All voice-satellite work targets HA. Our R-08 conclusion on licences stands: all hardware-facing ones are MIT except Kiosk Satellite and View Assist (non-commercial/no-derivatives) and EchoLocal's AGPL library.

## 4. Feature ideas for ClawDot

Use cases used as filters: **(A)** coding-agent events spoken plus voice reply; **(B)** alerts from webhooks and Discord-style sources; **(C)** timers, alarms, reminders; **(D)** ask anything with a bring-your-own model; **(E)** Spotify control, Bluetooth speaker, radio. Budget filter: [spec 0002](../specs/0002-resource-budget.md). "Base" means the Dot image (TECHO5 preferred) already provides it, so ClawDot should use it through the ESPHome link and not rebuild it.

### 4.1 Adopt from the base (do not build; use or document)

| Idea | Use case | Why |
|---|---|---|
| Wake word on device, custom wake models, echo cancellation, noise suppression, end-of-speech detection | all | The base does the DSP and the wake word; ClawDot receives audio or events over ESPHome. Do not build AEC for the Dot (the PC and Pi cases still use our own audio path). |
| Stop/cancel without the wake word | A, C | TECHO5's stop word and button ladder (cancel turn, silence sound, pause music) is exactly this; ClawDot only needs to react to a cancelled turn. |
| Follow-up listening and the second-wake-word slot | A, D | TECHO5 implements follow-up (0 to 30 s) and two wake words; ClawDot must keep the session open when the base asks for a follow-up (needed for "yes, resume the agent"). |
| LED ring states and effects per step (wake, thinking, replying) | A, B | TECHO5 exposes `voice_effect` and per-state effects as entities; ClawDot should set an "inbox pending" effect through that, not draw the ring. |
| Hardware buttons and mute as events | all | Base exposes them as entities/events. Mute stays the base's job (privacy claim comes from the hardware latch). |
| Bluetooth speaker sink and Dot-to-Bluetooth-speaker, AirPlay, Spotify Connect receiver, Sendspin | E | TECHO5 documents Bluetooth audio both ways and Spotify Connect (librespot; with a documented upstream key failure). This covers "Spotify on the Dot" and "Bluetooth speaker" without ClawDot decoding anything. Document, do not build. The parked "Spotify Connect receiver" row can be closed as "provided by the base". |
| Internet radio and a sleep timer | E | Built into TECHO5 and LibreEcho; ClawDot only needs to be able to say "play radio X" through the base's media-player entity. |
| Signed A/B OTA, rollback, recovery, Wi-Fi change, remote shell, factory reset | ops | Base-image concerns. Our job is the install guide and the firewall rule. |
| Settings export/import and copying between devices **for the device** | ops | TECHO5 has a signed copy mechanism that deliberately excludes secrets; use it for device settings. |
| Device-level quiet hours / do-not-disturb switch | A, B | Exists as TECHO5 entities. ClawDot should read it (see Build). |
| Device-local timers and alarms as a fallback while ClawDot is down | C | TECHO5 keeps timers, alarms and reminders working with no HA and no brain. |

### 4.2 Build (fits the differentiator)

| Idea | Use case | Why |
|---|---|---|
| **Quiet hours and DND for agent announcements**, with priority bypass (needs-input over completed) and a "held" queue read out later | A, B | No project has this. Their DND silences HA announcements, not agent events, and none ranks events by priority. Cheap: a policy in the inbox, reading the base's DND entity where present. |
| **Spoken source and kind on every event, with a configurable "digest" mode** | A, B | The core of the inbox; not offered by any compared project. |
| **Voice-turn history, off by default, RAM-only, clearable**, with a portal view | A, D | Both TECHO5 and LibreEcho keep the last 10 turns in RAM; match that privacy default, add the agent session id and event source so a turn is traceable to the inbox item. |
| **Log viewer and diagnostics bundle** in the portal with secret and transcript redaction | ops | Every compared project has it; ours should redact tokens, webhook secrets and spoken text by default, as TECHO5's log and diagnostics do. |
| **Settings export/import** for ClawDot's config (providers, tools, skills, inbox sources), secrets excluded and rotated on import | ops | TECHO5's rule "keys and privacy switches never travel" is the right model; our portal already has per-group saving. |
| **Signed OTA with rollback for the ClawDot binary** (not the OS) | ops | The base updates the OS; the ClawDot binary needs its own signed, atomic, roll-back-capable update on the Dot, the Pi and the PC. Same trust model as the base (public key pinned), not a copy of their code. |
| **Assistant-slot routing**: second wake word (or a spoken prefix) to ClawDot, first wake word to the base's own assistant, so the base remains a working helper | A, D | TECHO5 has two wake-word slots with an assistant each. This resolves the "who is the brain" question without a hard dependency: "Hey ClawDot" goes to the agent, the other word to TECHO5's built-in local assistant for timers and radio. Needs the B0 trial to confirm a non-HA client can own one slot. |
| **Dot-aware use of the base's timers and alarms**: ClawDot's scheduler handles agent-related reminders; plain alarms are created through the base's `alarm_set` ESPHome action | C | Avoids two clock-driven ringers on one speaker (see section 5). |
| **Wake-word near-miss capture**: opt-in ring of failed activations for tuning, never uploaded | wake | TECHO5 (wake-word clips, scores page) and EchoMuse have it. For the PC and Pi path (own wake engine) it is the quickest way to tune thresholds; on the Dot, read the base's score events instead. |
| **HTTPS option for the portal and webhook receiver** (self-signed or local CA, off by default) | A, B | Webhook secrets cross the LAN; TECHO5 and LibreEcho are plain HTTP for their pages. A single TLS option on our listener is cheap with `rustls`, already in the lockfile. |
| **Nearest-device-answers for inbox announcements** (one device speaks, based on presence or last use) | A, B | Differs from the base's "nearest device answers a question": it chooses where to speak an event. Only matters with two or more satellites. |

### 4.3 Maybe later

| Idea | Use case | Why later |
|---|---|---|
| **Realtime speech-to-speech provider** (user key, opt-in, minute cap) | D | TECHO5 shows the design and its costs (minute caps, no memory between turns). Fits "bring your own model" and a post-v1 provider type, but it bypasses our STT/LLM/TTS split and the tool loop. Reuse the same safety rule: nothing that changes state runs after reading outside content. |
| **ChatGPT-subscription sign-in as a provider** | D | LibreEcho does it via an undocumented backend; terms and stability are unknown. Watch, do not depend. |
| **Per-wake-word assistant profiles with different voices** | D | Already parked as "multiple assistant profiles"; TECHO5 shares one HA conversation across slots (its issue #328), a design trap to avoid. |
| **Call/intercom between devices, broadcast** | B | All projects have it; only useful once we have two satellites and a use case beyond announcements. Already "not planned". |
| **Wake-word training** | wake | EchoMuse has a trainer, TECHO5 partly; already parked. |
| **Multi-device central management** | ops | Only EchoMuse has a server; out of scope for one household. |
| **MCP/HTTP tool for base control** (volume, mute, radio, ring) | D, E | After B2: expose the ESPHome entities as a tool so the agent can say "turn the volume down". |

### 4.4 Skip

| Idea | Why |
|---|---|
| Screens, cameras, dashboards, photo frames, Lovelace cards, kiosk mode | Not a Dot; View Assist/Kiosk Satellite territory. |
| Vendor-cloud removal, firewalling Amazon, unlock tooling, recovery to stock | The base images own this; we link to them. |
| Whisper detection, presence/gesture, clap, baby monitor, nursery sounds | Not our product. |
| HA-specific features (last-turn sensors, HA blueprints, HACS dashboards) | Needs HA; a HACS integration is already parked. |
| Per-model speaker DSP, hardware AEC, echo-cancellation reference tuning | Base responsibility. |
| Browser-based installer, WebUSB wizard | Install flow belongs to the bases. |
| Voice-command "add-on library" as HA blueprints | Our equivalent is skills and packs; blueprints cannot be reused without HA. |

## 5. Candid overlap: what not to build, what becomes redundant, what is unique

### 5.1 Already covered by the bases, so do not build for the Dot

Wake word and DSP, audio ownership, LED ring, buttons and mute, Wi-Fi, OTA, rollback, recovery, remote shell, timers and alarms and reminders for plain household use, internet radio, Bluetooth and AirPlay and Spotify Connect, music through Music Assistant, history of the last few voice turns, quiet hours and DND for HA announcements, settings export, follow-up listening, stop word, second wake word.

### 5.2 Modules we have built (or planned) that become redundant **on the Dot** but stay needed for the PC and Pi

I could not inspect `crates/` (out of scope for this task), so this is by the architecture docs and the roadmap, and should be confirmed against the code:

- **Audio capture and playback, wake-word engine, VAD/end-of-speech, LED/button/mute "board" code** (`crates/board`, R-07 data): redundant on a Dot running TECHO5 (the base gives events and a media player over ESPHome); still required for the PC, the Pi and the frozen Fire OS developer path.
- **Local timer/alarm/reminder scheduler and ringing** (roadmap A7): redundant for plain timers on the Dot; **keep** it for the PC/Pi and as the home of agent-related reminders and the inbox's snooze/escalation. Decide which owns a ringing alarm on a Dot to avoid double-ringing.
- **Radio/media source** (milestone M5): redundant on the Dot for radio/Bluetooth/Spotify Connect; keep the control-shape interfaces for PC/Pi and for Spotify Web API control.
- **Local fast-path intents** (hassil-style subset): partly redundant on a Dot whose base catches "stop", volume and timers before the brain, but still needed for PC/Pi and for latency.

### 5.3 Genuinely unique to ClawDot (nothing in the seven projects does this)

1. **An agent inbox**: webhook/event ingestion from coding agents and similar sources, spoken with the source and kind, with priority and filtering.
2. **Voice replies that resume the originating agent session** (reply target, scoped tokens, resume command per harness).
3. **Bring-your-own agent/LLM as the brain** with an actual tool loop over **MCP and user-defined HTTP tools** and **skills** (the compared assistants have fixed device tools, or none).
4. **Hardware-agnostic satellite protocol and one Rust core** across PC, Pi and Dot, with a strict resource budget.
5. **A portal** with grouped, saved settings, event/source configuration and secret handling aimed at developers.
6. **A security model for untrusted spoken text** (inbox events, tool results, LLM output as data) that covers an *agent inbox*; TECHO5's "after reading the web, nothing changes state" rule is a good precedent that confirms the direction.

### 5.4 Risks the table surfaces

- **Licence traps**: Kiosk Satellite (CC BY-NC-ND), View Assist (CC BY-NC), EchoLocal's `libcountertop` (AGPL-3.0), LibreEcho's bundled non-commercial wake-word model. Do not depend on or bundle these.
- **Churn**: release cadences of days (TECHO5, LibreEcho, EchoLocal, echo-dot-assist) mean ESPHome behaviour can change under us; pin and test against a named base version.
- **Single-author, AI-assisted projects**: the table's author and TECHO5's author are the same person, TECHO5 and LibreEcho are about two to four months old, and some LibreEcho paths are on a branch not yet released (`without-home-assistant.md`). Plan for the base being swapped (EchoMuse, LibreEcho) by keeping the ESPHome link as the only coupling.
- **Open verification items** (not settled by this note): whether `echod` accepts a non-HA ESPHome client for voice, announce and entities (B0); whether TECHO5's DND silences an ESPHome-delivered announcement; whether LibreEcho's announce API can be combined with its ESPHome satellite for announce-then-listen; actual RAM/CPU on the Dot with ClawDot co-resident.

## 6. Suggested follow-ups (nothing applied here)

1. Run B0 with these specific checks: non-HA ESPHome client for voice subscription, announce with start-conversation, entity write for `voice_effect` and DND, `alarm_set` action call without HA.
2. Add the corrected sentence in 2.1 to R-08 and the roadmap B-phase text when those files are next edited (other branches own them now).
3. Consider LibreEcho's `POST /api/v1/audio/announce` as a second, simpler "speak only" backend for the ESPHome-free case.
4. Promote the Build rows in [future scope](../future-scope.md) one at a time, starting with "quiet hours and DND for agent announcements".

## Confidence

High on the provenance and on the TECHO5 and LibreEcho documentation claims (read directly; the cell notes in `docs/comparison/data.json` were consulted for their definitions). Medium on behaviour, since nothing was run and code was read only in part (`voice.go`, `llm_provider.h`, `timer_intent.h`). Low on maintenance figures for projects whose repositories I did not open (echo-dot-assist, EchoLocal, Kiosk Satellite, View Assist): those come from the GitHub search API and the table's data file.
