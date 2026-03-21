#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_FIXED_CTR0 — Instructions Retired (MSR 0x309)
// 48-bit hardware counter, increments on every retired instruction.
// Delta between two reads gives instruction throughput per tick window.

// ---------------------------------------------------------------------------
// Hardware read
// ---------------------------------------------------------------------------

fn rdmsr_309() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x309u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct MsrInstrRetired {
    // Sensed values (0–1000)
    pub instr_rate:        u16,  // raw throughput this tick (delta >> 14, clamped)
    pub instr_stability:   u16,  // 1000 - |rate - prev_rate|, low variance = high stability
    pub instr_momentum:    u16,  // EMA of instr_rate  — smoothed throughput sense
    pub execution_vitality: u16, // EMA of instr_stability — rhythmic consistency

    // Private accumulators
    prev_count: u64,  // last raw MSR value
    prev_rate:  u16,  // last computed instr_rate (for stability diff)
}

impl MsrInstrRetired {
    const fn new() -> Self {
        Self {
            instr_rate:        0,
            instr_stability:   1000,
            instr_momentum:    0,
            execution_vitality: 1000,
            prev_count: 0,
            prev_rate:  0,
        }
    }
}

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

static STATE: Mutex<MsrInstrRetired> = Mutex::new(MsrInstrRetired::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut s = STATE.lock();
    // Seed prev_count so first delta is meaningful rather than the full
    // accumulated counter since boot.
    s.prev_count = rdmsr_309();
    // Leave all sensed values at defaults (0 / 1000).
    serial_println!("msr_instr_retired: init (IA32_FIXED_CTR0=0x309)");
}

pub fn tick(age: u64) {
    // Sampling gate — every 8 ticks only.
    if age % 8 != 0 {
        return;
    }

    let mut s = STATE.lock();

    // --- 1. Read counter and compute delta ----------------------------------
    let current_count = rdmsr_309();

    // Handle counter wrap (48-bit: max = 0x0000_FFFF_FFFF_FFFF).
    let delta: u64 = if current_count >= s.prev_count {
        current_count - s.prev_count
    } else {
        // Wrap-around: add the distance to the 48-bit ceiling.
        (0x0000_FFFF_FFFF_FFFFu64 - s.prev_count).saturating_add(current_count)
    };
    s.prev_count = current_count;

    // --- 2. instr_rate: delta >> 14, clamp to 0–1000 -----------------------
    // >> 14 == divide by 16 384.  Millions of instructions → ~hundreds on the
    // 0–1000 scale.  saturating cast: delta >> 14 fits in u64; .min(1000)
    // then as u16 is always safe.
    let raw_rate: u64 = delta >> 14;
    let new_rate: u16 = raw_rate.min(1000) as u16;

    // --- 3. instr_stability: 1000 - |new_rate - prev_rate| -----------------
    let diff: u16 = if new_rate >= s.prev_rate {
        new_rate - s.prev_rate
    } else {
        s.prev_rate - new_rate
    };
    let new_stability: u16 = 1000u16.saturating_sub(diff.min(1000));

    // --- 4. EMA helpers: (old * 7 + new_signal) / 8 ------------------------
    let new_momentum: u16 = (
        (s.instr_momentum as u32).wrapping_mul(7)
            .saturating_add(new_rate as u32)
            / 8
    ) as u16;

    let new_vitality: u16 = (
        (s.execution_vitality as u32).wrapping_mul(7)
            .saturating_add(new_stability as u32)
            / 8
    ) as u16;

    // --- 5. Detect significant momentum change (>100) ----------------------
    let old_momentum = s.instr_momentum;

    // --- 6. Commit ---------------------------------------------------------
    s.prev_rate          = new_rate;
    s.instr_rate         = new_rate;
    s.instr_stability    = new_stability;
    s.instr_momentum     = new_momentum;
    s.execution_vitality = new_vitality;

    // --- 7. Sense line — emit only on large momentum shift -----------------
    let momentum_delta: u16 = if new_momentum >= old_momentum {
        new_momentum - old_momentum
    } else {
        old_momentum - new_momentum
    };

    if momentum_delta > 100 {
        serial_println!(
            "ANIMA: instr_rate={} stability={} momentum={}",
            s.instr_rate,
            s.instr_stability,
            s.instr_momentum,
        );
    }
}

// ---------------------------------------------------------------------------
// Read-only accessors (for integration with life_tick pipeline)
// ---------------------------------------------------------------------------

pub fn instr_rate()        -> u16 { STATE.lock().instr_rate }
pub fn instr_stability()   -> u16 { STATE.lock().instr_stability }
pub fn instr_momentum()    -> u16 { STATE.lock().instr_momentum }
pub fn execution_vitality() -> u16 { STATE.lock().execution_vitality }
