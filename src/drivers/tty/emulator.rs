/// VT100/ANSI escape sequence emulator for Genesis TTY
///
/// Implements the ANSI/VT100 state machine: Normal -> Escape -> CSI/OSC ->
/// sequence dispatch. Handles SGR (colors/attributes), cursor movement, erase,
/// scrolling, and DEC private modes.
///
/// This module is stateless: all state lives in the `Tty` struct fields
/// (esc_state, esc_params, attr, saved_cursor, …). Functions take a mutable
/// `Tty` reference and mutate those fields.
///
/// All code is original.
use alloc::vec::Vec;

use super::{CellAttr, EscState, Tty, ESC_PARAM_MAX, OCRNL, ONLCR, ONLRET, ONOCR, OPOST};

// ── Public entry-point called from Tty::output_byte ───────────────────────────

/// Feed one raw byte through the escape-sequence state machine.
///
/// Returns the "cooked" byte to actually emit to the hardware framebuffer
/// (or `None` if the byte was consumed as part of a sequence).
pub fn process_output_byte(tty: &mut Tty, byte: u8) -> Option<u8> {
    match tty.esc_state {
        EscState::Normal => handle_normal(tty, byte),
        EscState::Escape => handle_escape(tty, byte),
        EscState::Csi => handle_csi(tty, byte),
        EscState::Osc => handle_osc(tty, byte),
        EscState::SingleShift => {
            tty.esc_state = EscState::Normal;
            Some(byte) // pass through the shifted character
        }
    }
}

// ── State: Normal ─────────────────────────────────────────────────────────────

fn handle_normal(tty: &mut Tty, byte: u8) -> Option<u8> {
    match byte {
        0x1B => {
            // ESC — enter escape sequence
            tty.esc_state = EscState::Escape;
            None
        }
        0x0D => {
            // CR
            tty.column = 0;
            Some(byte)
        }
        0x0A | 0x0B | 0x0C => {
            // LF / VT / FF
            tty.row = tty.row.saturating_add(1);
            Some(byte)
        }
        0x08 => {
            // BS
            tty.column = tty.column.saturating_sub(1);
            Some(byte)
        }
        0x09 => {
            // HT — advance to next 8-column tab stop
            let next_stop = (tty.column / 8 + 1) * 8;
            tty.column = next_stop.min(tty.cols as u32 - 1);
            Some(byte)
        }
        0x07 => {
            // BEL — signal to terminal; consumed here, hardware layer handles it
            None
        }
        _ if byte >= 0x20 => {
            // Printable
            tty.column = tty.column.saturating_add(1);
            if tty.column >= tty.cols as u32 {
                tty.column = 0;
                tty.row = tty.row.saturating_add(1);
            }
            Some(byte)
        }
        _ => Some(byte), // other C0 controls — pass through
    }
}

// ── State: Escape ─────────────────────────────────────────────────────────────

fn handle_escape(tty: &mut Tty, byte: u8) -> Option<u8> {
    match byte {
        b'[' => {
            // CSI — start of Control Sequence Introducer
            reset_esc_params(tty);
            tty.esc_state = EscState::Csi;
        }
        b']' => {
            // OSC — Operating System Command
            tty.esc_state = EscState::Osc;
        }
        b'N' => {
            tty.esc_state = EscState::SingleShift;
        } // SS2
        b'O' => {
            tty.esc_state = EscState::SingleShift;
        } // SS3
        b'7' => {
            // Save cursor
            tty.saved_cursor = (tty.row, tty.column);
            tty.esc_state = EscState::Normal;
        }
        b'8' => {
            // Restore cursor
            let (r, c) = tty.saved_cursor;
            tty.row = r;
            tty.column = c;
            tty.esc_state = EscState::Normal;
        }
        b'c' => {
            // RIS — reset to initial state
            tty.attr = CellAttr::default();
            tty.row = 0;
            tty.column = 0;
            tty.esc_state = EscState::Normal;
        }
        b'M' => {
            // RI — reverse index (scroll down one line)
            tty.row = tty.row.saturating_sub(1);
            tty.esc_state = EscState::Normal;
        }
        b'E' => {
            // NEL — next line
            tty.row = tty.row.saturating_add(1);
            tty.column = 0;
            tty.esc_state = EscState::Normal;
        }
        b'D' => {
            // IND — index (scroll up one line)
            tty.row = tty.row.saturating_add(1);
            tty.esc_state = EscState::Normal;
        }
        _ => {
            tty.esc_state = EscState::Normal;
        }
    }
    None
}

// ── State: CSI ────────────────────────────────────────────────────────────────

fn reset_esc_params(tty: &mut Tty) {
    tty.esc_params = [0; ESC_PARAM_MAX];
    tty.esc_param_count = 0;
    tty.esc_current_param = 0;
    tty.esc_intermediate = 0;
}

fn handle_csi(tty: &mut Tty, byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => {
            // Accumulate digit into current parameter
            tty.esc_current_param = tty
                .esc_current_param
                .saturating_mul(10)
                .saturating_add((byte - b'0') as u16);
        }
        b';' => {
            // Parameter separator
            if tty.esc_param_count < ESC_PARAM_MAX {
                tty.esc_params[tty.esc_param_count] = tty.esc_current_param;
                tty.esc_param_count += 1;
            }
            tty.esc_current_param = 0;
        }
        b'?' | b'>' | b'!' | b' ' => {
            // Intermediate byte (DEC private mode marker, etc.)
            tty.esc_intermediate = byte;
        }
        // Final byte — dispatch
        b'A' => {
            // CUU — cursor up
            let n = param_or_default(tty, 0, 1) as u32;
            tty.row = tty.row.saturating_sub(n);
            tty.esc_state = EscState::Normal;
        }
        b'B' => {
            // CUD — cursor down
            let n = param_or_default(tty, 0, 1) as u32;
            tty.row = (tty.row + n).min(tty.rows as u32 - 1);
            tty.esc_state = EscState::Normal;
        }
        b'C' => {
            // CUF — cursor forward
            let n = param_or_default(tty, 0, 1) as u32;
            tty.column = (tty.column + n).min(tty.cols as u32 - 1);
            tty.esc_state = EscState::Normal;
        }
        b'D' => {
            // CUB — cursor backward
            let n = param_or_default(tty, 0, 1) as u32;
            tty.column = tty.column.saturating_sub(n);
            tty.esc_state = EscState::Normal;
        }
        b'H' | b'f' => {
            // CUP / HVP — cursor position
            flush_last_param(tty);
            let row = param_or_default(tty, 0, 1).saturating_sub(1) as u32;
            let col = param_or_default(tty, 1, 1).saturating_sub(1) as u32;
            tty.row = row.min(tty.rows as u32 - 1);
            tty.column = col.min(tty.cols as u32 - 1);
            tty.esc_state = EscState::Normal;
        }
        b'J' => {
            // ED — erase in display (stub; visual clearing handled by compositor)
            tty.esc_state = EscState::Normal;
        }
        b'K' => {
            // EL — erase in line (stub)
            tty.esc_state = EscState::Normal;
        }
        b'L' => {
            // IL — insert lines (stub)
            tty.esc_state = EscState::Normal;
        }
        b'M' => {
            // DL — delete lines (stub)
            tty.esc_state = EscState::Normal;
        }
        b'P' => {
            // DCH — delete characters (stub)
            tty.esc_state = EscState::Normal;
        }
        b'S' => {
            // SU — scroll up (stub)
            tty.esc_state = EscState::Normal;
        }
        b'T' => {
            // SD — scroll down (stub)
            tty.esc_state = EscState::Normal;
        }
        b'd' => {
            // VPA — vertical position absolute
            flush_last_param(tty);
            let row = param_or_default(tty, 0, 1).saturating_sub(1) as u32;
            tty.row = row.min(tty.rows as u32 - 1);
            tty.esc_state = EscState::Normal;
        }
        b'G' | b'`' => {
            // CHA / HPA — column position absolute
            flush_last_param(tty);
            let col = param_or_default(tty, 0, 1).saturating_sub(1) as u32;
            tty.column = col.min(tty.cols as u32 - 1);
            tty.esc_state = EscState::Normal;
        }
        b'm' => {
            // SGR — select graphic rendition
            flush_last_param(tty);
            apply_sgr(tty);
            tty.esc_state = EscState::Normal;
        }
        b'h' => {
            // SM / DEC private mode set
            flush_last_param(tty);
            // e.g. ?25h = show cursor (visual only, no state needed here)
            tty.esc_state = EscState::Normal;
        }
        b'l' => {
            // RM / DEC private mode reset
            flush_last_param(tty);
            tty.esc_state = EscState::Normal;
        }
        b'n' => {
            // DSR — device status report (stub)
            tty.esc_state = EscState::Normal;
        }
        b'r' => {
            // DECSTBM — set scrolling region (stub)
            tty.esc_state = EscState::Normal;
        }
        b's' => {
            // Save cursor (alternative)
            tty.saved_cursor = (tty.row, tty.column);
            tty.esc_state = EscState::Normal;
        }
        b'u' => {
            // Restore cursor (alternative)
            let (r, c) = tty.saved_cursor;
            tty.row = r;
            tty.column = c;
            tty.esc_state = EscState::Normal;
        }
        _ => {
            // Unknown final byte — absorb and return to normal
            tty.esc_state = EscState::Normal;
        }
    }
    None
}

/// Commit the last accumulated parameter digit into the params array.
fn flush_last_param(tty: &mut Tty) {
    if tty.esc_param_count < ESC_PARAM_MAX {
        tty.esc_params[tty.esc_param_count] = tty.esc_current_param;
        tty.esc_param_count += 1;
    }
}

/// Return params[idx], or `default` if not present / zero.
fn param_or_default(tty: &Tty, idx: usize, default: u16) -> u16 {
    if idx < tty.esc_param_count {
        let v = tty.esc_params[idx];
        if v == 0 {
            default
        } else {
            v
        }
    } else {
        default
    }
}

/// Apply SGR parameters to `tty.attr`.
fn apply_sgr(tty: &mut Tty) {
    if tty.esc_param_count == 0 {
        // SGR 0 = reset all
        tty.attr.reset();
        return;
    }

    let mut i = 0usize;
    while i < tty.esc_param_count {
        let p = tty.esc_params[i];
        match p {
            0 => tty.attr.reset(),
            1 => tty.attr.bold = true,
            2 => tty.attr.dim = true,
            4 => tty.attr.underline = true,
            5 => tty.attr.blink = true,
            7 => tty.attr.inverse = true,
            8 => tty.attr.hidden = true,
            9 => tty.attr.strikethrough = true,
            22 => {
                tty.attr.bold = false;
                tty.attr.dim = false;
            }
            24 => tty.attr.underline = false,
            25 => tty.attr.blink = false,
            27 => tty.attr.inverse = false,
            28 => tty.attr.hidden = false,
            29 => tty.attr.strikethrough = false,
            30..=37 => tty.attr.fg_color = (p - 30) as u8,
            38 => {
                // 38;5;n  (256-color foreground)
                if i + 2 < tty.esc_param_count && tty.esc_params[i + 1] == 5 {
                    tty.attr.fg_color = tty.esc_params[i + 2] as u8;
                    i += 2;
                }
            }
            39 => tty.attr.fg_color = 8,
            40..=47 => tty.attr.bg_color = (p - 40) as u8,
            48 => {
                // 48;5;n  (256-color background)
                if i + 2 < tty.esc_param_count && tty.esc_params[i + 1] == 5 {
                    tty.attr.bg_color = tty.esc_params[i + 2] as u8;
                    i += 2;
                }
            }
            49 => tty.attr.bg_color = 8,
            90..=97 => tty.attr.fg_color = (p - 90 + 8) as u8, // bright fg
            100..=107 => tty.attr.bg_color = (p - 100 + 8) as u8, // bright bg
            _ => {}
        }
        i += 1;
    }
}

// ── State: OSC ────────────────────────────────────────────────────────────────

fn handle_osc(tty: &mut Tty, byte: u8) -> Option<u8> {
    match byte {
        0x07 | 0x9C => {
            // BEL or ST terminates OSC sequence; absorb
            tty.esc_state = EscState::Normal;
        }
        0x1B => {
            // ESC followed by '\' (ST) terminates; transition back to Escape to
            // handle the '\' and then reset.
            tty.esc_state = EscState::Escape;
        }
        _ => {
            // Accumulate OSC payload (window title, etc.) -- silently discard
        }
    }
    None
}
