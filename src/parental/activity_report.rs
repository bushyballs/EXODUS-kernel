use crate::sync::Mutex;
/// Activity reporting for Genesis parental controls
///
/// Weekly reports, app usage summaries, web history,
/// location history, communication monitoring.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct ActivityEntry {
    child_id: u32,
    timestamp: u64,
    event_type: ActivityEvent,
    app_id: Option<u32>,
    duration_secs: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum ActivityEvent {
    AppOpened,
    AppClosed,
    WebVisit,
    SearchQuery,
    AppInstall,
    LocationChange,
    ScreenUnlock,
    BlockedContent,
}

struct WeeklyReport {
    child_id: u32,
    week_start: u64,
    total_screen_min: u32,
    top_apps: [(u32, u32); 5], // (app_id, minutes)
    blocked_attempts: u32,
    searches_count: u32,
    new_apps_installed: u32,
    avg_daily_unlocks: u32,
}

struct ActivityReporter {
    entries: Vec<ActivityEntry>,
    reports: Vec<WeeklyReport>,
    max_entries: usize,
}

static ACTIVITY_REPORT: Mutex<Option<ActivityReporter>> = Mutex::new(None);

impl ActivityReporter {
    fn new() -> Self {
        ActivityReporter {
            entries: Vec::new(),
            reports: Vec::new(),
            max_entries: 5000,
        }
    }

    fn log(
        &mut self,
        child_id: u32,
        event: ActivityEvent,
        app_id: Option<u32>,
        timestamp: u64,
        duration: u32,
    ) {
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(ActivityEntry {
            child_id,
            timestamp,
            event_type: event,
            app_id,
            duration_secs: duration,
        });
    }

    fn screen_time_today(&self, child_id: u32) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.child_id == child_id && e.event_type == ActivityEvent::AppClosed)
            .map(|e| e.duration_secs)
            .sum::<u32>()
            / 60
    }

    fn blocked_today(&self, child_id: u32) -> u32 {
        self.entries
            .iter()
            .filter(|e| e.child_id == child_id && e.event_type == ActivityEvent::BlockedContent)
            .count() as u32
    }

    fn top_apps(&self, child_id: u32, n: usize) -> Vec<(u32, u32)> {
        let mut app_times: Vec<(u32, u32)> = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|e| e.child_id == child_id && e.app_id.is_some())
        {
            let aid = entry.app_id.unwrap();
            if let Some(at) = app_times.iter_mut().find(|(id, _)| *id == aid) {
                at.1 += entry.duration_secs;
            } else {
                app_times.push((aid, entry.duration_secs));
            }
        }
        app_times.sort_by(|a, b| b.1.cmp(&a.1));
        app_times.truncate(n);
        app_times
    }
}

pub fn init() {
    let mut r = ACTIVITY_REPORT.lock();
    *r = Some(ActivityReporter::new());
    serial_println!("    Parental: activity reporting ready");
}
