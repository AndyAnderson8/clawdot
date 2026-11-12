//! Music and radio: the service-agnostic media model and the built-in stream backend
//! (REQ-MUS-001, 002, 003).
//!
//! A backend has a **shape**. A *playback* backend hands the speaker a stream to play, which is what the
//! built-in station list does. A *control* backend commands a player somewhere else (Spotify Connect,
//! MPD, a Home Assistant media player); those are built from declarative HTTP tools (see
//! `docs/architecture/media.md`) and play on the controlled device, not on this speaker.
//!
//! The [`Player`] here is pure state: which station is selected and whether it is playing. It never touches
//! audio. It returns [`Action`]s that the satellite carries out, which keeps it testable and keeps ducking
//! and pausing for speech, alarms, and announcements (REQ-MUS-004) in one place, the satellite.

use crate::lang::Language;
use serde::Deserialize;

/// What a backend can do (REQ-MUS-003).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// Find something by name.
    pub search: bool,
    /// Pause and resume.
    pub pause: bool,
    /// Next and previous.
    pub skip: bool,
    /// Report what is playing.
    pub now_playing: bool,
}

/// Who produces the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// ClawDot plays the stream on its own speaker.
    Playback,
    /// ClawDot commands a player elsewhere.
    Control,
}

/// A radio station or plain HTTP audio stream, as configured.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Station {
    /// What the user says, for example `jazz` or `bbc world service`.
    pub name: String,
    /// The stream URL (`http://` or `https://`).
    pub url: String,
}

impl Station {
    /// Check a configured station.
    ///
    /// # Errors
    /// A message naming the problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 60 {
            return Err("a station needs a name of up to 60 characters".into());
        }
        if !(self.url.starts_with("http://") || self.url.starts_with("https://"))
            || self.url.contains(char::is_whitespace)
        {
            return Err(format!(
                "station `{}`: the URL must start with http:// or https://",
                self.name
            ));
        }
        Ok(())
    }
}

/// What the satellite should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start playing a stream, replacing whatever was playing.
    Play {
        /// Stream URL.
        url: String,
        /// Name to show and speak.
        title: String,
    },
    /// Pause playback, keeping the position.
    Pause,
    /// Continue after a pause.
    Resume,
}

/// Why a request could not be met; the text is spoken to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaError(pub String);

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The built-in playback backend: a list of stations.
#[derive(Debug, Clone)]
pub struct Player {
    lang: &'static Language,
    stations: Vec<Station>,
    current: Option<usize>,
    playing: bool,
}

impl Player {
    /// A player for these stations, understanding "some music" in `lang`.
    #[must_use]
    pub fn new(lang: &'static Language, stations: Vec<Station>) -> Self {
        Self {
            lang,
            stations,
            current: None,
            playing: false,
        }
    }

    /// Whether any station is configured.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        !self.stations.is_empty()
    }

    /// This backend's shape and abilities.
    #[must_use]
    pub fn describe(&self) -> (Shape, Capabilities) {
        (
            Shape::Playback,
            Capabilities {
                search: true,
                pause: true,
                skip: true,
                now_playing: true,
            },
        )
    }

    /// The station that is selected, if any.
    #[must_use]
    pub fn now_playing(&self) -> Option<&Station> {
        self.current.and_then(|i| self.stations.get(i))
    }

    /// Whether sound is supposed to be coming out.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Station names, for telling the user what is available.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.stations.iter().map(|s| s.name.as_str()).collect()
    }

    fn start(&mut self, i: usize) -> Action {
        self.current = Some(i);
        self.playing = true;
        let s = &self.stations[i];
        Action::Play {
            url: s.url.clone(),
            title: s.name.clone(),
        }
    }

    /// Find the best station for what the user asked for: the one sharing the most words, ties broken by
    /// list order. A request with no specific words ("some music") starts the first station.
    fn find(&self, query: &str) -> Option<usize> {
        let q: Vec<String> = words(query)
            .into_iter()
            .filter(|w| !self.lang.generic_music_words.contains(&w.as_str()))
            .collect();
        if q.is_empty() {
            return (!self.stations.is_empty()).then_some(0);
        }
        let mut best: Option<(usize, usize)> = None;
        for (i, s) in self.stations.iter().enumerate() {
            let name = words(&s.name);
            let hits = q.iter().filter(|w| name.contains(w)).count();
            if hits > 0 && best.is_none_or(|(_, h)| hits > h) {
                best = Some((i, hits));
            }
        }
        best.map(|(i, _)| i)
    }

    /// Play something by name.
    ///
    /// # Errors
    /// No stations are set up, or none matches.
    pub fn play(&mut self, query: &str) -> Result<Action, MediaError> {
        if self.stations.is_empty() {
            return Err(MediaError("No music is set up yet.".into()));
        }
        let i = self.find(query).ok_or_else(|| {
            MediaError(format!(
                "I don't have that station. I have {}.",
                self.names().join(", ")
            ))
        })?;
        Ok(self.start(i))
    }

    /// Pause.
    ///
    /// # Errors
    /// Nothing is playing.
    pub fn pause(&mut self) -> Result<Action, MediaError> {
        if !self.playing {
            return Err(MediaError("Nothing is playing.".into()));
        }
        self.playing = false;
        Ok(Action::Pause)
    }

    /// Resume after a pause, or start the first station if nothing was chosen yet.
    ///
    /// # Errors
    /// No stations are set up.
    pub fn resume(&mut self) -> Result<Action, MediaError> {
        match self.current {
            Some(_) if self.playing => Err(MediaError("It is already playing.".into())),
            Some(_) => {
                self.playing = true;
                Ok(Action::Resume)
            }
            None => self.play(""),
        }
    }

    /// The next station, wrapping around.
    ///
    /// # Errors
    /// No stations are set up.
    pub fn next_station(&mut self) -> Result<Action, MediaError> {
        self.step(true)
    }

    /// The previous station, wrapping around.
    ///
    /// # Errors
    /// No stations are set up.
    pub fn previous_station(&mut self) -> Result<Action, MediaError> {
        self.step(false)
    }

    fn step(&mut self, forward: bool) -> Result<Action, MediaError> {
        let n = self.stations.len();
        if n == 0 {
            return Err(MediaError("No music is set up yet.".into()));
        }
        let to = match (self.current, forward) {
            (None, true) => 0,
            (None, false) => n - 1,
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
        };
        Ok(self.start(to))
    }

    /// Stop everything (the user said "stop").
    pub fn stop(&mut self) {
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(name: &str) -> Station {
        Station {
            name: name.into(),
            url: format!("http://radio.example/{}", name.replace(' ', "-")),
        }
    }

    fn player() -> Player {
        Player::new(
            crate::lang::DEFAULT,
            vec![
                st("jazz"),
                st("bbc world service"),
                st("classic rock"),
                st("rock classics"),
            ],
        )
    }

    fn title(a: &Action) -> &str {
        match a {
            Action::Play { title, .. } => title,
            _ => panic!("expected Play, got {a:?}"),
        }
    }

    #[test]
    fn stations_are_validated() {
        assert!(st("jazz").validate().is_ok());
        assert!(
            Station {
                name: " ".into(),
                url: "http://x".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Station {
                name: "a".into(),
                url: "ftp://x".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            Station {
                name: "a".into(),
                url: "http://x y".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            serde_json::from_str::<Station>(r#"{"name":"a","url":"http://x","oops":1}"#).is_err()
        );
    }

    #[test]
    fn it_plays_the_station_that_best_matches_what_was_said() {
        let mut p = player();
        assert_eq!(title(&p.play("jazz").unwrap()), "jazz");
        assert_eq!(
            title(&p.play("the bbc world service").unwrap()),
            "bbc world service"
        );
        assert_eq!(title(&p.play("some classic rock").unwrap()), "classic rock");
        // Ties go to the earlier station.
        assert_eq!(title(&p.play("rock").unwrap()), "classic rock");
        assert!(p.is_playing());
    }

    #[test]
    fn a_generic_request_starts_the_first_station() {
        let mut p = player();
        assert_eq!(title(&p.play("some music").unwrap()), "jazz");
        let mut p = player();
        assert_eq!(title(&p.resume().unwrap()), "jazz");
    }

    #[test]
    fn an_unknown_station_lists_what_exists() {
        let e = player().play("polka").unwrap_err();
        assert!(
            e.0.contains("jazz") && e.0.contains("bbc world service"),
            "{e}"
        );
    }

    #[test]
    fn nothing_configured_means_nothing_to_play() {
        let mut p = Player::new(crate::lang::DEFAULT, vec![]);
        assert!(!p.is_configured());
        assert!(p.play("jazz").unwrap_err().0.contains("set up"));
        assert!(p.next_station().is_err() && p.resume().is_err());
    }

    #[test]
    fn pause_resume_and_stop() {
        let mut p = player();
        assert!(p.pause().unwrap_err().0.contains("Nothing"));
        p.play("jazz").unwrap();
        assert_eq!(p.pause().unwrap(), Action::Pause);
        assert!(!p.is_playing());
        assert_eq!(p.resume().unwrap(), Action::Resume);
        assert!(p.resume().unwrap_err().0.contains("already"));
        p.stop();
        assert!(!p.is_playing());
        // After stop the selection is kept, so resume continues it.
        assert_eq!(p.resume().unwrap(), Action::Resume);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut p = player();
        assert_eq!(title(&p.next_station().unwrap()), "jazz");
        assert_eq!(title(&p.next_station().unwrap()), "bbc world service");
        assert_eq!(title(&p.previous_station().unwrap()), "jazz");
        assert_eq!(title(&p.previous_station().unwrap()), "rock classics");
        assert_eq!(title(&p.next_station().unwrap()), "jazz");
        assert_eq!(p.now_playing().unwrap().name, "jazz");
    }

    #[test]
    fn it_describes_itself_as_a_playback_backend() {
        let (shape, caps) = player().describe();
        assert_eq!(shape, Shape::Playback);
        assert!(caps.search && caps.pause && caps.skip && caps.now_playing);
    }
}
