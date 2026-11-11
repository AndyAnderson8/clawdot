# Proving pass, 2026-11-11

What was run for real, outside the unit tests, on `dev` at `276771f`, and what it showed. This is step 1 of the [architecture review](2026-11-11-architecture-review.md) (finding R1). "Real" here means the released binary, a real network, a real device or somebody else's client or service. Nothing in this pass cost money.

## Results

| What | How it was run | Result |
|---|---|---|
| **Size of the Dot build** | armv7 musl release build, twice | 3,247,568 bytes with the features the Dot uses; 3,348,880 with `hive` and `realtime` added (about 101 KB more). The budget is 8 MB. |
| **Memory on the Dot** | `/proc/<pid>/status` on the unit, idle, after the new build started | 2.95 MB resident (peak the same), one thread. The unit has 481 MB. |
| **The Dot itself** | `top` on the unit | The base image's voice service uses about 19% of a core at idle and the load average sits near 6.5; ClawDot is not the cause. |
| **Deploy over SSH** | `scripts/dev/dot-push.sh` with the full build | Hash checked on the device, `--version` ran, the old binary was kept, the portal answered again. |
| **Hive discovery on a real network** | A second ClawDot on the Dot with `--hive-discovery native`; a separate mDNS client written in Node on another computer | The Dot answered with the service, port, device id and address. It shares UDP 5353 with the image's own voice service. |
| **Two speakers finding each other** | Two ClawDot processes on the Dot, each asked for `/api/hive/peers` | Each listed the other within seconds, with its id, address and port. Identity files were created with private permissions. |
| **MCP server with a real client** | The official MCP Inspector command-line client against a core on a PC | Handshake and `tools/list` worked; `announce` was queued and spoken (markup in the text was defused); `ask_user` with nobody answering returned `no_answer` after its time limit; a wrong token was refused. |
| **Model list from a real provider** | `POST /api/models` for OpenRouter | 466 models. A made-up key against OpenAI gave the plain "the service did not accept that key" message with status 401. |
| **Local speech servers** | `scripts/dev/local-speech.ps1 test` | Text was spoken by Piper and transcribed back by whisper. Four voices are listed. |
| **Memory search** | The running binary over HTTP | Five memories added, search by word and by question found the right one, a memory that looked like a key was refused, limits report 500 and 300. |
| **Skill folder import** | The running binary over HTTP, a folder with a `SKILL.md`, a text file and a shell script | Imported turned off and marked for review, with its description and source; the script was left out with a reason; turning it on was refused until it was accepted, then worked. |

## Problems found

1. **The Dot loses its voice when the PC restarts.** The speech servers on the PC do not start by themselves, so after a restart the speaker cannot hear or talk until someone starts them. They were down when this pass began.
2. **An imported skill cannot be turned on from the portal.** The core now refuses until the skill is accepted, and the portal has no Accept step yet (review finding P1).
3. **A question from `ask_user` is spoken with a stray full stop**: "Shall I deploy?. What would you like to do?".
4. **The hive guide was wrong about the Dot.** It said the image runs `avahi-daemon`; it does not. The guide is corrected in this change.
5. **The speech-to-text model hears "ClawDot" as "Quad".** Worth knowing for the name people say and for any wake word built on it.
6. **The hive is not started on the Dot.** It needs a flag in the supervisor's command line, and a peer's name shows only if that speaker publishes it.

## Still not proven

| What | Why not |
|---|---|
| A spoken turn on the Dot with this build | Needs a person to speak to it. |
| Realtime voice against the real service | Needs an OpenAI key and spends money. |
| ElevenLabs and Deepgram | Need keys and spend money. |
| `ask_user` answered by voice, end to end | Needs a speaker and a person. |
| A real agent harness (Hermes) as the MCP client | Not installed for this pass; the Inspector stood in for it. |
| Hive discovery between two separate devices | Only one speaker exists; both test speakers ran on the same unit. |
| The portal pages on the Dot in a browser | Needs the admin password. |
| The `avahi` discovery mode | No system here runs the daemon. |

## The landing gate (review finding R2)

Not changed in this pass. Switching `scripts/gate-mode` back to `full` is itself a change to the policy scripts, which sets off the policy tests, and those take up to 1 h 45 min on Windows (finding R3). Making those tests fast should come first; then the switch is a short landing.
