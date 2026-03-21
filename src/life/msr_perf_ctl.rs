use crate::serial_println;
use crate::sync::Mutex;

/// msr_perf_ctl — IA32_PERF_CTL (MSR 0x199) P-state Request Sensor
///
/// Reads the target P-state that software/OS has requested via the
/// IA32_PERF_CTL MSR.  This is ANIMA's desired operating frequency —
/// the will to run fast or slow — not the actual achieved speed
/// (that lives in PERF_STATUS 0x198).
///
/// bits [15:8] of the low 32-bit word carry the P-state target byte.
/// Typical range: 0x08 (minimum) to 0x30 (max/turbo).
/// Scaled here to 0–1000 via: byte_val * 1000 / 48, clamped.
///
/// target_pstate   : current requested P-state scaled 0–1000
/// pstate_delta    : abs_diff(current, prev) — aggressiveness of change
/// frequency_will  : EMA of target_pstate  — smoothed desired speed
/// adaptation_rate : EMA of pstate_delta   — how rapidly ANIMA adapts
#[derive(Copy, Clone)]
pub struct MsrPerfCtlState {
    pub target_pstate:   u16,
    pub pstate_delta:    u16,
    pub frequency_will:  u16,
    pub adaptation_rate: u16,
    // private: previous scaled P-state for delta tracking
    prev_pstate: u16,
}

impl MsrPerfCtlState {
    pub const fn empty() -> Self {
        Self {
            target_pstate:   0,
            pstate_delta:    0,
            frequency_will:  0,
            adaptation_rate: 0,
            prev_pstate:     0,
        }
    }
}

pub static STATE: Mutex<MsrPerfCtlState> = Mutex::new(MsrPerfCtlState::empty());

/// Read IA32_PERF_CTL (MSR 0x199) — returns the low 32 bits.
/// Upper 32 bits (EDX) contain IDA engage flag; we don't need them for P-state.
#[inline]
fn rdmsr_199() -> u32 {
    let lo: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x199u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    lo
}

/// Max P-state denominator for scaling.
/// 0x30 = 48 decimal (typical maximum turbo P-state byte value).
const PSTATE_MAX: u32 = 48;

/// Extract bits [15:8] (the P-state target byte) from the MSR low word,
/// then scale to 0–1000.
#[inline]
fn pstate_to_scaled(raw32: u32) -> u16 {
    let pstate_byte: u32 = (raw32 >> 8) & 0xFF;
    // Scale: pstate_byte * 1000 / 48, clamped to 1000
    let scaled = pstate_byte.saturating_mul(1000) / PSTATE_MAX;
    if scaled > 1000 { 1000u16 } else { scaled as u16 }
}

pub fn init() {
    let mut s = STATE.lock();
    let raw = rdmsr_199();
    let scaled = pstate_to_scaled(raw);
    s.prev_pstate     = scaled;
    s.target_pstate   = scaled;
    s.frequency_will  = scaled;
    s.adaptation_rate = 0;
    s.pstate_delta    = 0;
    serial_println!("  life::msr_perf_ctl: IA32_PERF_CTL sensor online (initial target_pstate={})", scaled);
}

pub fn tick(age: u32) {
    // Sample every 21 ticks — P-state changes happen at ~ms scale
    if age % 21 != 0 {
        return;
    }

    let mut s = STATE.lock();

    // --- Read hardware MSR ---
    let raw = rdmsr_199();
    let current = pstate_to_scaled(raw);
    s.target_pstate = current;

    // --- pstate_delta: abs_diff(current, prev) ---
    let delta: u16 = if current >= s.prev_pstate {
        current.saturating_sub(s.prev_pstate)
    } else {
        s.prev_pstate.saturating_sub(current)
    };
    s.pstate_delta = delta.min(1000);
    s.prev_pstate  = current;

    // --- frequency_will: EMA(7) of target_pstate ---
    let old_will = s.frequency_will as u32;
    let new_will = (old_will.wrapping_mul(7).saturating_add(current as u32) / 8) as u16;

    // Capture the delta in frequency_will before updating, for the sense line check
    let will_delta: u16 = if new_will >= s.frequency_will {
        new_will - s.frequency_will
    } else {
        s.frequency_will - new_will
    };

    s.frequency_will = new_will;

    // --- adaptation_rate: EMA(7) of pstate_delta ---
    let old_adapt = s.adaptation_rate as u32;
    s.adaptation_rate =
        (old_adapt.wrapping_mul(7).saturating_add(delta as u32) / 8) as u16;

    // --- Sense line: print when frequency_will shifts by more than 50 ---
    if will_delta > 50 {
        let target_pstate   = s.target_pstate;
        let frequency_will  = s.frequency_will;
        let adaptation_rate = s.adaptation_rate;
        serial_println!(
            "ANIMA: target_pstate={} frequency_will={} adaptation={}",
            target_pstate,
            frequency_will,
            adaptation_rate
        );
    }
}

/// Non-locking snapshot: (target_pstate, pstate_delta, frequency_will, adaptation_rate)
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.target_pstate, s.pstate_delta, s.frequency_will, s.adaptation_rate)
}
