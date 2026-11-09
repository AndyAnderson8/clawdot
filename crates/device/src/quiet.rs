//! Quiet hours, applied locally (REQ-DEV-006).
//!
//! The settings live on the device and apply even when the core is unreachable: a volume ceiling, an LED
//! brightness ceiling and deferral of unsolicited announcements. With no settings, nothing is quiet.

use std::time::{SystemTime, UNIX_EPOCH};

/// A daily window and what it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    start: u16,
    end: u16,
    /// Speaker volume ceiling during the window, percent.
    pub volume_ceiling: u8,
    /// LED brightness during the window, percent.
    pub led_pct: u8,
}

/// Why quiet hours could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietError(pub String);

impl std::fmt::Display for QuietError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QuietError {}

fn hhmm(s: &str) -> Result<u16, QuietError> {
    let bad = || QuietError(format!("`{s}` is not a time like 22:00"));
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    let (h, m): (u16, u16) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 {
        return Err(bad());
    }
    Ok(h * 60 + m)
}

impl QuietHours {
    /// Parse `HH:MM-HH:MM`. A window that ends before it starts crosses midnight. Equal times are refused (an
    /// empty or all-day window is ambiguous).
    ///
    /// # Errors
    /// [`QuietError`] saying what is wrong.
    pub fn parse(window: &str, volume_ceiling: u8, led_pct: u8) -> Result<Self, QuietError> {
        let (a, b) = window
            .split_once('-')
            .ok_or_else(|| QuietError(format!("`{window}` is not a window like 22:00-07:00")))?;
        let (start, end) = (hhmm(a.trim())?, hhmm(b.trim())?);
        if start == end {
            return Err(QuietError(
                "the window starts and ends at the same time".into(),
            ));
        }
        Ok(Self {
            start,
            end,
            volume_ceiling: volume_ceiling.min(100),
            led_pct: led_pct.min(100),
        })
    }

    /// Whether `minute_of_day` (0 to 1439, local time) is inside the window.
    #[must_use]
    pub fn active(&self, minute_of_day: u16) -> bool {
        let m = minute_of_day % 1440;
        if self.start < self.end {
            m >= self.start && m < self.end
        } else {
            m >= self.start || m < self.end
        }
    }
}

/// What quiet hours do to a request, given the current minute.
#[derive(Debug, Clone, Copy, Default)]
pub struct QuietPolicy {
    hours: Option<QuietHours>,
}

impl QuietPolicy {
    /// A policy with the given window, or none.
    #[must_use]
    pub fn new(hours: Option<QuietHours>) -> Self {
        Self { hours }
    }

    fn quiet(&self, minute: u16) -> Option<QuietHours> {
        self.hours.filter(|h| h.active(minute))
    }

    /// The volume to use for a request of `level`: capped during quiet hours.
    #[must_use]
    pub fn volume(&self, level: u8, minute: u16) -> u8 {
        let level = level.min(100);
        self.quiet(minute)
            .map_or(level, |h| level.min(h.volume_ceiling))
    }

    /// LED brightness to use, or `None` to leave the board's own brightness.
    #[must_use]
    pub fn led_brightness(&self, minute: u16) -> Option<u8> {
        self.quiet(minute).map(|h| h.led_pct)
    }

    /// Whether an unsolicited announcement (a timer chime is not one; an inbox read-aloud is) must wait.
    #[must_use]
    pub fn defer_announcement(&self, minute: u16) -> bool {
        self.quiet(minute).is_some()
    }
}

/// The current minute of the day at a fixed offset from UTC, in minutes (for example -360). The daemon has no
/// time-zone database; the offset comes from its settings.
#[must_use]
pub fn minute_of_day_now(utc_offset_min: i32) -> u16 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let minutes = i64::try_from(secs / 60).unwrap_or(0) + i64::from(utc_offset_min);
    u16::try_from(minutes.rem_euclid(1440)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_dev_006_a_window_across_midnight() {
        let q = QuietHours::parse("22:00-07:00", 20, 5).unwrap();
        assert!(q.active(22 * 60));
        assert!(q.active(23 * 60 + 59));
        assert!(q.active(0));
        assert!(q.active(6 * 60 + 59));
        assert!(!q.active(7 * 60));
        assert!(!q.active(12 * 60));
    }

    #[test]
    fn req_dev_006_a_window_within_a_day() {
        let q = QuietHours::parse("13:00-15:00", 20, 5).unwrap();
        assert!(q.active(13 * 60));
        assert!(!q.active(15 * 60));
        assert!(!q.active(2 * 60));
    }

    #[test]
    fn req_dev_006_bad_windows_are_refused() {
        for bad in [
            "",
            "22:00",
            "25:00-07:00",
            "22:60-07:00",
            "7-8",
            "08:00-08:00",
            "a:b-c:d",
        ] {
            assert!(QuietHours::parse(bad, 20, 5).is_err(), "{bad}");
        }
    }

    #[test]
    fn req_dev_006_quiet_hours_cap_volume_dim_leds_and_defer_announcements() {
        let p = QuietPolicy::new(Some(QuietHours::parse("22:00-07:00", 20, 5).unwrap()));
        let night = 23 * 60;
        let day = 12 * 60;
        assert_eq!(p.volume(80, night), 20);
        assert_eq!(p.volume(10, night), 10);
        assert_eq!(p.volume(80, day), 80);
        assert_eq!(p.led_brightness(night), Some(5));
        assert_eq!(p.led_brightness(day), None);
        assert!(p.defer_announcement(night));
        assert!(!p.defer_announcement(day));
    }

    #[test]
    fn req_dev_006_no_settings_means_no_quiet_hours() {
        let p = QuietPolicy::default();
        assert_eq!(p.volume(90, 0), 90);
        assert_eq!(p.led_brightness(0), None);
        assert!(!p.defer_announcement(0));
    }

    #[test]
    fn the_clock_offset_wraps_into_a_day() {
        for off in [-720, -1, 0, 1, 840, 5000, -5000] {
            assert!(minute_of_day_now(off) < 1440);
        }
    }
}
