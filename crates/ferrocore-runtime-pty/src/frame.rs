//! Incremental PTY sentinel and Windows cursor-position query parsing.
//!
//! The nonce and command ID distinguish accidental matches, not hostile output.
//! The caller must use a single reader and retain bytes returned by `finish` on
//! EOF or cancellation. A parser is created for each active command.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramePlatform {
    Linux,
    Windows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameStatus {
    ExitCode(i32),
    Ambiguous,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct FrameEvents {
    pub output: Vec<u8>,
    pub status: Option<FrameStatus>,
    /// Raw bytes after the sentinel, to be fed to the next command's parser.
    pub remainder: Vec<u8>,
    pub cursor_queries: usize,
}

pub struct FrameParser {
    platform: FramePlatform,
    prefix: Vec<u8>,
    pending: Vec<u8>,
    completed: bool,
}

const CURSOR_QUERY: &[u8] = b"\x1b[6n";
const WINDOWS_AMBIGUOUS: &[u8] = b"AMBIGUOUS";
// i32 includes at most a sign and ten decimal digits.
const MAX_EXIT_LEN: usize = 11;

impl FrameParser {
    pub fn new(platform: FramePlatform, nonce: &str, cmd_id: u64) -> Self {
        let mut prefix = Vec::new();
        if platform == FramePlatform::Linux {
            prefix.push(0x1e);
        }
        prefix.extend_from_slice(format!("__FC_END__:{nonce}:{cmd_id}:").as_bytes());
        Self { platform, prefix, pending: Vec::new(), completed: false }
    }

    /// Consumes a chunk up to and including the sentinel. Cursor queries before
    /// it are counted and removed from output. Bytes after it are returned
    /// unmodified in `remainder`, including on subsequent calls.
    pub fn push(&mut self, bytes: &[u8]) -> FrameEvents {
        let mut events = FrameEvents::default();
        for (index, &byte) in bytes.iter().enumerate() {
            if self.completed {
                events.remainder.extend_from_slice(&bytes[index..]);
                break;
            }
            self.pending.push(byte);
            self.process(&mut events);
        }
        events
    }

    /// Returns any unresolved prefix exactly as received, for EOF or cancel.
    pub fn finish(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }

    fn process(&mut self, events: &mut FrameEvents) {
        while !self.pending.is_empty() {
            if self.platform == FramePlatform::Windows && CURSOR_QUERY.starts_with(&self.pending) {
                if self.pending.len() == CURSOR_QUERY.len() {
                    self.pending.clear();
                    events.cursor_queries += 1;
                }
                break;
            }
            if !self.completed {
                match self.frame_candidate() {
                    Candidate::Incomplete => break,
                    Candidate::Complete(status) => {
                        self.pending.clear();
                        self.completed = true;
                        events.status = Some(status);
                        break;
                    }
                    Candidate::Invalid => {}
                }
            }
            events.output.push(self.pending.remove(0));
        }
    }

    fn frame_candidate(&self) -> Candidate {
        let input = &self.pending;
        if input.len() <= self.prefix.len() {
            return if self.prefix.starts_with(input) { Candidate::Incomplete } else { Candidate::Invalid };
        }
        if !input.starts_with(&self.prefix) {
            return Candidate::Invalid;
        }
        let rest = &input[self.prefix.len()..];
        let (status, end) = if self.platform == FramePlatform::Windows
            && rest.len() <= WINDOWS_AMBIGUOUS.len()
            && WINDOWS_AMBIGUOUS.starts_with(rest)
        {
            return Candidate::Incomplete;
        } else if self.platform == FramePlatform::Windows && rest.starts_with(WINDOWS_AMBIGUOUS) {
            (FrameStatus::Ambiguous, &rest[WINDOWS_AMBIGUOUS.len()..])
        } else {
            let mut digits_end = usize::from(rest.first() == Some(&b'-'));
            while digits_end < rest.len() && rest[digits_end].is_ascii_digit() {
                digits_end += 1;
            }
            if digits_end > MAX_EXIT_LEN {
                return Candidate::Invalid;
            }
            if digits_end == rest.len() {
                return Candidate::Incomplete;
            }
            if digits_end == 0 || (digits_end == 1 && rest[0] == b'-') {
                return Candidate::Invalid;
            }
            let Some(code) = std::str::from_utf8(&rest[..digits_end]).ok().and_then(|s| s.parse::<i32>().ok()) else {
                return Candidate::Invalid;
            };
            (FrameStatus::ExitCode(code), &rest[digits_end..])
        };
        let (lf, crlf): (&[u8], &[u8]) = if self.platform == FramePlatform::Linux {
            (b"__\x1f\n", b"__\x1f\r\n")
        } else {
            (b"__\n", b"__\r\n")
        };
        if !lf.starts_with(end) && !crlf.starts_with(end) {
            return Candidate::Invalid;
        }
        if end == lf || end == crlf { Candidate::Complete(status) } else { Candidate::Incomplete }
    }
}

enum Candidate {
    Incomplete,
    Complete(FrameStatus),
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    fn frame(platform: FramePlatform, code: &str, newline: &str) -> Vec<u8> {
        let (start, end) = if platform == FramePlatform::Linux { ("\x1e", "\x1f") } else { ("", "") };
        format!("{start}__FC_END__:{NONCE}:7:{code}__{end}{newline}").into_bytes()
    }

    #[test]
    fn every_split_and_bytewise_preserve_output() {
        for platform in [FramePlatform::Linux, FramePlatform::Windows] {
            for newline in ["\n", "\r\n"] {
                let mut input = b"prompt> result\r\n".to_vec();
                input.extend(frame(platform, "-2147483648", newline));
                input.extend_from_slice(b"next prompt> ");
                for split in 0..=input.len() {
                    let mut parser = FrameParser::new(platform, NONCE, 7);
                    let a = parser.push(&input[..split]);
                    let b = parser.push(&input[split..]);
                    assert_eq!([a.output, b.output, parser.finish()].concat(), b"prompt> result\r\n");
                    assert_eq!([a.remainder, b.remainder].concat(), b"next prompt> ");
                    assert_eq!(a.status.or(b.status), Some(FrameStatus::ExitCode(i32::MIN)));
                }
                let mut parser = FrameParser::new(platform, NONCE, 7);
                let mut output = Vec::new();
                let mut remainder = Vec::new();
                let mut status = None;
                for byte in &input {
                    let event = parser.push(std::slice::from_ref(byte));
                    output.extend(event.output);
                    remainder.extend(event.remainder);
                    status = status.or(event.status);
                }
                output.extend(parser.finish());
                assert_eq!(output, b"prompt> result\r\n");
                assert_eq!(remainder, b"next prompt> ");
                assert_eq!(status, Some(FrameStatus::ExitCode(i32::MIN)));
            }
        }
    }

    #[test]
    fn remainder_preserves_split_cursor_query_for_next_parser() {
        for split in 0..=CURSOR_QUERY.len() {
            let mut first = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let mut input = b"old".to_vec();
            input.extend(frame(FramePlatform::Windows, "0", "\r\n"));
            input.extend_from_slice(&CURSOR_QUERY[..split]);
            let event = first.push(&input);
            assert_eq!(event.output, b"old");
            assert_eq!(event.status, Some(FrameStatus::ExitCode(0)));
            assert_eq!(event.cursor_queries, 0);
            assert_eq!(event.remainder, &CURSOR_QUERY[..split]);
            let rest = first.push(&CURSOR_QUERY[split..]);
            assert!(rest.output.is_empty());
            assert_eq!(rest.remainder, &CURSOR_QUERY[split..]);

            let mut next = FrameParser::new(FramePlatform::Windows, NONCE, 8);
            let a = next.push(&event.remainder);
            let b = next.push(&rest.remainder);
            assert_eq!(a.cursor_queries + b.cursor_queries, 1);
            assert!(a.output.is_empty() && b.output.is_empty());
            assert!(next.finish().is_empty());
        }
    }

    #[test]
    fn malformed_prefix_before_frame_and_partial_query_after_are_preserved() {
        let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
        let malformed = format!("__FC_END__:{NONCE}:7:12x");
        let mut input = malformed.as_bytes().to_vec();
        input.extend(frame(FramePlatform::Windows, "3", "\n"));
        input.extend_from_slice(b"\x1b[6");
        let event = parser.push(&input);
        assert_eq!(event.output, malformed.as_bytes());
        assert_eq!(event.status, Some(FrameStatus::ExitCode(3)));
        assert_eq!(event.remainder, b"\x1b[6");
        assert!(parser.finish().is_empty());
    }

    #[test]
    fn invalid_candidates_and_nested_prefixes_are_lossless() {
        for platform in [FramePlatform::Linux, FramePlatform::Windows] {
            let mut input = frame(platform, "2147483648", "\n");
            input.extend(frame(platform, "-2147483649", "\n"));
            input.extend(frame(platform, "12x", "\n"));
            input.extend(frame(platform, "-", "\n"));
            if platform == FramePlatform::Linux {
                input.extend(frame(platform, "AMBIGUOUS", "\n"));
            }
            input.extend_from_slice(if platform == FramePlatform::Linux { b"\x1e__FC_END__:bad" } else { b"__FC_END__:bad" });
            let expected = input.clone();
            input.extend(frame(platform, "2", "\r\n"));
            let mut parser = FrameParser::new(platform, NONCE, 7);
            let events = parser.push(&input);
            assert_eq!(events.output, expected);
            assert_eq!(events.status, Some(FrameStatus::ExitCode(2)));
            assert!(parser.finish().is_empty());
        }
    }

    #[test]
    fn eof_returns_every_partial_prefix() {
        for platform in [FramePlatform::Linux, FramePlatform::Windows] {
            let full = frame(platform, "23", "\r\n");
            for end in 0..full.len() {
                let mut parser = FrameParser::new(platform, NONCE, 7);
                let events = parser.push(&full[..end]);
                assert_eq!([events.output, parser.finish()].concat(), &full[..end]);
                assert_eq!(events.status, None);
            }
        }
    }

    #[test]
    fn cursor_queries_split_overlap_and_eof() {
        let input = b"a\x1b[\x1b[6nb\x1b[6nc\x1b[6";
        for split in 0..=input.len() {
            let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let a = parser.push(&input[..split]);
            let b = parser.push(&input[split..]);
            assert_eq!(a.cursor_queries + b.cursor_queries, 2);
            assert_eq!([a.output, b.output, parser.finish()].concat(), b"a\x1b[bc\x1b[6");
        }
        let mut parser = FrameParser::new(FramePlatform::Linux, NONCE, 7);
        assert_eq!(parser.push(CURSOR_QUERY).output, CURSOR_QUERY);
    }

    #[test]
    fn wrong_identity_and_missing_line_ending_are_output() {
        let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
        let wrong = format!("__FC_END__:{NONCE}:8:0__\r\n");
        let mut input = wrong.as_bytes().to_vec();
        input.extend_from_slice(format!("__FC_END__:{NONCE}:7:0__").as_bytes());
        let event = parser.push(&input);
        assert_eq!([event.output, parser.finish()].concat(), input);
        assert_eq!(event.status, None);
    }

    #[test]
    fn windows_ambiguous_every_split_and_bytewise() {
        for newline in ["\n", "\r\n"] {
            let mut input = b"prompt> err\r\n".to_vec();
            input.extend(frame(FramePlatform::Windows, "AMBIGUOUS", newline));
            input.extend_from_slice(b"next prompt> ");
            for split in 0..=input.len() {
                let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
                let a = parser.push(&input[..split]);
                let b = parser.push(&input[split..]);
                assert_eq!([a.output, b.output, parser.finish()].concat(), b"prompt> err\r\n");
                assert_eq!([a.remainder, b.remainder].concat(), b"next prompt> ");
                assert_eq!(a.status.or(b.status), Some(FrameStatus::Ambiguous));
            }
            let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let mut output = Vec::new();
            let mut remainder = Vec::new();
            let mut status = None;
            for byte in &input {
                let event = parser.push(std::slice::from_ref(byte));
                output.extend(event.output);
                remainder.extend(event.remainder);
                status = status.or(event.status);
            }
            output.extend(parser.finish());
            assert_eq!(output, b"prompt> err\r\n");
            assert_eq!(remainder, b"next prompt> ");
            assert_eq!(status, Some(FrameStatus::Ambiguous));
        }
    }

    #[test]
    fn windows_ambiguous_eof_at_every_prefix_byte() {
        for newline in ["\n", "\r\n"] {
            let full = frame(FramePlatform::Windows, "AMBIGUOUS", newline);
            for end in 0..full.len() {
                let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
                let events = parser.push(&full[..end]);
                assert_eq!([events.output, parser.finish()].concat(), &full[..end]);
                assert_eq!(events.status, None);
            }
        }
    }

    #[test]
    fn linux_rejects_ambiguous_as_output() {
        for newline in ["\n", "\r\n"] {
            let invalid = frame(FramePlatform::Linux, "AMBIGUOUS", newline);
            let mut input = invalid.clone();
            input.extend(frame(FramePlatform::Linux, "-2147483648", newline));
            for split in 0..=input.len() {
                let mut parser = FrameParser::new(FramePlatform::Linux, NONCE, 7);
                let a = parser.push(&input[..split]);
                let b = parser.push(&input[split..]);
                assert_eq!([a.output, b.output, parser.finish()].concat(), invalid);
                assert_eq!(a.status.or(b.status), Some(FrameStatus::ExitCode(i32::MIN)));
            }
        }
    }

    #[test]
    fn windows_ambiguous_overlaps_with_cursor_queries_and_malformed_frames() {
        let mut expected = Vec::new();
        let mut input = Vec::new();
        let malformed_prefix = format!("__FC_END__:{NONCE}:7:AMBIGU");
        expected.extend_from_slice(malformed_prefix.as_bytes());
        input.extend_from_slice(malformed_prefix.as_bytes());
        input.extend_from_slice(CURSOR_QUERY);
        let malformed_suffix = frame(FramePlatform::Windows, "AMBIGUOUSX", "\r\n");
        expected.extend_from_slice(&malformed_suffix);
        input.extend_from_slice(&malformed_suffix);
        let missing_trailer = format!("__FC_END__:{NONCE}:7:AMBIGUOUS\r\n");
        expected.extend_from_slice(missing_trailer.as_bytes());
        input.extend_from_slice(missing_trailer.as_bytes());
        expected.extend_from_slice(b"\x1b[");
        input.extend_from_slice(b"\x1b[\x1b[6n");
        input.extend(frame(FramePlatform::Windows, "AMBIGUOUS", "\r\n"));
        input.extend_from_slice(b"after\x1b[6");

        for split in 0..=input.len() {
            let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let a = parser.push(&input[..split]);
            let b = parser.push(&input[split..]);
            assert_eq!([a.output, b.output, parser.finish()].concat(), expected);
            assert_eq!(a.cursor_queries + b.cursor_queries, 2);
            assert_eq!(a.status.or(b.status), Some(FrameStatus::Ambiguous));
            assert_eq!([a.remainder, b.remainder].concat(), b"after\x1b[6");
        }
        let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
        let mut output = Vec::new();
        let mut remainder = Vec::new();
        let mut queries = 0;
        let mut status = None;
        for byte in &input {
            let event = parser.push(std::slice::from_ref(byte));
            output.extend(event.output);
            remainder.extend(event.remainder);
            queries += event.cursor_queries;
            status = status.or(event.status);
        }
        output.extend(parser.finish());
        assert_eq!(output, expected);
        assert_eq!(queries, 2);
        assert_eq!(status, Some(FrameStatus::Ambiguous));
        assert_eq!(remainder, b"after\x1b[6");
    }

    #[test]
    fn windows_nested_prefix_before_numeric_min_and_ambiguous() {
        let nested_prefix = format!("__FC_END__:{NONCE}:7:");
        for (code, expected_status) in [
            ("-2147483648", FrameStatus::ExitCode(i32::MIN)),
            ("AMBIGUOUS", FrameStatus::Ambiguous),
        ] {
            let mut input = nested_prefix.as_bytes().to_vec();
            input.extend(frame(FramePlatform::Windows, code, "\r\n"));
            for split in 0..=input.len() {
                let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
                let a = parser.push(&input[..split]);
                let b = parser.push(&input[split..]);
                assert_eq!([a.output, b.output, parser.finish()].concat(), nested_prefix.as_bytes());
                assert_eq!(a.status.or(b.status), Some(expected_status));
            }
        }

        let partial_ambig = format!("__FC_END__:{NONCE}:7:AMBIG");
        let mut input = partial_ambig.as_bytes().to_vec();
        input.extend(frame(FramePlatform::Windows, "AMBIGUOUS", "\n"));
        for split in 0..=input.len() {
            let mut parser = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let a = parser.push(&input[..split]);
            let b = parser.push(&input[split..]);
            assert_eq!([a.output, b.output, parser.finish()].concat(), partial_ambig.as_bytes());
            assert_eq!(a.status.or(b.status), Some(FrameStatus::Ambiguous));
        }
    }

    #[test]
    fn windows_ambiguous_remainder_feeds_next_numeric_min_parser() {
        let mut stream = b"first".to_vec();
        stream.extend(frame(FramePlatform::Windows, "AMBIGUOUS", "\r\n"));
        stream.extend_from_slice(CURSOR_QUERY);
        stream.extend_from_slice(b"second");
        stream.extend(format!("__FC_END__:{NONCE}:8:-2147483648__\r\n").as_bytes());
        stream.extend_from_slice(b"tail");

        for split in 0..=stream.len() {
            let mut first = FrameParser::new(FramePlatform::Windows, NONCE, 7);
            let a = first.push(&stream[..split]);
            let b = first.push(&stream[split..]);
            assert_eq!([a.output, b.output, first.finish()].concat(), b"first");
            assert_eq!(a.status.or(b.status), Some(FrameStatus::Ambiguous));

            let mut second = FrameParser::new(FramePlatform::Windows, NONCE, 8);
            let c = second.push(&a.remainder);
            let d = second.push(&b.remainder);
            assert_eq!([c.output, d.output, second.finish()].concat(), b"second");
            assert_eq!(c.cursor_queries + d.cursor_queries, 1);
            assert_eq!(c.status.or(d.status), Some(FrameStatus::ExitCode(i32::MIN)));
            assert_eq!([c.remainder, d.remainder].concat(), b"tail");
        }
    }
}
