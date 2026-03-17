/// Tilemap Engine for Genesis
///
/// Tile-based map rendering with multiple layers, autotile rules,
/// tile animation, per-tile collision flags, camera scrolling, and
/// parallax background layers. All coordinates use i32 Q16 fixed-point
/// (16 fractional bits, 65536 = 1.0).

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;

/// Maximum number of tile layers per map.
const MAX_LAYERS: usize = 8;

/// Maximum number of parallax background layers.
const MAX_PARALLAX_LAYERS: usize = 4;

/// Maximum number of animated tile definitions.
const MAX_ANIMATED_TILES: usize = 64;

/// Maximum number of autotile rule sets.
const MAX_AUTOTILE_RULES: usize = 32;

/// Default tile dimensions in pixels.
const DEFAULT_TILE_WIDTH: u32 = 16;
const DEFAULT_TILE_HEIGHT: u32 = 16;

/// Q16 multiply: (a * b) >> 16, using i64 to prevent overflow.
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b, using i64 to prevent overflow.
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Tile collision flags stored per tile index.
#[derive(Clone, Copy, PartialEq)]
pub struct TileFlags {
    pub solid: bool,
    pub platform: bool,
    pub hazard: bool,
    pub trigger: bool,
    pub slope_type: u8,    // 0=none, 1=left-up, 2=right-up
    pub custom_bits: u8,
}

impl TileFlags {
    fn empty() -> Self {
        TileFlags {
            solid: false,
            platform: false,
            hazard: false,
            trigger: false,
            slope_type: 0,
            custom_bits: 0,
        }
    }
}

/// An animated tile cycles through a sequence of tile indices at a fixed speed.
#[derive(Clone, Copy)]
pub struct AnimatedTile {
    pub base_tile: u32,
    pub frames: [u32; 8],
    pub frame_count: u32,
    pub speed: u32,           // ticks between frame advances
    pub timer: u32,
    pub current_frame: u32,
    pub active: bool,
}

impl AnimatedTile {
    fn empty() -> Self {
        AnimatedTile {
            base_tile: 0,
            frames: [0; 8],
            frame_count: 0,
            speed: 10,
            timer: 0,
            current_frame: 0,
            active: false,
        }
    }
}

/// Autotile rule: maps a bitmask of neighboring tiles to a specific
/// sub-tile index within the autotile texture.
#[derive(Clone, Copy)]
pub struct AutotileRule {
    pub tile_id: u32,
    pub neighbor_mask: u8,    // 4-bit cardinal neighbors (N=1,E=2,S=4,W=8)
    pub result_tile: u32,
    pub active: bool,
}

/// A single tile layer containing a 2D grid of tile indices.
#[derive(Clone)]
pub struct TileLayer {
    pub tiles: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
    pub opacity: i32,         // Q16 [0..Q16_ONE]
    pub offset_x: i32,        // Q16 pixel offset
    pub offset_y: i32,
    pub depth: i32,           // rendering depth (lower = behind)
}

impl TileLayer {
    fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        TileLayer {
            tiles: vec![0u32; size],
            width,
            height,
            visible: true,
            opacity: Q16_ONE,
            offset_x: Q16_ZERO,
            offset_y: Q16_ZERO,
            depth: 0,
        }
    }

    /// Get the tile index at grid position (gx, gy).
    fn get_tile(&self, gx: u32, gy: u32) -> u32 {
        if gx >= self.width || gy >= self.height {
            return 0;
        }
        self.tiles[(gy * self.width + gx) as usize]
    }

    /// Set the tile index at grid position (gx, gy).
    fn set_tile(&mut self, gx: u32, gy: u32, tile: u32) {
        if gx < self.width && gy < self.height {
            self.tiles[(gy * self.width + gx) as usize] = tile;
        }
    }
}

/// Parallax layer scrolls at a fraction of the camera speed.
#[derive(Clone, Copy)]
pub struct ParallaxLayer {
    pub texture_hash: u64,
    pub scroll_factor_x: i32,   // Q16 (e.g., Q16_HALF = half speed)
    pub scroll_factor_y: i32,
    pub offset_x: i32,          // Q16 current computed offset
    pub offset_y: i32,
    pub repeat_x: bool,
    pub repeat_y: bool,
    pub depth: i32,
    pub active: bool,
}

impl ParallaxLayer {
    fn empty() -> Self {
        ParallaxLayer {
            texture_hash: 0,
            scroll_factor_x: Q16_HALF,
            scroll_factor_y: Q16_HALF,
            offset_x: Q16_ZERO,
            offset_y: Q16_ZERO,
            repeat_x: true,
            repeat_y: false,
            depth: -100,
            active: false,
        }
    }
}

/// Camera state for scrolling the tilemap view.
#[derive(Clone, Copy)]
pub struct Camera {
    pub x: i32,                 // Q16 camera position
    pub y: i32,
    pub target_x: i32,          // Q16 target for smooth follow
    pub target_y: i32,
    pub viewport_w: i32,        // Q16 viewport dimensions
    pub viewport_h: i32,
    pub follow_speed: i32,      // Q16 lerp factor per tick
    pub bound_left: i32,        // Q16 world bounds
    pub bound_top: i32,
    pub bound_right: i32,
    pub bound_bottom: i32,
    pub bounds_enabled: bool,
    pub shake_intensity: i32,   // Q16 shake magnitude
    pub shake_timer: u32,
    pub shake_offset_x: i32,
    pub shake_offset_y: i32,
}

impl Camera {
    fn new(viewport_w: i32, viewport_h: i32) -> Self {
        Camera {
            x: Q16_ZERO,
            y: Q16_ZERO,
            target_x: Q16_ZERO,
            target_y: Q16_ZERO,
            viewport_w,
            viewport_h,
            follow_speed: 6554,   // ~0.1 smooth follow
            bound_left: Q16_ZERO,
            bound_top: Q16_ZERO,
            bound_right: 1920 * Q16_ONE,
            bound_bottom: 1080 * Q16_ONE,
            bounds_enabled: false,
            shake_intensity: Q16_ZERO,
            shake_timer: 0,
            shake_offset_x: Q16_ZERO,
            shake_offset_y: Q16_ZERO,
        }
    }

    /// Smoothly move camera towards its target.
    fn update(&mut self) {
        // Lerp: pos += (target - pos) * speed
        let dx = self.target_x - self.x;
        let dy = self.target_y - self.y;
        self.x += q16_mul(dx, self.follow_speed);
        self.y += q16_mul(dy, self.follow_speed);

        // Apply world bounds
        if self.bounds_enabled {
            let half_w = self.viewport_w / 2;
            let half_h = self.viewport_h / 2;
            if self.x - half_w < self.bound_left {
                self.x = self.bound_left + half_w;
            }
            if self.x + half_w > self.bound_right {
                self.x = self.bound_right - half_w;
            }
            if self.y - half_h < self.bound_top {
                self.y = self.bound_top + half_h;
            }
            if self.y + half_h > self.bound_bottom {
                self.y = self.bound_bottom - half_h;
            }
        }

        // Screen shake decay
        if self.shake_timer > 0 {
            self.shake_timer = self.shake_timer.saturating_sub(1);
            // Simple deterministic shake using timer as seed
            let seed = self.shake_timer as i32;
            self.shake_offset_x = q16_mul(self.shake_intensity,
                ((seed * 7919) % 131) - 65);
            self.shake_offset_y = q16_mul(self.shake_intensity,
                ((seed * 6271) % 131) - 65);
            // Decay intensity
            self.shake_intensity = q16_mul(self.shake_intensity, 62259); // ~0.95
        } else {
            self.shake_offset_x = Q16_ZERO;
            self.shake_offset_y = Q16_ZERO;
        }
    }
}

/// The tilemap engine holds all layers, tile data, and camera state.
struct TilemapEngine {
    layers: Vec<TileLayer>,
    parallax: Vec<ParallaxLayer>,
    tile_flags: Vec<TileFlags>,
    animated_tiles: Vec<AnimatedTile>,
    autotile_rules: Vec<AutotileRule>,
    camera: Camera,
    tile_width: u32,
    tile_height: u32,
    max_tile_id: u32,
}

static TILEMAP: Mutex<Option<TilemapEngine>> = Mutex::new(None);

impl TilemapEngine {
    fn new() -> Self {
        TilemapEngine {
            layers: Vec::new(),
            parallax: Vec::new(),
            tile_flags: Vec::new(),
            animated_tiles: Vec::new(),
            autotile_rules: Vec::new(),
            camera: Camera::new(1920 * Q16_ONE, 1080 * Q16_ONE),
            tile_width: DEFAULT_TILE_WIDTH,
            tile_height: DEFAULT_TILE_HEIGHT,
            max_tile_id: 0,
        }
    }

    /// Add a new tile layer of the given grid dimensions.
    fn add_layer(&mut self, width: u32, height: u32, depth: i32) -> usize {
        if self.layers.len() >= MAX_LAYERS {
            serial_println!("    Tilemap: max layers reached ({})", MAX_LAYERS);
            return 0;
        }
        let mut layer = TileLayer::new(width, height);
        layer.depth = depth;
        self.layers.push(layer);
        self.layers.len() - 1
    }

    /// Set a tile on a specific layer.
    fn set_tile(&mut self, layer: usize, gx: u32, gy: u32, tile: u32) {
        if layer < self.layers.len() {
            self.layers[layer].set_tile(gx, gy, tile);
            if tile > self.max_tile_id {
                self.max_tile_id = tile;
            }
        }
    }

    /// Get a tile from a specific layer.
    fn get_tile(&self, layer: usize, gx: u32, gy: u32) -> u32 {
        if layer < self.layers.len() {
            self.layers[layer].get_tile(gx, gy)
        } else {
            0
        }
    }

    /// Register collision flags for a tile index.
    fn set_tile_flags(&mut self, tile_id: u32, flags: TileFlags) {
        let id = tile_id as usize;
        while self.tile_flags.len() <= id {
            self.tile_flags.push(TileFlags::empty());
        }
        self.tile_flags[id] = flags;
    }

    /// Get collision flags for a tile index.
    fn get_tile_flags(&self, tile_id: u32) -> TileFlags {
        let id = tile_id as usize;
        if id < self.tile_flags.len() {
            self.tile_flags[id]
        } else {
            TileFlags::empty()
        }
    }

    /// Check if a world-space Q16 point collides with a solid tile on any layer.
    fn check_collision_point(&self, world_x: i32, world_y: i32) -> bool {
        let gx = (world_x >> 16) as u32 / self.tile_width;
        let gy = (world_y >> 16) as u32 / self.tile_height;

        for layer in self.layers.iter() {
            let tile = layer.get_tile(gx, gy);
            if tile > 0 {
                let flags = self.get_tile_flags(tile);
                if flags.solid {
                    return true;
                }
            }
        }
        false
    }

    /// Check if an AABB (Q16 center + half-extents) overlaps any solid tile.
    fn check_collision_aabb(&self, cx: i32, cy: i32, hw: i32, hh: i32) -> bool {
        let left = ((cx - hw) >> 16) as u32 / self.tile_width;
        let right = ((cx + hw) >> 16) as u32 / self.tile_width;
        let top = ((cy - hh) >> 16) as u32 / self.tile_height;
        let bottom = ((cy + hh) >> 16) as u32 / self.tile_height;

        for gy in top..=bottom {
            for gx in left..=right {
                for layer in self.layers.iter() {
                    let tile = layer.get_tile(gx, gy);
                    if tile > 0 {
                        let flags = self.get_tile_flags(tile);
                        if flags.solid {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Register an animated tile definition.
    fn add_animated_tile(&mut self, base_tile: u32, frames: &[u32], speed: u32) -> bool {
        if self.animated_tiles.len() >= MAX_ANIMATED_TILES {
            return false;
        }
        let mut anim = AnimatedTile::empty();
        anim.base_tile = base_tile;
        anim.speed = if speed == 0 { 1 } else { speed };
        anim.frame_count = frames.len().min(8) as u32;
        for i in 0..anim.frame_count as usize {
            anim.frames[i] = frames[i];
        }
        anim.active = true;
        self.animated_tiles.push(anim);
        true
    }

    /// Advance all animated tile timers.
    fn update_animated_tiles(&mut self) {
        for anim in self.animated_tiles.iter_mut() {
            if !anim.active || anim.frame_count == 0 {
                continue;
            }
            anim.timer = anim.timer.saturating_add(1);
            if anim.timer >= anim.speed {
                anim.timer = 0;
                anim.current_frame = anim.current_frame.saturating_add(1);
                if anim.current_frame >= anim.frame_count {
                    anim.current_frame = 0;
                }
            }
        }
    }

    /// Get the display tile for a given tile index (resolves animation).
    fn resolve_tile(&self, tile_id: u32) -> u32 {
        for anim in self.animated_tiles.iter() {
            if anim.active && anim.base_tile == tile_id && anim.frame_count > 0 {
                return anim.frames[anim.current_frame as usize];
            }
        }
        tile_id
    }

    /// Add an autotile rule.
    fn add_autotile_rule(&mut self, tile_id: u32, neighbor_mask: u8, result_tile: u32) -> bool {
        if self.autotile_rules.len() >= MAX_AUTOTILE_RULES {
            return false;
        }
        self.autotile_rules.push(AutotileRule {
            tile_id,
            neighbor_mask,
            result_tile,
            active: true,
        });
        true
    }

    /// Apply autotile rules to a layer, computing the correct sub-tile
    /// based on cardinal neighbors.
    fn apply_autotile(&mut self, layer_idx: usize) {
        if layer_idx >= self.layers.len() {
            return;
        }
        let w = self.layers[layer_idx].width;
        let h = self.layers[layer_idx].height;
        // Build a copy of tiles to read from while writing
        let original = self.layers[layer_idx].tiles.clone();

        for gy in 0..h {
            for gx in 0..w {
                let idx = (gy * w + gx) as usize;
                let tile = original[idx];
                if tile == 0 { continue; }

                // Compute 4-bit neighbor mask (N=1, E=2, S=4, W=8)
                let mut mask: u8 = 0;
                if gy > 0 && original[((gy - 1) * w + gx) as usize] == tile { mask |= 1; }
                if gx + 1 < w && original[(gy * w + gx + 1) as usize] == tile { mask |= 2; }
                if gy + 1 < h && original[((gy + 1) * w + gx) as usize] == tile { mask |= 4; }
                if gx > 0 && original[(gy * w + gx - 1) as usize] == tile { mask |= 8; }

                // Find matching autotile rule
                for rule in self.autotile_rules.iter() {
                    if rule.active && rule.tile_id == tile && rule.neighbor_mask == mask {
                        self.layers[layer_idx].tiles[idx] = rule.result_tile;
                        break;
                    }
                }
            }
        }
    }

    /// Add a parallax background layer.
    fn add_parallax_layer(&mut self, texture_hash: u64, scroll_x: i32, scroll_y: i32, depth: i32) -> bool {
        if self.parallax.len() >= MAX_PARALLAX_LAYERS {
            return false;
        }
        let mut layer = ParallaxLayer::empty();
        layer.texture_hash = texture_hash;
        layer.scroll_factor_x = scroll_x;
        layer.scroll_factor_y = scroll_y;
        layer.depth = depth;
        layer.active = true;
        self.parallax.push(layer);
        true
    }

    /// Update parallax layer offsets based on camera position.
    fn update_parallax(&mut self) {
        let cam_x = self.camera.x;
        let cam_y = self.camera.y;
        for layer in self.parallax.iter_mut() {
            if !layer.active { continue; }
            layer.offset_x = q16_mul(cam_x, layer.scroll_factor_x);
            layer.offset_y = q16_mul(cam_y, layer.scroll_factor_y);
        }
    }

    /// Set the camera follow target position (Q16 world coords).
    fn set_camera_target(&mut self, x: i32, y: i32) {
        self.camera.target_x = x;
        self.camera.target_y = y;
    }

    /// Set camera world bounds (Q16).
    fn set_camera_bounds(&mut self, left: i32, top: i32, right: i32, bottom: i32) {
        self.camera.bound_left = left;
        self.camera.bound_top = top;
        self.camera.bound_right = right;
        self.camera.bound_bottom = bottom;
        self.camera.bounds_enabled = true;
    }

    /// Trigger a camera shake effect.
    fn camera_shake(&mut self, intensity: i32, duration: u32) {
        self.camera.shake_intensity = intensity;
        self.camera.shake_timer = duration;
    }

    /// Full per-frame update: camera, parallax, animated tiles.
    fn update(&mut self) {
        self.camera.update();
        self.update_parallax();
        self.update_animated_tiles();
    }

    /// Compute visible tile range for the current camera position on a layer.
    /// Returns (start_gx, start_gy, end_gx, end_gy).
    fn visible_tile_range(&self, layer_idx: usize) -> (u32, u32, u32, u32) {
        if layer_idx >= self.layers.len() {
            return (0, 0, 0, 0);
        }
        let layer = &self.layers[layer_idx];
        let cam_x = self.camera.x + self.camera.shake_offset_x - layer.offset_x;
        let cam_y = self.camera.y + self.camera.shake_offset_y - layer.offset_y;
        let half_w = self.camera.viewport_w / 2;
        let half_h = self.camera.viewport_h / 2;

        let left_px = (cam_x - half_w) >> 16;
        let top_px = (cam_y - half_h) >> 16;
        let right_px = (cam_x + half_w) >> 16;
        let bottom_px = (cam_y + half_h) >> 16;

        let tw = self.tile_width as i32;
        let th = self.tile_height as i32;

        let start_gx = if left_px < 0 { 0 } else { (left_px / tw) as u32 };
        let start_gy = if top_px < 0 { 0 } else { (top_px / th) as u32 };
        let end_gx = ((right_px / tw) as u32 + 1).min(layer.width);
        let end_gy = ((bottom_px / th) as u32 + 1).min(layer.height);

        (start_gx, start_gy, end_gx, end_gy)
    }

    /// Get the layer count.
    fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Get the camera position (Q16).
    fn camera_position(&self) -> (i32, i32) {
        (self.camera.x + self.camera.shake_offset_x,
         self.camera.y + self.camera.shake_offset_y)
    }
}

// --- Public API ---

/// Add a tile layer with the given grid dimensions and depth.
pub fn add_layer(width: u32, height: u32, depth: i32) -> usize {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.add_layer(width, height, depth)
    } else { 0 }
}

/// Set a tile on a layer.
pub fn set_tile(layer: usize, gx: u32, gy: u32, tile: u32) {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.set_tile(layer, gx, gy, tile);
    }
}

/// Get a tile from a layer.
pub fn get_tile(layer: usize, gx: u32, gy: u32) -> u32 {
    let tm = TILEMAP.lock();
    if let Some(ref t) = *tm {
        t.get_tile(layer, gx, gy)
    } else { 0 }
}

/// Set collision flags for a tile index.
pub fn set_tile_flags(tile_id: u32, flags: TileFlags) {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.set_tile_flags(tile_id, flags);
    }
}

/// Check point collision against solid tiles.
pub fn check_collision_point(world_x: i32, world_y: i32) -> bool {
    let tm = TILEMAP.lock();
    if let Some(ref t) = *tm {
        t.check_collision_point(world_x, world_y)
    } else { false }
}

/// Check AABB collision against solid tiles.
pub fn check_collision_aabb(cx: i32, cy: i32, hw: i32, hh: i32) -> bool {
    let tm = TILEMAP.lock();
    if let Some(ref t) = *tm {
        t.check_collision_aabb(cx, cy, hw, hh)
    } else { false }
}

/// Set camera follow target (Q16 world coordinates).
pub fn set_camera_target(x: i32, y: i32) {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.set_camera_target(x, y);
    }
}

/// Set camera world bounds (Q16).
pub fn set_camera_bounds(left: i32, top: i32, right: i32, bottom: i32) {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.set_camera_bounds(left, top, right, bottom);
    }
}

/// Trigger camera shake.
pub fn camera_shake(intensity: i32, duration: u32) {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.camera_shake(intensity, duration);
    }
}

/// Add a parallax background layer.
pub fn add_parallax_layer(texture_hash: u64, scroll_x: i32, scroll_y: i32, depth: i32) -> bool {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.add_parallax_layer(texture_hash, scroll_x, scroll_y, depth)
    } else { false }
}

/// Add an animated tile definition.
pub fn add_animated_tile(base_tile: u32, frames: &[u32], speed: u32) -> bool {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.add_animated_tile(base_tile, frames, speed)
    } else { false }
}

/// Update the tilemap (camera, parallax, animations) once per frame.
pub fn update() {
    let mut tm = TILEMAP.lock();
    if let Some(ref mut t) = *tm {
        t.update();
    }
}

/// Get camera position including shake offset (Q16).
pub fn camera_position() -> (i32, i32) {
    let tm = TILEMAP.lock();
    if let Some(ref t) = *tm {
        t.camera_position()
    } else { (0, 0) }
}

pub fn init() {
    let mut tm = TILEMAP.lock();
    *tm = Some(TilemapEngine::new());
    serial_println!("    Tilemap: {} layers, {} parallax, autotile, collision, camera scroll",
        MAX_LAYERS, MAX_PARALLAX_LAYERS);
}
