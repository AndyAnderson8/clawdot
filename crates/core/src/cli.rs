//! Command-line arguments and help text for the `clawdot` binary.
//!
//! [`parse`] turns an argument list into [`Args`]; values from the configuration file and from flags are kept
//! in separate settings layers so saved settings can sit between them (REQ-CFG-038).

use crate::assistant::DEFAULT_PERSONA;
use crate::audit::ActionPolicy;
use crate::config::{
    AgentConfig, Config, DeviceConfig, SatelliteConfig, SpeechConfig, UpdatesConfig,
};
#[cfg(feature = "esphome-satellite")]
use crate::esphome_cfg::EsphomeConfig;
use crate::httptools::ToolDef;
use crate::inbox::endpoint::QuietHours;
use crate::mcp::ServerDef;
use crate::media::Station;
use crate::seed::seed_from_config;
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

#[derive(Debug)]
pub struct Args {
    /// Values from the configuration file and from flags, kept apart so saved settings can sit between them
    /// (REQ-CFG-038).
    pub file_layer: Layer,
    pub flag_layer: Layer,
    /// The persona file came from a flag, not the configuration file.
    pub persona_flag: bool,
    /// The system prompt in effect.
    pub persona: String,
    pub data_dir: PathBuf,
    pub listen: SocketAddr,
    pub public_url: Option<String>,
    pub zone: Option<String>,
    pub create: Vec<String>,
    pub rotate: Vec<String>,
    pub reset_admin: bool,
    /// An endpoint command was given on the command line: print and exit instead of serving.
    pub manage: bool,
    pub console: bool,
    /// Echo spoken text to stdout even without the console (diagnosis only).
    pub echo_speech: bool,
    pub llm_url: Option<String>,
    pub llm_model: Option<String>,
    pub key_env: String,
    pub persona_file: Option<PathBuf>,
    pub tools: Vec<ToolDef>,
    pub mcp: Vec<ServerDef>,
    pub stations: Vec<Station>,
    pub speech: SpeechConfig,
    pub satellite: SatelliteConfig,
    pub updates: UpdatesConfig,
    /// What the device is called and which parts it has that cannot be seen (the `device` section).
    pub device: DeviceConfig,
    #[cfg(feature = "esphome-satellite")]
    pub esphome: EsphomeConfig,
    /// Hive discovery mode (`--hive-discovery`); `None` leaves the hive unstarted.
    #[cfg(feature = "hive")]
    pub hive: Option<crate::hive::Mode>,
    #[cfg(feature = "hive")]
    pub hive_publish_name: bool,
    pub voice_in: Option<PathBuf>,
    pub voice_out: Option<PathBuf>,
    pub daily_spend: Option<f64>,
    /// Daily action caps and the dry-run switch (REQ-SEC-015, REQ-SEC-016); the effective settings (REQ-CFG-062).
    pub actions: ActionPolicy,
    /// Turn limits, speech while working and notes (REQ-AST-019..025); the effective settings (REQ-CFG-062).
    pub agent: AgentConfig,
    /// The device-wide quiet-hours window (REQ-INB-040).
    pub quiet: Option<QuietHours>,
}

/// Parse `argv` (without the program name), reading the `--config` file first so flags override it.
/// An empty error means help was asked for.
pub fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = defaults()?;
    if let Some(i) = argv.iter().position(|x| x == "--config") {
        let path = argv.get(i + 1).ok_or("--config needs a value")?;
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        seed_from_config(&mut a, &Config::parse(&text)?)?;
    }
    apply_flags(&mut a, argv)?;
    Ok(a)
}

/// The arguments as they are with no file and no flags.
fn defaults() -> Result<Args, String> {
    Ok(Args {
        file_layer: Layer::new(),
        flag_layer: Layer::new(),
        persona_flag: false,
        persona: DEFAULT_PERSONA.to_owned(),
        data_dir: PathBuf::from("clawdot-data"),
        listen: "127.0.0.1:3335"
            .parse()
            .map_err(|_| "internal: bad default address".to_owned())?,
        public_url: None,
        zone: None,
        create: Vec::new(),
        rotate: Vec::new(),
        reset_admin: false,
        manage: false,
        console: true,
        echo_speech: false,
        llm_url: None,
        llm_model: None,
        key_env: "OPENROUTER_API_KEY".into(),
        persona_file: None,
        tools: Vec::new(),
        mcp: Vec::new(),
        stations: Vec::new(),
        speech: SpeechConfig::default(),
        satellite: SatelliteConfig::default(),
        updates: UpdatesConfig::default(),
        device: DeviceConfig::default(),
        #[cfg(feature = "esphome-satellite")]
        esphome: EsphomeConfig::default(),
        #[cfg(feature = "hive")]
        hive: None,
        #[cfg(feature = "hive")]
        hive_publish_name: false,
        voice_in: None,
        voice_out: None,
        quiet: None,
        daily_spend: None,
        actions: ActionPolicy::default(),
        agent: AgentConfig::default(),
    })
}

/// Apply the command-line flags over `a`, so a flag overrides the file. An empty error means help was asked for.
fn apply_flags(a: &mut Args, argv: &[String]) -> Result<(), String> {
    let mut it = argv.iter().cloned();
    while let Some(flag) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match flag.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--config" => {
                value("--config")?;
            }
            "--data-dir" => a.data_dir = PathBuf::from(value("--data-dir")?),
            #[cfg(feature = "hive")]
            "--hive-discovery" => {
                a.hive = Some(
                    crate::hive::Mode::parse(&value("--hive-discovery")?)
                        .map_err(|e| format!("bad --hive-discovery: {e}"))?,
                );
            }
            #[cfg(feature = "hive")]
            "--hive-publish-name" => a.hive_publish_name = true,
            "--listen" => {
                a.listen = value("--listen")?
                    .parse()
                    .map_err(|e| format!("bad --listen: {e}"))?;
                a.flag_layer
                    .insert("listen".into(), a.listen.to_string().into());
            }
            "--public-url" => {
                a.flag_layer
                    .insert("public_url".into(), value("--public-url")?.into());
            }
            "--zone" => {
                a.flag_layer.insert("zone".into(), value("--zone")?.into());
            }
            "--name" => {
                a.flag_layer
                    .insert("device_name".into(), value("--name")?.into());
            }
            "--create-endpoint" => {
                a.create.push(value("--create-endpoint")?);
                a.manage = true;
            }
            "--rotate-endpoint" => {
                a.rotate.push(value("--rotate-endpoint")?);
                a.manage = true;
            }
            "--reset-admin" => a.reset_admin = true,
            "--rotate-admin-token" => {
                return Err(
                    "--rotate-admin-token was removed: the web UI now asks for a password on first run; \
                     use --reset-admin to start setup again"
                        .into(),
                );
            }
            "--voice-in" => a.voice_in = Some(PathBuf::from(value("--voice-in")?)),
            "--voice-out" => a.voice_out = Some(PathBuf::from(value("--voice-out")?)),
            "--no-console" => a.console = false,
            "--echo-speech" => a.echo_speech = true,
            "--llm-url" => {
                a.flag_layer
                    .insert("llm_url".into(), value("--llm-url")?.into());
            }
            "--llm-model" => {
                a.flag_layer
                    .insert("llm_model".into(), value("--llm-model")?.into());
            }
            "--llm-key-env" => {
                a.flag_layer
                    .insert("llm_key_env".into(), value("--llm-key-env")?.into());
            }
            "--persona-file" => {
                a.persona_file = Some(PathBuf::from(value("--persona-file")?));
                a.persona_flag = true;
            }
            "--dry-run" => {
                a.flag_layer.insert("dry_run".into(), true.into());
            }
            "--daily-spend-usd" => {
                let x: f64 = value("--daily-spend-usd")?
                    .parse()
                    .map_err(|e| format!("bad number: {e}"))?;
                a.flag_layer.insert("daily_spend_usd".into(), x.into());
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
    fn good_flags_fill_the_arguments_and_the_flag_layer() {
        let a = parse(&argv(&[
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
        assert_eq!(a.data_dir, PathBuf::from("/var/lib/dot"));
        assert_eq!(a.listen, "127.0.0.1:4000".parse().unwrap());
        assert!(!a.console && a.manage && !a.reset_admin);
        assert_eq!(
            (a.create, a.rotate),
            (vec!["ci:generic".into()], vec!["ci".into()])
        );
        let f = &a.flag_layer;
        assert_eq!(f["device_name"], "Kitchen");
        assert_eq!(f["listen"], "127.0.0.1:4000");
        assert_eq!(f["zone"], "America/Chicago");
        assert_eq!(f["llm_url"], "http://127.0.0.1:1/v1");
        assert_eq!(f["daily_spend_usd"], 2.5);
        assert_eq!(f["dry_run"], true);
        assert!(a.file_layer.is_empty());
    }

    #[test]
    fn no_flags_leaves_the_defaults() {
        let a = parse(&[]).unwrap();
        assert_eq!(a.data_dir, PathBuf::from("clawdot-data"));
        assert_eq!(a.listen, "127.0.0.1:3335".parse().unwrap());
        assert!(a.console && !a.manage && a.flag_layer.is_empty());
        assert_eq!(a.key_env, "OPENROUTER_API_KEY");
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
        let a = parse(&argv(&["--config", &p])).unwrap();
        assert_eq!(a.listen, "127.0.0.1:4100".parse().unwrap());
        assert_eq!(a.file_layer["device_name"], "Hall");
        assert_eq!(a.create, vec!["ci".to_owned()]);
        assert!(a.flag_layer.is_empty());

        let a = parse(&argv(&["--config", &p, "--listen", "127.0.0.1:4200"])).unwrap();
        assert_eq!(a.listen, "127.0.0.1:4200".parse().unwrap());
        assert_eq!(a.flag_layer["listen"], "127.0.0.1:4200");
        assert_eq!(a.file_layer["listen"], "127.0.0.1:4100");
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
