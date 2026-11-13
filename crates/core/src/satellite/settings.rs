//! The public types a caller of the satellite server uses: a question for the assistant, its answer, the reset
//! hook and the server settings.

use super::announcer::Announcer;
use crate::assistant::Effect;
use crate::cancel::CancelToken;
use crate::lang::Language;
use crate::rtvoice::Realtime;
use crate::voice::Config as DialogConfig;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// A question for the assistant, with a place to send the answer.
#[derive(Debug)]
pub struct Ask {
    /// Which speaker said it: one connection, one conversation (REQ-AST-026).
    pub speaker: String,
    /// What the user said.
    pub text: String,
    /// Where the answer goes.
    pub reply: oneshot::Sender<Answer>,
    /// Where plan and progress phrases go while the turn runs (REQ-AST-020).
    pub progress: Option<mpsc::UnboundedSender<String>>,
    /// Set to stop the turn (REQ-AST-022).
    pub cancel: CancelToken,
}

/// The assistant's answer.
#[derive(Debug, Clone, Default)]
pub struct Answer {
    /// What to say.
    pub say: String,
    /// Side effects for the satellite.
    pub effects: Vec<Effect>,
    /// The answer ends with a question, so listen longer for the reply (REQ-AST-021).
    pub expects_reply: bool,
}

/// What a satellite's administrator-reset request does (REQ-SAT-020): open the local reset window and return the
/// seconds it stays open, or a short reason in plain words.
#[derive(Clone)]
pub struct AdminResetHook(pub(super) Arc<dyn Fn() -> Result<u32, String> + Send + Sync>);

impl AdminResetHook {
    /// Wrap the function the core provides.
    pub fn new(f: impl Fn() -> Result<u32, String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for AdminResetHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AdminResetHook")
    }
}

/// A satellite may ask for the reset window at most this often.
pub(super) const RESET_ASK_EVERY: Duration = Duration::from_secs(5);

/// Server settings.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Hex SHA-256 hashes of accepted satellite tokens. Empty means no token is required, which is only
    /// reasonable on a trusted loopback or home network (REQ-SAT-010).
    pub token_hashes: Vec<String>,
    /// Dialog behaviour.
    pub dialog: DialogConfig,
    /// How often to ping.
    pub keepalive: Duration,
    /// Drop a peer that has sent nothing for this long.
    pub dead_after: Duration,
    /// How long a new connection has to say hello.
    pub hello_timeout: Duration,
    /// Connections one address may hold at once, so one host cannot fill every session slot (T12).
    pub max_per_ip: usize,
    /// Simultaneous satellites.
    pub max_sessions: usize,
    /// Lets the rest of the program speak to connected satellites.
    pub announcer: Announcer,
    /// Opens the local administrator reset window when a satellite asks (REQ-SAT-020). `None` refuses.
    pub admin_reset: Option<AdminResetHook>,
    /// Set by [`serve`] for a connection from a loopback address; such a peer is on the device itself.
    pub peer_loopback: bool,
    /// The optional realtime voice (REQ-RTV-009). `None`, or not configured, keeps the normal path.
    pub realtime: Option<Realtime>,
    /// A realtime session ends after this long without speech from either side.
    pub rt_idle: Duration,
    /// After a realtime failure the normal path is used for this long (REQ-RTV-010).
    pub rt_cooldown: Duration,
    /// How far ahead of playback a reply is synthesised: the next sentence is asked for when the speaker is
    /// estimated to have this much audio or less left to play. A stop then wastes at most about one sentence.
    pub reply_lead: Duration,
    /// The language replies are cut into sentences in, and a spoken stop is read in.
    pub lang: &'static Language,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            token_hashes: Vec::new(),
            dialog: DialogConfig::default(),
            keepalive: Duration::from_secs(5),
            dead_after: Duration::from_secs(20),
            hello_timeout: Duration::from_secs(5),
            max_per_ip: 2,
            max_sessions: 4,
            announcer: Announcer::default(),
            admin_reset: None,
            peer_loopback: false,
            realtime: None,
            rt_idle: Duration::from_secs(10),
            rt_cooldown: Duration::from_secs(60),
            reply_lead: Duration::from_secs(2),
            lang: crate::lang::DEFAULT,
        }
    }
}
