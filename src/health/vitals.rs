use crate::sync::Mutex;
/// Vital signs monitoring for Genesis
///
/// Heart rate, blood oxygen, blood pressure,
/// body temperature, respiratory rate, ECG.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum VitalType {
    HeartRate,
    BloodOxygen,
    BloodPressure,
    Temperature,
    RespiratoryRate,
    Ecg,
}

#[derive(Clone, Copy)]
pub struct VitalReading {
    pub vital_type: VitalType,
    pub value: u32,           // scaled integer (HR in bpm, SpO2 in %, temp in C*10)
    pub value_secondary: u32, // for BP: diastolic
    pub timestamp: u64,
    pub source: SensorSource,
    pub confidence: u8, // 0-100
}

#[derive(Clone, Copy, PartialEq)]
pub enum SensorSource {
    BuiltIn,
    Wearable,
    External,
    Manual,
}

#[derive(Clone, Copy, PartialEq)]
pub enum VitalAlert {
    Normal,
    Low,
    High,
    Critical,
}

struct VitalsEngine {
    readings: Vec<VitalReading>,
    latest: [Option<VitalReading>; 6], // one per VitalType
    alert_thresholds: AlertThresholds,
}

struct AlertThresholds {
    hr_low: u32,
    hr_high: u32,
    spo2_low: u32,
    bp_sys_high: u32,
    bp_dia_high: u32,
    temp_low: u32,
    temp_high: u32,
    rr_low: u32,
    rr_high: u32,
}

static VITALS: Mutex<Option<VitalsEngine>> = Mutex::new(None);

impl VitalsEngine {
    fn new() -> Self {
        VitalsEngine {
            readings: Vec::new(),
            latest: [None; 6],
            alert_thresholds: AlertThresholds {
                hr_low: 40,
                hr_high: 150,
                spo2_low: 90,
                bp_sys_high: 140,
                bp_dia_high: 90,
                temp_low: 355,
                temp_high: 385, // 35.5C - 38.5C
                rr_low: 8,
                rr_high: 25,
            },
        }
    }

    fn record(&mut self, reading: VitalReading) -> VitalAlert {
        let idx = match reading.vital_type {
            VitalType::HeartRate => 0,
            VitalType::BloodOxygen => 1,
            VitalType::BloodPressure => 2,
            VitalType::Temperature => 3,
            VitalType::RespiratoryRate => 4,
            VitalType::Ecg => 5,
        };
        self.latest[idx] = Some(reading);
        if self.readings.len() < 10000 {
            self.readings.push(reading);
        }
        self.check_alert(&reading)
    }

    fn check_alert(&self, reading: &VitalReading) -> VitalAlert {
        let t = &self.alert_thresholds;
        match reading.vital_type {
            VitalType::HeartRate => {
                if reading.value < 30 || reading.value > 200 {
                    VitalAlert::Critical
                } else if reading.value < t.hr_low || reading.value > t.hr_high {
                    VitalAlert::High
                } else {
                    VitalAlert::Normal
                }
            }
            VitalType::BloodOxygen => {
                if reading.value < 85 {
                    VitalAlert::Critical
                } else if reading.value < t.spo2_low {
                    VitalAlert::Low
                } else {
                    VitalAlert::Normal
                }
            }
            VitalType::BloodPressure => {
                if reading.value > 180 || reading.value_secondary > 120 {
                    VitalAlert::Critical
                } else if reading.value > t.bp_sys_high || reading.value_secondary > t.bp_dia_high {
                    VitalAlert::High
                } else {
                    VitalAlert::Normal
                }
            }
            VitalType::Temperature => {
                if reading.value > 400 || reading.value < 340 {
                    VitalAlert::Critical
                } else if reading.value > t.temp_high || reading.value < t.temp_low {
                    VitalAlert::High
                } else {
                    VitalAlert::Normal
                }
            }
            VitalType::RespiratoryRate => {
                if reading.value < 6 || reading.value > 35 {
                    VitalAlert::Critical
                } else if reading.value < t.rr_low || reading.value > t.rr_high {
                    VitalAlert::High
                } else {
                    VitalAlert::Normal
                }
            }
            VitalType::Ecg => VitalAlert::Normal,
        }
    }

    fn average(&self, vital_type: VitalType, last_n: usize) -> Option<u32> {
        let readings: Vec<_> = self
            .readings
            .iter()
            .filter(|r| r.vital_type == vital_type)
            .rev()
            .take(last_n)
            .collect();
        if readings.is_empty() {
            return None;
        }
        let sum: u64 = readings.iter().map(|r| r.value as u64).sum();
        Some((sum / readings.len() as u64) as u32)
    }
}

pub fn init() {
    let mut v = VITALS.lock();
    *v = Some(VitalsEngine::new());
    serial_println!("    Health: vitals monitoring (HR, SpO2, BP, temp, RR, ECG) ready");
}
