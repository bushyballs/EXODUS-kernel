use crate::sync::Mutex;
/// Screen time tracking for Genesis
///
/// Per-app usage, daily/weekly reports, app limits,
/// unlock counting, pickup tracking.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct AppUsage {
    app_id: u32,
    daily_seconds: u32,
    weekly_seconds: u32,
    opens_today: u32,
    limit_seconds: Option<u32>,
    notifications_today: u32,
}

struct DailySummary {
    date_days: u32,
    total_screen_seconds: u32,
    total_unlocks: u32,
    total_pickups: u32,
    total_notifications: u32,
    app_count_used: u16,
}

struct ScreenTimeEngine {
    app_usage: Vec<AppUsage>,
    daily_summaries: Vec<DailySummary>,
    current_day_seconds: u32,
    unlocks_today: u32,
    pickups_today: u32,
    daily_limit_seconds: Option<u32>,
    screen_on: bool,
    screen_on_since: u64,
}

static SCREEN_TIME: Mutex<Option<ScreenTimeEngine>> = Mutex::new(None);

impl ScreenTimeEngine {
    fn new() -> Self {
        ScreenTimeEngine {
            app_usage: Vec::new(),
            daily_summaries: Vec::new(),
            current_day_seconds: 0,
            unlocks_today: 0,
            pickups_today: 0,
            daily_limit_seconds: None,
            screen_on: false,
            screen_on_since: 0,
        }
    }

    fn record_app_foreground(&mut self, app_id: u32, duration_secs: u32) {
        if let Some(app) = self.app_usage.iter_mut().find(|a| a.app_id == app_id) {
            app.daily_seconds += duration_secs;
            app.weekly_seconds += duration_secs;
            app.opens_today = app.opens_today.saturating_add(1);
        } else {
            self.app_usage.push(AppUsage {
                app_id,
                daily_seconds: duration_secs,
                weekly_seconds: duration_secs,
                opens_today: 1,
                limit_seconds: None,
                notifications_today: 0,
            });
        }
        self.current_day_seconds += duration_secs;
    }

    fn is_app_over_limit(&self, app_id: u32) -> bool {
        self.app_usage
            .iter()
            .find(|a| a.app_id == app_id)
            .and_then(|a| a.limit_seconds.map(|limit| a.daily_seconds >= limit))
            .unwrap_or(false)
    }

    fn set_app_limit(&mut self, app_id: u32, limit_secs: u32) {
        if let Some(app) = self.app_usage.iter_mut().find(|a| a.app_id == app_id) {
            app.limit_seconds = Some(limit_secs);
        }
    }

    fn record_unlock(&mut self) {
        self.unlocks_today = self.unlocks_today.saturating_add(1);
    }

    fn record_pickup(&mut self) {
        self.pickups_today = self.pickups_today.saturating_add(1);
    }

    fn is_over_daily_limit(&self) -> bool {
        self.daily_limit_seconds
            .map_or(false, |limit| self.current_day_seconds >= limit)
    }

    fn end_day(&mut self, date_days: u32) {
        if self.daily_summaries.len() < 90 {
            self.daily_summaries.push(DailySummary {
                date_days,
                total_screen_seconds: self.current_day_seconds,
                total_unlocks: self.unlocks_today,
                total_pickups: self.pickups_today,
                total_notifications: self.app_usage.iter().map(|a| a.notifications_today).sum(),
                app_count_used: self
                    .app_usage
                    .iter()
                    .filter(|a| a.daily_seconds > 0)
                    .count() as u16,
            });
        }
        self.current_day_seconds = 0;
        self.unlocks_today = 0;
        self.pickups_today = 0;
        for app in self.app_usage.iter_mut() {
            app.daily_seconds = 0;
            app.opens_today = 0;
            app.notifications_today = 0;
        }
    }
}

pub fn init() {
    let mut s = SCREEN_TIME.lock();
    *s = Some(ScreenTimeEngine::new());
    serial_println!("    Wellbeing: screen time tracking ready");
}
