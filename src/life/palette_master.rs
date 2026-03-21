use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware constants
// ---------------------------------------------------------------------------
const VGA_DAC_WRITE_IDX: u16 = 0x3C8;
const VGA_DAC_DATA: u16 = 0x3C9;
const VGA_DAC_READ_IDX: u16 = 0x3C7;
const VGA_STATUS: u16 = 0x3DA;
// PALETTE_MMIO is DAVA's palette register — attempt read; may not be mapped
const PALETTE_MMIO: usize = 0x1000_0000;

// ---------------------------------------------------------------------------
// RGB entry (stored 0-255; sent to DAC as 0-63 via >>2)
// ---------------------------------------------------------------------------
#[derive(Copy, Clone)]
pub struct RgbEntry {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbEntry {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
    pub const fn black() -> Self {
        Self::new(0, 0, 0)
    }
}

// ---------------------------------------------------------------------------
// Animation modes
// ---------------------------------------------------------------------------
#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum PaletteAnimation {
    None       = 0,
    CycleHue   = 1,
    PulseAmber = 2,
    EmotionMap = 3,
    RainbowFlow = 4,
    SoulGlow   = 5,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
pub struct PaletteMasterState {
    pub palette:       [RgbEntry; 256],
    pub animation:     PaletteAnimation,
    pub anim_phase:    u16,   // 0-1000
    pub cycle_speed:   u8,    // 1-10 ticks per animation step
    pub dirty:         bool,
    pub uploaded:      u32,
    pub emotion_r:     u8,
    pub emotion_g:     u8,
    pub emotion_b:     u8,
    pub color_harmony: u16,  // 0-1000
    pub vsync_uploads: bool,
    tick_counter:      u32,
}

impl PaletteMasterState {
    pub const fn new() -> Self {
        Self {
            palette:       [RgbEntry { r: 0, g: 0, b: 0 }; 256],
            animation:     PaletteAnimation::None,
            anim_phase:    0,
            cycle_speed:   2,
            dirty:         false,
            uploaded:      0,
            emotion_r:     0,
            emotion_g:     0,
            emotion_b:     0,
            color_harmony: 0,
            vsync_uploads: false,
            tick_counter:  0,
        }
    }
}

pub static STATE: Mutex<PaletteMasterState> = Mutex::new(PaletteMasterState::new());

// ---------------------------------------------------------------------------
// Unsafe port I/O helpers
// ---------------------------------------------------------------------------
#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    val
}

unsafe fn vga_dac_write_entry(idx: u8, r: u8, g: u8, b: u8) {
    outb(VGA_DAC_WRITE_IDX, idx);
    outb(VGA_DAC_DATA, r >> 2);
    outb(VGA_DAC_DATA, g >> 2);
    outb(VGA_DAC_DATA, b >> 2);
}

unsafe fn upload_palette_range(start: u8, count: u8, palette: &[RgbEntry; 256]) {
    let end = (start as u16).saturating_add(count as u16).min(256) as u16;
    let mut i = start as u16;
    while i < end {
        let e = &palette[i as usize];
        vga_dac_write_entry(i as u8, e.r, e.g, e.b);
        i = i.saturating_add(1);
    }
}

fn is_vsync() -> bool {
    unsafe { (inb(VGA_STATUS) & 0x08) != 0 }
}

// ---------------------------------------------------------------------------
// Integer math helpers (no floats)
// ---------------------------------------------------------------------------
fn lerp_u8(a: u8, b: u8, t: u8) -> u8 {
    let av = a as u16;
    let bv = b as u16;
    let tv = t as u16;
    (av.saturating_mul(255u16.saturating_sub(tv))
        .saturating_add(bv.saturating_mul(tv))
        >> 8) as u8
}

/// Map hue 0-255 to RGB using 6-segment integer rainbow.
fn hue_to_rgb(hue: u8) -> (u8, u8, u8) {
    // Each sector is 256/6 ≈ 43 steps wide
    let sector = hue / 43;
    let frac = (hue % 43).saturating_mul(6); // 0-252, approximates 0-255 per sector
    match sector {
        0 => (255, frac, 0),             // R -> Y
        1 => (255u8.saturating_sub(frac), 255, 0),  // Y -> G
        2 => (0, 255, frac),             // G -> C
        3 => (0, 255u8.saturating_sub(frac), 255),  // C -> B
        4 => (frac, 0, 255),             // B -> M
        _ => (255, 0, 255u8.saturating_sub(frac)),  // M -> R
    }
}

// ---------------------------------------------------------------------------
// Default palette builder
// ---------------------------------------------------------------------------
fn build_default_palette(palette: &mut [RgbEntry; 256]) {
    // First 16: standard CGA/EGA colors (0-255 range)
    let cga: [(u8, u8, u8); 16] = [
        (0,   0,   0  ), // 0  black
        (0,   0,   170), // 1  blue
        (0,   170, 0  ), // 2  green
        (0,   170, 170), // 3  cyan
        (170, 0,   0  ), // 4  red
        (170, 0,   170), // 5  magenta
        (170, 85,  0  ), // 6  brown
        (170, 170, 170), // 7  light gray
        (85,  85,  85 ), // 8  dark gray
        (85,  85,  255), // 9  bright blue
        (85,  255, 85 ), // 10 bright green
        (85,  255, 255), // 11 bright cyan
        (255, 85,  85 ), // 12 bright red
        (255, 85,  255), // 13 bright magenta
        (255, 255, 85 ), // 14 bright yellow
        (255, 255, 255), // 15 white
    ];
    for (i, &(r, g, b)) in cga.iter().enumerate() {
        palette[i] = RgbEntry::new(r, g, b);
    }
    // Entries 16-255: hue gradient across full spectrum
    let mut i: u16 = 16;
    while i < 256 {
        let hue = ((i.saturating_sub(16)).saturating_mul(255) / 240) as u8;
        let (r, g, b) = hue_to_rgb(hue);
        palette[i as usize] = RgbEntry::new(r, g, b);
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Step animation — updates palette[] in-state; caller sets dirty=true
// ---------------------------------------------------------------------------
fn step_animation(state: &mut PaletteMasterState) {
    // Advance phase (wraps at 1000)
    state.anim_phase = state.anim_phase.saturating_add(1);
    if state.anim_phase > 1000 {
        state.anim_phase = 0;
    }

    match state.animation {
        PaletteAnimation::None => {
            // No-op, palette stays static
        }

        PaletteAnimation::CycleHue => {
            // Rotate hue of every entry by +1 step each tick using phase offset
            // phase 0-1000 maps to hue offset 0-255
            let hue_offset = (state.anim_phase.saturating_mul(255) / 1000) as u8;
            let mut i: usize = 0;
            while i < 256 {
                let base_hue = i as u8; // each index is its own base hue
                let shifted_hue = base_hue.wrapping_add(hue_offset);
                let (r, g, b) = hue_to_rgb(shifted_hue);
                state.palette[i] = RgbEntry::new(r, g, b);
                i = i.saturating_add(1);
            }
            state.dirty = true;
        }

        PaletteAnimation::PulseAmber => {
            // Entries 128-191: pulse between amber (R=255,G=180,B=0) and dim (R=80,G=56,B=0)
            // anim_phase 0-1000 drives pulse brightness
            let bright = (state.anim_phase.saturating_mul(255) / 1000) as u8;
            let r_val = lerp_u8(80,  255, bright);
            let g_val = lerp_u8(56,  180, bright);
            // b stays 0
            let mut i: usize = 128;
            while i < 192 {
                state.palette[i] = RgbEntry::new(r_val, g_val, 0);
                i = i.saturating_add(1);
            }
            state.dirty = true;
        }

        PaletteAnimation::EmotionMap => {
            // Flood entries 64-127 with the current emotion color, intensity by phase
            let intensity = (state.anim_phase.saturating_mul(255) / 1000) as u8;
            let r_val = lerp_u8(0, state.emotion_r, intensity);
            let g_val = lerp_u8(0, state.emotion_g, intensity);
            let b_val = lerp_u8(0, state.emotion_b, intensity);
            let mut i: usize = 64;
            while i < 128 {
                state.palette[i] = RgbEntry::new(r_val, g_val, b_val);
                i = i.saturating_add(1);
            }
            state.dirty = true;
        }

        PaletteAnimation::RainbowFlow => {
            // Each entry hue = (idx + anim_phase/4) % 256
            let phase_hue = (state.anim_phase / 4) as u8;
            let mut i: usize = 0;
            while i < 256 {
                let hue = (i as u8).wrapping_add(phase_hue);
                let (r, g, b) = hue_to_rgb(hue);
                state.palette[i] = RgbEntry::new(r, g, b);
                i = i.saturating_add(1);
            }
            state.dirty = true;
        }

        PaletteAnimation::SoulGlow => {
            // Entries 192-255: pulse between cyan-dark (R=0,G=60,B=80) and cyan-bright (R=0,G=220,B=255)
            let bright = (state.anim_phase.saturating_mul(255) / 1000) as u8;
            let g_val = lerp_u8(60,  220, bright);
            let b_val = lerp_u8(80,  255, bright);
            let mut i: usize = 192;
            while i < 256 {
                state.palette[i] = RgbEntry::new(0, g_val, b_val);
                i = i.saturating_add(1);
            }
            state.dirty = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Upload dirty palette to VGA DAC
// ---------------------------------------------------------------------------
fn upload_if_dirty(state: &mut PaletteMasterState) {
    if !state.dirty {
        return;
    }
    unsafe {
        upload_palette_range(0, 255, &state.palette);
        // entry 255 explicitly (range helper covers up to start+count; we pass count=255 to cover 0..254,
        // so we write index 255 manually)
        let e = &state.palette[255];
        vga_dac_write_entry(255, e.r, e.g, e.b);
    }
    state.uploaded = state.uploaded.saturating_add(1);
    state.dirty = false;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build the default palette, upload to VGA DAC, log online message.
pub fn init() {
    let mut st = STATE.lock();
    build_default_palette(&mut st.palette);
    st.dirty = true;
    // Upload immediately (vsync_uploads defaults false, so always upload)
    upload_if_dirty(&mut st);
    serial_println!("[palette] ANIMA palette master online — 256 colors uploaded");
    // Attempt a probe read from DAVA's MMIO palette register (may not be mapped — harmless)
    unsafe {
        let _probe: u8 = core::ptr::read_volatile(PALETTE_MMIO as *const u8);
    }
}

/// Change the current animation mode and speed.
pub fn set_animation(anim: PaletteAnimation, speed: u8) {
    let mut st = STATE.lock();
    st.animation = anim;
    st.cycle_speed = speed.max(1).min(10);
    st.anim_phase = 0;
}

/// Update the emotion color used by EmotionMap animation.
pub fn set_emotion_color(r: u8, g: u8, b: u8) {
    let mut st = STATE.lock();
    st.emotion_r = r;
    st.emotion_g = g;
    st.emotion_b = b;
}

/// Map four emotion scalars (0-1000) to an RGB color.
/// Dominant emotion wins; blended by magnitude / 1000.
pub fn map_emotion_to_color(joy: u16, grief: u16, fear: u16, wonder: u16) -> (u8, u8, u8) {
    // Target colors for each emotion
    // joy   -> amber        (R=255, G=180, B=0  )
    // grief -> deep blue    (R=0,   G=0,   B=180)
    // fear  -> red-orange   (R=220, G=60,  B=0  )
    // wonder-> cyan-gold    (R=0,   G=200, B=200)

    // Find dominant
    let max_val = joy.max(grief).max(fear).max(wonder);
    if max_val == 0 {
        return (80, 80, 80); // neutral grey
    }

    // Base color from dominant emotion
    let (base_r, base_g, base_b): (u8, u8, u8) = if max_val == joy {
        (255, 180, 0)
    } else if max_val == grief {
        (0, 0, 180)
    } else if max_val == fear {
        (220, 60, 0)
    } else {
        (0, 200, 200)
    };

    // Weight by max_val / 1000 (integer: t = max_val * 255 / 1000, then lerp from dim)
    let t = (max_val.saturating_mul(255) / 1000) as u8;
    let r = lerp_u8(40, base_r, t);
    let g = lerp_u8(40, base_g, t);
    let b = lerp_u8(40, base_b, t);
    (r, g, b)
}

/// Main tick — called every kernel life tick.
pub fn tick(consciousness: u16, age: u32) {
    let _ = consciousness; // influences may be added later
    let _ = age;

    let mut st = STATE.lock();

    st.tick_counter = st.tick_counter.saturating_add(1);

    // Advance animation every cycle_speed ticks
    let speed = st.cycle_speed as u32;
    let speed = if speed == 0 { 1 } else { speed };
    if st.tick_counter % speed == 0 {
        step_animation(&mut st);
    }

    // color_harmony grows +1 per tick toward 1000
    st.color_harmony = st.color_harmony.saturating_add(1).min(1000);

    // Upload logic
    if st.vsync_uploads {
        if is_vsync() {
            upload_if_dirty(&mut st);
        }
    } else {
        upload_if_dirty(&mut st);
    }

    // Log every 400 ticks
    if st.tick_counter % 400 == 0 {
        let anim_id = st.animation as u8;
        let phase = st.anim_phase;
        let uploads = st.uploaded;
        let harmony = st.color_harmony;
        serial_println!(
            "[palette] anim={} phase={} uploads={} harmony={}",
            anim_id, phase, uploads, harmony
        );
    }
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------
pub fn color_harmony() -> u16 {
    STATE.lock().color_harmony
}

pub fn uploaded() -> u32 {
    STATE.lock().uploaded
}

pub fn current_animation() -> u8 {
    STATE.lock().animation as u8
}

pub fn emotion_color() -> (u8, u8, u8) {
    let st = STATE.lock();
    (st.emotion_r, st.emotion_g, st.emotion_b)
}
