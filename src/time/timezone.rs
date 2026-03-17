/// Timezone database and conversion for Genesis
///
/// Provides UTC offset tracking, DST rules, IANA timezone name lookup,
/// and local/UTC conversion. Uses a built-in static database of common
/// timezones with their UTC offsets and daylight-saving transition rules.
///
/// All offsets stored in Q16 fixed-point seconds for sub-second precision
/// (though most zones use whole-hour or half-hour offsets).
///
/// All code is original.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (i32 with 16 fractional bits)
// ---------------------------------------------------------------------------

/// Convert whole seconds to Q16
const fn secs_to_q16(s: i32) -> i32 {
    s << 16
}

/// Convert Q16 back to whole seconds (truncates fractional part)
const fn q16_to_secs(q: i32) -> i32 {
    q >> 16
}

/// Convert hours to Q16 seconds
const fn hours_to_q16(h: i32) -> i32 {
    secs_to_q16(h * 3600)
}

/// Convert hours + minutes to Q16 seconds
const fn hm_to_q16(h: i32, m: i32) -> i32 {
    secs_to_q16(h * 3600 + m * 60)
}

// ---------------------------------------------------------------------------
// DST transition rule
// ---------------------------------------------------------------------------

/// How DST transitions are expressed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DstRule {
    /// No DST in this zone
    None,
    /// Transition on a fixed month/day/hour (rare, used by some zones)
    Fixed {
        start_month: u8,
        start_day: u8,
        start_hour: u8,
        end_month: u8,
        end_day: u8,
        end_hour: u8,
    },
    /// Transition on Nth weekday of month (common US/EU rule)
    /// week=1..5 (5 = last), weekday=0(Sun)..6(Sat)
    NthWeekday {
        start_month: u8,
        start_week: u8,
        start_weekday: u8,
        start_hour: u8,
        end_month: u8,
        end_week: u8,
        end_weekday: u8,
        end_hour: u8,
    },
}

// ---------------------------------------------------------------------------
// Timezone entry
// ---------------------------------------------------------------------------

/// A timezone definition with standard and daylight offsets plus transition rules
#[derive(Debug, Clone)]
pub struct TimezoneEntry {
    /// IANA name (e.g. "America/New_York")
    pub name: String,
    /// Short standard abbreviation (e.g. "EST")
    pub abbrev_std: String,
    /// Short daylight abbreviation (e.g. "EDT"), empty if no DST
    pub abbrev_dst: String,
    /// Standard UTC offset in Q16 seconds
    pub offset_std_q16: i32,
    /// Daylight UTC offset in Q16 seconds (same as std if no DST)
    pub offset_dst_q16: i32,
    /// DST transition rule
    pub dst_rule: DstRule,
}

// ---------------------------------------------------------------------------
// Timezone database
// ---------------------------------------------------------------------------

/// The global timezone state
pub struct TimezoneDb {
    /// All known timezones
    pub zones: Vec<TimezoneEntry>,
    /// Index of the currently active timezone
    pub current_zone: usize,
}

impl TimezoneDb {
    const fn new() -> Self {
        TimezoneDb {
            zones: Vec::new(),
            current_zone: 0,
        }
    }

    /// Populate the built-in timezone table
    fn populate(&mut self) {
        // US zones
        self.add("America/New_York", "EST", "EDT",
                 hours_to_q16(-5), hours_to_q16(-4),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 2, start_weekday: 0, start_hour: 2,
                     end_month: 11, end_week: 1, end_weekday: 0, end_hour: 2,
                 });
        self.add("America/Chicago", "CST", "CDT",
                 hours_to_q16(-6), hours_to_q16(-5),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 2, start_weekday: 0, start_hour: 2,
                     end_month: 11, end_week: 1, end_weekday: 0, end_hour: 2,
                 });
        self.add("America/Denver", "MST", "MDT",
                 hours_to_q16(-7), hours_to_q16(-6),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 2, start_weekday: 0, start_hour: 2,
                     end_month: 11, end_week: 1, end_weekday: 0, end_hour: 2,
                 });
        self.add("America/Los_Angeles", "PST", "PDT",
                 hours_to_q16(-8), hours_to_q16(-7),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 2, start_weekday: 0, start_hour: 2,
                     end_month: 11, end_week: 1, end_weekday: 0, end_hour: 2,
                 });
        self.add("America/Anchorage", "AKST", "AKDT",
                 hours_to_q16(-9), hours_to_q16(-8),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 2, start_weekday: 0, start_hour: 2,
                     end_month: 11, end_week: 1, end_weekday: 0, end_hour: 2,
                 });
        self.add("Pacific/Honolulu", "HST", "",
                 hours_to_q16(-10), hours_to_q16(-10), DstRule::None);
        self.add("America/Phoenix", "MST", "",
                 hours_to_q16(-7), hours_to_q16(-7), DstRule::None);

        // Europe
        self.add("Europe/London", "GMT", "BST",
                 hours_to_q16(0), hours_to_q16(1),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 5, start_weekday: 0, start_hour: 1,
                     end_month: 10, end_week: 5, end_weekday: 0, end_hour: 2,
                 });
        self.add("Europe/Berlin", "CET", "CEST",
                 hours_to_q16(1), hours_to_q16(2),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 5, start_weekday: 0, start_hour: 2,
                     end_month: 10, end_week: 5, end_weekday: 0, end_hour: 3,
                 });
        self.add("Europe/Paris", "CET", "CEST",
                 hours_to_q16(1), hours_to_q16(2),
                 DstRule::NthWeekday {
                     start_month: 3, start_week: 5, start_weekday: 0, start_hour: 2,
                     end_month: 10, end_week: 5, end_weekday: 0, end_hour: 3,
                 });
        self.add("Europe/Moscow", "MSK", "",
                 hours_to_q16(3), hours_to_q16(3), DstRule::None);

        // Asia
        self.add("Asia/Tokyo", "JST", "",
                 hours_to_q16(9), hours_to_q16(9), DstRule::None);
        self.add("Asia/Shanghai", "CST", "",
                 hours_to_q16(8), hours_to_q16(8), DstRule::None);
        self.add("Asia/Kolkata", "IST", "",
                 hm_to_q16(5, 30), hm_to_q16(5, 30), DstRule::None);
        self.add("Asia/Dubai", "GST", "",
                 hours_to_q16(4), hours_to_q16(4), DstRule::None);
        self.add("Asia/Singapore", "SGT", "",
                 hours_to_q16(8), hours_to_q16(8), DstRule::None);

        // Oceania
        self.add("Australia/Sydney", "AEST", "AEDT",
                 hours_to_q16(10), hours_to_q16(11),
                 DstRule::NthWeekday {
                     start_month: 10, start_week: 1, start_weekday: 0, start_hour: 2,
                     end_month: 4, end_week: 1, end_weekday: 0, end_hour: 3,
                 });
        self.add("Pacific/Auckland", "NZST", "NZDT",
                 hours_to_q16(12), hours_to_q16(13),
                 DstRule::NthWeekday {
                     start_month: 9, start_week: 5, start_weekday: 0, start_hour: 2,
                     end_month: 4, end_week: 1, end_weekday: 0, end_hour: 3,
                 });

        // South America
        self.add("America/Sao_Paulo", "BRT", "",
                 hours_to_q16(-3), hours_to_q16(-3), DstRule::None);
        self.add("America/Argentina/Buenos_Aires", "ART", "",
                 hours_to_q16(-3), hours_to_q16(-3), DstRule::None);

        // Africa
        self.add("Africa/Cairo", "EET", "",
                 hours_to_q16(2), hours_to_q16(2), DstRule::None);
        self.add("Africa/Johannesburg", "SAST", "",
                 hours_to_q16(2), hours_to_q16(2), DstRule::None);

        // UTC itself
        self.add("Etc/UTC", "UTC", "",
                 hours_to_q16(0), hours_to_q16(0), DstRule::None);
    }

    fn add(&mut self, name: &str, abbrev_std: &str, abbrev_dst: &str,
           offset_std_q16: i32, offset_dst_q16: i32, dst_rule: DstRule) {
        self.zones.push(TimezoneEntry {
            name: String::from(name),
            abbrev_std: String::from(abbrev_std),
            abbrev_dst: String::from(abbrev_dst),
            offset_std_q16,
            offset_dst_q16,
            dst_rule,
        });
    }

    /// Find a timezone by IANA name, returns index
    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.zones.iter().position(|z| z.name.as_str() == name)
    }

    /// Find a timezone by abbreviation (searches both std and dst)
    pub fn find_by_abbrev(&self, abbrev: &str) -> Option<usize> {
        self.zones.iter().position(|z| {
            z.abbrev_std.as_str() == abbrev || z.abbrev_dst.as_str() == abbrev
        })
    }

    /// Set the current timezone by IANA name, returns true on success
    pub fn set_current(&mut self, name: &str) -> bool {
        if let Some(idx) = self.find_by_name(name) {
            self.current_zone = idx;
            true
        } else {
            false
        }
    }

    /// Get the currently active timezone entry
    pub fn current(&self) -> &TimezoneEntry {
        &self.zones[self.current_zone]
    }

    /// Get all timezone names
    pub fn list_names(&self) -> Vec<String> {
        self.zones.iter().map(|z| z.name.clone()).collect()
    }

    /// Return how many zones are loaded
    pub fn zone_count(&self) -> usize {
        self.zones.len()
    }
}

// ---------------------------------------------------------------------------
// DST detection
// ---------------------------------------------------------------------------

/// Determine if DST is active for a given timezone at the specified UTC timestamp
pub fn is_dst_active(entry: &TimezoneEntry, unix_utc: u64) -> bool {
    match entry.dst_rule {
        DstRule::None => false,
        DstRule::Fixed { start_month, start_day, start_hour,
                         end_month, end_day, end_hour } => {
            let dt = unix_to_components(unix_utc);
            let start_stamp = make_day_stamp(dt.2, start_month, start_day, start_hour);
            let end_stamp = make_day_stamp(dt.2, end_month, end_day, end_hour);
            let cur_stamp = make_day_stamp(dt.2, dt.1, dt.0, dt.3);

            if start_stamp < end_stamp {
                cur_stamp >= start_stamp && cur_stamp < end_stamp
            } else {
                // Southern hemisphere: DST wraps around new year
                cur_stamp >= start_stamp || cur_stamp < end_stamp
            }
        }
        DstRule::NthWeekday { start_month, start_week, start_weekday, start_hour,
                              end_month, end_week, end_weekday, end_hour } => {
            let dt = unix_to_components(unix_utc);
            let year = dt.2;
            let start_day = nth_weekday_of_month(year, start_month, start_week, start_weekday);
            let end_day = nth_weekday_of_month(year, end_month, end_week, end_weekday);

            let start_stamp = make_day_stamp(year, start_month, start_day, start_hour);
            let end_stamp = make_day_stamp(year, end_month, end_day, end_hour);
            let cur_stamp = make_day_stamp(year, dt.1, dt.0, dt.3);

            if start_stamp < end_stamp {
                cur_stamp >= start_stamp && cur_stamp < end_stamp
            } else {
                cur_stamp >= start_stamp || cur_stamp < end_stamp
            }
        }
    }
}

/// Get the effective UTC offset (in whole seconds) for a zone at a given UTC time
pub fn effective_offset_secs(entry: &TimezoneEntry, unix_utc: u64) -> i32 {
    if is_dst_active(entry, unix_utc) {
        q16_to_secs(entry.offset_dst_q16)
    } else {
        q16_to_secs(entry.offset_std_q16)
    }
}

/// Get the effective abbreviation for a zone at a given UTC time
pub fn effective_abbrev(entry: &TimezoneEntry, unix_utc: u64) -> &str {
    if is_dst_active(entry, unix_utc) && !entry.abbrev_dst.is_empty() {
        entry.abbrev_dst.as_str()
    } else {
        entry.abbrev_std.as_str()
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert UTC unix timestamp to local unix timestamp using the current zone
pub fn utc_to_local(unix_utc: u64) -> u64 {
    let db = TIMEZONE_DB.lock();
    let entry = db.current();
    let offset = effective_offset_secs(entry, unix_utc);
    if offset >= 0 {
        unix_utc + offset as u64
    } else {
        unix_utc.saturating_sub((-offset) as u64)
    }
}

/// Convert local unix timestamp to UTC using the current zone
pub fn local_to_utc(unix_local: u64) -> u64 {
    let db = TIMEZONE_DB.lock();
    let entry = db.current();
    // Use the local time as an approximation for DST check
    let offset = effective_offset_secs(entry, unix_local);
    if offset >= 0 {
        unix_local.saturating_sub(offset as u64)
    } else {
        unix_local + (-offset) as u64
    }
}

// ---------------------------------------------------------------------------
// Calendar math for DST transitions
// ---------------------------------------------------------------------------

/// Returns (day, month, year, hour) from a Unix timestamp
fn unix_to_components(unix: u64) -> (u8, u8, u16, u8) {
    let secs = unix;
    let days = (secs / 86400) as u32;
    let remaining = (secs % 86400) as u32;
    let hour = (remaining / 3600) as u8;

    // Days since 1970-01-01
    let mut y: u16 = 1970;
    let mut d = days;
    loop {
        let ydays = if is_leap_year(y) { 366 } else { 365 };
        if d < ydays { break; }
        d -= ydays;
        y += 1;
    }

    let month_days: [u32; 12] = if is_leap_year(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m: u8 = 1;
    for &md in &month_days {
        if d < md { break; }
        d -= md;
        m += 1;
    }
    let day = (d + 1) as u8;

    (day, m, y, hour)
}

fn is_leap_year(y: u16) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Day of week for a given date (0=Sunday, 6=Saturday)
/// Uses Tomohiko Sakamoto's algorithm
fn day_of_week(year: u16, month: u8, day: u8) -> u8 {
    let t: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year as i32;
    if month < 3 { y -= 1; }
    ((y + y / 4 - y / 100 + y / 400 + t[(month - 1) as usize] + day as i32) % 7) as u8
}

/// Find the day-of-month for the Nth weekday of a month
/// week: 1-4 for first..fourth, 5 for last
/// weekday: 0=Sunday, 6=Saturday
fn nth_weekday_of_month(year: u16, month: u8, week: u8, weekday: u8) -> u8 {
    if week == 5 {
        // Last occurrence — scan backward from end of month
        let days_in = days_in_month(year, month);
        let mut d = days_in;
        while d > 0 {
            if day_of_week(year, month, d) == weekday {
                return d;
            }
            d -= 1;
        }
        return 1; // fallback (should not happen)
    }

    // Forward scan: find the Nth occurrence
    let mut count = 0u8;
    let days_in = days_in_month(year, month);
    for d in 1..=days_in {
        if day_of_week(year, month, d) == weekday {
            count += 1;
            if count == week {
                return d;
            }
        }
    }
    1 // fallback
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 => 31, 2 => if is_leap_year(year) { 29 } else { 28 },
        3 => 31, 4 => 30, 5 => 31, 6 => 30,
        7 => 31, 8 => 31, 9 => 30, 10 => 31, 11 => 30, 12 => 31,
        _ => 30,
    }
}

/// Compact stamp for intra-year comparisons: month*100_00_00 + day*10000 + hour*100
fn make_day_stamp(year: u16, month: u8, day: u8, hour: u8) -> u32 {
    let _ = year; // year is implicit (same year comparison)
    (month as u32) * 1_000_000 + (day as u32) * 10_000 + (hour as u32) * 100
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TIMEZONE_DB: Mutex<TimezoneDb> = Mutex::new(TimezoneDb::new());

/// Set the system timezone by IANA name (e.g. "America/New_York")
pub fn set_timezone(name: &str) -> bool {
    TIMEZONE_DB.lock().set_current(name)
}

/// Get the name of the current timezone
pub fn current_timezone_name() -> String {
    TIMEZONE_DB.lock().current().name.clone()
}

/// Get zone count
pub fn zone_count() -> usize {
    TIMEZONE_DB.lock().zone_count()
}

/// List all available timezone names
pub fn list_zones() -> Vec<String> {
    TIMEZONE_DB.lock().list_names()
}

/// Detect timezone from RTC offset heuristic
/// Compares RTC time with a known UTC source and picks the closest zone
pub fn detect_from_rtc() -> Option<String> {
    let rtc = super::rtc::read();
    let rtc_unix = rtc.to_unix();
    let sys_utc = super::clock::unix_time();

    if sys_utc == 0 || rtc_unix == 0 {
        return None;
    }

    // Estimate offset in seconds
    let offset_secs = rtc_unix as i64 - sys_utc as i64;

    let db = TIMEZONE_DB.lock();
    let mut best_idx = 0usize;
    let mut best_diff = i64::MAX;

    for (i, zone) in db.zones.iter().enumerate() {
        let zo = q16_to_secs(zone.offset_std_q16) as i64;
        let diff = (offset_secs - zo).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }

    if best_diff < 1800 {
        // Within 30 minutes — plausible match
        Some(db.zones[best_idx].name.clone())
    } else {
        None
    }
}

pub fn init() {
    let mut db = TIMEZONE_DB.lock();
    db.populate();
    let count = db.zone_count();
    // Default to UTC
    let _ = db.set_current("Etc/UTC");
    serial_println!("    [timezone] Loaded {} zones, current=UTC", count);
}
