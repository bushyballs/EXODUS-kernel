use crate::sync::Mutex;
/// Battery optimization for Genesis
///
/// Adaptive battery management, app standby buckets, doze mode scheduling,
/// background execution limits, usage predictions, and power budget allocation.
///
/// Uses Q16 fixed-point arithmetic (i32, 16 fractional bits).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------
const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;

/// Multiply two Q16 values: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Divide two Q16 values: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    ((a as i64) << 16).checked_div(b as i64).unwrap_or(0) as i32
}

/// Convert integer to Q16
fn q16_from_int(v: i32) -> i32 {
    v << 16
}

// ---------------------------------------------------------------------------
// App standby bucket classification
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppBucket {
    /// Actively in use by the user right now
    Active,
    /// Recently used (within last few hours)
    WorkingSet,
    /// Used regularly but not recently
    Frequent,
    /// Rarely used
    Rare,
    /// System-restricted (misbehaving or user-restricted)
    Restricted,
    /// Never launched or explicitly disabled
    Never,
}

impl AppBucket {
    /// Maximum allowed background jobs per hour for this bucket
    pub fn max_jobs_per_hour(&self) -> u32 {
        match self {
            AppBucket::Active => 0xFFFF_FFFF, // unlimited
            AppBucket::WorkingSet => 30,
            AppBucket::Frequent => 10,
            AppBucket::Rare => 2,
            AppBucket::Restricted => 0,
            AppBucket::Never => 0,
        }
    }

    /// Maximum network access window in seconds per cycle
    pub fn network_window_secs(&self) -> u32 {
        match self {
            AppBucket::Active => 0xFFFF_FFFF,
            AppBucket::WorkingSet => 600,
            AppBucket::Frequent => 300,
            AppBucket::Rare => 60,
            AppBucket::Restricted => 0,
            AppBucket::Never => 0,
        }
    }

    /// Alarm deferral limit in seconds
    pub fn alarm_defer_secs(&self) -> u64 {
        match self {
            AppBucket::Active => 0,
            AppBucket::WorkingSet => 0,
            AppBucket::Frequent => 300,
            AppBucket::Rare => 1800,
            AppBucket::Restricted => 7200,
            AppBucket::Never => 0xFFFF_FFFF_FFFF_FFFF,
        }
    }
}

// ---------------------------------------------------------------------------
// Doze mode states
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DozePhase {
    /// Device is actively in use
    FullActive,
    /// Screen off, motion detected — light doze
    LightIdle,
    /// Stationary for a while — entering deep doze
    DeepIdlePending,
    /// Full deep doze — strict restrictions
    DeepIdle,
    /// Periodic maintenance window during deep doze
    MaintenanceWindow,
}

// ---------------------------------------------------------------------------
// Per-app usage tracking entry
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct AppUsageEntry {
    pub app_id: String,
    pub bucket: AppBucket,
    /// Cumulative CPU time consumed (milliseconds)
    pub cpu_ms: u64,
    /// Cumulative wakelock held time (milliseconds)
    pub wakelock_ms: u64,
    /// Network bytes transferred
    pub net_bytes: u64,
    /// Foreground time (milliseconds)
    pub fg_ms: u64,
    /// Background job count in the current period
    pub bg_jobs_this_period: u32,
    /// Last time the user opened this app (unix timestamp)
    pub last_used_ts: u64,
    /// Predicted next use time (unix timestamp, 0 = unknown)
    pub predicted_next_use_ts: u64,
    /// Historical average daily usage in milliseconds (Q16)
    pub avg_daily_usage_q16: i32,
    /// Exponential-moving-average drain contribution (Q16, mAh)
    pub ema_drain_q16: i32,
}

// ---------------------------------------------------------------------------
// Usage prediction record
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct UsagePrediction {
    pub app_id: String,
    /// Probability of use in the next hour (Q16, 0..Q16_ONE)
    pub prob_next_hour_q16: i32,
    /// Predicted drain in the next hour (Q16, mAh)
    pub predicted_drain_q16: i32,
    /// Suggested bucket based on prediction
    pub suggested_bucket: AppBucket,
}

// ---------------------------------------------------------------------------
// Background execution limit policy
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgLimitPolicy {
    /// No limits on background execution
    None,
    /// Moderate limits — defer non-critical work
    Moderate,
    /// Strict — only whitelisted services
    Strict,
    /// Extreme — kill all background tasks except system
    Extreme,
}

// ---------------------------------------------------------------------------
// Doze schedule entry
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct DozeScheduleEntry {
    /// When the next maintenance window opens (unix timestamp)
    pub window_start_ts: u64,
    /// Duration of the maintenance window (seconds)
    pub window_duration_secs: u32,
    /// Interval between maintenance windows (seconds) — grows exponentially
    pub interval_secs: u32,
}

// ---------------------------------------------------------------------------
// Power budget allocation
// ---------------------------------------------------------------------------
#[derive(Debug, Clone)]
pub struct PowerBudget {
    /// Total budget for this cycle (Q16, mAh)
    pub total_q16: i32,
    /// Allocated to foreground app (Q16, mAh)
    pub foreground_q16: i32,
    /// Allocated to background services (Q16, mAh)
    pub background_q16: i32,
    /// Allocated to system (Q16, mAh)
    pub system_q16: i32,
    /// Remaining unallocated (Q16, mAh)
    pub remaining_q16: i32,
}

// ---------------------------------------------------------------------------
// Main battery optimiser
// ---------------------------------------------------------------------------
pub struct BatteryOptimizer {
    /// Current doze phase
    pub doze_phase: DozePhase,
    /// App usage tracking
    pub apps: Vec<AppUsageEntry>,
    /// Whitelisted app IDs (exempt from doze/bg-limits)
    pub whitelist: Vec<String>,
    /// Background limit policy
    pub bg_policy: BgLimitPolicy,
    /// Current power budget
    pub budget: PowerBudget,
    /// Doze schedule
    pub doze_schedule: DozeScheduleEntry,
    /// Accumulated idle time (seconds)
    pub idle_secs: u64,
    /// Screen state
    pub screen_on: bool,
    /// Device stationary flag
    pub stationary: bool,
    /// Last user interaction timestamp
    pub last_interaction_ts: u64,
    /// Total number of optimisation decisions made
    pub total_decisions: u64,
    /// Usage history window (app_id -> list of (timestamp, duration_ms))
    pub usage_history: BTreeMap<String, Vec<(u64, u64)>>,
    /// Current battery level (0-100)
    pub battery_level: u8,
    /// Charging flag
    pub charging: bool,
    /// Per-bucket job counters reset timestamp
    pub bucket_period_start_ts: u64,
}

impl BatteryOptimizer {
    pub const fn new() -> Self {
        BatteryOptimizer {
            doze_phase: DozePhase::FullActive,
            apps: Vec::new(),
            whitelist: Vec::new(),
            bg_policy: BgLimitPolicy::None,
            budget: PowerBudget {
                total_q16: Q16_ZERO,
                foreground_q16: Q16_ZERO,
                background_q16: Q16_ZERO,
                system_q16: Q16_ZERO,
                remaining_q16: Q16_ZERO,
            },
            doze_schedule: DozeScheduleEntry {
                window_start_ts: 0,
                window_duration_secs: 30,
                interval_secs: 300,
            },
            idle_secs: 0,
            screen_on: true,
            stationary: false,
            last_interaction_ts: 0,
            total_decisions: 0,
            usage_history: BTreeMap::new(),
            battery_level: 100,
            charging: false,
            bucket_period_start_ts: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Doze state machine
    // -----------------------------------------------------------------------
    pub fn tick(&mut self, now: u64) {
        if self.screen_on || self.charging {
            self.doze_phase = DozePhase::FullActive;
            self.idle_secs = 0;
            return;
        }

        let since_interaction = now.saturating_sub(self.last_interaction_ts);
        self.idle_secs = since_interaction;

        match self.doze_phase {
            DozePhase::FullActive => {
                if since_interaction > 120 {
                    self.doze_phase = DozePhase::LightIdle;
                    serial_println!("    [bat-opt] Entering light idle");
                }
            }
            DozePhase::LightIdle => {
                if self.stationary && since_interaction > 600 {
                    self.doze_phase = DozePhase::DeepIdlePending;
                    serial_println!("    [bat-opt] Deep idle pending (stationary)");
                }
            }
            DozePhase::DeepIdlePending => {
                if since_interaction > 1800 {
                    self.doze_phase = DozePhase::DeepIdle;
                    self.doze_schedule.window_start_ts =
                        now + self.doze_schedule.interval_secs as u64;
                    serial_println!("    [bat-opt] Entering deep doze");
                }
            }
            DozePhase::DeepIdle => {
                if now >= self.doze_schedule.window_start_ts {
                    self.doze_phase = DozePhase::MaintenanceWindow;
                    serial_println!("    [bat-opt] Maintenance window opened");
                }
            }
            DozePhase::MaintenanceWindow => {
                let window_end = self.doze_schedule.window_start_ts
                    + self.doze_schedule.window_duration_secs as u64;
                if now >= window_end {
                    // Exponential back-off: double interval, cap at 6 hours
                    self.doze_schedule.interval_secs =
                        (self.doze_schedule.interval_secs * 2).min(21600);
                    self.doze_schedule.window_start_ts =
                        now + self.doze_schedule.interval_secs as u64;
                    self.doze_phase = DozePhase::DeepIdle;
                    serial_println!(
                        "    [bat-opt] Maintenance window closed, next in {}s",
                        self.doze_schedule.interval_secs
                    );
                }
            }
        }

        // Auto background-limit policy based on doze phase
        self.bg_policy = match self.doze_phase {
            DozePhase::FullActive => BgLimitPolicy::None,
            DozePhase::LightIdle => BgLimitPolicy::Moderate,
            DozePhase::DeepIdlePending => BgLimitPolicy::Strict,
            DozePhase::DeepIdle => BgLimitPolicy::Extreme,
            DozePhase::MaintenanceWindow => BgLimitPolicy::Moderate,
        };

        self.total_decisions = self.total_decisions.saturating_add(1);
    }

    // -----------------------------------------------------------------------
    // User interaction
    // -----------------------------------------------------------------------
    pub fn on_user_interaction(&mut self) {
        let now = crate::time::clock::unix_time();
        self.last_interaction_ts = now;
        self.doze_phase = DozePhase::FullActive;
        self.idle_secs = 0;
        // Reset doze schedule for next deep-doze cycle
        self.doze_schedule.interval_secs = 300;
    }

    pub fn on_screen_change(&mut self, on: bool) {
        self.screen_on = on;
        if on {
            self.on_user_interaction();
        }
    }

    pub fn on_motion_detected(&mut self, moving: bool) {
        self.stationary = !moving;
        if moving && self.doze_phase == DozePhase::DeepIdlePending {
            self.doze_phase = DozePhase::LightIdle;
        }
    }

    // -----------------------------------------------------------------------
    // App bucket management
    // -----------------------------------------------------------------------
    pub fn classify_app(&mut self, app_id: &str, now: u64) -> AppBucket {
        let entry = match self.apps.iter().position(|a| a.app_id == app_id) {
            Some(idx) => &self.apps[idx],
            None => return AppBucket::Never,
        };

        let since_used = now.saturating_sub(entry.last_used_ts);
        let hours_since = since_used / 3600;

        let bucket = if hours_since < 1 {
            AppBucket::Active
        } else if hours_since < 12 {
            AppBucket::WorkingSet
        } else if hours_since < 72 {
            AppBucket::Frequent
        } else if hours_since < 720 {
            AppBucket::Rare
        } else {
            AppBucket::Restricted
        };

        // Apply classification
        if let Some(e) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            e.bucket = bucket;
        }
        bucket
    }

    pub fn reclassify_all(&mut self) {
        let now = crate::time::clock::unix_time();
        let ids: Vec<String> = self.apps.iter().map(|a| a.app_id.clone()).collect();
        for id in ids {
            self.classify_app(&id, now);
        }
    }

    pub fn add_to_whitelist(&mut self, app_id: &str) {
        let s = String::from(app_id);
        if !self.whitelist.contains(&s) {
            self.whitelist.push(s);
        }
    }

    pub fn remove_from_whitelist(&mut self, app_id: &str) {
        self.whitelist.retain(|s| s.as_str() != app_id);
    }

    pub fn is_whitelisted(&self, app_id: &str) -> bool {
        self.whitelist.iter().any(|s| s.as_str() == app_id)
    }

    // -----------------------------------------------------------------------
    // Background execution control
    // -----------------------------------------------------------------------
    pub fn can_run_background(&self, app_id: &str) -> bool {
        if self.is_whitelisted(app_id) {
            return true;
        }

        match self.bg_policy {
            BgLimitPolicy::None => true,
            BgLimitPolicy::Extreme => false,
            BgLimitPolicy::Moderate | BgLimitPolicy::Strict => {
                let entry = match self.apps.iter().find(|a| a.app_id == app_id) {
                    Some(e) => e,
                    None => return false,
                };
                let max_jobs = entry.bucket.max_jobs_per_hour();
                if self.bg_policy == BgLimitPolicy::Strict {
                    entry.bg_jobs_this_period < max_jobs / 2
                } else {
                    entry.bg_jobs_this_period < max_jobs
                }
            }
        }
    }

    pub fn record_bg_job(&mut self, app_id: &str) {
        if let Some(e) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            e.bg_jobs_this_period = e.bg_jobs_this_period.saturating_add(1);
        }
    }

    pub fn reset_period_counters(&mut self, now: u64) {
        self.bucket_period_start_ts = now;
        for app in self.apps.iter_mut() {
            app.bg_jobs_this_period = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Usage recording and prediction
    // -----------------------------------------------------------------------
    pub fn record_app_usage(&mut self, app_id: &str, duration_ms: u64) {
        let now = crate::time::clock::unix_time();

        // Update or create entry
        if let Some(e) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            e.fg_ms += duration_ms;
            e.last_used_ts = now;
            // Update EMA of daily usage (Q16)
            let dur_q16 = q16_from_int(duration_ms as i32);
            let alpha = Q16_ONE / 8; // 0.125 smoothing factor
            e.avg_daily_usage_q16 =
                q16_mul(Q16_ONE - alpha, e.avg_daily_usage_q16) + q16_mul(alpha, dur_q16);
        } else {
            self.apps.push(AppUsageEntry {
                app_id: String::from(app_id),
                bucket: AppBucket::Active,
                cpu_ms: 0,
                wakelock_ms: 0,
                net_bytes: 0,
                fg_ms: duration_ms,
                bg_jobs_this_period: 0,
                last_used_ts: now,
                predicted_next_use_ts: 0,
                avg_daily_usage_q16: q16_from_int(duration_ms as i32),
                ema_drain_q16: Q16_ZERO,
            });
        }

        // Append to history
        let history = self
            .usage_history
            .entry(String::from(app_id))
            .or_insert_with(Vec::new);
        history.push((now, duration_ms));
        // Cap history at 500 entries per app
        if history.len() > 500 {
            history.remove(0);
        }
    }

    pub fn predict_usage(&self, app_id: &str, now: u64) -> Option<UsagePrediction> {
        let entry = self.apps.iter().find(|a| a.app_id == app_id)?;
        let history = self.usage_history.get(app_id)?;

        if history.len() < 3 {
            return None;
        }

        // Simple frequency-based prediction: how often was it used in the
        // same hour-of-day in the past?
        let current_hour = ((now / 3600) % 24) as u32;
        let matching_count = history
            .iter()
            .filter(|(ts, _)| ((*ts / 3600) % 24) as u32 == current_hour)
            .count() as i32;
        let total_days = {
            let first_ts = history.first().map(|(t, _)| *t).unwrap_or(now);
            let span = now.saturating_sub(first_ts);
            (span / 86400).max(1) as i32
        };

        let prob = q16_div(q16_from_int(matching_count), q16_from_int(total_days));
        let prob_clamped = prob.min(Q16_ONE).max(Q16_ZERO);

        let predicted_drain = q16_mul(prob_clamped, entry.ema_drain_q16);

        let suggested = if prob_clamped > Q16_HALF {
            AppBucket::WorkingSet
        } else if prob_clamped > Q16_ONE / 4 {
            AppBucket::Frequent
        } else {
            AppBucket::Rare
        };

        Some(UsagePrediction {
            app_id: String::from(app_id),
            prob_next_hour_q16: prob_clamped,
            predicted_drain_q16: predicted_drain,
            suggested_bucket: suggested,
        })
    }

    pub fn predict_all(&self) -> Vec<UsagePrediction> {
        let now = crate::time::clock::unix_time();
        let mut results = Vec::new();
        for app in &self.apps {
            if let Some(pred) = self.predict_usage(&app.app_id, now) {
                results.push(pred);
            }
        }
        results
    }

    // -----------------------------------------------------------------------
    // Power budget allocation
    // -----------------------------------------------------------------------
    pub fn allocate_budget(&mut self, total_mah_q16: i32) {
        // Foreground gets 50%, background 30%, system 20%
        let fg_share = q16_mul(total_mah_q16, Q16_HALF);
        let bg_share = q16_mul(total_mah_q16, Q16_ONE * 3 / 10);
        let sys_share = total_mah_q16 - fg_share - bg_share;

        // In deep doze, shift more to system, less to background
        let (fg, bg, sys) = match self.doze_phase {
            DozePhase::DeepIdle => {
                let shifted = bg_share / 2;
                (fg_share, bg_share - shifted, sys_share + shifted)
            }
            DozePhase::DeepIdlePending => {
                let shifted = bg_share / 4;
                (fg_share, bg_share - shifted, sys_share + shifted)
            }
            _ => (fg_share, bg_share, sys_share),
        };

        self.budget = PowerBudget {
            total_q16: total_mah_q16,
            foreground_q16: fg,
            background_q16: bg,
            system_q16: sys,
            remaining_q16: total_mah_q16,
        };
    }

    pub fn consume_budget(&mut self, amount_q16: i32, foreground: bool) {
        if foreground {
            self.budget.foreground_q16 = self.budget.foreground_q16.saturating_sub(amount_q16);
        } else {
            self.budget.background_q16 = self.budget.background_q16.saturating_sub(amount_q16);
        }
        self.budget.remaining_q16 = self.budget.remaining_q16.saturating_sub(amount_q16);
    }

    // -----------------------------------------------------------------------
    // Battery level updates
    // -----------------------------------------------------------------------
    pub fn update_battery(&mut self, level: u8, charging: bool) {
        self.battery_level = level;
        self.charging = charging;
        if charging {
            self.doze_phase = DozePhase::FullActive;
            self.bg_policy = BgLimitPolicy::None;
        }
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------
    pub fn app_count(&self) -> usize {
        self.apps.len()
    }

    pub fn restricted_app_count(&self) -> usize {
        self.apps
            .iter()
            .filter(|a| matches!(a.bucket, AppBucket::Restricted | AppBucket::Never))
            .count()
    }

    pub fn whitelist_count(&self) -> usize {
        self.whitelist.len()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------
static OPTIMIZER: Mutex<Option<BatteryOptimizer>> = Mutex::new(None);

pub fn init() {
    let mut opt = BatteryOptimizer::new();
    opt.last_interaction_ts = crate::time::clock::unix_time();
    opt.bucket_period_start_ts = crate::time::clock::unix_time();
    *OPTIMIZER.lock() = Some(opt);
    serial_println!(
        "    [bat-opt] Battery optimizer initialized (doze, buckets, predictions, budgets)"
    );
}

pub fn tick() {
    let now = crate::time::clock::unix_time();
    if let Some(ref mut opt) = *OPTIMIZER.lock() {
        opt.tick(now);
    }
}

pub fn on_user_interaction() {
    if let Some(ref mut opt) = *OPTIMIZER.lock() {
        opt.on_user_interaction();
    }
}

pub fn on_screen_change(on: bool) {
    if let Some(ref mut opt) = *OPTIMIZER.lock() {
        opt.on_screen_change(on);
    }
}

pub fn can_run_background(app_id: &str) -> bool {
    if let Some(ref opt) = *OPTIMIZER.lock() {
        opt.can_run_background(app_id)
    } else {
        true
    }
}

pub fn record_app_usage(app_id: &str, duration_ms: u64) {
    if let Some(ref mut opt) = *OPTIMIZER.lock() {
        opt.record_app_usage(app_id, duration_ms);
    }
}

pub fn update_battery(level: u8, charging: bool) {
    if let Some(ref mut opt) = *OPTIMIZER.lock() {
        opt.update_battery(level, charging);
    }
}
