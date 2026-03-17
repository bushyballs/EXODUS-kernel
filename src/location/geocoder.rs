/// Geocoding for Genesis
///
/// Lat/lng to address, address to lat/lng, reverse geocode,
/// place search, nearby search with offline database.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;

// ─── Data Types ──────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AddressComponent {
    StreetNumber,
    Street,
    City,
    State,
    PostalCode,
    Country,
    County,
    Neighborhood,
    Poi,
}

#[derive(Clone, Debug)]
pub struct GeoAddress {
    pub components: Vec<(AddressComponent, u32)>,  // component type -> string table index
    pub lat_x1e7: i32,
    pub lon_x1e7: i32,
    pub confidence_q16: i32,  // Q16 confidence 0..Q16_ONE
    pub timestamp: u64,
}

impl GeoAddress {
    pub fn new(lat_x1e7: i32, lon_x1e7: i32) -> Self {
        Self {
            components: Vec::new(),
            lat_x1e7,
            lon_x1e7,
            confidence_q16: 0,
            timestamp: 0,
        }
    }

    pub fn with_component(mut self, kind: AddressComponent, str_idx: u32) -> Self {
        self.components.push((kind, str_idx));
        self
    }

    pub fn with_confidence(mut self, confidence_q16: i32) -> Self {
        self.confidence_q16 = confidence_q16;
        self
    }

    pub fn with_timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeocodeStatus {
    Ok,
    NotFound,
    Ambiguous,
    OfflineOnly,
    RateLimited,
}

#[derive(Clone, Debug)]
pub struct GeocodeResult {
    pub status: GeocodeStatus,
    pub addresses: Vec<GeoAddress>,
    pub elapsed_ms: u32,
}

impl GeocodeResult {
    pub fn empty(status: GeocodeStatus) -> Self {
        Self {
            status,
            addresses: Vec::new(),
            elapsed_ms: 0,
        }
    }
}

// ─── String Table (offline address strings) ─────────────

#[derive(Clone, Debug)]
struct StringEntry {
    data: String,
}

#[derive(Clone, Debug)]
struct StringTable {
    entries: Vec<StringEntry>,
}

impl StringTable {
    fn new() -> Self {
        Self { entries: Vec::new() }
    }

    fn intern(&mut self, s: &str) -> u32 {
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.data.as_str() == s {
                return i as u32;
            }
        }
        let idx = self.entries.len() as u32;
        self.entries.push(StringEntry {
            data: String::from(s),
        });
        idx
    }

    fn get(&self, idx: u32) -> Option<&str> {
        self.entries.get(idx as usize).map(|e| e.data.as_str())
    }
}

// ─── Offline Grid Cell ──────────────────────────────────

#[derive(Clone, Debug)]
struct GridCell {
    lat_base_x1e7: i32,
    lon_base_x1e7: i32,
    addresses: Vec<u32>,  // indices into address database
}

// ─── Nearby Result ──────────────────────────────────────

#[derive(Clone, Debug)]
pub struct NearbyResult {
    pub address_idx: u32,
    pub distance_m: u32,
    pub bearing_deg: u16,
}

// ─── Geocoder Engine ────────────────────────────────────

pub struct Geocoder {
    addresses: Vec<GeoAddress>,
    string_table: StringTable,
    grid: Vec<GridCell>,
    grid_resolution_x1e7: i32,  // grid cell size in x1e7 units
    max_addresses: usize,
    cache_hits: u32,
    cache_misses: u32,
    total_forward: u64,
    total_reverse: u64,
    total_nearby: u64,
    last_query_ms: u32,
    reverse_cache: Vec<(i32, i32, u32)>,  // (lat, lon, address_idx) LRU
    max_cache: usize,
}

impl Geocoder {
    pub fn new() -> Self {
        Self {
            addresses: Vec::new(),
            string_table: StringTable::new(),
            grid: Vec::new(),
            grid_resolution_x1e7: 10000,  // ~1km grid cells
            max_addresses: 10000,
            cache_hits: 0,
            cache_misses: 0,
            total_forward: 0,
            total_reverse: 0,
            total_nearby: 0,
            last_query_ms: 0,
            reverse_cache: Vec::new(),
            max_cache: 64,
        }
    }

    /// Register an address in the offline database
    pub fn register_address(&mut self, address: GeoAddress) -> Result<u32, &'static str> {
        if self.addresses.len() >= self.max_addresses {
            return Err("Address database full");
        }
        let idx = self.addresses.len() as u32;
        let lat = address.lat_x1e7;
        let lon = address.lon_x1e7;
        self.addresses.push(address);
        self.index_to_grid(idx, lat, lon);
        Ok(idx)
    }

    /// Intern a string into the string table, returning its index
    pub fn intern_string(&mut self, s: &str) -> u32 {
        self.string_table.intern(s)
    }

    /// Look up a string by index
    pub fn get_string(&self, idx: u32) -> Option<&str> {
        self.string_table.get(idx)
    }

    fn index_to_grid(&mut self, addr_idx: u32, lat_x1e7: i32, lon_x1e7: i32) {
        let cell_lat = lat_x1e7 / self.grid_resolution_x1e7;
        let cell_lon = lon_x1e7 / self.grid_resolution_x1e7;

        for cell in self.grid.iter_mut() {
            if cell.lat_base_x1e7 == cell_lat && cell.lon_base_x1e7 == cell_lon {
                cell.addresses.push(addr_idx);
                return;
            }
        }
        self.grid.push(GridCell {
            lat_base_x1e7: cell_lat,
            lon_base_x1e7: cell_lon,
            addresses: vec![addr_idx],
        });
    }

    /// Reverse geocode: lat/lng to nearest address
    pub fn reverse_geocode(&mut self, lat_x1e7: i32, lon_x1e7: i32) -> GeocodeResult {
        self.total_reverse = self.total_reverse.saturating_add(1);

        // Check cache first
        for &(clat, clon, cidx) in &self.reverse_cache {
            let dlat = (clat - lat_x1e7).abs();
            let dlon = (clon - lon_x1e7).abs();
            if dlat < 100 && dlon < 100 {
                self.cache_hits = self.cache_hits.saturating_add(1);
                if let Some(addr) = self.addresses.get(cidx as usize) {
                    let mut result = GeocodeResult::empty(GeocodeStatus::Ok);
                    result.addresses.push(addr.clone());
                    return result;
                }
            }
        }
        self.cache_misses = self.cache_misses.saturating_add(1);

        // Search grid neighborhood
        let cell_lat = lat_x1e7 / self.grid_resolution_x1e7;
        let cell_lon = lon_x1e7 / self.grid_resolution_x1e7;

        let mut best_idx: Option<u32> = None;
        let mut best_dist: u64 = u64::MAX;

        for dl in -1i32..=1 {
            for dc in -1i32..=1 {
                let tl = cell_lat + dl;
                let tc = cell_lon + dc;
                for cell in &self.grid {
                    if cell.lat_base_x1e7 == tl && cell.lon_base_x1e7 == tc {
                        for &aidx in &cell.addresses {
                            if let Some(addr) = self.addresses.get(aidx as usize) {
                                let d = Self::distance_sq(
                                    lat_x1e7, lon_x1e7,
                                    addr.lat_x1e7, addr.lon_x1e7,
                                );
                                if d < best_dist {
                                    best_dist = d;
                                    best_idx = Some(aidx);
                                }
                            }
                        }
                    }
                }
            }
        }

        match best_idx {
            Some(idx) => {
                // Update cache
                if self.reverse_cache.len() >= self.max_cache {
                    self.reverse_cache.remove(0);
                }
                self.reverse_cache.push((lat_x1e7, lon_x1e7, idx));

                let mut result = GeocodeResult::empty(GeocodeStatus::Ok);
                if let Some(addr) = self.addresses.get(idx as usize) {
                    result.addresses.push(addr.clone());
                }
                result
            }
            None => GeocodeResult::empty(GeocodeStatus::NotFound),
        }
    }

    /// Forward geocode: search by string table index for a component match
    pub fn forward_geocode(&mut self, component: AddressComponent, str_idx: u32) -> GeocodeResult {
        self.total_forward = self.total_forward.saturating_add(1);

        let mut matches: Vec<GeoAddress> = Vec::new();
        for addr in &self.addresses {
            for &(kind, sidx) in &addr.components {
                if kind == component && sidx == str_idx {
                    matches.push(addr.clone());
                    break;
                }
            }
        }

        let status = if matches.is_empty() {
            GeocodeStatus::NotFound
        } else if matches.len() > 1 {
            GeocodeStatus::Ambiguous
        } else {
            GeocodeStatus::Ok
        };

        GeocodeResult {
            status,
            addresses: matches,
            elapsed_ms: 0,
        }
    }

    /// Search for nearby addresses within radius_m meters
    pub fn nearby_search(
        &mut self,
        lat_x1e7: i32,
        lon_x1e7: i32,
        radius_m: u32,
        max_results: usize,
    ) -> Vec<NearbyResult> {
        self.total_nearby = self.total_nearby.saturating_add(1);

        // Convert radius to approximate x1e7 delta
        // 1 degree lat ~= 111km, so 1m ~= 90 in x1e7
        let radius_x1e7 = (radius_m as i64 * 90) as i32;

        let mut results: Vec<NearbyResult> = Vec::new();

        for (i, addr) in self.addresses.iter().enumerate() {
            let dlat = (addr.lat_x1e7 - lat_x1e7).abs();
            let dlon = (addr.lon_x1e7 - lon_x1e7).abs();

            // Quick bounding box check
            if dlat > radius_x1e7 || dlon > radius_x1e7 {
                continue;
            }

            let dist_sq = Self::distance_sq(lat_x1e7, lon_x1e7, addr.lat_x1e7, addr.lon_x1e7);
            let radius_sq = (radius_x1e7 as u64) * (radius_x1e7 as u64);

            if dist_sq <= radius_sq {
                let dist_m = Self::approx_distance_m(lat_x1e7, lon_x1e7, addr.lat_x1e7, addr.lon_x1e7);
                let bearing = Self::compute_bearing(lat_x1e7, lon_x1e7, addr.lat_x1e7, addr.lon_x1e7);
                results.push(NearbyResult {
                    address_idx: i as u32,
                    distance_m: dist_m,
                    bearing_deg: bearing,
                });
            }
        }

        // Sort by distance (simple insertion sort for small results)
        for i in 1..results.len() {
            let mut j = i;
            while j > 0 && results[j].distance_m < results[j - 1].distance_m {
                results.swap(j, j - 1);
                j -= 1;
            }
        }

        results.truncate(max_results);
        results
    }

    /// Get address by index
    pub fn get_address(&self, idx: u32) -> Option<&GeoAddress> {
        self.addresses.get(idx as usize)
    }

    /// Get total registered addresses
    pub fn address_count(&self) -> usize {
        self.addresses.len()
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, u64, u64, u32, u32) {
        (self.total_forward, self.total_reverse, self.total_nearby, self.cache_hits, self.cache_misses)
    }

    // ─── Internal Helpers ─────────────────────────────────

    fn distance_sq(lat1: i32, lon1: i32, lat2: i32, lon2: i32) -> u64 {
        let dlat = (lat2 - lat1) as i64;
        let dlon = (lon2 - lon1) as i64;
        ((dlat * dlat) + (dlon * dlon)) as u64
    }

    fn approx_distance_m(lat1: i32, lon1: i32, lat2: i32, lon2: i32) -> u32 {
        let dlat = ((lat2 - lat1) as i64).abs();
        let dlon = ((lon2 - lon1) as i64).abs();
        // 1 degree = ~111km, x1e7 scale
        // 1 unit x1e7 = 111000m / 10_000_000 = 0.0111m
        // distance_m = sqrt(dlat^2 + dlon^2) * 0.0111
        let sq = dlat * dlat + dlon * dlon;
        let root = Self::isqrt64(sq as u64);
        // Multiply by 111000 and divide by 10_000_000
        ((root * 111000) / 10_000_000) as u32
    }

    fn compute_bearing(lat1: i32, lon1: i32, lat2: i32, lon2: i32) -> u16 {
        let dlat = (lat2 - lat1) as i64;
        let dlon = (lon2 - lon1) as i64;

        if dlat == 0 && dlon == 0 {
            return 0;
        }

        // Simple 8-sector bearing from atan2 approximation
        let abs_dlat = dlat.abs();
        let abs_dlon = dlon.abs();

        let base_angle: u16 = if abs_dlon > abs_dlat {
            // More east-west
            let ratio_q16 = (((abs_dlat) << 16) / abs_dlon) as i32;
            // atan approximation: angle ~= ratio * 45 / Q16_ONE
            let angle = (((ratio_q16 as i64) * 45) / (Q16_ONE as i64)) as u16;
            if angle > 45 { 45 } else { angle }
        } else if abs_dlat > 0 {
            let ratio_q16 = (((abs_dlon) << 16) / abs_dlat) as i32;
            let angle = (((ratio_q16 as i64) * 45) / (Q16_ONE as i64)) as u16;
            let a = if angle > 45 { 45 } else { angle };
            90 - a
        } else {
            0
        };

        // Adjust for quadrant
        if dlat >= 0 && dlon >= 0 {
            base_angle           // NE quadrant: 0-90
        } else if dlat < 0 && dlon >= 0 {
            180 - base_angle     // SE quadrant: 90-180
        } else if dlat < 0 && dlon < 0 {
            180 + base_angle     // SW quadrant: 180-270
        } else {
            360 - base_angle     // NW quadrant: 270-360
        }
    }

    fn isqrt64(n: u64) -> u64 {
        if n == 0 { return 0; }
        let mut x = n;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + n / x) / 2;
        }
        x
    }
}

// ─── Global State ────────────────────────────────────────

static GEOCODER: Mutex<Option<Geocoder>> = Mutex::new(None);

pub fn init() {
    let mut geo = GEOCODER.lock();
    *geo = Some(Geocoder::new());
    serial_println!("[GEOCODER] Geocoding engine initialized (max: 10000 addresses)");
}

pub fn get_geocoder() -> &'static Mutex<Option<Geocoder>> {
    &GEOCODER
}
