//! Command-line arguments and help text for the `clawdot` binary.
//!
//! [`parse`] turns an argument list into a [`Cli`] (what the line said) and reads the `--config` file into a
//! [`Config`] (what the file said). Neither is what the program runs with: [`crate::seed::resolve`] combines them
//! with saved settings and the defaults into a `Resolved` (REQ-CFG-038).

use crate::config::Config;
use crate::settings::Layer;
use std::net::SocketAddr;
use std::path::PathBuf;

pub const HELP: &str = "\
clawdot: the ClawDot core (assistant, web portal, HTTP API and a text console)

USAGE:
    clawdot [options]
    clawdot say TEXT [options]   Speak a message on a running ClawDot (see: clawdot say --help)

OPTIONS:
    --config FILE              JSON configuration file (flags override it); see docs/architecture/configuration.md
    --data-dir DIR             Where state is kept (default: ./clawdot-data)
    --listen ADDR              Address for the HTTP server (default: 127.0.0.1:3335)
    --public-url URL           How agents reach this device, used in printed snippets (default: from --listen)
    --zone NAME                IANA time zone, for example America/New_York (default: UTC)
    --name NAME                Device name shown in the web UI header (default: ClawDot)
    --create-endpoint SPEC     NAME or NAME:ADAPTER (adapters: claude-code, generic); prints the token once and exits
    --rotate-endpoint NAME     Issue a new token for an endpoint, print it once, and exit
    --reset-admin              Clear the web UI administrator setting and exit, so first-run setup shows again
                               (run on the device with the server stopped)
    --voice-in FILE            Answer one recorded utterance (WAV, mono 16-bit 16 kHz) and exit; needs a `speech` config
    --voice-out FILE           With --voice-in, write the spoken reply to this WAV file
    --no-console               Do not read the console (run as a server only)
    --echo-speech              With --no-console, still print what the speaker says to stdout (diagnosis;
                               stdout may be kept by a service manager, so leave it off in production)
    --hive-discovery MODE      Hive (builds with the `hive` feature only): off, native, avahi or avahi:PATH. Makes the
                               speaker's identity available and, for native or avahi, announces it on the home network
                               and lists other speakers (docs/guides/hive.md). Default: not started.
    --hive-publish-name        Include the device name in the announcement (default: only an id label is public)

LANGUAGE MODEL (optional; without it only the local fast path and fallback summaries work):
    --llm-url URL              OpenAI-compatible base URL, for example https://openrouter.ai/api/v1
    --llm-model NAME           Model for conversation and tool calls
    --llm-key-env VAR          Environment variable holding the API key (default: OPENROUTER_API_KEY)
    --persona-file FILE        Replace the default system prompt
    --daily-spend-usd X        Stop calling the model after this much spend per day
    --dry-run                  Describe actions instead of running them (read-only tools still run)

    -h, --help                 Show this help
";

/// What the command line said, and nothing else: no default is filled in and nothing from the configuration file
/// is mixed in. A value the line did not give is `None` or empty. The configuration file is a [`Config`], and what
/// the program runs with is built from the two in [`crate::seed::resolve`].
#[derive(Debug, Default)]
pub struct Cli {
    /// `--data-dir`.
    pub data_dir: Option<PathBuf>,
    /// The settings the flags give (`--listen`, `--public-url`, `--zone`, `--name`, the language model and spend
    /// flags, `--dry-run`), kept apart so saved settings can sit between the file and them (REQ-CFG-038).
    pub flags: Layer,
    /// `--persona-file`.
    pub persona_file: Option<PathBuf>,
    /// `--create-endpoint`, in the order given.
    pub create: Vec<String>,
    /// `--rotate-endpoint`, in the order given.
    pub rotate: Vec<String>,
    pub reset_admin: bool,
    /// `--no-console` was given.
    pub no_console: bool,
    /// `--echo-speech`: echo spoken text to stdout even without the console (diagnosis only).
    pub echo_speech: bool,
    pub voice_in: Option<PathBuf>,
    pub voice_out: Option<PathBuf>,
    /// Hive discovery mode (`--hive-discovery`); `None` leaves the hive unstarted.
    #[cfg(feature = "hive")]
    pub hive: Option<crate::hive::Mode>,
    #[cfg(feature = "hive")]
    pub hive_publish_name: bool,
}

impl Cli {
    /// Where state is kept: the flag, else the configuration file, else `clawdot-data`.
    #[must_use]
    pub fn data_dir(&self, file: &Config) -> PathBuf {
        self.data_dir
            .clone()
            .or_else(|| file.data_dir.clone())
            .unwrap_or_else(|| PathBuf::from("clawdot-data"))
    }
}

/// Parse `argv` (without the program name), reading the `--config` file first so a problem with it is reported
/// before one with a flag. Returns what the line said and what the file said (an empty [`Config`] without
/// `--config`). An empty error means help was asked for.
pub fn parse(argv: &[String]) -> Result<(Cli, Config), String> {
    let mut file = Config::default();
    if let Some(i) = argv.iter().position(|x| x == "--config") {
        let path = argv.get(i + 1).ok_or("--config needs a value")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        file = Config::parse(&text)?;
    }
    let mut cli = Cli::default();
    apply_flags(&mut cli, argv)?;
    Ok((cli, file))
}

/// Record the command-line flags in `cli`. An empty error means help was asked for.
fn apply_flags(cli: &mut Cli, argv: &[String]) -> Result<(), String> {
    let mut it = argv.iter().cloned();
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--config" => {
                value("--config")?;
            }
            "--data-dir" => cli.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            #[cfg(feature = "hive")]
            "--hive-discovery" => {
                cli.hive = Some(
                    crate::hive::Mode::parse(&value("--hive-discovery")?)
                        .map_err(|e| format!("bad --hive-discovery: {e}"))?,
                );
            }
            #[cfg(feature = "hive")]
            "--hive-publish-name" => cli.hive_publish_name = true,
            "--listen" => {
                let listen: SocketAddr = value("--listen")?
                    .parse()
                    .map_err(|e| format!("bad --listen: {e}"))?;
                cli.flags.insert("listen".into(), listen.to_string().into());
            }
            "--public-url" => {
                cli.flags
                    .insert("public_url".into(), value("--public-url")?.into());
            }
            "--zone" => {
                cli.flags.insert("zone".into(), value("--zone")?.into());
            }
            "--name" => {
                cli.flags
                    .insert("device_name".into(), value("--name")?.into());
            }
            "--create-endpoint" => cli.create.push(value("--create-endpoint")?),
            "--rotate-endpoint" => cli.rotate.push(value("--rotate-endpoint")?),
            "--reset-admin" => cli.reset_admin = true,
            "--rotate-admin-token" => {
                return Err(
                    "--rotate-admin-token was removed: the web UI now asks for a password on first run; \
                     use --reset-admin to start setup again"
                        .into(),
                );
            }
            "--voice-in" => cli.voice_in = Some(PathBuf::from(value("--voice-in")?)),
            "--voice-out" => cli.voice_out = Some(PathBuf::from(value("--voice-out")?)),
            "--no-console" => cli.no_console = true,
            "--echo-speech" => cli.echo_speech = true,
            "--llm-url" => {
                cli.flags
                    .insert("llm_url".into(), value("--llm-url")?.into());
            }
            "--llm-model" => {
                cli.flags
                    .insert("llm_model".into(), value("--llm-model")?.into());
            }
            "--llm-key-env" => {
                cli.flags
                    .insert("llm_key_env".into(), value("--llm-key-env")?.into());
            }
            "--persona-file" => cli.persona_file = Some(PathBuf::from(value("--persona-file")?)),
            "--dry-run" => {
                cli.flags.insert("dry_run".into(), true.into());
            }
            "--daily-spend-usd" => {
                let x: f64 = value("--daily-spend-usd")?
                    .parse()
                    .map_err(|e| format!("bad number: {e}"))?;
                cli.flags.insert("daily_spend_usd".into(), x.into());
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::TempDir;

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    // REQ-CFG-038: flags go in their own layer, above the configuration file and saved settings.
    #[test]
    fn good_flags_fill_the_command_line_and_the_flag_layer() {
        let (a, file) = parse(&argv(&[
            "--data-dir",
            "/var/lib/dot",
            "--listen",
            "127.0.0.1:4000",
            "--name",
            "Kitchen",
            "--zone",
            "America/Chicago",
            "--llm-url",
            "http://127.0.0.1:1/v1",
            "--llm-model",
            "m",
            "--daily-spend-usd",
            "2.5",
            "--dry-run",
            "--no-console",
            "--create-endpoint",
            "ci:generic",
            "--rotate-endpoint",
            "ci",
        ]))
        .unwrap();
        assert_eq!(a.data_dir, Some(PathBuf::from("/var/lib/dot")));
        assert!(a.no_console && !a.reset_admin);
        assert_eq!(
            (a.create.clone(), a.rotate.clone()),
            (vec!["ci:generic".into()], vec!["ci".into()])
        );
        let f = &a.flags;
        assert_eq!(f["device_name"], "Kitchen");
        assert_eq!(f["listen"], "127.0.0.1:4000");
        assert_eq!(f["zone"], "America/Chicago");
        assert_eq!(f["llm_url"], "http://127.0.0.1:1/v1");
        assert_eq!(f["daily_spend_usd"], 2.5);
        assert_eq!(f["dry_run"], true);
        // The file said nothing, so it is an empty configuration.
        assert_eq!(file, Config::default());
    }

    // The command line records only what was given: no default of the program is in it.
    #[test]
    fn no_flags_says_nothing() {
        let (a, file) = parse(&[]).unwrap();
        assert_eq!(a.data_dir, None);
        assert!(!a.no_console && a.create.is_empty() && a.rotate.is_empty());
        assert!(a.flags.is_empty() && a.persona_file.is_none());
        assert_eq!(a.data_dir(&file), PathBuf::from("clawdot-data"));
    }

    #[test]
    fn an_unknown_flag_is_an_error() {
        let e = parse(&argv(&["--no-console", "--bogus"])).unwrap_err();
        assert_eq!(e, "unknown option `--bogus`");
    }

    #[test]
    fn a_flag_without_its_value_is_an_error() {
        for flag in ["--listen", "--data-dir", "--llm-url", "--daily-spend-usd"] {
            let e = parse(&argv(&["--no-console", flag])).unwrap_err();
            assert_eq!(e, format!("{flag} needs a value"));
        }
    }

    #[test]
    fn a_bad_value_is_an_error_that_names_the_flag() {
        let e = parse(&argv(&["--listen", "not-an-address"])).unwrap_err();
        assert!(e.starts_with("bad --listen:"), "{e}");
        let e = parse(&argv(&["--daily-spend-usd", "lots"])).unwrap_err();
        assert!(e.starts_with("bad number:"), "{e}");
    }

    #[test]
    fn help_is_an_empty_error_and_a_removed_flag_says_what_to_use() {
        assert_eq!(parse(&argv(&["--help"])).unwrap_err(), "");
        assert_eq!(parse(&argv(&["-h"])).unwrap_err(), "");
        let e = parse(&argv(&["--rotate-admin-token"])).unwrap_err();
        assert!(e.contains("--reset-admin"), "{e}");
    }

    #[test]
    fn the_help_text_lists_every_flag_the_parser_accepts() {
        for flag in [
            "--config",
            "--data-dir",
            "--listen",
            "--public-url",
            "--zone",
            "--name",
            "--create-endpoint",
            "--rotate-endpoint",
            "--reset-admin",
            "--voice-in",
            "--voice-out",
            "--no-console",
            "--echo-speech",
            "--llm-url",
            "--llm-model",
            "--llm-key-env",
            "--persona-file",
            "--daily-spend-usd",
            "--dry-run",
        ] {
            assert!(HELP.contains(flag), "{flag}");
        }
    }

    #[test]
    fn a_configuration_file_is_read_first_and_flags_override_it() {
        let dir = TempDir::new("cli-config");
        let path = dir.0.join("clawdot.json");
        std::fs::write(
            &path,
            r#"{"listen": "127.0.0.1:4100", "name": "Hall", "endpoints": ["ci"]}"#,
        )
        .unwrap();
        let p = path.display().to_string();
        let (a, file) = parse(&argv(&["--config", &p])).unwrap();
        assert_eq!(file.listen.as_deref(), Some("127.0.0.1:4100"));
        assert_eq!(file.name.as_deref(), Some("Hall"));
        assert_eq!(file.endpoints, vec!["ci".to_owned()]);
        assert!(a.flags.is_empty() && a.create.is_empty());

        let (a, file) = parse(&argv(&["--config", &p, "--listen", "127.0.0.1:4200"])).unwrap();
        assert_eq!(a.flags["listen"], "127.0.0.1:4200");
        assert_eq!(file.listen.as_deref(), Some("127.0.0.1:4100"));
    }

    // A problem with the file is reported before one with a flag, as it always was.
    #[test]
    fn the_configuration_file_is_read_before_the_flags() {
        let e = parse(&argv(&["--config", "/no/such/clawdot.json", "--bogus"])).unwrap_err();
        assert!(e.starts_with("cannot read /no/such/clawdot.json"), "{e}");
        let e = parse(&argv(&["--config", "/no/such/clawdot.json", "--help"])).unwrap_err();
        assert!(e.starts_with("cannot read"), "{e}");
    }

    #[test]
    fn a_missing_or_bad_configuration_file_is_an_error() {
        assert_eq!(
            parse(&argv(&["--config"])).unwrap_err(),
            "--config needs a value"
        );
        let e = parse(&argv(&["--config", "/no/such/clawdot.json"])).unwrap_err();
        assert!(e.starts_with("cannot read /no/such/clawdot.json"), "{e}");
    }
}
