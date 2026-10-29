//! Server-sent events, as both model APIs stream a reply: `event:` and `data:` lines, a
//! blank line between events. Hand-parsed: the bytes come in whatever chunks the network
//! makes, a multi-byte character may be split across two, and a block is only read once it
//! is whole. Comment lines (`: ping`) and `id:`/`retry:` fields are skipped; several `data:`
//! lines are joined with a newline, as the standard says.

/// One event: its `event:` name ("" when the server sends none, as Chat Completions does)
/// and its data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

/// The bytes received so far, with line endings normalised to `\n` (a trailing `\r` is
/// held back until the next byte says whether it was half of a `\r\n`).
#[derive(Debug, Default)]
pub struct Sse {
    buf: Vec<u8>,
    cr: bool,
}

impl Sse {
    /// Feed a chunk; the events it completed, in order.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        for &b in bytes {
            match b {
                b'\r' => {
                    if self.cr {
                        self.buf.push(b'\n');
                    }
                    self.cr = true;
                }
                b'\n' => {
                    self.buf.push(b'\n');
                    self.cr = false;
                }
                other => {
                    if self.cr {
                        self.buf.push(b'\n');
                        self.cr = false;
                    }
                    self.buf.push(other);
                }
            }
        }
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = self.buf.drain(..end + 2).collect();
            if let Some(ev) = parse_block(&String::from_utf8_lossy(&block[..end])) {
                out.push(ev);
            }
        }
        out
    }

    /// The stream ended: a last block without its blank line, if any.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.cr {
            self.buf.push(b'\n');
            self.cr = false;
        }
        let rest = std::mem::take(&mut self.buf);
        parse_block(String::from_utf8_lossy(&rest).trim_end_matches('\n'))
    }
}

fn parse_block(text: &str) -> Option<SseEvent> {
    let mut event = String::new();
    let mut data: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => event = value.to_string(),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
        }
    }

    #[test]
    fn events_arrive_in_any_chunking() {
        let text = "event: a\ndata: {\"n\": 1}\n\nevent: b\ndata: {\"n\": 2}\n\n";
        let whole = Sse::default().push(text.as_bytes());
        assert_eq!(whole, vec![ev("a", "{\"n\": 1}"), ev("b", "{\"n\": 2}")]);
        let mut sse = Sse::default();
        let mut split = Vec::new();
        for b in text.as_bytes() {
            split.extend(sse.push(&[*b]));
        }
        assert_eq!(split, whole);
        assert_eq!(sse.finish(), None);
    }

    #[test]
    fn crlf_comments_and_multiline_data() {
        let text = ": keepalive\r\nid: 7\r\ndata: one\r\ndata: two\r\n\r\n";
        let mut sse = Sse::default();
        let mut out = Vec::new();
        for chunk in text.as_bytes().chunks(5) {
            out.extend(sse.push(chunk));
        }
        assert_eq!(out, vec![ev("", "one\ntwo")]);
    }

    #[test]
    fn a_last_block_without_its_blank_line_is_read_at_the_end() {
        let mut sse = Sse::default();
        assert_eq!(sse.push(b"data: [DONE]"), vec![]);
        assert_eq!(sse.finish(), Some(ev("", "[DONE]")));
        assert_eq!(Sse::default().finish(), None);
    }

    #[test]
    fn a_split_multibyte_character_survives() {
        let text = "data: caf\u{e9}\n\n";
        let bytes = text.as_bytes();
        let mut sse = Sse::default();
        let mut out = sse.push(&bytes[..9]);
        out.extend(sse.push(&bytes[9..]));
        assert_eq!(out, vec![ev("", "caf\u{e9}")]);
    }
}
