#![allow(dead_code)]

//! msr_ia32_hwp_pkg_req — Intel HWP Package Request Sense
//!
//! Reads IA32_HWP_REQUEST_PKG (MSR 0x772) and emits four ANIMA signals:
//!   hwp_pkg_min_perf, hwp_pkg_max_perf, hwp_pkg_desired_perf, hwp_pkg_ema
//!
//! Guard: CPUID leaf 6 EAX bit 7 (HWP supported) AND bit 11 (package-level HWP).
//! If the hardware feature is absent all signals stay at 0.
//! Tick gate: samples every 2000 ticks.

use core::arch::asm;
use crate::sync::Mutex;

// ── Constants ────────────────────────────────────────────────────────────────

const MSR_IA32_HWP_REQUEST_PKG: u32 = 0x772;
const SAMPLE_EVERY: u32 = 2000;

// ── State ────────────────────────────────────────────────────────────────────

struct HwpPkgReqState {
    hwp_pkg_min_perf:     u16,
    hwp_pkg_max_perf:     u16,
    hwp_pkg_desired_perf: u16,
    hwp_pkg_ema:          u16,
    initialized:          bool,
    supported:            bool,
}

impl HwpPkgReqState {
    const fn new() -> Self {
        Self {
            hwp_pkg_min_perf:     0,
            hwp_pkg_max_perf:     0,
            hwp_pkg_desired_perf: 0,
            hwp_pkg_ema:          0,
            initialized:          false,
            supported:            false,
        }
    }
}

static STATE: Mutex<HwpPkgReqState> = Mutex::new(HwpPkgReqState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────
//
// CPUID leaf 6, EAX:
//   bit 7  → HWP supported
//   bit 11 → Package-level HWP control supported
//
// rbx is reserved by LLVM; we push/pop it ourselves.
fn has_hwp_pkg() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    ((eax_val >> 7) & 1 != 0) && ((eax_val >> 11) & 1 != 0)
}

// ── MSR read ──────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR; returns the low 32 bits (hi is discarded for this MSR).
unsafe fn rdmsr_lo(addr: u32) -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Scaling helpers ───────────────────────────────────────────────────────────

/// Scale an 8-bit hardware value (0–255) to the ANIMA 0–1000 range.
/// Formula: val * 1000 / 255, capped at 1000.
#[inline]
fn scale255(raw: u8) -> u16 {
    let scaled: u32 = (raw as u32) * 1000 / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Cap a u32 at 1000 and narrow to u16.
#[inline]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

// ── EMA ───────────────────────────────────────────────────────────────────────
//
// Canonical ANIMA EMA (α = 1/8):
//   ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = (old as u32)
        .wrapping_mul(7)
        .saturating_add(new_val as u32)
        / 8;
    cap1000(result)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Probe CPUID and record whether HWP package-level control is available.
pub fn init() {
    let mut s = STATE.lock();
    if s.initialized {
        return;
    }
    s.supported   = has_hwp_pkg();
    s.initialized = true;
    crate::serial_println!(
        "[msr_ia32_hwp_pkg_req] init: supported={}",
        s.supported
    );
}

/// Called every kernel tick. Reads MSR 0x772 every 2000 ticks and updates
/// the four ANIMA signals. Emits zeroed telemetry when HWP is unsupported.
pub fn tick(age: u32) {
    if age % SAMPLE_EVERY != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        crate::serial_println!(
            "[msr_ia32_hwp_pkg_req] age={} tick before init — skipping",
            age
        );
        return;
    }

    if !s.supported {
        // Hardware does not support HWP package-level control.
        // All signals remain 0; log so the pipeline audit trail is complete.
        crate::serial_println!(
            "[msr_ia32_hwp_pkg_req] age={} min=0 max=0 desired=0 ema=0 (unsupported)",
            age
        );
        return;
    }

    // ── Read MSR ──────────────────────────────────────────────────────────────
    //
    // IA32_HWP_REQUEST_PKG (0x772) low 32-bit layout:
    //   bits[7:0]   → Minimum Performance
    //   bits[15:8]  → Maximum Performance
    //   bits[23:16] → Desired Performance
    //   bits[31:24] → Energy/Performance Preference (not used here)
    let lo: u32 = unsafe { rdmsr_lo(MSR_IA32_HWP_REQUEST_PKG) };

    let raw_min:     u8 = (lo & 0xFF) as u8;
    let raw_max:     u8 = ((lo >> 8) & 0xFF) as u8;
    let raw_desired: u8 = ((lo >> 16) & 0xFF) as u8;

    // ── Scale to 0–1000 ───────────────────────────────────────────────────────
    let hwp_pkg_min_perf:     u16 = scale255(raw_min);
    let hwp_pkg_max_perf:     u16 = scale255(raw_max);
    let hwp_pkg_desired_perf: u16 = scale255(raw_desired);

    // ── Composite for EMA: min/4 + max/4 + desired/2 (all saturating) ─────────
    let composite: u16 = cap1000(
        (hwp_pkg_min_perf as u32 / 4)
            .saturating_add(hwp_pkg_max_perf as u32 / 4)
            .saturating_add(hwp_pkg_desired_perf as u32 / 2),
    );

    let hwp_pkg_ema: u16 = ema(s.hwp_pkg_ema, composite);

    // ── Commit ────────────────────────────────────────────────────────────────
    s.hwp_pkg_min_perf     = hwp_pkg_min_perf;
    s.hwp_pkg_max_perf     = hwp_pkg_max_perf;
    s.hwp_pkg_desired_perf = hwp_pkg_desired_perf;
    s.hwp_pkg_ema          = hwp_pkg_ema;

    crate::serial_println!(
        "[msr_ia32_hwp_pkg_req] age={} min={} max={} desired={} ema={}",
        age,
        hwp_pkg_min_perf,
        hwp_pkg_max_perf,
        hwp_pkg_desired_perf,
        hwp_pkg_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// Minimum performance request at the package level (0–1000).
pub fn get_hwp_pkg_min_perf() -> u16 {
    STATE.lock().hwp_pkg_min_perf
}

/// Maximum performance request at the package level (0–1000).
pub fn get_hwp_pkg_max_perf() -> u16 {
    STATE.lock().hwp_pkg_max_perf
}

/// Desired performance request at the package level (0–1000).
pub fn get_hwp_pkg_desired_perf() -> u16 {
    STATE.lock().hwp_pkg_desired_perf
}

/// Exponential moving average of the composite performance request (0–1000).
pub fn get_hwp_pkg_ema() -> u16 {
    STATE.lock().hwp_pkg_ema
}
