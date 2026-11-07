//! Wyoming-compatible framing (ADR-0008, REQ-SAT-001).
//!
//! A frame is one line of JSON (the header), then an optional JSON data block, then an optional binary
//! payload. The header is `{"type": "...", "data": {...}?, "data_length": n?, "payload_length": n?}`.
//! Everything is length-limited so a hostile peer cannot make a receiver allocate without bound
//! (REQ-NFR-110, REQ-SEC-004).

use serde_json::{Map, Value};
use std::fmt;

/// One protocol message on the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// Event name, for example `audio-chunk` or `clawdot.hello`.
    pub kind: String,
    /// Event fields.
    pub data: Map<String, Value>,
    /// Raw bytes that follow the header (audio, for example).
    pub payload: Vec<u8>,
}

impl Frame {
    /// A frame with no data and no payload.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            data: Map::new(),
            payload: Vec::new(),
        }
    }

    /// Replace the data fields.
    #[must_use]
    pub fn with_data(mut self, data: Map<String, Value>) -> Self {
        self.data = data;
        self
    }

    /// Attach a binary payload.
    #[must_use]
    pub fn with_payload(mut self, payload: Vec<u8>) -> Self {
        self.payload = payload;
        self
    }
}

/// Hard bounds applied while decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest permitted header line, in bytes (excluding the newline).
    pub max_header: usize,
    /// Longest permitted trailing data block.
    pub max_data: usize,
    /// Longest permitted binary payload.
    pub max_payload: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header: 16 * 1024,
            max_data: 64 * 1024,
            max_payload: 1024 * 1024,
        }
    }
}

/// Why a frame could not be encoded or decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The header line exceeded [`Limits::max_header`].
    HeaderTooLong,
    /// The declared data block exceeded [`Limits::max_data`].
    DataTooLong(usize),
    /// The declared payload exceeded [`Limits::max_payload`].
    PayloadTooLong(usize),
    /// The header or data block was not valid JSON.
    InvalidJson(String),
    /// A part that must be a JSON object was something else.
    NotAnObject(&'static str),
    /// The header had no usable `type`.
    MissingType,
    /// A length field was negative, fractional, or not a number.
    InvalidLength(&'static str),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooLong => write!(f, "header line too long"),
            Self::DataTooLong(n) => write!(f, "data block too long ({n} bytes)"),
            Self::PayloadTooLong(n) => write!(f, "payload too long ({n} bytes)"),
            Self::InvalidJson(e) => write!(f, "invalid JSON: {e}"),
            Self::NotAnObject(what) => write!(f, "{what} must be a JSON object"),
            Self::MissingType => write!(f, "frame has no type"),
            Self::InvalidLength(what) => write!(f, "invalid {what}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Serialise a frame. Data is carried inline in the header; the payload follows the newline.
///
/// # Errors
/// Returns [`FrameError::MissingType`] for an empty kind.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, FrameError> {
    if frame.kind.is_empty() {
        return Err(FrameError::MissingType);
    }
    let mut header = Map::new();
    header.insert("type".into(), Value::String(frame.kind.clone()));
    if !frame.data.is_empty() {
        header.insert("data".into(), Value::Object(frame.data.clone()));
    }
    if !frame.payload.is_empty() {
        header.insert("payload_length".into(), Value::from(frame.payload.len()));
    }
    // Compact JSON never contains a raw newline, so the header is always exactly one line.
    let mut out = serde_json::to_vec(&Value::Object(header))
        .map_err(|e| FrameError::InvalidJson(e.to_string()))?;
    out.push(b'\n');
    out.extend_from_slice(&frame.payload);
    Ok(out)
}

#[derive(Debug)]
struct Pending {
    kind: String,
    data: Map<String, Value>,
    data_len: usize,
    payload_len: usize,
}

/// Incremental decoder: feed bytes with [`push`](Decoder::push), pull frames with
/// [`next_frame`](Decoder::next_frame). It does no I/O, so it works with sync and async transports alike.
///
/// After any error the stream is desynchronised; the decoder refuses further input and the caller
/// must close the connection.
#[derive(Debug)]
pub struct Decoder {
    limits: Limits,
    buf: Vec<u8>,
    pending: Option<Pending>,
    failed: Option<FrameError>,
}

impl Decoder {
    /// A decoder enforcing `limits`.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            buf: Vec::new(),
            pending: None,
            failed: None,
        }
    }

    /// Append received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.failed.is_none() {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// Bytes received but not yet consumed.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// The next complete frame, `Ok(None)` if more bytes are needed.
    ///
    /// # Errors
    /// Any framing violation; the error is sticky.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        match self.step() {
            Ok(frame) => Ok(frame),
            Err(e) => {
                self.failed = Some(e.clone());
                self.buf.clear();
                self.pending = None;
                Err(e)
            }
        }
    }

    fn step(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.pending.is_none() {
            let Some(newline) = self.buf.iter().position(|&b| b == b'\n') else {
                if self.buf.len() > self.limits.max_header {
                    return Err(FrameError::HeaderTooLong);
                }
                return Ok(None);
            };
            if newline > self.limits.max_header {
                return Err(FrameError::HeaderTooLong);
            }
            let pending = parse_header(&self.buf[..newline], &self.limits)?;
            self.buf.drain(..=newline);
            self.pending = Some(pending);
        }

        let Some(p) = self.pending.as_ref() else {
            return Ok(None);
        };
        let need = p.data_len + p.payload_len;
        if self.buf.len() < need {
            return Ok(None);
        }
        let Some(p) = self.pending.take() else {
            return Ok(None);
        };

        let mut data = p.data;
        if p.data_len > 0 {
            let extra: Value = serde_json::from_slice(&self.buf[..p.data_len])
                .map_err(|e| FrameError::InvalidJson(e.to_string()))?;
            let Value::Object(extra) = extra else {
                return Err(FrameError::NotAnObject("data block"));
            };
            for (k, v) in extra {
                data.insert(k, v);
            }
        }
        let payload = self.buf[p.data_len..need].to_vec();
        self.buf.drain(..need);
        Ok(Some(Frame {
            kind: p.kind,
            data,
            payload,
        }))
    }
}

fn parse_header(line: &[u8], limits: &Limits) -> Result<Pending, FrameError> {
    let value: Value =
        serde_json::from_slice(line).map_err(|e| FrameError::InvalidJson(e.to_string()))?;
    let Value::Object(mut obj) = value else {
        return Err(FrameError::NotAnObject("header"));
    };
    let kind = match obj.remove("type") {
        Some(Value::String(s)) if !s.is_empty() => s,
        _ => return Err(FrameError::MissingType),
    };
    let data = match obj.remove("data") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err(FrameError::NotAnObject("data")),
    };
    let data_len = length(
        obj.get("data_length"),
        "data_length",
        limits.max_data,
        FrameError::DataTooLong,
    )?;
    let payload_len = length(
        obj.get("payload_length"),
        "payload_length",
        limits.max_payload,
        FrameError::PayloadTooLong,
    )?;
    Ok(Pending {
        kind,
        data,
        data_len,
        payload_len,
    })
}

fn length(
    value: Option<&Value>,
    name: &'static str,
    max: usize,
    too_long: fn(usize) -> FrameError,
) -> Result<usize, FrameError> {
    match value {
        None | Some(Value::Null) => Ok(0),
        Some(Value::Number(n)) => {
            let n = n.as_u64().ok_or(FrameError::InvalidLength(name))?;
            let n = usize::try_from(n).map_err(|_| FrameError::InvalidLength(name))?;
            if n > max { Err(too_long(n)) } else { Ok(n) }
        }
        Some(_) => Err(FrameError::InvalidLength(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Frame {
        let mut data = Map::new();
        data.insert("rate".into(), json!(16000));
        data.insert("name".into(), json!("hey"));
        Frame::new("audio-chunk")
            .with_data(data)
            .with_payload(vec![1, 2, 3, 4, 5])
    }

    fn decode_all(bytes: &[u8], chunk: usize) -> Result<Vec<Frame>, FrameError> {
        let mut d = Decoder::new(Limits::default());
        let mut out = Vec::new();
        for piece in bytes.chunks(chunk.max(1)) {
            d.push(piece);
            while let Some(f) = d.next_frame()? {
                out.push(f);
            }
        }
        Ok(out)
    }

    #[test]
    fn round_trips() {
        let bytes = encode(&sample()).unwrap();
        assert_eq!(decode_all(&bytes, bytes.len()).unwrap(), vec![sample()]);
    }

    #[test]
    fn survives_any_chunking() {
        let bytes = encode(&sample()).unwrap();
        for chunk in 1..=bytes.len() {
            assert_eq!(
                decode_all(&bytes, chunk).unwrap(),
                vec![sample()],
                "chunk {chunk}"
            );
        }
    }

    #[test]
    fn decodes_back_to_back_frames() {
        let mut bytes = encode(&sample()).unwrap();
        bytes.extend(encode(&Frame::new("ping")).unwrap());
        let frames = decode_all(&bytes, 3).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].kind, "ping");
    }

    #[test]
    fn accepts_trailing_data_block_and_merges_it_over_header_data() {
        let block = br#"{"b":2,"a":9}"#;
        let header = format!(
            "{{\"type\":\"x\",\"data\":{{\"a\":1}},\"data_length\":{}}}\n",
            block.len()
        );
        let mut bytes = header.into_bytes();
        bytes.extend_from_slice(block);
        let frames = decode_all(&bytes, 4).unwrap();
        assert_eq!(frames[0].data["a"], json!(9));
        assert_eq!(frames[0].data["b"], json!(2));
    }

    #[test]
    fn empty_kind_cannot_be_encoded() {
        assert_eq!(encode(&Frame::new("")), Err(FrameError::MissingType));
    }

    #[test]
    fn rejects_oversized_declared_payload_without_buffering_it() {
        let mut d = Decoder::new(Limits::default());
        d.push(b"{\"type\":\"x\",\"payload_length\":999999999999}\n");
        assert!(matches!(d.next_frame(), Err(FrameError::PayloadTooLong(_))));
    }

    #[test]
    fn rejects_header_without_newline_once_too_long() {
        let mut d = Decoder::new(Limits {
            max_header: 32,
            ..Limits::default()
        });
        d.push(&[b'a'; 64]);
        assert_eq!(d.next_frame(), Err(FrameError::HeaderTooLong));
    }

    #[test]
    fn rejects_malformed_headers() {
        for bad in [
            &b"not json\n"[..],
            b"[1,2]\n",
            b"{}\n",
            b"{\"type\":\"\"}\n",
            b"{\"type\":5}\n",
            b"{\"type\":\"x\",\"payload_length\":-1}\n",
            b"{\"type\":\"x\",\"payload_length\":1.5}\n",
            b"{\"type\":\"x\",\"payload_length\":\"3\"}\n",
            b"{\"type\":\"x\",\"data\":[1]}\n",
        ] {
            let mut d = Decoder::new(Limits::default());
            d.push(bad);
            assert!(
                d.next_frame().is_err(),
                "accepted {:?}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn errors_are_sticky_and_drop_buffered_input() {
        let mut d = Decoder::new(Limits::default());
        d.push(b"garbage\n");
        let first = d.next_frame().unwrap_err();
        d.push(&encode(&Frame::new("ping")).unwrap());
        assert_eq!(d.next_frame().unwrap_err(), first);
        assert_eq!(d.buffered(), 0);
    }

    #[test]
    fn random_garbage_never_panics_or_buffers_without_bound() {
        let limits = Limits::default();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let mut d = Decoder::new(limits);
            let len = (next() % 600) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();
            for piece in bytes.chunks(1 + (next() % 13) as usize) {
                d.push(piece);
                let _ = d.next_frame();
                assert!(
                    d.buffered() <= limits.max_header + limits.max_data + limits.max_payload + 600
                );
            }
        }
    }
}
