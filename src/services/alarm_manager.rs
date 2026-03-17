/// Alarm manager for Genesis
///
/// Exact and inexact alarms, repeating alarms,
/// wake-up alarms, and alarm batching for power efficiency.
///
/// Inspired by: Android AlarmManager, iOS UNNotificationTrigger. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Alarm type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmType {
    ElapsedRealtime,
    ElapsedRealtimeWakeup,
    Rtc,
    RtcWakeup,
}

/// An alarm
pub struct Alarm {
    pub id: u32,
    pub app_id: String,
    pub alarm_type: AlarmType,
    pub trigger_time: u64,     // seconds since epoch or uptime
    pub interval: Option<u64>, // repeat interval in seconds
    pub exact: bool,
    pub allow_while_idle: bool,
    pub tag: String,
    pub fired: bool,
}

/// Alarm manager
pub struct AlarmManager {
    pub alarms: Vec<Alarm>,
    pub next_id: u32,
    pub batch_window_ms: u64,
}

impl AlarmManager {
    const fn new() -> Self {
        AlarmManager {
            alarms: Vec::new(),
            next_id: 1,
            batch_window_ms: 60000, // 1 minute batching window
        }
    }

    pub fn set_alarm(
        &mut self,
        app_id: &str,
        alarm_type: AlarmType,
        trigger_time: u64,
        tag: &str,
        exact: bool,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.alarms.push(Alarm {
            id,
            app_id: String::from(app_id),
            alarm_type,
            trigger_time,
            interval: None,
            exact,
            allow_while_idle: false,
            tag: String::from(tag),
            fired: false,
        });
        id
    }

    pub fn set_repeating(
        &mut self,
        app_id: &str,
        alarm_type: AlarmType,
        trigger_time: u64,
        interval: u64,
        tag: &str,
    ) -> u32 {
        let id = self.set_alarm(app_id, alarm_type, trigger_time, tag, false);
        if let Some(alarm) = self.alarms.iter_mut().find(|a| a.id == id) {
            alarm.interval = Some(interval);
        }
        id
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        let len = self.alarms.len();
        self.alarms.retain(|a| a.id != id);
        self.alarms.len() < len
    }

    pub fn cancel_all_for_app(&mut self, app_id: &str) {
        self.alarms.retain(|a| a.app_id != app_id);
    }

    pub fn check_alarms(&mut self) -> Vec<u32> {
        let now = crate::time::clock::unix_time();
        let mut fired = Vec::new();

        for alarm in &mut self.alarms {
            if alarm.fired {
                continue;
            }

            let trigger = if alarm.exact {
                alarm.trigger_time
            } else {
                // Allow batching within window
                alarm.trigger_time + self.batch_window_ms / 1000
            };

            if now >= trigger {
                alarm.fired = true;
                fired.push(alarm.id);

                // Reschedule repeating alarms
                if let Some(interval) = alarm.interval {
                    alarm.trigger_time += interval;
                    alarm.fired = false;
                }
            }
        }

        // Clean up one-shot fired alarms
        self.alarms.retain(|a| !a.fired || a.interval.is_some());
        fired
    }

    pub fn next_alarm_time(&self) -> Option<u64> {
        self.alarms
            .iter()
            .filter(|a| !a.fired)
            .map(|a| a.trigger_time)
            .min()
    }

    pub fn alarm_count(&self) -> usize {
        self.alarms.len()
    }
}

static ALARMS: Mutex<AlarmManager> = Mutex::new(AlarmManager::new());

pub fn init() {
    crate::serial_println!("  [services] Alarm manager initialized");
}

pub fn check() -> Vec<u32> {
    ALARMS.lock().check_alarms()
}
