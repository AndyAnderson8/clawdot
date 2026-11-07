//! A small Server-Sent Events parser.

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `id:` field, if given.
    pub id: Option<String>,
    /// The `event:` field; `message` when absent.
    pub event: String,
    /// The `data:` lines joined with newlines.
    pub data: String,
}

/// Feed lines (without their line ending); get events at blank lines.
#[derive(Debug, Default)]
pub struct SseParser {
    id: Option<String>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// Process one line. Returns an event when the line is the blank line that ends one.
    pub fn feed_line(&mut self, line: &str) -> Option<SseEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            let had_data = !self.data.is_empty();
            let event = SseEvent {
                id: self.id.take(),
                event: self.event.take().unwrap_or_else(|| "message".to_owned()),
                data: self.data.join("\n"),
            };
            self.data.clear();
            return had_data.then_some(event);
        }
        if line.starts_with(':') {
            return None; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "id" => self.id = Some(value.to_owned()),
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            _ => {} // retry and unknown fields are ignored
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str) -> Vec<SseEvent> {
        let mut p = SseParser::default();
        input.split('\n').filter_map(|l| p.feed_line(l)).collect()
    }

    #[test]
    fn parses_a_reply_event() {
        let ev = run("id: 7\nevent: reply\ndata: {\"a\":1}\n\n");
        assert_eq!(
            ev,
            vec![SseEvent {
                id: Some("7".into()),
                event: "reply".into(),
                data: "{\"a\":1}".into()
            }]
        );
    }

    #[test]
    fn ignores_comments_retry_and_blank_keepalives() {
        let ev = run("retry: 3000\n\n: keepalive\n\n: keepalive\n\nevent: reply\ndata: x\n\n");
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].data, "x");
    }

    #[test]
    fn joins_multi_line_data_and_handles_crlf() {
        let ev = run("data: one\r\ndata: two\r\n\r\n");
        assert_eq!(ev[0].data, "one\ntwo");
        assert_eq!(ev[0].event, "message");
    }

    #[test]
    fn state_does_not_leak_between_events() {
        let ev = run("id: 1\nevent: reply\ndata: a\n\ndata: b\n\n");
        assert_eq!(
            ev[1],
            SseEvent {
                id: None,
                event: "message".into(),
                data: "b".into()
            }
        );
    }

    #[test]
    fn a_colonless_line_is_a_field_with_an_empty_value() {
        let ev = run("data\n\n");
        assert_eq!(ev[0].data, "");
    }
}
