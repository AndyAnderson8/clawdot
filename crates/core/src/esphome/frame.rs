//! Framing for the ESPHome native API, and the connection ("link") that reads and writes messages
//! (spec 0004, REQ-ESP-003).
//!
//! Plaintext frame: `0x00`, payload length (varint), message type (varint), payload. A frame that declares
//! more than [`MAX_PAYLOAD`] bytes, or whose varints are malformed, is fatal: the caller closes the connection
//! and does not try to resynchronise.

use super::proto::{Msg, read_varint};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest payload accepted from a device.
pub const MAX_PAYLOAD: usize = 64 * 1024;
/// The varints of a plaintext header are at most this long (a value of up to 32 bits).
const MAX_HEADER_VARINT: usize = 5;
/// How much is read from the socket at a time.
const READ_CHUNK: usize = 4096;

/// Encode one plaintext frame.
#[must_use]
pub fn plain_frame(ty: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 11);
    out.push(0);
    super::proto::put_varint(&mut out, payload.len() as u64);
    super::proto::put_varint(&mut out, u64::from(ty));
    out.extend_from_slice(payload);
    out
}

/// Take one plaintext frame from the front of `buf`, if a whole one has arrived.
///
/// # Errors
/// A message when the bytes cannot be a frame (REQ-ESP-003).
pub fn take_plain(buf: &mut Vec<u8>) -> Result<Option<(u32, Vec<u8>)>, String> {
    match buf.first() {
        None => return Ok(None),
        Some(0) => {}
        Some(1) => {
            return Err("the device requires Noise encryption; set `psk_env` for it".into());
        }
        Some(b) => {
            return Err(format!(
                "not an ESPHome plaintext frame (first byte {b:#04x})"
            ));
        }
    }
    let body = &buf[1..];
    let Some((len, n1)) = header_varint(body)? else {
        return Ok(None);
    };
    let Some((ty, n2)) = header_varint(&body[n1..])? else {
        return Ok(None);
    };
    let len = usize::try_from(len).map_err(|_| "frame length overflow".to_owned())?;
    if len > MAX_PAYLOAD {
        return Err(format!(
            "frame of {len} bytes exceeds the {MAX_PAYLOAD}-byte limit"
        ));
    }
    let ty = u32::try_from(ty).map_err(|_| "message type overflow".to_owned())?;
    let start = 1 + n1 + n2;
    if buf.len() < start + len {
        return Ok(None);
    }
    let payload = buf[start..start + len].to_vec();
    buf.drain(..start + len);
    Ok(Some((ty, payload)))
}

fn header_varint(b: &[u8]) -> Result<Option<(u64, usize)>, String> {
    match read_varint(b) {
        Ok(Some((v, n))) if n <= MAX_HEADER_VARINT => Ok(Some((v, n))),
        Ok(Some(_)) | Err(_) => Err("malformed length or type in a frame header".into()),
        Ok(None) if b.len() >= MAX_HEADER_VARINT => {
            Err("malformed length or type in a frame header".into())
        }
        Ok(None) => Ok(None),
    }
}

/// How bytes become messages on one connection.
#[derive(Debug)]
enum Mode {
    Plain,
    #[cfg(feature = "esphome-noise")]
    Noise(Box<super::noise::Transport>),
}

/// A connection to a device: framing plus the socket. Reading keeps its partial state inside the value, so
/// [`Link::recv`] is safe to use as a `select!` branch.
#[derive(Debug)]
pub struct Link<IO> {
    io: IO,
    rx: Vec<u8>,
    mode: Mode,
}

impl<IO: AsyncRead + AsyncWrite + Unpin> Link<IO> {
    /// A plaintext link.
    pub fn plain(io: IO) -> Self {
        Self {
            io,
            rx: Vec::new(),
            mode: Mode::Plain,
        }
    }

    /// A link that has finished a Noise handshake.
    #[cfg(feature = "esphome-noise")]
    pub(super) fn noise(io: IO, rx: Vec<u8>, t: super::noise::Transport) -> Self {
        Self {
            io,
            rx,
            mode: Mode::Noise(Box::new(t)),
        }
    }

    fn take(&mut self) -> Result<Option<(u32, Vec<u8>)>, String> {
        match &mut self.mode {
            Mode::Plain => take_plain(&mut self.rx),
            #[cfg(feature = "esphome-noise")]
            Mode::Noise(t) => match super::noise::take_record(&mut self.rx)? {
                Some(rec) => t.open(&rec).map(Some),
                None => Ok(None),
            },
        }
    }

    /// The next message, or `None` when the device closed the connection.
    ///
    /// # Errors
    /// A message for a read failure, a framing violation, or a malformed message.
    pub async fn recv(&mut self) -> Result<Option<Msg>, String> {
        loop {
            if let Some((ty, payload)) = self.take()? {
                return Msg::decode(ty, &payload)
                    .map(Some)
                    .map_err(|e| format!("message type {ty}: {e}"));
            }
            let mut tmp = [0u8; READ_CHUNK];
            let n = self
                .io
                .read(&mut tmp)
                .await
                .map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Ok(None);
            }
            self.rx.extend_from_slice(&tmp[..n]);
        }
    }

    /// The underlying socket, for tests that need to send bytes that are not valid messages.
    pub fn io_mut(&mut self) -> &mut IO {
        &mut self.io
    }

    /// Send one message.
    ///
    /// # Errors
    /// A message for a write failure or an unsendable message.
    pub async fn send(&mut self, m: &Msg) -> Result<(), String> {
        let (ty, payload) = m.encode();
        let bytes = match &mut self.mode {
            Mode::Plain => plain_frame(ty, &payload),
            #[cfg(feature = "esphome-noise")]
            Mode::Noise(t) => t.seal(ty, &payload)?,
        };
        self.io
            .write_all(&bytes)
            .await
            .map_err(|e| format!("write: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-ESP-003
    #[test]
    fn req_esp_003_a_frame_round_trips_and_waits_for_the_rest() {
        let f = plain_frame(7, b"abc");
        assert_eq!(f, [0, 3, 7, b'a', b'b', b'c']);
        let mut buf = f[..4].to_vec();
        assert_eq!(take_plain(&mut buf).unwrap(), None);
        buf.extend_from_slice(&f[4..]);
        buf.extend_from_slice(&plain_frame(8, b""));
        assert_eq!(take_plain(&mut buf).unwrap(), Some((7, b"abc".to_vec())));
        assert_eq!(take_plain(&mut buf).unwrap(), Some((8, vec![])));
        assert_eq!(take_plain(&mut buf).unwrap(), None);
    }

    // REQ-ESP-003
    #[test]
    fn req_esp_003_bad_frames_are_fatal() {
        let big = {
            let mut b = vec![0u8];
            super::super::proto::put_varint(&mut b, (MAX_PAYLOAD + 1) as u64);
            super::super::proto::put_varint(&mut b, 7);
            b
        };
        for (bytes, needle) in [
            (vec![1u8, 0, 0], "Noise"),
            (vec![0x47], "not an ESPHome"),
            (vec![0, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01], "malformed"),
            (vec![0, 1, 0x80, 0x80, 0x80, 0x80, 0x80], "malformed"),
            (big, "limit"),
        ] {
            let mut buf = bytes;
            let e = take_plain(&mut buf).unwrap_err();
            assert!(e.contains(needle), "{e}");
        }
    }

    // REQ-ESP-003: random bytes never panic, whatever they say.
    #[test]
    fn req_esp_003_random_bytes_never_panic() {
        let mut x: u64 = 0x1234_5678_9abc_def1;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..20_000 {
            let len = (next() % 40) as usize;
            let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if next() % 2 == 0 && !buf.is_empty() {
                buf[0] = 0;
            }
            while let Ok(Some(_)) = take_plain(&mut buf) {}
        }
    }

    #[tokio::test]
    async fn recv_reads_messages_split_across_reads() {
        let (a, mut b) = tokio::io::duplex(64);
        let mut link = Link::plain(a);
        let (t, p) = Msg::PingRequest.encode();
        let frame = plain_frame(t, &p);
        tokio::spawn(async move {
            for byte in frame {
                b.write_all(&[byte]).await.unwrap();
            }
            b.shutdown().await.unwrap();
        });
        assert_eq!(link.recv().await.unwrap(), Some(Msg::PingRequest));
        assert_eq!(link.recv().await.unwrap(), None);
    }
}
