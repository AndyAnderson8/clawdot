//! Satellite protocol conformance harness (REQ-SAT-012, TASK-005).
//!
//! Two scripted peers speak the wire protocol with nothing else attached:
//!
//! - [`FakeSatellite`] connects to a core and plays the satellite's side, so a **core** can be checked.
//! - [`FakeCore`] listens for a satellite and plays the core's side, so a **satellite** (the reference client,
//!   or a port on real hardware pointed at [`FakeCore::addr`]) can be checked.
//!
//! Both are a [`Peer`]: a blocking socket with the protocol's own decoder, so tests read as a conversation
//! (`send`, then `expect`). The scenarios live in `tests/conformance_core.rs` and
//! `tests/conformance_satellite.rs`; each test names the requirement it covers. This is test support, not
//! shipped code, so it may panic on a failed expectation.
#![forbid(unsafe_code)]
#![allow(clippy::expect_used, clippy::panic)]

use clawdot_protocol::message::{Capabilities, Hello, Message, PROTOCOL_VERSION};
use clawdot_protocol::{Decoder, Frame, Limits, encode};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// How long `expect` waits before failing a test.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// One end of a protocol connection.
#[derive(Debug)]
pub struct Peer {
    sock: TcpStream,
    dec: Decoder,
    closed: bool,
}

impl Peer {
    fn new(sock: TcpStream) -> Self {
        sock.set_read_timeout(Some(Duration::from_millis(10)))
            .expect("set read timeout");
        Self {
            sock,
            dec: Decoder::new(Limits::default()),
            closed: false,
        }
    }

    /// Send one message.
    pub fn send(&mut self, m: &Message) {
        self.send_raw(&encode(&m.to_frame()).expect("encodable"));
    }

    /// Send a frame that is not a known message.
    pub fn send_frame(&mut self, f: &Frame) {
        self.send_raw(&encode(f).expect("encodable"));
    }

    /// Send bytes exactly as given (for malformed input).
    pub fn send_raw(&mut self, bytes: &[u8]) {
        // The peer may already have hung up (that is often the point of the test); ignore write errors.
        let _ = self.sock.write_all(bytes);
    }

    /// The next message within `wait`, or `None`. A frame the decoder rejects counts as the end of the
    /// conversation (`closed()` becomes true), as it does for a real peer.
    pub fn recv_within(&mut self, wait: Duration) -> Option<Message> {
        let end = Instant::now() + wait;
        let mut buf = [0u8; 8192];
        loop {
            match self.dec.next_frame() {
                Ok(Some(f)) => return Message::from_frame(f).ok(),
                Ok(None) => {}
                Err(_) => {
                    self.closed = true;
                    return None;
                }
            }
            if self.closed || Instant::now() >= end {
                return None;
            }
            match self.sock.read(&mut buf) {
                Ok(0) => self.closed = true,
                Ok(n) => self.dec.push(&buf[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => self.closed = true,
            }
        }
    }

    /// The next message, failing the test if none arrives.
    pub fn recv(&mut self) -> Message {
        self.recv_within(PATIENCE)
            .unwrap_or_else(|| panic!("no message from the peer (closed: {})", self.closed))
    }

    /// Skip messages until one satisfies `want`; fail if the peer goes quiet or hangs up first.
    pub fn expect(&mut self, what: &str, want: impl Fn(&Message) -> bool) -> Message {
        let end = Instant::now() + PATIENCE;
        while Instant::now() < end {
            match self.recv_within(Duration::from_millis(200)) {
                Some(m) if want(&m) => return m,
                Some(_) => {}
                None if self.closed => panic!("the peer hung up while waiting for {what}"),
                None => {}
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Assert that nothing matching `unwanted` arrives within `wait`.
    pub fn expect_none(&mut self, wait: Duration, unwanted: impl Fn(&Message) -> bool) {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            if let Some(m) = self.recv_within(Duration::from_millis(20)) {
                assert!(!unwanted(&m), "unexpected message: {m:?}");
            }
        }
    }

    /// Wait for the peer to hang up (or send something undecodable), draining whatever arrives first.
    pub fn expect_closed(&mut self) {
        let end = Instant::now() + PATIENCE;
        while Instant::now() < end {
            let _ = self.recv_within(Duration::from_millis(50));
            if self.closed {
                return;
            }
        }
        panic!("the peer did not close the connection");
    }

    /// Whether the peer has hung up.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed
    }
}

impl Drop for Peer {
    /// Hang up politely. Closing a TCP socket that still has unread input makes the OS send RST instead of FIN,
    /// and the other side's pending read then fails with `ECONNRESET` rather than seeing a clean end. Whether
    /// unread input exists depends on thread timing (e.g. a satellite's `wake` sent just after `welcome`), so a
    /// test that merely finishes a conversation would fail at random. Send FIN first, then drain until the other
    /// side closes (bounded), so the close is always orderly. Tests wanting an abrupt hang-up send malformed
    /// bytes instead.
    fn drop(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Write);
        let end = Instant::now() + Duration::from_millis(500);
        let mut buf = [0u8; 8192];
        while Instant::now() < end {
            match self.sock.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    }
}

/// The satellite's side of a conversation, connected to a core.
#[derive(Debug)]
pub struct FakeSatellite;

impl FakeSatellite {
    /// Connect to `addr`.
    #[must_use]
    pub fn connect(addr: SocketAddr) -> Peer {
        Peer::new(TcpStream::connect(addr).expect("connect to the core"))
    }

    /// A `hello` for a satellite with no optional capabilities (REQ-SAT-013).
    #[must_use]
    pub fn hello(device_id: &str, token: Option<&str>) -> Message {
        Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: device_id.to_owned(),
            name: None,
            token: token.map(str::to_owned),
            capabilities: Capabilities::default(),
        })
    }

    /// Connect, say hello, and require `welcome`.
    #[must_use]
    pub fn connect_and_welcome(addr: SocketAddr, token: Option<&str>) -> Peer {
        let mut p = Self::connect(addr);
        p.send(&Self::hello("fake", token));
        let m = p.recv();
        assert!(
            matches!(m, Message::Welcome(_)),
            "expected welcome, got {m:?}"
        );
        p
    }
}

/// The core's side of a conversation: a listener waiting for one satellite.
#[derive(Debug)]
pub struct FakeCore {
    listener: TcpListener,
}

impl FakeCore {
    /// Listen on a free loopback port.
    #[must_use]
    pub fn listen() -> Self {
        Self {
            listener: TcpListener::bind("127.0.0.1:0").expect("bind"),
        }
    }

    /// Where a satellite should connect.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("local address")
    }

    /// Wait for the satellite to connect and return its `hello`, with the peer to continue the conversation on.
    #[must_use]
    pub fn accept(&self) -> (Peer, Hello) {
        let (sock, _) = self.listener.accept().expect("accept");
        let mut p = Peer::new(sock);
        match p.recv() {
            Message::Hello(h) => (p, h),
            other => panic!("the first message from a satellite must be hello, got {other:?}"),
        }
    }
}
