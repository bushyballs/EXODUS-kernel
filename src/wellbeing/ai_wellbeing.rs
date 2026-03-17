use crate::sync::Mutex;
/// AI-enhanced digital wellbeing for Genesis
///
/// Usage pattern prediction, smart breaks,
/// addiction detection, habit recommendations.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum UsagePattern {
    Healthy,
    Moderate,
    Excessive,
    Addictive,
}

struct AppHabit {
    app_id: u32,
    avg_daily_min: u32,
    avg_sessions: u32,
    avg_session_length_min: u32,
    longest_session_min: u32,
    late_night_use_count: u32,
    compulsive_checks: u32, // opens < 30 sec
}

struct AiWellbeingEngine {
    habits: Vec<AppHabit>,
    break_suggestion_interval_min: u32,
    last_break_time: u64,
    total_breaks_taken: u32,
    total_breaks_skipped: u32,
    continuous_use_min: u32,
}

static AI_WELLBEING: Mutex<Option<AiWellbeingEngine>> = Mutex::new(None);

impl AiWellbeingEngine {
    fn new() -> Self {
        AiWellbeingEngine {
            habits: Vec::new(),
            break_suggestion_interval_min: 45,
            last_break_time: 0,
            total_breaks_taken: 0,
            total_breaks_skipped: 0,
            continuous_use_min: 0,
        }
    }

    fn classify_app_usage(&self, app_id: u32) -> UsagePattern {
        if let Some(habit) = self.habits.iter().find(|h| h.app_id == app_id) {
            let mut risk = 0u32;
            if habit.avg_daily_min > 120 {
                risk += 20;
            }
            if habit.avg_daily_min > 240 {
                risk += 20;
            }
            if habit.avg_sessions > 20 {
                risk += 15;
            }
            if habit.compulsive_checks > 10 {
                risk += 20;
            }
            if habit.late_night_use_count > 5 {
                risk += 15;
            }
            if habit.longest_session_min > 180 {
                risk += 10;
            }

            if risk > 60 {
                UsagePattern::Addictive
            } else if risk > 35 {
                UsagePattern::Excessive
            } else if risk > 15 {
                UsagePattern::Moderate
            } else {
                UsagePattern::Healthy
            }
        } else {
            UsagePattern::Healthy
        }
    }

    fn should_suggest_break(&self, current_time: u64) -> bool {
        let elapsed_min = (current_time.saturating_sub(self.last_break_time)) / 60;
        elapsed_min as u32 >= self.break_suggestion_interval_min && self.continuous_use_min > 30
    }

    fn record_break(&mut self, timestamp: u64) {
        self.last_break_time = timestamp;
        self.total_breaks_taken = self.total_breaks_taken.saturating_add(1);
        self.continuous_use_min = 0;
    }

    fn update_usage(&mut self, app_id: u32, session_min: u32, is_late_night: bool) {
        self.continuous_use_min += session_min;
        if let Some(habit) = self.habits.iter_mut().find(|h| h.app_id == app_id) {
            habit.avg_sessions = habit.avg_sessions.saturating_add(1);
            habit.avg_daily_min += session_min;
            if session_min > habit.longest_session_min {
                habit.longest_session_min = session_min;
            }
            if is_late_night {
                habit.late_night_use_count = habit.late_night_use_count.saturating_add(1);
            }
            if session_min == 0 {
                habit.compulsive_checks = habit.compulsive_checks.saturating_add(1);
            }
        } else if self.habits.len() < 100 {
            self.habits.push(AppHabit {
                app_id,
                avg_daily_min: session_min,
                avg_sessions: 1,
                avg_session_length_min: session_min,
                longest_session_min: session_min,
                late_night_use_count: if is_late_night { 1 } else { 0 },
                compulsive_checks: if session_min == 0 { 1 } else { 0 },
            });
        }
    }
}

pub fn init() {
    let mut engine = AI_WELLBEING.lock();
    *engine = Some(AiWellbeingEngine::new());
    serial_println!("    AI wellbeing: usage patterns, break suggestions, habit analysis ready");
}
