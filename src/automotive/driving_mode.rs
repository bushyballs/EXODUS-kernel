/// Driving mode for Genesis
///
/// Simplified UI, voice-first interaction, auto-reply,
/// speed-based volume, parking detection.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

struct DrivingMode {
    active: bool,
    auto_activate_above_kph: u32,
    auto_deactivate_below_kph: u32,
    auto_reply_enabled: bool,
    speed_based_volume: bool,
    simplified_ui: bool,
    voice_only: bool,
    block_notifications: bool,
    allow_navigation: bool,
    allow_music: bool,
    allow_calls: bool,
    total_drives: u32,
    total_driving_minutes: u64,
}

static DRIVING: Mutex<Option<DrivingMode>> = Mutex::new(None);

impl DrivingMode {
    fn new() -> Self {
        DrivingMode {
            active: false,
            auto_activate_above_kph: 15,
            auto_deactivate_below_kph: 5,
            auto_reply_enabled: true,
            speed_based_volume: true,
            simplified_ui: true,
            voice_only: false,
            block_notifications: true,
            allow_navigation: true,
            allow_music: true,
            allow_calls: true,
            total_drives: 0,
            total_driving_minutes: 0,
        }
    }

    fn check_speed(&mut self, kph: u32) {
        if !self.active && kph > self.auto_activate_above_kph {
            self.active = true;
            self.total_drives = self.total_drives.saturating_add(1);
        } else if self.active && kph < self.auto_deactivate_below_kph {
            self.active = false;
        }
    }

    fn volume_adjustment(&self, speed_kph: u32) -> i8 {
        if !self.speed_based_volume {
            return 0;
        }
        // Increase volume with speed to compensate for road noise
        match speed_kph {
            0..=30 => 0,
            31..=60 => 2,
            61..=100 => 4,
            _ => 6,
        }
    }
}

pub fn init() {
    let mut d = DRIVING.lock();
    *d = Some(DrivingMode::new());
    serial_println!("    Automotive: driving mode ready");
}
