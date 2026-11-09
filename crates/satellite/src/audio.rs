//! The audio interface: how a satellite reaches a microphone and a speaker (REQ-VOI-013).
//!
//! Two small traits, [`AudioIn`] and [`AudioOut`], hide whatever produces and consumes PCM. Backends:
//!
//! * [`crate::command`]: spawn an external program (`ffmpeg`, `sox`, `arecord`, `aplay`, `ffplay`) and pass audio
//!   through its standard input and output (REQ-VOI-014, REQ-VOI-015).
//! * [`WavIn`] and [`WavOut`] here: a recording in, a recording out, for tests and headless runs (REQ-VOI-016).
//!
//! The format is fixed: 16 kHz, mono, signed 16-bit samples, in 20 ms frames of [`FRAME_SAMPLES`].

use std::fmt;
use std::time::Duration;

/// Samples in one 20 ms frame at 16 kHz.
pub const FRAME_SAMPLES: usize = 320;
/// Bytes in one frame of little-endian 16-bit samples.
pub const FRAME_BYTES: usize = FRAME_SAMPLES * 2;
/// Sample rate of every backend.
pub const SAMPLE_RATE: u32 = 16_000;

/// Why audio did not flow. Each cause is distinct so a caller never mistakes a fault for silence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// The program (or every candidate program) is not installed. The text says what was looked for.
    NotFound(String),
    /// The program exists but could not be started.
    Spawn(String),
    /// The source ended: a recording ran out, or the capture program exited.
    Eof,
    /// The device or program produced or accepted nothing for too long and was stopped.
    Stalled(String),
    /// Reading, writing or the program's exit failed.
    Io(String),
    /// A call was made out of order (reading before starting).
    NotStarted,
    /// A recording is not in the supported format.
    Format(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m) | Self::Spawn(m) | Self::Io(m) | Self::Format(m) => f.write_str(m),
            Self::Eof => f.write_str("the audio source ended"),
            Self::Stalled(m) => write!(f, "audio stalled: {m}"),
            Self::NotStarted => f.write_str("audio was not started"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A source of 16 kHz mono 16-bit microphone audio.
pub trait AudioIn {
    /// Open the device. Idempotent: starting a started input does nothing.
    ///
    /// # Errors
    /// The program is missing or cannot start.
    fn start(&mut self) -> Result<(), AudioError>;

    /// The next 20 ms frame, waiting at most `wait`. `Ok(None)` means none yet.
    ///
    /// # Errors
    /// [`AudioError::Eof`] once the source has ended and every frame was read, [`AudioError::Stalled`] if
    /// nothing arrived for too long, [`AudioError::NotStarted`] before `start`.
    fn read_frame(&mut self, wait: Duration) -> Result<Option<Vec<i16>>, AudioError>;

    /// Release the device and discard anything buffered. Idempotent.
    fn stop(&mut self);
}

/// A sink for 16 kHz mono 16-bit reply audio. One reply is `begin`, many `play`, then `finish`.
pub trait AudioOut {
    /// Prepare to play a reply.
    ///
    /// # Errors
    /// The program is missing or cannot start.
    fn begin(&mut self) -> Result<(), AudioError>;

    /// Queue audio. Returns quickly; playback happens in the background.
    ///
    /// # Errors
    /// The player has gone away, or too much audio is queued.
    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError>;

    /// No more audio for this reply: what is queued plays out. Does not wait.
    fn finish(&mut self);

    /// Whether everything queued has finished playing (`Ok(true)`), or is still playing (`Ok(false)`). With
    /// nothing playing it is `Ok(true)`.
    ///
    /// # Errors
    /// The player stalled or failed; it has been stopped and the next call reports idle.
    fn poll_done(&mut self) -> Result<bool, AudioError>;

    /// Silence now and discard what is queued. Idempotent.
    fn stop(&mut self);

    /// What this output can do beyond playing samples.
    fn capabilities(&self) -> crate::hw::OutputCaps {
        crate::hw::OutputCaps::default()
    }

    /// Reduce the output level to `level` percent of normal (100 restores it). A backend that cannot may
    /// ignore this.
    fn duck(&mut self, _level: u8) {}

    /// Set the speaker volume, 0 to 100. A backend that cannot may ignore this.
    fn set_volume(&mut self, _level: u8) {}
}

impl<T: AudioIn + ?Sized> AudioIn for Box<T> {
    fn start(&mut self) -> Result<(), AudioError> {
        (**self).start()
    }
    fn read_frame(&mut self, wait: Duration) -> Result<Option<Vec<i16>>, AudioError> {
        (**self).read_frame(wait)
    }
    fn stop(&mut self) {
        (**self).stop();
    }
}

impl<T: AudioOut + ?Sized> AudioOut for Box<T> {
    fn begin(&mut self) -> Result<(), AudioError> {
        (**self).begin()
    }
    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        (**self).play(pcm)
    }
    fn finish(&mut self) {
        (**self).finish();
    }
    fn poll_done(&mut self) -> Result<bool, AudioError> {
        (**self).poll_done()
    }
    fn stop(&mut self) {
        (**self).stop();
    }
    fn capabilities(&self) -> crate::hw::OutputCaps {
        (**self).capabilities()
    }
    fn duck(&mut self, level: u8) {
        (**self).duck(level);
    }
    fn set_volume(&mut self, level: u8) {
        (**self).set_volume(level);
    }
}

/// Parse a WAV file in the uplink format: mono 16-bit PCM at 16 kHz.
///
/// # Errors
/// [`AudioError::Format`] saying what is wrong.
pub fn parse_wav(bytes: &[u8]) -> Result<Vec<i16>, AudioError> {
    let bad = |m: &str| AudioError::Format(m.to_owned());
    if bytes.len() < 44 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a WAV file"));
    }
    let mut pos = 12;
    let mut ok = false;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match &bytes[pos..pos + 4] {
            b"fmt " if end - body >= 16 => {
                let f = &bytes[body..end];
                let (tag, ch) = (
                    u16::from_le_bytes([f[0], f[1]]),
                    u16::from_le_bytes([f[2], f[3]]),
                );
                let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
                let bits = u16::from_le_bytes([f[14], f[15]]);
                if (tag, ch, rate, bits) != (1, 1, SAMPLE_RATE, 16) {
                    return Err(bad("the recording must be mono 16-bit PCM at 16000 Hz"));
                }
                ok = true;
            }
            b"data" if ok => {
                return Ok(bytes[body..end]
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]))
                    .collect());
            }
            _ => {}
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    Err(bad("no audio data in the WAV file"))
}

/// The 44-byte header of a WAV stream. `data_len` of `u32::MAX` marks a stream of unknown length.
#[must_use]
pub fn wav_header(data_len: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out
}

/// A complete WAV file.
#[must_use]
pub fn encode_wav(pcm: &[i16]) -> Vec<u8> {
    let data = u32::try_from(pcm.len().saturating_mul(2)).unwrap_or(u32::MAX - 36);
    let mut out = wav_header(data);
    out.reserve(pcm.len() * 2);
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Microphone audio from a recording, once: it is read from the start and ends with [`AudioError::Eof`].
/// The last partial frame is padded with silence, and `tail_silence_ms` of silence follows so the endpointer
/// sees the end of the utterance.
#[derive(Debug, Clone)]
pub struct WavIn {
    pcm: Vec<i16>,
    pos: usize,
    started: bool,
}

impl WavIn {
    /// A source that plays `pcm` then `tail_silence_ms` of silence.
    #[must_use]
    pub fn new(mut pcm: Vec<i16>, tail_silence_ms: usize) -> Self {
        pcm.extend(std::iter::repeat_n(0i16, tail_silence_ms * 16));
        let pad = (FRAME_SAMPLES - pcm.len() % FRAME_SAMPLES) % FRAME_SAMPLES;
        pcm.extend(std::iter::repeat_n(0i16, pad));
        Self {
            pcm,
            pos: 0,
            started: false,
        }
    }

    /// Read a WAV file (mono 16-bit PCM at 16 kHz).
    ///
    /// # Errors
    /// The file cannot be read or is in another format.
    pub fn open(path: &std::path::Path, tail_silence_ms: usize) -> Result<Self, AudioError> {
        let bytes = std::fs::read(path)
            .map_err(|e| AudioError::Io(format!("cannot read {}: {e}", path.display())))?;
        Ok(Self::new(parse_wav(&bytes)?, tail_silence_ms))
    }
}

impl AudioIn for WavIn {
    fn start(&mut self) -> Result<(), AudioError> {
        self.started = true;
        Ok(())
    }

    fn read_frame(&mut self, _wait: Duration) -> Result<Option<Vec<i16>>, AudioError> {
        if !self.started {
            return Err(AudioError::NotStarted);
        }
        if self.pos >= self.pcm.len() {
            return Err(AudioError::Eof);
        }
        let frame = self.pcm[self.pos..self.pos + FRAME_SAMPLES].to_vec();
        self.pos += FRAME_SAMPLES;
        Ok(Some(frame))
    }

    fn stop(&mut self) {
        self.started = false;
    }
}

/// Reply audio collected in memory and, if a path is given, written as a WAV file when each reply finishes.
#[derive(Debug, Default)]
pub struct WavOut {
    path: Option<std::path::PathBuf>,
    /// Everything played so far.
    pub samples: Vec<i16>,
    playing: bool,
    /// The last problem writing the file.
    pub write_error: Option<String>,
}

impl WavOut {
    /// Collect in memory only.
    #[must_use]
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Collect and write `path` at the end of each reply.
    #[must_use]
    pub fn to_file(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
            ..Self::default()
        }
    }
}

impl AudioOut for WavOut {
    fn begin(&mut self) -> Result<(), AudioError> {
        self.playing = true;
        Ok(())
    }

    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        if !self.playing {
            return Err(AudioError::NotStarted);
        }
        self.samples.extend_from_slice(pcm);
        Ok(())
    }

    fn finish(&mut self) {
        self.playing = false;
        if let Some(p) = &self.path {
            self.write_error = std::fs::write(p, encode_wav(&self.samples))
                .err()
                .map(|e| format!("cannot write {}: {e}", p.display()));
        }
    }

    fn poll_done(&mut self) -> Result<bool, AudioError> {
        Ok(!self.playing)
    }

    fn stop(&mut self) {
        self.playing = false;
    }

    fn capabilities(&self) -> crate::hw::OutputCaps {
        crate::hw::OutputCaps {
            stop: true,
            ..crate::hw::OutputCaps::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_voi_016_a_recording_round_trips_through_wav() {
        let pcm: Vec<i16> = (0..1000).map(|i| (i * 7 - 3000) as i16).collect();
        assert_eq!(parse_wav(&encode_wav(&pcm)).unwrap(), pcm);
    }

    #[test]
    fn req_voi_016_other_formats_are_refused_with_a_reason() {
        let mut w = encode_wav(&[1, 2, 3]);
        w[24..28].copy_from_slice(&44_100u32.to_le_bytes());
        assert!(matches!(parse_wav(&w), Err(AudioError::Format(m)) if m.contains("16000")));
        assert!(matches!(parse_wav(b"nope"), Err(AudioError::Format(_))));
    }

    #[test]
    fn req_voi_016_wav_in_yields_whole_frames_then_eof() {
        let mut src = WavIn::new(vec![5; 400], 40);
        assert_eq!(src.read_frame(Duration::ZERO), Err(AudioError::NotStarted));
        src.start().unwrap();
        let mut frames = 0;
        let mut samples = Vec::new();
        loop {
            match src.read_frame(Duration::ZERO) {
                Ok(Some(f)) => {
                    assert_eq!(f.len(), FRAME_SAMPLES);
                    samples.extend(f);
                    frames += 1;
                }
                Err(AudioError::Eof) => break,
                other => panic!("{other:?}"),
            }
        }
        // 400 samples plus 640 of tail silence is 1040, padded to 4 frames of 320.
        assert_eq!(frames, 4);
        assert_eq!(&samples[..400], &[5; 400][..]);
        assert!(samples[400..].iter().all(|&s| s == 0));
    }

    #[test]
    fn req_voi_016_wav_out_collects_and_reports_idle() {
        let mut out = WavOut::in_memory();
        assert_eq!(out.poll_done(), Ok(true));
        assert_eq!(out.play(&[1]), Err(AudioError::NotStarted));
        out.begin().unwrap();
        assert_eq!(out.poll_done(), Ok(false));
        out.play(&[1, 2]).unwrap();
        out.play(&[3]).unwrap();
        out.finish();
        assert_eq!(out.poll_done(), Ok(true));
        assert_eq!(out.samples, vec![1, 2, 3]);
    }
}
