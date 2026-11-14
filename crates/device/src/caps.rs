//! The capability model (spec 0006 section 5).
//!
//! What the daemon declares is what the board file allows **and** what this build implements. A capability that
//! is absent is declared absent with its documented equivalent; nothing is faked (REQ-DEV-013).

use crate::board_file::{BoardFile, ButtonSource, DuckKind, LedDriverKind, MuteKind, UpdateMode};
use clawdot_protocol::message::{AudioCodec, Capabilities};

/// What this build of the daemon implements. Raised as features are added; a board file cannot declare more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Build {
    /// Console LEDs exist. Device LED drivers do not yet.
    pub console_leds: bool,
    /// Terminal-key buttons exist. Input-device buttons do not yet.
    pub key_buttons: bool,
    /// Echo cancellation exists.
    pub aec: bool,
    /// A wake engine exists.
    pub wake_engine: bool,
    /// Bluetooth speaker mode exists.
    pub bluetooth: bool,
    /// A Spotify Connect receiver exists.
    pub spotify_connect: bool,
}

impl Build {
    /// This build: console LEDs and terminal keys only; no AEC, wake engine, Bluetooth or Connect.
    pub const CURRENT: Build = Build {
        console_leds: true,
        key_buttons: true,
        aec: false,
        wake_engine: false,
        bluetooth: false,
        spotify_connect: false,
    };

    /// Whether this build can drive the board's LEDs. The one place that decides it: the capability model and the
    /// board assembly both ask here, so what is declared is what is built.
    #[must_use]
    pub fn drives_leds(&self, board: &BoardFile) -> bool {
        board
            .leds
            .as_ref()
            .is_some_and(|l| l.driver == LedDriverKind::Console)
            && self.console_leds
    }

    /// Whether this build can read every button the board declares (and there is at least one). The one place
    /// that decides it.
    #[must_use]
    pub fn reads_buttons(&self, board: &BoardFile) -> bool {
        !board.buttons.is_empty()
            && board
                .buttons
                .iter()
                .all(|b| matches!(b.source, ButtonSource::Key(_)))
            && self.key_buttons
    }
}

/// Required, Should or Optional (spec 0006 section 5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A supported board provides it, possibly through its equivalent.
    Required,
    /// Expected on most boards.
    Should,
    /// Absence is normal.
    Optional,
}

/// One capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Microphone.
    AudioIn,
    /// Speaker.
    AudioOut,
    /// Built-in tones.
    Tone,
    /// LED states.
    Leds,
    /// Buttons.
    Buttons,
    /// Microphone mute control (any kind).
    Mute,
    /// Mute that is proved electrical.
    HardwareMute,
    /// Stop and cancel without the wake word.
    LocalStop,
    /// Reopening the microphone after a reply.
    FollowUp,
    /// Ducking the daemon's own audio.
    DuckOwn,
    /// Ducking other sources through a mixer.
    DuckOthers,
    /// Quiet-hours hook.
    QuietHours,
    /// Wake word on the device.
    WakeWord,
    /// Echo cancellation.
    Aec,
    /// Wi-Fi setup without a keyboard.
    WifiSetup,
    /// Clock trust reporting.
    ClockTrust,
    /// Signed A/B updates.
    UpdatesAb,
    /// Bluetooth speaker mode.
    Bluetooth,
    /// Spotify Connect receiver.
    SpotifyConnect,
}

impl Capability {
    /// Every capability, in table order.
    pub const ALL: [Capability; 19] = [
        Self::AudioIn,
        Self::AudioOut,
        Self::Tone,
        Self::Leds,
        Self::Buttons,
        Self::Mute,
        Self::HardwareMute,
        Self::LocalStop,
        Self::FollowUp,
        Self::DuckOwn,
        Self::DuckOthers,
        Self::QuietHours,
        Self::WakeWord,
        Self::Aec,
        Self::WifiSetup,
        Self::ClockTrust,
        Self::UpdatesAb,
        Self::Bluetooth,
        Self::SpotifyConnect,
    ];

    /// A short name for status and test output.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AudioIn => "audio_in",
            Self::AudioOut => "audio_out",
            Self::Tone => "tone",
            Self::Leds => "leds",
            Self::Buttons => "buttons",
            Self::Mute => "mute",
            Self::HardwareMute => "hardware_mute",
            Self::LocalStop => "local_stop",
            Self::FollowUp => "follow_up",
            Self::DuckOwn => "duck_own",
            Self::DuckOthers => "duck_others",
            Self::QuietHours => "quiet_hours",
            Self::WakeWord => "wake_word",
            Self::Aec => "aec",
            Self::WifiSetup => "wifi_setup",
            Self::ClockTrust => "clock_trust",
            Self::UpdatesAb => "updates_ab",
            Self::Bluetooth => "bluetooth",
            Self::SpotifyConnect => "spotify_connect",
        }
    }

    /// The class in the contract.
    #[must_use]
    pub fn class(self) -> Class {
        match self {
            Self::AudioIn
            | Self::AudioOut
            | Self::Tone
            | Self::Mute
            | Self::LocalStop
            | Self::DuckOwn
            | Self::QuietHours => Class::Required,
            Self::Leds | Self::Buttons | Self::FollowUp | Self::WifiSetup | Self::ClockTrust => {
                Class::Should
            }
            _ => Class::Optional,
        }
    }

    /// What the user gets when the capability is absent (spec 0006 section 5.1). Required capabilities that
    /// have no equivalent say so.
    #[must_use]
    pub fn equivalent(self) -> &'static str {
        match self {
            Self::AudioIn => "none: a board without a microphone is unsupported",
            Self::AudioOut => "none: a board without a speaker is unsupported",
            Self::Tone => "always present",
            Self::Leds => {
                "tones for wake, error, timer and inbox; the portal status line shows the state"
            }
            Self::Buttons => "talk, stop, mute and volume from the portal or the protocol",
            Self::Mute => "software mute from the portal or the protocol",
            Self::HardwareMute => {
                "software mute; the assistant never says the microphone is disconnected"
            }
            Self::LocalStop => {
                "the portal Stop control and the stop-playback command, acted on locally first"
            }
            Self::FollowUp => "wake word or push-to-talk again for each turn",
            Self::DuckOwn => "always present: the daemon scales its own audio",
            Self::DuckOthers => "pause the other source while the agent speaks, then resume",
            Self::QuietHours => "always present",
            Self::WakeWord => "core-side detection on the uplink stream, or push-to-talk",
            Self::Aec => {
                "half-duplex: the microphone is closed while the assistant speaks; barge-in off"
            }
            Self::WifiSetup => "a wired or pre-configured network, per the board's install guide",
            Self::ClockTrust => "the core's clock status governs timers",
            Self::UpdatesAb => "manual updates, declared in status",
            Self::Bluetooth => "none: the speaker plays only the assistant and its streams",
            Self::SpotifyConnect => "Spotify control through the assistant still works",
        }
    }
}

/// The result of applying the board file and the build to the capability list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    present: Vec<Capability>,
    /// What the board file would allow but this build does not implement yet, with the reason.
    pub not_implemented: Vec<(Capability, &'static str)>,
    /// The mute kind (so callers can say `software` or `hardware`).
    pub mute_kind: MuteKind,
    /// The update mode.
    pub update: UpdateMode,
}

impl Declared {
    /// Work out what is present: allowed by the board file and implemented by `build`.
    #[must_use]
    pub fn from_board(board: &BoardFile, build: Build) -> Self {
        use Capability as C;
        let mut present = vec![
            C::Tone,
            C::Mute,
            C::LocalStop,
            C::FollowUp,
            C::DuckOwn,
            C::QuietHours,
        ];
        let mut not_implemented = Vec::new();
        let mut gate = |cap: Capability, wanted: bool, implemented: bool, why: &'static str| {
            if wanted && implemented {
                present.push(cap);
            } else if wanted {
                not_implemented.push((cap, why));
            }
        };
        use crate::board_file::AudioKind;
        gate(
            C::AudioIn,
            true,
            board.audio.capture.kind != AudioKind::Alsa,
            "the ALSA backend is not built; use `default` or `command`",
        );
        gate(
            C::AudioOut,
            true,
            board.audio.playback.kind != AudioKind::Alsa,
            "the ALSA backend is not built; use `default` or `command`",
        );
        gate(
            C::Leds,
            board.leds.is_some(),
            build.drives_leds(board),
            "this build has only the console LED driver",
        );
        gate(
            C::Buttons,
            !board.buttons.is_empty(),
            build.reads_buttons(board),
            "this build reads terminal keys only, not input devices",
        );
        gate(
            C::HardwareMute,
            board.mute.kind == MuteKind::Hardware,
            true,
            "",
        );
        gate(
            C::DuckOthers,
            board.audio.playback.duck == DuckKind::Mixer,
            false,
            "mixer ducking needs the ALSA backend",
        );
        gate(
            C::WakeWord,
            board.features.on_device_wake,
            build.wake_engine,
            "no wake engine is built yet; the seam and push-to-talk exist",
        );
        gate(
            C::Aec,
            board.features.aec,
            build.aec,
            "no echo canceller is built yet",
        );
        gate(
            C::WifiSetup,
            board.wifi.is_some(),
            false,
            "Wi-Fi setup belongs to the image (B5)",
        );
        gate(
            C::ClockTrust,
            board.clock.is_some(),
            false,
            "clock trust reporting is not built yet",
        );
        gate(
            C::UpdatesAb,
            board.update == UpdateMode::Ab,
            false,
            "updates belong to the image (B5)",
        );
        gate(
            C::Bluetooth,
            board.features.bluetooth_sink || board.features.bluetooth_source,
            build.bluetooth,
            "Bluetooth speaker mode is not built yet",
        );
        gate(
            C::SpotifyConnect,
            board.features.spotify_connect,
            build.spotify_connect,
            "no Connect receiver is built yet",
        );
        Self {
            present,
            not_implemented,
            mute_kind: board.mute.kind,
            update: board.update,
        }
    }

    /// What a board with no hardware declares: only the software features (the null board).
    #[must_use]
    pub fn software_only() -> Self {
        use Capability as C;
        Self {
            present: vec![
                C::Tone,
                C::Mute,
                C::LocalStop,
                C::FollowUp,
                C::DuckOwn,
                C::QuietHours,
            ],
            not_implemented: Vec::new(),
            mute_kind: MuteKind::None,
            update: UpdateMode::Manual,
        }
    }

    /// Whether `cap` is declared present.
    #[must_use]
    pub fn has(&self, cap: Capability) -> bool {
        self.present.contains(&cap)
    }

    /// The `hello` capabilities for the satellite protocol (REQ-DEV-002): everything present and nothing absent.
    #[must_use]
    pub fn to_protocol(&self) -> Capabilities {
        Capabilities {
            wake_word: self.has(Capability::WakeWord),
            leds: self.has(Capability::Leds),
            buttons: self.has(Capability::Buttons),
            display: false,
            hardware_mute: self.has(Capability::HardwareMute),
            aec: self.has(Capability::Aec),
            noise_suppression: false,
            vad: false,
            beamforming: false,
            audio_out: vec![AudioCodec::Pcm],
            media_url: false,
        }
    }

    /// A readable report: one line per capability with its class, presence and, if absent, the equivalent.
    #[must_use]
    pub fn report(&self) -> String {
        let mut out = String::new();
        for cap in Capability::ALL {
            let class = match cap.class() {
                Class::Required => "required",
                Class::Should => "should",
                Class::Optional => "optional",
            };
            if self.has(cap) {
                out.push_str(&format!("{:<16} {class:<9} present\n", cap.name()));
            } else {
                let note = self
                    .not_implemented
                    .iter()
                    .find(|(c, _)| *c == cap)
                    .map_or(String::new(), |(_, why)| {
                        format!(" (board allows it; {why})")
                    });
                out.push_str(&format!(
                    "{:<16} {class:<9} absent{note}; instead: {}\n",
                    cap.name(),
                    cap.equivalent()
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(json: &str) -> BoardFile {
        BoardFile::parse(json).unwrap()
    }

    const DESKTOP: &str = include_str!("../../../boards/desktop.json");
    const DOT: &str = include_str!("../../../boards/echo-dot2.json");

    #[test]
    fn req_dev_013_the_desktop_declares_what_it_has_and_nothing_else() {
        let d = Declared::from_board(&board(DESKTOP), Build::CURRENT);
        assert!(d.has(Capability::Leds));
        assert!(d.has(Capability::Buttons));
        assert!(d.has(Capability::LocalStop));
        assert!(!d.has(Capability::HardwareMute));
        assert!(!d.has(Capability::Aec));
        // The desktop board file says on_device_wake, but this build has no engine, so it is not declared.
        assert!(!d.has(Capability::WakeWord));
        assert!(
            d.not_implemented
                .iter()
                .any(|(c, _)| *c == Capability::WakeWord)
        );
        let p = d.to_protocol();
        assert!(p.leds && p.buttons && !p.wake_word && !p.hardware_mute && !p.aec);
    }

    #[test]
    fn req_dev_013_a_dot_file_declares_no_device_drivers_this_build_lacks() {
        let d = Declared::from_board(&board(DOT), Build::CURRENT);
        assert!(!d.has(Capability::Leds), "ledctrl driver is not built");
        assert!(!d.has(Capability::Buttons), "evdev buttons are not built");
        assert!(!d.has(Capability::UpdatesAb));
        assert!(d.has(Capability::Mute));
        assert_eq!(d.mute_kind, MuteKind::Software);
        assert!(
            !d.to_protocol().hardware_mute,
            "software mute is never claimed as hardware"
        );
    }

    #[test]
    fn req_dev_013_every_required_capability_has_an_equivalent_or_is_present() {
        for json in [DESKTOP, DOT] {
            let d = Declared::from_board(&board(json), Build::CURRENT);
            for cap in Capability::ALL {
                assert!(!cap.equivalent().is_empty());
                // Audio has no equivalent: a board without it is unsupported, and this build cannot open the
                // Dot's ALSA devices yet (see the next test).
                if cap.class() == Class::Required
                    && !d.has(cap)
                    && !matches!(cap, Capability::AudioIn | Capability::AudioOut)
                {
                    panic!("{} is required but absent", cap.name());
                }
            }
        }
    }

    #[test]
    fn req_dev_013_an_alsa_board_has_no_audio_in_this_build() {
        let d = Declared::from_board(&board(DOT), Build::CURRENT);
        assert!(!d.has(Capability::AudioIn) && !d.has(Capability::AudioOut));
        assert!(d.report().contains("the ALSA backend is not built"));
        let d = Declared::from_board(&board(DESKTOP), Build::CURRENT);
        assert!(d.has(Capability::AudioIn) && d.has(Capability::AudioOut));
    }

    #[test]
    fn req_dev_013_the_report_lists_the_equivalent_of_each_absent_capability() {
        let d = Declared::from_board(&board(DESKTOP), Build::CURRENT);
        let r = d.report();
        assert_eq!(r.lines().count(), Capability::ALL.len());
        assert!(r.contains("aec") && r.contains("half-duplex"));
        assert!(r.contains("wake_word") && r.contains("no wake engine is built yet"));
    }
}
