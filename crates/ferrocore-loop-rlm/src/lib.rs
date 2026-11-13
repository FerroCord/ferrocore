//! RLM response parsing and per-turn terminal context.

use async_trait::async_trait;
use ferrocore_types::{AgentLoop, LoopAction, LoopError, Message, Session, TerminalSnapshot};

#[derive(Debug, Default, Clone, Copy)]
pub struct RlmLoop;

const INSTRUCTIONS: &str = "Respond with exactly one action outside Markdown fences. Put <sh> and its closing </sh> on separate, unindented lines around a non-empty shell command, or <final> and </final> on separate, unindented lines around a non-empty final answer. Within shell code, an inline </sh> is literal text and does not close the block.";

fn malformed() -> LoopAction {
    LoopAction::Malformed("Expected exactly one non-empty <sh> or <final> block with unindented tags on separate lines, outside closed Markdown fences.".into())
}

// Container fences cannot be tracked reliably without parsing Markdown containers.
// Reject them rather than mistaking their unindented contents for actions.
fn ambiguous_fence(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut pos = 0;
    let mut container = false;
    loop {
        let start = pos;
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') { pos += 1; }
        if bytes[start..pos].contains(&b'\t') || pos - start > 3 { container = true; }
        if pos < bytes.len() && bytes[pos] == b'>' {
            container = true;
            pos += 1;
            continue;
        }
        let marker = pos;
        if pos < bytes.len() && matches!(bytes[pos], b'-' | b'+' | b'*') {
            pos += 1;
        } else {
            while pos < bytes.len() && bytes[pos].is_ascii_digit() && pos - marker < 9 { pos += 1; }
            if pos == marker || pos >= bytes.len() || !matches!(bytes[pos], b'.' | b')') { pos = marker; }
            else { pos += 1; }
        }
        if pos > marker && pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
            container = true;
            continue;
        }
        pos = marker;
        break;
    }
    container && pos < bytes.len() && matches!(bytes[pos], b'`' | b'~')
        && bytes[pos..].iter().take_while(|&&b| b == bytes[pos]).count() >= 3
}

/// Parse the complete response before allowing any command to execute.
pub fn parse_response(text: &str) -> LoopAction {
    let mut fence: Option<(u8, usize)> = None;
    let mut open: Option<(&str, usize)> = None;
    let mut action = None;
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw).strip_suffix('\r').unwrap_or(raw.strip_suffix('\n').unwrap_or(raw));
        let inline_shell_close = open.is_some_and(|(kind, _)| kind == "sh")
            && !matches!(line, "<sh>" | "</sh>" | "<final>" | "</final>")
            && line.contains("</sh>");
        if (!matches!(line, "<sh>" | "</sh>" | "<final>" | "</final>") || (open.is_some() && fence.is_some()))
            && ["<sh>", "</sh>", "<final>", "</final>"].iter().any(|tag| line.contains(tag) && !(*tag == "</sh>" && inline_shell_close))
        {
            return malformed();
        }
        if fence.is_none() && ambiguous_fence(line) { return malformed(); }
        let trimmed = line.trim_start_matches(' ');
        let indent = line.len() - trimmed.len();
        let bytes = trimmed.as_bytes();
        if indent <= 3 && !bytes.is_empty() && (bytes[0] == b'`' || bytes[0] == b'~') {
            let marker = bytes[0];
            let count = bytes.iter().take_while(|&&b| b == marker).count();
            if count >= 3 {
                match fence {
                    Some((kind, length)) if marker == kind && count >= length && trimmed[count..].trim().is_empty() => fence = None,
                    None if marker != b'`' || !trimmed[count..].contains('`') => fence = Some((marker, count)),
                    _ => {}
                }
                offset += raw.len();
                continue;
            }
        }
        if fence.is_none() {
            match line {
                "<sh>" | "<final>" => {
                    if open.is_some() || action.is_some() { return malformed(); }
                    open = Some((if line == "<sh>" { "sh" } else { "final" }, offset + raw.len()));
                }
                "</sh>" | "</final>" => {
                    let Some((kind, start)) = open.take() else { return malformed(); };
                    if (kind == "sh" && line != "</sh>") || (kind == "final" && line != "</final>") { return malformed(); }
                    let body = text[start..offset].strip_suffix('\n').unwrap_or(&text[start..offset]);
                    let body = body.strip_suffix('\r').unwrap_or(body);
                    if body.trim().is_empty() { return malformed(); }
                    action = Some(if kind == "sh" { LoopAction::ExecShell(body.into()) } else { LoopAction::Final(body.into()) });
                }
                _ => {}
            }
        }
        offset += raw.len();
    }
    if fence.is_some() || open.is_some() { return malformed(); }
    action.unwrap_or_else(malformed)
}

fn snapshot_header(state: &TerminalSnapshot) -> String {
    let pwd = state.pwd.as_deref().unwrap_or("unknown");
    let exit = state.last_exit_code.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into());
    let jobs = state.jobs.as_ref().map(|v| format!("{v:?}")).unwrap_or_else(|| "unknown".into());
    let names = state.scratch_names.as_ref().map(|v| {
        let mut names: Vec<_> = v.iter().collect();
        names.sort();
        names.truncate(20);
        format!("{names:?}")
    }).unwrap_or_else(|| "unknown".into());
    let total = state.scratch_total.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into());
    format!("Terminal snapshot (unknown values are not available):\npwd: {pwd:?}\nlast_exit_code: {exit}\njobs: {jobs}\nscratch_names (first 20 sorted): {names}\nscratch_total: {total}")
}

#[async_trait]
impl AgentLoop for RlmLoop {
    async fn prepare_turn(&self, session: &Session, state: &TerminalSnapshot) -> Result<Vec<Message>, LoopError> {
        let mut messages = Vec::with_capacity(session.history.len() + 2);
        messages.push(Message { role: "system".into(), content: INSTRUCTIONS.into() });
        messages.extend(session.history.iter().cloned());
        messages.push(Message { role: "system".into(), content: snapshot_header(state) });
        Ok(messages)
    }

    async fn on_model_response(&self, text: &str) -> Result<LoopAction, LoopError> {
        Ok(parse_response(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_actions_and_preserves_body() {
        assert_eq!(parse_response("explanation\n<sh>\necho hi\necho bye\n</sh>\n"), LoopAction::ExecShell("echo hi\necho bye".into()));
        assert_eq!(parse_response("<final>\r\ndone\r\n</final>"), LoopAction::Final("done".into()));
        assert_eq!(parse_response("<sh>\necho \"</sh>\"\n</sh>"), LoopAction::ExecShell("echo \"</sh>\"".into()));
    }

    #[test]
    fn rejects_incomplete_ambiguous_or_multiple_actions() {
        for text in ["", "<sh>\n", "<sh>\n  \n</sh>", "<final>\n\n</final>", " <sh>\nx\n</sh>", "<sh>\nx</sh>", "<sh>\nx\n</sh>\n<final>\ny\n</final>", "<sh>\n<final>\nx\n</final>\n</sh>", "</sh>"] {
            assert!(matches!(parse_response(text), LoopAction::Malformed(_)), "{text:?}");
        }
    }

    #[test]
    fn rejects_embedded_tags_inside_and_outside_actions() {
        for tag in ["<sh>", "</sh>", "<final>", "</final>"] {
            for text in [
                format!("prefix {tag} suffix\n<sh>\necho unsafe\n</sh>"),
                format!("<sh>\necho '{tag}'\n</sh>"),
                format!("<final>\ntext {tag} text\n</final>"),
                format!("<sh>\necho unsafe\n</sh>\nsuffix {tag}"),
            ] {
                if tag == "</sh>" && text.starts_with("<sh>\n") && text.contains("echo '</sh>'") {
                    assert_eq!(parse_response(&text), LoopAction::ExecShell("echo '</sh>'".into()));
                } else {
                    assert!(matches!(parse_response(&text), LoopAction::Malformed(_)), "{text:?}");
                }
            }
        }
    }

    #[test]
    fn fences_exclude_examples_and_unclosed_fences_fail_closed() {
        assert!(matches!(parse_response("```text\n<sh>\necho hi\n</sh>\n```"), LoopAction::Malformed(_)));
        assert_eq!(parse_response("~~~text\n<sh>\nx\n</sh>\n~~~\n<final>\nok\n</final>"), LoopAction::Final("ok".into()));
        assert!(matches!(parse_response("<sh>\necho hi\n</sh>\n```"), LoopAction::Malformed(_)));
    }

    #[test]
    fn container_fences_fail_closed() {
        for text in [
            "> ```\n<sh>\necho unsafe\n</sh>\n> ```",
            "- ```\n<sh>\necho unsafe\n</sh>\n  ```",
            "1. ~~~\n<final>\nunsafe\n</final>\n   ~~~",
            "> - ```text\n<sh>\necho unsafe\n</sh>\n>   ```",
            "- item\n    ```\n<sh>\necho unsafe\n</sh>\n    ```",
            "\t~~~\n<sh>\necho unsafe\n</sh>\n\t~~~",
            "<final>\nok\n</final>\n> ```\nexample\n> ```",
        ] {
            assert!(matches!(parse_response(text), LoopAction::Malformed(_)), "{text:?}");
        }
        assert_eq!(parse_response("```text\n> ```\n```\n<final>\nok\n</final>"), LoopAction::Final("ok".into()));
    }

    #[test]
    fn snapshot_distinguishes_unknown_and_limits_sorted_names() {
        let unknown = snapshot_header(&TerminalSnapshot::default());
        assert!(unknown.contains("scratch_total: unknown"));
        assert!(unknown.contains("jobs: unknown"));
        let state = TerminalSnapshot { scratch_names: Some((0..25).rev().map(|n| format!("entry{n:02}")).collect()), scratch_total: Some(25), jobs: Some(vec![]), ..Default::default() };
        let header = snapshot_header(&state);
        assert!(header.contains("entry00"));
        assert!(header.contains("entry19"));
        assert!(!header.contains("entry20"));
        assert!(header.contains("scratch_total: 25"));
        assert!(header.contains("jobs: []"));
    }
}
