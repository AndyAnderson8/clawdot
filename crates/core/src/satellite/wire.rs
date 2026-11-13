//! Reading and writing frames: the bounded writer, one message out, one message in, and the wire form of a dialog
//! cue.

use crate::voice::Cue;
use clawdot_protocol::frame::{Decoder, encode};
use clawdot_protocol::message::{Indicator, Message};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The write half of a session, with a bound on how long a write may make no progress. A satellite that stops
/// reading fills the pipe, and a plain write then waits for room forever; the loop is stuck inside it, so the
/// keepalive tick that would notice a dead peer never runs. With this wrapper a write that moves no byte for the
/// dead-peer time fails like a write to a closed connection, and the session ends. Progress of any size restarts
/// the clock, so a slow but live peer is not cut off.
pub(super) struct TimedWrite<W> {
    inner: W,
    limit: Duration,
    stall: Pin<Box<tokio::time::Sleep>>,
    /// Whether `stall` is running for the write now waiting.
    waiting: bool,
}

impl<W> TimedWrite<W> {
    pub(super) fn new(inner: W, limit: Duration) -> Self {
        Self {
            inner,
            limit,
            stall: Box::pin(tokio::time::sleep(limit)),
            waiting: false,
        }
    }

    /// Turn the result of one poll of the inner writer into the result for the caller: any answer ends the wait,
    /// and a wait that has gone on for the limit is an error.
    fn bounded<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<std::io::Result<T>>,
    ) -> Poll<std::io::Result<T>> {
        if polled.is_ready() {
            self.waiting = false;
            return polled;
        }
        if !self.waiting {
            self.waiting = true;
            self.stall
                .as_mut()
                .reset(tokio::time::Instant::now() + self.limit);
        }
        if self.stall.as_mut().poll(cx).is_ready() {
            self.waiting = false;
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the peer is not reading",
            )));
        }
        Poll::Pending
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TimedWrite<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.bounded(cx, polled)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        self.bounded(cx, polled)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.bounded(cx, polled)
    }
}

pub(super) async fn send<W: AsyncWrite + Unpin>(w: &mut W, m: &Message) -> Result<(), String> {
    let bytes = encode(&m.to_frame()).map_err(|e| e.to_string())?;
    w.write_all(&bytes).await.map_err(|e| format!("write: {e}"))
}

pub(super) fn new_session_id() -> String {
    let mut b = [0u8; 8];
    // Falling back to zeros only weakens an identifier that is not a secret.
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Read the next message, or `None` when the peer closed the connection.
pub(crate) async fn next_message<R: AsyncRead + Unpin>(
    r: &mut R,
    dec: &mut Decoder,
) -> Result<Option<Message>, String> {
    let mut buf = [0u8; 4096];
    loop {
        if let Some(frame) = dec.next_frame().map_err(|e| e.to_string())? {
            return Message::from_frame(frame)
                .map(Some)
                .map_err(|e| e.to_string());
        }
        let n = r.read(&mut buf).await.map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Ok(None);
        }
        dec.push(&buf[..n]);
    }
}

pub(super) async fn cue_to_wire<W: AsyncWrite + Unpin>(cue: Cue, w: &mut W) -> Result<(), String> {
    match cue {
        Cue::Wake | Cue::Listening => {
            send(w, &Message::SetIndicator(Indicator::Listening)).await?;
            send(w, &Message::StreamStart).await
        }
        Cue::Thinking => send(w, &Message::SetIndicator(Indicator::Thinking)).await,
        Cue::Error => {
            send(w, &Message::StreamStop).await?;
            send(w, &Message::SetIndicator(Indicator::Error)).await
        }
        Cue::Muted => {
            send(w, &Message::StreamStop).await?;
            send(w, &Message::SetIndicator(Indicator::Muted)).await
        }
        Cue::Unmuted => send(w, &Message::SetIndicator(Indicator::Idle)).await,
    }
}
