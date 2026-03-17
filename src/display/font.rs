/// Bitmap font renderer for Genesis
///
/// Renders text using an 8x16 bitmap font (the classic VGA/PC font).
/// Each character is stored as 16 bytes (16 rows of 8 pixels).
/// Built-in — no external font files needed.
///
/// Additional capabilities:
///   - Subpixel antialiasing for LCD panels (RGB stripe layout)
///   - Glyph cache: up to 128 rendered glyphs stored as 8×16 pixel masks
///   - `measure_string_sized` — text metrics without drawing
///   - Basic pair kerning table for common letter pairs
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
pub const FONT_WIDTH: u32 = 8;
pub const FONT_HEIGHT: u32 = 16;

// ---------------------------------------------------------------------------
// Glyph cache
//
// A flat array of (char, [u64; 16]) slots.  Each entry stores the character
// and its 8×16 monochrome mask packed as 64-bit rows (high bit = leftmost
// pixel).  Slot 0..128 are used; the array is statically initialised to the
// "empty" sentinel char '\0'.
//
// The cache is write-once-per-char: once a glyph has been rendered and
// stored it is never evicted (the font is constant so there is nothing to
// invalidate).
// ---------------------------------------------------------------------------

const GLYPH_CACHE_SIZE: usize = 128;

/// A cached glyph — 8 pixels wide × 16 rows stored as one byte per row.
/// The `char` field is '\0' when the slot is empty.
struct GlyphEntry {
    ch: char,
    rows: [u8; 16],
}

impl GlyphEntry {
    const fn empty() -> Self {
        GlyphEntry {
            ch: '\0',
            rows: [0u8; 16],
        }
    }
}

/// Static glyph cache.  Access via `cache_lookup` / `cache_store`.
static mut GLYPH_CACHE: [GlyphEntry; GLYPH_CACHE_SIZE] = {
    // Can't use array-init syntax with non-Copy types in const context,
    // so we use a manual expansion.  128 entries is manageable.
    const EMPTY: GlyphEntry = GlyphEntry::empty();
    [
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 0-7
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 8-15
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 16-23
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 24-31
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 32-39
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 40-47
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 48-55
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 56-63
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 64-71
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 72-79
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 80-87
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 88-95
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 96-103
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 104-111
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 112-119
        EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, EMPTY, // 120-127
    ]
};

/// Look up a character in the glyph cache.
/// Returns the cached row bytes if present, otherwise `None`.
fn cache_lookup(c: char) -> Option<[u8; 16]> {
    let idx = c as usize;
    if idx >= GLYPH_CACHE_SIZE {
        return None;
    }
    // SAFETY: single-threaded kernel; no concurrent access.
    let entry = unsafe { &GLYPH_CACHE[idx] };
    if entry.ch == c {
        Some(entry.rows)
    } else {
        None
    }
}

/// Store a rendered glyph in the cache.
fn cache_store(c: char, rows: [u8; 16]) {
    let idx = c as usize;
    if idx >= GLYPH_CACHE_SIZE {
        return;
    }
    // SAFETY: single-threaded kernel; no concurrent access.
    unsafe {
        GLYPH_CACHE[idx].ch = c;
        GLYPH_CACHE[idx].rows = rows;
    }
}

/// Retrieve a glyph, using the cache when possible.
fn cached_glyph(c: char) -> [u8; 16] {
    if let Some(rows) = cache_lookup(c) {
        return rows;
    }
    let rows = get_builtin_glyph(c);
    cache_store(c, rows);
    rows
}

// ---------------------------------------------------------------------------
// Kerning table
//
// Each entry is (left_char, right_char, kern_adjust_pixels).
// Negative values tighten the pair; positive values add space.
// Values are in 1/8-pixel units to allow sub-pixel positioning in
// scaled rendering paths, but the integer draw path rounds to nearest pixel.
// ---------------------------------------------------------------------------

const KERN_TABLE: &[(char, char, i8)] = &[
    // Classic optical pairs — tighten
    ('A', 'V', -2),
    ('A', 'W', -2),
    ('A', 'T', -2),
    ('A', 'Y', -2),
    ('V', 'A', -2),
    ('W', 'A', -2),
    ('T', 'a', -2),
    ('T', 'e', -2),
    ('T', 'o', -2),
    ('T', 'r', -1),
    ('T', 'y', -2),
    ('Y', 'a', -2),
    ('Y', 'e', -2),
    ('Y', 'o', -2),
    ('F', 'a', -1),
    ('F', 'e', -1),
    ('F', 'o', -1),
    ('P', 'a', -1),
    ('P', 'o', -1),
    ('r', '.', -1),
    ('r', ',', -1),
    ('f', '.', -1),
    ('f', ',', -1),
    // Add space (rare but correct for certain pairs)
    ('f', 'f', 0), // ff ligature — no change in bitmap font
];

/// Look up kerning adjustment in 1/8-pixel units for a character pair.
/// Returns 0 if the pair is not in the table.
pub fn kern_pair(left: char, right: char) -> i8 {
    for &(l, r, adjust) in KERN_TABLE {
        if l == left && r == right {
            return adjust;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Text metrics
// ---------------------------------------------------------------------------

/// Return (width_px, height_px) for a string at the given font_size.
///
/// `font_size` is in points; 16 pt corresponds to the native FONT_HEIGHT.
/// The calculation scales proportionally and applies pair kerning.
/// No pixels are touched.
pub fn measure_string_sized(s: &str, font_size: u8) -> (u32, u32) {
    if s.is_empty() || font_size == 0 {
        return (0, 0);
    }
    // Scale factor as fixed-point 8.8 (256 = 1.0)
    let scale = (font_size as u32 * 256) / 16; // 256 units per "1× scale"

    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();

    let mut total_w_fp: i32 = 0; // accumulate in 8.8 fixed-point (256 = 1px)
    for i in 0..n {
        let base_w = (FONT_WIDTH * scale) as i32; // 8.8 fp
        let kern = if i + 1 < n {
            kern_pair(chars[i], chars[i + 1]) as i32 * (scale as i32 / 8)
        } else {
            0
        };
        total_w_fp += base_w + kern;
    }

    let width = (total_w_fp as u32 + 128) / 256; // round to nearest pixel
    let height = (FONT_HEIGHT * scale + 128) / 256;
    (width, height)
}

// ---------------------------------------------------------------------------
// Subpixel antialiasing (LCD RGB stripe)
//
// For each pixel in the glyph bitmap we emit three sub-pixel samples that
// weight the RGB channels independently to give smoother left/right edges.
//
// Sub-pixel weights for a pixel with bit-pattern context (left, center, right):
//   R channel ← weight from center-1 (shifted left)
//   G channel ← weight from center
//   B channel ← weight from center+1 (shifted right)
//
// `fb_color` is the background colour; `fg_color` is the text colour (ARGB).
// The function blends each channel separately.
// ---------------------------------------------------------------------------

/// Render a single character into `fb` using subpixel RGB weighting.
///
/// - `fb`     : framebuffer pixel slice (ARGB u32, packed as 0xAARRGGBB)
/// - `fb_w`   : framebuffer stride in pixels
/// - `x`, `y` : top-left of the glyph (pixel coordinates)
/// - `fg`     : foreground colour (0x00RRGGBB, alpha ignored — always opaque)
pub fn render_char_subpixel(c: char, x: u32, y: u32, fb: &mut [u32], fb_w: u32, fg: u32) {
    if x.saturating_add(FONT_WIDTH) > fb_w {
        return;
    }

    let glyph = cached_glyph(c);
    let fb_h = (fb.len() as u32) / fb_w.max(1);
    if y.saturating_add(FONT_HEIGHT) > fb_h {
        return;
    }

    let fg_r = ((fg >> 16) & 0xFF) as u8;
    let fg_g = ((fg >> 8) & 0xFF) as u8;
    let fg_b = (fg & 0xFF) as u8;

    for row in 0..FONT_HEIGHT {
        let byte = glyph[row as usize];
        // Extend the row by one bit on each side for neighbour sampling.
        // bit(col) is set when the glyph has a filled pixel at that column.
        for col in 0..FONT_WIDTH {
            let px = x + col;
            let py = y + row;
            let idx = (py * fb_w + px) as usize;
            if idx >= fb.len() {
                continue;
            }

            let bg = fb[idx];
            let bg_r = ((bg >> 16) & 0xFF) as u8;
            let bg_g = ((bg >> 8) & 0xFF) as u8;
            let bg_b = (bg & 0xFF) as u8;

            // Subpixel weights: each colour channel uses a slightly offset
            // sample so that the effective horizontal resolution is 3×.
            // Column masks for R (one to the left), G (center), B (one to the right).
            let bit_center = 0x80u8 >> col;
            let bit_left = if col > 0 { 0x80u8 >> (col - 1) } else { 0 };
            let bit_right = if col + 1 < FONT_WIDTH {
                0x80u8 >> (col + 1)
            } else {
                0
            };

            let cov_r: u32 = if byte & bit_left != 0 { 255 } else { 0 };
            let cov_g: u32 = if byte & bit_center != 0 { 255 } else { 0 };
            let cov_b: u32 = if byte & bit_right != 0 { 255 } else { 0 };

            // Blend each channel: out = bg + cov*(fg - bg) / 255
            // Use signed arithmetic to handle fg < bg correctly (dark text on
            // light background), then clamp to [0, 255].
            let blend_ch = |bg: u8, fg: u8, cov: u32| -> u8 {
                let diff = fg as i32 - bg as i32; // signed delta
                let contribution = (cov as i32 * diff) / 255;
                (bg as i32 + contribution).clamp(0, 255) as u8
            };
            let out_r = blend_ch(bg_r, fg_r, cov_r);
            let out_g = blend_ch(bg_g, fg_g, cov_g);
            let out_b = blend_ch(bg_b, fg_b, cov_b);

            fb[idx] = 0xFF000000 | ((out_r as u32) << 16) | ((out_g as u32) << 8) | out_b as u32;
        }
    }
}

/// Render a full string with subpixel antialiasing.
pub fn render_string_subpixel(s: &str, mut x: u32, y: u32, fb: &mut [u32], fb_w: u32, fg: u32) {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    for i in 0..n {
        render_char_subpixel(chars[i], x, y, fb, fb_w, fg);
        let kern_adj = if i + 1 < n {
            // kern_pair returns 1/8-pixel units; round to nearest whole pixel.
            let k = kern_pair(chars[i], chars[i + 1]) as i32;
            (k + 4) / 8 // round toward zero
        } else {
            0
        };
        let advance = FONT_WIDTH as i32 + kern_adj;
        x = if advance >= 0 {
            x.saturating_add(advance as u32)
        } else {
            x.saturating_sub((-advance) as u32)
        };
    }
}

/// Draw a single character using a simple built-in font
/// Characters are 8x16 pixels, ASCII 32-126
///
/// `buf` is a pixel buffer of `buf_width` x `buf_height` pixels (u32 ARGB)
pub fn draw_char(buf: &mut [u32], buf_width: u32, x: u32, y: u32, c: char, color: u32) {
    let idx = c as u32;
    if idx < 32 || idx > 126 {
        return;
    }

    // Use the cache to avoid recomputing the glyph bitmap every call.
    let glyph = cached_glyph(c);

    for row in 0..FONT_HEIGHT {
        let byte = glyph[row as usize];
        for col in 0..FONT_WIDTH {
            if byte & (0x80 >> col) != 0 {
                let px = x + col;
                let py = y + row;
                let buf_height = buf.len() as u32 / buf_width.max(1);
                if px < buf_width && py < buf_height {
                    buf[(py * buf_width + px) as usize] = color;
                }
            }
        }
    }
}

/// Draw a string
pub fn draw_string(buf: &mut [u32], buf_width: u32, x: u32, y: u32, text: &str, color: u32) {
    let mut cx = x;
    for c in text.chars() {
        draw_char(buf, buf_width, cx, y, c, color);
        cx += FONT_WIDTH;
    }
}

/// Measure text width in pixels
pub fn measure_string(text: &str) -> u32 {
    text.len() as u32 * FONT_WIDTH
}

/// Get a glyph for a character (public API for fbconsole and other renderers).
/// Uses the glyph cache; populates it on first access.
pub fn get_glyph(c: char) -> [u8; 16] {
    cached_glyph(c)
}

/// Built-in 8x16 glyphs for ASCII 32-126
/// Only key characters are hand-coded here. The rest get a default glyph.
fn get_builtin_glyph(c: char) -> [u8; 16] {
    match c {
        ' ' => [0x00; 16],
        'A' => [
            0x00, 0x00, 0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'B' => [
            0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x66, 0x7C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'C' => [
            0x00, 0x00, 0x3C, 0x66, 0x60, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'D' => [
            0x00, 0x00, 0x78, 0x6C, 0x66, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'E' => [
            0x00, 0x00, 0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x7E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'F' => [
            0x00, 0x00, 0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'G' => [
            0x00, 0x00, 0x3C, 0x66, 0x60, 0x60, 0x6E, 0x66, 0x66, 0x3E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'H' => [
            0x00, 0x00, 0x66, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'I' => [
            0x00, 0x00, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'K' => [
            0x00, 0x00, 0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'L' => [
            0x00, 0x00, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'M' => [
            0x00, 0x00, 0xC6, 0xEE, 0xFE, 0xD6, 0xC6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'N' => [
            0x00, 0x00, 0x66, 0x76, 0x7E, 0x7E, 0x6E, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'O' => [
            0x00, 0x00, 0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'P' => [
            0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'R' => [
            0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'S' => [
            0x00, 0x00, 0x3C, 0x66, 0x60, 0x3C, 0x06, 0x06, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'T' => [
            0x00, 0x00, 0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'U' => [
            0x00, 0x00, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'W' => [
            0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xD6, 0xFE, 0xEE, 0xC6, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'X' => [
            0x00, 0x00, 0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'Y' => [
            0x00, 0x00, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'a' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x06, 0x3E, 0x66, 0x66, 0x3E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'b' => [
            0x00, 0x00, 0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x7C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'c' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x66, 0x60, 0x60, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'd' => [
            0x00, 0x00, 0x06, 0x06, 0x3E, 0x66, 0x66, 0x66, 0x66, 0x3E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'e' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x66, 0x7E, 0x60, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'g' => [
            0x00, 0x00, 0x00, 0x00, 0x3E, 0x66, 0x66, 0x3E, 0x06, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'h' => [
            0x00, 0x00, 0x60, 0x60, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'i' => [
            0x00, 0x00, 0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'l' => [
            0x00, 0x00, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'm' => [
            0x00, 0x00, 0x00, 0x00, 0xEC, 0xFE, 0xD6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'n' => [
            0x00, 0x00, 0x00, 0x00, 0x7C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'o' => [
            0x00, 0x00, 0x00, 0x00, 0x3C, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'p' => [
            0x00, 0x00, 0x00, 0x00, 0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'r' => [
            0x00, 0x00, 0x00, 0x00, 0x6E, 0x76, 0x60, 0x60, 0x60, 0x60, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        's' => [
            0x00, 0x00, 0x00, 0x00, 0x3E, 0x60, 0x3C, 0x06, 0x06, 0x7C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        't' => [
            0x00, 0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x18, 0x18, 0x0E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'u' => [
            0x00, 0x00, 0x00, 0x00, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'v' => [
            0x00, 0x00, 0x00, 0x00, 0x66, 0x66, 0x66, 0x3C, 0x3C, 0x18, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'w' => [
            0x00, 0x00, 0x00, 0x00, 0xC6, 0xC6, 0xD6, 0xFE, 0xEE, 0xC6, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        'y' => [
            0x00, 0x00, 0x00, 0x00, 0x66, 0x66, 0x66, 0x3E, 0x06, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '0' => [
            0x00, 0x00, 0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '1' => [
            0x00, 0x00, 0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '2' => [
            0x00, 0x00, 0x3C, 0x66, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7E, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '3' => [
            0x00, 0x00, 0x3C, 0x66, 0x06, 0x1C, 0x06, 0x06, 0x66, 0x3C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '.' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        ':' => [
            0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '-' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '_' => [
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '/' => [
            0x00, 0x00, 0x02, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '>' => [
            0x00, 0x00, 0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        '|' => [
            0x00, 0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ],
        _ => {
            // Default: filled square for undefined characters
            [
                0x00, 0x00, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0x7E, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
        }
    }
}

/// Initialize font subsystem
pub fn init() {
    serial_println!("  Font: built-in 8x16 bitmap renderer ready");
}
