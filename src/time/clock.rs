use crate::io::{hlt, inb, outb};
use crate::sync::Mutex;
/// System clock — monotonic and wall clock time
///
/// Monotonic clock: never goes backwards, based on PIT/TSC
/// Wall clock: actual time of day, synced from RTC
use crate::{serial_print, serial_println};

static SYSTEM_CLOCK: Mutex<SystemClock> = Mutex::new(SystemClock::new());

/// Ticks per second (PIT frequency)
pub const TICKS_PER_SEC: u64 = 1000; // 1ms resolution

pub struct SystemClock {
    /// Monotonic tick counter (incremented by timer interrupt)
    pub ticks: u64,
    /// Boot time in Unix seconds (from RTC at boot)
    pub boot_time_unix: u64,
    /// TSC frequency (cycles per second, calibrated at boot)
    pub tsc_freq: u64,
    /// TSC value at the moment init() ran (used as monotonic origin)
    pub boot_tsc: u64,
}

impl SystemClock {
    const fn new() -> Self {
        SystemClock {
            ticks: 0,
            boot_time_unix: 0,
            tsc_freq: 0,
            boot_tsc: 0,
        }
    }
}

/// Called by the timer interrupt handler every tick
pub fn tick() {
    let mut clk = SYSTEM_CLOCK.lock();
    clk.ticks = clk.ticks.saturating_add(1);
}

/// Get monotonic time in milliseconds since boot
pub fn uptime_ms() -> u64 {
    SYSTEM_CLOCK.lock().ticks
}

/// Get monotonic time in seconds since boot
pub fn uptime_secs() -> u64 {
    SYSTEM_CLOCK.lock().ticks / TICKS_PER_SEC
}

/// Get wall clock time as Unix timestamp
pub fn unix_time() -> u64 {
    let clock = SYSTEM_CLOCK.lock();
    clock.boot_time_unix + clock.ticks / TICKS_PER_SEC
}

/// Sleep for approximately the given number of milliseconds
/// (busy-wait — proper sleep needs scheduler integration)
pub fn sleep_ms(ms: u64) {
    let target = uptime_ms() + ms;
    while uptime_ms() < target {
        hlt();
    }
}

/// Calibrate TSC frequency using PIT
fn calibrate_tsc() -> u64 {
    // Read TSC before and after a known PIT delay
    let start: u64;
    let end: u64;

    unsafe {
        // Program PIT channel 2 for one-shot, ~10ms
        // Enable PIT channel 2 gate
        let gate: u8 = inb(0x61);
        outb(0x61, (gate & 0xFC) | 0x01);

        // Mode 0, lobyte/hibyte, channel 2
        outb(0x43, 0xB0);

        // Count value for ~10ms at 1.193182 MHz = 11932
        outb(0x42, (11932 & 0xFF) as u8);
        outb(0x42, (11932 >> 8) as u8);

        // Read TSC
        core::arch::asm!("rdtsc", out("eax") _, out("edx") _, options(nomem, nostack));
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
        start = ((hi as u64) << 32) | (lo as u64);

        // Wait for PIT to count down
        while inb(0x61) & 0x20 == 0 {}

        let lo2: u32;
        let hi2: u32;
        core::arch::asm!("rdtsc", out("eax") lo2, out("edx") hi2);
        end = ((hi2 as u64) << 32) | (lo2 as u64);

        // Restore gate
        outb(0x61, gate);
    }

    // TSC delta over 10ms -> multiply by 100 for per-second
    (end - start) * 100
}

/// Get TSC frequency in MHz
pub fn tsc_freq_mhz() -> u64 {
    SYSTEM_CLOCK.lock().tsc_freq / 1_000_000
}

/// Get TSC frequency in Hz (cycles per second).
/// Returns the calibrated value, or 0 if init has not run yet.
pub fn tsc_freq_hz() -> u64 {
    SYSTEM_CLOCK.lock().tsc_freq
}

/// Return the raw TSC value captured during `init()`.
/// Used by `kernel::wallclock` as the monotonic clock origin.
pub fn boot_tsc() -> u64 {
    SYSTEM_CLOCK.lock().boot_tsc
}

/// Override the wall clock's boot-time Unix epoch.
///
/// Called by the NTP client and `kernel::wallclock` after a successful
/// time sync.  The ticks counter is left unchanged; the next call to
/// `unix_time()` will reflect the new epoch.
pub fn set_wallclock_secs(unix_secs: u64) {
    let mut clock = SYSTEM_CLOCK.lock();
    // Adjust boot_time_unix so that  boot_time_unix + ticks/TPSec == unix_secs
    let elapsed = clock.ticks / TICKS_PER_SEC;
    clock.boot_time_unix = unix_secs.saturating_sub(elapsed);
}

/// Read TSC inline (duplicated here to avoid circular dependency with wallclock).
#[inline]
fn read_tsc() -> u64 {
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

pub fn init() {
    let rtc_time = super::rtc::read();
    let tsc_now = read_tsc();
    let mut clock = SYSTEM_CLOCK.lock();
    clock.boot_time_unix = rtc_time.to_unix();
    clock.tsc_freq = calibrate_tsc();
    clock.boot_tsc = tsc_now;
    serial_println!(
        "    [clock] System clock: boot_time={}, tsc_freq={}Hz",
        clock.boot_time_unix,
        clock.tsc_freq
    );
}
