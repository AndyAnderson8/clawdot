//! `clawdot say "text"`: speak a message on a running ClawDot (REQ-INB-026).
//!
//! A thin client of `POST /api/announce`. The token is read from an environment variable whose name is given
//! on the command line, never from the command line itself, so it does not appear in a process list or a
//! shell history.

use crate::transport::{Connector, Endpoint};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// What the command was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SayArgs {
    /// The request body.
    pub body: Value,
    /// The server's base URL.
    pub url: String,
    /// The name of the environment variable that holds the token.
    pub token_env: String,
}

/// The help text for `clawdot say --help`.
pub const HELP: &str = "\
clawdot say: speak a message on a running ClawDot

USAGE:
    clawdot say TEXT [options]

OPTIONS:
    --source NAME     Who is speaking; the speaker says \"From NAME: ...\" (default: the endpoint's name)
    --title TEXT      A short headline (default: the first sentence of the text)
    --priority P      low, normal or urgent; only urgent is spoken in quiet hours (default: normal)
    --reply           Allow the listener to answer it (default: a one-way message)
    --ttl SECS        Drop it if it has not been spoken after this many seconds (1 to 86400)
    --endpoint NAME   With an administrator session instead of an endpoint token: the endpoint to speak on
    --url URL         Where ClawDot listens (default: $CLAWDOT_URL, else http://127.0.0.1:3335)
    --token-env VAR   Name of the environment variable holding the endpoint token (default: CLAWDOT_TOKEN)
    -h, --help        Show this help

The token is never taken from the command line. Create an endpoint with `clawdot --create-endpoint NAME:generic`.
";

/// Read the command line after `say`.
///
/// # Errors
/// A message for the user; an empty message means `--help` was asked for.
pub fn parse(args: &[String], env: impl Fn(&str) -> Option<String>) -> Result<SayArgs, String> {
    let mut body = Map::new();
    let mut text: Option<String> = None;
    let mut url = None;
    let mut token_env = "CLAWDOT_TOKEN".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--source" => {
                body.insert("source".into(), json!(value("--source")?));
            }
            "--title" => {
                body.insert("title".into(), json!(value("--title")?));
            }
            "--priority" => {
                body.insert("priority".into(), json!(value("--priority")?));
            }
            "--endpoint" => {
                body.insert("endpoint".into(), json!(value("--endpoint")?));
            }
            "--ttl" => {
                let v = value("--ttl")?;
                let n: i64 = v
                    .parse()
                    .map_err(|_| format!("--ttl needs a whole number of seconds, got `{v}`"))?;
                body.insert("ttl_secs".into(), json!(n));
            }
            "--reply" => {
                body.insert("reply".into(), json!(true));
            }
            "--url" => url = Some(value("--url")?),
            "--token-env" => token_env = value("--token-env")?,
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ if text.is_none() => text = Some(a.clone()),
            _ => return Err("give the text as one argument; put it in quotes".into()),
        }
    }
    let text = text.ok_or("give the text to say, for example: clawdot say \"dinner is ready\"")?;
    body.insert("text".into(), json!(text));
    Ok(SayArgs {
        body: Value::Object(body),
        url: url
            .or_else(|| env("CLAWDOT_URL").filter(|u| !u.is_empty()))
            .unwrap_or_else(|| "http://127.0.0.1:3335".into()),
        token_env,
    })
}

/// A one-line explanation of a response.
fn explain(status: u16, body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = v.get("error").and_then(Value::as_str);
    match status {
        202 => match v.get("filtered").and_then(Value::as_str) {
            Some(why) => Ok(format!(
                "not queued: the endpoint's filter dropped it ({why})"
            )),
            None => Ok("queued".into()),
        },
        401 => Err("the server refused the token (is it current, and is the endpoint on?)".into()),
        429 => Err("the server says too many requests; try again in a moment".into()),
        s => Err(format!(
            "the server answered {s}{}",
            error.map(|e| format!(": {e}")).unwrap_or_default()
        )),
    }
}

/// Post the announcement and report the outcome in one line.
///
/// # Errors
/// A one-line reason: no token, no connection, or a refusal.
pub async fn run(a: &SayArgs, token: Option<&str>) -> Result<String, String> {
    let endpoint = Endpoint::parse(&a.url)?;
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("{}/api/announce", endpoint.base))
        .header("host", endpoint.host_header())
        .header("connection", "close")
        .header("user-agent", "clawdot-say")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let request = req
        .body(Full::new(Bytes::from(a.body.to_string())))
        .map_err(|e| e.to_string())?;
    let work = async {
        let connector = Connector::new()?;
        let response = connector.request(&endpoint, request).await?;
        let status = response.status().as_u16();
        let mut body = response.into_body();
        let mut kept = Vec::new();
        while let Some(Ok(frame)) = body.frame().await {
            if let Some(d) = frame.data_ref() {
                kept.extend_from_slice(d);
                if kept.len() >= 4096 {
                    break;
                }
            }
        }
        Ok::<_, String>((status, String::from_utf8_lossy(&kept).into_owned()))
    };
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), work)
        .await
        .map_err(|_| "no answer within 10 s".to_owned())??;
    explain(status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn the_text_and_options_become_the_request_body() {
        let a = parse(
            &args(&[
                "dinner is ready",
                "--source",
                "Kitchen",
                "--priority",
                "urgent",
                "--ttl",
                "300",
                "--reply",
            ]),
            no_env,
        )
        .unwrap();
        assert_eq!(
            a.body,
            json!({"text":"dinner is ready","source":"Kitchen","priority":"urgent","ttl_secs":300,"reply":true})
        );
        assert_eq!(a.url, "http://127.0.0.1:3335");
        assert_eq!(a.token_env, "CLAWDOT_TOKEN");
    }

    #[test]
    fn the_url_comes_from_the_flag_then_the_environment_and_a_token_is_never_a_flag() {
        let env = |k: &str| (k == "CLAWDOT_URL").then(|| "http://speaker.lan:3335".to_owned());
        assert_eq!(
            parse(&args(&["hi"]), env).unwrap().url,
            "http://speaker.lan:3335"
        );
        assert_eq!(
            parse(
                &args(&["hi", "--url", "http://x:1", "--token-env", "MY_TOKEN"]),
                env
            )
            .unwrap()
            .url,
            "http://x:1"
        );
        let e = parse(&args(&["hi", "--token", "cdt_secret"]), no_env).unwrap_err();
        assert!(e.contains("unknown option"), "{e}");
    }

    #[test]
    fn bad_command_lines_say_what_is_wrong() {
        for (list, needle) in [
            (vec![], "give the text"),
            (vec!["a", "b"], "quotes"),
            (vec!["a", "--ttl", "soon"], "--ttl"),
            (vec!["a", "--source"], "needs a value"),
        ] {
            let e = parse(&args(&list), no_env).unwrap_err();
            assert!(e.contains(needle), "{list:?}: {e}");
        }
        assert_eq!(parse(&args(&["--help"]), no_env).unwrap_err(), "");
    }

    #[test]
    fn responses_become_one_honest_line() {
        assert_eq!(explain(202, r#"{"queued":true,"id":3}"#).unwrap(), "queued");
        assert!(
            explain(202, r#"{"queued":false,"filtered":"muted"}"#)
                .unwrap()
                .contains("muted")
        );
        assert!(explain(401, "{}").unwrap_err().contains("token"));
        assert!(
            explain(400, r#"{"error":"priority must be low"}"#)
                .unwrap_err()
                .contains("priority")
        );
    }
}
