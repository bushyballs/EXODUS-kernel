use crate::sync::Mutex;
/// AI-enhanced location for Genesis
///
/// Next-location prediction, place categorization,
/// adaptive polling, commute learning.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PlaceCategory {
    Home,
    Work,
    Gym,
    Restaurant,
    Store,
    Transit,
    School,
    Medical,
    Entertainment,
    Unknown,
}

#[derive(Clone, Copy)]
struct FrequentPlace {
    lat_x1e7: i32,
    lon_x1e7: i32,
    category: PlaceCategory,
    visit_count: u32,
    avg_dwell_min: u16,
    last_visit: u64,
}

#[derive(Clone, Copy)]
struct CommuteLeg {
    from_place: u32,
    to_place: u32,
    typical_depart_hour: u8,
    typical_duration_min: u16,
    day_mask: u8, // bitmask Mon=1..Sun=64
}

struct AiLocationEngine {
    frequent_places: Vec<FrequentPlace>,
    commutes: Vec<CommuteLeg>,
    battery_aware: bool,
    prediction_accuracy: u8,
    total_predictions: u32,
}

static AI_LOC: Mutex<Option<AiLocationEngine>> = Mutex::new(None);

impl AiLocationEngine {
    fn new() -> Self {
        AiLocationEngine {
            frequent_places: Vec::new(),
            commutes: Vec::new(),
            battery_aware: true,
            prediction_accuracy: 0,
            total_predictions: 0,
        }
    }

    fn predict_next_location(&mut self, hour: u8, day_of_week: u8) -> Option<(i32, i32)> {
        self.total_predictions = self.total_predictions.saturating_add(1);
        // Check commute patterns for this hour/day
        for leg in &self.commutes {
            if leg.typical_depart_hour == hour && (leg.day_mask & (1 << day_of_week)) != 0 {
                if let Some(dest) = self.frequent_places.get(leg.to_place as usize) {
                    return Some((dest.lat_x1e7, dest.lon_x1e7));
                }
            }
        }
        // Fall back to most-visited place at this hour
        self.frequent_places
            .iter()
            .max_by_key(|p| p.visit_count)
            .map(|p| (p.lat_x1e7, p.lon_x1e7))
    }

    fn categorize_place(&self, dwell_min: u16, hour: u8) -> PlaceCategory {
        // Heuristic: long dwell at night = home, long dwell during work hours = work
        if dwell_min > 300 && (hour >= 22 || hour < 6) {
            PlaceCategory::Home
        } else if dwell_min > 240 && hour >= 8 && hour <= 18 {
            PlaceCategory::Work
        } else if dwell_min > 30 && dwell_min < 120 && (hour >= 6 && hour < 8) {
            PlaceCategory::Gym
        } else if dwell_min > 20
            && dwell_min < 90
            && (hour >= 11 && hour <= 14 || hour >= 17 && hour <= 21)
        {
            PlaceCategory::Restaurant
        } else if dwell_min < 30 {
            PlaceCategory::Store
        } else {
            PlaceCategory::Unknown
        }
    }

    fn optimize_polling_rate(&self, at_known_place: bool, battery_pct: u8) -> u32 {
        // Return polling interval in milliseconds
        if at_known_place && self.battery_aware {
            if battery_pct < 20 {
                300_000
            }
            // 5 min
            else {
                120_000
            } // 2 min
        } else if battery_pct < 10 {
            600_000 // 10 min
        } else {
            30_000 // 30 sec
        }
    }

    fn learn_commute(
        &mut self,
        from_idx: u32,
        to_idx: u32,
        depart_hour: u8,
        duration_min: u16,
        day: u8,
    ) {
        // Check if we already have this commute
        for leg in &mut self.commutes {
            if leg.from_place == from_idx && leg.to_place == to_idx {
                leg.day_mask |= 1 << day;
                // Rolling average
                leg.typical_duration_min = (leg.typical_duration_min + duration_min) / 2;
                return;
            }
        }
        self.commutes.push(CommuteLeg {
            from_place: from_idx,
            to_place: to_idx,
            typical_depart_hour: depart_hour,
            typical_duration_min: duration_min,
            day_mask: 1 << day,
        });
    }

    fn record_visit(&mut self, lat: i32, lon: i32, dwell_min: u16, hour: u8, timestamp: u64) {
        let threshold = 1000; // ~100m in x1e7
        for place in &mut self.frequent_places {
            let dlat = (place.lat_x1e7 - lat).abs();
            let dlon = (place.lon_x1e7 - lon).abs();
            if dlat < threshold && dlon < threshold {
                place.visit_count = place.visit_count.saturating_add(1);
                place.avg_dwell_min = (place.avg_dwell_min + dwell_min) / 2;
                place.last_visit = timestamp;
                return;
            }
        }
        let category = self.categorize_place(dwell_min, hour);
        self.frequent_places.push(FrequentPlace {
            lat_x1e7: lat,
            lon_x1e7: lon,
            category,
            visit_count: 1,
            avg_dwell_min: dwell_min,
            last_visit: timestamp,
        });
    }
}

pub fn init() {
    let mut engine = AI_LOC.lock();
    *engine = Some(AiLocationEngine::new());
    serial_println!("    AI location: prediction, categorization, adaptive polling ready");
}
