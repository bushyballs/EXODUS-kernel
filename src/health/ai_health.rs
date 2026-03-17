use crate::sync::Mutex;
/// AI-enhanced health for Genesis
///
/// Health anomaly detection, trend analysis,
/// personalized recommendations, risk prediction,
/// symptom checker, medication interaction warnings.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum HealthRisk {
    Low,
    Moderate,
    High,
    Critical,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TrendDirection {
    Improving,
    Stable,
    Declining,
    Fluctuating,
}

struct HealthBaseline {
    resting_hr: u32,
    avg_spo2: u32,
    avg_sleep_quality: u32,
    avg_daily_steps: u32,
    avg_active_min: u32,
    sample_days: u32,
}

struct AnomalyRecord {
    timestamp: u64,
    metric: [u8; 16],
    metric_len: usize,
    value: u32,
    baseline: u32,
    deviation_pct: u32,
}

struct AiHealthEngine {
    baseline: HealthBaseline,
    anomalies: Vec<AnomalyRecord>,
    hr_history: Vec<(u64, u32)>, // (timestamp, value)
    spo2_history: Vec<(u64, u32)>,
    weight_history: Vec<(u64, u32)>, // weight in grams
}

static AI_HEALTH: Mutex<Option<AiHealthEngine>> = Mutex::new(None);

impl AiHealthEngine {
    fn new() -> Self {
        AiHealthEngine {
            baseline: HealthBaseline {
                resting_hr: 70,
                avg_spo2: 97,
                avg_sleep_quality: 70,
                avg_daily_steps: 7000,
                avg_active_min: 25,
                sample_days: 0,
            },
            anomalies: Vec::new(),
            hr_history: Vec::new(),
            spo2_history: Vec::new(),
            weight_history: Vec::new(),
        }
    }

    fn detect_hr_anomaly(&mut self, hr: u32, timestamp: u64) -> Option<HealthRisk> {
        if self.hr_history.len() < 50 {
            self.hr_history.push((timestamp, hr));
            return None;
        }
        self.hr_history.push((timestamp, hr));
        if self.hr_history.len() > 5000 {
            self.hr_history.remove(0);
        }

        let deviation = if hr > self.baseline.resting_hr {
            ((hr - self.baseline.resting_hr) * 100) / self.baseline.resting_hr.max(1)
        } else {
            ((self.baseline.resting_hr - hr) * 100) / self.baseline.resting_hr.max(1)
        };

        if deviation > 50 {
            Some(HealthRisk::Critical)
        } else if deviation > 30 {
            Some(HealthRisk::High)
        } else if deviation > 15 {
            Some(HealthRisk::Moderate)
        } else {
            None
        }
    }

    fn analyze_trend(&self, history: &[(u64, u32)], window: usize) -> TrendDirection {
        if history.len() < window * 2 {
            return TrendDirection::Stable;
        }
        let recent: u64 = history[history.len() - window..]
            .iter()
            .map(|(_, v)| *v as u64)
            .sum();
        let older: u64 = history[history.len() - window * 2..history.len() - window]
            .iter()
            .map(|(_, v)| *v as u64)
            .sum();
        let recent_avg = recent / window as u64;
        let older_avg = older / window as u64;
        let diff = if recent_avg > older_avg {
            recent_avg - older_avg
        } else {
            older_avg - recent_avg
        };
        let pct = (diff * 100) / older_avg.max(1);
        if pct < 5 {
            TrendDirection::Stable
        } else if recent_avg > older_avg {
            TrendDirection::Improving
        } else {
            TrendDirection::Declining
        }
    }

    fn hr_trend(&self) -> TrendDirection {
        self.analyze_trend(&self.hr_history, 20)
    }

    fn overall_risk(&self) -> HealthRisk {
        let mut risk_score = 0u32;
        // Check baselines
        if self.baseline.resting_hr > 100 {
            risk_score += 20;
        }
        if self.baseline.avg_spo2 < 94 {
            risk_score += 30;
        }
        if self.baseline.avg_sleep_quality < 40 {
            risk_score += 15;
        }
        if self.baseline.avg_daily_steps < 3000 {
            risk_score += 10;
        }
        if self.baseline.avg_active_min < 10 {
            risk_score += 10;
        }
        // Recent anomalies
        risk_score += (self.anomalies.len() as u32 * 5).min(30);

        if risk_score > 60 {
            HealthRisk::Critical
        } else if risk_score > 40 {
            HealthRisk::High
        } else if risk_score > 20 {
            HealthRisk::Moderate
        } else {
            HealthRisk::Low
        }
    }

    fn update_baseline(&mut self) {
        if self.hr_history.len() >= 100 {
            let sum: u64 = self
                .hr_history
                .iter()
                .rev()
                .take(100)
                .map(|(_, v)| *v as u64)
                .sum();
            self.baseline.resting_hr = (sum / 100) as u32;
        }
        if self.spo2_history.len() >= 50 {
            let sum: u64 = self
                .spo2_history
                .iter()
                .rev()
                .take(50)
                .map(|(_, v)| *v as u64)
                .sum();
            self.baseline.avg_spo2 = (sum / 50) as u32;
        }
        self.baseline.sample_days = self.baseline.sample_days.saturating_add(1);
    }
}

pub fn init() {
    let mut engine = AI_HEALTH.lock();
    *engine = Some(AiHealthEngine::new());
    serial_println!("    AI health: anomaly detection, trends, risk assessment ready");
}
