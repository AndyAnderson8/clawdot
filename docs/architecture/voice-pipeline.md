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
| One turn: transcribe, ask the assistant, synthesise a sentence at a time | `voice::voice_turn` | REQ-VOI-003, 004 |

## Rules the dialog enforces for every satellite

- **Mute really mutes.** While muted the dialog asks for no audio (`wants_audio` is false), ignores the wake word,
  and drops any capture in progress. Muting and unmuting each produce a cue so the state is observable.
- **Half-duplex.** Audio and wake events are ignored while the assistant is thinking or speaking, so it never hears
  itself. (A satellite that declares echo cancellation may opt into barge-in later; that is not built.)
- **Follow-up window.** After a reply finishes, listening re-opens for `follow_up_secs` (default 8) without the
  wake word. If nobody speaks, the dialog goes idle with the error cue. Set it to 0 to turn this off.
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
- Speech synthesis fails part-way: the text reply is kept, sentences already synthesised can still play, and the
  problem is recorded (it never contains audio or credentials).

## Not built yet

- Provider settings in the configuration file and the satellite that uses them.
- Wake-word detection, audio capture and playback, and the satellite itself.
- Opt-in barge-in and adaptive end-of-utterance thresholds (REQ-VOI-005, REQ-VOI-011, both later milestones).
- Starting synthesis while the model is still streaming: replies are split into sentences after they are complete.
