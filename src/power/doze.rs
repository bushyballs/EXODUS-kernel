/// Doze mode / Adaptive battery for Genesis
///
/// Deep sleep optimization, app standby buckets,
/// battery saver, thermal management, and power stats.
///
/// Inspired by: Android Doze, iOS Low Power Mode. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Doze state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DozeState {
    Active,
    Inactive,
    IdlePending,
    Idle,
    IdleMaintenance,
}

/// App standby bucket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandbyBucket {
    Active,
    WorkingSet,
    Frequent,
    Rare,
    Restricted,
    Never,
}

/// Battery saver level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatterySaverLevel {
    Off,
    Moderate,
    Extreme,
}

/// Thermal status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalStatus {
    None,
    Light,
    Moderate,
    Severe,
    Critical,
    Emergency,
    Shutdown,
}

/// Per-app power usage
pub struct AppPowerUsage {
    pub app_id: String,
    pub cpu_ms: u64,
    pub wakelock_ms: u64,
    pub network_bytes: u64,
    pub gps_ms: u64,
    pub sensor_ms: u64,
    pub foreground_ms: u64,
    pub bucket: StandbyBucket,
}

/// Power manager (doze + battery saver + thermal)
pub struct PowerManager {
    pub doze_state: DozeState,
    pub battery_level: u8,
    pub charging: bool,
    pub battery_saver: BatterySaverLevel,
    pub battery_saver_threshold: u8,
    pub auto_battery_saver: bool,
    pub thermal_status: ThermalStatus,
    pub screen_on: bool,
    pub last_user_activity: u64,
    pub idle_timeout_s: u64,
    pub app_usage: Vec<AppPowerUsage>,
    pub wakelocks: BTreeMap<String, u64>, // tag -> acquired timestamp
    pub cpu_freq_limit: Option<u32>,      // MHz limit during thermal throttle
}

impl PowerManager {
    const fn new() -> Self {
        PowerManager {
            doze_state: DozeState::Active,
            battery_level: 100,
            charging: false,
            battery_saver: BatterySaverLevel::Off,
            battery_saver_threshold: 15,
            auto_battery_saver: true,
            thermal_status: ThermalStatus::None,
            screen_on: true,
            last_user_activity: 0,
            idle_timeout_s: 300,
            app_usage: Vec::new(),
            wakelocks: BTreeMap::new(),
            cpu_freq_limit: None,
        }
    }

    pub fn tick(&mut self) {
        let now = crate::time::clock::unix_time();
        let idle_time = now.saturating_sub(self.last_user_activity);

        // Doze state machine
        match self.doze_state {
            DozeState::Active => {
                if !self.screen_on && idle_time > self.idle_timeout_s {
                    self.doze_state = DozeState::Inactive;
                }
            }
            DozeState::Inactive => {
                if idle_time > self.idle_timeout_s * 2 {
                    self.doze_state = DozeState::IdlePending;
                }
            }
            DozeState::IdlePending => {
                if idle_time > self.idle_timeout_s * 4 {
                    self.doze_state = DozeState::Idle;
                }
            }
            DozeState::Idle => {
                // Periodic maintenance windows
                if idle_time % 900 < 60 {
                    // 15min cycle, 1min window
                    self.doze_state = DozeState::IdleMaintenance;
                }
            }
            DozeState::IdleMaintenance => {
                self.doze_state = DozeState::Idle;
            }
        }

        // Auto battery saver
        if self.auto_battery_saver && !self.charging {
            if self.battery_level <= self.battery_saver_threshold
                && self.battery_saver == BatterySaverLevel::Off
            {
                self.battery_saver = BatterySaverLevel::Moderate;
            }
        }

        // Disable battery saver when charging
        if self.charging && self.battery_saver != BatterySaverLevel::Off {
            self.battery_saver = BatterySaverLevel::Off;
        }
    }

    pub fn on_user_activity(&mut self) {
        self.last_user_activity = crate::time::clock::unix_time();
        self.doze_state = DozeState::Active;
    }

    pub fn on_screen_change(&mut self, on: bool) {
        self.screen_on = on;
        if on {
            self.on_user_activity();
        }
    }

    pub fn acquire_wakelock(&mut self, tag: &str) {
        self.wakelocks
            .insert(String::from(tag), crate::time::clock::unix_time());
    }

    pub fn release_wakelock(&mut self, tag: &str) {
        self.wakelocks.remove(tag);
    }

    pub fn set_app_bucket(&mut self, app_id: &str, bucket: StandbyBucket) {
        if let Some(usage) = self.app_usage.iter_mut().find(|a| a.app_id == app_id) {
            usage.bucket = bucket;
        } else {
            self.app_usage.push(AppPowerUsage {
                app_id: String::from(app_id),
                cpu_ms: 0,
                wakelock_ms: 0,
                network_bytes: 0,
                gps_ms: 0,
                sensor_ms: 0,
                foreground_ms: 0,
                bucket,
            });
        }
    }

    pub fn update_thermal(&mut self, status: ThermalStatus) {
        self.thermal_status = status;
        self.cpu_freq_limit = match status {
            ThermalStatus::None | ThermalStatus::Light => None,
            ThermalStatus::Moderate => Some(2000),
            ThermalStatus::Severe => Some(1500),
            ThermalStatus::Critical => Some(1000),
            ThermalStatus::Emergency | ThermalStatus::Shutdown => Some(500),
        };
    }

    pub fn can_run_background(&self, app_id: &str) -> bool {
        if self.doze_state == DozeState::Idle {
            return false;
        }
        if self.battery_saver == BatterySaverLevel::Extreme {
            return false;
        }
        let bucket = self
            .app_usage
            .iter()
            .find(|a| a.app_id == app_id)
            .map(|a| a.bucket)
            .unwrap_or(StandbyBucket::Rare);
        !matches!(bucket, StandbyBucket::Restricted | StandbyBucket::Never)
    }

    pub fn active_wakelocks(&self) -> usize {
        self.wakelocks.len()
    }
}

static POWER: Mutex<PowerManager> = Mutex::new(PowerManager::new());

pub fn init() {
    POWER.lock().last_user_activity = crate::time::clock::unix_time();
    crate::serial_println!("  [power] Doze + adaptive battery + thermal initialized");
}

pub fn tick() {
    POWER.lock().tick();
}
pub fn on_user_activity() {
    POWER.lock().on_user_activity();
}
