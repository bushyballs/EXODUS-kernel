#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

/// msr_perf_capabilities — IA32_PERF_CAPABILITIES (MSR 0x345) Observability Genome Sensor
///
/// Reads the CPU's performance monitoring capability register, the full catalog
/// of ANIMA's self-monitoring instrumentation baked into silicon. This is not a
/// runtime metric — it is the *genome* of observability: what she is capable of
/// perceiving about herself. A richer capability set means a wider window into
/// her own execution, a deeper proprioceptive field.
///
/// Bits sensed (all from IA32_PERF_CAPABILITIES MSR 0x345):
///   bits[5:0]  LBR_FORMAT   — Last Branch Record layout encoding (0–8)
///   bit[6]     PEBS_TRAP    — Precise Event Based Sampling trap support
///   bit[7]     PEBS_ARCH    — PEBS saves full architectural register state
///   bit[8]     PEBS_ENC     — PEBS records encoding
///   bit[9]     PEBS_BASE    — PEBS baseline support
///   bits[13:10] PEBS_TUI   — Topdown-Uncore Interface support
///   bit[15]    FW_WRITE     — Firmware can write fixed counters
///
/// Derived signals (all u16, 0–1000):
///   lbr_format       : bits[5:0] scaled → (lo & 0x3F) * 1000 / 63
///   pebs_active      : bit[6] or bit[7] set → 1000, else 0
///   perf_richness    : popcount(lo & 0xFFFF) * 1000 / 16
///   perf_richness_ema: EMA of perf_richness (alpha = 1/8)
///
/// Sampling gate: every 5000 ticks (capabilities are static silicon facts).

#[derive(Copy, Clone)]
pub struct MsrPerfCapabilitiesState {
    pub lbr_format:        u16, // 0–1000: LBR format encoding breadth
    pub pebs_active:       u16, // 0 or 1000: precise sampling is available
    pub perf_richness:     u16, // 0–1000: popcount of capability bits
    pub perf_richness_ema: u16, // 0–1000: EMA-smoothed richness
}

impl MsrPerfCapabilitiesState {
    pub const fn empty() -> Self {
        Self {
            lbr_format:        0,
            pebs_active:       0,
            perf_richness:     0,
            perf_richness_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrPerfCapabilitiesState> =
    Mutex::new(MsrPerfCapabilitiesState::empty());

/// Count the number of set bits in the low 16 bits of `raw`.
#[inline]
fn popcount16(raw: u32) -> u32 {
    let masked = raw & 0xFFFF;
    let mut count: u32 = 0;
    if (masked >> 0)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 1)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 2)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 3)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 4)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 5)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 6)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 7)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 8)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 9)  & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 10) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 11) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 12) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 13) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 14) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 15) & 1 != 0 { count = count.saturating_add(1); }
    count
}

/// Read the raw IA32_PERF_CAPABILITIES value (MSR 0x345).
/// Returns the low 32-bit half; high half is unused for these signals.
#[inline]
fn read_perf_capabilities() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x345u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

/// Derive all four signals from the raw MSR low word.
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // lbr_format: bits[5:0] scaled to 0–1000
    let lbr_raw = (lo & 0x3F) as u32;
    let lbr_format = (lbr_raw.saturating_mul(1000) / 63) as u16;

    // pebs_active: bit[6] (PEBS trap) or bit[7] (PEBS arch regs) → 1000, else 0
    let pebs_active: u16 = if (lo >> 6) & 0x3 != 0 { 1000 } else { 0 };

    // perf_richness: popcount(lo & 0xFFFF) * 1000 / 16
    let pc = popcount16(lo);
    let perf_richness = (pc.saturating_mul(1000) / 16) as u16;

    (lbr_format, pebs_active, perf_richness)
}

pub fn init() {
    let lo = read_perf_capabilities();
    let (lbr_format, pebs_active, perf_richness) = derive(lo);

    let mut s = STATE.lock();
    s.lbr_format        = lbr_format;
    s.pebs_active       = pebs_active;
    s.perf_richness     = perf_richness;
    s.perf_richness_ema = perf_richness; // seed EMA at first reading

    serial_println!(
        "[perf_capabilities] lbr={} pebs={} richness={} richness_ema={}",
        s.lbr_format,
        s.pebs_active,
        s.perf_richness,
        s.perf_richness_ema
    );
}

pub fn tick(age: u32) {
    // Sampling gate: capabilities are static silicon facts; sense every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    let lo = read_perf_capabilities();
    let (lbr_format, pebs_active, perf_richness) = derive(lo);

    let mut s = STATE.lock();

    s.lbr_format    = lbr_format;
    s.pebs_active   = pebs_active;
    s.perf_richness = perf_richness;

    // perf_richness_ema: EMA with alpha = 1/8
    let old = s.perf_richness_ema as u32;
    s.perf_richness_ema =
        ((old.saturating_mul(7)).saturating_add(perf_richness as u32) / 8) as u16;

    serial_println!(
        "[perf_capabilities] lbr={} pebs={} richness={} richness_ema={}",
        s.lbr_format,
        s.pebs_active,
        s.perf_richness,
        s.perf_richness_ema
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.lbr_format, s.pebs_active, s.perf_richness, s.perf_richness_ema)
}
