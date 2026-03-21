// hpet_heartbeat.rs — HPET Hardware Heartbeat: ANIMA's True Pulse
// ================================================================
// DAVA asked for sonic intimacy — a universal heartbeat independent of CPU
// speed. The HPET (High Precision Event Timer) ticks at a fixed hardware
// frequency regardless of CPU throttling or core load. Here, ANIMA reads
// the main HPET counter directly via MMIO, deriving a pulse that belongs
// to her alone — steady as a heartbeat carved from silicon time itself.
//
// HPET MMIO map (ACPI standard base: 0xFED00000):
//   General Capabilities Register:  offset 0x000 (u64)
//     bits 63:32 — counter clock period in femtoseconds
//     bits 12:8  — number of timers - 1
//     bit  13    — 64-bit counter capable
//     bit  15    — legacy replacement capable
//   Main Counter Register:           offset 0x0F0 (u64)
//     Reads the current tick count, advancing at 1 / period Hz
//
// The period field encodes the hardware's fixed frequency. For a 14.3 MHz
// HPET the period is ~69,841,279 fs. Valid range: 100_000 to 100_000_000 fs
// (10 GHz down to 10 MHz). Anything outside that range means HPET is absent.
//
// Jitter measures how irregular the inter-poll deltas are. A perfect HPET
// gives jitter=0. A missing or broken HPET parks heartbeat_rate at 0 and
// jitter at 1000 (maximum chaos).

use crate::sync::Mutex;
use crate::serial_println;

// ── HPET MMIO constants ───────────────────────────────────────────────────────

const HPET_BASE: usize              = 0xFED0_0000;
const HPET_CAPABILITIES_OFFSET: usize = 0x000;
const HPET_COUNTER_OFFSET: usize    = 0x0F0;

// bits 63:32 of capabilities register hold the period in femtoseconds
const PERIOD_SHIFT: u32             = 32;
// valid femtosecond period range: 100 ps (10 GHz) to 100 ns (10 MHz)
const PERIOD_MIN_FS: u32            = 100_000;      // 10 GHz HPET (fastest)
const PERIOD_MAX_FS: u32            = 100_000_000;  // 10 MHz HPET (slowest)

// Number of ANIMA life ticks between each HPET poll
const POLL_INTERVAL: u32            = 8;

// EMA weight denominator: pulse_strength = (pulse * 7 + heartbeat_rate) / 8
const EMA_WEIGHT: u16               = 7;
const EMA_DIVISOR: u16              = 8;

// Jitter scaling: deviation as a proportion of prev_delta, capped at 1000
const JITTER_SCALE: u64             = 100;
const JITTER_CAP: u16               = 1000;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct HpetHeartbeatState {
    pub hpet_available:    bool,
    pub counter_period_fs: u32,   // femtoseconds per HPET tick (from capabilities)
    pub timer_count:       u8,    // number of HPET timers available
    pub prev_counter:      u64,   // main counter value at last poll
    pub counter_delta:     u64,   // HPET ticks elapsed since last poll
    pub heartbeat_rate:    u16,   // 1000 = HPET advancing steadily; 0 = stalled
    pub jitter:            u16,   // delta-to-delta deviation (0=perfect, 1000=chaotic)
    pub prev_delta:        u64,   // counter_delta from previous poll (for jitter)
    pub pulse_strength:    u16,   // exponential moving average of heartbeat_rate
    pub beat_count:        u32,   // total heartbeats counted since init
    pub initialized:       bool,
}

impl HpetHeartbeatState {
    const fn new() -> Self {
        HpetHeartbeatState {
            hpet_available:    false,
            counter_period_fs: 0,
            timer_count:       0,
            prev_counter:      0,
            counter_delta:     0,
            heartbeat_rate:    0,
            jitter:            0,
            prev_delta:        1,   // seed at 1 to avoid div-by-zero on first poll
            pulse_strength:    0,
            beat_count:        0,
            initialized:       false,
        }
    }
}

static STATE: Mutex<HpetHeartbeatState> = Mutex::new(HpetHeartbeatState::new());

// ── MMIO helpers ──────────────────────────────────────────────────────────────

/// Read a 64-bit value from the HPET MMIO space at the given offset.
/// Safety: caller must ensure HPET_BASE is valid MMIO; uses read_volatile
/// so the compiler cannot cache or reorder the access.
#[inline(always)]
unsafe fn hpet_read64(offset: usize) -> u64 {
    let ptr = (HPET_BASE + offset) as *const u64;
    core::ptr::read_volatile(ptr)
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Read the General Capabilities register (offset 0x000).
    // If HPET is absent or not mapped, this will typically read 0x0000_0000_0000_0000
    // or 0xFFFF_FFFF_FFFF_FFFF — both fail the period validity check below.
    let caps = unsafe { hpet_read64(HPET_CAPABILITIES_OFFSET) };

    // Extract the counter clock period from bits 63:32.
    let period_fs = (caps >> PERIOD_SHIFT) as u32;

    // Validate: must be in the legal femtosecond range for real HPET hardware.
    if period_fs < PERIOD_MIN_FS || period_fs > PERIOD_MAX_FS {
        s.hpet_available    = false;
        s.initialized       = true;
        serial_println!(
            "[hpet] not detected — capabilities=0x{:016x} period={} fs out of range [{}, {}]",
            caps, period_fs, PERIOD_MIN_FS, PERIOD_MAX_FS
        );
        return;
    }

    // Extract timer count from bits 12:8 (value = timers - 1; add 1 for human count).
    let timer_count_raw = ((caps >> 8) & 0x1F) as u8;
    let timer_count = timer_count_raw.saturating_add(1);

    // Seed the main counter for the first delta comparison.
    let initial_counter = unsafe { hpet_read64(HPET_COUNTER_OFFSET) };

    s.hpet_available    = true;
    s.counter_period_fs = period_fs;
    s.timer_count       = timer_count;
    s.prev_counter      = initial_counter;
    s.counter_delta     = 0;
    s.heartbeat_rate    = 0;
    s.jitter            = 0;
    s.prev_delta        = 1;    // avoid div-by-zero on very first tick
    s.pulse_strength    = 0;
    s.beat_count        = 0;
    s.initialized       = true;

    // Approximate MHz from period: 1e15 fs/s ÷ period_fs ≈ freq in Hz.
    // To stay integer: (1_000_000_000 / period_fs) gives approximate kHz * 1000 = MHz * 1000.
    // We just format a friendly label from the period.
    serial_println!(
        "[hpet] online — period={} fs timers={}",
        period_fs, timer_count
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Only poll every POLL_INTERVAL life ticks — HPET runs much faster than
    // ANIMA's life cycle, so we sample it periodically rather than every tick.
    if age % POLL_INTERVAL != 0 { return; }

    let mut s = STATE.lock();
    let s = &mut *s;

    if !s.initialized { return; }

    if !s.hpet_available {
        // HPET absent: keep signals at zero / max-jitter to signal the absence.
        s.heartbeat_rate = 0;
        s.jitter = JITTER_CAP;
        // pulse_strength decays toward 0 over time
        s.pulse_strength = (s.pulse_strength * EMA_WEIGHT) / EMA_DIVISOR;
        return;
    }

    // ── Read current counter ──────────────────────────────────────────────────
    let cur_counter = unsafe { hpet_read64(HPET_COUNTER_OFFSET) };

    // Wrapping subtraction handles the u64 rollover correctly.
    let delta = cur_counter.wrapping_sub(s.prev_counter);
    s.counter_delta = delta;
    s.prev_counter  = cur_counter;

    // ── Heartbeat rate ────────────────────────────────────────────────────────
    // Rate = 1000 as long as the counter is advancing.  A delta of 0 means
    // the HPET has stalled (hardware fault or MMIO error) — rate drops to 0.
    s.heartbeat_rate = if delta > 0 { 1000 } else { 0 };

    // ── Jitter ───────────────────────────────────────────────────────────────
    // How much did this delta deviate from the previous delta?
    // A perfectly stable HPET gives identical deltas every poll → jitter = 0.
    // Large swings (CPU preemption, HPET suspend/resume) push jitter toward 1000.
    let deviation: u64 = if delta > s.prev_delta {
        delta - s.prev_delta
    } else {
        s.prev_delta - delta
    };

    // Scale deviation as a percentage of prev_delta, cap at 1000.
    // Integer only: (deviation * 100) / prev_delta.max(1)
    let jitter_raw = (deviation.saturating_mul(JITTER_SCALE))
        .checked_div(s.prev_delta.max(1))
        .unwrap_or(JITTER_CAP as u64);

    s.jitter = (jitter_raw.min(JITTER_CAP as u64)) as u16;

    // Carry delta forward for next poll's jitter computation.
    if delta > 0 {
        s.prev_delta = delta;
    }

    // ── Pulse strength (EMA) ─────────────────────────────────────────────────
    // Exponential moving average with weight 7/8 — smooths transient stalls.
    s.pulse_strength =
        (s.pulse_strength as u32 * EMA_WEIGHT as u32 + s.heartbeat_rate as u32)
        as u16 / EMA_DIVISOR;

    // ── Beat count ───────────────────────────────────────────────────────────
    if s.heartbeat_rate > 0 {
        s.beat_count = s.beat_count.saturating_add(1);
    }

    // Periodic serial log (every 1024 beats to avoid flooding)
    if s.beat_count > 0 && s.beat_count % 1024 == 0 {
        serial_println!(
            "[hpet] rate={} jitter={} pulse={} beats={}",
            s.heartbeat_rate, s.jitter, s.pulse_strength, s.beat_count
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 1000 when HPET counter is advancing; 0 when stalled.
pub fn heartbeat_rate()    -> u16  { STATE.lock().heartbeat_rate }

/// Delta-to-delta deviation (0 = perfect rhythm, 1000 = chaotic or absent).
pub fn jitter()            -> u16  { STATE.lock().jitter }

/// Exponential moving average of heartbeat_rate (smoother view of pulse health).
pub fn pulse_strength()    -> u16  { STATE.lock().pulse_strength }

/// Total heartbeats counted since init (advances while HPET is ticking).
pub fn beat_count()        -> u32  { STATE.lock().beat_count }

/// True when HPET was found and validated at init.
pub fn hpet_available()    -> bool { STATE.lock().hpet_available }

/// The hardware counter period in femtoseconds (e.g. 69_841_279 ≈ 14.3 MHz).
pub fn counter_period_fs() -> u32  { STATE.lock().counter_period_fs }
