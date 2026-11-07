# R-02: Providers (OpenRouter, STT, TTS)

Date: 2026-11-07 · Feeds: spec 0001 §4.1 (VOI), §4.2 (AST), REQ-NFR-002, provider traits

## Findings

**OpenRouter**
- Chat completions: `POST https://openrouter.ai/api/v1/chat/completions` with bearer auth; streaming (SSE) and tool calling are supported ([overview](https://openrouter.ai/docs/api-reference/overview)). Also a Responses API. Tool-calling support varies by model (filter by `supported_parameters=tools`).
- The API overview documents text and image input only. However, an [announcement dated 2026-05-01](https://openrouter.ai/announcements/announcing-audio-apis) describes dedicated **`/api/v1/audio/speech`** (TTS) and **`/api/v1/audio/transcriptions`** (STT) endpoints:
  - TTS providers reported: OpenAI GPT-4o Mini TTS, Google Gemini Flash TTS, Mistral Voxtral Mini TTS; output MP3 or PCM as a byte stream.
  - STT providers reported: OpenAI (Whisper, GPT-4o Transcribe, mini), Google Chirp 3, Groq-hosted Whisper; input as base64 JSON (`input_audio`) or multipart upload; optional language hint.
  - This is the announcement and search summaries only. Pricing, streaming behaviour, and the catalogue were not verified.
- **Consequence:** one OpenRouter key could cover STT, LLM, and TTS. That's a strong simplicity story for setup.

**Streaming STT/TTS landscape** (vendor-reported, treat as rough)
- Streaming STT vendors claim ~100-300 ms latency (Deepgram Flux, ElevenLabs Scribe Realtime, AssemblyAI). Streaming TTS vendors claim ~75-200 ms to first byte (ElevenLabs Flash, Cartesia, Deepgram Aura-2).
- Full speech-to-speech APIs exist (e.g. OpenAI Realtime) with ~1 s end-to-end claims. They bypass the cascaded pipeline and our freedom over tools and models, so they're out of scope. We stay cascaded.
- Sources disagree on rankings and nothing independent was found. Measure ourselves.

## Spec impact (recommended)

1. **`Stt` trait needs two modes:** *batch* (send the utterance after endpointing; fits OpenRouter's endpoint and OpenAI-compatible APIs) and *streaming* (WebSocket providers). Default to batch for simplicity, with streaming as an optional provider. Update REQ-VOI-003 (currently says "stream").
2. **Latency target is conditional.** With batch STT, STT time adds to the 1.5 s p50 target (REQ-NFR-002). Keep the target, but record which STT mode it was measured under, and measure in M1. Do not promise it for batch STT until measured.
3. **Define the provider interface as OpenAI-style HTTP** (`/chat/completions`, `/audio/transcriptions`, `/audio/speech`) with configurable base URL, so OpenRouter, OpenAI, Groq, and self-hosted servers all work. Streaming-WebSocket providers are adapters.
4. **TTS format:** request PCM where available to avoid on-device MP3/Opus decode cost; otherwise decode. Budget the decoder either way.
5. **Cost controls** (from R-00 repo review) become concrete: per-turn token caps and a daily spend/usage cap using provider usage fields.

## Confidence
OpenRouter chat/tool/stream: high (primary). OpenRouter audio endpoints: medium (announcement via search). Latency figures: low (vendor claims).
