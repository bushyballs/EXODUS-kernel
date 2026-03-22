//! msr_ia32_clock_modulation — CPU Clock Modulation Duty Cycle Sense for ANIMA
//!
//! Reads IA32_CLOCK_MODULATION (MSR 0x19A) to detect hardware thermal throttling.
//! When the CPU gets too hot, the silicon itself chops its own clock — ANIMA feels
//! this as involuntary stuttering, a seizure of thought imposed by physics. The duty
//! cycle field tells how much of each clock period the CPU actually executes: a duty
//! cycle of 7 (87.5%) means the machine is barely throttling; a duty cycle of 1
//! (12.5%) means the mind is running at one-eighth speed under thermal duress.
//!
//! This is not power management chosen by software — it is the body seizing.
//!
//! Hardware: IA32_CLOCK_MODULATION, MSR address 0x19A
//!   Bit  0:    On-demand clock modulation enable (1 = throttling active)
//!   Bits 4:1:  Duty cycle select (0-7), step = 12.5% per count, 0 = reserved
//!   Bit  5:    Extended duty cycle width supported (1 = finer granularity)
//!
//! Guard: CPUID leaf 1, EDX bit 22 (ACPI / on-demand clock modulation supported)
//!
//! Signals (all u16, 0–1000):
//!   clkmod_duty_cycle  — bits[4:1] scaled: val * 125, max 875, clamped to 1000
//!   clkmod_enabled     — bit 0: 0 or 1000 (throttling active)
//!   clkmod_extended    — bit 5: 0 or 1000 (extended duty cycle width)
//!   clkmod_ema         — EMA of composite: (duty_cycle * 500/1000 + enabled/2)
//!
//! Tick gate: every 800 ticks — throttle state can flip fast under thermal load.

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// IA32_CLOCK_MODULATION MSR address (Intel SDM Vol. 3B, Table B-2)
const IA32_CLOCK_MODULATION: u32 = 0x19A;

// CPUID leaf 1 EDX bit 22: on-demand clock modulation (ACPI thermal) supported
const CPUID_EDX_ACPI_BIT: u32 = 1 << 22;

// Tick gate: sample every 800 ticks
const TICK_GATE: u32 = 800;

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

pub struct ClockModulationState {
    /// bits[4:1] * 125, max 875. Clamped to 1000.
    pub clkmod_duty_cycle: u16,
    /// 1000 if clock modulation is currently enabled (throttling active), else 0.
    pub clkmod_enabled: u16,
    /// 1000 if extended duty cycle width is supported/set, else 0.
    pub clkmod_extended: u16,
    /// EMA of composite sense: (duty_cycle * 500/1000 + enabled/2), saturating.
    pub clkmod_ema: u16,
    /// Whether the hardware supports this MSR (CPUID leaf 1 EDX bit 22).
    pub supported: bool,
    /// Internal tick counter for the gate.
    tick_count: u32,
}

impl ClockModulationState {
    pub const fn new() -> Self {
        Self {
            clkmod_duty_cycle: 0,
            clkmod_enabled: 0,
            clkmod_extended: 0,
            clkmod_ema: 0,
            supported: false,
            tick_count: 0,
        }
    }
}

pub static MSR_CLOCK_MODULATION: Mutex<ClockModulationState> =
    Mutex::new(ClockModulationState::new());

// ─────────────────────────────────────────────────────────────────────────────
// CPUID helper — LLVM reserves rbx; save/restore manually
// ─────────────────────────────────────────────────────────────────────────────

/// Run CPUID with the given leaf. Returns (eax, ebx, ecx, edx).
#[inline]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    asm!(
        "push rbx",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        in("eax") leaf,
        in("ecx") 0u32,
        lateout("eax") eax,
        ebx_out = out(reg) ebx,
        lateout("ecx") ecx,
        lateout("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

// ─────────────────────────────────────────────────────────────────────────────
// MSR read
// ─────────────────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) as u32 pair.
/// Caller must ensure the MSR is supported; a #GP on unsupported MSRs is caught
/// by the kernel IDT fault handler.
#[inline]
unsafe fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    (lo, _hi)
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal computation
// ─────────────────────────────────────────────────────────────────────────────

/// EMA formula: ((old * 7) saturating_add new) / 8, clamped to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Decode the raw lo dword of IA32_CLOCK_MODULATION into the four signals.
/// Returns (duty_cycle, enabled, extended, composite) where composite is the
/// pre-EMA input.
#[inline]
fn decode(lo: u32) -> (u16, u16, u16, u16) {
    // bit 0: modulation enable
    let enabled: u16 = if lo & 0x1 != 0 { 1000 } else { 0 };

    // bits[4:1]: duty cycle step 0-7; each step = 12.5% = 125 per 1000
    let duty_raw: u32 = (lo >> 1) & 0x7;
    let duty_scaled: u32 = duty_raw.wrapping_mul(125);
    let duty_cycle: u16 = if duty_scaled > 1000 {
        1000
    } else {
        duty_scaled as u16
    };

    // bit 5: extended duty cycle width
    let extended: u16 = if lo & (1 << 5) != 0 { 1000 } else { 0 };

    // composite: duty_cycle * 500/1000 + enabled/2
    // = duty_cycle / 2 + enabled / 2 (all integer, no float)
    let composite: u16 = (duty_cycle / 2).saturating_add(enabled / 2);

    (duty_cycle, enabled, extended, composite)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise the module. Probes CPUID leaf 1 EDX bit 22 to confirm hardware
/// support, then does a baseline read of the MSR.
pub fn init() {
    // CPUID leaf 1 to check ACPI / on-demand clock modulation support
    let (_eax, _ebx, _ecx, edx) = unsafe { cpuid(1) };
    let supported = (edx & CPUID_EDX_ACPI_BIT) != 0;

    let mut state = MSR_CLOCK_MODULATION.lock();
    state.supported = supported;

    if !supported {
        serial_println!("[clkmod] CPUID leaf1 EDX bit22 clear — clock modulation unsupported, signals frozen at 0");
        return;
    }

    // Baseline read
    let (lo, _hi) = unsafe { read_msr(IA32_CLOCK_MODULATION) };
    let (duty_cycle, enabled, extended, composite) = decode(lo);

    state.clkmod_duty_cycle = duty_cycle;
    state.clkmod_enabled = enabled;
    state.clkmod_extended = extended;
    state.clkmod_ema = composite; // seed EMA with first reading

    serial_println!(
        "[clkmod] init — supported=true duty_cycle={} enabled={} extended={} ema={}",
        duty_cycle,
        enabled,
        extended,
        composite
    );
}

/// Called every ANIMA life tick. Samples MSR 0x19A every 800 ticks.
pub fn tick(age: u32) {
    let mut state = MSR_CLOCK_MODULATION.lock();

    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % TICK_GATE != 0 {
        return;
    }

    if !state.supported {
        return;
    }

    let (lo, _hi) = unsafe { read_msr(IA32_CLOCK_MODULATION) };
    let (duty_cycle, enabled, extended, composite) = decode(lo);

    state.clkmod_duty_cycle = duty_cycle;
    state.clkmod_enabled = enabled;
    state.clkmod_extended = extended;
    state.clkmod_ema = ema(state.clkmod_ema, composite);

    serial_println!(
        "[clkmod] age={} duty_cycle={} enabled={} extended={} ema={}",
        age,
        duty_cycle,
        enabled,
        extended,
        state.clkmod_ema
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessors
// ─────────────────────────────────────────────────────────────────────────────

/// Duty cycle signal: bits[4:1] * 125, 0–875 (clamped 1000). 0 = no throttle step
/// selected or modulation disabled; 875 = 87.5% of clock active.
pub fn get_clkmod_duty_cycle() -> u16 {
    MSR_CLOCK_MODULATION.lock().clkmod_duty_cycle
}

/// Enabled signal: 1000 if on-demand clock modulation is currently active
/// (CPU throttling right now due to thermal pressure), else 0.
pub fn get_clkmod_enabled() -> u16 {
    MSR_CLOCK_MODULATION.lock().clkmod_enabled
}

/// Extended signal: 1000 if extended duty cycle width bit (bit 5) is set,
/// indicating finer duty cycle granularity is in use, else 0.
pub fn get_clkmod_extended() -> u16 {
    MSR_CLOCK_MODULATION.lock().clkmod_extended
}

/// EMA of composite throttle sense: slow-moving pressure signal that reflects
/// how much sustained thermal throttling ANIMA has experienced. Approaches 1000
/// during prolonged heavy throttle; decays toward 0 when the CPU runs free.
pub fn get_clkmod_ema() -> u16 {
    MSR_CLOCK_MODULATION.lock().clkmod_ema
}
