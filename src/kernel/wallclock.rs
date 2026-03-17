use crate::serial_println;
/// Kernel wall clock — Genesis AIOS
///
/// Tracks Unix time (seconds since 1970-01-01 00:00:00 UTC) with millisecond
/// sub-second resolution derived from the TSC.
///
/// Two orthogonal time sources:
///   1. **Wall clock** — absolute UTC time set from the hardware RTC at boot
///      and adjusted by NTP when available.
///   2. **Monotonic clock** — TSC-based, never goes backwards, used for
///      measuring elapsed durations regardless of wall clock corrections.
///
/// Constraints (bare-metal #![no_std]):
///   - No float casts (as f32 / as f64)
///   - No heap (no Vec / Box / String)
///   - No panic — early return / serial_println! on unexpected inputs
///   - Saturating arithmetic on all counters, wrapping_add on TSC deltas
///
/// Integration:
///   - `init()`            — call early in boot, before net stack
///   - `set_wallclock_secs()` — called by `drivers::rtc::rtc_sync_wallclock()`
///                              and `net::ntp::ntp_sync()`
///   - `get_wallclock_secs()` / `get_wallclock_ms()` — general-purpose wall time
///   - `clock_gettime_realtime()` / `clock_gettime_monotonic()` — POSIX-style
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Fallback TSC frequency
// ---------------------------------------------------------------------------

/// Assumed TSC frequency when calibration has not run yet (3 GHz).
/// The calibrated value from `time::clock::tsc_freq_hz()` is used whenever
/// it is non-zero.
const FALLBACK_TSC_HZ: u64 = 3_000_000_000;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Wall clock state: epoch (Unix seconds) locked in at the last set_wallclock_secs
/// call, paired with the TSC value at that moment.
struct WallClockState {
    /// Unix seconds at the instant `tsc_at_epoch` was captured.
    epoch_secs: u64,
    /// TSC value when `epoch_secs` was set.
    tsc_at_epoch: u64,
}

impl WallClockState {
    const fn new() -> Self {
        WallClockState {
            epoch_secs: 0,
            tsc_at_epoch: 0,
        }
    }
}

static WALL: Mutex<WallClockState> = Mutex::new(WallClockState::new());

// ---------------------------------------------------------------------------
// TSC helpers
// ---------------------------------------------------------------------------

/// Read the 64-bit Time Stamp Counter.
#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Effective TSC frequency: calibrated value if available, else fallback.
#[inline]
fn tsc_hz() -> u64 {
    let f = crate::time::clock::tsc_freq_hz();
    if f == 0 {
        FALLBACK_TSC_HZ
    } else {
        f
    }
}

/// Convert a TSC delta (cycles) to whole seconds.
///
/// Uses integer division: secs = delta / hz.
/// No float, no overflow for reasonable deltas (< 584 years at 3 GHz).
#[inline]
fn tsc_delta_to_secs(delta: u64, hz: u64) -> u64 {
    if hz == 0 {
        return 0;
    }
    delta / hz
}

/// Convert a TSC delta (cycles) to milliseconds.
///
/// ms = delta * 1000 / hz
/// To avoid overflow we compute:  delta / (hz / 1000), clamping hz/1000 ≥ 1.
#[inline]
fn tsc_delta_to_ms(delta: u64, hz: u64) -> u64 {
    if hz == 0 {
        return 0;
    }
    let hz_per_ms = hz / 1000;
    if hz_per_ms == 0 {
        return delta;
    } // hz < 1 kHz — very unlikely
    delta / hz_per_ms
}

/// Convert a TSC delta (cycles) to microseconds.
#[inline]
fn tsc_delta_to_us(delta: u64, hz: u64) -> u64 {
    if hz == 0 {
        return 0;
    }
    let hz_per_us = hz / 1_000_000;
    if hz_per_us == 0 {
        // hz < 1 MHz — compute via ms path
        return tsc_delta_to_ms(delta, hz).saturating_mul(1000);
    }
    delta / hz_per_us
}

/// Convert a TSC delta (cycles) to nanoseconds.
///
/// ns = delta * 1_000_000_000 / hz.
/// To avoid 64-bit overflow we split the delta:
///   ns = (delta / hz) * 1_000_000_000 + ((delta % hz) * 1_000_000_000 / hz)
#[inline]
fn tsc_delta_to_ns(delta: u64, hz: u64) -> u64 {
    if hz == 0 {
        return 0;
    }
    let whole_secs = delta / hz;
    let remainder = delta % hz;
    // remainder < hz, so remainder * 1_000_000_000 can overflow if hz > ~18e9.
    // Use a safe path: remainder * 1_000_000 / hz * 1000
    let ns_frac = if remainder <= u64::MAX / 1_000_000_000 {
        remainder.saturating_mul(1_000_000_000) / hz
    } else {
        // Reduce precision to avoid overflow
        (remainder / 1000).saturating_mul(1_000_000) / (hz / 1000).max(1)
    };
    whole_secs
        .saturating_mul(1_000_000_000)
        .saturating_add(ns_frac)
}

// ---------------------------------------------------------------------------
// POSIX-compatible time structures
// ---------------------------------------------------------------------------

/// Equivalent to `struct timeval` (POSIX).
#[derive(Debug, Clone, Copy)]
pub struct Timeval {
    pub sec: u64,
    pub usec: u64,
}

/// Equivalent to `struct timespec` (POSIX).
#[derive(Debug, Clone, Copy)]
pub struct Timespec {
    pub sec: u64,
    pub nsec: u64,
}

// ---------------------------------------------------------------------------
// Core API
// ---------------------------------------------------------------------------

/// Set (or correct) the wall clock to `unix_secs`.
///
/// Snaps both the stored Unix epoch and the current TSC value.  Subsequent
/// calls to `get_wallclock_secs()` will compute:
///   result = unix_secs + elapsed_tsc_seconds_since_now
pub fn set_wallclock_secs(unix_secs: u64) {
    let tsc_now = rdtsc();
    let mut state = WALL.lock();
    state.epoch_secs = unix_secs;
    state.tsc_at_epoch = tsc_now;
    // Also update time::clock so unix_time() stays consistent
    crate::time::clock::set_wallclock_secs(unix_secs);
}

/// Get the current wall clock time as a Unix timestamp in whole seconds.
pub fn get_wallclock_secs() -> u64 {
    let tsc_now = rdtsc();
    let hz = tsc_hz();
    let (epoch, tsc_base) = {
        let s = WALL.lock();
        (s.epoch_secs, s.tsc_at_epoch)
    };
    let elapsed_secs = tsc_delta_to_secs(tsc_now.wrapping_sub(tsc_base), hz);
    epoch.saturating_add(elapsed_secs)
}

/// Get the current wall clock time in milliseconds (Unix epoch × 1000 + ms).
pub fn get_wallclock_ms() -> u64 {
    let tsc_now = rdtsc();
    let hz = tsc_hz();
    let (epoch, tsc_base) = {
        let s = WALL.lock();
        (s.epoch_secs, s.tsc_at_epoch)
    };
    let elapsed_ms = tsc_delta_to_ms(tsc_now.wrapping_sub(tsc_base), hz);
    epoch.saturating_mul(1000).saturating_add(elapsed_ms)
}

/// Return milliseconds elapsed since kernel boot (monotonic).
///
/// Uses `time::clock::uptime_ms()` which is driven by the PIT timer interrupt.
pub fn time_since_boot_ms() -> u64 {
    crate::time::clock::uptime_ms()
}

// ---------------------------------------------------------------------------
// POSIX-style getters
// ---------------------------------------------------------------------------

/// Return the current wall clock time as a `Timeval` (seconds + microseconds).
pub fn gettimeofday() -> Timeval {
    let tsc_now = rdtsc();
    let hz = tsc_hz();
    let (epoch, tsc_base) = {
        let s = WALL.lock();
        (s.epoch_secs, s.tsc_at_epoch)
    };
    let delta = tsc_now.wrapping_sub(tsc_base);
    let elapsed_secs = tsc_delta_to_secs(delta, hz);
    // Sub-second residual TSC cycles
    let residual_cycles = delta.wrapping_sub(elapsed_secs.saturating_mul(hz));
    let usec = tsc_delta_to_us(residual_cycles, hz).min(999_999);
    Timeval {
        sec: epoch.saturating_add(elapsed_secs),
        usec,
    }
}

/// Monotonic clock (CLOCK_MONOTONIC) — TSC-based, never goes backwards.
pub fn clock_gettime_monotonic() -> Timespec {
    let tsc_now = rdtsc();
    // We store boot TSC inside time::clock; if it is zero we just use tsc_now
    // directly scaled (accurate enough for a monotonic source).
    let hz = tsc_hz();
    let boot_tsc = crate::time::clock::boot_tsc();
    let delta = tsc_now.wrapping_sub(boot_tsc);
    let secs = tsc_delta_to_secs(delta, hz);
    let residual = delta.wrapping_sub(secs.saturating_mul(hz));
    let nsec = tsc_delta_to_ns(residual, hz).min(999_999_999);
    Timespec { sec: secs, nsec }
}

/// Real-time clock (CLOCK_REALTIME) — wall clock with nanosecond resolution.
pub fn clock_gettime_realtime() -> Timespec {
    let tsc_now = rdtsc();
    let hz = tsc_hz();
    let (epoch, tsc_base) = {
        let s = WALL.lock();
        (s.epoch_secs, s.tsc_at_epoch)
    };
    let delta = tsc_now.wrapping_sub(tsc_base);
    let elapsed_secs = tsc_delta_to_secs(delta, hz);
    let residual = delta.wrapping_sub(elapsed_secs.saturating_mul(hz));
    let nsec = tsc_delta_to_ns(residual, hz).min(999_999_999);
    Timespec {
        sec: epoch.saturating_add(elapsed_secs),
        nsec,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the wall clock.
///
/// Seeds `epoch_secs` from the hardware RTC via `time::clock::unix_time()`
/// (which is already set by `time::rtc::init()` at this point in boot).
/// Records the current TSC as the epoch baseline.
pub fn init() {
    let unix_now = crate::time::clock::unix_time();
    let tsc_now = rdtsc();
    {
        let mut state = WALL.lock();
        state.epoch_secs = unix_now;
        state.tsc_at_epoch = tsc_now;
    }
    serial_println!(
        "  Wallclock: epoch={}s TSC-based realtime+monotonic ready",
        unix_now
    );
}
