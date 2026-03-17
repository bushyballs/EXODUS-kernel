/// TTY line discipline — keyboard input and canonical mode processing
///
/// Handles the input side of the POSIX line discipline:
///   - Input flag transformations (ICRNL, IGNCR, INLCR, ISTRIP, IXON/XOFF)
///   - Signal generation (SIGINT on Ctrl+C, SIGQUIT on Ctrl+\, SIGTSTP on Ctrl+Z)
///   - Canonical mode (line buffering, erase, kill, word-erase, reprint)
///   - Raw / CBreak pass-through
///   - Echo (ECHO, ECHOE, ECHOK, ECHONL, ECHOCTL)
///
/// All functions take a mutable `Tty` reference; no state lives in this module.
///
/// All code is original.
use alloc::vec::Vec;

use super::{
    Tty, TtyMode, ECHO, ECHOCTL, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, IEXTEN, IGNCR, INLCR,
    INPUT_BUF_CAPACITY, ISIG, ISTRIP, IXANY, IXON, NOFLSH, QUEUE_CAPACITY,
};

// ── Public entry-point called from tty::input / Tty::input_char ───────────────

/// Feed one raw keyboard byte through the full line discipline.
///
/// In Raw mode  : byte goes directly to the read queue.
/// In CBreak    : signal processing + flow-control, then read queue.
/// In Cooked    : full canonical processing (echo, erase, kill, newline).
pub fn input_char(tty: &mut Tty, byte: u8) {
    let ch = process_input_flags(tty, byte);
    if ch == 0xFF {
        return;
    } // character was consumed / ignored

    match tty.termios.mode {
        TtyMode::Raw => {
            tty.read_queue.push_back(ch);
            trim_read_queue(tty);
        }
        TtyMode::CBreak => {
            if tty.termios.isig && check_signal(tty, ch) {
                return;
            }
            if check_flow_control(tty, ch) {
                return;
            }
            if tty.termios.echo {
                echo_char(tty, ch);
            }
            tty.read_queue.push_back(ch);
            trim_read_queue(tty);
        }
        TtyMode::Cooked => {
            process_cooked(tty, ch);
        }
    }
}

// ── Input flag transforms ──────────────────────────────────────────────────────

/// Apply POSIX input flags (ISTRIP, IGNCR, ICRNL, INLCR).
///
/// Returns the (possibly transformed) byte, or `0xFF` to discard the byte.
fn process_input_flags(tty: &Tty, ch: u8) -> u8 {
    let mut c = ch;

    if tty.termios.iflag & ISTRIP != 0 {
        c &= 0x7F;
    }

    if c == b'\r' {
        if tty.termios.iflag & IGNCR != 0 {
            return 0xFF; // discard CR entirely
        }
        if tty.termios.iflag & ICRNL != 0 {
            c = b'\n'; // CR -> NL
        }
    }

    if c == b'\n' && tty.termios.iflag & INLCR != 0 {
        c = b'\r'; // NL -> CR
    }

    c
}

// ── Signal generation ──────────────────────────────────────────────────────────

/// Check whether `ch` is a signal-generating control character.
///
/// Returns `true` if the character was consumed (signal delivered).
fn check_signal(tty: &Tty, ch: u8) -> bool {
    let cc = &tty.termios.cc;

    if ch == cc.vintr {
        signal_fg(tty, crate::process::pcb::signal::SIGINT);
        return true;
    }
    if ch == cc.vquit {
        signal_fg(tty, crate::process::pcb::signal::SIGQUIT);
        return true;
    }
    if ch == cc.vsusp {
        signal_fg(tty, crate::process::pcb::signal::SIGTSTP);
        return true;
    }
    false
}

/// Deliver `signal` to the foreground process group.
fn signal_fg(tty: &Tty, signal: u8) {
    if tty.fg_pgid > 0 {
        let _ = crate::process::send_signal(tty.fg_pgid, signal);
    }
}

// ── XON/XOFF flow control ─────────────────────────────────────────────────────

/// Check for XON (Ctrl+Q) / XOFF (Ctrl+S) flow control bytes.
///
/// Returns `true` if the character was consumed.
pub fn check_flow_control(tty: &mut Tty, ch: u8) -> bool {
    if tty.termios.iflag & IXON == 0 {
        return false;
    }

    let cc = tty.termios.cc;
    if ch == cc.vstop {
        tty.output_stopped = true;
        return true;
    }
    if ch == cc.vstart {
        tty.output_stopped = false;
        return true;
    }
    // IXANY: any input character restarts output (but is not consumed)
    if tty.output_stopped && tty.termios.iflag & IXANY != 0 {
        tty.output_stopped = false;
    }
    false
}

// ── Canonical (cooked) mode processing ────────────────────────────────────────

/// Full canonical-mode character handler.
///
/// Processes signals, flow control, line-editing keys (erase, kill, word-erase,
/// reprint, literal-next, EOF), newline flushing, tab, and regular printable
/// characters.  Echo is written through `output::output_byte()`.
fn process_cooked(tty: &mut Tty, ch: u8) {
    let cc = tty.termios.cc;

    // ── Signal-generating characters ──────────────────────────────────────
    if tty.termios.isig {
        if ch == cc.vintr {
            signal_fg(tty, crate::process::pcb::signal::SIGINT);
            if tty.termios.echo {
                echo_ctrl(tty, b'C');
                super::output::output_byte(tty, b'\n');
            }
            if tty.termios.lflag & NOFLSH == 0 {
                tty.input_buf.clear();
                tty.read_queue.clear();
            }
            return;
        }
        if ch == cc.vquit {
            signal_fg(tty, crate::process::pcb::signal::SIGQUIT);
            if tty.termios.echo {
                echo_ctrl(tty, b'\\');
                super::output::output_byte(tty, b'\n');
            }
            if tty.termios.lflag & NOFLSH == 0 {
                tty.input_buf.clear();
                tty.read_queue.clear();
            }
            return;
        }
        if ch == cc.vsusp {
            signal_fg(tty, crate::process::pcb::signal::SIGTSTP);
            if tty.termios.echo {
                echo_ctrl(tty, b'Z');
                super::output::output_byte(tty, b'\n');
            }
            return;
        }
    }

    // ── XON/XOFF ──────────────────────────────────────────────────────────
    if check_flow_control(tty, ch) {
        return;
    }

    // ── Literal next (Ctrl+V) ─────────────────────────────────────────────
    if ch == cc.vlnext && tty.termios.lflag & IEXTEN != 0 {
        if tty.termios.echo && tty.termios.lflag & ECHOCTL != 0 {
            echo_ctrl(tty, b'V');
        }
        return;
    }

    // ── Reprint line (Ctrl+R) ─────────────────────────────────────────────
    if ch == cc.vreprint && tty.termios.lflag & IEXTEN != 0 {
        if tty.termios.echo {
            echo_ctrl(tty, b'R');
            super::output::output_byte(tty, b'\n');
            let buf_copy: Vec<u8> = tty.input_buf.bytes().collect();
            for b in buf_copy {
                super::output::output_byte(tty, b);
            }
        }
        return;
    }

    // ── EOF (Ctrl+D) ──────────────────────────────────────────────────────
    if ch == cc.veof {
        for b in tty.input_buf.bytes() {
            tty.read_queue.push_back(b);
        }
        tty.input_buf.clear();
        return;
    }

    // ── Erase character (Backspace / DEL) ─────────────────────────────────
    if ch == cc.verase || ch == 0x08 {
        if !tty.input_buf.is_empty() {
            tty.input_buf.pop();
            if tty.termios.echo && tty.termios.lflag & ECHOE != 0 {
                super::output::output_str(tty, "\x08 \x08");
            }
        }
        return;
    }

    // ── Kill line (Ctrl+U) ────────────────────────────────────────────────
    if ch == cc.vkill {
        if tty.termios.echo {
            if tty.termios.lflag & ECHOE != 0 {
                for _ in 0..tty.input_buf.len() {
                    super::output::output_str(tty, "\x08 \x08");
                }
            } else if tty.termios.lflag & ECHOK != 0 {
                super::output::output_byte(tty, b'\n');
            }
        }
        tty.input_buf.clear();
        return;
    }

    // ── Word erase (Ctrl+W) ───────────────────────────────────────────────
    if ch == cc.vwerase && tty.termios.lflag & IEXTEN != 0 {
        while tty.input_buf.ends_with(' ') {
            tty.input_buf.pop();
            if tty.termios.echo && tty.termios.lflag & ECHOE != 0 {
                super::output::output_str(tty, "\x08 \x08");
            }
        }
        while !tty.input_buf.is_empty() && !tty.input_buf.ends_with(' ') {
            tty.input_buf.pop();
            if tty.termios.echo && tty.termios.lflag & ECHOE != 0 {
                super::output::output_str(tty, "\x08 \x08");
            }
        }
        return;
    }

    // ── End-of-line (LF or CR) — flush canonical buffer ───────────────────
    if ch == b'\n' || ch == b'\r' {
        if tty.termios.echo || tty.termios.lflag & ECHONL != 0 {
            super::output::output_byte(tty, b'\n');
        }
        tty.input_buf.push('\n');
        for b in tty.input_buf.bytes() {
            tty.read_queue.push_back(b);
        }
        tty.input_buf.clear();
        tty.column = 0;
        return;
    }

    // ── Horizontal tab ────────────────────────────────────────────────────
    if ch == b'\t' {
        tty.input_buf.push('\t');
        if tty.termios.echo {
            let spaces = 8u32.saturating_sub(tty.column % 8);
            for _ in 0..spaces {
                super::output::output_byte(tty, b' ');
            }
            tty.column = tty.column.saturating_add(spaces);
        }
        return;
    }

    // ── Printable character ───────────────────────────────────────────────
    if ch >= 0x20 {
        if tty.input_buf.len() < INPUT_BUF_CAPACITY {
            tty.input_buf.push(ch as char);
            if tty.termios.echo {
                super::output::output_byte(tty, ch);
                tty.column = tty.column.saturating_add(1);
            }
        }
        // silently discard when the line buffer is full
    } else {
        // Non-printable control character
        if tty.termios.echo && tty.termios.lflag & ECHOCTL != 0 {
            echo_ctrl(tty, ch.wrapping_add(b'@'));
        }
        if tty.input_buf.len() < INPUT_BUF_CAPACITY {
            tty.input_buf.push(ch as char);
        }
    }
}

// ── Echo helpers ──────────────────────────────────────────────────────────────

/// Echo a control character as the two-byte sequence `^X`.
pub fn echo_ctrl(tty: &mut Tty, ch: u8) {
    super::output::output_byte(tty, b'^');
    super::output::output_byte(tty, ch);
    tty.column = tty.column.saturating_add(2);
}

/// Echo a single character, applying ECHOCTL for non-printable bytes.
pub fn echo_char(tty: &mut Tty, ch: u8) {
    if ch < 0x20 && ch != b'\n' && ch != b'\r' && ch != b'\t' {
        if tty.termios.lflag & ECHOCTL != 0 {
            echo_ctrl(tty, ch.wrapping_add(b'@'));
        }
    } else {
        super::output::output_byte(tty, ch);
    }
}

// ── Read-queue helpers ────────────────────────────────────────────────────────

/// Read bytes out of the TTY's read queue into `buf`.
///
/// In canonical mode, reading stops at the first newline (inclusive).
/// Returns the number of bytes placed into `buf`.
pub fn tty_read(tty: &mut Tty, buf: &mut [u8]) -> usize {
    let mut count = 0;
    while count < buf.len() {
        if let Some(byte) = tty.read_queue.pop_front() {
            buf[count] = byte;
            count = count.saturating_add(1);
            tty.bytes_read = tty.bytes_read.saturating_add(1);
            if tty.termios.icanon && byte == b'\n' {
                break; // deliver exactly one line at a time
            }
        } else {
            break;
        }
    }
    count
}

/// Returns `true` when data is available for a `read()` call.
///
/// In canonical mode, data is only "available" once a complete line
/// (terminated by `\n`) is in the read queue.
pub fn can_read(tty: &Tty) -> bool {
    if tty.termios.icanon {
        tty.read_queue.iter().any(|&b| b == b'\n')
    } else {
        !tty.read_queue.is_empty()
    }
}

/// Trim the read queue to `QUEUE_CAPACITY`, dropping the oldest bytes.
fn trim_read_queue(tty: &mut Tty) {
    while tty.read_queue.len() > QUEUE_CAPACITY {
        tty.read_queue.pop_front();
    }
}
