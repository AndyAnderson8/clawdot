# Music and radio

Voice control of music works the same way whichever backend produces the sound (REQ-MUS-001, REQ-MUS-003).
Every backend has a **shape**:

| Shape | Who plays the audio | Example |
|---|---|---|
| **Playback** | ClawDot, on its own speaker | Internet radio and plain HTTP streams (built in) |
| **Control** | A player elsewhere that ClawDot commands | Spotify Connect, MPD, Music Assistant, a Home Assistant media player |

## Built-in radio (playback shape)

List stations in the configuration file (REQ-MUS-002):

```json
"stations": [
  { "name": "jazz", "url": "http://stream.example.com/jazz" },
  { "name": "bbc world service", "url": "http://stream.example.com/bbc" }
]
```

Then, with no model call:

| You say | What happens |
|---|---|
| "play jazz", "put on the BBC world service" | The station sharing the most words with what you said starts; ties go to the earlier one in the list. |
| "play some music", "resume" (nothing chosen yet) | The first station starts. |
| "pause", "resume" | Pause and continue. |
| "next station", "go back" | Move through the list, wrapping around. |
| "stop" | Stops playback; "resume" continues the same station. |
| "play polka" (no such station) | "I don't have that station. I have jazz, bbc world service." |

With no stations configured, "play ..." goes to the language model like any other request, and `pause`/`resume`
produce plain player commands for a satellite that has its own player. With stations, the model also gets
`play_music` and `music_control` tools.

The core never decodes or plays audio. On a satellite the stream is played by the device itself: it declares
`media_url` in its hello and receives `clawdot.play-url` and `clawdot.media-control` ([satellite-server.md](satellite-server.md)).
It hands the satellite effects (`PlayStream { url, title }`,
`Media(Pause | Resume)`, `StopPlayback`) and the satellite plays the stream. **Pausing music during assistant speech and
announcements (REQ-MUS-004)**: the core sends the satellite `pause` before the speech and `resume` after it
(unless the reply itself changed the music, see [satellite-server.md](satellite-server.md)); the satellite does the
pausing, because only it owns the audio device.

## Control shape with HTTP tools

A control backend is a set of [HTTP tools](http-tools.md) that call the other player's API, so no core change is
needed. Audio plays on the controlled device, not on ClawDot's speaker (REQ-MUS-006). Sketch for a Home
Assistant media player; **these recipes are untested**, so check each call against your own setup first:

```json
"tools": [
  {
    "name": "tv_music",
    "description": "Control the living room speaker: play, pause, next, previous",
    "method": "POST",
    "url": "http://homeassistant.local:8123/api/services/media_player/{action}",
    "auth_env": "HA_TOKEN",
    "params": {
      "action": { "type": "string", "enum": ["media_play", "media_pause", "media_next_track", "media_previous_track"], "required": true },
      "entity_id": { "type": "string", "enum": ["media_player.living_room"], "required": true }
    },
    "confirm": false
  }
]
```

MPD and Music Assistant expose similar HTTP or JSON-RPC interfaces; Spotify's Web API needs OAuth tokens
that expire, which a fixed bearer token from the environment does not handle, so use a small sidecar that
refreshes the token and exposes a simple endpoint (see [the Spotify recipe](../recipes/spotify.md), which also explains why Spotify voice control is a policy problem).

## Not built yet

- Resume positions for podcasts and audiobooks (REQ-MUS-005).
- Search across a large library; the built-in backend only matches configured station names.
- A first-class control-shape backend type; for now it is expressed as HTTP tools.
