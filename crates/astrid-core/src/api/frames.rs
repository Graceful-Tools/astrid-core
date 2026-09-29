//! Cutting a server-sent-event stream into events.
//!
//! Bytes, not text, until an event is whole. A network read ends wherever it ends — inside an
//! emoji, inside an accented letter — and decoding each read on its own garbled every character a
//! read boundary fell in (the Apple apps' AITD-313, which their own buffer fixed and this one did
//! not). An event ends on a blank line, which the spec lets a server write as `\n\n`, `\r\n\r\n`,
//! `\r\r` or a mix; and a server that never sends one must not grow the buffer without bound.
//!
//! Ported with its tests from `astrid-ios/Astrid App/Core/RealTime/SSEFrameBuffer.swift`.

/// More than this pending with no end in sight is a broken stream, not a big event: it is dropped
/// and the next event starts clean.
pub const MAX_PENDING_BYTES: usize = 1_048_576;

#[derive(Debug, Default)]
pub struct FrameBuffer {
    pending: Vec<u8>,
}

enum Break {
    /// A line break of this many bytes.
    Of(usize),
    /// Not a line break.
    No,
    /// A `\r` at the very end: it may yet be the first half of `\r\n`.
    Undecided,
}

impl FrameBuffer {
    /// Take in one read; answer the events it completed, oldest first, without their separator.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(bytes);
        let mut frames = Vec::new();
        while let Some((end, next)) = self.boundary() {
            let frame = String::from_utf8_lossy(&self.pending[..end]).into_owned();
            self.pending.drain(..next);
            if !frame.trim().is_empty() {
                frames.push(frame);
            }
        }
        if self.pending.len() > MAX_PENDING_BYTES {
            tracing::warn!(bytes = self.pending.len(), "an event never ended; dropped");
            self.pending.clear();
        }
        frames
    }

    /// How many bytes are waiting for their event to end.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The first blank line: where the event before it ends, and where the next one starts.
    fn boundary(&self) -> Option<(usize, usize)> {
        let bytes = &self.pending;
        let mut index = 0;
        while index < bytes.len() {
            match line_break(bytes, index) {
                Break::No => index += 1,
                Break::Undecided => return None,
                Break::Of(width) => {
                    let after = index + width;
                    if after >= bytes.len() {
                        return None;
                    }
                    match line_break(bytes, after) {
                        Break::Of(second) => return Some((index, after + second)),
                        Break::Undecided => return None,
                        Break::No => index = after,
                    }
                }
            }
        }
        None
    }
}

fn line_break(bytes: &[u8], index: usize) -> Break {
    match bytes[index] {
        b'\n' => Break::Of(1),
        b'\r' => match bytes.get(index + 1) {
            Some(b'\n') => Break::Of(2),
            Some(_) => Break::Of(1),
            None => Break::Undecided,
        },
        _ => Break::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed text through the buffer the way a socket might: one byte at a time.
    fn streamed(text: &str) -> Vec<String> {
        let mut buffer = FrameBuffer::default();
        text.as_bytes()
            .iter()
            .flat_map(|byte| buffer.push(std::slice::from_ref(byte)))
            .collect()
    }

    // ── The bug ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn non_ascii_survives_a_byte_at_a_time() {
        let payload = r#"data: {"text":"Café 日本 🚀"}"#;
        assert_eq!(streamed(&format!("{payload}\n\n")), vec![payload]);
    }

    #[test]
    fn a_multi_byte_character_split_across_reads_is_not_mangled() {
        let payload = r#"data: {"a":"é","b":"日","c":"🚀","d":"Ω≈ç√"}"#;
        assert_eq!(streamed(&format!("{payload}\n\n")), vec![payload]);
    }

    #[test]
    fn an_event_is_byte_identical_to_what_was_sent() {
        let payload = "data: Grüße aus München — 東京 🎌";
        let received = streamed(&format!("{payload}\n\n"));
        assert_eq!(received[0].as_bytes(), payload.as_bytes());
    }

    // ── Framing ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn several_events_in_one_read_are_all_returned() {
        let mut buffer = FrameBuffer::default();
        assert_eq!(
            buffer.push(b"data: one\n\ndata: two\n\ndata: three\n\n"),
            vec!["data: one", "data: two", "data: three"]
        );
    }

    #[test]
    fn an_incomplete_event_waits_for_its_end() {
        let mut buffer = FrameBuffer::default();
        assert!(buffer.push(b"data: partial").is_empty());
        assert!(buffer.push(b"\n").is_empty());
        assert_eq!(buffer.push(b"\n"), vec!["data: partial"]);
    }

    #[test]
    fn a_multi_line_event_keeps_its_inner_newlines() {
        assert_eq!(
            streamed("event: comment_added\ndata: {\"id\":\"1\"}\n\n"),
            vec!["event: comment_added\ndata: {\"id\":\"1\"}"]
        );
    }

    #[test]
    fn carriage_return_line_endings_end_an_event() {
        assert_eq!(streamed("data: crlf\r\n\r\n"), vec!["data: crlf"]);
        assert_eq!(
            streamed("data: cr\r\rdata: next\r\rx"),
            vec!["data: cr", "data: next"]
        );
    }

    #[test]
    fn a_trailing_carriage_return_is_held_rather_than_guessed_at() {
        let mut buffer = FrameBuffer::default();
        assert!(buffer.push(b"data: x\r\r").is_empty());
        assert_eq!(buffer.push(b"\ndata: y\n\n"), vec!["data: x", "data: y"]);
    }

    #[test]
    fn keepalive_blank_lines_are_not_events() {
        assert_eq!(streamed("\n\n\n\ndata: real\n\n"), vec!["data: real"]);
    }

    #[test]
    fn a_comment_only_keepalive_is_returned_and_harmless() {
        assert_eq!(streamed(": ping\n\n"), vec![": ping"]);
    }

    // ── Hardening ───────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_server_that_never_ends_an_event_cannot_grow_the_buffer_without_bound() {
        let mut buffer = FrameBuffer::default();
        let junk = vec![b'x'; 64 * 1024];
        for _ in 0..64 {
            buffer.push(&junk);
            assert!(buffer.pending_len() <= MAX_PENDING_BYTES);
        }
    }

    #[test]
    fn the_buffer_recovers_after_an_overflow() {
        let mut buffer = FrameBuffer::default();
        buffer.push(&vec![b'x'; MAX_PENDING_BYTES + 1]);
        assert_eq!(buffer.push(b"data: after\n\n"), vec!["data: after"]);
    }
}
