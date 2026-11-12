//! Running one realtime session for a speaker (REQ-RTV-009, REQ-RTV-010): settings in, a plain outcome out.
//!
//! This is the step between the speaker's connection, which only moves audio, and the engine in the parent
//! module. It checks the spend cap before anything is opened, connects, runs the session through the host and
//! turns every way it can end into what the speaker should say and whether to use the normal voice instead.

use super::{Env, Io, OpenAi, RealtimeConfig, RealtimeError, SessionEnd, connect, run};
use crate::rtvoice::RtSettings;
use crate::session::ToolHost;
use crate::usage::LimitReached;
use std::time::Duration;

/// How one session ended, for the speaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finish {
    /// What to say to the user, if anything.
    pub speak: Option<String>,
    /// It could not run or broke, so the normal voice is used for a while.
    pub failed: bool,
    /// One plain line for the status, or `None` after a good session.
    pub error: Option<String>,
}

impl Finish {
    fn failed(speak: String) -> Self {
        Self {
            error: Some(speak.clone()),
            speak: Some(speak),
            failed: true,
        }
    }

    fn cap(l: &LimitReached) -> Self {
        Self::failed(l.to_string())
    }
}

/// The engine's configuration from the settings and the key, or why it cannot be built.
///
/// # Errors
/// [`RealtimeError::Config`] when the address, model or key is missing.
pub fn config_from(
    s: &RtSettings,
    key: Option<&str>,
    instructions: &str,
) -> Result<RealtimeConfig, RealtimeError> {
    let (Some(url), Some(model)) = (s.url.as_deref(), s.model.as_deref()) else {
        return Err(RealtimeError::Config(
            "the realtime address and model are not set".into(),
        ));
    };
    let Some(key) = key.filter(|k| !k.is_empty()) else {
        return Err(RealtimeError::Config("there is no API key".into()));
    };
    let mut cfg = RealtimeConfig::new(url, model, key);
    cfg.voice.clone_from(&s.voice);
    cfg.transcription_model.clone_from(&s.transcription_model);
    cfg.usd_per_min_in = s.usd_per_min_in;
    cfg.usd_per_min_out = s.usd_per_min_out;
    cfg.max_session = Duration::from_secs(s.max_session_secs);
    if !instructions.is_empty() {
        cfg.instructions = format!("{}\n\n{instructions}", cfg.instructions);
    }
    Ok(cfg)
}

/// Run one session to its end. `instructions` is added to the engine's own (the persona).
///
/// The spend cap is checked before the connection is opened, so a day that is already over its cap costs no
/// connection at all. Audio cost is added to the counters as the session runs (REQ-RTV-004).
pub async fn serve<H: ToolHost>(
    s: &RtSettings,
    key: Option<&str>,
    instructions: &str,
    mut io: Io,
    env: &mut Env<'_, H>,
) -> Finish {
    let (day, _, _) = (env.clock)();
    if let Err(limit) = env.usage.check(day) {
        return Finish::cap(&limit);
    }
    let cfg = match config_from(s, key, instructions) {
        Ok(c) => c,
        Err(e) => return Finish::failed(e.spoken()),
    };
    let mut ws = match connect(&cfg, &OpenAi).await {
        Ok(ws) => ws,
        Err(e) => return Finish::failed(e.spoken()),
    };
    match run(&mut ws, &OpenAi, &cfg, &mut io, env).await {
        Ok(SessionEnd::SpendCap(l)) => Finish::cap(&l),
        Ok(end) => Finish {
            speak: end.spoken(),
            failed: false,
            error: None,
        },
        Err(e) => Finish::failed(e.spoken()),
    }
}
