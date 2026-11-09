//! Matching actual tool arguments against what a scenario expects.
//!
//! An expected object matches an actual object when every expected key is present and matches (extra keys
//! in the actual one are fine). Anything else must be equal, except for these one-key operator objects:
//!
//! * `{"$any": true}`: any value, but present
//! * `{"$absent": true}`: the key must not be present
//! * `{"$contains": "x"}`, `{"$icontains": "x"}`, `{"$prefix": "x"}`: text tests (`$icontains` ignores case)
//! * `{"$one_of": [a, b]}`: equal to one of the values
//! * `{"$range": [lo, hi]}`: a number from lo to hi, inclusive

use serde_json::Value;

/// Whether `actual` satisfies `expected`. On failure the error says where and why.
///
/// # Errors
/// A short description of the first mismatch, with the path to it.
pub fn matches(expected: &Value, actual: &Value) -> Result<(), String> {
    check(expected, Some(actual), "args")
}

fn operator(v: &Value) -> Option<(&str, &Value)> {
    let m = v.as_object()?;
    if m.len() != 1 {
        return None;
    }
    let (k, v) = m.iter().next()?;
    k.starts_with('$').then_some((k.as_str(), v))
}

fn show(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 80 {
        format!("{}...", s.chars().take(80).collect::<String>())
    } else {
        s
    }
}

fn check(expected: &Value, actual: Option<&Value>, path: &str) -> Result<(), String> {
    if let Some((op, arg)) = operator(expected) {
        return apply(op, arg, actual, path);
    }
    let Some(actual) = actual else {
        return Err(format!("{path}: missing, expected {}", show(expected)));
    };
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (k, ev) in e {
                check(ev, a.get(k), &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        (Value::Array(e), Value::Array(a)) => {
            if e.len() != a.len() {
                return Err(format!(
                    "{path}: expected {} items, got {}",
                    e.len(),
                    a.len()
                ));
            }
            for (i, (ev, av)) in e.iter().zip(a).enumerate() {
                check(ev, Some(av), &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        (e, a) if e == a => Ok(()),
        (e, a) => Err(format!("{path}: expected {}, got {}", show(e), show(a))),
    }
}

fn apply(op: &str, arg: &Value, actual: Option<&Value>, path: &str) -> Result<(), String> {
    if op == "$absent" {
        return match actual {
            None => Ok(()),
            Some(a) => Err(format!("{path}: expected to be absent, got {}", show(a))),
        };
    }
    let Some(actual) = actual else {
        return Err(format!("{path}: missing"));
    };
    let text = actual.as_str();
    let want = arg.as_str();
    let ok = match op {
        "$any" => true,
        "$contains" => matches!((text, want), (Some(t), Some(w)) if t.contains(w)),
        "$icontains" => {
            matches!((text, want), (Some(t), Some(w)) if t.to_lowercase().contains(&w.to_lowercase()))
        }
        "$prefix" => matches!((text, want), (Some(t), Some(w)) if t.starts_with(w)),
        "$one_of" => arg.as_array().is_some_and(|v| v.contains(actual)),
        "$range" => match (arg.as_array().map(Vec::as_slice), actual.as_f64()) {
            (Some([lo, hi]), Some(x)) => {
                matches!((lo.as_f64(), hi.as_f64()), (Some(l), Some(h)) if x >= l && x <= h)
            }
            _ => false,
        },
        other => return Err(format!("{path}: unknown matcher `{other}`")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{path}: {} does not satisfy {{\"{op}\": {}}}",
            show(actual),
            show(arg)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn subsets_and_operators() {
        let actual =
            json!({"seconds": 600, "label": "Pasta water", "days": ["mon", "tue"], "x": {"y": 1}});
        let ok = |e: Value| matches(&e, &actual).unwrap();
        let bad = |e: Value| matches(&e, &actual).unwrap_err();
        ok(json!({}));
        ok(json!({"seconds": 600}));
        ok(json!({"label": {"$icontains": "pasta"}, "x": {"y": {"$any": true}}}));
        ok(json!({"seconds": {"$range": [590, 610]}, "days": ["mon", {"$prefix": "tu"}]}));
        ok(json!({"seconds": {"$one_of": [60, 600]}, "nope": {"$absent": true}}));
        assert!(bad(json!({"seconds": 601})).contains("args.seconds"));
        assert!(bad(json!({"label": {"$contains": "pasta"}})).contains("label"));
        assert!(bad(json!({"days": ["mon"]})).contains("expected 1 items, got 2"));
        assert!(bad(json!({"missing": 1})).contains("missing"));
        assert!(bad(json!({"seconds": {"$absent": true}})).contains("absent"));
        assert!(bad(json!({"seconds": {"$range": [1, 2]}})).contains("$range"));
        assert!(bad(json!({"seconds": {"$wat": 1}})).contains("unknown matcher"));
        // a non-object actual never panics
        assert!(matches(&json!({"a": 1}), &json!("text")).is_err());
        assert!(matches(&json!({"$any": true}), &Value::Null).is_ok());
    }
}
