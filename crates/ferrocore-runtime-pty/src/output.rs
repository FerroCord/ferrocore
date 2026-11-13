//! Streaming command output capture and bounded model-context view.
//!
//! Raw command output is streamed to an exclusive temporary file in `$FC_SCRATCH`
//! and published to `$FC_SCRATCH/last_output.log` only after the command
//! completes and the file is flushed and synced.
//!
//! Disclosures:
//! - The configured view byte limit (default `16_384` bytes) caps rendered UTF-8
//!   bytes and is **not** a model token guarantee.
//! - Raw output logs (`last_output.log`) are **not** redacted and may contain
//!   secrets; they must be protected separately.
//! - On Windows, replacing `last_output.log` via `std::fs::rename` under
//!   concurrent readers is not guaranteed atomic, and preserving an existing
//!   `last_output.log` on publish failure is not guaranteed across all states.
//! - Truncation, omission, and lossy UTF-8 notices require up to
//!   `MIN_VIEW_BYTES_FOR_NOTICE` (`192`) bytes; `PtyRuntime::new` enforces this
//!   minimum up front so commands cannot fail post-execution due to an undersized
//!   view limit. `OutputCapture::new` still permits smaller limits for raw
//!   captures that do not emit a notice and fails explicitly in `finish()` if
//!   a notice is required.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const PRUNE_BYTE_THRESHOLD: u64 = 10_240;
pub const PRUNE_LINE_THRESHOLD: u64 = 150;
pub const DEFAULT_VIEW_BYTES: usize = 16_384;
/// Minimum `view_limit` guaranteed to fit the longest possible wrapped notice.
pub const MIN_VIEW_BYTES_FOR_NOTICE: usize = 192;
const HEAD_LINES: usize = 50;
const TAIL_LINES: usize = 50;

static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputView {
    pub text: String,
    pub raw_bytes: u64,
    pub lines: u64,
    pub pruned: bool,
    pub invalid_utf8: bool,
}

pub struct OutputCapture {
    scratch_dir: PathBuf,
    tmp_path: PathBuf,
    writer: Option<BufWriter<File>>,
    write_failed: bool,
    utf8_pending: [u8; 4],
    utf8_pending_len: usize,
    invalid_utf8: bool,
    view_limit: usize,
    raw_bytes: u64,
    lf_count: u64,
    last_byte: Option<u8>,
    line_50_end: Option<u64>,
    tail_line_starts: VecDeque<u64>,
    completed: bool,
}

impl OutputCapture {
    pub fn new(scratch: &Path, view_limit: usize) -> io::Result<Self> {
        if view_limit == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output view limit must be greater than zero",
            ));
        }
        let pid = std::process::id();
        for _ in 0..128 {
            let seq = NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed);
            let tmp_path = scratch.join(format!(".last_output.{pid}.{seq}.tmp"));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&tmp_path) {
                Ok(file) => {
                    let mut tail_line_starts = VecDeque::with_capacity(TAIL_LINES + 2);
                    tail_line_starts.push_back(0);
                    return Ok(Self {
                        scratch_dir: scratch.to_path_buf(),
                        tmp_path,
                        writer: Some(BufWriter::new(file)),
                        write_failed: false,
                        utf8_pending: [0; 4],
                        utf8_pending_len: 0,
                        invalid_utf8: false,
                        view_limit,
                        raw_bytes: 0,
                        lf_count: 0,
                        last_byte: None,
                        line_50_end: None,
                        tail_line_starts,
                        completed: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "temporary output file collision limit reached",
        ))
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.write_failed {
            return Err(io::Error::new(io::ErrorKind::Other, "output capture is poisoned by an earlier write failure"));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        let writer = self.writer.as_mut().expect("writer active before finish");
        if let Err(error) = writer.write_all(bytes) {
            self.write_failed = true;
            return Err(error);
        }
        self.validate_utf8(bytes);
        for (idx, &byte) in bytes.iter().enumerate() {
            let offset = self.raw_bytes + idx as u64;
            if byte == b'\n' {
                self.lf_count += 1;
                if self.lf_count == HEAD_LINES as u64 {
                    self.line_50_end = Some(offset + 1);
                }
                if self.tail_line_starts.len() == TAIL_LINES + 2 {
                    self.tail_line_starts.pop_front();
                }
                self.tail_line_starts.push_back(offset + 1);
            }
        }
        self.raw_bytes += bytes.len() as u64;
        self.last_byte = Some(*bytes.last().expect("non-empty"));
        Ok(())
    }

    fn validate_utf8(&mut self, mut bytes: &[u8]) {
        if self.invalid_utf8 { return; }
        while self.utf8_pending_len > 0 && !bytes.is_empty() {
            self.utf8_pending[self.utf8_pending_len] = bytes[0];
            self.utf8_pending_len += 1;
            bytes = &bytes[1..];
            match std::str::from_utf8(&self.utf8_pending[..self.utf8_pending_len]) {
                Ok(_) => self.utf8_pending_len = 0,
                Err(error) if error.error_len().is_some() => {
                    self.invalid_utf8 = true;
                    return;
                }
                Err(_) => (),
            }
        }
        if self.utf8_pending_len == 0 {
            if let Err(error) = std::str::from_utf8(bytes) {
                if error.error_len().is_some() {
                    self.invalid_utf8 = true;
                } else {
                    let pending = &bytes[error.valid_up_to()..];
                    self.utf8_pending[..pending.len()].copy_from_slice(pending);
                    self.utf8_pending_len = pending.len();
                }
            }
        }
    }

    pub fn finish(mut self) -> io::Result<OutputView> {
        if self.write_failed {
            return Err(io::Error::new(io::ErrorKind::Other, "output capture is poisoned by an earlier write failure"));
        }
        self.invalid_utf8 |= self.utf8_pending_len != 0;
        let mut writer = self.writer.take().expect("writer active before finish");
        writer.flush()?;
        let mut file = writer.into_inner().map_err(io::IntoInnerError::into_error)?;
        file.sync_data()?;

        let lines = line_count(self.raw_bytes, self.lf_count, self.last_byte);
        let pruned = self.raw_bytes > PRUNE_BYTE_THRESHOLD || lines > PRUNE_LINE_THRESHOLD;
        let view = self.build_view(&mut file, lines, pruned)?;
        drop(file);

        let dest = self.scratch_dir.join("last_output.log");
        if let Err(error) = fs::rename(&self.tmp_path, &dest) {
            let _ = fs::remove_file(&self.tmp_path);
            return Err(io::Error::new(
                error.kind(),
                format!("failed to publish last_output.log: {error}"),
            ));
        }
        self.completed = true;
        Ok(view)
    }

    fn build_view(&mut self, file: &mut File, lines: u64, pruned: bool) -> io::Result<OutputView> {
        if self.raw_bytes == 0 {
            return Ok(OutputView {
                text: String::new(),
                raw_bytes: 0,
                lines: 0,
                pruned: false,
                invalid_utf8: false,
            });
        }

        if self.last_byte == Some(b'\n') && self.tail_line_starts.back() == Some(&self.raw_bytes) {
            self.tail_line_starts.pop_back();
        }

        if !pruned || lines <= (HEAD_LINES + TAIL_LINES) as u64 {
            return self.build_unsplit_view(file, lines, pruned);
        }

        let head_end = self.line_50_end.unwrap_or(self.raw_bytes);
        let tail_start = self
            .tail_line_starts
            .len()
            .checked_sub(TAIL_LINES)
            .and_then(|idx| self.tail_line_starts.get(idx).copied())
            .unwrap_or(self.raw_bytes);
        let omitted_lines = lines - (HEAD_LINES + TAIL_LINES) as u64;
        let omitted_bytes = tail_start.saturating_sub(head_end);

        let head_cap = (self.view_limit as u64).saturating_add(4);
        let head_bytes = read_head_slice(file, head_end.min(head_cap), head_end > head_cap)?;
        let head_truncated = head_end > head_cap;

        let tail_len = self.raw_bytes.saturating_sub(tail_start);
        let tail_read_start = if tail_len > head_cap {
            self.raw_bytes - head_cap
        } else {
            tail_start
        };
        let tail_bytes = read_tail_slice(file, tail_read_start, self.raw_bytes, tail_read_start > tail_start)?;
        let tail_truncated = tail_read_start > tail_start;

        let head_text = String::from_utf8_lossy(&head_bytes).into_owned();
        let tail_text = String::from_utf8_lossy(&tail_bytes).into_owned();
        let raw_invalid = self.invalid_utf8;

        if !head_truncated && !tail_truncated {
            let notice = format_omission_notice(omitted_lines, omitted_bytes, raw_invalid);
            let sep_head = usize::from(!head_text.is_empty() && !head_text.ends_with('\n'));
            let sep_tail = usize::from(!tail_text.is_empty() && !notice.ends_with('\n'));
            let total = head_text
                .len()
                .saturating_add(sep_head)
                .saturating_add(notice.len())
                .saturating_add(sep_tail)
                .saturating_add(tail_text.len());
            if total <= self.view_limit {
                let mut text = String::with_capacity(total);
                text.push_str(&head_text);
                if sep_head == 1 {
                    text.push('\n');
                }
                text.push_str(&notice);
                if sep_tail == 1 {
                    text.push('\n');
                }
                text.push_str(&tail_text);
                return Ok(OutputView {
                    text,
                    raw_bytes: self.raw_bytes,
                    lines,
                    pruned,
                    invalid_utf8: raw_invalid,
                });
            }
        }

        let text = fit_view(
            &head_text,
            &tail_text,
            self.view_limit,
            &format_omission_notice(omitted_lines, omitted_bytes, raw_invalid),
        )?;
        Ok(OutputView {
            text,
            raw_bytes: self.raw_bytes,
            lines,
            pruned,
            invalid_utf8: raw_invalid,
        })
    }

    fn build_unsplit_view(
        &self,
        file: &mut File,
        lines: u64,
        pruned: bool,
    ) -> io::Result<OutputView> {
        let cap = (self.view_limit as u64).saturating_add(4);
        if self.raw_bytes <= cap {
            let bytes = read_range(file, 0, self.raw_bytes)?;
            let raw_text = String::from_utf8_lossy(&bytes).into_owned();
            let raw_invalid = self.invalid_utf8;
            let utf8_note = if raw_invalid {
                "[... ferrocore: invalid UTF-8 replaced lossily in view; raw bytes preserved in $FC_SCRATCH/last_output.log ...]\n"
            } else {
                ""
            };
            if utf8_note.len().saturating_add(raw_text.len()) <= self.view_limit {
                let mut text = String::with_capacity(utf8_note.len() + raw_text.len());
                text.push_str(utf8_note);
                text.push_str(&raw_text);
                return Ok(OutputView {
                    text,
                    raw_bytes: self.raw_bytes,
                    lines,
                    pruned,
                    invalid_utf8: raw_invalid,
                });
            }
            let text = fit_view(
                &raw_text,
                &raw_text,
                self.view_limit,
                &format_truncation_notice(self.raw_bytes, raw_invalid),
            )?;
            return Ok(OutputView {
                text,
                raw_bytes: self.raw_bytes,
                lines,
                pruned,
                invalid_utf8: raw_invalid,
            });
        }

        let head_bytes = read_head_slice(file, cap, true)?;
        let tail_bytes = read_tail_slice(file, self.raw_bytes - cap, self.raw_bytes, true)?;
        let head_text = String::from_utf8_lossy(&head_bytes).into_owned();
        let tail_text = String::from_utf8_lossy(&tail_bytes).into_owned();
        let text = fit_view(
            &head_text,
            &tail_text,
            self.view_limit,
            &format_truncation_notice(self.raw_bytes, self.invalid_utf8),
        )?;
        Ok(OutputView {
            text,
            raw_bytes: self.raw_bytes,
            lines,
            pruned,
            invalid_utf8: self.invalid_utf8,
        })
    }
}

impl Drop for OutputCapture {
    fn drop(&mut self) {
        self.writer.take();
        if !self.completed {
            let _ = fs::remove_file(&self.tmp_path);
        }
    }
}

fn line_count(raw_bytes: u64, lf_count: u64, last_byte: Option<u8>) -> u64 {
    if raw_bytes == 0 {
        0
    } else if last_byte == Some(b'\n') {
        lf_count
    } else {
        lf_count + 1
    }
}

fn read_range(file: &mut File, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let len = usize::try_from(len).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "slice length exceeds usize"))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Reads up to `max_len` bytes starting at `offset` (stopping at EOF).
fn read_up_to(file: &mut File, offset: u64, max_len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity(max_len);
    let mut chunk = [0u8; 4];
    while buf.len() < max_len {
        let to_read = (max_len - buf.len()).min(chunk.len());
        match file.read(&mut chunk[..to_read]) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(buf)
}

fn utf8_char_width(b: u8) -> usize {
    if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

/// Reads `len` bytes from offset 0, trimming an incomplete trailing UTF-8
/// sequence only if it belongs to a valid multi-byte character split at `len`.
fn read_head_slice(file: &mut File, len: u64, capped: bool) -> io::Result<Vec<u8>> {
    let mut buf = read_range(file, 0, len)?;
    if capped {
        // Strip at most 3 bytes only if they start a valid multi-byte UTF-8 char
        // that continues past `len`.
        for back in 1..=3.min(buf.len()) {
            let start = buf.len() - back;
            let b = buf[start];
            if b & 0xC0 == 0x80 {
                continue;
            }
            let expected = utf8_char_width(b);
            if back < expected {
                let needed = expected - back;
                let lookahead = read_up_to(file, len, needed)?;
                if lookahead.len() == needed {
                    let mut seq = [0u8; 4];
                    seq[..back].copy_from_slice(&buf[start..]);
                    seq[back..expected].copy_from_slice(&lookahead);
                    if std::str::from_utf8(&seq[..expected]).is_ok() {
                        buf.truncate(start);
                    }
                }
            }
            break;
        }
    }
    Ok(buf)
}

/// Reads `[start..end)`, skipping leading UTF-8 continuation bytes only if they
/// belong to a valid multi-byte character split at `start`.
fn read_tail_slice(
    file: &mut File,
    start: u64,
    end: u64,
    capped_mid_stream: bool,
) -> io::Result<Vec<u8>> {
    if !capped_mid_stream || start == 0 {
        return read_range(file, start, end - start);
    }
    let lookback = start.min(3);
    let buf = read_range(file, start - lookback, end - (start - lookback))?;
    let lb = lookback as usize;
    // Check if a valid multi-byte UTF-8 sequence started in the lookback window
    // and crossed `start`.
    for back in 1..=lb {
        let lead_idx = lb - back;
        let b = buf[lead_idx];
        if b & 0xC0 == 0x80 {
            continue;
        }
        let expected = utf8_char_width(b);
        if back < expected && lead_idx + expected <= buf.len() {
            if std::str::from_utf8(&buf[lead_idx..lead_idx + expected]).is_ok() {
                return Ok(buf[lead_idx + expected..].to_vec());
            }
        }
        break;
    }
    Ok(buf[lb..].to_vec())
}

fn format_omission_notice(omitted_lines: u64, omitted_bytes: u64, invalid_utf8: bool) -> String {
    if invalid_utf8 {
        format!(
            "[... ferrocore: {omitted_lines} lines ({omitted_bytes} bytes) omitted; invalid UTF-8 replaced lossily; full output: $FC_SCRATCH/last_output.log ...]"
        )
    } else {
        format!(
            "[... ferrocore: {omitted_lines} lines ({omitted_bytes} bytes) omitted; full output: $FC_SCRATCH/last_output.log ...]"
        )
    }
}

fn format_truncation_notice(raw_bytes: u64, invalid_utf8: bool) -> String {
    if invalid_utf8 {
        format!(
            "[... ferrocore: output truncated ({raw_bytes} raw bytes); invalid UTF-8 replaced lossily; full output: $FC_SCRATCH/last_output.log ...]"
        )
    } else {
        format!(
            "[... ferrocore: output truncated ({raw_bytes} raw bytes); full output: $FC_SCRATCH/last_output.log ...]"
        )
    }
}

fn fit_view(
    head_src: &str,
    tail_src: &str,
    view_limit: usize,
    notice: &str,
) -> io::Result<String> {
    if notice.len() > view_limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "output view limit ({view_limit} bytes) is too small for the required notice ({} bytes)",
                notice.len()
            ),
        ));
    }
    let (head, tail, notice_block) = allocate_head_tail(head_src, tail_src, notice, view_limit);
    let mut out = String::with_capacity(head.len() + notice_block.len() + tail.len());
    out.push_str(head);
    out.push_str(&notice_block);
    out.push_str(tail);
    Ok(out)
}

fn allocate_head_tail<'a>(
    head_src: &'a str,
    tail_src: &'a str,
    notice: &str,
    view_limit: usize,
) -> (&'a str, &'a str, String) {
    let notice_block = if notice.len().saturating_add(2) <= view_limit {
        format!("\n{notice}\n")
    } else {
        notice.to_string()
    };
    let budget = view_limit.saturating_sub(notice_block.len());
    let half = budget / 2;
    let head_budget = if tail_src.len() < budget - half {
        budget - tail_src.len()
    } else if head_src.len() < half {
        head_src.len()
    } else {
        half
    };
    let tail_budget = budget - head_budget;
    let head = truncate_str_prefix(head_src, head_budget);
    let tail = truncate_str_suffix(tail_src, tail_budget);
    (head, tail, notice_block)
}

fn truncate_str_prefix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut idx = max_bytes;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    &s[..idx]
}

fn truncate_str_suffix(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut idx = s.len() - max_bytes;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    &s[idx..]
}

#[cfg(test)]
impl OutputCapture {
    pub(crate) fn poison_for_test(&mut self) {
        self.write_failed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_view_bytes_fits_worst_case_notices() {
        let omission = format_omission_notice(u64::MAX, u64::MAX, true);
        let truncation = format_truncation_notice(u64::MAX, true);
        let utf8_note = "[... ferrocore: invalid UTF-8 replaced lossily in view; raw bytes preserved in $FC_SCRATCH/last_output.log ...]\n";
        assert!(omission.len() + 2 <= MIN_VIEW_BYTES_FOR_NOTICE, "omission={}", omission.len() + 2);
        assert!(truncation.len() + 2 <= MIN_VIEW_BYTES_FOR_NOTICE, "truncation={}", truncation.len() + 2);
        assert!(utf8_note.len() <= MIN_VIEW_BYTES_FOR_NOTICE, "utf8_note={}", utf8_note.len());
    }
}
