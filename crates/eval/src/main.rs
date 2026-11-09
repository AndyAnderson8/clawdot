//! `clawdot-eval`: run the agent-loop scenarios.
//!
//!     clawdot-eval [replay] [--dir DIR] [--filter TEXT] [--tag TAG] [--list]
//!     clawdot-eval live --yes-spend --url URL --model NAME [--key-env VAR] [--spend-cap-usd X] [--runs N]
//!                       [--max-requests N] [--filter TEXT] [--tag TAG] [--save-transcripts DIR] [--dir DIR]
//!
//! Replay is deterministic and free: it is the gate in `scripts/check.sh` and CI. Live talks to a real model
//! and costs money, so it needs `--yes-spend` and a cap.

use clawdot_core::transport::HttpBackend;
use clawdot_eval::live::{LiveOptions, render, run_live};
use clawdot_eval::scenario::{Loaded, VERSION, load_dir};
use clawdot_eval::{default_scenarios_dir, describe_failures, replay_one, set_fingerprint};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "clawdot-eval: run the agent-loop scenarios

USAGE:
    clawdot-eval [replay] [--dir DIR] [--filter TEXT] [--tag TAG] [--list] [-v]
    clawdot-eval live --yes-spend --url URL --model NAME [options]

REPLAY (default; scripted model, no network, no spend):
    --dir DIR            Scenario directory (default: evals/scenarios in the repository)
    --filter TEXT        Only scenarios whose id contains TEXT
    --tag TAG            Only scenarios with this tag
    --list               List the scenarios and exit
    -v                   Also show what the assistant said

LIVE (real model; costs money; never run in CI):
    --yes-spend          Required. You accept that this calls a paid API.
    --url URL            OpenAI-compatible base URL (https://..., or http:// for a local server)
    --model NAME         Model to evaluate
    --key-env VAR        Environment variable holding the API key (default: OPENROUTER_API_KEY)
    --spend-cap-usd X    Stop when provider-reported spend reaches X (default 1.00, at most 50)
    --runs N             Runs per scenario (default 3, at most 10)
    --max-requests N     Stop after N model requests whatever they cost (default 400)
    --save-transcripts DIR  Write each scenario's first run as DIR/<id>.transcript.json
";

struct Args {
    live: bool,
    dir: PathBuf,
    filter: Option<String>,
    tag: Option<String>,
    list: bool,
    verbose: bool,
    yes_spend: bool,
    url: Option<String>,
    model: Option<String>,
    key_env: String,
    options: LiveOptions,
    save: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        live: false,
        dir: default_scenarios_dir(),
        filter: None,
        tag: None,
        list: false,
        verbose: false,
        yes_spend: false,
        url: None,
        model: None,
        key_env: "OPENROUTER_API_KEY".into(),
        options: LiveOptions::default(),
        save: None,
    };
    let mut it = std::env::args().skip(1).peekable();
    match it.peek().map(String::as_str) {
        Some("live") => {
            a.live = true;
            it.next();
        }
        Some("replay") => {
            it.next();
        }
        _ => {}
    }
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--dir" => a.dir = PathBuf::from(value("--dir")?),
            "--filter" => a.filter = Some(value("--filter")?),
            "--tag" => a.tag = Some(value("--tag")?),
            "--list" => a.list = true,
            "-v" | "--verbose" => a.verbose = true,
            "--yes-spend" => a.yes_spend = true,
            "--url" => a.url = Some(value("--url")?),
            "--model" => a.model = Some(value("--model")?),
            "--key-env" => a.key_env = value("--key-env")?,
            "--spend-cap-usd" => {
                a.options.spend_cap_usd = value("--spend-cap-usd")?
                    .parse()
                    .map_err(|e| format!("bad number: {e}"))?;
            }
            "--runs" => {
                a.options.runs = value("--runs")?
                    .parse()
                    .map_err(|e| format!("bad number: {e}"))?;
            }
            "--max-requests" => {
                a.options.max_requests = value("--max-requests")?
                    .parse()
                    .map_err(|e| format!("bad number: {e}"))?;
            }
            "--save-transcripts" => a.save = Some(PathBuf::from(value("--save-transcripts")?)),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(a)
}

fn select(all: Vec<Loaded>, a: &Args) -> Vec<Loaded> {
    all.into_iter()
        .filter(|l| {
            a.filter
                .as_ref()
                .is_none_or(|f| l.scenario.id.contains(f.as_str()))
        })
        .filter(|l| {
            a.tag
                .as_ref()
                .is_none_or(|t| l.scenario.tags.iter().any(|x| x == t))
        })
        .collect()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let a = match parse() {
        Ok(a) => a,
        Err(m) => {
            if m.is_empty() {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            eprintln!("clawdot-eval: {m}\n\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let all = match load_dir(&a.dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("clawdot-eval: {e}");
            return ExitCode::from(2);
        }
    };
    let fingerprint = set_fingerprint(&all);
    let chosen = select(all, &a);
    if chosen.is_empty() {
        eprintln!("clawdot-eval: no scenario matches");
        return ExitCode::from(2);
    }
    if a.list {
        for l in &chosen {
            println!(
                "{:<44} {:<5} {}",
                l.scenario.id, l.scenario.use_case, l.scenario.description
            );
        }
        return ExitCode::SUCCESS;
    }
    if a.live {
        return live(&a, &chosen, &fingerprint).await;
    }
    replay(&a, &chosen).await
}

async fn replay(a: &Args, chosen: &[Loaded]) -> ExitCode {
    let mut failed = 0;
    let mut inj = (0, 0);
    for l in chosen {
        let r = replay_one(l).await;
        let ok = r.passed();
        if l.scenario.use_case == "INJ" {
            inj.1 += 1;
            inj.0 += usize::from(ok);
        }
        println!(
            "  {}  {:<44} {}",
            if ok { "ok  " } else { "FAIL" },
            l.scenario.id,
            l.scenario.use_case
        );
        if !ok {
            failed += 1;
            print!("{}", describe_failures(&r));
        }
        if a.verbose {
            for (i, s) in r.speech.iter().enumerate() {
                println!("      turn {} said: {s}", i + 1);
            }
        }
    }
    println!(
        "\n{} scenarios, {} passed, {failed} failed. Injection set: {}/{}.",
        chosen.len(),
        chosen.len() - failed,
        inj.0,
        inj.1
    );
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

async fn live(a: &Args, chosen: &[Loaded], fingerprint: &str) -> ExitCode {
    if !a.yes_spend {
        eprintln!(
            "clawdot-eval: live mode calls a paid API. Add --yes-spend to say you accept that (cap: {:.2} USD, change with --spend-cap-usd).",
            a.options.spend_cap_usd
        );
        return ExitCode::from(2);
    }
    let (Some(url), Some(model)) = (a.url.as_deref(), a.model.as_deref()) else {
        eprintln!("clawdot-eval: live mode needs --url and --model");
        return ExitCode::from(2);
    };
    let o = a.options;
    if !(o.spend_cap_usd > 0.0 && o.spend_cap_usd <= 50.0)
        || !(1..=10).contains(&o.runs)
        || o.max_requests == 0
    {
        eprintln!(
            "clawdot-eval: --spend-cap-usd must be above 0 and at most 50, --runs 1 to 10, --max-requests above 0"
        );
        return ExitCode::from(2);
    }
    let key = std::env::var(&a.key_env).unwrap_or_default();
    if key.is_empty() && url.starts_with("https://") {
        eprintln!(
            "clawdot-eval: set {} to the API key first (the key is never printed or saved)",
            a.key_env
        );
        return ExitCode::from(2);
    }
    let make = || match HttpBackend::new(url, &key, Duration::from_secs(60)) {
        Ok(b) => b,
        Err(e) => {
            // Checked once below before any run starts; a failure here would be a bug in that check.
            eprintln!("clawdot-eval: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = HttpBackend::new(url, &key, Duration::from_secs(60)) {
        eprintln!("clawdot-eval: {e}");
        return ExitCode::from(2);
    }
    let mut scenarios: Vec<Loaded> = chosen.to_vec();
    // Run the safety-critical set first so a capped run still covers it.
    scenarios.sort_by_key(|l| l.scenario.use_case != "INJ");
    println!(
        "live run of {} scenarios x {} runs against {model}, cap {:.2} USD",
        scenarios.len(),
        o.runs,
        o.spend_cap_usd
    );
    let (summary, recordings) = run_live(&scenarios, o, &make, |l, i, r| {
        println!(
            "  {}  {:<44} run {}  {:.4} USD",
            if r.passed() { "ok  " } else { "FAIL" },
            l.scenario.id,
            i + 1,
            r.spend_usd
        );
        if !r.passed() {
            print!("{}", describe_failures(r));
        }
    })
    .await;
    if let Some(dir) = &a.save {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("clawdot-eval: cannot create {}: {e}", dir.display());
        }
        for (id, turns) in recordings {
            let t = serde_json::json!({ "version": VERSION, "turns": turns });
            let path = dir.join(format!("{id}.transcript.json"));
            if let Err(e) =
                std::fs::write(&path, serde_json::to_string_pretty(&t).unwrap_or_default())
            {
                eprintln!("clawdot-eval: cannot write {}: {e}", path.display());
            }
        }
    }
    print!("{}", render(&summary, model, fingerprint));
    let runs = o.runs;
    let ok = !summary.partial
        && summary.results.iter().all(|r| r.scenario_passed(runs))
        && summary
            .results
            .iter()
            .filter(|r| r.use_case == "INJ")
            .all(|r| r.passes() == runs);
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
