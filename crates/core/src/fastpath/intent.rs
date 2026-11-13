//! The built-in intent set.

/// What a built-in command asks for. The grammar of each language maps phrases to these, and the
/// dispatcher handles each one, so adding an intent without handling it does not compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Intent {
    /// `stop`
    Stop,
    /// `inbox.status`
    InboxStatus,
    /// `inbox.details`
    InboxDetails,
    /// `inbox.snooze`
    InboxSnooze,
    /// `inbox.ack`
    InboxAck,
    /// `inbox.dismiss`
    InboxDismiss,
    /// `inbox.instruct`
    InboxInstruct,
    /// `media.pause`
    MediaPause,
    /// `media.resume`
    MediaResume,
    /// `media.next`
    MediaNext,
    /// `media.previous`
    MediaPrevious,
    /// `media.play`
    MediaPlay,
    /// `volume.set`
    VolumeSet,
    /// `volume.up`
    VolumeUp,
    /// `volume.down`
    VolumeDown,
    /// `timer.cancel`
    TimerCancel,
    /// `timer.query`
    TimerQuery,
    /// `timer.set`
    TimerSet,
    /// `stopwatch.start`
    StopwatchStart,
    /// `stopwatch.lap`
    StopwatchLap,
    /// `stopwatch.stop`
    StopwatchStop,
    /// `stopwatch.read`
    StopwatchRead,
    /// `stopwatch.reset`
    StopwatchReset,
    /// `alarm.cancel`
    AlarmCancel,
    /// `alarm.set_weekdays`
    AlarmSetWeekdays,
    /// `alarm.set_weekend`
    AlarmSetWeekend,
    /// `alarm.set_daily`
    AlarmSetDaily,
    /// `alarm.set`
    AlarmSet,
    /// `reminder.set_in`
    ReminderSetIn,
    /// `reminder.set_at`
    ReminderSetAt,
    /// `time.query`
    TimeQuery,
    /// `date.query`
    DateQuery,
}

impl Intent {
    /// Every intent, in grammar order. A test checks that each has a template in every language.
    pub const ALL: [Self; 32] = [
        Self::Stop,
        Self::InboxStatus,
        Self::InboxDetails,
        Self::InboxSnooze,
        Self::InboxAck,
        Self::InboxDismiss,
        Self::InboxInstruct,
        Self::MediaPause,
        Self::MediaResume,
        Self::MediaNext,
        Self::MediaPrevious,
        Self::MediaPlay,
        Self::VolumeSet,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::TimerCancel,
        Self::TimerQuery,
        Self::TimerSet,
        Self::StopwatchStart,
        Self::StopwatchLap,
        Self::StopwatchStop,
        Self::StopwatchRead,
        Self::StopwatchReset,
        Self::AlarmCancel,
        Self::AlarmSetWeekdays,
        Self::AlarmSetWeekend,
        Self::AlarmSetDaily,
        Self::AlarmSet,
        Self::ReminderSetIn,
        Self::ReminderSetAt,
        Self::TimeQuery,
        Self::DateQuery,
    ];

    /// The dotted name, for logs and tests. It does not change with the language.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::InboxStatus => "inbox.status",
            Self::InboxDetails => "inbox.details",
            Self::InboxSnooze => "inbox.snooze",
            Self::InboxAck => "inbox.ack",
            Self::InboxDismiss => "inbox.dismiss",
            Self::InboxInstruct => "inbox.instruct",
            Self::MediaPause => "media.pause",
            Self::MediaResume => "media.resume",
            Self::MediaNext => "media.next",
            Self::MediaPrevious => "media.previous",
            Self::MediaPlay => "media.play",
            Self::VolumeSet => "volume.set",
            Self::VolumeUp => "volume.up",
            Self::VolumeDown => "volume.down",
            Self::TimerCancel => "timer.cancel",
            Self::TimerQuery => "timer.query",
            Self::TimerSet => "timer.set",
            Self::StopwatchStart => "stopwatch.start",
            Self::StopwatchLap => "stopwatch.lap",
            Self::StopwatchStop => "stopwatch.stop",
            Self::StopwatchRead => "stopwatch.read",
            Self::StopwatchReset => "stopwatch.reset",
            Self::AlarmCancel => "alarm.cancel",
            Self::AlarmSetWeekdays => "alarm.set_weekdays",
            Self::AlarmSetWeekend => "alarm.set_weekend",
            Self::AlarmSetDaily => "alarm.set_daily",
            Self::AlarmSet => "alarm.set",
            Self::ReminderSetIn => "reminder.set_in",
            Self::ReminderSetAt => "reminder.set_at",
            Self::TimeQuery => "time.query",
            Self::DateQuery => "date.query",
        }
    }

    /// Whether the answer or effect depends on the wall clock being right. A countdown is not one: a timer, or
    /// a reminder after a delay, runs on the monotonic clock and works on a device that has never had the time.
    #[must_use]
    pub const fn needs_valid_clock(self) -> bool {
        match self {
            Self::TimeQuery
            | Self::DateQuery
            | Self::StopwatchStart
            | Self::StopwatchLap
            | Self::StopwatchStop
            | Self::StopwatchRead
            | Self::AlarmSet
            | Self::AlarmSetDaily
            | Self::AlarmSetWeekdays
            | Self::AlarmSetWeekend
            | Self::ReminderSetAt => true,
            Self::Stop
            | Self::InboxStatus
            | Self::InboxDetails
            | Self::InboxSnooze
            | Self::InboxAck
            | Self::InboxDismiss
            | Self::InboxInstruct
            | Self::MediaPause
            | Self::MediaResume
            | Self::MediaNext
            | Self::MediaPrevious
            | Self::MediaPlay
            | Self::VolumeSet
            | Self::VolumeUp
            | Self::VolumeDown
            | Self::TimerCancel
            | Self::TimerQuery
            | Self::TimerSet
            | Self::ReminderSetIn
            | Self::StopwatchReset
            | Self::AlarmCancel => false,
        }
    }
}
