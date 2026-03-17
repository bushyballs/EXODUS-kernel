use crate::sync::Mutex;
/// AI-enhanced system services for Genesis
///
/// Intelligent job scheduling, predictive alarms,
/// smart download optimization, location prediction,
/// and sensor fusion with ML.
///
/// Original implementation for Hoags OS.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Job Intelligence ──

#[derive(Clone, Copy, PartialEq)]
pub enum JobPriority {
    Critical,
    High,
    Normal,
    Low,
    Opportunistic,
}

#[derive(Clone, Copy, PartialEq)]
pub enum JobConstraint {
    RequiresCharging,
    RequiresNetwork,
    RequiresIdle,
    RequiresStorage,
    TimeWindow,
}

struct SmartJob {
    id: u32,
    name: [u8; 32],
    name_len: usize,
    priority: JobPriority,
    constraints: [Option<JobConstraint>; 4],
    avg_duration_ms: u64,
    avg_battery_drain: u32, // millipercent
    success_rate: u32,      // 0-1000 (x10 for precision)
    last_run: u64,
    run_count: u32,
    preferred_hour: u8,        // learned from patterns
    preferred_battery_min: u8, // minimum battery for this job
}

// ── Location Prediction ──

#[derive(Clone, Copy)]
struct LocationPattern {
    lat_x1000: i32,
    lon_x1000: i32,
    hour: u8,
    day_of_week: u8,
    visit_count: u32,
    avg_duration_min: u32,
}

#[derive(Clone, Copy)]
pub struct LocationPrediction {
    pub lat_x1000: i32,
    pub lon_x1000: i32,
    pub confidence: u32, // 0-100
    pub predicted_duration_min: u32,
}

// ── Sensor Fusion ──

#[derive(Clone, Copy, PartialEq)]
pub enum ActivityType {
    Still,
    Walking,
    Running,
    Cycling,
    Driving,
    Unknown,
}

#[derive(Clone, Copy)]
struct SensorReading {
    accel_x: i32, // milli-g
    accel_y: i32,
    accel_z: i32,
    gyro_x: i32, // milli-deg/s
    gyro_y: i32,
    gyro_z: i32,
    timestamp: u64,
}

// ── Download Intelligence ──

#[derive(Clone, Copy)]
struct DownloadPattern {
    hour: u8,
    avg_speed_kbps: u32,
    congestion_score: u32, // 0-100
    sample_count: u32,
}

// ── Main Engine ──

struct AiServicesEngine {
    jobs: Vec<SmartJob>,
    location_patterns: Vec<LocationPattern>,
    sensor_history: Vec<SensorReading>,
    download_patterns: [DownloadPattern; 24],
    current_activity: ActivityType,
    activity_confidence: u32,
}

static AI_SERVICES: Mutex<Option<AiServicesEngine>> = Mutex::new(None);

impl AiServicesEngine {
    fn new() -> Self {
        let mut download_patterns = [DownloadPattern {
            hour: 0,
            avg_speed_kbps: 0,
            congestion_score: 0,
            sample_count: 0,
        }; 24];
        for i in 0..24 {
            download_patterns[i].hour = i as u8;
        }
        AiServicesEngine {
            jobs: Vec::new(),
            location_patterns: Vec::new(),
            sensor_history: Vec::new(),
            download_patterns,
            current_activity: ActivityType::Still,
            activity_confidence: 0,
        }
    }

    /// Schedule a job intelligently based on learned patterns
    fn optimal_schedule_time(&self, job_id: u32, current_hour: u8, battery_pct: u8) -> u8 {
        if let Some(job) = self.jobs.iter().find(|j| j.id == job_id) {
            // If job needs charging and battery is low, defer
            let has_charging = job
                .constraints
                .iter()
                .any(|c| *c == Some(JobConstraint::RequiresCharging));
            if has_charging && battery_pct < 80 {
                // Schedule for typical charging time (overnight)
                return 2; // 2 AM
            }
            // Use learned preferred hour
            if job.run_count > 3 {
                return job.preferred_hour;
            }
            // Default: schedule for low-activity period
            if current_hour < 6 {
                return current_hour;
            }
            return 3; // 3 AM default
        }
        current_hour
    }

    /// Learn job execution patterns
    fn record_job_result(
        &mut self,
        job_id: u32,
        duration_ms: u64,
        battery_drain: u32,
        success: bool,
        hour: u8,
    ) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            // Exponential moving average for duration
            if job.run_count > 0 {
                job.avg_duration_ms = (job.avg_duration_ms * 7 + duration_ms) / 8;
                job.avg_battery_drain = (job.avg_battery_drain * 7 + battery_drain) / 8;
            } else {
                job.avg_duration_ms = duration_ms;
                job.avg_battery_drain = battery_drain;
            }
            // Success rate
            let success_val = if success { 1000u32 } else { 0 };
            job.success_rate =
                (job.success_rate * job.run_count + success_val) / (job.run_count + 1);
            job.run_count = job.run_count.saturating_add(1);
            // Learn preferred hour (mode tracking via bias toward frequent hours)
            if job.run_count > 2 {
                let diff = if hour > job.preferred_hour {
                    hour - job.preferred_hour
                } else {
                    job.preferred_hour - hour
                };
                if diff < 3 {
                    // Close to current preferred, nudge toward this hour
                    job.preferred_hour = (job.preferred_hour as u16 * 3 + hour as u16) as u8 / 4;
                }
            } else {
                job.preferred_hour = hour;
            }
        }
    }

    /// Predict user's next location based on time patterns
    fn predict_location(&self, hour: u8, day_of_week: u8) -> Option<LocationPrediction> {
        let mut best: Option<(&LocationPattern, u32)> = None;
        for pattern in &self.location_patterns {
            if pattern.visit_count < 2 {
                continue;
            }
            let hour_diff = if hour > pattern.hour {
                hour - pattern.hour
            } else {
                pattern.hour - hour
            };
            let day_match = if pattern.day_of_week == day_of_week {
                20
            } else {
                0
            };
            let score = pattern.visit_count * 10 + day_match - hour_diff as u32 * 5;
            if let Some((_, best_score)) = best {
                if score > best_score {
                    best = Some((pattern, score));
                }
            } else {
                best = Some((pattern, score));
            }
        }
        best.map(|(p, score)| LocationPrediction {
            lat_x1000: p.lat_x1000,
            lon_x1000: p.lon_x1000,
            confidence: (score).min(100) as u32,
            predicted_duration_min: p.avg_duration_min,
        })
    }

    /// Detect user activity from sensor data
    fn detect_activity(&mut self) -> ActivityType {
        if self.sensor_history.len() < 10 {
            return ActivityType::Unknown;
        }
        let recent = &self.sensor_history[self.sensor_history.len().saturating_sub(20)..];

        // Calculate acceleration magnitude variance
        let mut magnitudes = [0i64; 20];
        let count = recent.len().min(20);
        let mut sum = 0i64;
        for (i, r) in recent.iter().enumerate().take(count) {
            let mag =
                (r.accel_x as i64).pow(2) + (r.accel_y as i64).pow(2) + (r.accel_z as i64).pow(2);
            magnitudes[i] = mag;
            sum += mag;
        }
        let mean = sum / count as i64;
        let mut variance = 0i64;
        for i in 0..count {
            let diff = magnitudes[i] - mean;
            variance += diff * diff;
        }
        variance /= count as i64;

        // Calculate gyroscope activity
        let mut gyro_activity = 0i64;
        for r in recent {
            gyro_activity += (r.gyro_x.abs() + r.gyro_y.abs() + r.gyro_z.abs()) as i64;
        }
        gyro_activity /= count as i64;

        // Classify based on thresholds
        let activity = if variance < 1000 && gyro_activity < 50 {
            ActivityType::Still
        } else if variance < 50000 && gyro_activity < 200 {
            ActivityType::Walking
        } else if variance < 200000 {
            if gyro_activity > 500 {
                ActivityType::Cycling
            } else {
                ActivityType::Running
            }
        } else {
            ActivityType::Driving
        };

        self.current_activity = activity;
        self.activity_confidence = 70; // base confidence
        activity
    }

    /// Find optimal download time based on learned network patterns
    fn best_download_hour(&self, _size_kb: u64) -> u8 {
        let mut best_hour = 0u8;
        let mut best_score = 0u32;
        for pattern in &self.download_patterns {
            if pattern.sample_count < 2 {
                continue;
            }
            // Score = speed * (100 - congestion)
            let score = pattern.avg_speed_kbps * (100 - pattern.congestion_score.min(99));
            if score > best_score {
                best_score = score;
                best_hour = pattern.hour;
            }
        }
        best_hour
    }

    /// Record network speed observation
    fn record_network_speed(&mut self, hour: u8, speed_kbps: u32, congested: bool) {
        let idx = (hour % 24) as usize;
        let p = &mut self.download_patterns[idx];
        let congestion_val = if congested { 100u32 } else { 0 };
        if p.sample_count > 0 {
            p.avg_speed_kbps =
                (p.avg_speed_kbps * p.sample_count + speed_kbps) / (p.sample_count + 1);
            p.congestion_score =
                (p.congestion_score * p.sample_count + congestion_val) / (p.sample_count + 1);
        } else {
            p.avg_speed_kbps = speed_kbps;
            p.congestion_score = congestion_val;
        }
        p.sample_count = p.sample_count.saturating_add(1);
    }

    /// Record a location visit for pattern learning
    fn record_location(
        &mut self,
        lat_x1000: i32,
        lon_x1000: i32,
        hour: u8,
        day_of_week: u8,
        duration_min: u32,
    ) {
        // Check if we already have a pattern for this location (within ~100m)
        for pattern in self.location_patterns.iter_mut() {
            let dlat = (pattern.lat_x1000 - lat_x1000).abs();
            let dlon = (pattern.lon_x1000 - lon_x1000).abs();
            if dlat < 1 && dlon < 1 {
                // Update existing pattern
                pattern.visit_count = pattern.visit_count.saturating_add(1);
                pattern.avg_duration_min = (pattern.avg_duration_min * (pattern.visit_count - 1)
                    + duration_min)
                    / pattern.visit_count;
                return;
            }
        }
        // New location
        if self.location_patterns.len() < 100 {
            self.location_patterns.push(LocationPattern {
                lat_x1000,
                lon_x1000,
                hour,
                day_of_week,
                visit_count: 1,
                avg_duration_min: duration_min,
            });
        }
    }

    /// Add sensor reading for activity detection
    fn add_sensor_reading(
        &mut self,
        accel_x: i32,
        accel_y: i32,
        accel_z: i32,
        gyro_x: i32,
        gyro_y: i32,
        gyro_z: i32,
        timestamp: u64,
    ) {
        if self.sensor_history.len() >= 200 {
            self.sensor_history.remove(0);
        }
        self.sensor_history.push(SensorReading {
            accel_x,
            accel_y,
            accel_z,
            gyro_x,
            gyro_y,
            gyro_z,
            timestamp,
        });
    }
}

pub fn init() {
    let mut engine = AI_SERVICES.lock();
    *engine = Some(AiServicesEngine::new());
    serial_println!("    AI services: job intelligence, location prediction, sensor fusion ready");
}

/// Get the optimal schedule time for a job
pub fn optimal_job_time(job_id: u32, current_hour: u8, battery_pct: u8) -> u8 {
    let engine = AI_SERVICES.lock();
    engine
        .as_ref()
        .map(|e| e.optimal_schedule_time(job_id, current_hour, battery_pct))
        .unwrap_or(current_hour)
}

/// Predict user location
pub fn predict_location(hour: u8, day: u8) -> Option<LocationPrediction> {
    let engine = AI_SERVICES.lock();
    engine.as_ref().and_then(|e| e.predict_location(hour, day))
}

/// Detect current activity from sensors
pub fn detect_activity() -> ActivityType {
    let mut engine = AI_SERVICES.lock();
    engine
        .as_mut()
        .map(|e| e.detect_activity())
        .unwrap_or(ActivityType::Unknown)
}

/// Find best hour for large downloads
pub fn best_download_hour(size_kb: u64) -> u8 {
    let engine = AI_SERVICES.lock();
    engine
        .as_ref()
        .map(|e| e.best_download_hour(size_kb))
        .unwrap_or(3)
}
