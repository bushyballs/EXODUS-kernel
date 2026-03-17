use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Hoags Data Usage Monitor — network traffic tracking for Genesis
///
/// Features:
///   - Per-application network usage tracking (bytes sent/received)
///   - Configurable billing periods (hourly, daily, weekly, monthly)
///   - Data limits with warning thresholds, throttling, and blocking
///   - Top-app ranking by bandwidth consumption
///   - Daily average computation (Q16 fixed-point)
///   - Cycle reset and export for reporting
///
/// All thresholds and averages use Q16 fixed-point (i32, 1.0 = 65536).
/// No floating-point. No external crates. All code is original.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (1.0 = 65536)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_from_int(v: i32) -> i32 {
    v * Q16_ONE
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) / Q16_ONE as i64) as i32
}

fn q16_percent(fraction: i32, total: i32) -> i32 {
    if total == 0 {
        return 0;
    }
    q16_div(fraction * 100, total)
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Time period for aggregating usage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePeriod {
    Hour,
    Day,
    Week,
    Month,
}

/// Action to take when a data limit is reached
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitAction {
    /// Only warn the user
    Warn,
    /// Reduce bandwidth for the app
    Throttle,
    /// Block network access for the app
    Block,
}

// ---------------------------------------------------------------------------
// Records and limits
// ---------------------------------------------------------------------------

/// A single usage record for a specific app and period
#[derive(Debug, Clone)]
pub struct UsageRecord {
    /// Hash of the application identifier
    pub app_hash: u64,
    /// Total bytes sent during this period
    pub bytes_sent: u64,
    /// Total bytes received during this period
    pub bytes_received: u64,
    /// The aggregation period
    pub period: UsagePeriod,
    /// Timestamp of this record (start of period)
    pub timestamp: u64,
}

/// Data limit configuration for an app or the entire system
#[derive(Debug, Clone)]
pub struct DataLimit {
    /// Maximum bytes allowed in the billing cycle
    pub limit_bytes: u64,
    /// Bytes consumed so far
    pub used_bytes: u64,
    /// Warning threshold as Q16 fraction (e.g., 0.80 = warn at 80%)
    pub warning_threshold: i32,
    /// Action to take when limit is reached
    pub action: LimitAction,
}

/// Result of checking a limit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStatus {
    /// Under the warning threshold
    Ok,
    /// Exceeded warning threshold but not limit
    Warning,
    /// Exceeded the limit
    Exceeded,
}

// ---------------------------------------------------------------------------
// Monitor state
// ---------------------------------------------------------------------------

struct DataMonitor {
    /// All recorded usage entries
    records: Vec<UsageRecord>,
    /// Per-app data limits (app_hash, limit)
    limits: Vec<(u64, DataLimit)>,
    /// Global data limit (0 = no global limit set)
    global_limit: Option<DataLimit>,
    /// Current billing cycle start timestamp
    cycle_start: u64,
    /// Billing cycle duration in seconds
    cycle_duration: u64,
    /// Total bytes sent this cycle
    total_sent: u64,
    /// Total bytes received this cycle
    total_received: u64,
}

impl DataMonitor {
    const fn new() -> Self {
        DataMonitor {
            records: Vec::new(),
            limits: Vec::new(),
            global_limit: None,
            cycle_start: 0,
            cycle_duration: 2592000, // ~30 days in seconds
            total_sent: 0,
            total_received: 0,
        }
    }
}

static MONITOR: Mutex<Option<DataMonitor>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Record network usage for an application
pub fn record_usage(
    app_hash: u64,
    bytes_sent: u64,
    bytes_received: u64,
    period: UsagePeriod,
    timestamp: u64,
) {
    let mut guard = MONITOR.lock();
    if let Some(ref mut mon) = *guard {
        // Check if we already have a record for this app/period/timestamp
        let existing = mon.records.iter_mut().find(|r| {
            r.app_hash == app_hash
                && r.timestamp == timestamp
                && matches!(
                    (&r.period, &period),
                    (UsagePeriod::Hour, UsagePeriod::Hour)
                        | (UsagePeriod::Day, UsagePeriod::Day)
                        | (UsagePeriod::Week, UsagePeriod::Week)
                        | (UsagePeriod::Month, UsagePeriod::Month)
                )
        });

        if let Some(rec) = existing {
            rec.bytes_sent += bytes_sent;
            rec.bytes_received += bytes_received;
        } else {
            mon.records.push(UsageRecord {
                app_hash,
                bytes_sent,
                bytes_received,
                period,
                timestamp,
            });
        }

        mon.total_sent += bytes_sent;
        mon.total_received += bytes_received;

        // Update per-app limit tracking
        if let Some((_, ref mut limit)) = mon.limits.iter_mut().find(|(h, _)| *h == app_hash) {
            limit.used_bytes += bytes_sent + bytes_received;
        }

        // Update global limit tracking
        if let Some(ref mut gl) = mon.global_limit {
            gl.used_bytes += bytes_sent + bytes_received;
        }
    }
}

/// Get total usage across all apps for the current cycle
pub fn get_usage() -> (u64, u64) {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        (mon.total_sent, mon.total_received)
    } else {
        (0, 0)
    }
}

/// Get usage records for a specific application
pub fn get_by_app(app_hash: u64) -> Vec<UsageRecord> {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        mon.records
            .iter()
            .filter(|r| r.app_hash == app_hash)
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Set a data limit for a specific app
pub fn set_limit(app_hash: u64, limit_bytes: u64, warning_threshold_q16: i32, action: LimitAction) {
    let mut guard = MONITOR.lock();
    if let Some(ref mut mon) = *guard {
        // Calculate currently used bytes for this app
        let used: u64 = mon
            .records
            .iter()
            .filter(|r| r.app_hash == app_hash)
            .map(|r| r.bytes_sent + r.bytes_received)
            .sum();

        let limit = DataLimit {
            limit_bytes,
            used_bytes: used,
            warning_threshold: warning_threshold_q16,
            action,
        };

        // Replace existing or insert new
        if let Some((_, ref mut existing)) = mon.limits.iter_mut().find(|(h, _)| *h == app_hash) {
            *existing = limit;
        } else {
            mon.limits.push((app_hash, limit));
        }
        serial_println!(
            "  DataUsage: limit set for app {:016X}: {} bytes",
            app_hash,
            limit_bytes
        );
    }
}

/// Set a global data limit (system-wide)
pub fn set_global_limit(limit_bytes: u64, warning_threshold_q16: i32, action: LimitAction) {
    let mut guard = MONITOR.lock();
    if let Some(ref mut mon) = *guard {
        mon.global_limit = Some(DataLimit {
            limit_bytes,
            used_bytes: mon.total_sent + mon.total_received,
            warning_threshold: warning_threshold_q16,
            action,
        });
        serial_println!("  DataUsage: global limit set: {} bytes", limit_bytes);
    }
}

/// Check the limit status for a specific app
pub fn check_limit(app_hash: u64) -> (LimitStatus, LimitAction) {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        if let Some((_, ref limit)) = mon.limits.iter().find(|(h, _)| *h == app_hash) {
            return evaluate_limit(limit);
        }
        // Fall back to global limit
        if let Some(ref gl) = mon.global_limit {
            return evaluate_limit(gl);
        }
    }
    (LimitStatus::Ok, LimitAction::Warn)
}

/// Evaluate limit status from a DataLimit
fn evaluate_limit(limit: &DataLimit) -> (LimitStatus, LimitAction) {
    if limit.limit_bytes == 0 {
        return (LimitStatus::Ok, limit.action);
    }
    if limit.used_bytes >= limit.limit_bytes {
        return (LimitStatus::Exceeded, limit.action);
    }
    // Check warning threshold
    let usage_ratio = q16_div(limit.used_bytes as i32, limit.limit_bytes as i32);
    if usage_ratio >= limit.warning_threshold {
        return (LimitStatus::Warning, limit.action);
    }
    (LimitStatus::Ok, limit.action)
}

/// Get the top N apps by total bytes (sent + received), sorted descending
pub fn get_top_apps(count: usize) -> Vec<(u64, u64)> {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        // Aggregate per-app totals
        let mut app_totals: Vec<(u64, u64)> = Vec::new();
        for rec in &mon.records {
            let total = rec.bytes_sent + rec.bytes_received;
            if let Some(entry) = app_totals.iter_mut().find(|(h, _)| *h == rec.app_hash) {
                entry.1 += total;
            } else {
                app_totals.push((rec.app_hash, total));
            }
        }
        // Sort descending by total bytes
        app_totals.sort_by(|a, b| b.1.cmp(&a.1));
        app_totals.truncate(count);
        app_totals
    } else {
        Vec::new()
    }
}

/// Reset the billing cycle, archiving current records
pub fn reset_cycle(new_cycle_start: u64) {
    let mut guard = MONITOR.lock();
    if let Some(ref mut mon) = *guard {
        let old_count = mon.records.len();
        mon.records.clear();
        mon.total_sent = 0;
        mon.total_received = 0;
        mon.cycle_start = new_cycle_start;

        // Reset all per-app limit counters
        for (_, ref mut limit) in mon.limits.iter_mut() {
            limit.used_bytes = 0;
        }
        // Reset global limit counter
        if let Some(ref mut gl) = mon.global_limit {
            gl.used_bytes = 0;
        }
        serial_println!("  DataUsage: cycle reset, cleared {} records", old_count);
    }
}

/// Export a usage report as a binary blob
/// Format: [total_sent:u64][total_received:u64][record_count:u32][records...]
pub fn export_report() -> Vec<u8> {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        let mut blob = Vec::new();
        blob.extend_from_slice(&mon.total_sent.to_le_bytes());
        blob.extend_from_slice(&mon.total_received.to_le_bytes());
        let count = mon.records.len() as u32;
        blob.extend_from_slice(&count.to_le_bytes());
        for rec in &mon.records {
            blob.extend_from_slice(&rec.app_hash.to_le_bytes());
            blob.extend_from_slice(&rec.bytes_sent.to_le_bytes());
            blob.extend_from_slice(&rec.bytes_received.to_le_bytes());
            blob.push(period_to_byte(rec.period));
            blob.extend_from_slice(&rec.timestamp.to_le_bytes());
        }
        serial_println!("  DataUsage: exported report, {} records", count);
        blob
    } else {
        Vec::new()
    }
}

/// Get the daily average bytes (sent + received) as Q16 fixed-point
/// `days_in_cycle` is the number of days elapsed in the current billing cycle
pub fn get_daily_average(days_in_cycle: i32) -> i32 {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        if days_in_cycle <= 0 {
            return 0;
        }
        let total = (mon.total_sent + mon.total_received) as i32;
        q16_div(total, days_in_cycle)
    } else {
        0
    }
}

/// Get usage breakdown by period type for a specific app
pub fn get_by_period(app_hash: u64, period: UsagePeriod) -> Vec<UsageRecord> {
    let guard = MONITOR.lock();
    if let Some(ref mon) = *guard {
        mon.records
            .iter()
            .filter(|r| {
                r.app_hash == app_hash
                    && matches!(
                        (&r.period, &period),
                        (UsagePeriod::Hour, UsagePeriod::Hour)
                            | (UsagePeriod::Day, UsagePeriod::Day)
                            | (UsagePeriod::Week, UsagePeriod::Week)
                            | (UsagePeriod::Month, UsagePeriod::Month)
                    )
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    }
}

/// Remove the data limit for a specific app
pub fn remove_limit(app_hash: u64) -> bool {
    let mut guard = MONITOR.lock();
    if let Some(ref mut mon) = *guard {
        let before = mon.limits.len();
        mon.limits.retain(|(h, _)| *h != app_hash);
        mon.limits.len() < before
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn period_to_byte(p: UsagePeriod) -> u8 {
    match p {
        UsagePeriod::Hour => 0,
        UsagePeriod::Day => 1,
        UsagePeriod::Week => 2,
        UsagePeriod::Month => 3,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the data usage monitor
pub fn init() {
    let mut guard = MONITOR.lock();
    *guard = Some(DataMonitor::new());
    serial_println!("  DataUsage: monitor initialized");
}
