/// Location services for Genesis
///
/// GPS, network-based location, geofencing,
/// location history, and privacy controls.
///
/// Inspired by: Android LocationManager, iOS CLLocationManager. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Location provider
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationProvider {
    Gps,
    Network,
    Fused,
    Passive,
}

/// Location accuracy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accuracy {
    High,     // ~10m (GPS)
    Balanced, // ~100m (WiFi/Cell)
    Low,      // ~1km (Cell only)
    Passive,  // No power cost
}

/// A location fix
#[derive(Clone)]
pub struct Location {
    pub latitude: i64,  // microdegrees (lat * 1_000_000)
    pub longitude: i64, // microdegrees
    pub altitude_m: i32,
    pub accuracy_m: u32,
    pub speed_mps: u32, // millimeters per second
    pub bearing_deg: u16,
    pub timestamp: u64,
    pub provider: LocationProvider,
}

/// A geofence
pub struct Geofence {
    pub id: u32,
    pub center_lat: i64,
    pub center_lon: i64,
    pub radius_m: u32,
    pub app_id: String,
    pub enter_trigger: bool,
    pub exit_trigger: bool,
    pub dwell_trigger: bool,
    pub dwell_ms: u64,
    pub active: bool,
}

/// Location permission level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationPermission {
    Denied,
    WhileInUse,
    Always,
}

/// Location manager
pub struct LocationManager {
    pub enabled: bool,
    pub last_location: Option<Location>,
    pub location_history: Vec<Location>,
    pub max_history: usize,
    pub geofences: Vec<Geofence>,
    pub next_geofence_id: u32,
    pub accuracy: Accuracy,
    pub update_interval_ms: u64,
    pub gps_enabled: bool,
    pub network_enabled: bool,
    pub app_permissions: Vec<(String, LocationPermission)>,
}

impl LocationManager {
    const fn new() -> Self {
        LocationManager {
            enabled: true,
            last_location: None,
            location_history: Vec::new(),
            max_history: 1000,
            geofences: Vec::new(),
            next_geofence_id: 1,
            accuracy: Accuracy::Balanced,
            update_interval_ms: 10000,
            gps_enabled: true,
            network_enabled: true,
            app_permissions: Vec::new(),
        }
    }

    pub fn update_location(&mut self, loc: Location) {
        if self.location_history.len() >= self.max_history {
            self.location_history.remove(0);
        }
        self.location_history.push(loc.clone());
        self.last_location = Some(loc);

        // Check geofences
        if let Some(ref loc) = self.last_location {
            for fence in &self.geofences {
                if !fence.active {
                    continue;
                }
                let _inside = self.is_inside_geofence(loc, fence);
                // In real implementation: fire enter/exit/dwell events
            }
        }
    }

    fn is_inside_geofence(&self, loc: &Location, fence: &Geofence) -> bool {
        // Simple distance check using equirectangular approximation
        let dlat = (loc.latitude - fence.center_lat).abs();
        let dlon = (loc.longitude - fence.center_lon).abs();
        // Rough meters: 1 microdegree lat ~ 0.111m, 1 microdegree lon ~ 0.111m * cos(lat)
        let dist_m = ((dlat * dlat + dlon * dlon) as f64).sqrt() * 0.000111;
        dist_m < fence.radius_m as f64
    }

    pub fn add_geofence(&mut self, app_id: &str, lat: i64, lon: i64, radius: u32) -> u32 {
        let id = self.next_geofence_id;
        self.next_geofence_id = self.next_geofence_id.saturating_add(1);
        self.geofences.push(Geofence {
            id,
            center_lat: lat,
            center_lon: lon,
            radius_m: radius,
            app_id: String::from(app_id),
            enter_trigger: true,
            exit_trigger: true,
            dwell_trigger: false,
            dwell_ms: 0,
            active: true,
        });
        id
    }

    pub fn remove_geofence(&mut self, id: u32) {
        self.geofences.retain(|g| g.id != id);
    }

    pub fn set_permission(&mut self, app_id: &str, perm: LocationPermission) {
        if let Some(entry) = self.app_permissions.iter_mut().find(|(a, _)| a == app_id) {
            entry.1 = perm;
        } else {
            self.app_permissions.push((String::from(app_id), perm));
        }
    }

    pub fn get_permission(&self, app_id: &str) -> LocationPermission {
        self.app_permissions
            .iter()
            .find(|(a, _)| a == app_id)
            .map(|(_, p)| *p)
            .unwrap_or(LocationPermission::Denied)
    }
}

/// Trait to support f64 sqrt in no_std
trait F64Ext {
    fn sqrt(self) -> f64;
}
impl F64Ext for f64 {
    fn sqrt(self) -> f64 {
        if self <= 0.0 {
            return 0.0;
        }
        let mut guess = self / 2.0;
        for _ in 0..15 {
            guess = (guess + self / guess) / 2.0;
        }
        guess
    }
}

static LOCATION: Mutex<LocationManager> = Mutex::new(LocationManager::new());

pub fn init() {
    crate::serial_println!("  [services] Location services initialized");
}
