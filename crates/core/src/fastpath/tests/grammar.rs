//! Template compiling, its limits, and the step budget.

use super::*;

#[test]
fn the_built_in_grammar_compiles() {
    assert!(!Matcher::builtin(L).intents.is_empty());
    assert!(Matcher::new(L, L.grammar).is_ok());
}

#[test]
fn malformed_templates_are_rejected_not_ignored() {
    for bad in [
        "(unclosed",
        "unclosed)",
        "[oops",
        "{slot",
        "{}",
        "()",
        "[a|b]",
    ] {
        assert!(Matcher::new(L, &[("x", &[bad])]).is_err(), "{bad}");
    }
}

#[test]
fn deeply_nested_templates_are_refused_instead_of_overflowing_the_stack() {
    // Templates are compiled in today, but packs will bring them from outside (REQ-EXT-002).
    let nested = format!("{}a{}", "(".repeat(100_000), ")".repeat(100_000));
    assert!(Matcher::new(L, &[("x", &[nested.as_str()])]).is_err());
    let optional = format!("{}a{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(Matcher::new(L, &[("x", &[optional.as_str()])]).is_err());
    // Ordinary nesting still works.
    assert!(Matcher::new(L, &[("x", &["(a (b [c]) | d) e"])]).is_ok());
}

#[test]
fn explosive_templates_are_refused() {
    let t = "[a] [b] [c] [d] [e] [f] [g] [h] [i] [j] x";
    assert!(Matcher::new(L, &[("x", &[t])]).is_err());
}

#[test]
fn pathological_input_cannot_stall_the_matcher() {
    let matcher = Matcher::new(
        L,
        &[(
            "greedy",
            &["{a:text} {b:text} {c:text} {d:text} {e:text} {f:text} end"],
        )],
    )
    .unwrap();
    let long = "word ".repeat(200);
    let start = std::time::Instant::now();
    assert!(matcher.matches(&long).is_none());
    assert!(
        start.elapsed() < std::time::Duration::from_secs(1),
        "took {:?}",
        start.elapsed()
    );
}

#[test]
fn matching_is_fast_enough_for_the_budget() {
    // REQ-NFR-111: at most 20 ms per utterance on the reference CPU. Check a generous bound in
    // debug builds on a fast host so a regression to something quadratic is caught.
    let matcher = Matcher::builtin(L);
    let start = std::time::Instant::now();
    for _ in 0..200 {
        let _ = matcher.matches("set a timer for ten minutes");
        let _ = matcher.matches("what's the capital of australia please");
    }
    let per = start.elapsed() / 400;
    assert!(
        per < std::time::Duration::from_millis(5),
        "per match {per:?}"
    );
}
