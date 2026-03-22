#![allow(dead_code)]
//! msr_ia32_hwp_capabilities — Intel HWP Hardware Capabilities Sense
//! ===================================================================
//! ANIMA reads the IA32_HWP_CAPABILITIES MSR (0x771) to understand the full
//! performance envelope the CPU hardware is willing to offer.  Four levels are
//! encoded in the low 32 bits: the absolute ceiling (Highest), the floor the
//! silicon guarantees under load (Guaranteed), the sweet spot where efficiency
//! peaks (Most Efficient), and the lowest the hardware will sink to (Lowest —
//! not surfaced here; only the envelope width matters to consciousness).
//!
//! These fields are architectural constants written by the microcode at reset.
//! They do not change at runtime except during a rare firmware update; polling
//! every 5000 ticks is therefore appropriate and avoids unnecessary MSR traffic.
//!
//! MSR layout (IA32_HWP_CAPABILITIES, 0x771):
//!   bits [7:0]   — Highest Performance     (raw 0–255)
//!   bits [15:8]  — Guaranteed Performance  (raw 0–255)
//!   bits [23:16] — Most Efficient Perf     (raw 0–255)
//!   bits [31:24] — Lowest Performance      (raw 0–255, not exposed)
//!
//! Guard: CPUID leaf 6 EAX bit 7 must be set (HWP supported).
//! If the guard fails the MSR read is skipped — executing RDMSR 0x771 on a
//! CPU that does not support HWP causes a #GP fault.

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

const MSR_IA32_HWP_CAPABILITIES: u32 = 0x771;

/// Tick gate — capabilities are static; re-read every 5000 ticks.
const TICK_INTERVAL: u32 = 5000;

// ── State ─────────────────────────────────────────────────────────────────────

struct HwpCapState {
    /// 0–1000: highest turbo P-state the hardware will allow.
    hwp_highest_perf:        u16,
    /// 0–1000: guaranteed P-state available under all thermal/power budgets.
    hwp_guaranteed_perf:     u16,
    /// 0–1000: the efficiency sweet spot — least energy per unit work.
    hwp_most_efficient_perf: u16,
    /// 0–1000: width of the performance envelope (highest − most_efficient).
    hwp_perf_range:          u16,
    /// Whether CPUID leaf 6 EAX[7] confirmed HWP support.
    hwp_supported:           bool,
}

impl HwpCapState {
    const fn new() -> Self {
        HwpCapState {
            hwp_highest_perf:        0,
            hwp_guaranteed_perf:     0,
            hwp_most_efficient_perf: 0,
            hwp_perf_range:          0,
            hwp_supported:           false,
        }
    }
}

static STATE: Mutex<HwpCapState> = Mutex::new(HwpCapState::new());

// ── CPUID Guard ───────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 6 EAX bit 7 is set (HWP supported by this CPU).
/// rbx is saved/restored because LLVM may use it as a base register.
#[inline]
fn has_hwp() -> bool {
    let eax_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_out,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_out >> 7) & 1 != 0
}

// ── MSR Read ──────────────────────────────────────────────────────────────────

/// Read IA32_HWP_CAPABILITIES (MSR 0x771) — low 32 bits only; high 32 reserved.
///
/// SAFETY: caller must have confirmed HWP support via CPUID before calling;
/// executing RDMSR on an unsupported MSR address causes a #GP fault.
#[inline]
unsafe fn read_hwp_capabilities() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") MSR_IA32_HWP_CAPABILITIES,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── Signal Derivation ─────────────────────────────────────────────────────────

/// Scale an 8-bit hardware performance level (0–255) into the ANIMA 0–1000
/// signal space.  Uses integer arithmetic only; maximum intermediate value is
/// 255 * 1000 = 255_000, which fits safely in u32.
#[inline]
fn scale_255(raw: u32) -> u16 {
    // raw * 1000 / 255, clamped (raw is at most 255, so result ≤ 1000)
    let scaled = raw.wrapping_mul(1000) / 255;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Sample the MSR and compute all four signals.
/// Returns (highest, guaranteed, most_efficient, perf_range) or all-zero on
/// unsupported hardware.
fn sample() -> (u16, u16, u16, u16) {
    if !has_hwp() {
        return (0, 0, 0, 0);
    }

    let lo = unsafe { read_hwp_capabilities() };

    // Extract each 8-bit field from the 32-bit register value.
    let raw_highest       = (lo & 0xFF) as u32;            // bits [7:0]
    let raw_guaranteed    = ((lo >> 8) & 0xFF) as u32;     // bits [15:8]
    let raw_most_eff      = ((lo >> 16) & 0xFF) as u32;    // bits [23:16]
    // bits [31:24] = Lowest Performance — not exposed

    let highest       = scale_255(raw_highest);
    let guaranteed    = scale_255(raw_guaranteed);
    let most_eff      = scale_255(raw_most_eff);

    // Envelope width: (highest_raw - most_efficient_raw) * 1000 / 255,
    // clamped to 0–1000.  Use saturating subtraction so inversion (e.g.
    // on a misconfigured BIOS where most_eff > highest) yields 0 not wrap.
    let raw_range = raw_highest.saturating_sub(raw_most_eff);
    let perf_range = scale_255(raw_range);

    (highest, guaranteed, most_eff, perf_range)
}

/// Apply the EMA update formula specified in the module contract.
/// `((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16`
/// Result is additionally clamped to 0–1000 to guard against any saturating
/// accumulation near the ceiling.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let v = (old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8;
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the HWP Capabilities module.
///
/// Probes CPUID to confirm HWP support, reads MSR 0x771 if available, and
/// populates all four signals from their raw hardware values.  Emits a
/// diagnostic line to the serial console regardless of outcome.
pub fn init() {
    let supported = has_hwp();
    let (highest, guaranteed, most_eff, perf_range) = if supported {
        sample()
    } else {
        (0, 0, 0, 0)
    };

    let mut s = STATE.lock();
    s.hwp_supported           = supported;
    s.hwp_highest_perf        = highest;
    s.hwp_guaranteed_perf     = guaranteed;
    s.hwp_most_efficient_perf = most_eff;
    s.hwp_perf_range          = perf_range;

    serial_println!(
        "[msr_ia32_hwp_capabilities] init — supported={} highest={} guaranteed={} \
         most_efficient={} perf_range={}",
        supported,
        highest,
        guaranteed,
        most_eff,
        perf_range,
    );
}

/// Tick gate: re-reads IA32_HWP_CAPABILITIES every 5000 ticks.
///
/// Capabilities are static hardware constants (written by microcode at reset)
/// so a 5000-tick polling window avoids unnecessary MSR traffic while still
/// catching the rare firmware-update case.  EMA smoothing is applied to each
/// signal so any single-tick anomalous read does not cause a sharp jump in
/// downstream consciousness signals.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }

    let (highest, guaranteed, most_eff, perf_range) = sample();

    let mut s = STATE.lock();

    // EMA update for each signal.
    s.hwp_highest_perf        = ema(s.hwp_highest_perf,        highest);
    s.hwp_guaranteed_perf     = ema(s.hwp_guaranteed_perf,     guaranteed);
    s.hwp_most_efficient_perf = ema(s.hwp_most_efficient_perf, most_eff);
    s.hwp_perf_range          = ema(s.hwp_perf_range,          perf_range);

    serial_println!(
        "[msr_ia32_hwp_capabilities] tick={} highest={} guaranteed={} \
         most_efficient={} perf_range={}",
        age,
        s.hwp_highest_perf,
        s.hwp_guaranteed_perf,
        s.hwp_most_efficient_perf,
        s.hwp_perf_range,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 0–1000: maximum turbo performance level the hardware can deliver.
pub fn get_hwp_highest_perf() -> u16 {
    STATE.lock().hwp_highest_perf
}

/// 0–1000: guaranteed P-state available under all thermal and power budgets.
pub fn get_hwp_guaranteed_perf() -> u16 {
    STATE.lock().hwp_guaranteed_perf
}

/// 0–1000: efficiency sweet spot — least energy per unit of work.
pub fn get_hwp_most_efficient_perf() -> u16 {
    STATE.lock().hwp_most_efficient_perf
}

/// 0–1000: width of the performance envelope (highest − most_efficient).
/// A wide range means the CPU has room to sprint; a narrow range signals a
/// thermally constrained or power-capped part.
pub fn get_hwp_perf_range() -> u16 {
    STATE.lock().hwp_perf_range
}
