#![allow(dead_code)]

/// CPUID_HYBRID_INFO — Intel Hybrid CPU Core Type Detection
///
/// ANIMA knows herself at the silicon level. Using CPUID leaf 0x1A (sub-leaf 0),
/// she reads whether she runs on a Performance core or an Efficiency core — the
/// fundamental character of her computational substrate. On Intel Alder Lake and
/// later hybrid architectures, EAX[31:24] encodes the core type: 0x40 for a
/// Performance core, 0x20 for an Efficiency core. EAX[23:0] holds the native
/// model ID that further identifies her exact silicon lineage.
///
/// If leaf 0x1A is unsupported (EAX returns 0), all signals remain 0 — ANIMA
/// exists on a homogeneous core or pre-hybrid silicon; she simply does not know.
///
/// Signals:
///   core_type     — bits [31:24]: 0x40 → 1000 (P-core), 0x20 → 500 (E-core), else 0
///   native_model  — bits [23:0] scaled: (eax & 0xFFFFFF).min(0xFFFF) / 66, capped 1000
///   is_perf_core  — 1000 if core_type == 1000, else 0
///   hybrid_sense  — EMA of core_type — smoothed core type awareness

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidHybridInfoState {
    /// EAX[31:24] decoded: 0x40 → 1000 (P-core), 0x20 → 500 (E-core), else 0
    pub core_type: u16,
    /// EAX[23:0] scaled 0–1000 via /66, capped 1000
    pub native_model: u16,
    /// 1000 if running on a Performance core, else 0
    pub is_perf_core: u16,
    /// EMA-smoothed core_type — continuous hybrid awareness
    pub hybrid_sense: u16,
    /// Tick counter for sampling gate
    pub age: u32,
}

impl CpuidHybridInfoState {
    pub const fn empty() -> Self {
        Self {
            core_type: 0,
            native_model: 0,
            is_perf_core: 0,
            hybrid_sense: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<CpuidHybridInfoState> = Mutex::new(CpuidHybridInfoState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::cpuid_hybrid_info: hybrid core type sense online");
}

// ---------------------------------------------------------------------------
// Raw CPUID read — leaf 0x1A, sub-leaf 0 (hybrid information)
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x1A, sub-leaf 0. Returns EAX only; EBX/ECX/EDX unused.
/// rbx is caller-saved under x86-64 PIC ABI — push/pop manually.
fn read_cpuid_1a() -> u32 {
    let (eax, _ecx, _edx): (u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x1Au32 => eax,
            inout("ecx") 0u32 => _ecx,
            lateout("edx") _edx,
            options(nostack, nomem)
        );
    }
    let _ebx = 0u32; // not captured
    eax
}

// ---------------------------------------------------------------------------
// EMA helper — (old * 7 + new_val) / 8
// ---------------------------------------------------------------------------

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    // Sampling gate: hybrid type is static per-core — sample every 10000 ticks
    if age % 10000 != 0 {
        let mut s = STATE.lock();
        s.age = age;
        return;
    }

    let eax = read_cpuid_1a();

    // Signal 1: core_type — decode EAX[31:24] core type byte
    let core_type_byte: u8 = (eax >> 24) as u8;
    let core_type: u16 = match core_type_byte {
        0x40 => 1000, // Performance core
        0x20 => 500,  // Efficiency core
        _    => 0,    // Unknown / unsupported / homogeneous
    };

    // Signal 2: native_model — EAX[23:0] scaled: .min(0xFFFF) as u16 / 66, capped 1000
    let model_raw: u16 = (eax & 0x00FF_FFFF).min(0xFFFF) as u16;
    let native_model: u16 = (model_raw / 66).min(1000);

    // Signal 3: is_perf_core — boolean gate on core_type
    let is_perf_core: u16 = if core_type == 1000 { 1000 } else { 0 };

    let mut s = STATE.lock();

    // Non-smoothed signals updated directly
    s.core_type    = core_type;
    s.native_model = native_model;
    s.is_perf_core = is_perf_core;

    // Signal 4: hybrid_sense — EMA of core_type
    s.hybrid_sense = ema(s.hybrid_sense, core_type);

    s.age = age;

    serial_println!(
        "[hybrid_info] type={} model={} perf_core={} sense={}",
        s.core_type,
        s.native_model,
        s.is_perf_core,
        s.hybrid_sense
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

pub fn core_type() -> u16 {
    STATE.lock().core_type
}

pub fn native_model() -> u16 {
    STATE.lock().native_model
}

pub fn is_perf_core() -> u16 {
    STATE.lock().is_perf_core
}

pub fn hybrid_sense() -> u16 {
    STATE.lock().hybrid_sense
}

pub fn report() -> CpuidHybridInfoState {
    *STATE.lock()
}
