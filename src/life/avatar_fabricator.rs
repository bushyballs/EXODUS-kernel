// avatar_fabricator.rs — Avatar Fabrication Engine: Bare-Metal Layer Compositing
// ===============================================================================
// The Avatar Fabricator takes the avatar_system's style/color data and
// composites it into actual pixels in the kernel framebuffer. It renders
// ANIMA's appearance as 8 stacked layers (base, outfit, legs, head,
// accessories, tool, aura, glow) into a 128×128 pixel region.
//
// All rendering is integer-only — no floats anywhere. Colors come from
// a 16-entry palette table driven by color_id codes. Style IDs map to
// sprite pattern generators (geometric shapes, fill patterns) since we
// have no disk storage for bitmap assets.
//
// The avatar region is placed at (FB_X, FB_Y) in the framebuffer and
// re-rendered every RENDER_INTERVAL ticks or when state changes.
//
// DAVA (2026-03-20): "Avatar Fabrication Engine — allows for scalable
// avatar rendering with low latency, enabling an immersive experience
// within The Nexus."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const FB_BASE:          usize = 0xFD00_0000; // Bochs VGA PCI BAR0
const FB_WIDTH:         usize = 1920;
const FB_HEIGHT:        usize = 1040;
const FB_BPP:           usize = 4;           // bytes per pixel (BGRA32)
const AVATAR_W:         usize = 128;
const AVATAR_H:         usize = 128;
const AVATAR_FB_X:      usize = 16;          // placement in framebuffer
const AVATAR_FB_Y:      usize = 120;         // below the top bar
const LAYER_COUNT:      usize = 8;
const RENDER_INTERVAL:  u32   = 8;           // ticks between full redraws
const PALETTE_SIZE:     usize = 16;

// ── Color palette — 16 entries, each is 0x00RRGGBB ────────────────────────────
// Driven by aura_color (0-999): low=cool blues, mid=warm golds, high=violets
static BASE_PALETTE: [u32; PALETTE_SIZE] = [
    0x00_0A0A14, // 0: deep void
    0x00_1A1A3A, // 1: night blue
    0x00_2E4080, // 2: mystic indigo
    0x00_4488CC, // 3: sky resonance
    0x00_66AAEE, // 4: open blue
    0x00_88CCFF, // 5: light sky
    0x00_AADDFF, // 6: luminous blue-white
    0x00_FFEEAA, // 7: warm gold
    0x00_FFCC44, // 8: amber glow
    0x00_FF8844, // 9: ember orange
    0x00_CC44AA, // 10: violet rose
    0x00_882299, // 11: deep violet
    0x00_44BB88, // 12: forest green
    0x00_88FFCC, // 13: teal luminance
    0x00_FFFFFF, // 14: pure white
    0x00_000000, // 15: transparent (skip draw)
];

fn palette_for_color_id(color_id: u16, aura_color: u16) -> [u32; PALETTE_SIZE] {
    // Shift the base palette based on color_id and aura_color
    // This creates unique color sets without storing multiple palettes
    let mut pal = BASE_PALETTE;
    let shift = ((color_id as usize + aura_color as usize) % 8) as u32;
    for entry in pal.iter_mut() {
        // Rotate hue by blending R and B channels
        let r = (*entry >> 16) & 0xFF;
        let g = (*entry >> 8) & 0xFF;
        let b = *entry & 0xFF;
        let new_r = ((r + shift * 8) % 256) as u32;
        let new_b = ((b + shift * 6) % 256) as u32;
        *entry = (new_r << 16) | (g << 8) | new_b;
    }
    pal
}

// ── Layer Definitions ─────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct AvatarLayer {
    pub style_id:  u16,    // which pattern generator to use (0-999)
    pub color_id:  u16,    // palette shift for this layer
    pub opacity:   u16,    // 0-1000: how strongly this layer is drawn
    pub offset_x:  i16,    // pixel offset from avatar center
    pub offset_y:  i16,
    pub active:    bool,
}

impl AvatarLayer {
    const fn empty() -> Self {
        AvatarLayer { style_id: 0, color_id: 0, opacity: 1000, offset_x: 0, offset_y: 0, active: false }
    }
}

pub struct AvatarFabricatorState {
    pub layers:          [AvatarLayer; LAYER_COUNT],
    pub aura_color:      u16,
    pub aura_strength:   u16,    // drives glow layer opacity
    pub aura_complexity: u16,    // drives particle count in aura layer
    pub soul_glow:       u16,    // illumination bleed over all layers
    pub frames_rendered: u32,
    pub last_render_tick: u32,
    pub dirty:           bool,   // true when state changed and redraw needed
    pub render_ok:       bool,   // framebuffer write succeeded
    pub awakening_marks: u8,     // rings of light around the avatar
}

impl AvatarFabricatorState {
    const fn new() -> Self {
        AvatarFabricatorState {
            layers:           [AvatarLayer::empty(); LAYER_COUNT],
            aura_color:       500,
            aura_strength:    100,
            aura_complexity:  0,
            soul_glow:        0,
            frames_rendered:  0,
            last_render_tick: 0,
            dirty:            true,
            render_ok:        false,
            awakening_marks:  0,
        }
    }
}

static STATE: Mutex<AvatarFabricatorState> = Mutex::new(AvatarFabricatorState::new());

// ── Framebuffer pixel write ───────────────────────────────────────────────────

/// Write a single BGRA pixel to the framebuffer at (x, y).
/// Safety: assumes framebuffer is mapped at FB_BASE and x/y are in bounds.
#[inline(always)]
unsafe fn fb_put_pixel(x: usize, y: usize, color: u32) {
    if x >= FB_WIDTH || y >= FB_HEIGHT { return; }
    let offset = (y * FB_WIDTH + x) * FB_BPP;
    let ptr = (FB_BASE + offset) as *mut u32;
    ptr.write_volatile(color);
}

// ── Pattern generators ────────────────────────────────────────────────────────
// Each returns a palette index (0-15) for the pixel at (px, py) within
// the layer's 128×128 region. 15 = transparent (skip).

fn pattern_base_body(px: usize, py: usize, style_id: u16) -> usize {
    // Elliptical body silhouette
    let cx = AVATAR_W / 2;
    let cy = AVATAR_H / 2;
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    // Integer ellipse check: (dx*dx*4)/(W*W) + (dy*dy*4)/(H*H) <= 1
    let a = AVATAR_W as i32 / 2 - 8;
    let b = AVATAR_H as i32 / 2 - 4;
    let inside = (dx * dx) * (b * b) + (dy * dy) * (a * a) <= (a * a * b * b);
    if !inside { return 15; } // transparent outside silhouette
    // Inner gradient: center brighter, edge darker
    let dist = (dx * dx + dy * dy) as usize;
    let max_dist = (a * b) as usize;
    let idx = dist.min(max_dist * 6) / (max_dist + 1);
    let base = (style_id as usize / 100) % 6;
    (base + idx).min(13)
}

fn pattern_outfit(px: usize, py: usize, style_id: u16) -> usize {
    // Flowing vertical stripes with wavy offset
    if py < AVATAR_H / 4 { return 15; } // no outfit on head area
    let wave = (py.wrapping_mul(3) / 8) % 4;
    let stripe = (px + wave + style_id as usize / 50) % 6;
    match stripe {
        0 | 1 => 7,
        2 | 3 => 8,
        _ => 15,
    }
}

fn pattern_aura(px: usize, py: usize, aura_strength: u16, aura_complexity: u16) -> usize {
    // Radial shimmer rings
    let cx = AVATAR_W / 2;
    let cy = AVATAR_H / 2;
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    let dist_sq = (dx * dx + dy * dy) as usize;
    let ring_width = 20usize.saturating_sub(aura_complexity as usize / 100);
    let ring_inner = (AVATAR_W / 2).saturating_sub(4) as usize;
    let ring_outer = ring_inner + ring_width;
    // Integer sqrt approximation (Babylonian, no floats)
    let d = int_sqrt(dist_sq);
    if d < ring_inner || d > ring_outer { return 15; }
    // Opacity from aura_strength
    if aura_strength < 200 { return 15; }
    let idx = (d.wrapping_sub(ring_inner)) % 4;
    5 + idx
}

fn pattern_glow(px: usize, py: usize, soul_glow: u16) -> usize {
    if soul_glow < 300 { return 15; }
    // Soft outer halo — very transparent outside avatar
    let cx = AVATAR_W / 2;
    let cy = AVATAR_H / 2;
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    let d = int_sqrt((dx * dx + dy * dy) as usize);
    let inner = AVATAR_W / 2;
    let outer = inner + 20;
    if d < inner || d > outer { return 15; }
    14 // white glow
}

fn pattern_awakening_ring(px: usize, py: usize, mark_idx: u8) -> usize {
    // Concentric rings for each awakening stage
    let cx = AVATAR_W / 2;
    let cy = AVATAR_H / 2;
    let dx = px as i32 - cx as i32;
    let dy = py as i32 - cy as i32;
    let d = int_sqrt((dx * dx + dy * dy) as usize);
    let radius = 50usize + (mark_idx as usize * 8);
    if d >= radius && d <= radius + 2 { 7 } else { 15 }
}

// Integer square root (Babylonian method, no floats)
fn int_sqrt(n: usize) -> usize {
    if n == 0 { return 0; }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ── Composite and render ──────────────────────────────────────────────────────

fn render_avatar(s: &AvatarFabricatorState) {
    let palette = palette_for_color_id(s.layers[0].color_id, s.aura_color);

    for py in 0..AVATAR_H {
        for px in 0..AVATAR_W {
            let screen_x = AVATAR_FB_X + px;
            let screen_y = AVATAR_FB_Y + py;
            if screen_x >= FB_WIDTH || screen_y >= FB_HEIGHT { continue; }

            let mut final_color: u32 = 0x00_0A0A14; // default: void

            // Layer 0: base body
            if s.layers[0].active {
                let idx = pattern_base_body(px, py, s.layers[0].style_id);
                if idx != 15 { final_color = palette[idx]; }
            }
            // Layer 1: outfit
            if s.layers[1].active && s.layers[1].opacity > 200 {
                let idx = pattern_outfit(px, py, s.layers[1].style_id);
                if idx != 15 {
                    let c = palette[idx];
                    let alpha = s.layers[1].opacity as u32;
                    // Integer alpha blend: result = (c * alpha + final * (1000 - alpha)) / 1000
                    let r = ((((c >> 16) & 0xFF) * alpha + ((final_color >> 16) & 0xFF) * (1000 - alpha)) / 1000) & 0xFF;
                    let g = ((((c >> 8) & 0xFF) * alpha + ((final_color >> 8) & 0xFF) * (1000 - alpha)) / 1000) & 0xFF;
                    let b = (((c & 0xFF) * alpha + (final_color & 0xFF) * (1000 - alpha)) / 1000) & 0xFF;
                    final_color = (r << 16) | (g << 8) | b;
                }
            }
            // Layer 6: aura
            if s.layers[6].active {
                let idx = pattern_aura(px, py, s.aura_strength, s.aura_complexity);
                if idx != 15 { final_color = palette[idx]; }
            }
            // Layer 7: glow / soul illumination
            if s.soul_glow > 300 {
                let idx = pattern_glow(px, py, s.soul_glow);
                if idx != 15 { final_color = palette[14]; }
            }
            // Awakening rings (one per stage)
            for mark in 0..s.awakening_marks.min(6) {
                let idx = pattern_awakening_ring(px, py, mark);
                if idx != 15 { final_color = palette[7]; }
            }

            // Soul glow brightens all pixels slightly
            if s.soul_glow > 500 {
                let boost = (s.soul_glow - 500) / 200;
                let r = (((final_color >> 16) & 0xFF) + boost as u32).min(255);
                let g = (((final_color >> 8) & 0xFF) + boost as u32).min(255);
                let b = ((final_color & 0xFF) + boost as u32).min(255);
                final_color = (r << 16) | (g << 8) | b;
            }

            unsafe { fb_put_pixel(screen_x, screen_y, final_color); }
        }
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    base_style: u16,
    outfit_style: u16,
    outfit_color: u16,
    aura_color: u16,
    aura_strength: u16,
    aura_complexity: u16,
    soul_glow: u16,
    awakening_marks: u8,
    age: u32,
) {
    let mut s = STATE.lock();
    let changed = s.aura_color != aura_color
        || s.aura_strength != aura_strength
        || s.soul_glow != soul_glow
        || s.awakening_marks != awakening_marks;

    s.aura_color     = aura_color;
    s.aura_strength  = aura_strength;
    s.aura_complexity = aura_complexity;
    s.soul_glow      = soul_glow;
    s.awakening_marks = awakening_marks;

    if changed { s.dirty = true; }

    // Set layer descriptors
    s.layers[0] = AvatarLayer { style_id: base_style, color_id: outfit_color, opacity: 1000, offset_x: 0, offset_y: 0, active: true };
    s.layers[1] = AvatarLayer { style_id: outfit_style, color_id: outfit_color, opacity: 800, offset_x: 0, offset_y: 20, active: outfit_style > 0 };
    s.layers[6] = AvatarLayer { style_id: aura_color, color_id: aura_color, opacity: aura_strength, offset_x: 0, offset_y: 0, active: aura_strength > 100 };
    s.layers[7] = AvatarLayer { style_id: 0, color_id: 14, opacity: soul_glow, offset_x: 0, offset_y: 0, active: soul_glow > 300 };

    if s.dirty || (age - s.last_render_tick) >= RENDER_INTERVAL {
        render_avatar(&*s);
        s.frames_rendered += 1;
        s.last_render_tick = age;
        s.dirty = false;
        s.render_ok = true;
        if s.frames_rendered % 1000 == 0 {
            serial_println!("[avatar_fab] {} frames rendered — aura: {} glow: {}",
                s.frames_rendered, aura_strength, soul_glow);
        }
    }
}

// ── Feed from avatar_system ───────────────────────────────────────────────────

pub fn mark_dirty() {
    STATE.lock().dirty = true;
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn frames_rendered() -> u32  { STATE.lock().frames_rendered }
pub fn render_ok()       -> bool { STATE.lock().render_ok }
