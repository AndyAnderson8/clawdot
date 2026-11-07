//! The tools the language model can call (REQ-AST-004): the same timers, alarms, reminders, and inbox
//! operations the fast path offers, as validated, schema-described functions.
//!
//! These are local, reversible actions, so none needs spoken confirmation. Anything with an outside
//! effect (a future HTTP tool, REQ-AST-014) must declare `needs_confirmation`.

use super::{Assistant, MONTHS, WEEKDAYS, format_clock, format_duration};
use crate::inbox::reply::ReplyKind;
use crate::inbox::summary::{details_text, status_sentence};
use crate::llm::ToolSpec;
use crate::sched::{Kind, civil_from_days, days, next_occurrence, weekday};
use crate::session::ToolHost;
use serde_json::{Value, json};

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters: json!({ "type": "object", "properties": properties, "required": required }),
    }
}

fn int(args: &Value, key: &str) -> Result<i64, String> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("`{key}` must be a whole number"))
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("`{key}` must be non-empty text"))
}

fn opt_text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).map_or("", str::trim)
}

fn clock_args(args: &Value) -> Result<(u8, u8), String> {
    let h = u8::try_from(int(args, "hour")?)
        .ok()
        .filter(|h| *h <= 23)
        .ok_or("`hour` must be 0 to 23")?;
    let m = u8::try_from(int(args, "minute")?)
        .ok()
        .filter(|m| *m <= 59)
        .ok_or("`minute` must be 0 to 59")?;
    Ok((h, m))
}

/// "mon", "Tuesday", "weekdays", "weekend", "daily" to a weekday mask.
fn day_mask(names: &[Value]) -> Result<u8, String> {
    let mut mask = 0_u8;
    for n in names {
        let s = n
            .as_str()
            .ok_or("`days` must be a list of day names")?
            .to_ascii_lowercase();
        mask |= match s.as_str() {
            "daily" | "every day" | "everyday" | "all" => days::ALL,
            "weekdays" | "weekday" => days::WEEKDAYS,
            "weekend" | "weekends" => days::WEEKEND,
            other => {
                let idx = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
                    .iter()
                    .position(|d| other.starts_with(d))
                    .ok_or_else(|| format!("`{other}` is not a day name"))?;
                1 << idx
            }
        };
    }
    if mask == 0 {
        Err("`days` is empty".into())
    } else {
        Ok(mask)
    }
}

impl ToolHost for Assistant {
    fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut tools = vec![
            spec(
                "set_timer",
                "Start a countdown timer.",
                json!({ "seconds": {"type":"integer","description":"Length in seconds (1 to 604800)"},
                        "label": {"type":"string","description":"Optional name, for example pasta"} }),
                &["seconds"],
            ),
            spec(
                "list_timers",
                "List running timers and the time left on each.",
                json!({}),
                &[],
            ),
            spec(
                "cancel_timers",
                "Cancel timers. With a label, only timers whose name contains it; without, all timers.",
                json!({ "label": {"type":"string"} }),
                &[],
            ),
            spec(
                "set_alarm",
                "Set an alarm at a local time, once or repeating on some days.",
                json!({ "hour": {"type":"integer","description":"0 to 23, 24-hour clock"},
                        "minute": {"type":"integer","description":"0 to 59"},
                        "days": {"type":"array","items":{"type":"string"},
                                 "description":"Optional repeat: day names, or weekdays, weekend, daily. Omit for a single alarm."},
                        "label": {"type":"string"} }),
                &["hour", "minute"],
            ),
            spec("cancel_alarms", "Cancel every alarm.", json!({}), &[]),
            spec(
                "set_reminder",
                "Remind the user of something after a delay.",
                json!({ "text": {"type":"string"}, "seconds": {"type":"integer","description":"Delay in seconds"} }),
                &["text", "seconds"],
            ),
            spec(
                "set_reminder_at",
                "Remind the user of something at the next occurrence of a local time.",
                json!({ "text": {"type":"string"}, "hour": {"type":"integer"}, "minute": {"type":"integer"} }),
                &["text", "hour", "minute"],
            ),
            spec(
                "current_time",
                "The current local time and date.",
                json!({}),
                &[],
            ),
        ];
        if self.core.is_some() {
            tools.extend([
                spec("inbox_status", "What the user's agents are doing: what is waiting and what finished.", json!({}), &[]),
                spec("inbox_details", "The details of the agent event currently being discussed.", json!({}), &[]),
                spec(
                    "inbox_reply",
                    "Answer the agent event just announced. kind `ack` means the user will handle it themselves; `instruct` sends text for the agent to act on.",
                    json!({ "kind": {"type":"string","enum":["ack","instruct"]}, "text": {"type":"string"} }),
                    &["kind"],
                ),
                spec("inbox_snooze", "Postpone the current agent event.", json!({ "seconds": {"type":"integer"} }), &["seconds"]),
                spec("inbox_dismiss", "Dismiss the current agent event.", json!({}), &[]),
            ]);
        }
        tools
    }

    fn needs_confirmation(&self, _name: &str) -> bool {
        false
    }

    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        self.run_tool(name, args)
    }
}

impl Assistant {
    fn run_tool(&mut self, name: &str, args: &Value) -> Result<String, String> {
        let now = self.clock.now();
        match name {
            "set_timer" => {
                let secs = int(args, "seconds")?;
                let label = opt_text(args, "label").to_owned();
                self.sched
                    .set_timer(now, secs, &label)
                    .map_err(|e| e.to_string())?;
                self.save();
                Ok(format!("Timer set for {}.", format_duration(secs)))
            }
            "list_timers" => {
                let timers = self.matching_timers(None);
                if timers.is_empty() {
                    return Ok("No timers running.".into());
                }
                Ok(timers
                    .iter()
                    .map(|(_, label, left)| {
                        let who = if label.is_empty() {
                            "timer"
                        } else {
                            label.as_str()
                        };
                        format!("{who}: {} left", format_duration(*left))
                    })
                    .collect::<Vec<_>>()
                    .join("; "))
            }
            "cancel_timers" => {
                let label = opt_text(args, "label");
                let found = self.matching_timers((!label.is_empty()).then_some(label));
                for (id, _, _) in &found {
                    let _ = self.sched.cancel(*id);
                }
                self.save();
                Ok(format!("Cancelled {} timer(s).", found.len()))
            }
            "set_alarm" => {
                let (h, m) = clock_args(args)?;
                let mask = match args.get("days") {
                    None | Some(Value::Null) => None,
                    Some(Value::Array(a)) => Some(day_mask(a)?),
                    Some(_) => return Err("`days` must be a list of day names".into()),
                };
                let label = opt_text(args, "label").to_owned();
                self.sched
                    .set_alarm(now, &*self.zone, h, m, mask, &label)
                    .map_err(|e| e.to_string())?;
                self.save();
                let at = next_occurrence(&*self.zone, now, mask.unwrap_or(days::ALL), h, m)
                    .unwrap_or(now);
                Ok(format!(
                    "Alarm set for {} {}.",
                    format_clock(h, m),
                    self.day_words(now, at)
                ))
            }
            "cancel_alarms" => {
                let n = self.sched.cancel_kind(Kind::Alarm);
                self.save();
                Ok(format!("Cancelled {n} alarm(s)."))
            }
            "set_reminder" => {
                let secs = int(args, "seconds")?;
                let what = text(args, "text")?.to_owned();
                self.sched
                    .set_reminder(now, now + secs, &what)
                    .map_err(|e| e.to_string())?;
                self.save();
                Ok(format!(
                    "Will remind about \"{what}\" in {}.",
                    format_duration(secs)
                ))
            }
            "set_reminder_at" => {
                let (h, m) = clock_args(args)?;
                let what = text(args, "text")?.to_owned();
                let at = next_occurrence(&*self.zone, now, days::ALL, h, m)
                    .ok_or("that time does not exist")?;
                self.sched
                    .set_reminder(now, at, &what)
                    .map_err(|e| e.to_string())?;
                self.save();
                Ok(format!(
                    "Will remind about \"{what}\" at {} {}.",
                    format_clock(h, m),
                    self.day_words(now, at)
                ))
            }
            "current_time" => {
                let minute = self.zone.local_minute_of_day(now);
                let day = self.zone.local_day(now);
                let (_, mo, d) = civil_from_days(day);
                Ok(format!(
                    "It is {} on {}, {} {}.",
                    format_clock(
                        u8::try_from(minute / 60).unwrap_or(0),
                        u8::try_from(minute % 60).unwrap_or(0)
                    ),
                    WEEKDAYS[usize::from(weekday(day))],
                    MONTHS[usize::try_from(mo - 1).unwrap_or(0) % 12],
                    d
                ))
            }
            n if n.starts_with("inbox_") => self.call_inbox_tool(n, args, now),
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

impl Assistant {
    fn day_words(&self, now: i64, at: i64) -> &'static str {
        match self.zone.local_day(at) - self.zone.local_day(now) {
            0 => "today",
            1 => "tomorrow",
            _ => "on its next occurrence",
        }
    }

    fn call_inbox_tool(&mut self, name: &str, args: &Value, now: i64) -> Result<String, String> {
        let core = self
            .core
            .clone()
            .ok_or("the agent inbox is not available")?;
        match name {
            "inbox_status" => Ok(status_sentence(&core.status())),
            "inbox_details" => core
                .current_event()
                .map(|e| details_text(&e))
                .ok_or_else(|| "no agent event is being discussed".to_owned()),
            "inbox_reply" => {
                let kind = match opt_text(args, "kind") {
                    "ack" => ReplyKind::Ack,
                    "instruct" => ReplyKind::Instruct,
                    other => return Err(format!("`kind` must be ack or instruct, not `{other}`")),
                };
                let body = if kind == ReplyKind::Instruct {
                    text(args, "text")?.to_owned()
                } else {
                    String::new()
                };
                core.reply(kind, &body)
                    .map(|r| format!("Sent to {}.", r.source))
                    .map_err(|e| match e {
                        crate::app::ReplyError::NothingToReplyTo => {
                            "no agent event is waiting for a reply".to_owned()
                        }
                        crate::app::ReplyError::QueueFull => {
                            "too many replies are waiting for the agent bridge".to_owned()
                        }
                    })
            }
            "inbox_snooze" => {
                let secs = int(args, "seconds")?;
                if !(60..=86_400).contains(&secs) {
                    return Err("`seconds` must be between 60 and 86400".into());
                }
                let _ = now;
                if core.snooze_current(secs) {
                    Ok(format!("Snoozed for {}.", format_duration(secs)))
                } else {
                    Err("no agent event is being discussed".into())
                }
            }
            "inbox_dismiss" => {
                if core.dismiss_current() {
                    Ok("Dismissed.".into())
                } else {
                    Err("no agent event is being discussed".into())
                }
            }
            other => Err(format!("unknown tool `{other}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::setup;
    use super::*;

    fn call(a: &mut Assistant, name: &str, args: Value) -> Result<String, String> {
        a.run_tool(name, &args)
    }

    #[test]
    fn every_tool_has_a_valid_schema_and_a_unique_name() {
        let (a, _, _) = setup(true);
        let specs = a.tool_specs();
        let mut names: Vec<_> = specs.iter().map(|s| s.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), specs.len());
        for s in &specs {
            assert_eq!(s.parameters["type"], "object", "{}", s.name);
            assert!(
                !s.description.is_empty()
                    && s.name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            );
        }
        let (no_core, _, _) = setup(false);
        assert!(
            !no_core
                .tool_specs()
                .iter()
                .any(|s| s.name.starts_with("inbox_")),
            "no inbox tools without an inbox"
        );
    }

    #[test]
    fn timer_tools_work_and_validate() {
        let (mut a, _, _) = setup(false);
        assert_eq!(
            call(
                &mut a,
                "set_timer",
                json!({"seconds": 600, "label": "pasta"})
            )
            .unwrap(),
            "Timer set for 10 minutes."
        );
        assert_eq!(
            call(&mut a, "list_timers", json!({})).unwrap(),
            "pasta: 10 minutes left"
        );
        assert!(
            call(&mut a, "set_timer", json!({"seconds": "ten"}))
                .unwrap_err()
                .contains("whole number")
        );
        assert!(call(&mut a, "set_timer", json!({"seconds": 0})).is_err());
        assert_eq!(
            call(&mut a, "cancel_timers", json!({"label": "pasta"})).unwrap(),
            "Cancelled 1 timer(s)."
        );
        assert_eq!(
            call(&mut a, "list_timers", json!({})).unwrap(),
            "No timers running."
        );
    }

    #[test]
    fn alarm_and_reminder_tools_work_and_validate() {
        let (mut a, _, _) = setup(false);
        assert_eq!(
            call(&mut a, "set_alarm", json!({"hour": 6, "minute": 30})).unwrap(),
            "Alarm set for 6:30 AM tomorrow."
        );
        assert!(call(&mut a, "set_alarm", json!({"hour": 25, "minute": 0})).is_err());
        assert!(
            call(
                &mut a,
                "set_alarm",
                json!({"hour": 7, "minute": 0, "days": ["funday"]})
            )
            .is_err()
        );
        assert!(
            call(
                &mut a,
                "set_alarm",
                json!({"hour": 7, "minute": 0, "days": []})
            )
            .is_err()
        );
        assert!(
            call(
                &mut a,
                "set_alarm",
                json!({"hour": 7, "minute": 0, "days": "mon"})
            )
            .is_err()
        );
        call(
            &mut a,
            "set_alarm",
            json!({"hour": 7, "minute": 0, "days": ["weekdays"], "label": "work"}),
        )
        .unwrap();
        assert_eq!(
            call(&mut a, "cancel_alarms", json!({})).unwrap(),
            "Cancelled 2 alarm(s)."
        );
        assert_eq!(
            call(
                &mut a,
                "set_reminder",
                json!({"text": "call mum", "seconds": 600})
            )
            .unwrap(),
            "Will remind about \"call mum\" in 10 minutes."
        );
        assert!(call(&mut a, "set_reminder", json!({"text": "  ", "seconds": 60})).is_err());
        assert_eq!(
            call(
                &mut a,
                "set_reminder_at",
                json!({"text": "dinner", "hour": 17, "minute": 0})
            )
            .unwrap(),
            "Will remind about \"dinner\" at 5 PM today."
        );
        assert_eq!(
            call(&mut a, "current_time", json!({})).unwrap(),
            "It is 2:42 PM on Saturday, November 7."
        );
    }

    #[test]
    fn day_names_parse_to_masks() {
        let m = |v: Value| day_mask(v.as_array().unwrap());
        assert_eq!(m(json!(["mon", "Wednesday"])).unwrap(), 0b101);
        assert_eq!(m(json!(["weekdays"])).unwrap(), days::WEEKDAYS);
        assert_eq!(m(json!(["weekend", "mon"])).unwrap(), days::WEEKEND | 1);
        assert_eq!(m(json!(["daily"])).unwrap(), days::ALL);
        assert!(m(json!([])).is_err() && m(json!([5])).is_err());
    }

    #[test]
    fn inbox_tools_drive_the_agent_loop() {
        let (mut a, _, core) = setup(true);
        let core = core.unwrap();
        let (_, token) = core.create_endpoint("claude-code", "claude-code").unwrap();
        assert_eq!(
            call(&mut a, "inbox_status", json!({})).unwrap(),
            "Nothing from your agents right now."
        );
        assert!(call(&mut a, "inbox_details", json!({})).is_err());
        assert!(
            call(
                &mut a,
                "inbox_reply",
                json!({"kind": "instruct", "text": "go"})
            )
            .unwrap_err()
            .contains("no agent event")
        );

        core.ingest_hook("claude-code", &token, br#"{"session_id":"s1","cwd":"/a/p","hook_event_name":"Stop","last_assistant_message":"Fixed it."}"#).unwrap();
        let id = core.next_announcement(true, 0).unwrap().event.id;
        core.mark_announced(&[id]);
        assert_eq!(
            call(&mut a, "inbox_details", json!({})).unwrap(),
            "Fixed it."
        );
        assert!(call(&mut a, "inbox_reply", json!({"kind": "shout"})).is_err());
        assert!(
            call(&mut a, "inbox_reply", json!({"kind": "instruct"}))
                .unwrap_err()
                .contains("non-empty")
        );
        assert_eq!(
            call(
                &mut a,
                "inbox_reply",
                json!({"kind": "instruct", "text": "add tests"})
            )
            .unwrap(),
            "Sent to Claude Code."
        );
        assert_eq!(
            core.bridge_pending("claude-code", &token, 0).unwrap()[0].text,
            "add tests"
        );
    }

    #[test]
    fn unknown_tools_are_refused() {
        let (mut a, _, _) = setup(true);
        assert!(
            call(&mut a, "format_disk", json!({}))
                .unwrap_err()
                .contains("unknown tool")
        );
        assert!(
            call(&mut a, "inbox_exfiltrate", json!({}))
                .unwrap_err()
                .contains("unknown tool")
        );
    }
}
