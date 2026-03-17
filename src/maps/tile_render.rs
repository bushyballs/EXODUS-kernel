use crate::sync::Mutex;
/// Map tile renderer for Genesis
///
/// Implements a slippy-map tile system with an LRU eviction cache.
/// Tiles follow the standard z/x/y convention where:
///   - zoom level 0 = entire world in 1 tile (256x256)
///   - zoom level 20 = street-level detail
///   - x ranges 0..(2^zoom - 1) left to right
///   - y ranges 0..(2^zoom - 1) top to bottom
///
/// All coordinates are Q16 fixed-point (i32, multiply by 65536).
/// Tile pixel data is referenced by hash (actual bitmap storage is
/// handled by the offline_maps module).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Maximum number of tiles held in the LRU cache before eviction
const MAX_CACHE_SIZE: usize = 256;

/// Tile pixel dimension (standard web map tile)
const TILE_SIZE_PX: i32 = 256;

/// Minimum zoom level
const MIN_ZOOM: u8 = 0;

/// Maximum zoom level
const MAX_ZOOM: u8 = 20;

/// Q16 scaling factor (65536 = 1.0 in fixed-point)
const Q16_ONE: i32 = 65536;

/// A single map tile identified by zoom/x/y coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    /// Tile x coordinate within the zoom level grid
    pub x: u32,
    /// Tile y coordinate within the zoom level grid
    pub y: u32,
    /// Zoom level (0-20)
    pub zoom: u8,
    /// Hash of the tile pixel data (used to look up actual bitmap bytes)
    pub data_hash: u64,
    /// Whether the tile data has been loaded into memory
    pub loaded: bool,
    /// Timestamp (kernel ticks) when this tile was last accessed
    pub timestamp: u64,
}

impl Tile {
    /// Create a new unloaded tile with the given coordinates.
    pub fn new(x: u32, y: u32, zoom: u8) -> Self {
        Tile {
            x,
            y,
            zoom,
            data_hash: 0,
            loaded: false,
            timestamp: 0,
        }
    }

    /// Compute a unique key for this tile based on z/x/y.
    /// Packs zoom (5 bits), x (13 bits), y (13 bits) into a u32.
    /// For zoom > 13 we fall back to a hash-style combination.
    pub fn tile_key(&self) -> u64 {
        let z = self.zoom as u64;
        let x = self.x as u64;
        let y = self.y as u64;
        (z << 40) | (x << 20) | y
    }
}

/// Viewport representing the visible portion of the map.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    /// Center latitude in Q16 fixed-point (degrees * 65536)
    pub center_lat_q16: i32,
    /// Center longitude in Q16 fixed-point (degrees * 65536)
    pub center_lon_q16: i32,
    /// Current zoom level
    pub zoom: u8,
    /// Viewport width in pixels
    pub width_px: u32,
    /// Viewport height in pixels
    pub height_px: u32,
}

impl Viewport {
    /// Create a default viewport centered on 0,0 at zoom 2.
    pub fn default_viewport() -> Self {
        Viewport {
            center_lat_q16: 0,
            center_lon_q16: 0,
            zoom: 2,
            width_px: 800,
            height_px: 600,
        }
    }
}

/// Entry in the LRU tile cache.
#[derive(Debug, Clone)]
struct CacheEntry {
    tile: Tile,
    /// Access counter for LRU ordering
    access_count: u64,
    /// Last access timestamp
    last_access: u64,
}

/// Tile cache with LRU eviction policy.
pub struct TileCache {
    /// Cached tile entries
    entries: Vec<CacheEntry>,
    /// Maximum number of entries before eviction triggers
    max_size: usize,
    /// Monotonic counter for ordering accesses
    access_counter: u64,
    /// Total cache hits since init
    pub hits: u64,
    /// Total cache misses since init
    pub misses: u64,
}

impl TileCache {
    /// Create a new tile cache with the given maximum size.
    pub fn new(max_size: usize) -> Self {
        TileCache {
            entries: Vec::new(),
            max_size,
            access_counter: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Look up a tile in the cache by z/x/y. Returns a copy if found.
    pub fn get(&mut self, x: u32, y: u32, zoom: u8) -> Option<Tile> {
        let target_key = {
            let t = Tile::new(x, y, zoom);
            t.tile_key()
        };

        for entry in self.entries.iter_mut() {
            if entry.tile.tile_key() == target_key {
                self.access_counter = self.access_counter.saturating_add(1);
                entry.access_count = self.access_counter;
                entry.last_access = self.access_counter;
                self.hits = self.hits.saturating_add(1);
                return Some(entry.tile);
            }
        }

        self.misses = self.misses.saturating_add(1);
        None
    }

    /// Insert or update a tile in the cache. Triggers eviction if full.
    pub fn cache_tile(&mut self, tile: Tile) {
        let target_key = tile.tile_key();

        // Check if already cached — update if so
        for entry in self.entries.iter_mut() {
            if entry.tile.tile_key() == target_key {
                self.access_counter = self.access_counter.saturating_add(1);
                entry.tile = tile;
                entry.access_count = self.access_counter;
                entry.last_access = self.access_counter;
                return;
            }
        }

        // Evict if at capacity
        if self.entries.len() >= self.max_size {
            self.evict_old();
        }

        self.access_counter = self.access_counter.saturating_add(1);
        self.entries.push(CacheEntry {
            tile,
            access_count: self.access_counter,
            last_access: self.access_counter,
        });
    }

    /// Evict the least-recently-used tile from the cache.
    /// Removes the entry with the lowest access_count.
    pub fn evict_old(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        let mut min_access = u64::MAX;
        let mut min_idx = 0;

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.access_count < min_access {
                min_access = entry.access_count;
                min_idx = i;
            }
        }

        let evicted = self.entries.remove(min_idx);
        serial_println!(
            "[TILE] Evicted tile z={} x={} y={} (access_count={})",
            evicted.tile.zoom,
            evicted.tile.x,
            evicted.tile.y,
            evicted.access_count
        );
    }

    /// Evict all tiles older than the given access threshold.
    pub fn evict_before(&mut self, threshold: u64) {
        let before = self.entries.len();
        self.entries.retain(|e| e.last_access >= threshold);
        let removed = before - self.entries.len();
        if removed > 0 {
            serial_println!(
                "[TILE] Bulk evicted {} tiles (threshold={})",
                removed,
                threshold
            );
        }
    }

    /// Return the number of tiles currently cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
        self.access_counter = 0;
    }
}

/// The global tile renderer state.
pub struct TileRenderer {
    /// LRU tile cache
    pub cache: TileCache,
    /// Current viewport
    pub viewport: Viewport,
}

/// Global tile renderer instance protected by a spinlock mutex.
pub static TILE_RENDERER: Mutex<Option<TileRenderer>> = Mutex::new(None);

impl TileRenderer {
    /// Create a new tile renderer with default viewport and cache.
    pub fn new() -> Self {
        TileRenderer {
            cache: TileCache::new(MAX_CACHE_SIZE),
            viewport: Viewport::default_viewport(),
        }
    }

    /// Get a tile, loading from offline storage or marking as needed.
    /// Returns the tile if found in cache, or creates a placeholder.
    pub fn get_tile(&mut self, x: u32, y: u32, zoom: u8) -> Tile {
        // Clamp zoom
        let z = if zoom > MAX_ZOOM { MAX_ZOOM } else { zoom };

        // Clamp x/y to valid range for this zoom level
        let max_coord = (1u32 << (z as u32)).saturating_sub(1);
        let cx = if x > max_coord { max_coord } else { x };
        let cy = if y > max_coord { max_coord } else { y };

        // Try cache first
        if let Some(tile) = self.cache.get(cx, cy, z) {
            return tile;
        }

        // Cache miss: create a new tile entry and try offline lookup
        let mut tile = Tile::new(cx, cy, z);

        // Compute a deterministic hash for the tile data
        // In a real implementation this would load from disk/flash
        tile.data_hash = Self::compute_tile_hash(cx, cy, z);
        tile.loaded = false;
        tile.timestamp = self.cache.access_counter;

        // Insert into cache
        self.cache.cache_tile(tile);
        tile
    }

    /// Compute a hash for tile data lookup. Uses FNV-1a style mixing.
    fn compute_tile_hash(x: u32, y: u32, zoom: u8) -> u64 {
        let mut hash: u64 = 0xCBF29CE484222325; // FNV offset basis
        let prime: u64 = 0x00000100000001B3; // FNV prime

        // Mix in zoom
        hash ^= zoom as u64;
        hash = hash.wrapping_mul(prime);

        // Mix in x
        hash ^= x as u64;
        hash = hash.wrapping_mul(prime);

        // Mix in y
        hash ^= y as u64;
        hash = hash.wrapping_mul(prime);

        hash
    }

    /// Determine which tiles are visible in the current viewport and
    /// ensure they are loaded. Returns the list of visible tiles.
    pub fn render_viewport(&mut self) -> Vec<Tile> {
        let vp = self.viewport;
        let z = vp.zoom;

        // Number of tiles across the world at this zoom
        let world_tiles: u32 = 1u32 << (z as u32);

        // Approximate how many tiles fit in the viewport
        let tiles_x = (vp.width_px / TILE_SIZE_PX as u32) + 2;
        let tiles_y = (vp.height_px / TILE_SIZE_PX as u32) + 2;

        // Convert center lat/lon (Q16) to tile coordinates
        // tile_x = (lon + 180) / 360 * 2^zoom
        // tile_y = (1 - ln(tan(lat) + sec(lat)) / pi) / 2 * 2^zoom
        // Using simplified linear projection for Q16:
        let center_tile_x = Self::lon_to_tile_x(vp.center_lon_q16, z);
        let center_tile_y = Self::lat_to_tile_y(vp.center_lat_q16, z);

        let half_x = tiles_x / 2;
        let half_y = tiles_y / 2;

        let start_x = if center_tile_x >= half_x {
            center_tile_x - half_x
        } else {
            0
        };
        let start_y = if center_tile_y >= half_y {
            center_tile_y - half_y
        } else {
            0
        };
        let end_x = (center_tile_x + half_x + 1).min(world_tiles);
        let end_y = (center_tile_y + half_y + 1).min(world_tiles);

        let mut visible_tiles = Vec::new();

        let mut ty = start_y;
        while ty < end_y {
            let mut tx = start_x;
            while tx < end_x {
                let tile = self.get_tile(tx, ty, z);
                visible_tiles.push(tile);
                tx += 1;
            }
            ty += 1;
        }

        visible_tiles
    }

    /// Convert Q16 longitude to tile X coordinate.
    /// lon_q16 is degrees * 65536, range -180*65536 to 180*65536.
    fn lon_to_tile_x(lon_q16: i32, zoom: u8) -> u32 {
        // tile_x = (lon + 180) / 360 * 2^zoom
        let lon_shifted = (lon_q16 as i64) + (180 * Q16_ONE as i64);
        let world_tiles = 1i64 << (zoom as i64);
        let tile_x = (lon_shifted * world_tiles) / (360 * Q16_ONE as i64);
        if tile_x < 0 {
            0u32
        } else {
            tile_x as u32
        }
    }

    /// Convert Q16 latitude to tile Y coordinate.
    /// Uses a linear approximation (Mercator requires ln/tan which
    /// we approximate with a polynomial for Q16).
    fn lat_to_tile_y(lat_q16: i32, zoom: u8) -> u32 {
        // Linear approximation: tile_y = (90 - lat) / 180 * 2^zoom
        // This is inaccurate near poles but works for most latitudes
        let lat_shifted = (90 * Q16_ONE as i64) - (lat_q16 as i64);
        let world_tiles = 1i64 << (zoom as i64);
        let tile_y = (lat_shifted * world_tiles) / (180 * Q16_ONE as i64);
        if tile_y < 0 {
            0u32
        } else {
            tile_y as u32
        }
    }

    /// Zoom in by one level (increase detail), keeping the center.
    pub fn zoom_in(&mut self) {
        if self.viewport.zoom < MAX_ZOOM {
            self.viewport.zoom += 1;
            serial_println!("[TILE] Zoom in -> level {}", self.viewport.zoom);
        }
    }

    /// Zoom out by one level (decrease detail), keeping the center.
    pub fn zoom_out(&mut self) {
        if self.viewport.zoom > MIN_ZOOM {
            self.viewport.zoom -= 1;
            serial_println!("[TILE] Zoom out -> level {}", self.viewport.zoom);
        }
    }

    /// Pan the viewport by the given Q16 delta in lat/lon.
    pub fn pan(&mut self, dlat_q16: i32, dlon_q16: i32) {
        self.viewport.center_lat_q16 = self.viewport.center_lat_q16.saturating_add(dlat_q16);
        self.viewport.center_lon_q16 = self.viewport.center_lon_q16.saturating_add(dlon_q16);

        // Clamp latitude to -90..90 degrees (in Q16)
        let max_lat = 90 * Q16_ONE;
        let min_lat = -90 * Q16_ONE;
        if self.viewport.center_lat_q16 > max_lat {
            self.viewport.center_lat_q16 = max_lat;
        }
        if self.viewport.center_lat_q16 < min_lat {
            self.viewport.center_lat_q16 = min_lat;
        }

        // Wrap longitude to -180..180 degrees (in Q16)
        let max_lon = 180 * Q16_ONE;
        let min_lon = -180 * Q16_ONE;
        if self.viewport.center_lon_q16 > max_lon {
            self.viewport.center_lon_q16 -= 360 * Q16_ONE;
        }
        if self.viewport.center_lon_q16 < min_lon {
            self.viewport.center_lon_q16 += 360 * Q16_ONE;
        }
    }

    /// Set the viewport center to a specific Q16 lat/lon.
    pub fn set_center(&mut self, lat_q16: i32, lon_q16: i32) {
        self.viewport.center_lat_q16 = lat_q16;
        self.viewport.center_lon_q16 = lon_q16;
    }

    /// Set the viewport zoom level (clamped to 0-20).
    pub fn set_zoom(&mut self, zoom: u8) {
        self.viewport.zoom = if zoom > MAX_ZOOM { MAX_ZOOM } else { zoom };
    }

    /// Get cache statistics: (hits, misses, current_size).
    pub fn cache_stats(&self) -> (u64, u64, usize) {
        (self.cache.hits, self.cache.misses, self.cache.len())
    }
}

/// Initialize the tile renderer subsystem.
pub fn init() {
    let renderer = TileRenderer::new();
    *TILE_RENDERER.lock() = Some(renderer);
    serial_println!(
        "[TILE] Tile renderer initialized (cache_max={}, tile={}px, zoom=0-{})",
        MAX_CACHE_SIZE,
        TILE_SIZE_PX,
        MAX_ZOOM
    );
}
