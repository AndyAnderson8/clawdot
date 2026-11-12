//! Switching the running assistant to new values of the settings that apply without a restart (REQ-CFG-039).
//!
//! What applies a setting is in [`APPLIERS`]: one row per piece of work, naming what it reads (a whole
//! [`Group`], or the settings it is keyed by) beside the function that does it. [`apply_live`] runs the rows in
//! order, each only when something it reads has changed. A live setting that no row covers would be accepted by
//! the portal and then do nothing, so a test requires every live row of [`settings::FIELDS`] to be covered by
//! exactly one applier, or to be listed in [`APPLIED_ELSEWHERE`] with the reason.

use super::{Brain, build_llm, with_agent, zone_for};
use crate::app::Core;
use crate::rtvoice::RtSettings;
use crate::seed::{agent_from, policy_from, quiet_from};
use crate::settings::{self, Group};

type Map = serde_json::Map<String, serde_json::Value>;

/// What an applier reads: the values that, when one of them changes, make it run.
#[derive(Debug, Clone, Copy)]
enum Reads {
    /// Every setting of a group (see [`Group`]).
    Group(Group),
    /// These settings by name.
    Keys(&'static [&'static str]),
}

impl Reads {
    fn changed(self, live: &Map, applied: &Map) -> bool {
        match self {
            Self::Group(g) => settings::group_changed(g, live, applied),
            Self::Keys(keys) => keys.iter().any(|k| live.get(*k) != applied.get(*k)),
        }
    }

    #[cfg(test)]
    fn covers(self, key: &str) -> bool {
        match self {
            Self::Group(g) => g != Group::Own && settings::field(key).is_some_and(|f| f.group == g),
            Self::Keys(keys) => keys.contains(&key),
        }
    }
}

/// One piece of work that follows a setting.
struct Applier {
    reads: Reads,
    apply: fn(&mut Brain, &Core, &Map),
}

/// The appliers, in the order they run. An applier may rely on an earlier one: the language model client is
/// built last because it takes the agent limits the `Agent` row has just set.
const APPLIERS: &[Applier] = &[
    Applier {
        reads: Reads::Keys(&["zone"]),
        apply: zone,
    },
    Applier {
        reads: Reads::Keys(&["persona"]),
        apply: persona,
    },
    Applier {
        reads: Reads::Keys(&["daily_spend_usd"]),
        apply: spend_limit,
    },
    Applier {
        reads: Reads::Keys(&["llm_usd_per_mtok_in", "llm_usd_per_mtok_out"]),
        apply: token_prices,
    },
    Applier {
        reads: Reads::Group(Group::Agent),
        apply: agent,
    },
    Applier {
        reads: Reads::Group(Group::Speech),
        apply: speech,
    },
    Applier {
        reads: Reads::Group(Group::Realtime),
        apply: realtime,
    },
    Applier {
        reads: Reads::Keys(&["update_check"]),
        apply: update_check,
    },
    Applier {
        reads: Reads::Keys(&["notes"]),
        apply: notes,
    },
    Applier {
        reads: Reads::Group(Group::Policy),
        apply: policy,
    },
    Applier {
        reads: Reads::Group(Group::Quiet),
        apply: quiet_hours,
    },
    Applier {
        reads: Reads::Group(Group::Llm),
        apply: language_model,
    },
];

/// Live settings that no applier needs, and why. Each is read from the settings state when it is used.
#[cfg(test)]
const APPLIED_ELSEWHERE: &[(&str, &str)] = &[(
    "device_name",
    "shown from the settings state each time the page is built; nothing is derived from it",
)];

/// Switch to the new values of the settings that apply without a restart (REQ-CFG-039). `applied` holds
/// what is already in effect, so only real changes do any work.
pub fn apply_live(brain: &mut Brain, core: &Core, live: &Map, applied: &mut Map) {
    for a in APPLIERS {
        if a.reads.changed(live, applied) {
            (a.apply)(brain, core, live);
        }
    }
    *applied = live.clone();
}

fn text<'a>(live: &'a Map, key: &str) -> Option<&'a str> {
    live.get(key).and_then(serde_json::Value::as_str)
}

fn zone(brain: &mut Brain, _core: &Core, live: &Map) {
    let z = text(live, "zone").filter(|z| *z != "UTC");
    match zone_for(z) {
        Ok(zone) => brain.assistant.set_zone(zone),
        Err(e) => crate::diag::error(&format!("cannot switch time zone: {e}")),
    }
}

fn persona(brain: &mut Brain, _core: &Core, live: &Map) {
    if let Some(p) = text(live, "persona") {
        p.clone_into(&mut brain.persona);
    }
}

fn spend_limit(brain: &mut Brain, _core: &Core, live: &Map) {
    let mut l = brain.usage.limits();
    l.daily_spend_cap_usd = live
        .get("daily_spend_usd")
        .and_then(serde_json::Value::as_f64);
    brain.usage.set_limits(l);
}

fn token_prices(brain: &mut Brain, _core: &Core, live: &Map) {
    let mut l = brain.usage.limits();
    (l.llm_usd_per_mtok_in, l.llm_usd_per_mtok_out) = super::model_prices(live);
    brain.usage.set_limits(l);
}

fn agent(brain: &mut Brain, _core: &Core, live: &Map) {
    brain.agent = agent_from(live, brain.agent.summarize_over_chars);
    if let Some(llm) = &mut brain.llm {
        llm.cfg = with_agent(llm.cfg.clone(), &brain.agent);
    }
}

fn speech(_brain: &mut Brain, core: &Core, live: &Map) {
    core.speech_handles().apply(live);
}

fn realtime(brain: &mut Brain, _core: &Core, live: &Map) {
    brain.rt.set_settings(RtSettings::from_live(live));
}

#[cfg(feature = "tls")]
fn update_check(_brain: &mut Brain, core: &Core, live: &Map) {
    if let Some(u) = core.updater() {
        u.set_enabled(
            live.get("update_check")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true),
        );
    }
}

/// A build without `tls` has no update check, so there is nothing to switch.
#[cfg(not(feature = "tls"))]
fn update_check(_brain: &mut Brain, _core: &Core, _live: &Map) {}

fn notes(_brain: &mut Brain, core: &Core, live: &Map) {
    if let Some(n) = core.notes() {
        n.set_enabled(
            live.get("notes")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
    }
}

fn policy(_brain: &mut Brain, core: &Core, live: &Map) {
    if let Some(a) = core.audit() {
        a.set_policy(policy_from(live));
    }
}

fn quiet_hours(_brain: &mut Brain, core: &Core, live: &Map) {
    core.set_quiet_hours(quiet_from(live));
}

fn language_model(brain: &mut Brain, core: &Core, live: &Map) {
    let max_tokens = brain.usage.limits().max_output_tokens_per_turn;
    match build_llm(
        text(live, "llm_url"),
        text(live, "llm_model"),
        core.secrets(),
        core.env(),
        text(live, "llm_key_env").unwrap_or("OPENROUTER_API_KEY"),
        max_tokens,
        &brain.agent,
    ) {
        Ok(l) => brain.llm = l,
        Err(e) => crate::diag::error(&format!("cannot switch language model: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // REQ-CFG-039: a setting marked live must be applied by something, once, or be named as needing nothing.
    #[test]
    fn every_live_setting_has_exactly_one_applier() {
        for f in settings::FIELDS.iter().filter(|f| f.live) {
            let appliers = APPLIERS.iter().filter(|a| a.reads.covers(f.key)).count();
            let elsewhere = APPLIED_ELSEWHERE.iter().any(|(k, _)| *k == f.key);
            assert_eq!(
                appliers + usize::from(elsewhere),
                1,
                "the live setting `{}` has {appliers} applier(s) and {} in APPLIED_ELSEWHERE; it needs exactly one",
                f.key,
                if elsewhere {
                    "is listed"
                } else {
                    "is not listed"
                },
            );
        }
    }

    #[test]
    fn the_tables_name_only_live_settings_that_exist() {
        for a in APPLIERS {
            if let Reads::Keys(keys) = a.reads {
                for k in keys {
                    assert_eq!(
                        settings::is_live(k),
                        Some(true),
                        "`{k}` is not a live setting"
                    );
                }
            }
        }
        for (k, why) in APPLIED_ELSEWHERE {
            assert_eq!(
                settings::is_live(k),
                Some(true),
                "`{k}` is not a live setting"
            );
            assert!(!why.is_empty(), "`{k}` needs a reason");
        }
    }

    #[test]
    fn a_group_applier_reads_a_group_that_has_members() {
        for a in APPLIERS {
            if let Reads::Group(g) = a.reads {
                assert!(
                    settings::FIELDS.iter().any(|f| f.live && f.group == g),
                    "{g:?} has no live setting"
                );
            }
        }
    }
}
