//! A minimal WebSocket client (RFC 6455) over any byte stream: the upgrade handshake, text frames, ping,
//! pong, close, and fragmented text messages. It exists so the realtime feature adds no crate (ADR-0020).
//!
//! Bounded: a message over [`MAX_MESSAGE`] bytes or a handshake reply over [`MAX_HEAD`] bytes is an error.
//! Binary frames are ignored (the realtime protocol is JSON text). The server's `Sec-WebSocket-Accept` is
//! not checked: it would need SHA-1, and the TLS channel already authenticates the server.

use super::b64;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message accepted, in bytes. A realtime event is JSON text and a base64 audio chunk is far below it.
const MAX_MESSAGE: usize = 1024 * 1024;
/// Largest handshake reply accepted, in bytes.
const MAX_HEAD: usize = 16 * 1024;

// Frame layout and opcodes, RFC 6455 section 5.2.
/// First byte: this frame ends its message.
const FIN: u8 = 0x80;
/// First byte: the three reserved bits, which must be zero (no extension is negotiated).
const RESERVED: u8 = 0x70;
/// First byte: the opcode.
const OPCODE: u8 = 0x0f;
/// Opcodes with this bit set are control frames.
const CONTROL: u8 = 0x8;
/// Second byte: the payload is masked (clients mask, servers do not).
const MASK_BIT: u8 = 0x80;
/// Length field value: a 16-bit length follows.
const LEN_16: u8 = 126;
/// Length field value: a 64-bit length follows.
const LEN_64: u8 = 127;
/// The most a control frame may carry (section 5.5).
const MAX_CONTROL_PAYLOAD: u8 = 125;
const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xa;
/// The HTTP status of an accepted upgrade.
const SWITCHING_PROTOCOLS: u16 = 101;
/// Close status 1000, normal closure (section 7.4.1).
const CLOSE_NORMAL: u16 = 1000;

/// Why a WebSocket operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsError {
    /// The stream failed.
    Io(String),
    /// The server answered the upgrade with this HTTP status instead of 101.
    Refused(u16),
    /// The peer sent something that is not valid WebSocket.
    Protocol(&'static str),
    /// A message or reply was larger than the bound.
    TooLarge,
    /// The stream ended without a close frame.
    Eof,
}

/// A received message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// A complete text message.
    Text(String),
    /// A ping; answer it with [`WebSocket::send_pong`].
    Ping(Vec<u8>),
    /// The peer closed the connection.
    Close,
}

/// What the data frames received so far belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fragment {
    /// No message is half received.
    Idle,
    /// A text message is being assembled in `partial`.
    Text,
    /// A binary message (ignored) is being skipped.
    Binary,
}

/// A connected WebSocket.
#[derive(Debug)]
pub struct WebSocket<S> {
    io: S,
    buf: Vec<u8>,
    /// Clients mask what they send; servers (tests) do not.
    client: bool,
    partial: Vec<u8>,
    fragment: Fragment,
}

fn io_err(e: &std::io::Error) -> WsError {
    WsError::Io(e.kind().to_string())
}

/// The client's upgrade request. Everything that goes into the request line or a header is checked, so
/// nothing with a line break is ever sent.
fn upgrade_request(host: &str, path: &str, headers: &[(&str, &str)]) -> Result<String, WsError> {
    if host.contains(['\r', '\n']) || path.contains(['\r', '\n', ' ']) {
        return Err(WsError::Protocol("the address contains a line break"));
    }
    let mut key = [0u8; 16];
    getrandom::fill(&mut key).map_err(|_| WsError::Protocol("no random bytes for the key"))?;
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n",
        b64::encode(&key)
    );
    for (k, v) in headers {
        if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
            return Err(WsError::Protocol("a header contains a line break"));
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    Ok(req)
}

/// Perform the client handshake on `io` and return the connected socket. `headers` are extra request
/// headers (for example `Authorization`); a value with a line break is refused.
///
/// # Errors
/// Stream failures, a non-101 reply, or a reply that is not valid.
pub async fn connect<S: AsyncRead + AsyncWrite + Unpin>(
    mut io: S,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<WebSocket<S>, WsError> {
    let req = upgrade_request(host, path, headers)?;
    io.write_all(req.as_bytes()).await.map_err(|e| io_err(&e))?;
    io.flush().await.map_err(|e| io_err(&e))?;

    let mut buf = Vec::new();
    let end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            return Err(WsError::TooLarge);
        }
        let mut tmp = [0u8; 1024];
        let n = io.read(&mut tmp).await.map_err(|e| io_err(&e))?;
        if n == 0 {
            return Err(WsError::Eof);
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    check_upgrade_reply(&String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase())?;
    buf.drain(..end);
    Ok(WebSocket {
        io,
        buf,
        client: true,
        partial: Vec::new(),
        fragment: Fragment::Idle,
    })
}

/// Check the (lower-cased) reply head: status 101, and an upgrade to websocket.
fn check_upgrade_reply(head: &str) -> Result<(), WsError> {
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or(WsError::Protocol("not an HTTP reply"))?;
    if status != SWITCHING_PROTOCOLS {
        return Err(WsError::Refused(status));
    }
    if !head.contains("upgrade: websocket") {
        return Err(WsError::Protocol("the reply does not upgrade to websocket"));
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The header of one frame, read from the front of the receive buffer.
struct FrameHead {
    fin: bool,
    op: u8,
    mask: Option<[u8; 4]>,
    /// Where the payload begins in the buffer.
    data_at: usize,
    /// Payload length in bytes.
    len: usize,
}

impl FrameHead {
    /// Read the header at the front of `b`, or `None` while more bytes are needed. `partial` is how much of a
    /// fragmented message is already held. Hostile headers are refused from the header alone, before any
    /// payload is waited for.
    fn read(b: &[u8], partial: usize) -> Result<Option<Self>, WsError> {
        if b.len() < 2 {
            return Ok(None);
        }
        let fin = b[0] & FIN != 0;
        if b[0] & RESERVED != 0 {
            return Err(WsError::Protocol("reserved bits set"));
        }
        let op = b[0] & OPCODE;
        let short = b[1] & !MASK_BIT;
        // Control frames (close, ping, pong) are never fragmented and carry at most 125 bytes (RFC 6455
        // section 5.5).
        if op & CONTROL != 0 {
            if !fin {
                return Err(WsError::Protocol("a control frame was fragmented"));
            }
            if short > MAX_CONTROL_PAYLOAD {
                return Err(WsError::Protocol("a control frame was too long"));
            }
        }
        let Some((len, mut at)) = decode_len(b, short)? else {
            return Ok(None);
        };
        if len > MAX_MESSAGE || partial + len > MAX_MESSAGE {
            return Err(WsError::TooLarge);
        }
        let mask = if b[1] & MASK_BIT != 0 {
            let Some(m) = b.get(at..at + 4) else {
                return Ok(None);
            };
            at += 4;
            Some([m[0], m[1], m[2], m[3]])
        } else {
            None
        };
        Ok(Some(Self {
            fin,
            op,
            mask,
            data_at: at,
            len,
        }))
    }
}

/// The payload length of a frame whose header is at the front of `b` and whose 7-bit length field is
/// `short`, with the offset where the length field ends. `None` while the extended length is incomplete.
fn decode_len(b: &[u8], short: u8) -> Result<Option<(usize, usize)>, WsError> {
    Ok(Some(match short {
        LEN_16 => {
            let Some(n) = b.get(2..4) else {
                return Ok(None);
            };
            (usize::from(u16::from_be_bytes([n[0], n[1]])), 4)
        }
        LEN_64 => {
            let Some(n) = b.get(2..10) else {
                return Ok(None);
            };
            let mut be = [0u8; 8];
            be.copy_from_slice(n);
            let n = u64::from_be_bytes(be);
            if n > MAX_MESSAGE as u64 {
                return Err(WsError::TooLarge);
            }
            (n as usize, 10)
        }
        n => (usize::from(n), 2),
    }))
}

/// Apply (or remove) the 4-byte masking key of RFC 6455 section 5.3.
fn unmask(payload: &mut [u8], mask: [u8; 4]) {
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> WebSocket<S> {
    /// Wait for the next message. Cancel safe: dropping the future loses no bytes.
    ///
    /// # Errors
    /// Stream failures, invalid frames, oversized messages, or the stream ending without a close.
    pub async fn recv(&mut self) -> Result<Msg, WsError> {
        loop {
            if let Some(m) = self.parse()? {
                return Ok(m);
            }
            let mut tmp = [0u8; 4096];
            let n = self.io.read(&mut tmp).await.map_err(|e| io_err(&e))?;
            if n == 0 {
                return Err(WsError::Eof);
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }

    /// Parse one complete message out of the buffer, if there is one. Frames that carry no message (pong,
    /// binary) are consumed and the next one is tried.
    fn parse(&mut self) -> Result<Option<Msg>, WsError> {
        loop {
            let Some(head) = FrameHead::read(&self.buf, self.partial.len())? else {
                return Ok(None);
            };
            let end = head.data_at + head.len;
            if self.buf.len() < end {
                return Ok(None);
            }
            let mut payload = self.buf[head.data_at..end].to_vec();
            if let Some(mask) = head.mask {
                unmask(&mut payload, mask);
            }
            self.buf.drain(..end);
            if let Some(msg) = self.on_frame(head.fin, head.op, &payload)? {
                return Ok(Some(msg));
            }
        }
    }

    /// One whole frame arrived: the message it completes, if any.
    fn on_frame(&mut self, fin: bool, op: u8, payload: &[u8]) -> Result<Option<Msg>, WsError> {
        match op {
            OP_CLOSE => Ok(Some(Msg::Close)),
            OP_PING => Ok(Some(Msg::Ping(payload.to_vec()))),
            OP_PONG => Ok(None),
            OP_BINARY => {
                // Binary is ignored, but the rest of a fragmented binary message must not be read as text.
                if self.fragment == Fragment::Idle && !fin {
                    self.fragment = Fragment::Binary;
                }
                Ok(None)
            }
            OP_TEXT => {
                if self.fragment != Fragment::Idle {
                    return Err(WsError::Protocol("a new message began inside another"));
                }
                self.partial.extend_from_slice(payload);
                Ok(self.text_piece(fin)?.map(Msg::Text))
            }
            OP_CONTINUATION => match self.fragment {
                Fragment::Idle => Err(WsError::Protocol("a continuation with nothing to continue")),
                Fragment::Binary => {
                    if fin {
                        self.fragment = Fragment::Idle;
                    }
                    Ok(None)
                }
                Fragment::Text => {
                    self.partial.extend_from_slice(payload);
                    Ok(self.text_piece(fin)?.map(Msg::Text))
                }
            },
            _ => Err(WsError::Protocol("unknown opcode")),
        }
    }

    /// After a text frame was added to `partial`: the whole message if `fin` ended it, else wait for more.
    fn text_piece(&mut self, fin: bool) -> Result<Option<String>, WsError> {
        if !fin {
            self.fragment = Fragment::Text;
            return Ok(None);
        }
        self.fragment = Fragment::Idle;
        String::from_utf8(std::mem::take(&mut self.partial))
            .map(Some)
            .map_err(|_| WsError::Protocol("text is not UTF-8"))
    }

    async fn send_frame(&mut self, op: u8, payload: &[u8]) -> Result<(), WsError> {
        let mut f = Vec::with_capacity(payload.len() + 14);
        f.push(FIN | op);
        let mask_bit = if self.client { MASK_BIT } else { 0 };
        match payload.len() {
            n if n < usize::from(LEN_16) => f.push(mask_bit | n as u8),
            n if n <= usize::from(u16::MAX) => {
                f.push(mask_bit | LEN_16);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(mask_bit | LEN_64);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        if self.client {
            let mut m = [0u8; 4];
            getrandom::fill(&mut m)
                .map_err(|_| WsError::Protocol("no random bytes for the mask"))?;
            f.extend_from_slice(&m);
            f.extend(payload.iter().enumerate().map(|(i, b)| b ^ m[i % 4]));
        } else {
            f.extend_from_slice(payload);
        }
        self.io.write_all(&f).await.map_err(|e| io_err(&e))?;
        self.io.flush().await.map_err(|e| io_err(&e))
    }

    /// Send a text message.
    ///
    /// # Errors
    /// Stream failures.
    pub async fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        self.send_frame(OP_TEXT, text.as_bytes()).await
    }

    /// Answer a ping.
    ///
    /// # Errors
    /// Stream failures.
    pub async fn send_pong(&mut self, payload: &[u8]) -> Result<(), WsError> {
        let limit = usize::from(MAX_CONTROL_PAYLOAD);
        self.send_frame(OP_PONG, &payload[..payload.len().min(limit)])
            .await
    }

    /// Send a close frame (normal closure). The caller drops the socket afterwards.
    ///
    /// # Errors
    /// Stream failures.
    pub async fn send_close(&mut self) -> Result<(), WsError> {
        self.send_frame(OP_CLOSE, &CLOSE_NORMAL.to_be_bytes()).await
    }
}

/// The server half, for tests only: accept an upgrade and return the request head.
#[cfg(test)]
pub(crate) async fn accept<S: AsyncRead + AsyncWrite + Unpin>(
    mut io: S,
) -> Result<(WebSocket<S>, String), WsError> {
    let mut buf = Vec::new();
    let end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
        let mut tmp = [0u8; 1024];
        let n = io.read(&mut tmp).await.map_err(|e| io_err(&e))?;
        if n == 0 {
            return Err(WsError::Eof);
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    buf.drain(..end);
    io.write_all(
        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: x\r\n\r\n",
    )
    .await
    .map_err(|e| io_err(&e))?;
    Ok((
        WebSocket {
            io,
            buf,
            client: false,
            partial: Vec::new(),
            fragment: Fragment::Idle,
        },
        head,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test(flavor = "current_thread")]
    async fn handshake_text_and_close_work_both_ways_and_the_client_masks() {
        let (a, b) = duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (mut ws, head) = accept(b).await.unwrap();
            assert!(head.starts_with("GET /v1/realtime?model=m HTTP/1.1"));
            assert!(head.contains("Authorization: Bearer k\r\n"));
            assert_eq!(ws.recv().await.unwrap(), Msg::Text("hello".into()));
            ws.send_text(&"x".repeat(70_000)).await.unwrap();
            ws.send_close().await.unwrap();
        });
        let mut ws = connect(
            a,
            "example.test",
            "/v1/realtime?model=m",
            &[("Authorization", "Bearer k")],
        )
        .await
        .unwrap();
        ws.send_text("hello").await.unwrap();
        assert_eq!(ws.recv().await.unwrap(), Msg::Text("x".repeat(70_000)));
        assert_eq!(ws.recv().await.unwrap(), Msg::Close);
        server.await.unwrap();
    }

    async fn raw_server(reply: &'static [u8], frames: Vec<u8>) -> tokio::io::DuplexStream {
        let (a, mut b) = duplex(64 * 1024);
        tokio::spawn(async move {
            let mut tmp = [0u8; 2048];
            let _ = b.read(&mut tmp).await;
            let _ = b.write_all(reply).await;
            let _ = b.write_all(&frames).await;
            // Keep the stream open.
            let _ = b.read(&mut tmp).await;
        });
        a
    }

    const OK: &[u8] = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\r\n";

    #[tokio::test(flavor = "current_thread")]
    async fn a_refused_upgrade_reports_the_status() {
        let io = raw_server(b"HTTP/1.1 401 Unauthorized\r\n\r\n", vec![]).await;
        let e = connect(io, "h", "/", &[]).await.unwrap_err();
        assert_eq!(e, WsError::Refused(401));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_path_or_host_with_a_line_break_is_refused_even_without_headers() {
        for (host, path) in [("h", "/x\r\nInjected: 1"), ("h\r\nInjected: 1", "/")] {
            let (a, _b) = duplex(1024);
            let e = connect(a, host, path, &[]).await.unwrap_err();
            assert!(matches!(e, WsError::Protocol(_)), "{host:?} {path:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_header_with_a_line_break_is_refused_before_anything_is_sent() {
        let (a, _b) = duplex(1024);
        let e = connect(a, "h", "/", &[("X", "a\r\nInjected: 1")])
            .await
            .unwrap_err();
        assert!(matches!(e, WsError::Protocol(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fragments_are_joined_pings_surface_and_binary_is_ignored() {
        let mut frames = vec![0x01, 3, b'a', b'b', b'c']; // text, not final
        frames.extend([0x89, 1, 7]); // ping inside the message
        frames.extend([0x82, 2, 1, 2]); // binary: ignored
        frames.extend([0x80, 2, b'd', b'e']); // final continuation
        let io = raw_server(OK, frames).await;
        let mut ws = connect(io, "h", "/", &[]).await.unwrap();
        assert_eq!(ws.recv().await.unwrap(), Msg::Ping(vec![7]));
        assert_eq!(ws.recv().await.unwrap(), Msg::Text("abcde".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_pieces_of_a_fragmented_binary_message_are_not_read_as_text() {
        let mut frames = vec![0x02, 2, b'{', b'}']; // binary, not final
        frames.extend([0x80, 2, b'1', b'2']); // its final continuation
        frames.extend([0x81, 2, b'o', b'k']); // a real text message
        let io = raw_server(OK, frames).await;
        let mut ws = connect(io, "h", "/", &[]).await.unwrap();
        assert_eq!(ws.recv().await.unwrap(), Msg::Text("ok".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_continuation_with_nothing_to_continue_is_an_error() {
        let io = raw_server(OK, vec![0x80, 2, b'h', b'i']).await;
        let mut ws = connect(io, "h", "/", &[]).await.unwrap();
        assert!(matches!(ws.recv().await, Err(WsError::Protocol(_))));
    }

    /// A socket whose input buffer already holds `bytes`.
    fn holding(bytes: &[u8]) -> WebSocket<tokio::io::DuplexStream> {
        let (io, _peer) = duplex(8);
        WebSocket {
            io,
            buf: bytes.to_vec(),
            client: true,
            partial: Vec::new(),
            fragment: Fragment::Idle,
        }
    }

    #[test]
    fn truncated_zero_length_and_oversized_frames_are_handled() {
        // Every proper prefix of a frame with a 16-bit length waits for more bytes.
        let mut frame = vec![0x81, 126, 0, 5];
        frame.extend(b"hello");
        for n in 0..frame.len() {
            assert_eq!(holding(&frame[..n]).parse(), Ok(None), "prefix of {n}");
        }
        assert_eq!(holding(&frame).parse(), Ok(Some(Msg::Text("hello".into()))));
        // A zero-length final text frame is an empty message, not an error.
        assert_eq!(
            holding(&[0x81, 0]).parse(),
            Ok(Some(Msg::Text(String::new())))
        );
        // A 16-bit length can never exceed the bound, but a 64-bit one that does is refused before any
        // bytes of it have arrived, and so is one that does not fit in a usize.
        let mut huge = vec![0x81, 127];
        huge.extend(u64::MAX.to_be_bytes());
        assert_eq!(holding(&huge).parse(), Err(WsError::TooLarge));
        // Fragments that add up to more than the bound are refused as they arrive.
        let mut ws = holding(&[]);
        ws.partial = vec![b'a'; MAX_MESSAGE - 1];
        ws.fragment = Fragment::Text;
        ws.buf = vec![0x80, 2, b'b', b'c'];
        assert_eq!(ws.parse(), Err(WsError::TooLarge));
    }

    // RFC 6455 section 5.5
    #[test]
    fn control_frames_must_be_whole_and_at_most_125_bytes() {
        for op in [0x8_u8, 0x9, 0xa] {
            // Not final.
            assert_eq!(
                holding(&[op, 0]).parse(),
                Err(WsError::Protocol("a control frame was fragmented")),
                "op {op:x}"
            );
            // A 16-bit or 64-bit length form means more than 125 bytes: refused at once, payload or not.
            for head in [
                vec![0x80 | op, 126, 0, 126],
                vec![0x80 | op, 127, 0, 0, 0, 0, 0, 0, 0, 126],
            ] {
                assert_eq!(
                    holding(&head).parse(),
                    Err(WsError::Protocol("a control frame was too long")),
                    "op {op:x}"
                );
            }
        }
        // The longest allowed one is still read.
        let mut ping = vec![0x89, 125];
        ping.extend([7u8; 125]);
        assert_eq!(holding(&ping).parse(), Ok(Some(Msg::Ping(vec![7; 125]))));
        assert_eq!(holding(&[0x88, 0]).parse(), Ok(Some(Msg::Close)));
    }

    #[test]
    fn random_bytes_never_panic_the_frame_reader() {
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        };
        for _ in 0..3000 {
            let len = usize::from(next()) % 48;
            let bytes: Vec<u8> = (0..len).map(|_| next()).collect();
            let mut ws = holding(&bytes);
            for _ in 0..8 {
                if !matches!(ws.parse(), Ok(Some(_))) {
                    break;
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn frames_that_arrive_with_the_handshake_reply_are_kept() {
        let io = raw_server(OK, vec![0x81, 2, b'h', b'i']).await;
        let mut ws = connect(io, "h", "/", &[]).await.unwrap();
        assert_eq!(ws.recv().await.unwrap(), Msg::Text("hi".into()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversized_or_malformed_input_is_an_error_and_a_dropped_stream_is_eof() {
        let mut big = vec![0x81, 127];
        big.extend((MAX_MESSAGE as u64 + 1).to_be_bytes());
        let mut ws = connect(raw_server(OK, big).await, "h", "/", &[])
            .await
            .unwrap();
        assert_eq!(ws.recv().await, Err(WsError::TooLarge));

        let mut ws = connect(raw_server(OK, vec![0xc1, 0]).await, "h", "/", &[])
            .await
            .unwrap();
        assert!(matches!(ws.recv().await, Err(WsError::Protocol(_))));

        let mut ws = connect(
            raw_server(OK, vec![0x81, 2, 0xff, 0xfe]).await,
            "h",
            "/",
            &[],
        )
        .await
        .unwrap();
        assert!(matches!(ws.recv().await, Err(WsError::Protocol(_))));

        let (a, mut b) = duplex(1024);
        tokio::spawn(async move {
            let mut tmp = [0u8; 1024];
            let _ = b.read(&mut tmp).await;
            let _ = b.write_all(OK).await;
        });
        let mut ws = connect(a, "h", "/", &[]).await.unwrap();
        assert_eq!(ws.recv().await, Err(WsError::Eof));
    }
}
