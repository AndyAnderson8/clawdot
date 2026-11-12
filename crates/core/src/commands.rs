//! The one-shot commands and console helpers of the `clawdot` binary: `say`, `--reset-admin`, endpoint
//! snippets, the web UI banner and the console's quit words.

use crate::app::Core;
use crate::lang::Language;
use std::net::SocketAddr;
use std::process::ExitCode;

/// `--reset-admin`: forget the administrator setting so first-run setup shows again (REQ-CFG-022).
pub fn reset_admin(dir: &std::path::Path) -> ExitCode {
    match crate::admin::reset(dir) {
        Ok(removed) => {
            println!(
                "{}Start clawdot and open the web UI to set an administrator password again.\n\
                 Until you do, the setup page can be completed by anyone on a private network who reaches it.",
                if removed {
                    "The web UI administrator setting was cleared. Endpoints and other state are unchanged.\n"
                } else {
                    "There was no administrator setting to clear.\n"
                }
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("clawdot: cannot clear the administrator setting: {e}");
            ExitCode::from(1)
        }
    }
}

/// Tell the person at the console where the web UI is and how it is protected. No secret is printed: the
/// device may have no visible console (REQ-CFG-018, REQ-CFG-021).
pub fn announce_admin(core: &Core, public: &str) {
    use crate::admin::Mode;
    match core.admin_mode() {
        Mode::Setup => {
            println!(
                "\nWeb UI: {public}/  (open it to finish first-run setup)\n\
                 Until setup is done, the first person on a private network to open it can set the password."
            );
        }
        Mode::Password => println!("\nWeb UI: {public}/"),
        Mode::Open => println!(
            "\nWeb UI: {public}/  is UNPROTECTED (open mode): anyone on your network can use it.\n\
             Set a password in the page, or run `clawdot --reset-admin` and start again."
        ),
    }
}

/// True if a console line asks to leave (REQ-CFG-017).
pub fn is_quit(lang: &Language, line: &str) -> bool {
    let word = line.trim().trim_end_matches(['.', '!']);
    lang.quit_words.iter().any(|q| word.eq_ignore_ascii_case(q))
}

pub const DEFAULT_PORT: u16 = 3335;

/// An actionable message for a failed bind (REQ-CFG-014).
pub fn bind_error(addr: SocketAddr, e: &std::io::Error) -> String {
    let hint = if e.kind() == std::io::ErrorKind::AddrInUse {
        format!(
            "another program is using port {}; choose a free one, for example --listen {}",
            addr.port(),
            SocketAddr::new(addr.ip(), DEFAULT_PORT)
        )
    } else {
        "choose another address with --listen (or `listen` in the configuration file)".to_owned()
    };
    format!("cannot listen on {addr}: {e}. {hint}")
}

pub fn print_snippet(public: &str, endpoint: &str, adapter: &str, token: &str) {
    println!(
        "\nEndpoint `{endpoint}` created (adapter: {adapter}). The token is shown only now:\n  {token}\n"
    );
    if adapter == "claude-code" {
        println!(
            "Claude Code hooks: add to ~/.claude/settings.json (merge with what is already there):"
        );
        let hook = |event: &str| {
            format!(
                "    \"{event}\": [{{ \"hooks\": [{{ \"type\": \"http\", \"url\": \"{public}/hook/{endpoint}\", \"headers\": {{ \"Authorization\": \"Bearer {token}\" }}, \"timeout\": 5 }}] }}]"
            )
        };
        println!(
            "{{\n  \"hooks\": {{\n{},\n{}\n  }}\n}}\n",
            hook("Stop"),
            hook("Notification")
        );
        println!(
            "Voice replies reach the agent through the bridge, on the machine where the agent runs:"
        );
        println!("  CLAWDOT_TOKEN=<token> clawdot-bridge --url {public} --endpoint {endpoint}\n");
    } else {
        println!(
            "Post events with:\n  curl -X POST {public}/hook/{endpoint} -H 'Authorization: Bearer <token>' -d '{{\"title\":\"Backup finished\",\"summary\":\"12 GB copied\"}}'\n"
        );
    }
}

/// `clawdot say TEXT ...`: speak a message on a running ClawDot (REQ-INB-026).
pub async fn say_command(rest: &[String], env: &crate::env::Env) -> ExitCode {
    let say_args = match crate::say::parse(rest, |k| env.get(k)) {
        Ok(a) => a,
        Err(m) if m.is_empty() => {
            print!("{}", crate::say::HELP);
            return ExitCode::SUCCESS;
        }
        Err(m) => {
            eprintln!("clawdot say: {m}");
            return ExitCode::from(2);
        }
    };
    let token = env.get(&say_args.token_env).filter(|t| !t.is_empty());
    if token.is_none() && say_args.body.get("endpoint").is_none() {
        eprintln!(
            "clawdot say: set the environment variable {} to an endpoint token (or choose another with --token-env)",
            say_args.token_env
        );
        return ExitCode::from(2);
    }
    match crate::say::run(&say_args, token.as_deref()).await {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(m) => {
            eprintln!("clawdot say: {m}");
            ExitCode::FAILURE
        }
    }
}

/// Create (`create`, each `NAME` or `NAME:ADAPTER`) and rotate (`rotate`) endpoints named on the command
/// line, printing each token once. Returns true if any failed.
pub fn run_endpoint_commands(
    core: &Core,
    create: &[String],
    rotate: &[String],
    public: &str,
) -> bool {
    let mut failed = false;
    for spec in create {
        let (name, adapter) = spec.split_once(':').unwrap_or((spec, spec));
        let adapter = if matches!(adapter, "claude-code" | "generic") {
            adapter
        } else {
            "generic"
        };
        match core.create_endpoint(name, adapter) {
            Ok((_, token)) => print_snippet(public, name, adapter, &token),
            Err(e) => {
                eprintln!("clawdot: endpoint `{name}`: {e}");
                failed = true;
            }
        }
    }
    for name in rotate {
        match core.rotate_endpoint(name) {
            Ok(token) => println!("New token for `{name}` (shown once): {token}"),
            Err(e) => {
                eprintln!("clawdot: endpoint `{name}`: {e}");
                failed = true;
            }
        }
    }
    failed
}

/// Read console lines on a thread of their own and send each, trimmed, to `tx` until input ends.
pub fn spawn_console(tx: tokio::sync::mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let mut buf = String::new();
        while std::io::stdin().read_line(&mut buf).is_ok_and(|n| n > 0) {
            if tx.blocking_send(buf.trim().to_owned()).is_err() {
                break;
            }
            buf.clear();
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::settings::public_url_from_listen;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn quit_and_exit_leave_the_console() {
        for l in ["quit", "exit", "Quit", " EXIT ", "quit."] {
            assert!(is_quit(crate::lang::DEFAULT, l), "{l}");
        }
        for l in ["quit smoking", "what time is it", "exit the building", ""] {
            assert!(!is_quit(crate::lang::DEFAULT, l), "{l}");
        }
    }

    #[test]
    fn snippet_urls_follow_the_listen_address() {
        assert_eq!(
            public_url_from_listen(addr("192.168.1.20:9000")),
            ("http://192.168.1.20:9000".to_owned(), false)
        );
        assert_eq!(
            public_url_from_listen(addr("[fe80::1]:9000")),
            ("http://[fe80::1]:9000".to_owned(), false)
        );
        for unspecified in ["0.0.0.0:9000", "[::]:9000"] {
            assert_eq!(
                public_url_from_listen(addr(unspecified)),
                ("http://clawdot.local:9000".to_owned(), true)
            );
        }
    }

    #[test]
    fn a_taken_port_gets_an_actionable_message() {
        let e = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        let m = bind_error(addr("0.0.0.0:8080"), &e);
        assert!(m.contains("0.0.0.0:8080"), "{m}");
        assert!(m.contains("--listen 0.0.0.0:3335"), "{m}");
    }
}
