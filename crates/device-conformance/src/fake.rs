//! Test doubles for the suite: a recording speaker, a shared text buffer, and a scripted core.

use clawdot_device::daemon::DaemonDevice;
use clawdot_protocol::message::{AudioCodec, Hello, Message, PROTOCOL_VERSION, Welcome};
use clawdot_protocol::{Decoder, Limits, encode};
use clawdot_satellite::audio::{AudioError, AudioOut};
use clawdot_satellite::{Config, DEAD_AFTER, Report};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// A speaker that records everything played and counts stops.
#[derive(Clone, Default)]
pub struct Rec {
    /// Samples played, in order.
    pub played: Arc<Mutex<Vec<i16>>>,
    /// Times `stop` was called.
    pub stops: Arc<Mutex<usize>>,
}

impl std::fmt::Debug for Rec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rec").finish_non_exhaustive()
    }
}

impl Rec {
    /// A copy of what was played.
    #[must_use]
    pub fn samples(&self) -> Vec<i16> {
        self.played.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// How many times the speaker was stopped.
    #[must_use]
    pub fn stop_count(&self) -> usize {
        self.stops.lock().map_or(0, |s| *s)
    }

    /// Forget what was played.
    pub fn clear(&self) {
        if let Ok(mut p) = self.played.lock() {
            p.clear();
        }
    }
}

impl AudioOut for Rec {
    fn begin(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        if let Ok(mut p) = self.played.lock() {
            p.extend_from_slice(pcm);
        }
        Ok(())
    }

    fn finish(&mut self) {}

    fn poll_done(&mut self) -> Result<bool, AudioError> {
        Ok(true)
    }

    fn stop(&mut self) {
        if let Ok(mut s) = self.stops.lock() {
            *s += 1;
        }
    }
}

/// A shared byte buffer that implements `Write`, for capturing console LED lines.
#[derive(Clone, Default)]
pub struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::fmt::Debug for SharedBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBuf").finish_non_exhaustive()
    }
}

impl SharedBuf {
    /// Everything written so far.
    #[must_use]
    pub fn text(&self) -> String {
        self.0
            .lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    }
}

impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut v) = self.0.lock() {
            v.extend_from_slice(b);
        }
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The core's end of one connection, handed to a script.
pub struct CoreConn {
    sock: TcpStream,
    dec: Decoder,
    /// The satellite's `hello`.
    pub hello: Hello,
}

impl std::fmt::Debug for CoreConn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreConn").finish_non_exhaustive()
    }
}

impl CoreConn {
    /// Send a message.
    ///
    /// # Errors
    /// The connection is gone.
    pub fn send(&mut self, m: &Message) -> Result<(), String> {
        let bytes = encode(&m.to_frame()).map_err(|e| e.to_string())?;
        self.sock.write_all(&bytes).map_err(|e| e.to_string())
    }

    /// The next message, or `None` if none arrives within `wait`.
    ///
    /// # Errors
    /// The satellite hung up or sent something undecodable.
    pub fn next(&mut self, wait: Duration) -> Result<Option<Message>, String> {
        let end = Instant::now() + wait;
        loop {
            if let Some(f) = self.dec.next_frame().map_err(|e| e.to_string())? {
                return Message::from_frame(f).map(Some).map_err(|e| e.to_string());
            }
            if Instant::now() >= end {
                return Ok(None);
            }
            let mut buf = [0u8; 8192];
            match self.sock.read(&mut buf) {
                Ok(0) => return Err("the daemon hung up".into()),
                Ok(n) => self.dec.push(&buf[..n]),
                Err(_) => {}
            }
        }
    }

    /// Read until a message satisfies `pred`, answering pings, or fail after `wait`.
    ///
    /// # Errors
    /// Nothing matched in time, or the connection failed.
    pub fn expect(
        &mut self,
        wait: Duration,
        mut pred: impl FnMut(&Message) -> bool,
    ) -> Result<Message, String> {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            match self.next(Duration::from_millis(50))? {
                Some(Message::Ping) => self.send(&Message::Pong)?,
                Some(m) if pred(&m) => return Ok(m),
                _ => {}
            }
        }
        Err("timed out waiting for the expected message".into())
    }

    /// Collect every message that arrives within `wait`, answering pings.
    ///
    /// # Errors
    /// The connection failed.
    pub fn collect(&mut self, wait: Duration) -> Result<Vec<Message>, String> {
        let end = Instant::now() + wait;
        let mut out = Vec::new();
        while Instant::now() < end {
            match self.next(Duration::from_millis(30))? {
                Some(Message::Ping) => self.send(&Message::Pong)?,
                Some(m) => out.push(m),
                None => {}
            }
        }
        Ok(out)
    }
}

/// The result of a session: what the core scripted, and the device afterwards.
pub struct Session {
    /// The script's result.
    pub script: Result<(), String>,
    /// The daemon's result.
    pub device_result: Result<Report, String>,
    /// The device, for inspection.
    pub device: DaemonDevice,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

/// Connect `device` to a scripted core and run until the script ends and the core hangs up.
#[must_use]
pub fn run_session(
    mut device: DaemonDevice,
    script: impl FnOnce(&mut CoreConn) -> Result<(), String> + Send + 'static,
) -> Session {
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            return Session {
                script: Err(format!("cannot listen: {e}")),
                device_result: Err("not run".into()),
                device,
            };
        }
    };
    let port = listener.local_addr().map_or(0, |a| a.port());
    let core = thread::spawn(move || -> Result<(), String> {
        let (sock, _) = listener.accept().map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(Duration::from_millis(20)))
            .map_err(|e| e.to_string())?;
        let mut conn = CoreConn {
            sock,
            dec: Decoder::new(Limits::default()),
            hello: Hello {
                protocol: PROTOCOL_VERSION,
                device_id: String::new(),
                name: None,
                token: None,
                capabilities: clawdot_protocol::message::Capabilities::default(),
            },
        };
        let hello = loop {
            match conn.next(Duration::from_secs(10))? {
                Some(Message::Hello(h)) => break h,
                Some(_) => {}
                None => return Err("no hello from the daemon".into()),
            }
        };
        conn.hello = hello;
        conn.send(&Message::Welcome(Welcome {
            protocol: PROTOCOL_VERSION,
            session_id: "conformance".into(),
            audio_out: AudioCodec::Pcm,
        }))?;
        script(&mut conn)
    });
    let cfg = Config {
        device_id: "conformance".into(),
        token: None,
        wake_on_connect: false,
        dead_after: DEAD_AFTER,
    };
    let device_result = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(sock) => {
            // Bound the run: the device stops when the core hangs up, which happens when the script returns.
            clawdot_satellite::run(sock, &mut device, &cfg)
        }
        Err(e) => Err(format!("cannot connect: {e}")),
    };
    let script = core
        .join()
        .unwrap_or_else(|_| Err("the scripted core panicked".into()));
    Session {
        script,
        device_result,
        device,
    }
}
