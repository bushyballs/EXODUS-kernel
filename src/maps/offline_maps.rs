use crate::sync::Mutex;
/// Offline map storage and region management for Genesis
///
/// Manages downloadable map regions for fully offline navigation.
/// Each region covers a geographic bounding box and contains
/// pre-rendered tiles at multiple zoom levels.
///
/// Storage is tile-based: each tile is identified by z/x/y and stored
/// as a blob with a hash. Regions track which tiles they contain.
///
/// All coordinates are Q16 fixed-point (i32 * 65536).
/// Sizes in bytes (u64). Tile counts (u32).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 scaling constant
const Q16_ONE: i32 = 65536;

/// Maximum number of offline regions
const MAX_REGIONS: usize = 64;

/// Maximum total offline storage budget in bytes (512 MB)
const MAX_STORAGE_BYTES: u64 = 512 * 1024 * 1024;

/// Default tile data size estimate in bytes (average compressed tile)
const DEFAULT_TILE_SIZE: u64 = 16384;

/// Geographic bounding box (Q16 coordinates).
#[derive(Debug, Clone, Copy)]
pub struct GeoBounds {
    /// Southern edge latitude (Q16)
    pub south_q16: i32,
    /// Northern edge latitude (Q16)
    pub north_q16: i32,
    /// Western edge longitude (Q16)
    pub west_q16: i32,
    /// Eastern edge longitude (Q16)
    pub east_q16: i32,
}

impl GeoBounds {
    /// Create a bounding box from Q16 coordinates.
    pub fn new(south_q16: i32, north_q16: i32, west_q16: i32, east_q16: i32) -> Self {
        GeoBounds {
            south_q16,
            north_q16,
            west_q16,
            east_q16,
        }
    }

    /// Check if a Q16 lat/lon point is within this bounding box.
    pub fn contains(&self, lat_q16: i32, lon_q16: i32) -> bool {
        lat_q16 >= self.south_q16
            && lat_q16 <= self.north_q16
            && lon_q16 >= self.west_q16
            && lon_q16 <= self.east_q16
    }

    /// Calculate the approximate area of the bounding box in square kilometers.
    /// Returns Q16 fixed-point value.
    pub fn area_sq_km_q16(&self) -> i32 {
        let dlat = ((self.north_q16 as i64) - (self.south_q16 as i64)).abs();
        let dlon = ((self.east_q16 as i64) - (self.west_q16 as i64)).abs();

        // degrees (Q16) -> km: 1 degree ~ 111.32 km
        // dlat_km = dlat / 65536 * 111
        // dlon_km = dlon / 65536 * 111
        // area = dlat_km * dlon_km
        let dlat_km = (dlat * 111) / (Q16_ONE as i64);
        let dlon_km = (dlon * 111) / (Q16_ONE as i64);
        let area = (dlat_km * dlon_km) / (Q16_ONE as i64);

        if area > i32::MAX as i64 {
            i32::MAX
        } else {
            area as i32
        }
    }

    /// Check if two bounding boxes overlap.
    pub fn overlaps(&self, other: &GeoBounds) -> bool {
        !(self.east_q16 < other.west_q16
            || self.west_q16 > other.east_q16
            || self.north_q16 < other.south_q16
            || self.south_q16 > other.north_q16)
    }
}

/// An offline map region.
#[derive(Debug, Clone)]
pub struct MapRegion {
    /// Unique region identifier
    pub id: u32,
    /// Hash of the region name (actual string stored elsewhere)
    pub name_hash: u64,
    /// Geographic bounding box
    pub bounds: GeoBounds,
    /// Number of tiles in this region across all zoom levels
    pub tile_count: u32,
    /// Total storage size in bytes
    pub size_bytes: u64,
    /// Whether the region has been fully downloaded
    pub downloaded: bool,
    /// Region data version for update checking
    pub version: u32,
    /// Minimum zoom level stored
    pub min_zoom: u8,
    /// Maximum zoom level stored
    pub max_zoom: u8,
    /// Download progress (0-100)
    pub download_progress: u8,
}

/// An offline tile entry, referencing stored data.
#[derive(Debug, Clone, Copy)]
pub struct OfflineTile {
    /// Tile x coordinate
    pub x: u32,
    /// Tile y coordinate
    pub y: u32,
    /// Zoom level
    pub zoom: u8,
    /// Hash of the tile data (for deduplication and lookup)
    pub data_hash: u64,
    /// Region this tile belongs to
    pub region_id: u32,
    /// Size of the tile data in bytes
    pub size_bytes: u32,
}

/// Offline map storage manager.
pub struct OfflineMapStore {
    /// Registered map regions
    pub regions: Vec<MapRegion>,
    /// Tile index (maps tile key -> OfflineTile)
    pub tiles: Vec<OfflineTile>,
    /// Next available region ID
    next_region_id: u32,
    /// Total storage used in bytes
    pub total_storage_used: u64,
}

/// Global offline map store.
pub static OFFLINE_STORE: Mutex<Option<OfflineMapStore>> = Mutex::new(None);

impl OfflineMapStore {
    /// Create a new empty offline map store.
    pub fn new() -> Self {
        OfflineMapStore {
            regions: Vec::new(),
            tiles: Vec::new(),
            next_region_id: 1,
            total_storage_used: 0,
        }
    }

    /// Download (register) a new offline map region.
    /// Returns the region ID on success, or 0 on failure.
    pub fn download_region(
        &mut self,
        name_hash: u64,
        bounds: GeoBounds,
        min_zoom: u8,
        max_zoom: u8,
    ) -> u32 {
        if self.regions.len() >= MAX_REGIONS {
            serial_println!(
                "[OFFLINE] Cannot add region: max regions reached ({})",
                MAX_REGIONS
            );
            return 0;
        }

        // Estimate tile count and storage size
        let tile_count = estimate_tile_count(&bounds, min_zoom, max_zoom);
        let size_bytes = tile_count as u64 * DEFAULT_TILE_SIZE;

        // Check storage budget
        if self.total_storage_used + size_bytes > MAX_STORAGE_BYTES {
            serial_println!(
                "[OFFLINE] Cannot add region: would exceed storage budget ({}MB + {}MB > {}MB)",
                self.total_storage_used / (1024 * 1024),
                size_bytes / (1024 * 1024),
                MAX_STORAGE_BYTES / (1024 * 1024),
            );
            return 0;
        }

        let id = self.next_region_id;
        self.next_region_id = self.next_region_id.saturating_add(1);

        let region = MapRegion {
            id,
            name_hash,
            bounds,
            tile_count,
            size_bytes,
            downloaded: false,
            version: 1,
            min_zoom,
            max_zoom,
            download_progress: 0,
        };

        serial_println!(
            "[OFFLINE] Downloading region {}: {} tiles, {}KB, zoom {}-{}",
            id,
            tile_count,
            size_bytes / 1024,
            min_zoom,
            max_zoom,
        );

        // Generate tile entries for this region
        self.generate_tiles_for_region(&region);

        // Mark as downloaded (in a real implementation this would be async)
        let mut completed_region = region;
        completed_region.downloaded = true;
        completed_region.download_progress = 100;

        self.total_storage_used += size_bytes;
        self.regions.push(completed_region);

        serial_println!("[OFFLINE] Region {} download complete", id);
        id
    }

    /// Generate tile entries for a region across its zoom levels.
    fn generate_tiles_for_region(&mut self, region: &MapRegion) {
        let mut zoom = region.min_zoom;
        while zoom <= region.max_zoom {
            let world = 1u64 << (zoom as u64);

            // Convert bounds to tile coordinates at this zoom
            let tile_west = lon_to_tile(region.bounds.west_q16, zoom);
            let tile_east = lon_to_tile(region.bounds.east_q16, zoom);
            let tile_north = lat_to_tile(region.bounds.north_q16, zoom);
            let tile_south = lat_to_tile(region.bounds.south_q16, zoom);

            let x_start = tile_west.min(world as u32);
            let x_end = (tile_east + 1).min(world as u32);
            let y_start = tile_north.min(world as u32);
            let y_end = (tile_south + 1).min(world as u32);

            let mut y = y_start;
            while y < y_end {
                let mut x = x_start;
                while x < x_end {
                    let data_hash = compute_tile_data_hash(x, y, zoom);
                    self.tiles.push(OfflineTile {
                        x,
                        y,
                        zoom,
                        data_hash,
                        region_id: region.id,
                        size_bytes: DEFAULT_TILE_SIZE as u32,
                    });
                    x += 1;
                }
                y += 1;
            }

            zoom += 1;
        }
    }

    /// Delete an offline map region and free its storage.
    pub fn delete_region(&mut self, region_id: u32) -> bool {
        let idx = self.regions.iter().position(|r| r.id == region_id);
        match idx {
            Some(i) => {
                let freed = self.regions[i].size_bytes;
                let tile_count = self.regions[i].tile_count;

                // Remove tiles belonging to this region
                self.tiles.retain(|t| t.region_id != region_id);

                // Remove the region
                self.regions.remove(i);

                self.total_storage_used = self.total_storage_used.saturating_sub(freed);

                serial_println!(
                    "[OFFLINE] Deleted region {}: freed {}KB ({} tiles)",
                    region_id,
                    freed / 1024,
                    tile_count,
                );
                true
            }
            None => {
                serial_println!("[OFFLINE] Region {} not found", region_id);
                false
            }
        }
    }

    /// List all downloaded regions.
    pub fn list_regions(&self) -> &[MapRegion] {
        &self.regions
    }

    /// Get a region by ID.
    pub fn get_region(&self, region_id: u32) -> Option<&MapRegion> {
        self.regions.iter().find(|r| r.id == region_id)
    }

    /// Look up an offline tile by z/x/y coordinates.
    /// Searches across all downloaded regions.
    pub fn get_offline_tile(&self, x: u32, y: u32, zoom: u8) -> Option<&OfflineTile> {
        self.tiles
            .iter()
            .find(|t| t.x == x && t.y == y && t.zoom == zoom)
    }

    /// Check if a position (Q16 lat/lon) is covered by any offline region.
    pub fn is_position_offline(&self, lat_q16: i32, lon_q16: i32) -> bool {
        for region in &self.regions {
            if region.downloaded && region.bounds.contains(lat_q16, lon_q16) {
                return true;
            }
        }
        false
    }

    /// Check for available updates to downloaded regions.
    /// Returns a list of region IDs that have newer versions available.
    /// In a real implementation this would query a map data server.
    pub fn check_updates(&self) -> Vec<u32> {
        let mut updates_available = Vec::new();

        for region in &self.regions {
            if !region.downloaded {
                continue;
            }
            // Stub: check if version is older than "latest" (version 2)
            // In production, this would compare against a server manifest
            if region.version < 2 {
                updates_available.push(region.id);
            }
        }

        if !updates_available.is_empty() {
            serial_println!(
                "[OFFLINE] {} region(s) have updates available",
                updates_available.len()
            );
        }

        updates_available
    }

    /// Get total storage used by all offline regions in bytes.
    pub fn get_storage_used(&self) -> u64 {
        self.total_storage_used
    }

    /// Get remaining storage budget in bytes.
    pub fn get_storage_remaining(&self) -> u64 {
        MAX_STORAGE_BYTES.saturating_sub(self.total_storage_used)
    }

    /// Get the total number of offline tiles stored.
    pub fn total_tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Find all regions that overlap with a given bounding box.
    pub fn find_regions_in_bounds(&self, bounds: &GeoBounds) -> Vec<u32> {
        let mut result = Vec::new();
        for region in &self.regions {
            if region.bounds.overlaps(bounds) {
                result.push(region.id);
            }
        }
        result
    }
}

/// Estimate the number of tiles in a bounding box across zoom levels.
fn estimate_tile_count(bounds: &GeoBounds, min_zoom: u8, max_zoom: u8) -> u32 {
    let mut total: u32 = 0;

    let mut zoom = min_zoom;
    while zoom <= max_zoom {
        let tile_west = lon_to_tile(bounds.west_q16, zoom);
        let tile_east = lon_to_tile(bounds.east_q16, zoom);
        let tile_north = lat_to_tile(bounds.north_q16, zoom);
        let tile_south = lat_to_tile(bounds.south_q16, zoom);

        let width = tile_east.saturating_sub(tile_west) + 1;
        let height = tile_south.saturating_sub(tile_north) + 1;

        total = total.saturating_add(width * height);
        zoom += 1;
    }

    total
}

/// Convert Q16 longitude to tile X coordinate at a given zoom level.
fn lon_to_tile(lon_q16: i32, zoom: u8) -> u32 {
    let lon_shifted = (lon_q16 as i64) + (180 * Q16_ONE as i64);
    let world_tiles = 1i64 << (zoom as i64);
    let tile_x = (lon_shifted * world_tiles) / (360 * Q16_ONE as i64);
    if tile_x < 0 {
        0u32
    } else {
        tile_x as u32
    }
}

/// Convert Q16 latitude to tile Y coordinate at a given zoom level.
/// Uses linear approximation (accurate enough for tile selection).
fn lat_to_tile(lat_q16: i32, zoom: u8) -> u32 {
    let lat_shifted = (90 * Q16_ONE as i64) - (lat_q16 as i64);
    let world_tiles = 1i64 << (zoom as i64);
    let tile_y = (lat_shifted * world_tiles) / (180 * Q16_ONE as i64);
    if tile_y < 0 {
        0u32
    } else {
        tile_y as u32
    }
}

/// Compute a deterministic hash for tile data (FNV-1a).
fn compute_tile_data_hash(x: u32, y: u32, zoom: u8) -> u64 {
    let mut hash: u64 = 0xCBF29CE484222325;
    let prime: u64 = 0x00000100000001B3;

    hash ^= zoom as u64;
    hash = hash.wrapping_mul(prime);
    hash ^= x as u64;
    hash = hash.wrapping_mul(prime);
    hash ^= y as u64;
    hash = hash.wrapping_mul(prime);

    hash
}

/// Initialize the offline maps subsystem.
pub fn init() {
    let store = OfflineMapStore::new();
    *OFFLINE_STORE.lock() = Some(store);
    serial_println!(
        "[OFFLINE] Offline map store initialized (max_regions={}, budget={}MB)",
        MAX_REGIONS,
        MAX_STORAGE_BYTES / (1024 * 1024),
    );
}
