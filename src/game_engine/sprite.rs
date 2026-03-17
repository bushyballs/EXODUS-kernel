use crate::sync::Mutex;
/// 2D Sprite Engine for Genesis
///
/// Full sprite management with animation, layering, transforms, and
/// painter's algorithm rendering. All coordinates and transforms use
/// i32 Q16 fixed-point (16 fractional bits, multiply by 65536).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536; // 1.0 in Q16
const Q16_HALF: i32 = 32768; // 0.5 in Q16

/// Maximum number of sprites allowed in the system
const MAX_SPRITES: usize = 512;

/// Screen bounds for clipping
const SCREEN_WIDTH: i32 = 1920;
const SCREEN_HEIGHT: i32 = 1080;

/// A 2D sprite with position, transform, animation, and rendering state.
#[derive(Clone, Copy)]
pub struct Sprite {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub texture_hash: u64,
    pub visible: bool,
    pub layer: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub rotation: i32, // Q16 radians
    pub scale: i32,    // Q16 scale factor (65536 = 1.0x)
    pub animation_frame: u32,
    pub animation_speed: u32, // frames between advances
    pub animation_timer: u32, // internal counter
    pub animation_start: u32, // first frame in sequence
    pub animation_end: u32,   // last frame in sequence
    pub animation_looping: bool,
    pub active: bool,
}

/// A sprite sheet defines how a texture is divided into animation frames.
#[derive(Clone, Copy)]
pub struct SpriteSheet {
    pub texture_hash: u64,
    pub frame_width: u32,
    pub frame_height: u32,
    pub columns: u32,
    pub rows: u32,
}

/// Render order entry for painter's algorithm sorting.
#[derive(Clone, Copy)]
struct RenderEntry {
    sprite_index: usize,
    layer: i32,
    y_pos: i32,
}

/// The sprite engine manages all sprites and sprite sheets.
struct SpriteEngine {
    sprites: Vec<Sprite>,
    sheets: Vec<SpriteSheet>,
    next_id: u32,
    render_order: Vec<RenderEntry>,
}

static SPRITE_ENGINE: Mutex<Option<SpriteEngine>> = Mutex::new(None);

impl Sprite {
    fn empty() -> Self {
        Sprite {
            id: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            texture_hash: 0,
            visible: true,
            layer: 0,
            flip_h: false,
            flip_v: false,
            rotation: 0,
            scale: Q16_ONE,
            animation_frame: 0,
            animation_speed: 1,
            animation_timer: 0,
            animation_start: 0,
            animation_end: 0,
            animation_looping: true,
            active: false,
        }
    }
}

impl SpriteEngine {
    fn new() -> Self {
        SpriteEngine {
            sprites: Vec::new(),
            sheets: Vec::new(),
            next_id: 1,
            render_order: Vec::new(),
        }
    }

    /// Create a new sprite with the given texture and dimensions.
    /// Returns the sprite id.
    fn create_sprite(&mut self, x: i32, y: i32, width: u32, height: u32, texture_hash: u64) -> u32 {
        if self.sprites.len() >= MAX_SPRITES {
            serial_println!("    Sprite: max sprites reached ({})", MAX_SPRITES);
            return 0;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let sprite = Sprite {
            id,
            x,
            y,
            width,
            height,
            texture_hash,
            visible: true,
            layer: 0,
            flip_h: false,
            flip_v: false,
            rotation: 0,
            scale: Q16_ONE,
            animation_frame: 0,
            animation_speed: 6,
            animation_timer: 0,
            animation_start: 0,
            animation_end: 0,
            animation_looping: true,
            active: true,
        };

        self.sprites.push(sprite);
        id
    }

    /// Move a sprite by a delta offset.
    fn move_sprite(&mut self, id: u32, dx: i32, dy: i32) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.x += dx;
                sprite.y += dy;
                return true;
            }
        }
        false
    }

    /// Set a sprite's absolute position.
    fn set_position(&mut self, id: u32, x: i32, y: i32) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.x = x;
                sprite.y = y;
                return true;
            }
        }
        false
    }

    /// Configure the animation range and speed for a sprite.
    fn set_animation(&mut self, id: u32, start: u32, end: u32, speed: u32, looping: bool) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.animation_start = start;
                sprite.animation_end = end;
                sprite.animation_speed = if speed == 0 { 1 } else { speed };
                sprite.animation_looping = looping;
                sprite.animation_frame = start;
                sprite.animation_timer = 0;
                return true;
            }
        }
        false
    }

    /// Advance animation frames for all active sprites based on their speed.
    fn update_frames(&mut self) {
        for sprite in self.sprites.iter_mut() {
            if !sprite.active || !sprite.visible {
                continue;
            }
            if sprite.animation_start == sprite.animation_end {
                continue;
            }

            sprite.animation_timer = sprite.animation_timer.saturating_add(1);
            if sprite.animation_timer >= sprite.animation_speed {
                sprite.animation_timer = 0;
                if sprite.animation_frame < sprite.animation_end {
                    sprite.animation_frame = sprite.animation_frame.saturating_add(1);
                } else if sprite.animation_looping {
                    sprite.animation_frame = sprite.animation_start;
                }
            }
        }
    }

    /// Set the rendering layer for a sprite. Higher layers render on top.
    fn set_layer(&mut self, id: u32, layer: i32) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.layer = layer;
                return true;
            }
        }
        false
    }

    /// Set the visibility flag for a sprite.
    fn set_visible(&mut self, id: u32, visible: bool) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.visible = visible;
                return true;
            }
        }
        false
    }

    /// Set scale (Q16) and rotation (Q16 radians) for a sprite.
    fn set_transform(&mut self, id: u32, scale: i32, rotation: i32) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.scale = scale;
                sprite.rotation = rotation;
                return true;
            }
        }
        false
    }

    /// Set horizontal and vertical flip flags.
    fn set_flip(&mut self, id: u32, flip_h: bool, flip_v: bool) -> bool {
        for sprite in self.sprites.iter_mut() {
            if sprite.id == id && sprite.active {
                sprite.flip_h = flip_h;
                sprite.flip_v = flip_v;
                return true;
            }
        }
        false
    }

    /// Sort visible sprites by layer (painter's algorithm) and produce
    /// a render order list. Within the same layer, sort by Y position
    /// for pseudo-depth ordering.
    fn render_sprites(&mut self) -> &[RenderEntry] {
        self.render_order.clear();

        for (i, sprite) in self.sprites.iter().enumerate() {
            if sprite.active && sprite.visible {
                self.render_order.push(RenderEntry {
                    sprite_index: i,
                    layer: sprite.layer,
                    y_pos: sprite.y,
                });
            }
        }

        // Insertion sort by (layer, y_pos) — no_std-friendly, stable sort
        let len = self.render_order.len();
        for i in 1..len {
            let key = self.render_order[i];
            let mut j = i;
            while j > 0 {
                let prev = self.render_order[j - 1];
                if prev.layer > key.layer || (prev.layer == key.layer && prev.y_pos > key.y_pos) {
                    self.render_order[j] = prev;
                    j -= 1;
                } else {
                    break;
                }
            }
            self.render_order[j] = key;
        }

        &self.render_order
    }

    /// Check if a sprite is within the visible screen bounds.
    /// Accounts for scale using Q16 multiplication.
    fn check_bounds(&self, id: u32) -> bool {
        for sprite in self.sprites.iter() {
            if sprite.id == id && sprite.active {
                // Compute scaled dimensions: (width * scale) >> 16
                let scaled_w = ((sprite.width as i32) * sprite.scale) >> 16;
                let scaled_h = ((sprite.height as i32) * sprite.scale) >> 16;

                let right = sprite.x + scaled_w;
                let bottom = sprite.y + scaled_h;

                return sprite.x < SCREEN_WIDTH
                    && right > 0
                    && sprite.y < SCREEN_HEIGHT
                    && bottom > 0;
            }
        }
        false
    }

    /// Destroy a sprite by marking it inactive and removing it.
    fn destroy_sprite(&mut self, id: u32) -> bool {
        if let Some(pos) = self.sprites.iter().position(|s| s.id == id) {
            self.sprites.swap_remove(pos);
            return true;
        }
        false
    }

    /// Register a sprite sheet for animation frame lookups.
    fn register_sheet(
        &mut self,
        texture_hash: u64,
        frame_width: u32,
        frame_height: u32,
        columns: u32,
        rows: u32,
    ) {
        let sheet = SpriteSheet {
            texture_hash,
            frame_width,
            frame_height,
            columns,
            rows,
        };
        self.sheets.push(sheet);
    }

    /// Look up the pixel region for a given frame in a sprite sheet.
    /// Returns (x_offset, y_offset, frame_width, frame_height).
    fn get_frame_rect(&self, texture_hash: u64, frame: u32) -> Option<(u32, u32, u32, u32)> {
        for sheet in self.sheets.iter() {
            if sheet.texture_hash == texture_hash {
                if sheet.columns == 0 {
                    return None;
                }
                let col = frame % sheet.columns;
                let row = frame / sheet.columns;
                if row >= sheet.rows {
                    return None;
                }
                return Some((
                    col * sheet.frame_width,
                    row * sheet.frame_height,
                    sheet.frame_width,
                    sheet.frame_height,
                ));
            }
        }
        None
    }

    /// Get the total count of active sprites.
    fn active_count(&self) -> usize {
        self.sprites.iter().filter(|s| s.active).count()
    }

    /// Get a reference to a sprite by id.
    fn get_sprite(&self, id: u32) -> Option<&Sprite> {
        self.sprites.iter().find(|s| s.id == id && s.active)
    }

    /// Check if two sprites overlap using AABB test.
    /// Accounts for Q16 scale on both sprites.
    fn sprites_overlap(&self, id_a: u32, id_b: u32) -> bool {
        let a = match self.get_sprite(id_a) {
            Some(s) => s,
            None => return false,
        };
        let b = match self.get_sprite(id_b) {
            Some(s) => s,
            None => return false,
        };

        let a_w = ((a.width as i32) * a.scale) >> 16;
        let a_h = ((a.height as i32) * a.scale) >> 16;
        let b_w = ((b.width as i32) * b.scale) >> 16;
        let b_h = ((b.height as i32) * b.scale) >> 16;

        let a_right = a.x + a_w;
        let a_bottom = a.y + a_h;
        let b_right = b.x + b_w;
        let b_bottom = b.y + b_h;

        a.x < b_right && a_right > b.x && a.y < b_bottom && a_bottom > b.y
    }
}

/// Public API: create a sprite with position, size, and texture.
pub fn create_sprite(x: i32, y: i32, width: u32, height: u32, texture_hash: u64) -> u32 {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.create_sprite(x, y, width, height, texture_hash)
    } else {
        0
    }
}

/// Public API: move a sprite by delta.
pub fn move_sprite(id: u32, dx: i32, dy: i32) -> bool {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.move_sprite(id, dx, dy)
    } else {
        false
    }
}

/// Public API: set animation parameters.
pub fn set_animation(id: u32, start: u32, end: u32, speed: u32, looping: bool) -> bool {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.set_animation(id, start, end, speed, looping)
    } else {
        false
    }
}

/// Public API: advance all sprite animations by one tick.
pub fn update_frames() {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.update_frames();
    }
}

/// Public API: set a sprite's rendering layer.
pub fn set_layer(id: u32, layer: i32) -> bool {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.set_layer(id, layer)
    } else {
        false
    }
}

/// Public API: check if a sprite is within visible bounds.
pub fn check_bounds(id: u32) -> bool {
    let engine = SPRITE_ENGINE.lock();
    if let Some(ref e) = *engine {
        e.check_bounds(id)
    } else {
        false
    }
}

/// Public API: destroy and remove a sprite.
pub fn destroy_sprite(id: u32) -> bool {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.destroy_sprite(id)
    } else {
        false
    }
}

/// Public API: sort and return the number of renderable sprites.
pub fn render_sprites() -> usize {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.render_sprites().len()
    } else {
        0
    }
}

/// Public API: register a sprite sheet for frame lookups.
pub fn register_sheet(
    texture_hash: u64,
    frame_width: u32,
    frame_height: u32,
    columns: u32,
    rows: u32,
) {
    let mut engine = SPRITE_ENGINE.lock();
    if let Some(ref mut e) = *engine {
        e.register_sheet(texture_hash, frame_width, frame_height, columns, rows);
    }
}

pub fn init() {
    let mut engine = SPRITE_ENGINE.lock();
    *engine = Some(SpriteEngine::new());
    serial_println!(
        "    Sprite engine: {} max sprites, Q16 transforms, painter's algorithm",
        MAX_SPRITES
    );
}
