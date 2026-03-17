use crate::sync::Mutex;
/// Clock application for Genesis OS
///
/// Multi-mode clock with alarm, timer, and stopwatch functionality.
/// Supports multiple alarms with day-of-week masks, countdown timers,
/// stopwatch with lap recording, and world clock time zones (stored
/// as UTC offset values in Q16 fixed-point).
///
/// Inspired by: GNOME Clocks, Android Clock, iOS Clock. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Milliseconds per second
const MS_PER_SEC: u32 = 1000;
/// Milliseconds per minute
const MS_PER_MIN: u32 = 60_000;
/// Milliseconds per hour
const MS_PER_HOUR: u32 = 3_600_000;
/// Seconds per day
const SECS_PER_DAY: u64 = 86400;
/// Seconds per hour
const SECS_PER_HOUR: u64 = 3600;
/// Maximum number of alarms
const MAX_ALARMS: usize = 32;
/// Maximum number of laps in stopwatch
const MAX_LAPS: usize = 200;
/// Maximum world clocks
const MAX_WORLD_CLOCKS: usize = 24;

// Day-of-week bit masks for alarm scheduling
/// Sunday
pub const DAY_SUN: u8 = 0x01;
/// Monday
pub const DAY_MON: u8 = 0x02;
/// Tuesday
pub const DAY_TUE: u8 = 0x04;
/// Wednesday
pub const DAY_WED: u8 = 0x08;
/// Thursday
pub const DAY_THU: u8 = 0x10;
/// Friday
pub const DAY_FRI: u8 = 0x20;
/// Saturday
pub const DAY_SAT: u8 = 0x40;
/// Every day
pub const DAY_ALL: u8 = 0x7F;
/// Weekdays only (Mon-Fri)
pub const DAY_WEEKDAYS: u8 = DAY_MON | DAY_TUE | DAY_WED | DAY_THU | DAY_FRI;
/// Weekends only (Sat-Sun)
pub const DAY_WEEKENDS: u8 = DAY_SAT | DAY_SUN;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Clock application mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClockMode {
    Clock,
    Alarm,
    Timer,
    Stopwatch,
}

/// Time format preference
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeFormat {
    Hour12,
    Hour24,
}

/// Alarm snooze status
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlarmStatus {
    Idle,
    Ringing,
    Snoozed,
}

/// Result codes for clock operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClockResult {
    Success,
    NotFound,
    AlreadyExists,
    LimitReached,
    InvalidTime,
    NotRunning,
    AlreadyRunning,
    IoError,
}

/// An alarm entry
#[derive(Debug, Clone)]
pub struct Alarm {
    pub id: u64,
    pub hour: u8,
    pub minute: u8,
    pub days: u8,
    pub enabled: bool,
    pub label_hash: u64,
    pub sound_hash: u64,
    pub snooze_minutes: u8,
    pub status: AlarmStatus,
    pub vibrate: bool,
}

/// A countdown timer
#[derive(Debug, Clone)]
pub struct Timer {
    pub duration_ms: u32,
    pub remaining_ms: u32,
    pub running: bool,
    pub label_hash: u64,
}

/// A stopwatch with lap support
#[derive(Debug, Clone)]
pub struct Stopwatch {
    pub elapsed_ms: u64,
    pub laps: Vec<u64>,
    pub running: bool,
    pub best_lap_ms: u64,
    pub worst_lap_ms: u64,
}

/// A world clock entry
#[derive(Debug, Clone)]
pub struct WorldClock {
    pub city_hash: u64,
    pub utc_offset_minutes: i32,
    pub label_hash: u64,
}

/// Decomposed time components
#[derive(Debug, Clone, Copy)]
pub struct TimeComponents {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub milliseconds: u16,
    pub day_of_week: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

/// Persistent clock state
struct ClockState {
    mode: ClockMode,
    format: TimeFormat,
    alarms: Vec<Alarm>,
    timers: Vec<Timer>,
    stopwatch: Stopwatch,
    world_clocks: Vec<WorldClock>,
    next_alarm_id: u64,
    current_epoch_secs: u64,
    snooze_duration_min: u8,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CLOCK: Mutex<Option<ClockState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_stopwatch() -> Stopwatch {
    Stopwatch {
        elapsed_ms: 0,
        laps: Vec::new(),
        running: false,
        best_lap_ms: u64::MAX,
        worst_lap_ms: 0,
    }
}

fn default_state() -> ClockState {
    ClockState {
        mode: ClockMode::Clock,
        format: TimeFormat::Hour24,
        alarms: Vec::new(),
        timers: Vec::new(),
        stopwatch: default_stopwatch(),
        world_clocks: Vec::new(),
        next_alarm_id: 1,
        current_epoch_secs: 1_700_000_000,
        snooze_duration_min: 9,
    }
}

/// Decompose epoch seconds into time components
fn decompose_time(epoch_secs: u64) -> TimeComponents {
    let total_days = epoch_secs / SECS_PER_DAY;
    let time_of_day = epoch_secs % SECS_PER_DAY;

    let hours = (time_of_day / 3600) as u8;
    let minutes = ((time_of_day % 3600) / 60) as u8;
    let seconds = (time_of_day % 60) as u8;

    // Day of week: epoch (Jan 1 1970) was a Thursday (4)
    let day_of_week = ((total_days + 4) % 7) as u8;

    // Simplified date decomposition (approximate, ignoring leap years for now)
    let mut remaining_days = total_days;
    let mut year: u16 = 1970;
    loop {
        let days_in_year: u64 = if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days: [u64; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month: u8 = 1;
    for &md in month_days.iter() {
        if remaining_days < md {
            break;
        }
        remaining_days -= md;
        month += 1;
    }
    let day = remaining_days as u8 + 1;

    TimeComponents {
        hours,
        minutes,
        seconds,
        milliseconds: 0,
        day_of_week,
        day,
        month,
        year,
    }
}

/// Check if a day-of-week bitmask includes a specific day (0=Sun, 1=Mon, ..., 6=Sat)
fn day_matches(days_mask: u8, day_of_week: u8) -> bool {
    let bit = 1u8 << day_of_week;
    (days_mask & bit) != 0
}

// ---------------------------------------------------------------------------
// Public API — Clock
// ---------------------------------------------------------------------------

/// Get the current time as decomposed components
pub fn get_time() -> TimeComponents {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => decompose_time(state.current_epoch_secs),
        None => decompose_time(0),
    }
}

/// Get the current epoch seconds
pub fn get_epoch_secs() -> u64 {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.current_epoch_secs,
        None => 0,
    }
}

/// Update the current time (called by the kernel timer interrupt)
pub fn tick(delta_ms: u32) {
    let mut guard = CLOCK.lock();
    if let Some(state) = guard.as_mut() {
        // Update epoch
        if delta_ms >= MS_PER_SEC {
            state.current_epoch_secs += (delta_ms / MS_PER_SEC) as u64;
        }

        // Update timers
        for timer in state.timers.iter_mut() {
            if timer.running && timer.remaining_ms > 0 {
                if delta_ms >= timer.remaining_ms {
                    timer.remaining_ms = 0;
                    timer.running = false;
                    // Timer expired — kernel would trigger notification here
                } else {
                    timer.remaining_ms -= delta_ms;
                }
            }
        }

        // Update stopwatch
        if state.stopwatch.running {
            state.stopwatch.elapsed_ms += delta_ms as u64;
        }

        // Check alarms
        let time = decompose_time(state.current_epoch_secs);
        for alarm in state.alarms.iter_mut() {
            if alarm.enabled
                && alarm.status == AlarmStatus::Idle
                && alarm.hour == time.hours
                && alarm.minute == time.minutes
                && time.seconds == 0
                && (alarm.days == 0 || day_matches(alarm.days, time.day_of_week))
            {
                alarm.status = AlarmStatus::Ringing;
            }
        }
    }
}

/// Set the time format (12h or 24h)
pub fn set_format(format: TimeFormat) {
    let mut guard = CLOCK.lock();
    if let Some(state) = guard.as_mut() {
        state.format = format;
    }
}

/// Get the current time format
pub fn get_format() -> TimeFormat {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.format,
        None => TimeFormat::Hour24,
    }
}

/// Set the active clock mode
pub fn set_mode(mode: ClockMode) {
    let mut guard = CLOCK.lock();
    if let Some(state) = guard.as_mut() {
        state.mode = mode;
    }
}

/// Get the active clock mode
pub fn get_mode() -> ClockMode {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.mode,
        None => ClockMode::Clock,
    }
}

// ---------------------------------------------------------------------------
// Public API — Alarms
// ---------------------------------------------------------------------------

/// Create a new alarm
pub fn set_alarm(hour: u8, minute: u8, days: u8, label_hash: u64, sound_hash: u64) -> ClockResult {
    if hour > 23 || minute > 59 {
        return ClockResult::InvalidTime;
    }
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if state.alarms.len() >= MAX_ALARMS {
        return ClockResult::LimitReached;
    }
    let id = state.next_alarm_id;
    state.next_alarm_id += 1;
    state.alarms.push(Alarm {
        id,
        hour,
        minute,
        days,
        enabled: true,
        label_hash,
        sound_hash,
        snooze_minutes: state.snooze_duration_min,
        status: AlarmStatus::Idle,
        vibrate: true,
    });
    ClockResult::Success
}

/// Delete an alarm by ID
pub fn delete_alarm(alarm_id: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    let before = state.alarms.len();
    state.alarms.retain(|a| a.id != alarm_id);
    if state.alarms.len() < before {
        ClockResult::Success
    } else {
        ClockResult::NotFound
    }
}

/// Toggle an alarm on/off
pub fn toggle_alarm(alarm_id: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if let Some(alarm) = state.alarms.iter_mut().find(|a| a.id == alarm_id) {
        alarm.enabled = !alarm.enabled;
        if !alarm.enabled {
            alarm.status = AlarmStatus::Idle;
        }
        ClockResult::Success
    } else {
        ClockResult::NotFound
    }
}

/// Dismiss a ringing alarm
pub fn dismiss_alarm(alarm_id: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if let Some(alarm) = state.alarms.iter_mut().find(|a| a.id == alarm_id) {
        alarm.status = AlarmStatus::Idle;
        // If no repeat days, disable after dismissal
        if alarm.days == 0 {
            alarm.enabled = false;
        }
        ClockResult::Success
    } else {
        ClockResult::NotFound
    }
}

/// Snooze a ringing alarm
pub fn snooze_alarm(alarm_id: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if let Some(alarm) = state.alarms.iter_mut().find(|a| a.id == alarm_id) {
        if alarm.status == AlarmStatus::Ringing {
            alarm.status = AlarmStatus::Snoozed;
            // Advance alarm time by snooze duration
            let total_min =
                alarm.hour as u16 * 60 + alarm.minute as u16 + alarm.snooze_minutes as u16;
            alarm.hour = ((total_min / 60) % 24) as u8;
            alarm.minute = (total_min % 60) as u8;
            ClockResult::Success
        } else {
            ClockResult::NotRunning
        }
    } else {
        ClockResult::NotFound
    }
}

/// Get all alarms
pub fn get_alarms() -> Vec<Alarm> {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.alarms.clone(),
        None => Vec::new(),
    }
}

/// Get the count of enabled alarms
pub fn enabled_alarm_count() -> usize {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.alarms.iter().filter(|a| a.enabled).count(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API — Timer
// ---------------------------------------------------------------------------

/// Start a new countdown timer
pub fn start_timer(duration_ms: u32, label_hash: u64) -> ClockResult {
    if duration_ms == 0 {
        return ClockResult::InvalidTime;
    }
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    state.timers.push(Timer {
        duration_ms,
        remaining_ms: duration_ms,
        running: true,
        label_hash,
    });
    ClockResult::Success
}

/// Pause a running timer by index
pub fn pause_timer(index: usize) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if index >= state.timers.len() {
        return ClockResult::NotFound;
    }
    if !state.timers[index].running {
        return ClockResult::NotRunning;
    }
    state.timers[index].running = false;
    ClockResult::Success
}

/// Resume a paused timer by index
pub fn resume_timer(index: usize) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if index >= state.timers.len() {
        return ClockResult::NotFound;
    }
    if state.timers[index].running {
        return ClockResult::AlreadyRunning;
    }
    state.timers[index].running = true;
    ClockResult::Success
}

/// Reset a timer to its original duration
pub fn reset_timer(index: usize) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if index >= state.timers.len() {
        return ClockResult::NotFound;
    }
    state.timers[index].remaining_ms = state.timers[index].duration_ms;
    state.timers[index].running = false;
    ClockResult::Success
}

/// Delete a timer by index
pub fn delete_timer(index: usize) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if index >= state.timers.len() {
        return ClockResult::NotFound;
    }
    state.timers.remove(index);
    ClockResult::Success
}

/// Get all active timers
pub fn get_timers() -> Vec<Timer> {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.timers.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API — Stopwatch
// ---------------------------------------------------------------------------

/// Start the stopwatch
pub fn start_stopwatch() -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if state.stopwatch.running {
        return ClockResult::AlreadyRunning;
    }
    state.stopwatch.running = true;
    ClockResult::Success
}

/// Stop (pause) the stopwatch
pub fn stop_stopwatch() -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if !state.stopwatch.running {
        return ClockResult::NotRunning;
    }
    state.stopwatch.running = false;
    ClockResult::Success
}

/// Reset the stopwatch to zero
pub fn reset_stopwatch() {
    let mut guard = CLOCK.lock();
    if let Some(state) = guard.as_mut() {
        state.stopwatch = default_stopwatch();
    }
}

/// Record a lap
pub fn lap() -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if !state.stopwatch.running {
        return ClockResult::NotRunning;
    }
    if state.stopwatch.laps.len() >= MAX_LAPS {
        return ClockResult::LimitReached;
    }

    let lap_time = if state.stopwatch.laps.is_empty() {
        state.stopwatch.elapsed_ms
    } else {
        let prev_total: u64 = state.stopwatch.laps.iter().sum();
        state.stopwatch.elapsed_ms - prev_total
    };

    state.stopwatch.laps.push(lap_time);

    // Track best/worst
    if lap_time < state.stopwatch.best_lap_ms {
        state.stopwatch.best_lap_ms = lap_time;
    }
    if lap_time > state.stopwatch.worst_lap_ms {
        state.stopwatch.worst_lap_ms = lap_time;
    }

    ClockResult::Success
}

/// Get the current stopwatch state
pub fn get_stopwatch() -> Stopwatch {
    let guard = CLOCK.lock();
    match guard.as_ref() {
        Some(state) => state.stopwatch.clone(),
        None => default_stopwatch(),
    }
}

// ---------------------------------------------------------------------------
// Public API — World Clocks
// ---------------------------------------------------------------------------

/// Add a world clock (city with UTC offset in minutes)
pub fn add_world_clock(city_hash: u64, utc_offset_minutes: i32, label_hash: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    if state.world_clocks.len() >= MAX_WORLD_CLOCKS {
        return ClockResult::LimitReached;
    }
    if state.world_clocks.iter().any(|w| w.city_hash == city_hash) {
        return ClockResult::AlreadyExists;
    }
    state.world_clocks.push(WorldClock {
        city_hash,
        utc_offset_minutes,
        label_hash,
    });
    ClockResult::Success
}

/// Remove a world clock
pub fn remove_world_clock(city_hash: u64) -> ClockResult {
    let mut guard = CLOCK.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return ClockResult::IoError,
    };
    let before = state.world_clocks.len();
    state.world_clocks.retain(|w| w.city_hash != city_hash);
    if state.world_clocks.len() < before {
        ClockResult::Success
    } else {
        ClockResult::NotFound
    }
}

/// Get all world clocks with their current times
pub fn get_world_clocks() -> Vec<(WorldClock, TimeComponents)> {
    let guard = CLOCK.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let base_secs = state.current_epoch_secs;
    state
        .world_clocks
        .iter()
        .map(|wc| {
            let offset_secs = wc.utc_offset_minutes as i64 * 60;
            let local_secs = (base_secs as i64 + offset_secs) as u64;
            let time = decompose_time(local_secs);
            (wc.clone(), time)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the clock subsystem
pub fn init() {
    let mut guard = CLOCK.lock();
    *guard = Some(default_state());
    serial_println!("    Clock ready (alarm, timer, stopwatch, world clocks)");
}
