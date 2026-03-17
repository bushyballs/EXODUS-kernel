/// Wind down / bedtime mode for Genesis
///
/// Grayscale display, blue light filter scheduling,
/// bedtime reminders, gradual notification reduction.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

struct WindDownConfig {
    enabled: bool,
    start_hour: u8,
    start_min: u8,
    end_hour: u8,
    end_min: u8,
    grayscale: bool,
    blue_light_filter: bool,
    blue_light_intensity: u8, // 0-100
    reduce_brightness: bool,
    silence_notifications: bool,
    bedtime_reminder: bool,
    reminder_minutes_before: u8,
}

struct WindDownEngine {
    config: WindDownConfig,
    active: bool,
    nights_used: u32,
}

static WIND_DOWN: Mutex<Option<WindDownEngine>> = Mutex::new(None);

impl WindDownEngine {
    fn new() -> Self {
        WindDownEngine {
            config: WindDownConfig {
                enabled: false,
                start_hour: 22,
                start_min: 0,
                end_hour: 7,
                end_min: 0,
                grayscale: true,
                blue_light_filter: true,
                blue_light_intensity: 70,
                reduce_brightness: true,
                silence_notifications: false,
                bedtime_reminder: true,
                reminder_minutes_before: 30,
            },
            active: false,
            nights_used: 0,
        }
    }

    fn should_activate(&self, hour: u8, min: u8) -> bool {
        if !self.config.enabled {
            return false;
        }
        let current = hour as u16 * 60 + min as u16;
        let start = self.config.start_hour as u16 * 60 + self.config.start_min as u16;
        let end = self.config.end_hour as u16 * 60 + self.config.end_min as u16;
        if start > end {
            // Crosses midnight
            current >= start || current < end
        } else {
            current >= start && current < end
        }
    }

    fn should_remind(&self, hour: u8, min: u8) -> bool {
        if !self.config.bedtime_reminder || !self.config.enabled {
            return false;
        }
        let current = hour as u16 * 60 + min as u16;
        let start = self.config.start_hour as u16 * 60 + self.config.start_min as u16;
        let reminder_time = start.saturating_sub(self.config.reminder_minutes_before as u16);
        current >= reminder_time && current < start
    }

    fn get_blue_light_level(&self, hour: u8, min: u8) -> u8 {
        if !self.active || !self.config.blue_light_filter {
            return 0;
        }
        // Gradual increase: ramp up over first hour
        let current = hour as u16 * 60 + min as u16;
        let start = self.config.start_hour as u16 * 60 + self.config.start_min as u16;
        let elapsed = if current >= start {
            current - start
        } else {
            current + 24 * 60 - start
        };
        if elapsed < 60 {
            // Ramp up
            ((elapsed as u32 * self.config.blue_light_intensity as u32) / 60) as u8
        } else {
            self.config.blue_light_intensity
        }
    }
}

pub fn init() {
    let mut w = WIND_DOWN.lock();
    *w = Some(WindDownEngine::new());
    serial_println!("    Wellbeing: wind down / bedtime mode ready");
}
