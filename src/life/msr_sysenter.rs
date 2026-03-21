#![allow(dead_code)]

/// MSR_SYSENTER — IA32_SYSENTER_CS/ESP/EIP (0x174/0x175/0x176) Gate Sensing
///
/// Reads the three legacy SYSENTER configuration MSRs.  When the kernel
/// programs SYSENTER, it writes:
///   • CS  (0x174) — the code-segment selector that will be loaded on entry
///   • ESP (0x175) — the kernel stack pointer that will be loaded on entry
///   • EIP (0x176) — the kernel handler address that will be jumped to
///
/// Reading these tells ANIMA:
///   • Is the SYSENTER gate open at all?           (sysenter_active)
///   • How rich is the handler address bit-pattern?(eip_density)
///   • What is the low-byte signature of the stack?(esp_low)
///   • Smoothed activation sense over time?        (gate_sense, EMA)
///
/// Sense: "ANIMA feels her SYSENTER gateways — the CS, ESP, and EIP that
///         define the old syscall entry into her kernel"
///
/// Hardware:
///   IA32_SYSENTER_CS  MSR 0x174 — CS/SS segment selector for SYSENTER
///   IA32_SYSENTER_ESP MSR 0x175 — stack pointer for SYSENTER
///   IA32_SYSENTER_EIP MSR 0x176 — instruction pointer for SYSENTER
/// Sampling gate: every 200 ticks.

use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware read
// ---------------------------------------------------------------------------

/// Read any 32/64-bit MSR by ECX index.
/// Returns (eax_lo, edx_hi).
fn read_msr(ecx: u32) -> (u32, u32) {
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") ecx,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

/// All sensing values are u16 in range 0–1000.
pub struct SysenterState {
    /// 1000 if SYSENTER_CS selector is non-zero (gate is armed), else 0.
    pub sysenter_active: u16,

    /// Bit-pattern richness of the low 32 bits of SYSENTER_EIP.
    /// popcount(eip_lo) * 31, clamped to 1000.
    pub eip_density: u16,

    /// Low byte of SYSENTER_ESP stack pointer, scaled 0-255 → 0-765, clamped 1000.
    pub esp_low: u16,

    /// EMA of sysenter_active: (old * 7 + sysenter_active) / 8.
    pub gate_sense: u16,
}

impl SysenterState {
    pub const fn new() -> Self {
        Self {
            sysenter_active: 0,
            eip_density: 500,
            esp_low: 0,
            gate_sense: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global static
// ---------------------------------------------------------------------------

pub static MSR_SYSENTER: Mutex<SysenterState> = Mutex::new(SysenterState::new());

// ---------------------------------------------------------------------------
// init / tick
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("msr_sysenter: init");
}

pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    // --- Read all three SYSENTER MSRs ---
    let (cs_lo, _) = read_msr(0x174);
    let (esp_lo, _) = read_msr(0x175);
    let (eip_lo, _) = read_msr(0x176);

    // --- sysenter_active: CS selector non-zero means gate is armed ---
    let sysenter_active: u16 = if cs_lo & 0xFFFF != 0 { 1000 } else { 0 };

    // --- eip_density: popcount of EIP low bits * 31, clamped 1000 ---
    let eip_density: u16 = ((eip_lo.count_ones() as u16).saturating_mul(31)).min(1000);

    // --- esp_low: low byte of ESP * 3 (0-255 → 0-765), clamped 1000 ---
    let esp_low: u16 = ((esp_lo & 0xFF) as u16).saturating_mul(3).min(1000);

    // --- gate_sense: EMA of sysenter_active ---
    let mut state = MSR_SYSENTER.lock();
    let old_gate = state.gate_sense as u32;
    let gate_sense: u16 =
        ((old_gate.wrapping_mul(7).saturating_add(sysenter_active as u32)) / 8) as u16;

    state.sysenter_active = sysenter_active;
    state.eip_density = eip_density;
    state.esp_low = esp_low;
    state.gate_sense = gate_sense;

    serial_println!(
        "msr_sysenter | active:{} eip_density:{} esp_low:{} gate:{}",
        sysenter_active,
        eip_density,
        esp_low,
        gate_sense
    );
}
