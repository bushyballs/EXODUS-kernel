use crate::io::{inb, outb};
use crate::sync::Mutex;
/// CMOS Real-Time Clock driver
///
/// Reads date/time from the MC146818 RTC chip via CMOS ports 0x70/0x71.
/// Handles BCD to binary conversion and century register.
use crate::{serial_print, serial_println};

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

static CURRENT_TIME: Mutex<DateTime> = Mutex::new(DateTime::zero());

#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub day_of_week: u8,
}

impl DateTime {
    const fn zero() -> Self {
        DateTime {
            year: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            day_of_week: 0,
        }
    }

    /// Unix timestamp (seconds since 1970-01-01)
    pub fn to_unix(&self) -> u64 {
        let mut days: u64 = 0;
        // Years
        for y in 1970..self.year {
            days += if is_leap(y) { 366 } else { 365 };
        }
        // Months
        let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for m in 1..self.month {
            days += month_days[m as usize] as u64;
            if m == 2 && is_leap(self.year) {
                days += 1;
            }
        }
        // Days
        days += (self.day - 1) as u64;

        days * 86400 + self.hour as u64 * 3600 + self.minute as u64 * 60 + self.second as u64
    }
}

fn is_leap(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Read a CMOS register
fn cmos_read(reg: u8) -> u8 {
    outb(CMOS_ADDR, reg);
    inb(CMOS_DATA)
}

/// Check if RTC update is in progress
fn update_in_progress() -> bool {
    cmos_read(0x0A) & 0x80 != 0
}

/// Convert BCD to binary
fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}

/// Read current date/time from RTC
pub fn read() -> DateTime {
    // Wait for update to finish
    while update_in_progress() {}

    let reg_b = cmos_read(0x0B);
    let is_bcd = reg_b & 0x04 == 0;
    let is_24h = reg_b & 0x02 != 0;

    let mut second = cmos_read(0x00);
    let mut minute = cmos_read(0x02);
    let mut hour = cmos_read(0x04);
    let mut day = cmos_read(0x07);
    let mut month = cmos_read(0x08);
    let mut year_lo = cmos_read(0x09);
    let day_of_week = cmos_read(0x06);
    let century = cmos_read(0x32); // century register (if available)

    if is_bcd {
        second = bcd_to_bin(second);
        minute = bcd_to_bin(minute);
        hour = bcd_to_bin(hour & 0x7F) | (hour & 0x80); // preserve PM bit
        day = bcd_to_bin(day);
        month = bcd_to_bin(month);
        year_lo = bcd_to_bin(year_lo);
    }

    // Handle 12-hour format
    if !is_24h && hour & 0x80 != 0 {
        hour = ((hour & 0x7F) + 12) % 24;
    }

    let year = if century != 0 {
        bcd_to_bin(century) as u16 * 100 + year_lo as u16
    } else {
        2000 + year_lo as u16
    };

    let dt = DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        day_of_week,
    };
    *CURRENT_TIME.lock() = dt;
    dt
}

pub fn init() {
    let dt = read();
    serial_println!(
        "    [rtc] {}-{:02}-{:02} {:02}:{:02}:{:02}",
        dt.year,
        dt.month,
        dt.day,
        dt.hour,
        dt.minute,
        dt.second
    );
}

/// Get cached current time
pub fn now() -> DateTime {
    *CURRENT_TIME.lock()
}
