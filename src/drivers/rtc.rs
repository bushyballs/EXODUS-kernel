use crate::sync::Mutex;
/// Real-time clock driver for Genesis -- MC146818/CMOS RTC
///
/// The MC146818 (or compatible) RTC is accessed via CMOS I/O ports 0x70/0x71.
/// This driver implements:
///   - CMOS register read/write with NMI-safe port access
///   - BCD <-> binary conversion for date/time fields
///   - Date/time get and set with update-in-progress guard
///   - Alarm configuration (hour/minute/second match)
///   - Periodic interrupt rate configuration
///   - Century register support (register 0x32)
///   - Day-of-week calculation (Tomohiko Sakamoto's algorithm)
///
/// Reference: MC146818A datasheet, OSDev wiki CMOS article.
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// CMOS I/O ports
// ---------------------------------------------------------------------------

/// CMOS address port (write index here; bit 7 controls NMI)
const CMOS_ADDR: u16 = 0x70;
/// CMOS data port (read/write data for selected register)
const CMOS_DATA: u16 = 0x71;

// ---------------------------------------------------------------------------
// CMOS RTC register indices
// ---------------------------------------------------------------------------

const REG_SECONDS: u8 = 0x00;
const REG_SECONDS_ALARM: u8 = 0x01;
const REG_MINUTES: u8 = 0x02;
const REG_MINUTES_ALARM: u8 = 0x03;
const REG_HOURS: u8 = 0x04;
const REG_HOURS_ALARM: u8 = 0x05;
const REG_DAY_OF_WEEK: u8 = 0x06;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;
const REG_STATUS_C: u8 = 0x0C;
const REG_STATUS_D: u8 = 0x0D;
const REG_CENTURY: u8 = 0x32;

// ---------------------------------------------------------------------------
// Status register B bits
// ---------------------------------------------------------------------------

/// Set = 24-hour mode, clear = 12-hour mode
const STATUS_B_24HR: u8 = 0x02;
/// Set = binary mode, clear = BCD mode
const STATUS_B_BINARY: u8 = 0x04;
/// Set = daylight saving enable
const STATUS_B_DSE: u8 = 0x01;
/// Set = enable periodic interrupt
const STATUS_B_PIE: u8 = 0x40;
/// Set = enable alarm interrupt
const STATUS_B_AIE: u8 = 0x20;
/// Set = enable update-ended interrupt
const STATUS_B_UIE: u8 = 0x10;
/// Set = inhibit RTC updates (freeze clock for writing)
const STATUS_B_SET: u8 = 0x80;

// ---------------------------------------------------------------------------
// Status register A bits
// ---------------------------------------------------------------------------

/// Update in progress flag (bit 7)
const STATUS_A_UIP: u8 = 0x80;

// ---------------------------------------------------------------------------
// Alarm wildcard: match any value ("don't care")
// ---------------------------------------------------------------------------

const ALARM_WILDCARD: u8 = 0xC0;

// ---------------------------------------------------------------------------
// RTC time representation
// ---------------------------------------------------------------------------

/// RTC time representation
#[derive(Debug, Clone, Copy)]
pub struct RtcTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl RtcTime {
    const fn empty() -> Self {
        RtcTime {
            year: 2000,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    /// Day of week (0=Sunday, 6=Saturday) using Tomohiko Sakamoto's algorithm
    pub fn day_of_week(&self) -> u8 {
        let y = self.year as i32;
        let m = self.month as i32;
        let d = self.day as i32;
        // Lookup table for month offsets
        const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let y_adj = if m < 3 { y.saturating_sub(1) } else { y };
        let t_idx = ((m - 1) as usize).min(11); // clamp month index to table bounds
        ((y_adj + y_adj / 4 - y_adj / 100 + y_adj / 400 + T[t_idx] + d) % 7) as u8
    }
}

// ---------------------------------------------------------------------------
// BCD <-> binary conversion
// ---------------------------------------------------------------------------

/// Convert BCD-encoded byte to binary
fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F).saturating_add((bcd >> 4).saturating_mul(10))
}

/// Convert binary byte to BCD
fn bin_to_bcd(bin: u8) -> u8 {
    ((bin / 10) << 4) | (bin % 10)
}

// ---------------------------------------------------------------------------
// Low-level CMOS register access
// ---------------------------------------------------------------------------

/// Read a CMOS register. Preserves NMI disable bit (bit 7 of port 0x70).
fn cmos_read(reg: u8) -> u8 {
    // Bit 7 = 0 to keep NMI enabled; OR with register index
    crate::io::outb(CMOS_ADDR, reg & 0x7F);
    crate::io::io_wait();
    crate::io::inb(CMOS_DATA)
}

/// Write a CMOS register. Preserves NMI disable bit.
fn cmos_write(reg: u8, val: u8) {
    crate::io::outb(CMOS_ADDR, reg & 0x7F);
    crate::io::io_wait();
    crate::io::outb(CMOS_DATA, val);
}

/// Wait until the RTC update-in-progress flag clears.
/// The UIP flag is set for ~244 us while the RTC latches new values.
fn wait_for_uip_clear() {
    // First, wait until UIP is set (so we know an update is happening)
    // then wait until it clears. If it's already clear, just proceed.
    for _ in 0..10_000 {
        if cmos_read(REG_STATUS_A) & STATUS_A_UIP == 0 {
            return;
        }
        crate::io::io_wait();
    }
}

// ---------------------------------------------------------------------------
// Internal driver state
// ---------------------------------------------------------------------------

/// Internal state behind the Mutex
struct RtcDriver {
    /// Whether we detected a working RTC
    initialized: bool,
    /// Whether the RTC is in BCD mode (most common)
    bcd_mode: bool,
    /// Whether the RTC is in 24-hour mode
    h24_mode: bool,
    /// Cached last-read time
    cached_time: RtcTime,
    /// Whether an alarm is active
    alarm_active: bool,
    /// Alarm time (only hour/minute/second used)
    alarm_time: RtcTime,
    /// Periodic interrupt rate (0 = disabled, 3-15 = rate divider)
    periodic_rate: u8,
}

impl RtcDriver {
    const fn new() -> Self {
        RtcDriver {
            initialized: false,
            bcd_mode: true,
            h24_mode: true,
            cached_time: RtcTime::empty(),
            alarm_active: false,
            alarm_time: RtcTime::empty(),
            periodic_rate: 0,
        }
    }

    /// Decode a raw register value based on current BCD/binary mode
    fn decode(&self, raw: u8) -> u8 {
        if self.bcd_mode {
            bcd_to_bin(raw)
        } else {
            raw
        }
    }

    /// Encode a value for writing based on current BCD/binary mode
    fn encode(&self, val: u8) -> u8 {
        if self.bcd_mode {
            bin_to_bcd(val)
        } else {
            val
        }
    }

    /// Read the current time from CMOS registers.
    /// Reads twice and compares to avoid getting a torn update.
    fn read_time_raw(&self) -> RtcTime {
        // Read twice until consistent to avoid torn reads.
        // Bound to 10 attempts to prevent infinite loop on stuck hardware.
        for _attempt in 0..10 {
            wait_for_uip_clear();

            let sec1 = cmos_read(REG_SECONDS);
            let min1 = cmos_read(REG_MINUTES);
            let hr1 = cmos_read(REG_HOURS);
            let day1 = cmos_read(REG_DAY);
            let mon1 = cmos_read(REG_MONTH);
            let yr1 = cmos_read(REG_YEAR);
            let cen1 = cmos_read(REG_CENTURY);

            wait_for_uip_clear();

            let sec2 = cmos_read(REG_SECONDS);
            let min2 = cmos_read(REG_MINUTES);
            let hr2 = cmos_read(REG_HOURS);
            let day2 = cmos_read(REG_DAY);
            let mon2 = cmos_read(REG_MONTH);
            let yr2 = cmos_read(REG_YEAR);
            let cen2 = cmos_read(REG_CENTURY);

            if sec1 == sec2
                && min1 == min2
                && hr1 == hr2
                && day1 == day2
                && mon1 == mon2
                && yr1 == yr2
                && cen1 == cen2
            {
                let second = self.decode(sec1);
                let minute = self.decode(min1);
                let mut hour = hr1;

                // Handle 12-hour mode (bit 7 = PM flag in raw register)
                if !self.h24_mode {
                    let pm = hour & 0x80 != 0;
                    hour &= 0x7F;
                    hour = self.decode(hour);
                    if pm && hour < 12 {
                        hour = hour.saturating_add(12);
                    } else if !pm && hour == 12 {
                        hour = 0;
                    }
                } else {
                    hour = self.decode(hour);
                }

                let day = self.decode(day1);
                let month = self.decode(mon1);
                let year_low = self.decode(yr1);
                let century = self.decode(cen1);

                let year = if century > 0 {
                    (century as u16)
                        .saturating_mul(100)
                        .saturating_add(year_low as u16)
                } else {
                    // Assume 2000s if no century register data
                    2000u16.saturating_add(year_low as u16)
                };

                return RtcTime {
                    year,
                    month,
                    day,
                    hour,
                    minute,
                    second,
                };
            }
            // Torn read -- try again
        }
        // Fallback: return the last single read if consistency never achieved
        RtcTime::empty()
    }

    /// Write a time to the CMOS RTC registers.
    fn write_time_raw(&self, time: &RtcTime) {
        // Inhibit updates while writing
        let status_b = cmos_read(REG_STATUS_B);
        cmos_write(REG_STATUS_B, status_b | STATUS_B_SET);

        cmos_write(REG_SECONDS, self.encode(time.second));
        cmos_write(REG_MINUTES, self.encode(time.minute));
        cmos_write(REG_HOURS, self.encode(time.hour));
        cmos_write(REG_DAY, self.encode(time.day));
        cmos_write(REG_MONTH, self.encode(time.month));

        let year_low = (time.year % 100) as u8;
        let century = (time.year / 100) as u8;
        cmos_write(REG_YEAR, self.encode(year_low));
        cmos_write(REG_CENTURY, self.encode(century));

        // Update day of week register
        cmos_write(REG_DAY_OF_WEEK, time.day_of_week().saturating_add(1)); // RTC uses 1-7

        // Re-enable updates
        cmos_write(REG_STATUS_B, status_b & !STATUS_B_SET);
    }

    /// Configure the alarm registers.
    fn set_alarm_raw(&self, time: &RtcTime) {
        cmos_write(REG_SECONDS_ALARM, self.encode(time.second));
        cmos_write(REG_MINUTES_ALARM, self.encode(time.minute));
        cmos_write(REG_HOURS_ALARM, self.encode(time.hour));

        // Enable alarm interrupt in status register B
        let status_b = cmos_read(REG_STATUS_B);
        cmos_write(REG_STATUS_B, status_b | STATUS_B_AIE);
    }

    /// Disable the alarm interrupt.
    fn disable_alarm_raw(&self) {
        let status_b = cmos_read(REG_STATUS_B);
        cmos_write(REG_STATUS_B, status_b & !STATUS_B_AIE);
    }

    /// Set the periodic interrupt rate.
    /// rate = 0 disables, 3-15 sets divider (frequency = 32768 >> (rate-1)).
    fn set_periodic_rate_raw(&self, rate: u8) {
        let status_a = cmos_read(REG_STATUS_A);
        // Lower 4 bits of status A = rate selector
        cmos_write(REG_STATUS_A, (status_a & 0xF0) | (rate & 0x0F));

        let status_b = cmos_read(REG_STATUS_B);
        if rate > 0 {
            cmos_write(REG_STATUS_B, status_b | STATUS_B_PIE);
        } else {
            cmos_write(REG_STATUS_B, status_b & !STATUS_B_PIE);
        }
    }

    /// Acknowledge pending RTC interrupts by reading status C.
    /// Must be called in the IRQ 8 handler to allow further interrupts.
    fn ack_interrupt(&self) -> u8 {
        cmos_read(REG_STATUS_C)
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static RTC: Mutex<RtcDriver> = Mutex::new(RtcDriver::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the CMOS RTC driver.
/// Detects BCD/binary mode, 12/24-hour mode, reads current time.
pub fn init() {
    let mut drv = RTC.lock();

    // Read status registers to determine mode
    let status_b = cmos_read(REG_STATUS_B);
    let status_d = cmos_read(REG_STATUS_D);

    // Check if RTC has valid RAM (bit 7 of status D)
    if status_d & 0x80 == 0 {
        serial_println!("  RTC: CMOS battery dead or RTC not present");
        // Continue anyway -- values may be garbage but we can still set them
    }

    drv.bcd_mode = status_b & STATUS_B_BINARY == 0;
    drv.h24_mode = status_b & STATUS_B_24HR != 0;

    // Force 24-hour mode if not already set
    if !drv.h24_mode {
        cmos_write(REG_STATUS_B, status_b | STATUS_B_24HR);
        drv.h24_mode = true;
        serial_println!("  RTC: switched to 24-hour mode");
    }

    // Read initial time
    let time = drv.read_time_raw();
    drv.cached_time = time;
    drv.initialized = true;

    let dow_names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let dow = time.day_of_week() as usize;
    let dow_str = if dow < 7 { dow_names[dow] } else { "???" };

    serial_println!(
        "  RTC: {}-{:02}-{:02} {} {:02}:{:02}:{:02} ({})",
        time.year,
        time.month,
        time.day,
        dow_str,
        time.hour,
        time.minute,
        time.second,
        if drv.bcd_mode { "BCD" } else { "binary" }
    );

    drop(drv);
    super::register("rtc", super::DeviceType::Timer);

    // Sync the kernel wall clock to the hardware RTC time.
    rtc_sync_wallclock();
}

/// Read the current date and time from the RTC.
pub fn read_time() -> RtcTime {
    let mut drv = RTC.lock();
    if !drv.initialized {
        return RtcTime::empty();
    }
    let time = drv.read_time_raw();
    drv.cached_time = time;
    time
}

/// Set the RTC date and time.
pub fn set_time(time: &RtcTime) -> Result<(), ()> {
    let drv = RTC.lock();
    if !drv.initialized {
        return Err(());
    }
    // Basic validation
    if time.month == 0
        || time.month > 12
        || time.day == 0
        || time.day > 31
        || time.hour > 23
        || time.minute > 59
        || time.second > 59
    {
        return Err(());
    }
    drv.write_time_raw(time);
    serial_println!(
        "  RTC: time set to {}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.year,
        time.month,
        time.day,
        time.hour,
        time.minute,
        time.second
    );
    Ok(())
}

/// Set a one-shot alarm at the given hour:minute:second.
/// When the alarm fires, IRQ 8 will be raised.
pub fn set_alarm(time: &RtcTime) -> Result<(), ()> {
    let mut drv = RTC.lock();
    if !drv.initialized {
        return Err(());
    }
    if time.hour > 23 || time.minute > 59 || time.second > 59 {
        return Err(());
    }
    drv.set_alarm_raw(time);
    drv.alarm_active = true;
    drv.alarm_time = *time;
    serial_println!(
        "  RTC: alarm set for {:02}:{:02}:{:02}",
        time.hour,
        time.minute,
        time.second
    );
    Ok(())
}

/// Disable the RTC alarm.
pub fn disable_alarm() {
    let mut drv = RTC.lock();
    if !drv.initialized {
        return;
    }
    drv.disable_alarm_raw();
    drv.alarm_active = false;
}

/// Set the periodic interrupt rate.
/// rate=0 disables. rate=6 gives 1024 Hz. rate=15 gives 2 Hz.
/// Frequency = 32768 >> (rate - 1) for rate in 3..=15.
pub fn set_periodic_rate(rate: u8) -> Result<(), ()> {
    let mut drv = RTC.lock();
    if !drv.initialized {
        return Err(());
    }
    if rate != 0 && (rate < 3 || rate > 15) {
        return Err(());
    }
    drv.set_periodic_rate_raw(rate);
    drv.periodic_rate = rate;
    if rate > 0 {
        let freq = 32768u32 >> (rate as u32 - 1);
        serial_println!("  RTC: periodic interrupt rate={} ({}Hz)", rate, freq);
    } else {
        serial_println!("  RTC: periodic interrupt disabled");
    }
    Ok(())
}

/// Acknowledge RTC interrupt (call from IRQ 8 handler).
/// Returns the status C register value indicating which interrupt(s) fired:
///   bit 4 = update-ended, bit 5 = alarm, bit 6 = periodic.
pub fn ack_interrupt() -> u8 {
    let drv = RTC.lock();
    drv.ack_interrupt()
}

/// Check if the alarm is currently active.
pub fn alarm_active() -> bool {
    RTC.lock().alarm_active
}

/// Get the cached time from the last read (no hardware access).
pub fn cached_time() -> RtcTime {
    RTC.lock().cached_time
}

/// Check if the RTC is initialized.
pub fn is_initialized() -> bool {
    RTC.lock().initialized
}

// ---------------------------------------------------------------------------
// Unix timestamp conversion
// ---------------------------------------------------------------------------

/// Days in each month (non-leap).  Index 0 is unused; 1 = January … 12 = December.
const DAYS_IN_MONTH: [u64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// True if `year` is a Gregorian leap year.
fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Compute days elapsed from 1970-01-01 to `year`-`month`-`day` (1-based month/day).
///
/// Pure integer arithmetic — no float casts, no heap, no panic.
fn days_from_epoch(year: u16, month: u8, day: u8) -> u64 {
    let mut days: u64 = 0;

    // Full years 1970 … (year-1)
    for y in 1970u16..year {
        days = days.saturating_add(if is_leap_year(y) { 366 } else { 365 });
    }

    // Full months in `year` before `month`
    let m = month.min(12) as usize;
    for mi in 1..m {
        days = days.saturating_add(DAYS_IN_MONTH[mi]);
        if mi == 2 && is_leap_year(year) {
            days = days.saturating_add(1);
        }
    }

    // Days within the current month (day is 1-based)
    if day > 0 {
        days = days.saturating_add((day - 1) as u64);
    }

    days
}

/// Convert an `RtcTime` to a Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
///
/// This function uses integer-only date arithmetic and is safe to call from
/// any context (no alloc, no float, no panic).
pub fn rtc_to_unix(t: &RtcTime) -> u64 {
    let days = days_from_epoch(t.year, t.month, t.day);
    days.saturating_mul(86_400)
        .saturating_add((t.hour as u64).saturating_mul(3_600))
        .saturating_add((t.minute as u64).saturating_mul(60))
        .saturating_add(t.second as u64)
}

/// Read the hardware RTC and set the kernel wall clock to match.
///
/// Called early in boot (before the network stack is available) so the
/// system has a sane baseline time even without NTP.
pub fn rtc_sync_wallclock() {
    let t = read_time();
    let unix = rtc_to_unix(&t);
    crate::time::clock::set_wallclock_secs(unix);
    serial_println!(
        "  RTC: wall clock synced — unix={} ({}-{:02}-{:02} {:02}:{:02}:{:02})",
        unix,
        t.year,
        t.month,
        t.day,
        t.hour,
        t.minute,
        t.second
    );
}
