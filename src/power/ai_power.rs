/// AI-powered power management for Genesis
///
/// Battery prediction, smart charging, adaptive power profiles,
/// app power budgeting, thermal prediction, charge time estimation.
///
/// Inspired by: Android Adaptive Battery, iOS Optimized Charging. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Power profile selected by AI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    UltraPerformance,
    Performance,
    Balanced,
    PowerSaver,
    UltraSaver,
    Sleeping,
}

/// Battery prediction
pub struct BatteryPrediction {
    pub current_percent: u8,
    pub estimated_remaining_min: u32,
    pub estimated_full_charge_min: u32,
    pub drain_rate_per_hour: f32,
    pub will_last_until: u64, // unix timestamp
    pub confidence: f32,
}

/// App power usage record
pub struct AppPowerRecord {
    pub app_name: String,
    pub cpu_time_ms: u64,
    pub wake_locks_ms: u64,
    pub network_bytes: u64,
    pub gps_time_ms: u64,
    pub screen_time_ms: u64,
    pub estimated_mah: f32,
    pub percentage: f32,
}

/// Charging pattern for smart charging
pub struct ChargingPattern {
    pub day: u8,
    pub plug_in_hour: u8,
    pub unplug_hour: u8,
    pub target_percent: u8,
    pub frequency: u32,
}

/// Thermal zone prediction
pub struct ThermalPrediction {
    pub zone: String,
    pub current_temp: f32,
    pub predicted_temp: f32,
    pub time_to_throttle_sec: u32,
    pub recommended_action: String,
}

/// AI power engine
pub struct AiPowerEngine {
    pub enabled: bool,
    pub current_profile: PowerProfile,
    pub battery_history: Vec<(u64, u8)>, // (timestamp, percent)
    pub app_power: Vec<AppPowerRecord>,
    pub charging_patterns: Vec<ChargingPattern>,
    pub thermal_predictions: Vec<ThermalPrediction>,
    pub smart_charging_enabled: bool,
    pub charge_limit: u8,
    pub predicted_drain_rate: f32,
    pub learned_capacity_mah: u32,
    pub charge_cycles: u32,
    pub battery_health_percent: u8,
    pub restricted_apps: Vec<String>,
    pub total_optimizations: u64,
}

impl AiPowerEngine {
    const fn new() -> Self {
        AiPowerEngine {
            enabled: true,
            current_profile: PowerProfile::Balanced,
            battery_history: Vec::new(),
            app_power: Vec::new(),
            charging_patterns: Vec::new(),
            thermal_predictions: Vec::new(),
            smart_charging_enabled: true,
            charge_limit: 100,
            predicted_drain_rate: 5.0,
            learned_capacity_mah: 5000,
            charge_cycles: 0,
            battery_health_percent: 100,
            restricted_apps: Vec::new(),
            total_optimizations: 0,
        }
    }

    /// Record battery level for prediction
    pub fn record_battery(&mut self, percent: u8) {
        let now = crate::time::clock::unix_time();
        self.battery_history.push((now, percent));
        if self.battery_history.len() > 1000 {
            self.battery_history.remove(0);
        }

        // Update drain rate from recent history
        if self.battery_history.len() >= 2 {
            let len = self.battery_history.len();
            let (t1, p1) = self.battery_history[len - 2];
            let (t2, p2) = self.battery_history[len - 1];
            let dt_hours = (t2 - t1) as f32 / 3600.0;
            if dt_hours > 0.0 && p1 > p2 {
                let new_rate = (p1 - p2) as f32 / dt_hours;
                // Exponential moving average
                self.predicted_drain_rate = self.predicted_drain_rate * 0.8 + new_rate * 0.2;
            }
        }
    }

    /// Predict remaining battery time
    pub fn predict_battery(&self) -> BatteryPrediction {
        let current = self.battery_history.last().map(|(_, p)| *p).unwrap_or(100);
        let remaining_min = if self.predicted_drain_rate > 0.0 {
            ((current as f32 / self.predicted_drain_rate) * 60.0) as u32
        } else {
            999
        };
        let now = crate::time::clock::unix_time();
        BatteryPrediction {
            current_percent: current,
            estimated_remaining_min: remaining_min,
            estimated_full_charge_min: ((100 - current) as u32 * 2), // ~2 min per percent
            drain_rate_per_hour: self.predicted_drain_rate,
            will_last_until: now + remaining_min as u64 * 60,
            confidence: if self.battery_history.len() > 10 {
                0.85
            } else {
                0.5
            },
        }
    }

    /// Select optimal power profile based on context
    pub fn select_profile(&mut self, battery: u8, charging: bool, activity: u8) -> PowerProfile {
        self.total_optimizations = self.total_optimizations.saturating_add(1);
        let profile = if charging {
            PowerProfile::Performance
        } else if battery < 10 {
            PowerProfile::UltraSaver
        } else if battery < 20 {
            PowerProfile::PowerSaver
        } else if activity == 0 {
            // idle/sleeping
            PowerProfile::Sleeping
        } else if battery > 80 {
            PowerProfile::Performance
        } else {
            PowerProfile::Balanced
        };
        self.current_profile = profile;
        profile
    }

    /// Get smart charging target (to preserve battery health)
    pub fn smart_charge_target(&self) -> u8 {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;

        // Look for a matching unplug pattern
        for pattern in &self.charging_patterns {
            if pattern.plug_in_hour <= hour && hour < pattern.unplug_hour {
                // We know when they'll unplug — slow charge to reach target just in time
                return pattern.target_percent.min(self.charge_limit);
            }
        }

        // Default: charge to 80% for longevity, unless user overrides
        if self.battery_health_percent > 90 {
            80
        } else {
            self.charge_limit
        }
    }

    /// Record charging pattern
    pub fn record_charge_event(&mut self, plugged_in: bool) {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let day = ((now / 86400) % 7) as u8;
        let current = self.battery_history.last().map(|(_, p)| *p).unwrap_or(50);

        if plugged_in {
            if let Some(pattern) = self
                .charging_patterns
                .iter_mut()
                .find(|p| p.day == day && (p.plug_in_hour as i8 - hour as i8).abs() <= 1)
            {
                pattern.frequency = pattern.frequency.saturating_add(1);
                pattern.plug_in_hour = hour;
            } else {
                self.charging_patterns.push(ChargingPattern {
                    day,
                    plug_in_hour: hour,
                    unplug_hour: hour + 8,
                    target_percent: 100,
                    frequency: 1,
                });
            }
        } else {
            if let Some(pattern) = self
                .charging_patterns
                .iter_mut()
                .find(|p| p.day == day && p.plug_in_hour < hour)
            {
                pattern.unplug_hour = hour;
                pattern.target_percent = current;
            }
            self.charge_cycles = self.charge_cycles.saturating_add(1);
        }
    }

    /// Get top power-consuming apps
    pub fn top_power_apps(&self, count: usize) -> Vec<&AppPowerRecord> {
        let mut sorted: Vec<&AppPowerRecord> = self.app_power.iter().collect();
        sorted.sort_by(|a, b| {
            b.estimated_mah
                .partial_cmp(&a.estimated_mah)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        sorted.truncate(count);
        sorted
    }

    /// Should an app be restricted for power savings?
    pub fn should_restrict_app(&self, app_name: &str) -> bool {
        if matches!(
            self.current_profile,
            PowerProfile::UltraSaver | PowerProfile::PowerSaver
        ) {
            self.restricted_apps.contains(&String::from(app_name))
        } else {
            false
        }
    }

    /// Restrict a power-hungry background app
    pub fn restrict_app(&mut self, app_name: &str) {
        if !self.restricted_apps.contains(&String::from(app_name)) {
            self.restricted_apps.push(String::from(app_name));
        }
    }
}

static AI_POWER: Mutex<AiPowerEngine> = Mutex::new(AiPowerEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-power] AI power management initialized (predict, smart charge, thermal)"
    );
}

pub fn record_battery(percent: u8) {
    AI_POWER.lock().record_battery(percent);
}

pub fn predict_battery() -> BatteryPrediction {
    AI_POWER.lock().predict_battery()
}

pub fn select_profile(battery: u8, charging: bool, activity: u8) -> PowerProfile {
    AI_POWER.lock().select_profile(battery, charging, activity)
}

pub fn smart_charge_target() -> u8 {
    AI_POWER.lock().smart_charge_target()
}

pub fn record_charge_event(plugged: bool) {
    AI_POWER.lock().record_charge_event(plugged);
}
