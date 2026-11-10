//! The daemon: a satellite [`Device`] built from a board's [`Parts`] (REQ-DEV-001, 002, 005, 006, 014, 015, 020).
//!
//! It wraps the reference satellite's [`AudioDevice`] and adds what the device layer owns: LED states with
//! precedence, buttons and gestures, local stop, mute with an honest kind, built-in tones, software ducking and
//! volume, and quiet hours. Everything protocol-shaped stays in `clawdot_satellite::run`.

use crate::board::Parts;
use crate::button::{Action, ActionMap, ButtonEvent, ButtonSource, Gesture, GestureDetector};
use crate::caps::{Capability, Declared};
use crate::led::{LedController, LedState};
use crate::quiet::{QuietPolicy, minute_of_day_now};
use crate::tone::{self, Tone};
use clawdot_protocol::message::{Capabilities, Indicator};
use clawdot_satellite::audio::{AudioError, AudioIn, AudioOut};
use clawdot_satellite::front::{AudioDevice, DeviceConfig, Press, Trigger, WakeSink};
use clawdot_satellite::hw::OutputCaps;
use clawdot_satellite::{Config, Device, Report, backoff, is_refusal, run};
use std::net::TcpStream;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

/// Volume change per button press, percent.
const VOLUME_STEP: u8 = 10;

/// An output that applies a volume and a duck level in software, to the daemon's own audio only.
#[derive(Debug)]
pub struct GainOut<O> {
    inner: O,
    volume: u8,
    duck: u8,
}

impl<O: AudioOut> GainOut<O> {
    /// Wrap an output at full volume and no ducking.
    pub fn new(inner: O) -> Self {
        Self {
            inner,
            volume: 100,
            duck: 100,
        }
    }

    /// The combined gain, 0.0 to 1.0.
    #[must_use]
    pub fn gain(&self) -> f32 {
        f32::from(self.volume) * f32::from(self.duck) / 10_000.0
    }

    /// The inner output, for inspection.
    #[must_use]
    pub fn inner(&self) -> &O {
        &self.inner
    }
}

impl<O: AudioOut> AudioOut for GainOut<O> {
    fn begin(&mut self) -> Result<(), AudioError> {
        self.inner.begin()
    }

    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        let g = self.gain();
        if (g - 1.0).abs() < f32::EPSILON {
            return self.inner.play(pcm);
        }
        let scaled: Vec<i16> = pcm.iter().map(|s| (f32::from(*s) * g) as i16).collect();
        self.inner.play(&scaled)
    }

    fn finish(&mut self) {
        self.inner.finish();
    }

    fn poll_done(&mut self) -> Result<bool, AudioError> {
        self.inner.poll_done()
    }

    fn stop(&mut self) {
        self.inner.stop();
    }

    fn capabilities(&self) -> OutputCaps {
        OutputCaps {
            duck: true,
            volume: true,
            stop: self.inner.capabilities().stop,
        }
    }

    fn duck(&mut self, level: u8) {
        self.duck = level.min(100);
    }

    fn set_volume(&mut self, level: u8) {
        self.volume = level.min(100);
    }
}

/// Turn presses sent from the button handler into a satellite [`Trigger`].
/// Presses from the button handler, as a satellite trigger.
#[derive(Debug)]
pub struct ChannelTrigger(Receiver<Press>);

impl Trigger for ChannelTrigger {
    fn poll(&mut self) -> Press {
        match self.0.try_recv() {
            Ok(p) => p,
            Err(TryRecvError::Empty) => Press::Nothing,
            Err(TryRecvError::Disconnected) => Press::Quit,
        }
    }
}

/// Settings for the daemon that are not part of the board.
#[derive(Default)]
pub struct DaemonConfig {
    /// Quiet-hours policy (REQ-DEV-006).
    pub quiet: QuietPolicy,
    /// Offset of local time from UTC in minutes, used by the default clock.
    pub utc_offset_min: i32,
    /// Leave after this many replies (development and tests).
    pub max_turns: Option<usize>,
    /// Print one line per state to standard error.
    pub verbose: bool,
    /// Keep the microphone open between turns (REQ-DEV-025). Implied by `wake`.
    pub always_on: bool,
    /// A wake word detector fed from the open microphone (REQ-DEV-029). Push-to-talk works with or without it.
    pub wake: Option<Box<dyn WakeSink>>,
}

impl std::fmt::Debug for DaemonConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonConfig").finish_non_exhaustive()
    }
}

/// What status shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Board id.
    pub board: String,
    /// The LED state that is shown (or would be, on a board with no LEDs).
    pub led: LedState,
    /// Whether the microphone is muted.
    pub muted: bool,
    /// The volume in effect, percent.
    pub volume: u8,
    /// Whether a core session is open.
    pub online: bool,
    /// Parts that could not be set up.
    pub problems: Vec<String>,
}

type Inner = AudioDevice<Box<dyn AudioIn>, GainOut<Box<dyn AudioOut>>, ChannelTrigger>;

/// The daemon's [`Device`].
pub struct DaemonDevice {
    inner: Inner,
    leds: LedController,
    buttons: Box<dyn ButtonSource>,
    actions: ActionMap,
    gestures: GestureDetector,
    declared: Declared,
    quiet: QuietPolicy,
    clock: Box<dyn Fn() -> u16 + Send>,
    led_brightness: (u8, u8),
    last_quiet: Option<bool>,
    trigger: SyncSender<Press>,
    name: String,
    problems: Vec<String>,
    requested_volume: u8,
    muted: bool,
    pending_mute: Option<bool>,
    playing: bool,
    online: bool,
    stops: usize,
    admin_reset_pending: bool,
}

impl std::fmt::Debug for DaemonDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonDevice")
            .field("board", &self.name)
            .field("muted", &self.muted)
            .field("online", &self.online)
            .finish_non_exhaustive()
    }
}

impl DaemonDevice {
    /// Build the daemon over a board's parts.
    #[must_use]
    pub fn new(parts: Parts, cfg: DaemonConfig) -> Self {
        let (tx, rx) = sync_channel(8);
        let present = parts.declared.has(Capability::Leds);
        let offset = cfg.utc_offset_min;
        let always_on = cfg.always_on || cfg.wake.is_some();
        let mut inner = AudioDevice::new(
            parts.input,
            GainOut::new(parts.output),
            ChannelTrigger(rx),
            DeviceConfig {
                max_turns: cfg.max_turns,
                verbose: cfg.verbose,
                always_on,
                ..DeviceConfig::default()
            },
        );
        if let Some(w) = cfg.wake {
            inner = inner.with_wake_boxed(w);
        }
        let gestures = GestureDetector::new(
            parts.gesture_ms.0,
            parts.gesture_ms.1,
            parts.actions.double_buttons(),
        );
        let mut leds = LedController::new(parts.leds, present);
        // Until a session opens the core is not there: show it.
        leds.set(LedState::Offline, true);
        Self {
            inner,
            leds,
            buttons: parts.buttons,
            actions: parts.actions,
            gestures,
            declared: parts.declared,
            quiet: cfg.quiet,
            clock: Box::new(move || minute_of_day_now(offset)),
            led_brightness: parts.led_brightness,
            last_quiet: None,
            trigger: tx,
            name: parts.name,
            problems: parts.problems,
            requested_volume: 100,
            muted: false,
            pending_mute: None,
            playing: false,
            online: false,
            stops: 0,
            admin_reset_pending: false,
        }
    }

    /// Replace the clock (minute of the local day) the quiet-hours policy reads. Tests use this.
    #[must_use]
    pub fn with_clock(mut self, clock: impl Fn() -> u16 + Send + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// What status shows now.
    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            board: self.name.clone(),
            led: self.leds.current(),
            muted: self.muted,
            volume: self.effective_volume(),
            online: self.online,
            problems: self.problems.clone(),
        }
    }

    /// What this daemon declares to the core.
    #[must_use]
    pub fn declared(&self) -> &Declared {
        &self.declared
    }

    /// The reference satellite's device, for inspection after a run.
    #[must_use]
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// How many local stops were performed.
    #[must_use]
    pub fn stops(&self) -> usize {
        self.stops
    }

    fn minute(&self) -> u16 {
        (self.clock)()
    }

    fn effective_volume(&self) -> u8 {
        self.quiet.volume(self.requested_volume, self.minute())
    }

    fn apply_volume(&mut self) {
        let v = self.effective_volume();
        self.inner.set_volume(v);
    }

    /// Mark the core connection open or closed. Offline shows the `offline` state; local functions keep working
    /// (REQ-DEV-020).
    pub fn set_online(&mut self, online: bool) {
        self.online = online;
        self.leds.set(LedState::Offline, !online);
    }

    /// Play a built-in tone locally, if nothing else is playing (REQ-DEV-014).
    fn play_tone(&mut self, t: Tone) {
        if self.playing {
            return;
        }
        let out = self.inner.output_mut();
        if out.begin().is_ok() {
            let _ = out.play(&tone::samples(t));
            out.finish();
        }
    }

    /// A tone stands in for a state the board cannot show with LEDs (REQ-DEV-013).
    fn equivalent_tone(&mut self, state: LedState) {
        if !self.leds.present()
            && let Some(t) = state.tone_equivalent()
        {
            self.play_tone(t);
        }
    }

    fn apply_mute(&mut self, muted: bool) {
        self.muted = muted;
        self.inner.set_mute(muted);
        self.leds.set(LedState::Muted, muted);
        self.equivalent_tone(LedState::Muted);
    }

    fn handle_action(&mut self, a: Action) {
        match a {
            Action::Talk => {
                if self.online {
                    self.leds.turn(LedState::Wake);
                    self.equivalent_tone(LedState::Wake);
                    // A full queue means presses are already waiting: drop this one.
                    let _ = self.trigger.try_send(Press::Talk);
                } else {
                    self.play_tone(Tone::Error);
                }
            }
            Action::Stop => {
                // Local first: silence the speaker and close the microphone now, whether or not the core is there.
                self.inner.stop_playback();
                self.inner.mic_active(false);
                self.playing = false;
                self.stops += 1;
                self.leds.turn(LedState::Idle);
            }
            Action::MuteToggle => {
                let m = !self.muted;
                self.apply_mute(m);
                self.pending_mute = Some(m);
            }
            Action::VolumeUp | Action::VolumeDown => {
                self.requested_volume = if a == Action::VolumeUp {
                    self.requested_volume.saturating_add(VOLUME_STEP).min(100)
                } else {
                    self.requested_volume.saturating_sub(VOLUME_STEP)
                };
                self.apply_volume();
                self.leds
                    .show_volume(self.effective_volume() / VOLUME_STEP, Instant::now());
            }
        }
    }

    /// Read buttons, finish gestures, expire transient LED states, and apply quiet hours. Called on every pass
    /// of the satellite loop and, while the core is away, by [`run_forever`].
    pub fn pump_local(&mut self) {
        let now = Instant::now();
        while let Some(ev) = self.buttons.poll() {
            match ev {
                ButtonEvent::Quit => {
                    let _ = self.trigger.try_send(Press::Quit);
                }
                ButtonEvent::AdminReset => self.admin_reset_pending = true,
                ButtonEvent::Edge { button, down, at } => {
                    if let Some(g) = self.gestures.edge(&button, down, at)
                        && let Some(a) = self.actions.lookup(&button, g)
                    {
                        self.handle_action(a);
                    }
                }
            }
        }
        for (button, g) in self.gestures.tick(now) {
            if g == Gesture::AdminHold {
                self.admin_reset_pending = true;
                continue;
            }
            if let Some(a) = self.actions.lookup(&button, g) {
                self.handle_action(a);
            }
        }
        self.leds.tick(now);
        let quiet = self.quiet.led_brightness(self.minute());
        if self.last_quiet != Some(quiet.is_some()) {
            self.last_quiet = Some(quiet.is_some());
            let pct = quiet.map_or(self.led_brightness.0, |q| q.min(self.led_brightness.1));
            self.leds.brightness(pct);
            self.apply_volume();
        }
    }

    /// Whether an unsolicited announcement must wait for quiet hours to end (REQ-DEV-006).
    #[must_use]
    pub fn defer_announcement(&self) -> bool {
        self.quiet.defer_announcement(self.minute())
    }
}

impl Device for DaemonDevice {
    fn read_mic(&mut self) -> Option<Vec<i16>> {
        self.inner.read_mic()
    }

    fn begin_playback(&mut self) {
        self.playing = true;
        self.inner.begin_playback();
    }

    fn play(&mut self, pcm: &[i16]) {
        self.inner.play(pcm);
    }

    fn end_playback(&mut self) {
        self.inner.end_playback();
    }

    fn stop_playback(&mut self) {
        self.playing = false;
        self.inner.stop_playback();
    }

    fn indicator(&mut self, state: Indicator) {
        let led = match state {
            Indicator::Idle => {
                self.leds.set(LedState::Muted, self.muted);
                Some(LedState::Idle)
            }
            Indicator::WakeDetected => Some(LedState::Wake),
            Indicator::Listening => Some(LedState::Listening),
            Indicator::Thinking => Some(LedState::Thinking),
            Indicator::Speaking => Some(LedState::Speaking),
            Indicator::Error => Some(LedState::Error),
            Indicator::Notification => Some(LedState::Inbox),
            Indicator::Muted => {
                self.leds.set(LedState::Muted, true);
                None
            }
        };
        if let Some(s) = led {
            self.leds.turn(s);
            self.equivalent_tone(s);
        }
        self.inner.indicator(state);
    }

    fn set_volume(&mut self, level: u8) {
        self.requested_volume = level.min(100);
        self.apply_volume();
    }

    fn set_mute(&mut self, muted: bool) {
        self.apply_mute(muted);
    }

    fn finished(&self) -> bool {
        self.inner.finished()
    }

    fn mic_active(&mut self, on: bool) {
        self.inner.mic_active(on);
    }

    fn mic_burst(&self) -> usize {
        self.inner.mic_burst()
    }

    fn poll_wake(&mut self) -> bool {
        self.pump_local();
        self.inner.poll_wake()
    }

    fn playback_idle(&mut self) -> bool {
        let idle = self.inner.playback_idle();
        if idle && std::mem::replace(&mut self.playing, false) {
            // The core says nothing more when the follow-up window is off; if it does, its state replaces this.
            self.leds.turn(LedState::Idle);
        }
        idle
    }

    fn capabilities(&self) -> Capabilities {
        self.declared.to_protocol()
    }

    fn poll_mute_change(&mut self) -> Option<bool> {
        self.pending_mute.take()
    }

    fn poll_admin_reset(&mut self) -> bool {
        std::mem::take(&mut self.admin_reset_pending)
    }

    fn admin_reset_window(&mut self, open_secs: u32, _reason: Option<&str>) {
        // The core decides; the device only shows the answer, with a tone so a board without LEDs can too.
        if open_secs > 0 {
            self.leds.turn(LedState::Inbox);
            self.play_tone(Tone::Inbox);
        } else {
            self.leds.turn(LedState::Error);
            self.play_tone(Tone::Error);
        }
    }
}

/// Keep a session with the core, reconnecting with backoff, and keep local functions alive while it is away
/// (REQ-DEV-020, REQ-SAT-008). Returns when the device is finished, or with the reason if the core refuses it.
///
/// `connect` opens a connection; `max_attempts` bounds consecutive failed connections (`None` retries forever).
///
/// # Errors
/// The core refused this satellite (bad token or version), or `max_attempts` connections in a row failed.
pub fn run_forever(
    mut connect: impl FnMut() -> Result<TcpStream, String>,
    device: &mut DaemonDevice,
    cfg: &Config,
    max_attempts: Option<u32>,
    mut sleep: impl FnMut(Duration),
) -> Result<Report, String> {
    let mut attempt = 0u32;
    let mut last = Report::default();
    loop {
        let result = connect().and_then(|sock| {
            device.set_online(true);
            attempt = 0;
            run(sock, device, cfg)
        });
        device.set_online(false);
        match result {
            Ok(report) => {
                last = report;
                if device.finished() {
                    return Ok(last);
                }
            }
            Err(e) if is_refusal(&e) => return Err(e),
            Err(e) => {
                attempt += 1;
                if max_attempts.is_some_and(|m| attempt >= m) {
                    return Err(format!("could not keep a session with the core: {e}"));
                }
            }
        }
        // While the core is away, serve the buttons and LEDs in short steps until the backoff is over.
        let wait = backoff(attempt.saturating_sub(1));
        let mut waited = Duration::ZERO;
        while waited < wait {
            device.pump_local();
            if device.finished() {
                return Ok(last);
            }
            let step = Duration::from_millis(50);
            sleep(step);
            waited += step;
        }
    }
}

/// Sleep the real way.
pub fn real_sleep(d: Duration) {
    thread::sleep(d);
}
