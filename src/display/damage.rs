use crate::sync::Mutex;
/// Damage tracking for efficient redraws
///
/// Part of the AIOS display layer. Tracks which rectangular regions
/// of the screen have changed and need to be redrawn. Supports merging
/// overlapping regions, tile-based tracking, and full-screen redraw hints.
use alloc::vec::Vec;

/// A rectangular region that needs redrawing
#[derive(Debug, Clone, Copy)]
pub struct DamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    /// Check if two rectangles overlap
    fn overlaps(&self, other: &DamageRect) -> bool {
        let self_right = self.x + self.width;
        let self_bottom = self.y + self.height;
        let other_right = other.x + other.width;
        let other_bottom = other.y + other.height;

        self.x < other_right
            && self_right > other.x
            && self.y < other_bottom
            && self_bottom > other.y
    }

    /// Compute the bounding box union of two rectangles
    fn union(&self, other: &DamageRect) -> DamageRect {
        let min_x = if self.x < other.x { self.x } else { other.x };
        let min_y = if self.y < other.y { self.y } else { other.y };
        let self_right = self.x + self.width;
        let other_right = other.x + other.width;
        let self_bottom = self.y + self.height;
        let other_bottom = other.y + other.height;
        let max_right = if self_right > other_right {
            self_right
        } else {
            other_right
        };
        let max_bottom = if self_bottom > other_bottom {
            self_bottom
        } else {
            other_bottom
        };
        DamageRect {
            x: min_x,
            y: min_y,
            width: max_right - min_x,
            height: max_bottom - min_y,
        }
    }

    /// Compute the area of this rectangle
    fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Check if this rect is adjacent (touching or within margin) to another
    fn adjacent(&self, other: &DamageRect, margin: u32) -> bool {
        let self_right = self.x + self.width + margin;
        let self_bottom = self.y + self.height + margin;
        let other_right = other.x + other.width + margin;
        let other_bottom = other.y + other.height + margin;

        let self_x = if self.x >= margin { self.x - margin } else { 0 };
        let self_y = if self.y >= margin { self.y - margin } else { 0 };
        let other_x = if other.x >= margin {
            other.x - margin
        } else {
            0
        };
        let other_y = if other.y >= margin {
            other.y - margin
        } else {
            0
        };

        self_x < other_right
            && self_right > other_x
            && self_y < other_bottom
            && self_bottom > other_y
    }

    /// Check if this rect fully contains another
    fn contains(&self, other: &DamageRect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && (other.x + other.width) <= (self.x + self.width)
            && (other.y + other.height) <= (self.y + self.height)
    }
}

/// Tile-based tracking for large screens.
/// Divides the screen into tiles and marks dirty tiles.
struct TileGrid {
    tile_size: u32,
    cols: u32,
    rows: u32,
    dirty: Vec<bool>,
}

impl TileGrid {
    fn new(screen_width: u32, screen_height: u32, tile_size: u32) -> Self {
        let cols = (screen_width + tile_size - 1) / tile_size;
        let rows = (screen_height + tile_size - 1) / tile_size;
        let count = (cols * rows) as usize;
        let mut dirty = Vec::with_capacity(count);
        for _ in 0..count {
            dirty.push(false);
        }
        Self {
            tile_size,
            cols,
            rows,
            dirty,
        }
    }

    fn mark_dirty(&mut self, rect: &DamageRect) {
        let start_col = rect.x / self.tile_size;
        let start_row = rect.y / self.tile_size;
        let end_col = ((rect.x + rect.width + self.tile_size - 1) / self.tile_size).min(self.cols);
        let end_row = ((rect.y + rect.height + self.tile_size - 1) / self.tile_size).min(self.rows);

        for row in start_row..end_row {
            for col in start_col..end_col {
                let idx = (row * self.cols + col) as usize;
                if idx < self.dirty.len() {
                    self.dirty[idx] = true;
                }
            }
        }
    }

    fn dirty_tile_rects(&self) -> Vec<DamageRect> {
        let mut rects = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let idx = (row * self.cols + col) as usize;
                if idx < self.dirty.len() && self.dirty[idx] {
                    rects.push(DamageRect {
                        x: col * self.tile_size,
                        y: row * self.tile_size,
                        width: self.tile_size,
                        height: self.tile_size,
                    });
                }
            }
        }
        rects
    }

    fn clear(&mut self) {
        for d in self.dirty.iter_mut() {
            *d = false;
        }
    }

    fn dirty_count(&self) -> usize {
        let mut count = 0;
        for d in &self.dirty {
            if *d {
                count += 1;
            }
        }
        count
    }
}

/// Tracks damaged screen regions to minimize redraws
pub struct DamageTracker {
    pub rects: Vec<DamageRect>,
    pub full_redraw: bool,
    tile_grid: TileGrid,
    merge_margin: u32,
    max_rects: usize,
    screen_width: u32,
    screen_height: u32,
    total_damage_area: u64,
    frame_count: u64,
}

impl DamageTracker {
    pub fn new() -> Self {
        let sw = 1920u32;
        let sh = 1080u32;
        crate::serial_println!("[damage] tracker created, screen {}x{}", sw, sh);
        Self {
            rects: Vec::new(),
            full_redraw: true, // first frame is always full redraw
            tile_grid: TileGrid::new(sw, sh, 64),
            merge_margin: 8,
            max_rects: 64,
            screen_width: sw,
            screen_height: sh,
            total_damage_area: 0,
            frame_count: 0,
        }
    }

    pub fn add_damage(&mut self, rect: DamageRect) {
        // Ignore zero-area rects
        if rect.width == 0 || rect.height == 0 {
            return;
        }

        // Clamp to screen bounds
        let clamped = DamageRect {
            x: rect.x.min(self.screen_width),
            y: rect.y.min(self.screen_height),
            width: rect.width.min(self.screen_width.saturating_sub(rect.x)),
            height: rect.height.min(self.screen_height.saturating_sub(rect.y)),
        };

        if clamped.width == 0 || clamped.height == 0 {
            return;
        }

        // If this rect covers more than 70% of the screen, just do full redraw
        let screen_area = self.screen_width as u64 * self.screen_height as u64;
        if clamped.area() * 100 / screen_area > 70 {
            self.full_redraw = true;
            self.rects.clear();
            return;
        }

        // Mark tiles dirty
        self.tile_grid.mark_dirty(&clamped);

        // Try to merge with existing rects
        let mut merged = false;
        for existing in self.rects.iter_mut() {
            if existing.overlaps(&clamped) || existing.adjacent(&clamped, self.merge_margin) {
                // Merge by computing the bounding union
                let union_rect = existing.union(&clamped);
                // Only merge if the union doesn't waste too much area
                let waste =
                    union_rect.area() as i64 - existing.area() as i64 - clamped.area() as i64;
                if waste < (clamped.area() as i64) {
                    *existing = union_rect;
                    merged = true;
                    break;
                }
            }
            // If an existing rect fully contains the new one, skip
            if existing.contains(&clamped) {
                merged = true;
                break;
            }
        }

        if !merged {
            self.rects.push(clamped);
        }

        // If we have too many rects, consolidate by merging pairs
        if self.rects.len() > self.max_rects {
            self.consolidate();
        }

        self.total_damage_area += clamped.area();
    }

    /// Merge overlapping rectangles to reduce the count
    fn consolidate(&mut self) {
        let mut changed = true;
        while changed && self.rects.len() > self.max_rects / 2 {
            changed = false;
            let mut i = 0;
            while i < self.rects.len() {
                let mut j = i + 1;
                while j < self.rects.len() {
                    if self.rects[i].overlaps(&self.rects[j])
                        || self.rects[i].adjacent(&self.rects[j], self.merge_margin)
                    {
                        let merged = self.rects[i].union(&self.rects[j]);
                        self.rects[i] = merged;
                        self.rects.remove(j);
                        changed = true;
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
        }

        // If still too many, switch to full redraw
        if self.rects.len() > self.max_rects {
            self.full_redraw = true;
            self.rects.clear();
        }
    }

    /// Flush pending damage, returning the list of dirty rectangles.
    /// Resets the tracker for the next frame.
    pub fn flush(&mut self) -> Vec<DamageRect> {
        self.frame_count = self.frame_count.saturating_add(1);
        let result = if self.full_redraw {
            let mut v = Vec::with_capacity(1);
            v.push(DamageRect {
                x: 0,
                y: 0,
                width: self.screen_width,
                height: self.screen_height,
            });
            v
        } else if self.rects.is_empty() {
            Vec::new()
        } else {
            let mut out = Vec::new();
            for r in &self.rects {
                out.push(*r);
            }
            out
        };

        let dirty_tiles = self.tile_grid.dirty_count();
        if !result.is_empty() {
            crate::serial_println!(
                "[damage] flush frame {}: {} rects, {} dirty tiles, full={}",
                self.frame_count,
                result.len(),
                dirty_tiles,
                self.full_redraw
            );
        }

        // Reset state for next frame
        self.rects.clear();
        self.full_redraw = false;
        self.tile_grid.clear();
        self.total_damage_area = 0;

        result
    }

    /// Set screen dimensions
    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
        self.tile_grid = TileGrid::new(width, height, 64);
        self.full_redraw = true;
        crate::serial_println!("[damage] screen size changed to {}x{}", width, height);
    }

    /// Force a full-screen redraw on the next flush
    pub fn invalidate_all(&mut self) {
        self.full_redraw = true;
        self.rects.clear();
    }

    /// Get the total number of pending damage rects
    pub fn pending_count(&self) -> usize {
        self.rects.len()
    }
}

static TRACKER: Mutex<Option<DamageTracker>> = Mutex::new(None);

pub fn init() {
    let tracker = DamageTracker::new();
    let mut t = TRACKER.lock();
    *t = Some(tracker);
    crate::serial_println!("[damage] subsystem initialized");
}

/// Add damage from outside the module
pub fn add(rect: DamageRect) {
    let mut t = TRACKER.lock();
    if let Some(ref mut tracker) = *t {
        tracker.add_damage(rect);
    }
}
