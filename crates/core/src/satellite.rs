//! The core's side of the satellite protocol (spec 0003, REQ-SAT-001..010).
//!
//! A satellite is a microphone and speaker that knows nothing about assistants. It connects, says `hello`,
//! and then streams microphone audio when asked and plays what it is sent. Everything else happens here: the
//! [`Dialog`] decides who may listen, the endpointer finds the end of the utterance, the speech providers turn
//! audio into text and back, and a *responder* (reached through a channel, so the assistant keeps one owner)
//! answers.
//!
//! A session is generic over its transport, so the tests run it over in-memory pipes and the binary runs it
//! over TCP. Limits that protect a small device: connections are capped, the `hello` must arrive quickly, a
//! silent peer is dropped after the keepalive timeout, and frames are size-limited by the protocol decoder.

mod announcer;
mod handshake;
mod playback;
mod realtime;
mod reply;
mod session;
mod settings;
mod speech;
mod state;
mod turn;
mod wire;

pub use self::announcer::Announcer;
pub use self::session::{run_session, serve};
pub use self::settings::{AdminResetHook, Answer, Ask, Settings};
#[cfg(any(test, feature = "esphome-satellite"))]
pub(crate) use self::wire::next_message;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod phase_tests;
