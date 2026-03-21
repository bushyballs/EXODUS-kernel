#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct AmxState {
    amx_tiles:       u16,
    amx_rows:        u16,
    amx_total_bytes: u16,
    amx_ema:         u16,
    pt_supported:    bool,
}

impl AmxState {
    const fn new() -> Self {
        Self {
            amx_tiles:       0,
            amx_rows:        0,
            amx_total_bytes: 0,
            amx_ema:         0,
            pt_supported:    false,
        }
    }
}

static STATE: Mutex<AmxState> = Mutex::new(AmxState::new());

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Returns true if the CPU reports AMX-BF16 (bit 24) or AMX-TILE (bit 25)
/// support via CPUID leaf 7 sub-leaf 0 EDX.
fn has_amx() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    // bit 24 = AMX-BF16, bit 25 = AMX-TILE
    ((edx_val >> 24) & 3) != 0
}

/// Read CPUID leaf 0x1D sub-leaf 0: returns EAX (max palette index).
fn cpuid_1d_sl0_eax() -> u32 {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x1Du32 => eax_val,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax_val
}

/// Read CPUID leaf 0x1D sub-leaf 1:
///   EAX bits[15:0]  = total tile bytes
///   EAX bits[31:16] = bytes per row
///   EBX bits[15:0]  = max rows
///   EBX bits[31:16] = num tiles
fn cpuid_1d_sl1() -> (u32, u32) {
    let eax_val: u32;
    let ebx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out}, rbx",
            "pop rbx",
            inout("eax") 0x1Du32 => eax_val,
            in("ecx") 1u32,
            ebx_out = out(reg) ebx_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax_val, ebx_val)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// num_tiles (EBX bits[31:16]) → 0-1000.  8 tiles max → 8*125 = 1000.
#[inline]
fn scale_tiles(num_tiles: u32) -> u16 {
    (num_tiles.saturating_mul(125)).min(1000) as u16
}

/// max_rows (EBX bits[15:0]) → 0-1000.  16 rows max → 16*62 = 992 ≈ 1000.
#[inline]
fn scale_rows(max_rows: u32) -> u16 {
    (max_rows.saturating_mul(62)).min(1000) as u16
}

/// total_bytes → 0-1000.  Formula: (total_bytes / 1024) * 62, clamped to 1000.
/// (e.g. 16 KB → 16 * 62 = 992)
#[inline]
fn scale_total_bytes(total_bytes: u32) -> u16 {
    ((total_bytes / 1024).saturating_mul(62)).min(1000) as u16
}

/// EMA: (old * 7 + new_val) / 8, computed in u32 then cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let o = old as u32;
    let n = new_val as u32;
    ((o * 7 + n) / 8) as u16
}

// ── AMX read ─────────────────────────────────────────────────────────────────

/// Reads AMX tile geometry from leaf 0x1D.
/// Returns (amx_tiles, amx_rows, amx_total_bytes) as 0-1000 signals.
fn read_amx_signals() -> (u16, u16, u16) {
    // Consume sub-leaf 0 (max palette — informational, not used for signals).
    let _max_palette = cpuid_1d_sl0_eax();

    // Sub-leaf 1: tile dimensions.
    let (eax_val, ebx_val) = cpuid_1d_sl1();

    let total_bytes  = eax_val & 0x0000_FFFF;          // EAX[15:0]
    // bytes_per_row  = (eax_val >> 16) & 0xFFFF;      // EAX[31:16] — reserved for future use
    let max_rows     = ebx_val & 0x0000_FFFF;           // EBX[15:0]
    let num_tiles    = (ebx_val >> 16) & 0x0000_FFFF;  // EBX[31:16]

    (
        scale_tiles(num_tiles),
        scale_rows(max_rows),
        scale_total_bytes(total_bytes),
    )
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Initialise the AMX info module.  Checks hardware support and, if present,
/// performs the first CPUID read to populate baseline signal values.
pub fn init() {
    let mut s = STATE.lock();

    if !has_amx() {
        s.pt_supported = false;
        // All signals remain 0 — AMX absent.
        crate::serial_println!(
            "[cpuid_amx_info] AMX not supported — all signals zeroed"
        );
        return;
    }

    s.pt_supported = true;

    let (tiles, rows, total_bytes) = read_amx_signals();
    s.amx_tiles       = tiles;
    s.amx_rows        = rows;
    s.amx_total_bytes = total_bytes;
    s.amx_ema         = tiles; // seed EMA with first sample

    crate::serial_println!(
        "[cpuid_amx_info] init — tiles={} rows={} bytes={} ema={}",
        s.amx_tiles,
        s.amx_rows,
        s.amx_total_bytes,
        s.amx_ema,
    );
}

/// Tick the AMX info module.  Samples hardware every 10 000 ticks (AMX tile
/// geometry is static; the high interval avoids pointless CPUID overhead).
pub fn tick(age: u32) {
    // Sampling gate — static hardware rarely changes.
    if age % 10_000 != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.pt_supported {
        // AMX absent — nothing to do.
        return;
    }

    let (tiles, rows, total_bytes) = read_amx_signals();

    s.amx_tiles       = tiles;
    s.amx_rows        = rows;
    s.amx_total_bytes = total_bytes;
    s.amx_ema         = ema(s.amx_ema, tiles);

    crate::serial_println!(
        "[cpuid_amx_info] age={} tiles={} rows={} bytes={} ema={}",
        age,
        s.amx_tiles,
        s.amx_rows,
        s.amx_total_bytes,
        s.amx_ema,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Number-of-AMX-tiles signal (0-1000).
pub fn get_amx_tiles() -> u16 {
    STATE.lock().amx_tiles
}

/// Max-rows signal (0-1000).
pub fn get_amx_rows() -> u16 {
    STATE.lock().amx_rows
}

/// Total-tile-bytes signal (0-1000).
pub fn get_amx_total_bytes() -> u16 {
    STATE.lock().amx_total_bytes
}

/// EMA of the amx_tiles signal (0-1000).
pub fn get_amx_ema() -> u16 {
    STATE.lock().amx_ema
}
