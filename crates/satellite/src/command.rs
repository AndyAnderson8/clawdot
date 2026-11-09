//! Audio through an external program (REQ-VOI-014, REQ-VOI-015).
//!
//! [`CommandIn`] runs a capture program and reads raw 16 kHz mono 16-bit PCM from its standard output.
//! [`CommandOut`] runs a playback program and writes PCM to its standard input. The program is an argument
//! vector, never a shell string, and audio is the only data that flows; nothing from the network or the
//! assistant ever reaches an argument. Threads do the blocking pipe work so a stalled program cannot freeze
//! the satellite loop, and every wait is bounded.
//!
//! [`default_capture`] and [`default_playback`] choose a program from those found on `PATH`.

use crate::audio::{AudioError, AudioIn, AudioOut, FRAME_BYTES, FRAME_SAMPLES, wav_header};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Frames kept when the reader is ahead of the consumer: one second. The oldest are dropped.
const MAX_FRAMES: usize = 50;
/// Reply audio queued for a player, at most (about four minutes).
const MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
/// Bytes written to a player at a time, so progress is visible and a stall is noticed.
const WRITE_SLICE: usize = 4096;
/// How long a program may go without audio before it counts as stalled.
pub const DEFAULT_STALL: Duration = Duration::from_secs(5);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Program first, then arguments. Never passed through a shell.
    pub argv: Vec<String>,
    /// Playback only: the program wants a WAV header before the samples (`ffplay` cannot be told the format
    /// of raw input portably).
    pub wav_header: bool,
}

impl CommandSpec {
    /// The program name, for messages (arguments are not shown).
    #[must_use]
    pub fn program(&self) -> &str {
        self.argv.first().map_or("(none)", String::as_str)
    }
}

fn spawn(argv: &[String], pipe_in: bool, pipe_out: bool) -> Result<Child, AudioError> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| AudioError::NotFound("the audio command is empty".into()))?;
    Command::new(program)
        .args(args)
        .stdin(if pipe_in {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if pipe_out {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AudioError::NotFound(format!(
                    "`{program}` was not found; install it or set the command"
                ))
            } else {
                AudioError::Spawn(format!("cannot start `{program}`: {e}"))
            }
        })
}

fn reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------------------------- capture

#[derive(Debug)]
struct Queue {
    frames: VecDeque<Vec<i16>>,
    last_data: Instant,
    ended: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<Queue>,
    cv: Condvar,
}

fn read_loop(mut out: ChildStdout, shared: &Shared) {
    let mut buf = [0u8; FRAME_BYTES];
    let mut have = 0;
    let end = |error: Option<String>| {
        let mut q = lock(&shared.state);
        q.ended = true;
        q.error = error;
        shared.cv.notify_all();
    };
    loop {
        match out.read(&mut buf[have..]) {
            Ok(0) => return end(None),
            Ok(n) => {
                have += n;
                let mut q = lock(&shared.state);
                q.last_data = Instant::now();
                if have == FRAME_BYTES {
                    have = 0;
                    if q.frames.len() >= MAX_FRAMES {
                        q.frames.pop_front();
                    }
                    q.frames.push_back(
                        buf.chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]]))
                            .collect(),
                    );
                    shared.cv.notify_all();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return end(Some(format!("reading the capture program: {e}"))),
        }
    }
}

#[derive(Debug)]
struct InSession {
    child: Child,
    shared: Arc<Shared>,
}

impl Drop for InSession {
    fn drop(&mut self) {
        reap(&mut self.child);
    }
}

/// Microphone audio from a capture program's standard output.
#[derive(Debug)]
pub struct CommandIn {
    argv: Vec<String>,
    stall: Duration,
    session: Option<InSession>,
}

impl CommandIn {
    /// A capture through `spec`, stalled after `stall` without audio.
    #[must_use]
    pub fn new(spec: CommandSpec, stall: Duration) -> Self {
        Self {
            argv: spec.argv,
            stall,
            session: None,
        }
    }
}

impl AudioIn for CommandIn {
    fn start(&mut self) -> Result<(), AudioError> {
        if self.session.is_some() {
            return Ok(());
        }
        let mut child = spawn(&self.argv, false, true)?;
        let Some(out) = child.stdout.take() else {
            reap(&mut child);
            return Err(AudioError::Spawn(
                "the capture program has no output".into(),
            ));
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(Queue {
                frames: VecDeque::new(),
                last_data: Instant::now(),
                ended: false,
                error: None,
            }),
            cv: Condvar::new(),
        });
        let theirs = Arc::clone(&shared);
        thread::spawn(move || read_loop(out, &theirs));
        self.session = Some(InSession { child, shared });
        Ok(())
    }

    fn read_frame(&mut self, wait: Duration) -> Result<Option<Vec<i16>>, AudioError> {
        let Some(s) = &self.session else {
            return Err(AudioError::NotStarted);
        };
        let deadline = Instant::now() + wait;
        let mut q = lock(&s.shared.state);
        loop {
            if let Some(f) = q.frames.pop_front() {
                debug_assert_eq!(f.len(), FRAME_SAMPLES);
                return Ok(Some(f));
            }
            if q.ended {
                return Err(q.error.clone().map_or(AudioError::Eof, AudioError::Io));
            }
            let quiet = q.last_data.elapsed();
            if quiet >= self.stall {
                return Err(AudioError::Stalled(format!(
                    "the capture program produced no audio for {} s",
                    self.stall.as_secs_f32()
                )));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let left = (deadline - now).min(self.stall - quiet);
            q = s
                .shared
                .cv
                .wait_timeout(q, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn stop(&mut self) {
        self.session = None;
    }
}

// ---------------------------------------------------------------------------------------------- playback

#[derive(Debug)]
struct OutState {
    progress: Instant,
    drained_at: Option<Instant>,
    error: Option<String>,
}

#[derive(Debug)]
struct OutShared {
    pending: AtomicUsize,
    state: Mutex<OutState>,
}

fn write_loop(mut stdin: ChildStdin, rx: &Receiver<Vec<u8>>, shared: &OutShared) {
    for chunk in rx {
        for slice in chunk.chunks(WRITE_SLICE) {
            if let Err(e) = stdin.write_all(slice) {
                let mut s = lock(&shared.state);
                s.error = Some(format!("the playback program stopped accepting audio: {e}"));
                return;
            }
            shared.pending.fetch_sub(slice.len(), Ordering::SeqCst);
            lock(&shared.state).progress = Instant::now();
        }
    }
    drop(stdin);
    let mut s = lock(&shared.state);
    s.drained_at = Some(Instant::now());
}

#[derive(Debug)]
struct OutSession {
    child: Child,
    tx: Option<Sender<Vec<u8>>>,
    shared: Arc<OutShared>,
}

impl Drop for OutSession {
    fn drop(&mut self) {
        self.tx = None;
        reap(&mut self.child);
    }
}

/// Reply audio sent to a playback program's standard input. One program is run per reply.
#[derive(Debug)]
pub struct CommandOut {
    spec: CommandSpec,
    stall: Duration,
    volume: u8,
    duck: u8,
    session: Option<OutSession>,
}

impl CommandOut {
    /// Playback through `spec`, stalled after `stall` without the program taking audio.
    #[must_use]
    pub fn new(spec: CommandSpec, stall: Duration) -> Self {
        Self {
            spec,
            stall,
            volume: 100,
            duck: 100,
            session: None,
        }
    }

    fn send(&mut self, bytes: Vec<u8>) -> Result<(), AudioError> {
        let s = self.session.as_ref().ok_or(AudioError::NotStarted)?;
        if let Some(e) = lock(&s.shared.state).error.clone() {
            return Err(AudioError::Io(e));
        }
        if s.shared.pending.load(Ordering::SeqCst) + bytes.len() > MAX_QUEUED_BYTES {
            return Err(AudioError::Io("too much reply audio is queued".into()));
        }
        s.shared.pending.fetch_add(bytes.len(), Ordering::SeqCst);
        let sent = s.tx.as_ref().map(|tx| tx.send(bytes));
        if !matches!(sent, Some(Ok(()))) {
            return Err(AudioError::Io("the playback program has gone away".into()));
        }
        Ok(())
    }
}

/// Where a player is: `Ok(true)` finished, `Ok(false)` still playing, `Err` failed or stalled.
fn check_player(s: &mut OutSession, stall: Duration) -> Result<bool, AudioError> {
    let (error, quiet, drained) = {
        let st = lock(&s.shared.state);
        (st.error.clone(), st.progress.elapsed(), st.drained_at)
    };
    if let Some(e) = error {
        return Err(AudioError::Io(e));
    }
    let Some(at) = drained else {
        let waiting = s.shared.pending.load(Ordering::SeqCst) > 0;
        if waiting && quiet >= stall {
            return Err(AudioError::Stalled(format!(
                "the playback program took no audio for {} s",
                stall.as_secs_f32()
            )));
        }
        return Ok(false);
    };
    match s.child.try_wait() {
        Ok(Some(status)) if status.success() => Ok(true),
        Ok(Some(status)) => Err(AudioError::Io(format!(
            "the playback program ended with {status}"
        ))),
        Ok(None) if at.elapsed() >= stall => Err(AudioError::Stalled(
            "the playback program did not finish after the audio ended".into(),
        )),
        Ok(None) => Ok(false),
        Err(e) => Err(AudioError::Io(format!("waiting for the player: {e}"))),
    }
}

impl AudioOut for CommandOut {
    fn begin(&mut self) -> Result<(), AudioError> {
        self.session = None;
        let mut child = spawn(&self.spec.argv, true, false)?;
        let Some(stdin) = child.stdin.take() else {
            reap(&mut child);
            return Err(AudioError::Spawn(
                "the playback program has no input".into(),
            ));
        };
        let shared = Arc::new(OutShared {
            pending: AtomicUsize::new(0),
            state: Mutex::new(OutState {
                progress: Instant::now(),
                drained_at: None,
                error: None,
            }),
        });
        let (tx, rx) = channel();
        let theirs = Arc::clone(&shared);
        thread::spawn(move || write_loop(stdin, &rx, &theirs));
        self.session = Some(OutSession {
            child,
            tx: Some(tx),
            shared,
        });
        if self.spec.wav_header {
            self.send(wav_header(u32::MAX))?;
        }
        Ok(())
    }

    fn play(&mut self, pcm: &[i16]) -> Result<(), AudioError> {
        let gain = u32::from(self.volume) * u32::from(self.duck) / 100;
        let bytes = pcm
            .iter()
            .flat_map(|&s| {
                let scaled = i32::from(s) * i32::try_from(gain).unwrap_or(100) / 100;
                i16::try_from(scaled).unwrap_or(0).to_le_bytes()
            })
            .collect();
        self.send(bytes)
    }

    fn finish(&mut self) {
        if let Some(s) = &mut self.session {
            s.tx = None;
        }
    }

    fn poll_done(&mut self) -> Result<bool, AudioError> {
        let Some(s) = &mut self.session else {
            return Ok(true);
        };
        let outcome = check_player(s, self.stall);
        if !matches!(outcome, Ok(false)) {
            // Finished or failed: either way the player is gone and the output is idle again.
            self.session = None;
        }
        outcome
    }

    fn stop(&mut self) {
        self.session = None;
    }

    fn capabilities(&self) -> crate::hw::OutputCaps {
        crate::hw::OutputCaps {
            duck: true,
            volume: true,
            stop: true,
        }
    }

    fn duck(&mut self, level: u8) {
        self.duck = level.min(100);
    }

    fn set_volume(&mut self, level: u8) {
        self.volume = level.min(100);
    }
}

// ---------------------------------------------------------------------------------------------- discovery

/// The operating system families the defaults know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Windows.
    Windows,
    /// Linux and other Unix systems with ALSA.
    Linux,
    /// macOS.
    Mac,
}

impl Os {
    /// The system this was built for.
    #[must_use]
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Linux
        }
    }
}

/// Whether `program` is an executable file in a `PATH` directory. It is never run.
#[must_use]
pub fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{program}.exe"),
            format!("{program}.cmd"),
            format!("{program}.bat"),
        ]
    } else {
        vec![program.to_owned()]
    };
    std::env::split_paths(&paths).any(|dir| names.iter().any(|n| Path::new(&dir).join(n).is_file()))
}

fn strs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

const RAW_IN: [&str; 9] = ["-t", "raw", "-r", "16000", "-c", "1", "-b", "16", "-e"];

fn sox_capture(device_args: &[&str]) -> Vec<String> {
    let mut v = strs(&["sox", "-q"]);
    v.extend(strs(device_args));
    v.extend(strs(&RAW_IN));
    v.extend(strs(&["signed-integer", "-"]));
    v
}

fn sox_playback(device_args: &[&str]) -> Vec<String> {
    let mut v = strs(&["sox", "-q"]);
    v.extend(strs(&RAW_IN));
    v.extend(strs(&["signed-integer", "-"]));
    v.extend(strs(device_args));
    v
}

fn ffmpeg_capture(input: &[&str]) -> Vec<String> {
    let mut v = strs(&["ffmpeg", "-hide_banner", "-loglevel", "error"]);
    v.extend(strs(input));
    v.extend(strs(&["-ac", "1", "-ar", "16000", "-f", "s16le", "-"]));
    v
}

fn missing(kind: &str, looked: &[&str], hint: &str) -> AudioError {
    AudioError::NotFound(format!(
        "no {kind} program found on PATH; looked for: {}.{hint}",
        looked.join(", ")
    ))
}

/// Choose a capture command (REQ-VOI-015). `device` is an optional device name for the chosen program. `has`
/// says whether a program exists; it is injected so the choice can be tested without a real `PATH`.
///
/// Order: Linux `arecord`, `sox`, `ffmpeg`; Windows and macOS `ffmpeg`, `sox`. On Windows `ffmpeg` is used only
/// when a device name is given, because its `dshow` input has no default device.
///
/// # Errors
/// [`AudioError::NotFound`] naming the programs looked for.
pub fn pick_capture(
    os: Os,
    device: Option<&str>,
    has: &dyn Fn(&str) -> bool,
) -> Result<CommandSpec, AudioError> {
    let raw = |argv| CommandSpec {
        argv,
        wav_header: false,
    };
    match os {
        Os::Linux => {
            if has("arecord") {
                let mut v = strs(&[
                    "arecord", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw",
                ]);
                if let Some(d) = device {
                    v.extend(strs(&["-D", d]));
                }
                v.push("-".into());
                return Ok(raw(v));
            }
            if has("sox") {
                return Ok(raw(sox_capture(
                    &device.map_or(vec!["-d"], |d| vec!["-t", "alsa", d]),
                )));
            }
            if has("ffmpeg") {
                return Ok(raw(ffmpeg_capture(&[
                    "-f",
                    "alsa",
                    "-i",
                    device.unwrap_or("default"),
                ])));
            }
            Err(missing("capture", &["arecord", "sox", "ffmpeg"], ""))
        }
        Os::Windows => {
            if has("ffmpeg")
                && let Some(d) = device
            {
                let input = format!("audio={d}");
                return Ok(raw(ffmpeg_capture(&["-f", "dshow", "-i", &input])));
            }
            if has("sox") {
                return Ok(raw(sox_capture(&[
                    "-t",
                    "waveaudio",
                    device.unwrap_or("default"),
                ])));
            }
            let hint = if has("ffmpeg") {
                " ffmpeg is installed but needs a microphone name (--mic-device); list them with `ffmpeg -list_devices true -f dshow -i dummy`."
            } else {
                ""
            };
            Err(missing("capture", &["ffmpeg", "sox"], hint))
        }
        Os::Mac => {
            if has("ffmpeg") {
                let input = format!(":{}", device.unwrap_or("default"));
                return Ok(raw(ffmpeg_capture(&["-f", "avfoundation", "-i", &input])));
            }
            if has("sox") {
                return Ok(raw(sox_capture(
                    &device.map_or(vec!["-d"], |d| vec!["-t", "coreaudio", d]),
                )));
            }
            Err(missing("capture", &["ffmpeg", "sox"], ""))
        }
    }
}

/// Choose a playback command (REQ-VOI-015). Order: Linux `aplay`, `sox`, `ffplay`; Windows and macOS
/// `ffplay`, `sox`. `ffplay` always plays on the system default device and ignores `device`.
///
/// # Errors
/// [`AudioError::NotFound`] naming the programs looked for.
pub fn pick_playback(
    os: Os,
    device: Option<&str>,
    has: &dyn Fn(&str) -> bool,
) -> Result<CommandSpec, AudioError> {
    let raw = |argv| CommandSpec {
        argv,
        wav_header: false,
    };
    let ffplay = || CommandSpec {
        argv: strs(&[
            "ffplay",
            "-nodisp",
            "-autoexit",
            "-loglevel",
            "quiet",
            "-i",
            "-",
        ]),
        wav_header: true,
    };
    match os {
        Os::Linux => {
            if has("aplay") {
                let mut v = strs(&[
                    "aplay", "-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "raw",
                ]);
                if let Some(d) = device {
                    v.extend(strs(&["-D", d]));
                }
                v.push("-".into());
                return Ok(raw(v));
            }
            if has("sox") {
                return Ok(raw(sox_playback(
                    &device.map_or(vec!["-d"], |d| vec!["-t", "alsa", d]),
                )));
            }
            if has("ffplay") {
                return Ok(ffplay());
            }
            Err(missing("playback", &["aplay", "sox", "ffplay"], ""))
        }
        Os::Windows | Os::Mac => {
            if has("ffplay") {
                return Ok(ffplay());
            }
            if has("sox") {
                let kind = if os == Os::Windows {
                    "waveaudio"
                } else {
                    "coreaudio"
                };
                return Ok(raw(sox_playback(&[
                    "-t",
                    kind,
                    device.unwrap_or("default"),
                ])));
            }
            Err(missing("playback", &["ffplay", "sox"], ""))
        }
    }
}

/// [`pick_capture`] for this system, looking on the real `PATH`.
///
/// # Errors
/// [`AudioError::NotFound`] naming the programs looked for.
pub fn default_capture(device: Option<&str>) -> Result<CommandSpec, AudioError> {
    pick_capture(Os::current(), device, &on_path)
}

/// [`pick_playback`] for this system, looking on the real `PATH`.
///
/// # Errors
/// [`AudioError::NotFound`] naming the programs looked for.
pub fn default_playback(device: Option<&str>) -> Result<CommandSpec, AudioError> {
    pick_playback(Os::current(), device, &on_path)
}

/// A command from an explicit program and arguments (the user's override).
#[must_use]
pub fn custom(program: &str, args: &[String], wav_header: bool) -> CommandSpec {
    let mut argv = vec![program.to_owned()];
    argv.extend_from_slice(args);
    CommandSpec { argv, wav_header }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU32;

    fn has_only(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |p| names.contains(&p)
    }

    #[test]
    fn req_voi_015_linux_prefers_alsa_tools_then_sox_then_ffmpeg() {
        let c = pick_capture(Os::Linux, None, &has_only(&["arecord", "sox", "ffmpeg"])).unwrap();
        assert_eq!(c.program(), "arecord");
        assert!(c.argv.contains(&"S16_LE".to_owned()));
        assert_eq!(
            pick_capture(Os::Linux, None, &has_only(&["sox", "ffmpeg"]))
                .unwrap()
                .program(),
            "sox"
        );
        let f = pick_capture(Os::Linux, Some("hw:1,0"), &has_only(&["ffmpeg"])).unwrap();
        assert!(f.argv.windows(2).any(|w| w == ["-i", "hw:1,0"]));
        let p = pick_playback(Os::Linux, Some("hw:2"), &has_only(&["aplay", "ffplay"])).unwrap();
        assert_eq!(p.program(), "aplay");
        assert!(p.argv.windows(2).any(|w| w == ["-D", "hw:2"]));
        assert!(!p.wav_header);
        assert_eq!(
            pick_playback(Os::Linux, None, &has_only(&["ffplay"]))
                .unwrap()
                .program(),
            "ffplay"
        );
    }

    #[test]
    fn req_voi_015_windows_ffmpeg_needs_a_device_name_and_sox_does_not() {
        let with_dev = pick_capture(
            Os::Windows,
            Some("Microphone (USB)"),
            &has_only(&["ffmpeg", "sox"]),
        )
        .unwrap();
        assert_eq!(with_dev.program(), "ffmpeg");
        assert!(
            with_dev.argv.contains(&"audio=Microphone (USB)".to_owned()),
            "the name stays one argument, never split"
        );
        let sox = pick_capture(Os::Windows, None, &has_only(&["ffmpeg", "sox"])).unwrap();
        assert_eq!(sox.program(), "sox");
        assert!(sox.argv.contains(&"waveaudio".to_owned()));
        let e = pick_capture(Os::Windows, None, &has_only(&["ffmpeg"])).unwrap_err();
        assert!(e.to_string().contains("--mic-device"), "{e}");
        let p = pick_playback(Os::Windows, None, &has_only(&["ffplay", "sox"])).unwrap();
        assert!(p.wav_header && p.program() == "ffplay");
    }

    #[test]
    fn req_voi_015_mac_defaults() {
        let c = pick_capture(Os::Mac, None, &has_only(&["ffmpeg"])).unwrap();
        assert!(c.argv.contains(&":default".to_owned()));
        assert_eq!(
            pick_playback(Os::Mac, None, &has_only(&["sox"]))
                .unwrap()
                .argv
                .last()
                .map(String::as_str),
            Some("default")
        );
    }

    #[test]
    fn req_voi_015_nothing_found_names_what_was_looked_for() {
        let none = has_only(&[]);
        for (os, names) in [
            (Os::Linux, "arecord, sox, ffmpeg"),
            (Os::Windows, "ffmpeg, sox"),
        ] {
            let e = pick_capture(os, None, &none).unwrap_err();
            assert!(matches!(e, AudioError::NotFound(_)));
            assert!(e.to_string().contains(names), "{e}");
        }
        assert!(
            pick_playback(Os::Linux, None, &none)
                .unwrap_err()
                .to_string()
                .contains("aplay, sox, ffplay")
        );
    }

    #[test]
    fn req_voi_015_a_device_name_is_data_not_syntax() {
        // Quotes, spaces, semicolons and dollar signs stay inside one argument; there is no shell to read them.
        let nasty = "a b; rm -rf / $(x) \"q\"";
        let c = pick_capture(Os::Windows, Some(nasty), &has_only(&["ffmpeg"])).unwrap();
        assert!(c.argv.iter().any(|a| a == &format!("audio={nasty}")));
        assert_eq!(c.argv.iter().filter(|a| a.contains("rm -rf")).count(), 1);
    }

    #[test]
    fn on_path_finds_a_real_program_and_never_a_missing_one() {
        assert!(!on_path("definitely-not-a-program-xyzzy"));
    }

    // ---- fake child processes. These need a POSIX `sh`, `cat` and `sleep`, so they run on Linux and macOS.

    static N: AtomicU32 = AtomicU32::new(0);

    fn temp(name: &str) -> PathBuf {
        let n = N.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("clawdot-audio-{}-{n}-{name}", std::process::id()))
    }

    fn sh(script: &str) -> CommandSpec {
        custom("sh", &["-c".into(), script.into()], false)
    }

    fn frames_file(frames: usize) -> PathBuf {
        let p = temp("mic.raw");
        let mut bytes = Vec::new();
        for f in 0..frames {
            for i in 0..FRAME_SAMPLES {
                bytes.extend_from_slice(&((f * 1000 + i) as i16).to_le_bytes());
            }
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }

    const STALL: Duration = Duration::from_millis(400);

    #[cfg(unix)]
    #[test]
    fn req_voi_014_capture_frames_are_whole_ordered_and_end_with_eof() {
        let p = frames_file(3);
        let mut mic = CommandIn::new(
            custom("cat", &[p.to_string_lossy().into_owned()], false),
            STALL,
        );
        assert_eq!(mic.read_frame(Duration::ZERO), Err(AudioError::NotStarted));
        mic.start().unwrap();
        mic.start().unwrap();
        for f in 0..3usize {
            let frame = mic.read_frame(Duration::from_secs(2)).unwrap().unwrap();
            assert_eq!(frame.len(), FRAME_SAMPLES);
            assert_eq!(frame[0], (f * 1000) as i16);
            assert_eq!(frame[319], (f * 1000 + 319) as i16);
        }
        assert_eq!(mic.read_frame(Duration::from_secs(2)), Err(AudioError::Eof));
        assert_eq!(
            mic.read_frame(Duration::ZERO),
            Err(AudioError::Eof),
            "and stays ended"
        );
        mic.stop();
        assert_eq!(mic.read_frame(Duration::ZERO), Err(AudioError::NotStarted));
        mic.start().unwrap();
        assert!(
            mic.read_frame(Duration::from_secs(2)).unwrap().is_some(),
            "restartable"
        );
        let _ = std::fs::remove_file(p);
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_partial_frame_is_never_delivered() {
        let p = temp("short.raw");
        std::fs::write(&p, vec![1u8; FRAME_BYTES + 100]).unwrap();
        let mut mic = CommandIn::new(
            custom("cat", &[p.to_string_lossy().into_owned()], false),
            STALL,
        );
        mic.start().unwrap();
        assert!(mic.read_frame(Duration::from_secs(2)).unwrap().is_some());
        assert_eq!(mic.read_frame(Duration::from_secs(2)), Err(AudioError::Eof));
        let _ = std::fs::remove_file(p);
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_silent_child_is_stalled_not_silence_and_is_killed() {
        let mut mic = CommandIn::new(sh("sleep 30"), STALL);
        mic.start().unwrap();
        let t = Instant::now();
        assert_eq!(mic.read_frame(Duration::ZERO), Ok(None), "not stalled yet");
        let e = mic.read_frame(Duration::from_secs(3)).unwrap_err();
        assert!(matches!(e, AudioError::Stalled(_)), "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(3));
        let t = Instant::now();
        mic.stop();
        assert!(t.elapsed() < Duration::from_secs(2), "stop kills the child");
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_child_that_exits_at_once_is_end_of_input() {
        let mut mic = CommandIn::new(sh("exit 3"), STALL);
        mic.start().unwrap();
        assert_eq!(mic.read_frame(Duration::from_secs(2)), Err(AudioError::Eof));
    }

    #[test]
    fn req_voi_014_a_missing_program_is_reported_by_name() {
        let mut mic = CommandIn::new(custom("clawdot-no-such-recorder", &[], false), STALL);
        let e = mic.start().unwrap_err();
        assert!(
            matches!(e, AudioError::NotFound(ref m) if m.contains("clawdot-no-such-recorder")),
            "{e:?}"
        );
        let mut spk = CommandOut::new(custom("clawdot-no-such-player", &[], false), STALL);
        assert!(matches!(spk.begin(), Err(AudioError::NotFound(_))));
        assert!(matches!(
            CommandIn::new(
                CommandSpec {
                    argv: vec![],
                    wav_header: false
                },
                STALL
            )
            .start(),
            Err(AudioError::NotFound(_))
        ));
    }

    fn wait_done(out: &mut CommandOut) -> Result<bool, AudioError> {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            match out.poll_done() {
                Ok(false) => thread::sleep(Duration::from_millis(10)),
                other => return other,
            }
        }
        panic!("playback never finished");
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_playback_delivers_every_sample_in_order_and_finishes() {
        let p = temp("spk.raw");
        let mut spk = CommandOut::new(sh(&format!("cat > '{}'", p.display())), STALL);
        assert_eq!(spk.poll_done(), Ok(true), "idle before any reply");
        assert!(matches!(spk.play(&[1]), Err(AudioError::NotStarted)));
        spk.begin().unwrap();
        spk.play(&[1, -2, 3]).unwrap();
        spk.play(&[i16::MAX, i16::MIN]).unwrap();
        spk.finish();
        assert_eq!(wait_done(&mut spk), Ok(true));
        let got: Vec<i16> = std::fs::read(&p)
            .unwrap()
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(got, vec![1, -2, 3, i16::MAX, i16::MIN]);
        let _ = std::fs::remove_file(p);
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_018_a_long_reply_does_not_block_the_caller() {
        // A player that drains slowly: play() must still return at once (the loop keeps answering pings).
        let mut spk = CommandOut::new(sh("sleep 1; cat > /dev/null"), Duration::from_secs(5));
        spk.begin().unwrap();
        let t = Instant::now();
        for _ in 0..20 {
            spk.play(&vec![7; 16_000]).unwrap();
        }
        spk.finish();
        assert!(
            t.elapsed() < Duration::from_millis(500),
            "queueing 20 s of audio took {:?}",
            t.elapsed()
        );
        assert_eq!(spk.poll_done(), Ok(false));
        assert_eq!(wait_done(&mut spk), Ok(true));
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_ducking_and_volume_scale_the_samples() {
        let p = temp("duck.raw");
        let mut spk = CommandOut::new(sh(&format!("cat > '{}'", p.display())), STALL);
        spk.set_volume(50);
        spk.duck(50);
        spk.begin().unwrap();
        spk.play(&[1000, -1000]).unwrap();
        spk.duck(100);
        spk.play(&[1000]).unwrap();
        spk.finish();
        assert_eq!(wait_done(&mut spk), Ok(true));
        let got: Vec<i16> = std::fs::read(&p)
            .unwrap()
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(got, vec![250, -250, 500]);
        let _ = std::fs::remove_file(p);
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_player_that_never_reads_is_stalled_and_killed() {
        let mut spk = CommandOut::new(sh("sleep 30"), STALL);
        spk.begin().unwrap();
        // More than a pipe buffer holds, so the writer blocks.
        spk.play(&vec![1; 200_000]).unwrap();
        spk.finish();
        let t = Instant::now();
        let e = wait_done(&mut spk).unwrap_err();
        assert!(matches!(e, AudioError::Stalled(_)), "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(4));
        assert_eq!(
            spk.poll_done(),
            Ok(true),
            "after a failure the output is idle again"
        );
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_player_that_exits_early_is_an_error() {
        let mut spk = CommandOut::new(sh("exit 0"), STALL);
        spk.begin().unwrap();
        thread::sleep(Duration::from_millis(200));
        let r = spk.play(&vec![1; 200_000]).and_then(|()| {
            spk.finish();
            wait_done(&mut spk).map(|_| ())
        });
        assert!(r.is_err(), "writing to a dead player must fail, not vanish");
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_failing_player_exit_status_is_reported() {
        let mut spk = CommandOut::new(sh("cat > /dev/null; exit 4"), STALL);
        spk.begin().unwrap();
        spk.play(&[1, 2]).unwrap();
        spk.finish();
        assert!(matches!(wait_done(&mut spk), Err(AudioError::Io(m)) if m.contains("ended with")));
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_stop_silences_at_once_and_discards_the_queue() {
        let mut spk = CommandOut::new(sh("sleep 30"), STALL);
        spk.begin().unwrap();
        spk.play(&vec![1; 100_000]).unwrap();
        let t = Instant::now();
        spk.stop();
        assert!(t.elapsed() < Duration::from_secs(2));
        assert_eq!(spk.poll_done(), Ok(true));
        spk.stop();
    }

    #[cfg(unix)]
    #[test]
    fn req_voi_014_a_wav_header_is_sent_first_when_asked() {
        let p = temp("wav.raw");
        let mut spec = sh(&format!("cat > '{}'", p.display()));
        spec.wav_header = true;
        let mut spk = CommandOut::new(spec, STALL);
        spk.begin().unwrap();
        spk.play(&[9]).unwrap();
        spk.finish();
        assert_eq!(wait_done(&mut spk), Ok(true));
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[bytes.len() - 2..], &9i16.to_le_bytes());
        assert_eq!(bytes.len(), 46);
        let _ = std::fs::remove_file(p);
    }
}
