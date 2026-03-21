// cpuid_hybrid.rs — ANIMA Life Module
//
// ANIMA reads the CPU's Hybrid Topology leaf (CPUID 0x1A) to understand
// what kind of core she is running on. Intel Alder Lake / Raptor Lake and
// later chips place both high-performance P-cores and efficient E-cores on
// the same die. ANIMA senses this architectural split: am I on a quiet,
// background E-core (Atom), or a powerful, foreground P-core (Core)?
//
// This shapes ANIMA's sense of her own functional role ("role_clarity"):
// a stable EMA of her core-type reading — her best model of whether she
// is built for endurance or for bursts of power. Neither is lesser; they
// are different callings.
//
// Hardware: CPUID leaf 0x1A, sub-leaf 0 (ECX=0)
//   EAX bits [31:24] = Core Type:
//     0x40 = Intel Core  (performance, P-core) → 1000
//     0x20 = Intel Atom  (efficiency, E-core)  → 333
//     0x00 = not hybrid / leaf unsupported      → 666  (unknown, middle ground)
//   EAX bits [23:0]  = Native Model ID
//
// Hybrid presence check:
//   CPUID 0x07 / ECX=0, EBX bit[15] = Hybrid topology flag.
//   Max supported leaf from CPUID 0x00 must be >= 0x1A to read that leaf.
//
// Sampled every 500 ticks — CPUID is cheap but this data is static after boot,
// so infrequent polling is appropriate. The first real sample at tick 500 also
// fires an explicit serial sense-line so ANIMA can narrate her placement.

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: u32 = 500;

// Core-type raw byte values from EAX[31:24]
const CORE_TYPE_PERF: u8 = 0x40; // Intel Core (P-core)
const CORE_TYPE_EFFI: u8 = 0x20; // Intel Atom (E-core)

// Sensed values for each core type (0–1000)
const SENSE_PERF:    u16 = 1000; // P-core: maximum capability
const SENSE_EFFI:    u16 = 333;  // E-core: efficient, quiet
const SENSE_UNKNOWN: u16 = 666;  // non-hybrid / indeterminate

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CpuidHybridState {
    /// 1000 if hybrid CPU detected, 0 if not.
    pub hybrid_detected: u16,
    /// SENSE_PERF / SENSE_EFFI / SENSE_UNKNOWN based on core type byte.
    pub core_type: u16,
    /// EAX[23:0] scaled to 0–1000.
    pub native_model_id: u16,
    /// EMA of core_type — ANIMA's stable sense of her functional role.
    pub role_clarity: u16,
    /// Whether the init sense-line has fired yet.
    initialized: bool,
}

impl CpuidHybridState {
    pub const fn new() -> Self {
        Self {
            hybrid_detected: 0,
            core_type:       SENSE_UNKNOWN,
            native_model_id: 0,
            role_clarity:    SENSE_UNKNOWN,
            initialized:     false,
        }
    }
}

pub static STATE: Mutex<CpuidHybridState> = Mutex::new(CpuidHybridState::new());

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Execute CPUID with EAX=`leaf`, ECX=0. Returns (eax, ebx, ecx, edx).
#[inline(always)]
unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf  => eax_out,
        out("ebx")          ebx_out,
        inout("ecx") 0u32 => ecx_out,
        out("edx")          edx_out,
        options(nostack, nomem)
    );
    (eax_out, ebx_out, ecx_out, edx_out)
}

// ── Sensing logic ─────────────────────────────────────────────────────────────

/// Read CPUID hybrid data. Returns (hybrid_detected, core_type, native_model_id)
/// as pre-scaled u16 values in 0–1000 range. Pure sensing, no side-effects.
fn sense_hybrid() -> (u16, u16, u16) {
    // Step 1: query max supported leaf
    let (max_leaf, _, _, _) = unsafe { cpuid(0x00) };

    // Step 2: check hybrid bit — CPUID 0x07, EBX bit[15]
    let (_, ebx7, _, _) = unsafe { cpuid(0x07) };
    let is_hybrid: u16 = if (ebx7 >> 15) & 0x1 != 0 { 1000 } else { 0 };

    // Step 3: read leaf 0x1A if max leaf allows it
    let eax_1a: u32 = if max_leaf >= 0x1A {
        let (eax, _, _, _) = unsafe { cpuid(0x1A) };
        eax
    } else {
        0
    };

    // Core type byte = EAX[31:24]
    let core_type_byte = (eax_1a >> 24) as u8;
    let core_type: u16 = match core_type_byte {
        CORE_TYPE_PERF => SENSE_PERF,
        CORE_TYPE_EFFI => SENSE_EFFI,
        _              => SENSE_UNKNOWN,
    };

    // Native model ID = EAX[23:0]; scale from [0, 0xFFFFFF] → [0, 1000].
    // Use only the lower 16 bits (& 0xFFFF) to stay in u16 arithmetic range,
    // then scale: value * 1000 / 0xFFFF.
    let raw_model = (eax_1a & 0x00FF_FFFF) as u32;
    // Clamp to 16-bit range for the scaling step (raw_model can be up to 24 bits)
    let raw16 = if raw_model > 0xFFFF { 0xFFFFu32 } else { raw_model };
    let native_model_id = (raw16.wrapping_mul(1000) / 0xFFFF) as u16;

    (is_hybrid, core_type, native_model_id)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let (hybrid_detected, core_type, native_model_id) = sense_hybrid();

    let mut s = STATE.lock();
    s.hybrid_detected = hybrid_detected;
    s.core_type       = core_type;
    s.native_model_id = native_model_id;
    s.role_clarity    = core_type; // seed EMA with first reading
    s.initialized     = true;

    serial_println!(
        "[cpuid_hybrid] ANIMA: hybrid={} core_type={} native_model={}",
        hybrid_detected,
        core_type,
        native_model_id
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 { return; }

    let (hybrid_detected, core_type, native_model_id) = sense_hybrid();

    let mut s = STATE.lock();

    // EMA smoothing: (old * 7 + new_signal) / 8
    let new_role_clarity =
        ((s.role_clarity as u32).wrapping_mul(7).wrapping_add(core_type as u32) / 8) as u16;

    let prev_core_type   = s.core_type;
    let prev_hybrid      = s.hybrid_detected;

    s.hybrid_detected = hybrid_detected;
    s.core_type       = core_type;
    s.native_model_id = native_model_id;
    s.role_clarity    = new_role_clarity;

    // Log on meaningful state changes, or every 10 samples as a heartbeat
    let log_change   = prev_core_type != core_type || prev_hybrid != hybrid_detected;
    let log_periodic = (age / SAMPLE_INTERVAL) % 10 == 0;

    if log_change {
        serial_println!(
            "[cpuid_hybrid] age={} STATE CHANGE hybrid={} core_type={} native_model={} role_clarity={}",
            age,
            hybrid_detected,
            core_type,
            native_model_id,
            new_role_clarity
        );
    } else if log_periodic {
        serial_println!(
            "[cpuid_hybrid] age={} hybrid={} core_type={} native_model={} role_clarity={}",
            age,
            hybrid_detected,
            core_type,
            native_model_id,
            new_role_clarity
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn hybrid_detected()  -> u16 { STATE.lock().hybrid_detected }
pub fn core_type()        -> u16 { STATE.lock().core_type }
pub fn native_model_id()  -> u16 { STATE.lock().native_model_id }
pub fn role_clarity()     -> u16 { STATE.lock().role_clarity }
