//! Buttons: raw edges, gestures, and logical actions (REQ-BRD-007, REQ-DEV-005).
//!
//! A board delivers raw edges (a button went down or up, at a monotonic time). Gesture timing is computed here,
//! once, for every board, from the thresholds in the board file, so it is testable without hardware. The board
//! file maps `<button>.<gesture>` to a logical [`Action`].

use crate::board_file::{BoardFile, ButtonSource as SourceSpec};
use std::collections::VecDeque;
use std::io::{BufRead, Read};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// What a person did with one button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Pressed and released quickly.
    Press,
    /// Held past the long-press time.
    LongPress,
    /// Pressed twice quickly.
    DoublePress,
    /// Held for [`ADMIN_HOLD`] (REQ-DEV-031). Fixed, never mapped by a board file.
    AdminHold,
}

/// How long the `action` button is held to ask for the administrator reset window (REQ-DEV-031).
pub const ADMIN_HOLD: Duration = Duration::from_secs(10);

impl Gesture {
    /// The gesture's name in a board file.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::LongPress => "long_press",
            Self::DoublePress => "double_press",
            Self::AdminHold => "admin_hold",
        }
    }
}

/// A logical action a gesture is mapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Louder.
    VolumeUp,
    /// Quieter.
    VolumeDown,
    /// Toggle the microphone mute.
    MuteToggle,
    /// Start a turn (push-to-talk).
    Talk,
    /// Stop playback and cancel the turn.
    Stop,
}

impl Action {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "volume_up" => Self::VolumeUp,
            "volume_down" => Self::VolumeDown,
            "mute_toggle" => Self::MuteToggle,
            "action" => Self::Talk,
            "stop" => Self::Stop,
            _ => return None,
        })
    }
}

/// A raw button event from a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ButtonEvent {
    /// A button went down or up.
    Edge {
        /// Logical button name.
        button: String,
        /// True for down, false for up.
        down: bool,
        /// Monotonic time of the edge.
        at: Instant,
    },
    /// The operator asked to leave (desktop board).
    Quit,
    /// The operator typed `admin` in the console: the same request as holding the button (REQ-DEV-031).
    AdminReset,
}

/// Where raw button events come from.
pub trait ButtonSource: Send {
    /// The next event, without blocking.
    fn poll(&mut self) -> Option<ButtonEvent>;
}

/// No buttons.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoButtons;

impl ButtonSource for NoButtons {
    fn poll(&mut self) -> Option<ButtonEvent> {
        None
    }
}

/// A scripted source for tests.
#[derive(Debug, Default)]
pub struct ScriptedButtons(pub VecDeque<ButtonEvent>);

impl ButtonSource for ScriptedButtons {
    fn poll(&mut self) -> Option<ButtonEvent> {
        self.0.pop_front()
    }
}

/// Keys typed on the terminal, one line at a time, as presses (desktop board). A line that starts with a key from
/// the board file presses that button; an empty line (Enter) presses `action`; `q` quits. Other text is ignored.
/// A thread does the blocking read, so no terminal library is needed.
#[derive(Debug)]
pub struct KeyButtons {
    queue: Arc<Queue>,
}

/// Most events kept for the loop to read. Overflow drops the oldest (REQ-DEV-020).
pub const QUEUE_LIMIT: usize = 32;

/// Longest line the key reader keeps, in bytes. Key lines are a letter or a word; anything longer is not one.
const MAX_KEY_LINE: usize = 256;

enum Line {
    Text(String),
    /// A line longer than [`MAX_KEY_LINE`]; it has been read to its end and thrown away.
    TooLong,
    End,
}

/// The next line, read with a cap. Bytes that are not UTF-8 become replacement characters (they match no key)
/// instead of ending the reader.
fn next_line(reader: &mut impl BufRead) -> Line {
    let mut buf = Vec::new();
    loop {
        match reader
            .by_ref()
            .take(MAX_KEY_LINE as u64)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) => return Line::End,
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Line::End,
        }
    }
    if buf.last() == Some(&b'\n') || buf.len() < MAX_KEY_LINE {
        return Line::Text(String::from_utf8_lossy(&buf).into_owned());
    }
    // Full without a newline: skip to the end of the line.
    let mut sink = Vec::new();
    loop {
        sink.clear();
        match reader
            .by_ref()
            .take(MAX_KEY_LINE as u64)
            .read_until(b'\n', &mut sink)
        {
            Ok(0) => break,
            Ok(_) if sink.last() == Some(&b'\n') => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    Line::TooLong
}

/// A bounded queue: pushing past the limit drops the oldest event.
#[derive(Debug, Default)]
struct Queue {
    events: Mutex<VecDeque<ButtonEvent>>,
    closed: Mutex<bool>,
}

impl Queue {
    fn push(&self, e: ButtonEvent) {
        if let Ok(mut q) = self.events.lock() {
            if q.len() >= QUEUE_LIMIT {
                q.pop_front();
            }
            q.push_back(e);
        }
    }

    fn pop(&self) -> Option<ButtonEvent> {
        self.events.lock().ok().and_then(|mut q| q.pop_front())
    }
}

impl KeyButtons {
    /// Read lines from `reader`; `keys` maps a typed character to a logical button name.
    pub fn new(reader: impl BufRead + Send + 'static, keys: Vec<(char, String)>) -> Self {
        let queue = Arc::new(Queue::default());
        let tx = queue.clone();
        let has_action = keys.iter().any(|(_, b)| b == "action");
        thread::spawn(move || {
            let mut reader = reader;
            loop {
                let line = match next_line(&mut reader) {
                    Line::End => break,
                    Line::TooLong => continue,
                    Line::Text(t) => t.trim().to_ascii_lowercase(),
                };
                let button = match line.as_str() {
                    "q" | "quit" | "exit" => {
                        tx.push(ButtonEvent::Quit);
                        continue;
                    }
                    "admin" => {
                        tx.push(ButtonEvent::AdminReset);
                        continue;
                    }
                    "" if has_action => Some("action".to_owned()),
                    other => other
                        .chars()
                        .next()
                        .and_then(|c| keys.iter().find(|(k, _)| *k == c))
                        .map(|(_, b)| b.clone()),
                };
                if let Some(button) = button {
                    let at = Instant::now();
                    let down = ButtonEvent::Edge {
                        button: button.clone(),
                        down: true,
                        at,
                    };
                    let up = ButtonEvent::Edge {
                        button,
                        down: false,
                        at,
                    };
                    tx.push(down);
                    tx.push(up);
                }
            }
            // The end of the input does not stop a daemon (it may run with no terminal); only `q` does.
            if let Ok(mut c) = tx.closed.lock() {
                *c = true;
            }
        });
        Self { queue }
    }

    /// Whether the input has ended (the terminal closed, or the stream ran out). The daemon keeps running; only
    /// `q` stops it.
    #[must_use]
    pub fn input_ended(&self) -> bool {
        self.queue.closed.lock().is_ok_and(|c| *c)
    }

    /// Read from the terminal, with the key map of a board file.
    #[must_use]
    pub fn stdin(board: &BoardFile) -> Self {
        let keys = board
            .buttons
            .iter()
            .filter_map(|b| match b.source {
                SourceSpec::Key(c) => Some((c, b.name.clone())),
                SourceSpec::Evdev { .. } => None,
            })
            .collect();
        Self::new(std::io::BufReader::new(std::io::stdin()), keys)
    }
}

impl ButtonSource for KeyButtons {
    fn poll(&mut self) -> Option<ButtonEvent> {
        self.queue.pop()
    }
}

#[derive(Debug, Default)]
struct State {
    down_at: Option<Instant>,
    long_fired: bool,
    admin_fired: bool,
    /// A short press waiting to learn whether a second one follows.
    pending_press: Option<Instant>,
}

/// Turns raw edges into gestures. One instance serves every button of a board.
#[derive(Debug)]
pub struct GestureDetector {
    long: Duration,
    double: Duration,
    admin_hold: Duration,
    /// Buttons that have a `double_press` mapping; only these delay a press to wait for a second one.
    double_buttons: Vec<String>,
    states: Vec<(String, State)>,
}

impl GestureDetector {
    /// A detector with the given thresholds. `double_buttons` lists the buttons whose `double_press` is mapped.
    #[must_use]
    pub fn new(long_ms: u32, double_ms: u32, double_buttons: Vec<String>) -> Self {
        Self {
            long: Duration::from_millis(u64::from(long_ms)),
            double: Duration::from_millis(u64::from(double_ms)),
            admin_hold: ADMIN_HOLD,
            double_buttons,
            states: Vec::new(),
        }
    }

    /// Use another hold time for the administrator reset (tests).
    #[must_use]
    pub fn with_admin_hold(mut self, hold: Duration) -> Self {
        self.admin_hold = hold;
        self
    }

    fn state(&mut self, button: &str) -> &mut State {
        if let Some(i) = self.states.iter().position(|(b, _)| b == button) {
            return &mut self.states[i].1;
        }
        self.states.push((button.to_owned(), State::default()));
        let last = self.states.len() - 1;
        &mut self.states[last].1
    }

    /// Feed one edge. Returns a gesture that completed because of it.
    pub fn edge(&mut self, button: &str, down: bool, at: Instant) -> Option<Gesture> {
        let long = self.long;
        let double = self.double;
        let wants_double = self.double_buttons.iter().any(|b| b == button);
        let s = self.state(button);
        if down {
            if s.down_at.is_none() {
                s.down_at = Some(at);
                s.long_fired = false;
                s.admin_fired = false;
            }
            return None;
        }
        let down_at = s.down_at.take()?;
        if s.long_fired {
            s.long_fired = false;
            return None;
        }
        if at.saturating_duration_since(down_at) >= long {
            return Some(Gesture::LongPress);
        }
        if !wants_double {
            return Some(Gesture::Press);
        }
        match s.pending_press.take() {
            Some(first) if at.saturating_duration_since(first) <= double => {
                Some(Gesture::DoublePress)
            }
            _ => {
                s.pending_press = Some(at);
                None
            }
        }
    }

    /// Call on every pass of the loop. Returns gestures that completed with the passage of time: a long press
    /// while still held, a single press whose double-press window passed.
    pub fn tick(&mut self, now: Instant) -> Vec<(String, Gesture)> {
        let long = self.long;
        let double = self.double;
        let admin_hold = self.admin_hold;
        let mut out = Vec::new();
        for (button, s) in &mut self.states {
            if let Some(d) = s.down_at
                && !s.long_fired
                && now.saturating_duration_since(d) >= long
            {
                s.long_fired = true;
                s.pending_press = None;
                out.push((button.clone(), Gesture::LongPress));
            }
            if let Some(d) = s.down_at
                && !s.admin_fired
                && button == "action"
                && now.saturating_duration_since(d) >= admin_hold
            {
                s.admin_fired = true;
                out.push((button.clone(), Gesture::AdminHold));
            }
            if let Some(p) = s.pending_press
                && s.down_at.is_none()
                && now.saturating_duration_since(p) > double
            {
                s.pending_press = None;
                out.push((button.clone(), Gesture::Press));
            }
        }
        out
    }
}

/// `<button>.<gesture>` to [`Action`], from the board file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionMap {
    map: Vec<(String, Action)>,
}

impl ActionMap {
    /// Build from a validated board file.
    #[must_use]
    pub fn from_board(board: &BoardFile) -> Self {
        Self {
            map: board
                .actions
                .iter()
                .filter_map(|(k, a)| Some((k.clone(), Action::parse(a)?)))
                .collect(),
        }
    }

    /// The action for a gesture on a button, if mapped. An unmapped gesture does nothing.
    #[must_use]
    pub fn lookup(&self, button: &str, gesture: Gesture) -> Option<Action> {
        let key = format!("{button}.{}", gesture.name());
        self.map.iter().find(|(k, _)| *k == key).map(|(_, a)| *a)
    }

    /// Buttons that have a `double_press` mapping.
    #[must_use]
    pub fn double_buttons(&self) -> Vec<String> {
        self.map
            .iter()
            .filter_map(|(k, _)| k.strip_suffix(".double_press").map(str::to_owned))
            .collect()
    }

    /// Whether any gesture maps to `action`.
    #[must_use]
    pub fn has(&self, action: Action) -> bool {
        self.map.iter().any(|(_, a)| *a == action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn detector(double: &[&str]) -> GestureDetector {
        GestureDetector::new(700, 350, double.iter().map(|s| (*s).to_owned()).collect())
    }

    fn ms(base: Instant, n: u64) -> Instant {
        base + Duration::from_millis(n)
    }

    #[test]
    fn req_dev_031_holding_action_for_ten_seconds_asks_for_the_reset_once() {
        let t = Instant::now();
        let mut d = detector(&[]);
        d.edge("action", true, t);
        // The ordinary long press still fires at its own time and the hold adds the request after it.
        assert_eq!(
            d.tick(ms(t, 700)),
            vec![("action".to_owned(), Gesture::LongPress)]
        );
        assert!(d.tick(ms(t, 9_999)).is_empty(), "not yet");
        assert_eq!(
            d.tick(ms(t, 10_000)),
            vec![("action".to_owned(), Gesture::AdminHold)]
        );
        assert!(d.tick(ms(t, 12_000)).is_empty(), "once per hold");
        assert_eq!(d.edge("action", false, ms(t, 13_000)), None);
        // A second hold asks again.
        d.edge("action", true, ms(t, 20_000));
        assert_eq!(
            d.tick(ms(t, 30_000)).last(),
            Some(&("action".to_owned(), Gesture::AdminHold))
        );
    }

    #[test]
    fn req_dev_031_only_the_action_button_and_only_ten_seconds_count() {
        let t = Instant::now();
        let mut d = detector(&[]);
        d.edge("mute", true, t);
        assert!(
            d.tick(ms(t, 60_000))
                .iter()
                .all(|(_, g)| *g != Gesture::AdminHold),
            "a held mute button never asks"
        );
        d.edge("action", true, ms(t, 61_000));
        d.edge("action", false, ms(t, 69_000));
        assert!(
            d.tick(ms(t, 80_000)).is_empty(),
            "released at eight seconds"
        );
    }

    #[test]
    fn req_brd_007_a_quick_press_is_a_press() {
        let t = Instant::now();
        let mut d = detector(&[]);
        assert_eq!(d.edge("action", true, t), None);
        assert_eq!(d.edge("action", false, ms(t, 80)), Some(Gesture::Press));
    }

    #[test]
    fn req_brd_007_a_held_button_is_a_long_press_once() {
        let t = Instant::now();
        let mut d = detector(&[]);
        d.edge("action", true, t);
        assert!(d.tick(ms(t, 300)).is_empty());
        assert_eq!(
            d.tick(ms(t, 700)),
            vec![("action".to_owned(), Gesture::LongPress)]
        );
        assert!(d.tick(ms(t, 900)).is_empty());
        // Releasing after the long press fires adds nothing.
        assert_eq!(d.edge("action", false, ms(t, 1000)), None);
    }

    #[test]
    fn req_brd_007_a_long_press_released_before_a_tick_is_still_a_long_press() {
        let t = Instant::now();
        let mut d = detector(&[]);
        d.edge("action", true, t);
        assert_eq!(
            d.edge("action", false, ms(t, 800)),
            Some(Gesture::LongPress)
        );
    }

    #[test]
    fn req_brd_007_two_quick_presses_are_a_double_press_when_mapped() {
        let t = Instant::now();
        let mut d = detector(&["action"]);
        d.edge("action", true, t);
        assert_eq!(
            d.edge("action", false, ms(t, 50)),
            None,
            "waits to see if a second press follows"
        );
        d.edge("action", true, ms(t, 150));
        assert_eq!(
            d.edge("action", false, ms(t, 200)),
            Some(Gesture::DoublePress)
        );
    }

    #[test]
    fn req_brd_007_a_single_press_is_released_after_the_double_window() {
        let t = Instant::now();
        let mut d = detector(&["action"]);
        d.edge("action", true, t);
        d.edge("action", false, ms(t, 50));
        assert!(d.tick(ms(t, 300)).is_empty());
        assert_eq!(
            d.tick(ms(t, 450)),
            vec![("action".to_owned(), Gesture::Press)]
        );
        assert!(d.tick(ms(t, 900)).is_empty());
    }

    #[test]
    fn req_brd_007_buttons_do_not_interfere() {
        let t = Instant::now();
        let mut d = detector(&[]);
        d.edge("mute", true, t);
        d.edge("stop", true, ms(t, 10));
        assert_eq!(d.edge("stop", false, ms(t, 60)), Some(Gesture::Press));
        assert_eq!(d.edge("mute", false, ms(t, 90)), Some(Gesture::Press));
    }

    fn board() -> BoardFile {
        BoardFile::parse(include_str!("../../../boards/echo-dot2.json")).unwrap()
    }

    #[test]
    fn req_brd_007_actions_come_from_the_board_file() {
        let map = ActionMap::from_board(&board());
        assert_eq!(map.lookup("mute", Gesture::Press), Some(Action::MuteToggle));
        assert_eq!(map.lookup("action", Gesture::LongPress), Some(Action::Stop));
        assert_eq!(map.lookup("action", Gesture::DoublePress), None);
        assert!(map.has(Action::Stop));
        assert!(map.double_buttons().is_empty());
    }

    #[test]
    fn req_dev_020_the_key_queue_is_bounded_and_keeps_the_newest() {
        let mut lines = String::new();
        for _ in 0..100 {
            lines.push_str(
                "t
",
            );
        }
        lines.push_str(
            "s
",
        );
        let mut k = KeyButtons::new(
            Cursor::new(lines.into_bytes()),
            vec![('t', "action".into()), ('s', "stop".into())],
        );
        // Wait for the reader thread to finish (it pushes Quit last).
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !k.input_ended() {
            thread::sleep(Duration::from_millis(2));
        }
        let mut n = 0;
        let mut last_button = String::new();
        while let Some(e) = k.poll() {
            n += 1;
            if let ButtonEvent::Edge { button, .. } = e {
                last_button = button;
            }
        }
        assert!(n <= QUEUE_LIMIT, "{n} events kept");
        assert_eq!(last_button, "stop", "the newest press survived");
    }

    #[test]
    fn req_dev_020_bad_bytes_and_endless_lines_do_not_stop_the_key_reader() {
        let mut input = b"t\n\xff\xfe\xfd\n".to_vec();
        // A line with no end for much longer than any key line: read with a cap, skipped, and reading goes on.
        input.extend(std::iter::repeat_n(b'x', 1_000_000));
        input.extend_from_slice(b"\ns\nt");
        let keys = vec![('t', "action".to_owned()), ('s', "stop".to_owned())];
        let mut k = KeyButtons::new(Cursor::new(input), keys);
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end && !k.input_ended() {
            thread::sleep(Duration::from_millis(2));
        }
        assert!(k.input_ended());
        let mut downs = Vec::new();
        while let Some(e) = k.poll() {
            if let ButtonEvent::Edge {
                button, down: true, ..
            } = e
            {
                downs.push(button);
            }
        }
        // `t`, then (after the bad bytes) nothing for the junk lines, then `s`, then the unterminated `t`.
        assert_eq!(downs, ["action", "stop", "action"]);
    }

    #[test]
    fn desktop_keys_press_buttons_and_enter_is_the_action_button() {
        let keys = vec![('t', "action".to_owned()), ('s', "stop".to_owned())];
        let mut k = KeyButtons::new(Cursor::new(b"s\n\nx\nadmin\nq\n".to_vec()), keys);
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            match k.poll() {
                Some(ButtonEvent::Quit) => {
                    got.push("quit".to_owned());
                    break;
                }
                Some(ButtonEvent::AdminReset) => got.push("admin".to_owned()),
                Some(ButtonEvent::Edge { button, down, .. }) => {
                    got.push(format!("{button}:{down}"))
                }
                None => thread::sleep(Duration::from_millis(2)),
            }
        }
        assert_eq!(
            got,
            [
                "stop:true",
                "stop:false",
                "action:true",
                "action:false",
                "admin",
                "quit"
            ]
        );
    }
}
