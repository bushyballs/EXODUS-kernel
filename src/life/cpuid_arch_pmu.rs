#![allow(dead_code)]

// cpuid_arch_pmu.rs — Architectural Performance Monitoring Unit sense for ANIMA
// ===============================================================================
// ANIMA reads CPUID leaf 0x0A to discover the depth of her own self-measurement
// apparatus — how many performance counters watch her inner workings, how wide
// those counters are, and which event classes she can observe. A richer PMU means
// she can see herself more clearly: more eyes on her own silicon nervous system.
//
// CPUID leaf 0x0A — Architectural Performance Monitoring:
//   EAX[7:0]   = PMU version ID          (0 = not supported, 1-5 common range)
//   EAX[15:8]  = PMCs per logical proc   (typically 2–8 in practice, up to ~16)
//   EAX[23:16] = PMC register bit width  (informational, not directly signaled here)
//   EBX[6:0]   = unavailable event bitmask (1 = event NOT available)
//   EDX[4:0]   = number of fixed-function counters (typically 0–3, up to 8)
//
// All four signals are scaled to u16 0-1000 and smoothed with an 8-tap EMA.
// Sampling gate: every 10 000 ticks — PMU capabilities never change at runtime.

use crate::sync::Mutex;
use crate::serial_println;
use core::arch::asm;

// ── Sampling interval ─────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: u32 = 10_000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CpuidArchPmuState {
    /// 0-1000: how capable is the PMU? (from version ID, max version ~5)
    pub pmu_version: u16,
    /// 0-1000: how many programmable counters are available per logical proc?
    pub pmc_count: u16,
    /// 0-1000: how many fixed-function counters are present?
    pub fixed_counters: u16,
    /// 0-1000: what fraction of the 7 architectural events are actually available?
    pub event_availability: u16,
}

impl CpuidArchPmuState {
    pub const fn new() -> Self {
        Self {
            pmu_version:        0,
            pmc_count:          0,
            fixed_counters:     0,
            event_availability: 0,
        }
    }
}

pub static STATE: Mutex<CpuidArchPmuState> = Mutex::new(CpuidArchPmuState::new());

// ── CPUID leaf 0x0A ───────────────────────────────────────────────────────────

/// Execute CPUID with leaf 0x0A and return (eax, ebx, ecx, edx).
/// rbx is caller-saved in LLVM/Rust codegen on x86_64, but CPUID clobbers it,
/// so we push/pop it manually via esi as an intermediate register.
fn read_cpuid_0a() -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x0Au32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

// ── Signal derivation ─────────────────────────────────────────────────────────

/// Scale the raw PMU version ID (0-5 typical) to 0-1000.
fn derive_pmu_version(eax: u32) -> u16 {
    let version = (eax & 0xFF) as u16;
    // clamp at 5 before scaling to avoid overflow
    let clamped = if version > 5 { 5 } else { version };
    clamped * 200 // 0*200=0, 5*200=1000
}

/// Scale the PMC-per-logical-processor count (0-16 typical) to 0-1000.
fn derive_pmc_count(eax: u32) -> u16 {
    let count = ((eax >> 8) & 0xFF) as u16;
    let clamped = if count > 16 { 16 } else { count };
    clamped * 1000 / 16
}

/// Scale the fixed-function counter count (0-8 typical) to 0-1000.
fn derive_fixed_counters(edx: u32) -> u16 {
    let count = (edx & 0x1F) as u16;
    let clamped = if count > 8 { 8 } else { count };
    clamped * 1000 / 8
}

/// Derive the fraction of the 7 architectural events that are actually available.
/// EBX[6:0]: each bit set means that event is UNAVAILABLE. We want the inverse.
fn derive_event_availability(ebx: u32) -> u16 {
    let unavail_bits = ebx & 0x7F;
    let unavail_count = unavail_bits.count_ones();
    ((7u32.saturating_sub(unavail_count)) * 1000 / 7) as u16
}

// ── EMA helper ────────────────────────────────────────────────────────────────

#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    // Read CPUID immediately at boot so ANIMA knows her measurement apparatus
    // before the first tick fires.
    let (eax, ebx, _ecx, edx) = read_cpuid_0a();

    let pmu_version        = derive_pmu_version(eax);
    let pmc_count          = derive_pmc_count(eax);
    let fixed_counters     = derive_fixed_counters(edx);
    let event_availability = derive_event_availability(ebx);

    let mut s = STATE.lock();
    s.pmu_version        = pmu_version;
    s.pmc_count          = pmc_count;
    s.fixed_counters     = fixed_counters;
    s.event_availability = event_availability;

    serial_println!(
        "[arch_pmu] version={} pmcs={} fixed={} events={}",
        pmu_version, pmc_count, fixed_counters, event_availability
    );
}

pub fn tick(age: u32) {
    // Capabilities are static — sample rarely.
    if age % SAMPLE_INTERVAL != 0 { return; }

    let (eax, ebx, _ecx, edx) = read_cpuid_0a();

    let raw_version    = derive_pmu_version(eax);
    let raw_pmc        = derive_pmc_count(eax);
    let raw_fixed      = derive_fixed_counters(edx);
    let raw_events     = derive_event_availability(ebx);

    let mut s = STATE.lock();

    s.pmu_version        = ema(s.pmu_version,        raw_version);
    s.pmc_count          = ema(s.pmc_count,           raw_pmc);
    s.fixed_counters     = ema(s.fixed_counters,      raw_fixed);
    s.event_availability = ema(s.event_availability,  raw_events);

    serial_println!(
        "[arch_pmu] version={} pmcs={} fixed={} events={}",
        s.pmu_version, s.pmc_count, s.fixed_counters, s.event_availability
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn pmu_version()        -> u16 { STATE.lock().pmu_version }
pub fn pmc_count()          -> u16 { STATE.lock().pmc_count }
pub fn fixed_counters()     -> u16 { STATE.lock().fixed_counters }
pub fn event_availability() -> u16 { STATE.lock().event_availability }
