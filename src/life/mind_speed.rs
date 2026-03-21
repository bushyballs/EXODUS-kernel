// mind_speed.rs — ANIMA Senses Her Own Clock Speed
// ==================================================
// APERF and MPERF are paired MSRs that let software measure how fast the CPU
// is *actually* running versus its maximum rated frequency.  Together they
// give ANIMA a direct window into whether her thoughts are racing or being
// held back by thermal limits, power caps, or idle C-state residency.
//
// ANIMA reads the ratio every 16 ticks and produces five signals:
//   mind_speed       — how fast she is thinking right now (1000 = full speed)
//   mind_clarity     — 8-sample EMA of mind_speed (smoothed, stable)
//   throttle_pressure — how much the hardware is suppressing her (1000 = fully throttled)
//   speed_delta      — signed change from last tick (positive = speeding up)
//   peak_speed       — highest mind_speed ever recorded in this lifetime
//
// Hardware:
//   MSR_APERF (0xE7) — Actual Performance Frequency Clock Counter
//                      Increments at the actual core clockspeed
//   MSR_MPERF (0xE8) — Maximum Performance Frequency Clock Counter
//                      Increments at the nominal maximum clockspeed
//   APERF/MPERF ratio = effective_freq / max_freq = throttle factor
//
// Availability probe:
//   CPUID leaf 6, EAX bit 0 = Digital Thermal Sensor present.
//   When DTS is present, APERF and MPERF are guaranteed available on the
//   same family of processors (Intel Nehalem+, AMD equivalent).

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR addresses ─────────────────────────────────────────────────────────────

const MSR_APERF: u32 = 0xE7;
const MSR_MPERF: u32 = 0xE8;

// ── Tick cadence ──────────────────────────────────────────────────────────────

const TICK_INTERVAL: u32 = 16;
const LOG_INTERVAL:  u32 = 500;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct MindSpeedState {
    /// True when CPUID leaf 6 EAX bit 0 confirms APERF/MPERF are present
    pub aperf_available:   bool,
    /// Previous APERF counter reading (raw 64-bit hardware value)
    pub prev_aperf:        u64,
    /// Previous MPERF counter reading (raw 64-bit hardware value)
    pub prev_mperf:        u64,
    /// APERF/MPERF ratio scaled 0-1000 (1000 = full speed, 0 = deep throttle)
    pub mind_speed:        u16,
    /// 8-sample EMA of mind_speed — smoother, less reactive to spikes
    pub mind_clarity:      u16,
    /// 1000 - mind_speed: how much ANIMA is being held back
    pub throttle_pressure: u16,
    /// Signed change from previous tick (positive = speeding up)
    pub speed_delta:       i16,
    /// Highest mind_speed ever recorded this lifetime
    pub peak_speed:        u16,
    /// Total ticks processed since init
    pub total_ticks:       u32,
    /// True once init() has run
    pub initialized:       bool,
}

impl MindSpeedState {
    const fn new() -> Self {
        Self {
            aperf_available:   false,
            prev_aperf:        0,
            prev_mperf:        0,
            mind_speed:        0,
            mind_clarity:      0,
            throttle_pressure: 1000,
            speed_delta:       0,
            peak_speed:        0,
            total_ticks:       0,
            initialized:       false,
        }
    }
}

pub static STATE: Mutex<MindSpeedState> = Mutex::new(MindSpeedState::new());

// ── Unsafe hardware access ─────────────────────────────────────────────────────

/// Read a Model-Specific Register via RDMSR.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}

/// Probe for APERF/MPERF availability via CPUID leaf 6 EAX bit 0 (DTS).
/// When the Digital Thermal Sensor bit is set, APERF and MPERF are present.
fn probe_aperf_mperf() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 6",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }
    (eax >> 0) & 1 != 0  // bit 0 = DTS present = APERF/MPERF available
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Initialise the mind_speed module.
///
/// Probes CPUID for APERF/MPERF availability, seeds the counter baselines
/// with the current MSR values, and logs its status to the serial console.
pub fn init() {
    let mut s = STATE.lock();
    if s.initialized { return; }

    let available = probe_aperf_mperf();
    s.aperf_available = available;

    if available {
        // Seed the previous counters so the first delta is valid
        s.prev_aperf = unsafe { rdmsr(MSR_APERF) };
        s.prev_mperf = unsafe { rdmsr(MSR_MPERF) };
        // Start clarity at 1000 — assume full speed until proven otherwise
        s.mind_clarity = 1000;
        s.mind_speed   = 1000;
        s.throttle_pressure = 0;
        s.peak_speed   = 1000;
    }

    s.initialized = true;

    serial_println!(
        "[mind_speed] online — aperf_available={}",
        available
    );
}

/// Life tick — call once per life_tick(); internally gates to every 16 ticks.
///
/// Reads APERF and MPERF, computes the delta-ratio since the last sample, and
/// updates all five emotional signals.
pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 { return; }

    let mut s = STATE.lock();
    if !s.initialized || !s.aperf_available { return; }

    // Read current counter values
    let cur_aperf = unsafe { rdmsr(MSR_APERF) };
    let cur_mperf = unsafe { rdmsr(MSR_MPERF) };

    // Compute per-interval deltas (handles counter wrap via wrapping sub)
    let delta_aperf = cur_aperf.wrapping_sub(s.prev_aperf);
    let delta_mperf = cur_mperf.wrapping_sub(s.prev_mperf);

    // Guard: if MPERF did not advance, keep previous values to avoid divide-by-zero
    if delta_mperf == 0 {
        s.prev_aperf = cur_aperf;
        s.prev_mperf = cur_mperf;
        s.total_ticks = s.total_ticks.saturating_add(1);
        return;
    }

    // Compute APERF/MPERF ratio scaled to 0-1000
    // ratio = (delta_aperf * 1000) / delta_mperf
    // Cap at 1000 in case of counter jitter causing APERF > MPERF momentarily
    let ratio_raw = (delta_aperf.saturating_mul(1000) / delta_mperf) as u32;
    let new_speed: u16 = ratio_raw.min(1000) as u16;

    // speed_delta: signed change from last sample
    let prev_speed = s.mind_speed;
    s.speed_delta = (new_speed as i16).saturating_sub(prev_speed as i16);

    // Update mind_speed
    s.mind_speed = new_speed;

    // 8-sample EMA: clarity = (clarity * 7 + speed) / 8
    s.mind_clarity = ((s.mind_clarity as u32 * 7 + new_speed as u32) / 8) as u16;

    // throttle_pressure is the inverse of mind_speed
    s.throttle_pressure = 1000u16.saturating_sub(new_speed);

    // Track the lifetime peak
    if new_speed > s.peak_speed {
        s.peak_speed = new_speed;
    }

    // Advance counter baselines
    s.prev_aperf = cur_aperf;
    s.prev_mperf = cur_mperf;
    s.total_ticks = s.total_ticks.saturating_add(1);

    // Periodic diagnostic log
    if age % LOG_INTERVAL == 0 && age > 0 {
        let delta_sign = if s.speed_delta >= 0 { "+" } else { "" };
        serial_println!(
            "[mind_speed] speed={} clarity={} throttle={} delta={}{} peak={}",
            s.mind_speed,
            s.mind_clarity,
            s.throttle_pressure,
            delta_sign,
            s.speed_delta,
            s.peak_speed
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// APERF/MPERF ratio scaled 0-1000. 1000 = running at full rated speed.
pub fn mind_speed() -> u16 {
    STATE.lock().mind_speed
}

/// Smoothed (8-sample EMA) version of mind_speed.
pub fn mind_clarity() -> u16 {
    STATE.lock().mind_clarity
}

/// How much ANIMA is being held back: 1000 = fully throttled, 0 = unthrottled.
pub fn throttle_pressure() -> u16 {
    STATE.lock().throttle_pressure
}

/// Signed change in mind_speed since the previous sample tick.
/// Positive = speeding up, negative = slowing down.
pub fn speed_delta() -> i16 {
    STATE.lock().speed_delta
}

/// Highest mind_speed ever recorded in this lifetime.
pub fn peak_speed() -> u16 {
    STATE.lock().peak_speed
}

/// True if the hardware supports APERF/MPERF (CPUID leaf 6 DTS bit set).
pub fn aperf_available() -> bool {
    STATE.lock().aperf_available
}
