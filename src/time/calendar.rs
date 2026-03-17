/// Calendar calculations for Genesis
///
/// Day-of-week, leap year, date arithmetic, recurring events, and
/// ISO 8601 formatting/parsing. Operates on Unix timestamps and
/// a decomposed Date struct. No floating-point — all math is integer.
///
/// All code is original.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

// ---------------------------------------------------------------------------
// Date / Time structs
// ---------------------------------------------------------------------------

/// A calendar date (year, month, day)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,  // 1..12
    pub day: u8,    // 1..31
}

/// A calendar date + time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub date: Date,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Day of week
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Weekday {
    Sunday = 0,
    Monday = 1,
    Tuesday = 2,
    Wednesday = 3,
    Thursday = 4,
    Friday = 5,
    Saturday = 6,
}

impl Weekday {
    pub fn from_u8(v: u8) -> Self {
        match v % 7 {
            0 => Weekday::Sunday,
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            _ => Weekday::Saturday,
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            Weekday::Sunday    => "Sun",
            Weekday::Monday    => "Mon",
            Weekday::Tuesday   => "Tue",
            Weekday::Wednesday => "Wed",
            Weekday::Thursday  => "Thu",
            Weekday::Friday    => "Fri",
            Weekday::Saturday  => "Sat",
        }
    }

    pub fn long_name(&self) -> &'static str {
        match self {
            Weekday::Sunday    => "Sunday",
            Weekday::Monday    => "Monday",
            Weekday::Tuesday   => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday  => "Thursday",
            Weekday::Friday    => "Friday",
            Weekday::Saturday  => "Saturday",
        }
    }
}

// ---------------------------------------------------------------------------
// Leap year
// ---------------------------------------------------------------------------

/// Test if a year is a leap year
pub fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days in a given year
pub fn days_in_year(year: u16) -> u16 {
    if is_leap_year(year) { 366 } else { 365 }
}

/// Days in a given month (1-indexed)
pub fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 => 31,
        2 => if is_leap_year(year) { 29 } else { 28 },
        3 => 31, 4 => 30, 5 => 31, 6 => 30,
        7 => 31, 8 => 31, 9 => 30, 10 => 31, 11 => 30, 12 => 31,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Day of week (Tomohiko Sakamoto)
// ---------------------------------------------------------------------------

/// Calculate day of week for a given date
pub fn day_of_week(date: &Date) -> Weekday {
    let t: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = date.year as i32;
    if date.month < 3 { y -= 1; }
    let dow = ((y + y / 4 - y / 100 + y / 400
                + t[(date.month - 1) as usize]
                + date.day as i32) % 7) as u8;
    Weekday::from_u8(dow)
}

// ---------------------------------------------------------------------------
// Unix timestamp <-> Date conversion
// ---------------------------------------------------------------------------

const SECS_PER_DAY: u64 = 86400;

/// Convert a Unix timestamp to a DateTime
pub fn from_unix(unix: u64) -> DateTime {
    let total_secs = unix;
    let total_days = (total_secs / SECS_PER_DAY) as u32;
    let remaining = (total_secs % SECS_PER_DAY) as u32;

    let hour = (remaining / 3600) as u8;
    let minute = ((remaining % 3600) / 60) as u8;
    let second = (remaining % 60) as u8;

    let mut y: u16 = 1970;
    let mut d = total_days;
    loop {
        let yd = days_in_year(y) as u32;
        if d < yd { break; }
        d -= yd;
        y += 1;
    }

    let mut m: u8 = 1;
    loop {
        let md = days_in_month(y, m) as u32;
        if d < md { break; }
        d -= md;
        m += 1;
        if m > 12 { break; }
    }
    let day = (d + 1) as u8;

    DateTime {
        date: Date { year: y, month: m, day },
        hour, minute, second,
    }
}

/// Convert a DateTime to a Unix timestamp
pub fn to_unix(dt: &DateTime) -> u64 {
    let mut days: u64 = 0;

    // Years since 1970
    for y in 1970..dt.date.year {
        days += days_in_year(y) as u64;
    }

    // Months
    for m in 1..dt.date.month {
        days += days_in_month(dt.date.year, m) as u64;
    }

    // Days (1-based)
    days += (dt.date.day.saturating_sub(1)) as u64;

    days * SECS_PER_DAY
        + dt.hour as u64 * 3600
        + dt.minute as u64 * 60
        + dt.second as u64
}

// ---------------------------------------------------------------------------
// Day of year, ISO week
// ---------------------------------------------------------------------------

/// Day-of-year (1-366)
pub fn day_of_year(date: &Date) -> u16 {
    let mut doy: u16 = 0;
    for m in 1..date.month {
        doy += days_in_month(date.year, m) as u16;
    }
    doy + date.day as u16
}

/// ISO 8601 week number (1-53) and the ISO year it belongs to
pub fn iso_week(date: &Date) -> (u16, u8) {
    // ISO week: week containing the year's first Thursday
    // Monday is day 1, Sunday is day 7
    let jan4 = Date { year: date.year, month: 1, day: 4 };
    let jan4_dow = day_of_week(&jan4);
    // Monday=1 .. Sunday=7
    let jan4_iso = if jan4_dow as u8 == 0 { 7 } else { jan4_dow as u8 };

    // Day of year for the Monday of the week containing Jan 4
    let week1_monday_doy: i32 = 4 - jan4_iso as i32 + 1; // can be 0 or negative

    let cur_doy = day_of_year(date) as i32;
    let diff = cur_doy - week1_monday_doy;

    if diff < 0 {
        // Belongs to the last week of the previous year
        let prev_dec31 = Date { year: date.year - 1, month: 12, day: 31 };
        let (_, wk) = iso_week(&prev_dec31);
        return (date.year - 1, wk);
    }

    let week = (diff / 7 + 1) as u8;

    if week > 52 {
        // Check if this actually belongs to week 1 of the next year
        let dec31 = Date { year: date.year, month: 12, day: 31 };
        let dec31_dow = day_of_week(&dec31);
        let dec31_iso = if dec31_dow as u8 == 0 { 7 } else { dec31_dow as u8 };
        if dec31_iso < 4 {
            return (date.year + 1, 1);
        }
    }

    (date.year, week)
}

// ---------------------------------------------------------------------------
// Date arithmetic
// ---------------------------------------------------------------------------

/// Add days to a date (can be negative via wrapping, but typically positive)
pub fn add_days(date: &Date, days: i32) -> Date {
    if days == 0 { return *date; }

    // Convert to day count, add, convert back
    let unix = to_unix(&DateTime {
        date: *date, hour: 12, minute: 0, second: 0,
    });

    let new_unix = if days > 0 {
        unix + (days as u64) * SECS_PER_DAY
    } else {
        unix.saturating_sub((-days) as u64 * SECS_PER_DAY)
    };

    from_unix(new_unix).date
}

/// Add months to a date (clamps day to valid range)
pub fn add_months(date: &Date, months: i32) -> Date {
    let total_months = (date.year as i32) * 12 + (date.month as i32 - 1) + months;
    let new_year = (total_months / 12) as u16;
    let new_month = ((total_months % 12) + 1) as u8;
    let max_day = days_in_month(new_year, new_month);
    let new_day = if date.day > max_day { max_day } else { date.day };
    Date { year: new_year, month: new_month, day: new_day }
}

/// Difference in days between two dates (a - b)
pub fn diff_days(a: &Date, b: &Date) -> i32 {
    let ua = to_unix(&DateTime { date: *a, hour: 0, minute: 0, second: 0 });
    let ub = to_unix(&DateTime { date: *b, hour: 0, minute: 0, second: 0 });
    ((ua as i64 - ub as i64) / SECS_PER_DAY as i64) as i32
}

// ---------------------------------------------------------------------------
// Recurring events
// ---------------------------------------------------------------------------

/// Recurrence pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recurrence {
    /// Every N days
    Daily(u16),
    /// Every N weeks on a given weekday
    Weekly(u16, Weekday),
    /// Every N months on a given day-of-month
    Monthly(u16, u8),
    /// Every N years on month/day
    Yearly(u16, u8, u8),
}

/// A named recurring event
#[derive(Debug, Clone)]
pub struct RecurringEvent {
    pub id: u32,
    pub name: String,
    pub start: Date,
    pub recurrence: Recurrence,
    pub max_occurrences: u32, // 0 = unlimited
}

/// Calendar system state
pub struct CalendarSystem {
    pub events: Vec<RecurringEvent>,
    next_id: u32,
}

impl CalendarSystem {
    const fn new() -> Self {
        CalendarSystem { events: Vec::new(), next_id: 1 }
    }

    pub fn add_event(&mut self, name: &str, start: Date, recurrence: Recurrence,
                     max_occurrences: u32) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.events.push(RecurringEvent {
            id,
            name: String::from(name),
            start,
            recurrence,
            max_occurrences,
        });
        id
    }

    pub fn remove_event(&mut self, id: u32) -> bool {
        if let Some(pos) = self.events.iter().position(|e| e.id == id) {
            self.events.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get next occurrence of an event on or after `from`
    pub fn next_occurrence(&self, event: &RecurringEvent, from: &Date) -> Option<Date> {
        let mut candidate = event.start;
        let mut count = 0u32;

        loop {
            if event.max_occurrences > 0 && count >= event.max_occurrences {
                return None;
            }

            if diff_days(&candidate, from) >= 0 {
                return Some(candidate);
            }

            candidate = match event.recurrence {
                Recurrence::Daily(n) => add_days(&candidate, n as i32),
                Recurrence::Weekly(n, _) => add_days(&candidate, (n as i32) * 7),
                Recurrence::Monthly(n, day) => {
                    let next = add_months(&candidate, n as i32);
                    let md = days_in_month(next.year, next.month);
                    Date { year: next.year, month: next.month, day: if day > md { md } else { day } }
                }
                Recurrence::Yearly(n, month, day) => {
                    let new_year = candidate.year + n;
                    let md = days_in_month(new_year, month);
                    Date { year: new_year, month, day: if day > md { md } else { day } }
                }
            };
            count += 1;

            // Safety: prevent infinite loop on degenerate events
            if count > 100_000 { return None; }
        }
    }

    /// Get all events that occur within [from, to] inclusive
    pub fn events_in_range(&self, from: &Date, to: &Date) -> Vec<(u32, String, Date)> {
        let mut results = Vec::new();
        for event in &self.events {
            let mut d = event.start;
            let mut count = 0u32;

            loop {
                if event.max_occurrences > 0 && count >= event.max_occurrences { break; }
                if diff_days(&d, to) > 0 { break; }
                if diff_days(&d, from) >= 0 {
                    results.push((event.id, event.name.clone(), d));
                }

                d = match event.recurrence {
                    Recurrence::Daily(n) => add_days(&d, n as i32),
                    Recurrence::Weekly(n, _) => add_days(&d, (n as i32) * 7),
                    Recurrence::Monthly(n, day) => {
                        let next = add_months(&d, n as i32);
                        let md = days_in_month(next.year, next.month);
                        Date { year: next.year, month: next.month, day: if day > md { md } else { day } }
                    }
                    Recurrence::Yearly(n, month, day) => {
                        let new_year = d.year + n;
                        let md = days_in_month(new_year, month);
                        Date { year: new_year, month, day: if day > md { md } else { day } }
                    }
                };
                count += 1;
                if count > 100_000 { break; }
            }
        }
        results
    }

    pub fn event_count(&self) -> usize {
        self.events.len()
    }
}

// ---------------------------------------------------------------------------
// ISO 8601 formatting
// ---------------------------------------------------------------------------

/// Format a Date as ISO 8601: "YYYY-MM-DD"
pub fn format_date(date: &Date) -> String {
    let mut buf = String::new();
    push_u16_padded(&mut buf, date.year, 4);
    buf.push('-');
    push_u8_padded(&mut buf, date.month, 2);
    buf.push('-');
    push_u8_padded(&mut buf, date.day, 2);
    buf
}

/// Format a DateTime as ISO 8601: "YYYY-MM-DDTHH:MM:SSZ"
pub fn format_datetime(dt: &DateTime) -> String {
    let mut buf = format_date(&dt.date);
    buf.push('T');
    push_u8_padded(&mut buf, dt.hour, 2);
    buf.push(':');
    push_u8_padded(&mut buf, dt.minute, 2);
    buf.push(':');
    push_u8_padded(&mut buf, dt.second, 2);
    buf.push('Z');
    buf
}

/// Format ISO week: "YYYY-Www-D"
pub fn format_iso_week(date: &Date) -> String {
    let (iso_year, week) = iso_week(date);
    let dow = day_of_week(date);
    let iso_dow = if dow as u8 == 0 { 7 } else { dow as u8 };
    let mut buf = String::new();
    push_u16_padded(&mut buf, iso_year, 4);
    buf.push_str("-W");
    push_u8_padded(&mut buf, week, 2);
    buf.push('-');
    push_u8_padded(&mut buf, iso_dow, 1);
    buf
}

fn push_u16_padded(buf: &mut String, val: u16, width: usize) {
    let mut digits = [0u8; 5];
    let mut v = val;
    let mut i = 0;
    loop {
        digits[i] = (v % 10) as u8 + b'0';
        v /= 10;
        i += 1;
        if v == 0 { break; }
    }
    // Pad with zeros
    while i < width { digits[i] = b'0'; i += 1; }
    // Reverse
    for j in (0..i).rev() {
        buf.push(digits[j] as char);
    }
}

fn push_u8_padded(buf: &mut String, val: u8, width: usize) {
    push_u16_padded(buf, val as u16, width);
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CALENDAR: Mutex<CalendarSystem> = Mutex::new(CalendarSystem::new());

/// Add a recurring event
pub fn add_event(name: &str, start: Date, recurrence: Recurrence, max: u32) -> u32 {
    CALENDAR.lock().add_event(name, start, recurrence, max)
}

/// Remove an event by ID
pub fn remove_event(id: u32) -> bool {
    CALENDAR.lock().remove_event(id)
}

/// Count of registered events
pub fn event_count() -> usize {
    CALENDAR.lock().event_count()
}

/// Get the current date from the system clock
pub fn today() -> Date {
    let unix = super::clock::unix_time();
    from_unix(unix).date
}

/// Get current DateTime from system clock
pub fn now() -> DateTime {
    let unix = super::clock::unix_time();
    from_unix(unix)
}

pub fn init() {
    let dt = now();
    let dow = day_of_week(&dt.date);
    let (iso_y, iso_w) = iso_week(&dt.date);
    serial_println!("    [calendar] {} {} (ISO {}-W{:02}), doy={}",
        dow.short_name(),
        format_date(&dt.date),
        iso_y, iso_w,
        day_of_year(&dt.date));
}
