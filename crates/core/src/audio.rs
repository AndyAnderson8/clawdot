//! 16-bit mono PCM helpers shared by the speech clients, the satellite and realtime voice.
//!
//! Inside ClawDot audio is 16 kHz mono 16-bit little-endian PCM ([`RATE`]). This module reads and writes that
//! as a WAV file, decodes raw sample bytes, and converts between sample rates. It has no I/O and no
//! dependencies.

/// Sample rate used inside ClawDot.
pub const RATE: u32 = 16_000;

/// Encode 16 kHz mono 16-bit samples as a WAV file.
#[must_use]
pub fn wav(pcm: &[i16]) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len() * 2).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(44 + pcm.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Read a WAV file that is already in ClawDot's format: PCM, mono, 16-bit, 16 kHz.
///
/// # Errors
/// A message saying what is wrong with the file.
pub fn parse_wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut pos = 12;
    let mut format_ok = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(bytes.len());
        if id == b"fmt " {
            let f = &bytes[body..end];
            if f.len() < 16 {
                return Err("the WAV format block is too short".into());
            }
            let tag = u16::from_le_bytes([f[0], f[1]]);
            let channels = u16::from_le_bytes([f[2], f[3]]);
            let rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            let bits = u16::from_le_bytes([f[14], f[15]]);
            if (tag, channels, rate, bits) != (1, 1, RATE, 16) {
                return Err(format!(
                    "the audio must be mono 16-bit PCM at 16000 Hz (this is format {tag}, {channels} channel(s), {rate} Hz, {bits}-bit)"
                ));
            }
            format_ok = true;
        } else if id == b"data" {
            if !format_ok {
                return Err("the WAV data comes before its format".into());
            }
            return Ok(decode_pcm16(&bytes[body..end]));
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    Err("the WAV file has no audio data".into())
}

/// Decode little-endian 16-bit samples; a trailing odd byte is dropped.
#[must_use]
pub fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

/// Linear resampling of 16-bit mono PCM, for example a 16 kHz microphone to 24 kHz.
#[must_use]
pub fn resample(pcm: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || pcm.is_empty() || from == 0 || to == 0 {
        return pcm.to_vec();
    }
    let n = (pcm.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * f64::from(from) / f64::from(to);
            let i0 = (pos as usize).min(pcm.len() - 1);
            let i1 = (i0 + 1).min(pcm.len() - 1);
            let frac = pos - i0 as f64;
            (f64::from(pcm[i0]) * (1.0 - frac) + f64::from(pcm[i1]) * frac).round() as i16
        })
        .collect()
}

/// Convert to 16 kHz by linear interpolation. Good enough for speech playback.
///
/// The same algorithm as [`resample`] with the output rate fixed, but the position is tracked in exact integer
/// arithmetic, so the odd rates a speech provider may return (22 050, 44 100 Hz) carry no floating-point drift
/// in the sample position. For the simple ratios (8, 24, 32, 48 kHz) the two give identical samples; for the others they can
/// differ by one in the last bit at an exact tie, which is why they have not been merged (merging would change
/// the bytes the speech providers' answers are turned into).
#[must_use]
pub fn resample_to_16k(samples: &[i16], from_rate: u32) -> Vec<i16> {
    if from_rate == RATE || samples.is_empty() || from_rate == 0 {
        return samples.to_vec();
    }
    let out_len = (samples.len() as u64 * u64::from(RATE) / u64::from(from_rate)) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as u64 * u64::from(from_rate);
        let idx = (pos / u64::from(RATE)) as usize;
        let frac = (pos % u64::from(RATE)) as f64 / f64::from(RATE);
        let a = f64::from(samples[idx.min(samples.len() - 1)]);
        let b = f64::from(samples[(idx + 1).min(samples.len() - 1)]);
        #[allow(clippy::cast_possible_truncation)]
        out.push((a + (b - a) * frac).round() as i16);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_changes_the_length_by_the_rate_ratio() {
        let pcm: Vec<i16> = (0..160).collect();
        assert_eq!(resample(&pcm, 16_000, 24_000).len(), 240);
        assert_eq!(
            resample(&resample(&pcm, 16_000, 24_000), 24_000, 16_000).len(),
            160
        );
        assert_eq!(resample(&pcm, 16_000, 16_000), pcm);
        assert!(resample(&[], 16_000, 24_000).is_empty());
    }

    #[test]
    fn the_two_converters_agree_for_the_simple_ratios() {
        let mut seed = 12345u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as i16
        };
        for rate in [8_000, 24_000, 32_000, 48_000] {
            for len in [1usize, 2, 3, 7, 160, 1001, 4800] {
                let v: Vec<i16> = (0..len).map(|_| next()).collect();
                assert_eq!(
                    resample_to_16k(&v, rate),
                    resample(&v, rate, RATE),
                    "{rate} {len}"
                );
            }
        }
    }
}
