use crate::sync::Mutex;
/// Sleep analysis for Genesis
///
/// Sleep stage detection, sleep quality scoring,
/// bedtime reminders, smart alarm, snoring detection.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SleepStage {
    Awake,
    Light,
    Deep,
    Rem,
}

#[derive(Clone, Copy)]
struct SleepSegment {
    stage: SleepStage,
    start_time: u64,
    duration_min: u32,
}

#[derive(Clone, Copy)]
pub struct SleepSession {
    pub start_time: u64,
    pub end_time: u64,
    pub total_min: u32,
    pub light_min: u32,
    pub deep_min: u32,
    pub rem_min: u32,
    pub awake_min: u32,
    pub quality_score: u32, // 0-100
    pub snoring_min: u32,
    pub interruptions: u32,
}

struct SleepEngine {
    sessions: Vec<SleepSession>,
    current_segments: Vec<SleepSegment>,
    is_sleeping: bool,
    bedtime_reminder_hour: u8,
    bedtime_reminder_min: u8,
    smart_alarm_enabled: bool,
    smart_alarm_window_min: u8, // wake within N min of alarm
    avg_sleep_quality: u32,
}

static SLEEP: Mutex<Option<SleepEngine>> = Mutex::new(None);

impl SleepEngine {
    fn new() -> Self {
        SleepEngine {
            sessions: Vec::new(),
            current_segments: Vec::new(),
            is_sleeping: false,
            bedtime_reminder_hour: 22,
            bedtime_reminder_min: 30,
            smart_alarm_enabled: true,
            smart_alarm_window_min: 30,
            avg_sleep_quality: 0,
        }
    }

    fn start_sleep(&mut self, timestamp: u64) {
        self.is_sleeping = true;
        self.current_segments.clear();
        self.current_segments.push(SleepSegment {
            stage: SleepStage::Light,
            start_time: timestamp,
            duration_min: 0,
        });
    }

    fn record_stage(&mut self, stage: SleepStage, duration_min: u32, timestamp: u64) {
        if self.is_sleeping {
            self.current_segments.push(SleepSegment {
                stage,
                start_time: timestamp,
                duration_min,
            });
        }
    }

    fn end_sleep(&mut self, timestamp: u64) -> Option<SleepSession> {
        if !self.is_sleeping {
            return None;
        }
        self.is_sleeping = false;

        let start = self.current_segments.first()?.start_time;
        let mut light = 0u32;
        let mut deep = 0u32;
        let mut rem = 0u32;
        let mut awake = 0u32;
        let mut interruptions = 0u32;

        for seg in &self.current_segments {
            match seg.stage {
                SleepStage::Light => light += seg.duration_min,
                SleepStage::Deep => deep += seg.duration_min,
                SleepStage::Rem => rem += seg.duration_min,
                SleepStage::Awake => {
                    awake += seg.duration_min;
                    interruptions += 1;
                }
            }
        }

        let total = light + deep + rem + awake;
        // Quality: deep + rem should be ~40-50% of total
        let restorative_pct = if total > 0 {
            (deep + rem) * 100 / total
        } else {
            0
        };
        let quality = (restorative_pct * 2).min(100) - (interruptions * 5).min(30);

        let session = SleepSession {
            start_time: start,
            end_time: timestamp,
            total_min: total,
            light_min: light,
            deep_min: deep,
            rem_min: rem,
            awake_min: awake,
            quality_score: quality,
            snoring_min: 0,
            interruptions,
        };

        if self.sessions.len() < 365 {
            self.sessions.push(session);
        }
        // Update average
        let count = self.sessions.len() as u32;
        let sum: u32 = self.sessions.iter().map(|s| s.quality_score).sum();
        self.avg_sleep_quality = sum / count.max(1);

        Some(session)
    }

    fn should_trigger_smart_alarm(&self, alarm_time: u64, current_time: u64) -> bool {
        if !self.smart_alarm_enabled || !self.is_sleeping {
            return false;
        }
        let window = self.smart_alarm_window_min as u64 * 60;
        // In the wake window and in light sleep
        if current_time >= alarm_time.saturating_sub(window) && current_time <= alarm_time {
            if let Some(seg) = self.current_segments.last() {
                return seg.stage == SleepStage::Light || seg.stage == SleepStage::Awake;
            }
        }
        false
    }
}

pub fn init() {
    let mut s = SLEEP.lock();
    *s = Some(SleepEngine::new());
    serial_println!("    Health: sleep analysis (stages, quality, smart alarm) ready");
}
