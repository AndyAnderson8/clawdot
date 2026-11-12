# Voice pipeline

The pieces between a microphone and a speaker that do not depend on any particular hardware or provider. They are
pure logic, tested with synthetic audio and fake providers, so they run in CI with no audio device.

```
 wake word ─┐
 microphone ─┴─► Dialog ─► Endpointer ─► utterance ─► Transcriber ─► responder ─► Synthesizer ─► speaker
                  │                                                    (assistant)   (per sentence)
                  └─► cues: wake, listening, thinking, error, muted
```

| Part | Module | Requirement |
|---|---|---|
| Finds where an utterance ends in raw audio: adaptive noise floor, lead-in so the first word is not clipped, clicks ignored, pauses tolerated, no-speech timeout, maximum length | `vad` | REQ-VOI-002 |
| Dialog state machine: wake, listening, thinking, speaking; mute; half-duplex; follow-up window; late "thinking" cue | `voice::Dialog` | REQ-VOI-006, 007, 008, 010, 012 |
| One turn: transcribe, ask the assistant (`voice::hear`), synthesise a sentence at a time (`voice::voice_turn` for the console; the satellite session plays each sentence as it is made) | `voice`, `satellite/reply.rs` | REQ-VOI-003, 004 |

## Rules the dialog enforces for every satellite

- **Mute really mutes.** While muted the dialog asks for no audio (`wants_audio` is false), ignores the wake word,
  and drops any capture in progress. Muting and unmuting each produce a cue so the state is observable.
- **Half-duplex.** Audio is ignored while the assistant is thinking or speaking, and the wake word is ignored while
  it speaks, so it never hears itself. (A satellite that declares echo cancellation may opt into barge-in later;
  that is not built.) One exception while it is *thinking and silent*: a wake word opens a short capture that is
  checked for a stop phrase only, and the action button cancels at once (REQ-VOI-020, [agent-loop.md](agent-loop.md)).
  `interrupt_while_working` turns the capture off.
- **Follow-up window.** After a reply finishes, listening re-opens for `follow_up_secs` (default 8) without the
  wake word; after a question or a held action it stays open for `follow_up_question_secs` (default 15). If nobody
  speaks, the dialog goes idle with the error cue. Set `follow_up_secs` to 0 to turn this off.
- **Speech while working.** Plan and progress phrases from the assistant are played as they arrive without ending the
  "thinking" state, so the follow-up window opens only after the final answer (REQ-AST-020).
- **Thinking cue.** Played once if the reply takes longer than `thinking_delay_ms` (default 1000), and never if the
  reply starts sooner.

## Providers

`speech::HttpTranscriber` (batch upload of a WAV to `/audio/transcriptions`) and `speech::HttpSynthesizer`
(`/audio/speech` as raw PCM, resampled to 16 kHz) speak the OpenAI-compatible API that OpenAI, many hosted services,
and self-hosted servers such as Whisper and Piper wrappers provide. Audio is sensitive, so plain `http://` is
accepted only for localhost and private-network hosts (REQ-SEC-001); anything else needs `https://`. Audio is
never stored or logged, and errors never include it or the key. Verified against a fake server only; no real
provider has been called.

## Failure behaviour

- Nothing intelligible heard: no reply, the error cue.
- Speech recognition fails: a short spoken apology, and the assistant is not asked.
- Speech synthesis fails part-way: the text reply is kept, sentences already played stay played, the reply's
  audio stream is closed cleanly (no error cue, the follow-up window opens as usual), and the problem is logged
  (it never contains audio or credentials). If the first sentence fails nothing was played and the error cue shows.
- On a satellite the reply plays while it is still being made: see [speech-latency.md](speech-latency.md).

## Not built yet

- Wake-word detection (push-to-talk exists). The desktop satellite can feed live microphone audio and play the reply through
  external programs ([guides/pc-voice-loop.md](../guides/pc-voice-loop.md)), but it has only been exercised with scripted
  processes and recordings, not with a real microphone.
- Opt-in barge-in and adaptive end-of-utterance thresholds (REQ-VOI-005, REQ-VOI-011, both later milestones).
- Starting synthesis while the model is still streaming: replies are split into sentences after they are complete.
  (Plan and progress phrases are spoken as each model round returns; token streaming from the provider is not
  built. The design is in [speech-latency.md](speech-latency.md).)
- A local stop-word check, so that the stop capture does not cost a speech-to-text call, and "stop" over the
  assistant's own voice (barge-in).

## Trying it without a microphone

`clawdot --config clawdot.json --voice-in question.wav [--voice-out answer.wav]` answers one recorded utterance and
exits: it transcribes the file, answers with the same code as the text console (fast path, then the language
model and tools), and writes the spoken reply as a WAV. It prints what it heard, what it said, and any satellite
effects (for example a radio stream to start). The input must be mono 16-bit PCM at 16 kHz; other formats are
refused with a message saying what was found. The `speech` section of the configuration file is required:

```json
"speech": {
  "url": "https://api.openai.com/v1",
  "key_env": "OPENAI_API_KEY",
  "stt_model": "whisper-1",
  "tts_model": "tts-1",
  "voice": "alloy",
  "language": "en",
  "tts_sample_rate": 24000
}
```
