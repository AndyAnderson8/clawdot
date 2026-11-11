# Speech providers: OpenAI style, ElevenLabs and Deepgram

ClawDot turns your voice into text and its replies into speech through a speech provider. By default that is any
server that copies OpenAI's speech API (OpenAI itself, a local Whisper or Kokoro server, the Piper helper in
[local-speech.md](local-speech.md)). `speech.provider` lets you pick a service with a different API instead.
Requirements: REQ-VOI-021 to REQ-VOI-024 in [spec 0001](../specs/0001-product-requirements.md).

| Provider | `speech.provider` | Speech to text | Text to speech | Voice list |
|---|---|---|---|---|
| OpenAI style (default) | `openai` | `POST /audio/transcriptions` | `POST /audio/speech` | the server's own list when it has one, OpenAI's fixed set for OpenAI |
| ElevenLabs | `elevenlabs` | Scribe, `POST /v1/speech-to-text` | `POST /v1/text-to-speech/{voice_id}` | `GET /v2/voices`, including your cloned voices, by name |
| Deepgram | `deepgram` | Nova, `POST /v1/listen` | Aura, `POST /v1/speak` | a built-in set (Deepgram has no list endpoint) |

Each provider does both directions with one key, so there is one setting for the lot. Mixing (for example ElevenLabs
voices with a local Whisper) is not supported yet; see "Not yet" below.

The default `openai` provider behaves exactly as it did before this setting existed.

## Set it up

1. Make an account with the provider and create an API key in its dashboard. Both are paid services with a small
   free allowance; check the provider's current plans, and set a spending limit on the provider's side.
2. In the configuration file:

   ```json
   { "speech": { "provider": "elevenlabs" } }
   ```

   or save `speech_provider` from the portal (it needs a restart, because the speech clients are built at start). No
   `url`, models or voice are needed: each provider brings its own, and anything you set yourself wins.
3. Give ClawDot the key, either way:
   - paste it on the portal's voice settings page (`PUT /api/speech/key`). It is stored in the private secrets file
     in the data directory, never shown again (only a hint such as `sk_ab12…9f3c`), never logged, and applies to the
     next request without a restart. A pasted key wins over the variable.
   - or set an environment variable where ClawDot runs: `ELEVENLABS_API_KEY` or `DEEPGRAM_API_KEY` (or name your own
     with `speech.key_env`).
4. Pick a voice from the list the portal fetches (`GET /api/voices`). For ElevenLabs the voice is the voice id; the
   list shows each voice's name and sends its id.

If you change provider, change the voice too: a voice from one service means nothing to another.

## Defaults and options

| | ElevenLabs | Deepgram |
|---|---|---|
| Address (`speech.url`) | `https://api.elevenlabs.io` (regional hosts work too) | `https://api.deepgram.com` |
| Speech to text model (`speech.stt_model`) | `scribe_v2` | `nova-3` |
| Speech model (`speech.tts_model`) | `eleven_flash_v2_5` (fast, cheaper) | not used: the voice is the model |
| Voice (`speech.voice`) | `21m00Tcm4TlvDq8ikWAM` (a premade voice) | `aura-2-thalia-en` |
| Key variable (`speech.key_env`) | `ELEVENLABS_API_KEY` | `DEEPGRAM_API_KEY` |
| Audio asked for | raw PCM, `pcm_24000` | raw PCM, `linear16`, 24000 Hz, no container |
| Spoken language (`speech_language`) | sent as a two letter code (`pt-BR` becomes `pt`) | sent as is; empty asks Deepgram to detect it |

`speech.tts_sample_rate` (default 24000) is the rate asked of the provider; ClawDot converts whatever it gets to the
16 kHz mono it uses inside. ElevenLabs accepts 8000, 16000, 22050, 24000 and 44100; Deepgram 8000, 16000, 24000, 32000
and 48000. Another value stops start-up with a message naming the choices. Some ElevenLabs plans limit the higher
rates; 24000 is the safe choice.

If you set `speech.url` for these providers, leave off a trailing `/v1`; it is tolerated either way. Plain `http://` is
still refused for anything that is not on your own network (REQ-SEC-001); audio never leaves the device unencrypted.

## Cloned voices and consent

ElevenLabs can make a voice from a short recording of someone speaking (Instant Voice Cloning, and a professional
tier). You do that in ElevenLabs' own app, not in ClawDot, and the voice then appears in the voice list under the
name you gave it, marked with its category (`cloned` or `professional`). ClawDot never records, uploads or trains a
voice.

**Only clone a voice of someone who has agreed to it**, ideally in writing: yourself, or a family member who knows
what it will be used for. Never clone a voice to imitate someone else, to mislead a person, or to make a message
sound as if it came from them. ElevenLabs requires you to confirm you have the rights and consent to clone a voice,
and its terms apply to anything you make; read them. If the person withdraws their agreement, delete the voice in
ElevenLabs. Without that, ClawDot's replies in that voice are speech the person never said.

## When something goes wrong

The messages are plain and never include the key:

| You see | Meaning | Do |
|---|---|---|
| `No key is set for ElevenLabs...` | nothing pasted and the variable is empty | paste the key, or set the variable and restart |
| `HTTP 401: the key was not accepted...` | wrong key, or a key for a different service | paste the whole key again; ElevenLabs and Deepgram keys are not interchangeable |
| `HTTP 402: the plan has no credit or characters left` | the quota is used up | top up or wait for the plan to renew |
| `HTTP 429: too many requests...` | too many at once, or the plan's limit | wait a little; reduce use; upgrade the plan |
| `... is not a name this service accepts` | the voice contains characters that are not allowed in an id | choose a voice from the list |
| voice list: `Could not get the list of voices` | the service could not be reached or refused | the voice can still be typed |

## Proven and not proven

Everything here is tested against fake servers on the loopback address that answer the way the providers' public API
references say they do: the request address, headers and body, the reading of the answer, cloned voices across pages,
a refused key, a rate limit, and conversion of 24 kHz, 22.05 kHz and 48 kHz audio to 16 kHz.

**It has never been run against the real ElevenLabs or Deepgram services.** The first real run may find a difference,
for example in the exact error text, in which plans allow which sample rate, in the Scribe model names, or whether
Deepgram's language detection works for the chosen model. Treat the first run as a trial, and report what differs.

## Not yet

- Different providers for listening and speaking (for example a local Whisper with ElevenLabs voices). For now, the
  one provider does both.
- Streaming audio while it is generated (both providers support it); replies are fetched whole, a sentence at a time,
  as with the OpenAI style.
- Cloning or managing voices from ClawDot.
- Changing the provider without a restart.
- A provider with a different API than these three: add a dialect in `crates/core/src/speech.rs` (see `Dialect`)
  with tests against a fake server.
