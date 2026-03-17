use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LocationSource {
    Gps,
    Network,
    Wifi,
    Cell,
    Fused,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LocationAccuracy {
    High,     // <10m
    Balanced, // <50m
    Low,      // <500m
    Passive,  // Accept whatever is available
}

#[derive(Clone, Copy, Debug)]
pub struct Location {
    pub latitude_x1e7: i32,  // Latitude * 10^7 (e.g., 374214120 = 37.4214120°)
    pub longitude_x1e7: i32, // Longitude * 10^7
    pub altitude_cm: i32,    // Altitude in centimeters
    pub accuracy_m: u16,     // Horizontal accuracy in meters
    pub speed_cms: u16,      // Speed in cm/s
    pub bearing_deg: u16,    // Bearing in degrees (0-359)
    pub source: LocationSource,
    pub timestamp: u64, // Timestamp in milliseconds
}

pub struct LocationProvider {
    pub last_location: Option<Location>,
    pub update_interval_ms: u32,
    pub min_distance_m: u16,
    pub accuracy: LocationAccuracy,
    pub listeners_count: u8,
    pub total_fixes: u64,
    pub stale_threshold_ms: u32,
}

impl LocationProvider {
    pub fn new() -> Self {
        Self {
            last_location: None,
            update_interval_ms: 5000, // 5 seconds default
            min_distance_m: 10,       // 10 meters minimum displacement
            accuracy: LocationAccuracy::Balanced,
            listeners_count: 0,
            total_fixes: 0,
            stale_threshold_ms: 60000, // 1 minute
        }
    }

    pub fn request_update(&mut self, location: Location) {
        self.last_location = Some(location);
        self.total_fixes = self.total_fixes.saturating_add(1);
    }

    pub fn get_last_known(&self) -> Option<Location> {
        self.last_location
    }

    pub fn set_accuracy(&mut self, accuracy: LocationAccuracy) {
        self.accuracy = accuracy;
        // Adjust update interval based on accuracy
        self.update_interval_ms = match accuracy {
            LocationAccuracy::High => 1000,     // 1 second
            LocationAccuracy::Balanced => 5000, // 5 seconds
            LocationAccuracy::Low => 15000,     // 15 seconds
            LocationAccuracy::Passive => 60000, // 1 minute
        };
    }

    /// Calculate distance between two points using Haversine approximation
    /// Returns distance in meters (using integer math)
    pub fn distance_between(lat1_x1e7: i32, lon1_x1e7: i32, lat2_x1e7: i32, lon2_x1e7: i32) -> u32 {
        // Earth radius in meters
        const EARTH_RADIUS_M: u32 = 6371000;

        // Calculate differences (already in x1e7 format)
        let dlat = (lat2_x1e7 - lat1_x1e7).abs() as u32;
        let dlon = (lon2_x1e7 - lon1_x1e7).abs() as u32;

        // For small distances, use equirectangular approximation
        // This is much simpler than full Haversine for integer math
        // x = Δλ * cos(φ)
        // y = Δφ
        // distance = R * √(x² + y²)

        // Average latitude for cos approximation (in x1e7)
        let avg_lat = ((lat1_x1e7 as i64 + lat2_x1e7 as i64) / 2) as i32;

        // Simple cos approximation for lat (cos(lat) ≈ 1 - lat²/2 for small angles)
        // lat in radians ≈ lat_x1e7 / 10000000 * π/180
        // For simplicity, use lookup table approximation
        let cos_lat = Self::cos_approx(avg_lat);

        // Apply cos to longitude difference
        let x = (dlon as u64 * cos_lat as u64) / 1000;
        let y = dlat as u64;

        // Pythagorean distance (x² + y²)
        let dist_squared = (x * x + y * y) / 10000000; // Scale down from x1e7

        // Integer square root approximation
        let dist_x1e7 = Self::isqrt(dist_squared);

        // Convert to meters: (dist_x1e7 / 10000000) * EARTH_RADIUS_M * π/180
        // Simplified: dist_x1e7 * EARTH_RADIUS_M * 314159 / (10000000 * 18000000)
        ((dist_x1e7 * EARTH_RADIUS_M as u64 * 314159) / 180000000000000) as u32
    }

    /// Simple cosine approximation for latitude (returns value * 1000)
    fn cos_approx(lat_x1e7: i32) -> u32 {
        // Simplified cosine lookup for latitude -90 to +90
        // cos(0°) = 1.0 -> 1000
        // cos(45°) ≈ 0.707 -> 707
        // cos(90°) = 0.0 -> 0
        let lat_deg = lat_x1e7.abs() / 10000000;
        if lat_deg > 90 {
            return 0;
        }
        // Linear approximation: cos(lat) ≈ 1 - lat/90
        1000 - ((lat_deg * 1000) / 90) as u32
    }

    /// Integer square root using binary search
    fn isqrt(n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let mut x = n;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + n / x) / 2;
        }
        x
    }

    pub fn is_stale(&self, current_time: u64) -> bool {
        match self.last_location {
            Some(loc) => current_time - loc.timestamp > self.stale_threshold_ms as u64,
            None => true,
        }
    }
}

static LOCATION: Mutex<Option<LocationProvider>> = Mutex::new(None);

pub fn init() {
    let mut location = LOCATION.lock();
    *location = Some(LocationProvider::new());
    serial_println!("[PROVIDER] Location provider initialized");
}

pub fn get_provider() -> &'static Mutex<Option<LocationProvider>> {
    &LOCATION
}
