/// TTY output processing — post-processing, buffering, flow control, FIFO drain
///
/// Implements the output side of the POSIX line discipline:
///   - OPOST flag gate for all post-processing
///   - ONLCR  : NL -> CR+NL on output
///   - OCRNL  : CR -> NL on output
///   - ONOCR  : suppress CR at column 0
///   - ONLRET : NL also resets column
///   - XON/XOFF output gate (output_stopped flag)
///   - ANSI / VT100 escape sequence pass-through and state-machine integration
///   - Scrollback buffer management (push, commit, scroll)
///   - Raw (unprocessed) byte emission for use by the emulator
///
/// All functions take a mutable `Tty` reference.
///
/// All code is original.
use alloc::vec::Vec;

use super::{Tty, OCRNL, ONLCR, ONLRET, ONOCR, OPOST, QUEUE_CAPACITY, SCROLLBACK_MAX_LINES};

// ── Public byte/string output ─────────────────────────────────────────────────

/// Write a single byte to the output queue with full POSIX output post-processing.
///
/// Respects the `output_stopped` (XON/XOFF) gate; silently discards bytes
/// when output is paused.
pub fn output_byte(tty: &mut Tty, byte: u8) {
    if tty.output_stopped {
        return;
    }

    if tty.termios.oflag & OPOST != 0 {
        if byte == b'\n' && tty.termios.oflag & ONLCR != 0 {
            // NL -> CR + NL
            enqueue(tty, b'\r');
            enqueue(tty, b'\n');
            tty.column = 0;
        } else if byte == b'\r' && tty.termios.oflag & OCRNL != 0 {
            // CR -> NL
            enqueue(tty, b'\n');
            tty.column = 0;
        } else if byte == b'\r' && tty.termios.oflag & ONOCR != 0 && tty.column == 0 {
            // Suppress CR at column 0
        } else {
            enqueue(tty, byte);
            // Column tracking
            if byte == b'\r' {
                tty.column = 0;
            } else if byte == b'\n' {
                if tty.termios.oflag & ONLRET != 0 {
                    tty.column = 0;
                }
            } else if byte >= 0x20 {
                tty.column = tty.column.saturating_add(1);
            }
        }
    } else {
        // No post-processing
        enqueue(tty, byte);
    }

    tty.bytes_written = tty.bytes_written.saturating_add(1);
}

/// Write each byte of `s` through `output_byte()`.
pub fn output_str(tty: &mut Tty, s: &str) {
    for b in s.bytes() {
        output_byte(tty, b);
    }
}

/// Write a `data` slice to the TTY (called from `Tty::write`).
/// Bytes are routed through `output_byte()` for OPOST processing.
pub fn write(tty: &mut Tty, data: &[u8]) {
    for &byte in data {
        output_byte(tty, byte);
    }
}

/// Write a `data` slice through the full ANSI/VT100 emulator, then queue
/// processed output.  The emulator itself calls back into `raw_output()` and
/// `output_byte()` as appropriate.
pub fn write_ansi(tty: &mut Tty, data: &[u8]) {
    for &byte in data {
        crate::drivers::tty::emulator::process_output_byte(tty, byte);
    }
}

/// Drain and return all pending output bytes (called by the framebuffer/serial
/// driver on each display refresh tick).
pub fn drain_output(tty: &mut Tty) -> Vec<u8> {
    tty.output_queue.drain(..).collect()
}

// ── Raw (unprocessed) output ──────────────────────────────────────────────────

/// Write a byte directly to the output queue without any flag processing.
///
/// Used by the ANSI emulator to re-emit passthrough sequences and by
/// `raw_output` callers that must bypass OPOST entirely.
pub fn raw_output(tty: &mut Tty, byte: u8) {
    enqueue(tty, byte);
    tty.bytes_written = tty.bytes_written.saturating_add(1);
}

/// Emit a complete CSI escape sequence as passthrough bytes in the output queue.
///
/// This lets the display / framebuffer driver consume the sequence directly
/// rather than having the kernel interpret it.
pub fn emit_csi_passthrough(tty: &mut Tty, params: &[u16], intermediate: u8, final_byte: u8) {
    raw_output(tty, 0x1B);
    raw_output(tty, b'[');
    if intermediate != 0 {
        raw_output(tty, intermediate);
    }
    for (i, &val) in params.iter().enumerate() {
        if i > 0 {
            raw_output(tty, b';');
        }
        emit_u16(tty, val);
    }
    raw_output(tty, final_byte);
}

/// Write a `u16` as decimal ASCII digits into the raw output queue.
fn emit_u16(tty: &mut Tty, val: u16) {
    if val >= 10000 {
        raw_output(tty, b'0' + (val / 10000) as u8);
    }
    if val >= 1000 {
        raw_output(tty, b'0' + ((val / 1000) % 10) as u8);
    }
    if val >= 100 {
        raw_output(tty, b'0' + ((val / 100) % 10) as u8);
    }
    if val >= 10 {
        raw_output(tty, b'0' + ((val / 10) % 10) as u8);
    }
    raw_output(tty, b'0' + (val % 10) as u8);
}

// ── BEL / newline / scroll helpers ───────────────────────────────────────────

/// Handle a BEL character (0x07).  Future: trigger sound driver.
pub fn handle_bell(_tty: &mut Tty) {
    // Placeholder: audio driver call would go here.
}

/// Handle a newline: commit the current scrollback line, advance the row,
/// and scroll the display if we are at the bottom.
pub fn handle_newline(tty: &mut Tty) {
    scrollback_commit_line(tty);
    tty.row = tty.row.saturating_add(1);
    if tty.row >= tty.rows as u32 {
        tty.row = (tty.rows as u32).saturating_sub(1);
        scroll_up_one(tty);
    }
}

// ── Scrollback buffer ─────────────────────────────────────────────────────────

/// Push a single byte onto the current (in-progress) scrollback line.
pub fn scrollback_push_byte(tty: &mut Tty, byte: u8) {
    tty.current_line.push(byte);
}

/// Finalise the current scrollback line and start a new one.
///
/// Evicts the oldest line when `SCROLLBACK_MAX_LINES` is exceeded.
pub fn scrollback_commit_line(tty: &mut Tty) {
    let line = core::mem::replace(&mut tty.current_line, Vec::new());
    tty.scrollback.push_back(line);
    while tty.scrollback.len() > SCROLLBACK_MAX_LINES {
        tty.scrollback.pop_front();
    }
}

/// Advance the internal scroll offset by one line (called when the live
/// display scrolls up).  Only moves the offset if the user is already
/// viewing scrollback history.
pub fn scroll_up_one(tty: &mut Tty) {
    if tty.scroll_offset > 0 {
        tty.scroll_offset = tty.scroll_offset.saturating_add(1);
    }
}

/// Scroll the scrollback view upward by `n` lines (toward older history).
pub fn scroll_view_up(tty: &mut Tty, n: usize) {
    let max = tty.scrollback.len();
    tty.scroll_offset = tty.scroll_offset.saturating_add(n).min(max);
}

/// Scroll the scrollback view downward by `n` lines (toward live output).
pub fn scroll_view_down(tty: &mut Tty, n: usize) {
    tty.scroll_offset = tty.scroll_offset.saturating_sub(n);
}

/// Reset the scrollback view to the live (bottom) position.
pub fn scroll_view_reset(tty: &mut Tty) {
    tty.scroll_offset = 0;
}

/// Return up to `rows` scrollback lines visible at the current scroll offset.
pub fn visible_scrollback<'a>(tty: &'a Tty, rows: usize) -> Vec<&'a [u8]> {
    let total = tty.scrollback.len();
    if total == 0 || tty.scroll_offset == 0 {
        return Vec::new();
    }
    let end = total.saturating_sub(tty.scroll_offset);
    let start = end.saturating_sub(rows);
    tty.scrollback
        .iter()
        .skip(start)
        .take(end - start)
        .map(|line| line.as_slice())
        .collect()
}

/// Return the total number of committed scrollback lines.
pub fn scrollback_len(tty: &Tty) -> usize {
    tty.scrollback.len()
}

/// Clear the scrollback buffer and reset the view to live.
pub fn clear_scrollback(tty: &mut Tty) {
    tty.scrollback.clear();
    tty.scroll_offset = 0;
}

// ── Flush helpers ─────────────────────────────────────────────────────────────

/// Discard all pending output bytes.
pub fn flush_output(tty: &mut Tty) {
    tty.output_queue.clear();
}

// ── Internal queue management ─────────────────────────────────────────────────

/// Push one byte onto the output queue, trimming from the front if over capacity.
#[inline]
fn enqueue(tty: &mut Tty, byte: u8) {
    tty.output_queue.push_back(byte);
    trim_output_queue(tty);
}

/// Trim the output queue to `QUEUE_CAPACITY`, dropping the oldest bytes.
fn trim_output_queue(tty: &mut Tty) {
    while tty.output_queue.len() > QUEUE_CAPACITY {
        tty.output_queue.pop_front();
    }
}
