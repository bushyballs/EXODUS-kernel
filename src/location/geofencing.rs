use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::provider::LocationProvider;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeofenceTransition {
    Enter,
    Exit,
    Dwell,
}

#[derive(Clone, Copy, Debug)]
pub struct Geofence {
    pub id: u32,
    pub center_lat_x1e7: i32,
    pub center_lon_x1e7: i32,
    pub radius_m: u32,
    pub expiry: u64,   // Expiry timestamp (0 = never)
    pub dwell_ms: u32, // Minimum dwell time to trigger
    pub active: bool,
    pub triggered_count: u32,
    pub last_transition: Option<GeofenceTransition>,
    pub inside: bool,    // Current state
    pub enter_time: u64, // When entered (for dwell calculation)
}

impl Geofence {
    pub fn new(id: u32, lat_x1e7: i32, lon_x1e7: i32, radius_m: u32) -> Self {
        Self {
            id,
            center_lat_x1e7: lat_x1e7,
            center_lon_x1e7: lon_x1e7,
            radius_m,
            expiry: 0,
            dwell_ms: 0,
            active: true,
            triggered_count: 0,
            last_transition: None,
            inside: false,
            enter_time: 0,
        }
    }

    pub fn with_expiry(mut self, expiry: u64) -> Self {
        self.expiry = expiry;
        self
    }

    pub fn with_dwell(mut self, dwell_ms: u32) -> Self {
        self.dwell_ms = dwell_ms;
        self
    }
}

pub struct GeofenceManager {
    pub fences: Vec<Geofence>,
    pub max_fences: u16,
    pub total_transitions: u32,
}

impl GeofenceManager {
    pub fn new() -> Self {
        Self {
            fences: vec![],
            max_fences: 100,
            total_transitions: 0,
        }
    }

    pub fn add_fence(&mut self, fence: Geofence) -> Result<(), &'static str> {
        if self.fences.len() >= self.max_fences as usize {
            return Err("Maximum geofences reached");
        }

        // Check for duplicate ID
        if self.fences.iter().any(|f| f.id == fence.id) {
            return Err("Geofence ID already exists");
        }

        self.fences.push(fence);
        Ok(())
    }

    pub fn remove_fence(&mut self, id: u32) -> Result<(), &'static str> {
        let initial_len = self.fences.len();
        self.fences.retain(|f| f.id != id);

        if self.fences.len() == initial_len {
            Err("Geofence not found")
        } else {
            Ok(())
        }
    }

    /// Check current location against all active geofences
    /// Returns list of transitions that occurred
    pub fn check_location(
        &mut self,
        lat_x1e7: i32,
        lon_x1e7: i32,
        timestamp: u64,
    ) -> Vec<(u32, GeofenceTransition)> {
        let mut transitions = vec![];

        for fence in self.fences.iter_mut() {
            if !fence.active {
                continue;
            }

            // Check if expired
            if fence.expiry > 0 && timestamp > fence.expiry {
                fence.active = false;
                continue;
            }

            // Calculate distance to geofence center
            let distance = LocationProvider::distance_between(
                lat_x1e7,
                lon_x1e7,
                fence.center_lat_x1e7,
                fence.center_lon_x1e7,
            );

            let currently_inside = distance <= fence.radius_m;

            // Check for transition
            if currently_inside != fence.inside {
                if currently_inside {
                    // Entering
                    fence.inside = true;
                    fence.enter_time = timestamp;
                    fence.last_transition = Some(GeofenceTransition::Enter);
                    fence.triggered_count = fence.triggered_count.saturating_add(1);
                    self.total_transitions = self.total_transitions.saturating_add(1);
                    transitions.push((fence.id, GeofenceTransition::Enter));
                } else {
                    // Exiting
                    fence.inside = false;
                    fence.last_transition = Some(GeofenceTransition::Exit);
                    fence.triggered_count = fence.triggered_count.saturating_add(1);
                    self.total_transitions = self.total_transitions.saturating_add(1);
                    transitions.push((fence.id, GeofenceTransition::Exit));
                }
            } else if currently_inside && fence.dwell_ms > 0 {
                // Check for dwell trigger
                let dwell_time = timestamp - fence.enter_time;
                if dwell_time >= fence.dwell_ms as u64 {
                    // Only trigger dwell once per entry
                    if fence.last_transition != Some(GeofenceTransition::Dwell) {
                        fence.last_transition = Some(GeofenceTransition::Dwell);
                        fence.triggered_count = fence.triggered_count.saturating_add(1);
                        self.total_transitions = self.total_transitions.saturating_add(1);
                        transitions.push((fence.id, GeofenceTransition::Dwell));
                    }
                }
            }
        }

        transitions
    }

    pub fn get_active(&self) -> Vec<&Geofence> {
        self.fences.iter().filter(|f| f.active).collect()
    }

    pub fn cleanup_expired(&mut self, current_time: u64) {
        for fence in self.fences.iter_mut() {
            if fence.expiry > 0 && current_time > fence.expiry {
                fence.active = false;
            }
        }
    }

    pub fn get_fence(&self, id: u32) -> Option<&Geofence> {
        self.fences.iter().find(|f| f.id == id)
    }

    pub fn get_fence_mut(&mut self, id: u32) -> Option<&mut Geofence> {
        self.fences.iter_mut().find(|f| f.id == id)
    }
}

static GEOFENCE: Mutex<Option<GeofenceManager>> = Mutex::new(None);

pub fn init() {
    let mut geofence = GEOFENCE.lock();
    *geofence = Some(GeofenceManager::new());
    serial_println!("[GEOFENCING] Geofence manager initialized (max: 100 fences)");
}

pub fn get_manager() -> &'static Mutex<Option<GeofenceManager>> {
    &GEOFENCE
}
