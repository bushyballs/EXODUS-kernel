use crate::serial_println;
/// Round LCD display driver for Genesis wearable
///
/// Targets the GC9A01 circular display (240×240 pixels) used on smart watches
/// and wearable development boards.  The driver is also compatible with any
/// round display that exposes a standard 16-bit RGB565 SPI framebuffer.
///
/// ## Architecture
///
/// A 240×240 RGB565 backbuffer lives in `FRAMEBUF`.  Higher-level callers
/// (watch face, complications, fitness UI) render into it using the helper
/// drawing primitives exported here.  `flush()` copies the backbuffer to the
/// hardware framebuffer / SPI display.
///
/// ## Coordinate system
///
/// Origin (0, 0) is the **top-left** corner.  The display is square in memory
/// even though only a circular region is visible on the physical panel.
///
/// ## Pixel format
///
/// RGB565 (big-endian on the wire, native u16 in the buffer):
///   bits [15:11] — red   (5 bits)
///   bits [10:5]  — green (6 bits)
///   bits [4:0]   — blue  (5 bits)
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

pub const DISPLAY_WIDTH: usize = 240;
pub const DISPLAY_HEIGHT: usize = 240;
const PIXEL_COUNT: usize = DISPLAY_WIDTH * DISPLAY_HEIGHT;

/// Black in RGB565
pub const COLOR_BLACK: u16 = 0x0000;
/// White in RGB565
pub const COLOR_WHITE: u16 = 0xFFFF;
/// Amber (#F59E0B) in RGB565: R=30, G=39, B=1
pub const COLOR_AMBER: u16 = (30 << 11) | (39 << 5) | 1;
/// Red
pub const COLOR_RED: u16 = 0xF800;
/// Green
pub const COLOR_GREEN: u16 = 0x07E0;
/// Blue
pub const COLOR_BLUE: u16 = 0x001F;
/// Dark grey
pub const COLOR_DARK_GREY: u16 = 0x2104;

// ============================================================================
// Framebuffer
// ============================================================================

/// The backbuffer.  `flush()` copies this to the hardware.
static FRAMEBUF: Mutex<[u16; PIXEL_COUNT]> = Mutex::new([COLOR_BLACK; PIXEL_COUNT]);

/// Whether the display hardware is present and initialised.
static DISPLAY_PRESENT: Mutex<bool> = Mutex::new(false);

// ============================================================================
// Pixel helpers
// ============================================================================

/// Pack R, G, B (0-255 each) into an RGB565 u16.
#[inline(always)]
pub fn rgb(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3)
}

/// Linearly blend two RGB565 colours.
/// `alpha` 0 = fully `a`, 255 = fully `b`.
pub fn blend(a: u16, b: u16, alpha: u8) -> u16 {
    let alpha16 = alpha as u16;
    let inv = 255u16 - alpha16;

    let ra = ((a >> 11) & 0x1F) * inv / 255;
    let ga = ((a >> 5) & 0x3F) * inv / 255;
    let ba = (a & 0x1F) * inv / 255;

    let rb = ((b >> 11) & 0x1F) * alpha16 / 255;
    let gb = ((b >> 5) & 0x3F) * alpha16 / 255;
    let bb = (b & 0x1F) * alpha16 / 255;

    ((ra + rb) << 11) | ((ga + gb) << 5) | (ba + bb)
}

// ============================================================================
// Drawing primitives — all operate on the internal backbuffer
// ============================================================================

/// Set a single pixel.  Silently ignores out-of-bounds coordinates.
#[inline]
pub fn set_pixel(fb: &mut [u16; PIXEL_COUNT], x: i32, y: i32, color: u16) {
    if x < 0 || y < 0 || x >= DISPLAY_WIDTH as i32 || y >= DISPLAY_HEIGHT as i32 {
        return;
    }
    fb[(y as usize) * DISPLAY_WIDTH + (x as usize)] = color;
}

/// Fill the entire backbuffer with a colour.
pub fn clear(color: u16) {
    let mut fb = FRAMEBUF.lock();
    for px in fb.iter_mut() {
        *px = color;
    }
}

/// Draw a horizontal line.
pub fn hline(fb: &mut [u16; PIXEL_COUNT], x0: i32, x1: i32, y: i32, color: u16) {
    if y < 0 || y >= DISPLAY_HEIGHT as i32 {
        return;
    }
    let left = x0.max(0) as usize;
    let right = (x1.min(DISPLAY_WIDTH as i32 - 1)) as usize;
    if left > right {
        return;
    }
    let row = y as usize * DISPLAY_WIDTH;
    for x in left..=right {
        fb[row + x] = color;
    }
}

/// Draw a vertical line.
pub fn vline(fb: &mut [u16; PIXEL_COUNT], x: i32, y0: i32, y1: i32, color: u16) {
    if x < 0 || x >= DISPLAY_WIDTH as i32 {
        return;
    }
    let top = y0.max(0) as usize;
    let bot = (y1.min(DISPLAY_HEIGHT as i32 - 1)) as usize;
    for y in top..=bot {
        fb[y * DISPLAY_WIDTH + x as usize] = color;
    }
}

/// Fill a rectangle (clipped to display bounds).
pub fn fill_rect(fb: &mut [u16; PIXEL_COUNT], x: i32, y: i32, w: i32, h: i32, color: u16) {
    for row in y..(y + h) {
        hline(fb, x, x + w - 1, row, color);
    }
}

/// Draw an axis-aligned rectangle outline.
pub fn draw_rect(fb: &mut [u16; PIXEL_COUNT], x: i32, y: i32, w: i32, h: i32, color: u16) {
    hline(fb, x, x + w - 1, y, color);
    hline(fb, x, x + w - 1, y + h - 1, color);
    vline(fb, x, y, y + h - 1, color);
    vline(fb, x + w - 1, y, y + h - 1, color);
}

/// Draw a circle outline using the midpoint algorithm (integer arithmetic only).
///
/// `cx`, `cy` — centre; `r` — radius in pixels.
pub fn draw_circle(fb: &mut [u16; PIXEL_COUNT], cx: i32, cy: i32, r: i32, color: u16) {
    if r <= 0 {
        return;
    }
    let mut x = 0i32;
    let mut y = r;
    let mut d = 1 - r;

    while x <= y {
        // 8 symmetric points
        for (px, py) in [
            (cx + x, cy + y),
            (cx - x, cy + y),
            (cx + x, cy - y),
            (cx - x, cy - y),
            (cx + y, cy + x),
            (cx - y, cy + x),
            (cx + y, cy - x),
            (cx - y, cy - x),
        ] {
            set_pixel(fb, px, py, color);
        }

        if d < 0 {
            d += 2 * x + 3;
        } else {
            d += 2 * (x - y) + 5;
            y -= 1;
        }
        x += 1;
    }
}

/// Fill a circle using horizontal scanlines.
pub fn fill_circle(fb: &mut [u16; PIXEL_COUNT], cx: i32, cy: i32, r: i32, color: u16) {
    if r <= 0 {
        return;
    }
    let mut x = 0i32;
    let mut y = r;
    let mut d = 1 - r;

    while x <= y {
        hline(fb, cx - x, cx + x, cy + y, color);
        hline(fb, cx - x, cx + x, cy - y, color);
        hline(fb, cx - y, cx + y, cy + x, color);
        hline(fb, cx - y, cx + y, cy - x, color);

        if d < 0 {
            d += 2 * x + 3;
        } else {
            d += 2 * (x - y) + 5;
            y -= 1;
        }
        x += 1;
    }
}

/// Draw an arc (partial circle outline).
///
/// `start_deg` and `end_deg` are in degrees (0-359), measured clockwise
/// from the top of the circle (12 o'clock position).
///
/// Uses fixed-point sin/cos via a 360-entry lookup table (256-scaled).
pub fn draw_arc(
    fb: &mut [u16; PIXEL_COUNT],
    cx: i32,
    cy: i32,
    r: i32,
    start_deg: u16,
    end_deg: u16,
    color: u16,
) {
    if r <= 0 {
        return;
    }

    let (start, end) = (start_deg as i32, end_deg as i32);
    let mut deg = start;

    loop {
        let d = deg.rem_euclid(360) as usize;
        // sin/cos scaled ×256 from lookup
        let s = SIN_TABLE[d];
        let c = COS_TABLE[d];
        // "12 o'clock" = top = negative Y in screen coords, so:
        //   x = cx + r * sin(deg)
        //   y = cy - r * cos(deg)
        let px = cx + (r * s) / 256;
        let py = cy - (r * c) / 256;
        set_pixel(fb, px, py, color);

        if deg == end {
            break;
        }
        deg += 1;
        if deg > 360 + start {
            break; // safety — shouldn't happen with well-formed inputs
        }
    }
}

// ============================================================================
// Simple bitmap font — 5×7 pixels per character (ASCII 32-127)
// ============================================================================

/// Draw a single ASCII character at (x, y).  Returns the x-advance (6 px).
pub fn draw_char(fb: &mut [u16; PIXEL_COUNT], x: i32, y: i32, c: u8, color: u16) -> i32 {
    if c < 32 || c > 126 {
        return 6;
    }
    let idx = (c - 32) as usize;
    let glyph = &FONT_5X7[idx];
    for (col, &column_bits) in glyph.iter().enumerate() {
        for row in 0..7usize {
            if column_bits & (1 << row) != 0 {
                set_pixel(fb, x + col as i32, y + row as i32, color);
            }
        }
    }
    6 // 5 px glyph + 1 px spacing
}

/// Draw a null-terminated string at (x, y).  Returns final x position.
pub fn draw_string(fb: &mut [u16; PIXEL_COUNT], x: i32, y: i32, text: &[u8], color: u16) -> i32 {
    let mut cx = x;
    for &b in text {
        if b == 0 {
            break;
        }
        cx += draw_char(fb, cx, y, b, color);
    }
    cx
}

// ============================================================================
// Flush / hardware interface
// ============================================================================

/// Mark display hardware as present (called during board init).
pub fn set_present(present: bool) {
    *DISPLAY_PRESENT.lock() = present;
}

/// Copy the backbuffer to the display hardware.
///
/// In a real implementation this would DMA the 240×240×2 = 115 200 bytes
/// to the GC9A01 via SPI.  Here we provide the hook for the hardware driver
/// to call.
///
/// `hardware_flush_fn` — platform-specific SPI/DMA function.
///   Receives a pointer to the raw pixel data and its byte length.
pub fn flush<F>(hardware_flush_fn: F)
where
    F: FnOnce(*const u16, usize),
{
    let fb = FRAMEBUF.lock();
    hardware_flush_fn(fb.as_ptr(), fb.len());
}

/// Lock the framebuffer and call `draw_fn` with a mutable reference.
///
/// This is the primary entry point for watch-face and complication renderers.
pub fn with_framebuffer<F>(draw_fn: F)
where
    F: FnOnce(&mut [u16; PIXEL_COUNT]),
{
    let mut fb = FRAMEBUF.lock();
    draw_fn(&mut *fb);
}

// ============================================================================
// Module init
// ============================================================================

pub fn init() {
    serial_println!("    Wearable/display: round LCD (240×240 RGB565) driver ready");
}

// ============================================================================
// Fixed-point sin/cos lookup tables (256-scaled, degrees 0-359)
// ============================================================================
//
// Generated as: round(sin(deg * π / 180) * 256).
// Only the first quadrant is stored for space but here we store all 360 for
// simplicity and correctness.

#[rustfmt::skip]
const SIN_TABLE: [i32; 374] = [
    0,4,9,13,18,22,27,31,36,40,44,49,53,57,62,66,70,74,79,83,87,91,95,99,103,107,111,115,
    119,122,126,130,133,137,140,144,147,150,154,157,160,163,166,169,172,175,178,180,183,186,
    188,191,193,196,198,200,202,205,207,209,211,212,214,216,218,219,221,222,224,225,226,228,
    229,230,231,232,233,234,235,235,236,237,237,238,238,239,239,239,240,240,240,240,240,240,
    240,240,240,240,240,240,239,239,239,238,238,237,237,236,235,235,234,233,232,231,230,229,
    228,226,225,224,222,221,219,218,216,214,212,211,209,207,205,202,200,198,196,193,191,188,
    186,183,180,178,175,172,169,166,163,160,157,154,150,147,144,140,137,133,130,126,122,119,
    115,111,107,103,99,95,91,87,83,79,74,70,66,62,57,53,49,44,40,36,31,27,22,18,13,9,4,0,
    -4,-9,-13,-18,-22,-27,-31,-36,-40,-44,-49,-53,-57,-62,-66,-70,-74,-79,-83,-87,-91,-95,
    -99,-103,-107,-111,-115,-119,-122,-126,-130,-133,-137,-140,-144,-147,-150,-154,-157,-160,
    -163,-166,-169,-172,-175,-178,-180,-183,-186,-188,-191,-193,-196,-198,-200,-202,-205,-207,
    -209,-211,-212,-214,-216,-218,-219,-221,-222,-224,-225,-226,-228,-229,-230,-231,-232,-233,
    -234,-235,-235,-236,-237,-237,-238,-238,-239,-239,-239,-240,-240,-240,-240,-240,-240,-240,
    -240,-240,-240,-240,-240,-239,-239,-239,-238,-238,-237,-237,-236,-235,-235,-234,-233,-232,
    -231,-230,-229,-228,-226,-225,-224,-222,-221,-219,-218,-216,-214,-212,-211,-209,-207,-205,
    -202,-200,-198,-196,-193,-191,-188,-186,-183,-180,-178,-175,-172,-169,-166,-163,-160,-157,
    -154,-150,-147,-144,-140,-137,-133,-130,-126,-122,-119,-115,-111,-107,-103,-99,-95,-91,
    -87,-83,-79,-74,-70,-66,-62,-57,-53,-49,-44,-40,-36,-31,-27,-22,-18,-13,-9,-4,
];

#[rustfmt::skip]
const COS_TABLE: [i32; 368] = [
    256,256,255,255,254,253,252,251,250,248,247,245,244,242,240,238,236,234,231,229,226,224,
    221,218,215,212,209,206,202,199,195,192,188,184,180,176,172,168,164,160,156,151,147,143,
    138,134,129,124,120,115,110,105,100,95,90,85,80,75,70,65,60,54,49,44,39,34,28,23,18,13,
    7,2,-3,-8,-13,-18,-24,-29,-34,-39,-44,-50,-55,-60,-65,-70,-75,-80,-85,-90,-95,-100,-105,
    -110,-115,-120,-125,-129,-134,-139,-143,-148,-152,-157,-161,-165,-170,-174,-178,-182,-186,
    -190,-193,-197,-201,-204,-207,-211,-214,-217,-220,-223,-225,-228,-230,-233,-235,-237,-239,
    -241,-243,-244,-246,-247,-248,-250,-251,-252,-253,-253,-254,-255,-255,-255,-256,-256,-256,
    -256,-256,-255,-255,-255,-254,-253,-252,-251,-250,-248,-247,-245,-244,-242,-240,-238,-236,
    -234,-231,-229,-226,-224,-221,-218,-215,-212,-209,-206,-202,-199,-195,-192,-188,-184,-180,
    -176,-172,-168,-164,-160,-156,-151,-147,-143,-138,-134,-129,-124,-120,-115,-110,-105,-100,
    -95,-90,-85,-80,-75,-70,-65,-60,-54,-49,-44,-39,-34,-28,-23,-18,-13,-7,-2,3,8,13,18,24,
    29,34,39,44,50,55,60,65,70,75,80,85,90,95,100,105,110,115,120,125,129,134,139,143,148,
    152,157,161,165,170,174,178,182,186,190,193,197,201,204,207,211,214,217,220,223,225,228,
    230,233,235,237,239,241,243,244,246,247,248,250,251,252,253,253,254,255,255,255,256,256,
    256,256,256,255,255,255,254,253,252,251,250,248,247,245,244,242,240,238,236,234,231,229,
    226,224,221,218,215,212,209,206,202,199,195,192,188,184,180,176,172,168,164,160,156,151,
    147,143,138,134,129,124,120,115,110,105,100,95,90,85,80,75,70,65,60,54,49,44,39,34,28,
    23,18,13,7,2,
];

// ============================================================================
// Minimal 5×7 bitmap font (ASCII 32-127)
// Each entry is a 5-element array of column bitmaps (bit 0 = top row).
// ============================================================================

#[rustfmt::skip]
const FONT_5X7: [[u8; 5]; 96] = [
    // 32 ' '
    [0x00, 0x00, 0x00, 0x00, 0x00],
    // 33 '!'
    [0x00, 0x00, 0x5F, 0x00, 0x00],
    // 34 '"'
    [0x00, 0x07, 0x00, 0x07, 0x00],
    // 35 '#'
    [0x14, 0x7F, 0x14, 0x7F, 0x14],
    // 36 '$'
    [0x24, 0x2A, 0x7F, 0x2A, 0x12],
    // 37 '%'
    [0x23, 0x13, 0x08, 0x64, 0x62],
    // 38 '&'
    [0x36, 0x49, 0x55, 0x22, 0x50],
    // 39 '''
    [0x00, 0x05, 0x03, 0x00, 0x00],
    // 40 '('
    [0x00, 0x1C, 0x22, 0x41, 0x00],
    // 41 ')'
    [0x00, 0x41, 0x22, 0x1C, 0x00],
    // 42 '*'
    [0x0A, 0x04, 0x1F, 0x04, 0x0A],
    // 43 '+'
    [0x08, 0x08, 0x3E, 0x08, 0x08],
    // 44 ','
    [0x00, 0x50, 0x30, 0x00, 0x00],
    // 45 '-'
    [0x08, 0x08, 0x08, 0x08, 0x08],
    // 46 '.'
    [0x00, 0x60, 0x60, 0x00, 0x00],
    // 47 '/'
    [0x20, 0x10, 0x08, 0x04, 0x02],
    // 48 '0'
    [0x3E, 0x51, 0x49, 0x45, 0x3E],
    // 49 '1'
    [0x00, 0x42, 0x7F, 0x40, 0x00],
    // 50 '2'
    [0x42, 0x61, 0x51, 0x49, 0x46],
    // 51 '3'
    [0x21, 0x41, 0x45, 0x4B, 0x31],
    // 52 '4'
    [0x18, 0x14, 0x12, 0x7F, 0x10],
    // 53 '5'
    [0x27, 0x45, 0x45, 0x45, 0x39],
    // 54 '6'
    [0x3C, 0x4A, 0x49, 0x49, 0x30],
    // 55 '7'
    [0x01, 0x71, 0x09, 0x05, 0x03],
    // 56 '8'
    [0x36, 0x49, 0x49, 0x49, 0x36],
    // 57 '9'
    [0x06, 0x49, 0x49, 0x29, 0x1E],
    // 58 ':'
    [0x00, 0x36, 0x36, 0x00, 0x00],
    // 59 ';'
    [0x00, 0x56, 0x36, 0x00, 0x00],
    // 60 '<'
    [0x08, 0x14, 0x22, 0x41, 0x00],
    // 61 '='
    [0x14, 0x14, 0x14, 0x14, 0x14],
    // 62 '>'
    [0x00, 0x41, 0x22, 0x14, 0x08],
    // 63 '?'
    [0x02, 0x01, 0x51, 0x09, 0x06],
    // 64 '@'
    [0x32, 0x49, 0x79, 0x41, 0x3E],
    // 65 'A'
    [0x7E, 0x11, 0x11, 0x11, 0x7E],
    // 66 'B'
    [0x7F, 0x49, 0x49, 0x49, 0x36],
    // 67 'C'
    [0x3E, 0x41, 0x41, 0x41, 0x22],
    // 68 'D'
    [0x7F, 0x41, 0x41, 0x22, 0x1C],
    // 69 'E'
    [0x7F, 0x49, 0x49, 0x49, 0x41],
    // 70 'F'
    [0x7F, 0x09, 0x09, 0x09, 0x01],
    // 71 'G'
    [0x3E, 0x41, 0x49, 0x49, 0x7A],
    // 72 'H'
    [0x7F, 0x08, 0x08, 0x08, 0x7F],
    // 73 'I'
    [0x00, 0x41, 0x7F, 0x41, 0x00],
    // 74 'J'
    [0x20, 0x40, 0x41, 0x3F, 0x01],
    // 75 'K'
    [0x7F, 0x08, 0x14, 0x22, 0x41],
    // 76 'L'
    [0x7F, 0x40, 0x40, 0x40, 0x40],
    // 77 'M'
    [0x7F, 0x02, 0x0C, 0x02, 0x7F],
    // 78 'N'
    [0x7F, 0x04, 0x08, 0x10, 0x7F],
    // 79 'O'
    [0x3E, 0x41, 0x41, 0x41, 0x3E],
    // 80 'P'
    [0x7F, 0x09, 0x09, 0x09, 0x06],
    // 81 'Q'
    [0x3E, 0x41, 0x51, 0x21, 0x5E],
    // 82 'R'
    [0x7F, 0x09, 0x19, 0x29, 0x46],
    // 83 'S'
    [0x46, 0x49, 0x49, 0x49, 0x31],
    // 84 'T'
    [0x01, 0x01, 0x7F, 0x01, 0x01],
    // 85 'U'
    [0x3F, 0x40, 0x40, 0x40, 0x3F],
    // 86 'V'
    [0x1F, 0x20, 0x40, 0x20, 0x1F],
    // 87 'W'
    [0x3F, 0x40, 0x38, 0x40, 0x3F],
    // 88 'X'
    [0x63, 0x14, 0x08, 0x14, 0x63],
    // 89 'Y'
    [0x07, 0x08, 0x70, 0x08, 0x07],
    // 90 'Z'
    [0x61, 0x51, 0x49, 0x45, 0x43],
    // 91 '['
    [0x00, 0x7F, 0x41, 0x41, 0x00],
    // 92 '\'
    [0x02, 0x04, 0x08, 0x10, 0x20],
    // 93 ']'
    [0x00, 0x41, 0x41, 0x7F, 0x00],
    // 94 '^'
    [0x04, 0x02, 0x01, 0x02, 0x04],
    // 95 '_'
    [0x40, 0x40, 0x40, 0x40, 0x40],
    // 96 '`'
    [0x00, 0x01, 0x02, 0x04, 0x00],
    // 97 'a'
    [0x20, 0x54, 0x54, 0x54, 0x78],
    // 98 'b'
    [0x7F, 0x48, 0x44, 0x44, 0x38],
    // 99 'c'
    [0x38, 0x44, 0x44, 0x44, 0x20],
    // 100 'd'
    [0x38, 0x44, 0x44, 0x48, 0x7F],
    // 101 'e'
    [0x38, 0x54, 0x54, 0x54, 0x18],
    // 102 'f'
    [0x08, 0x7E, 0x09, 0x01, 0x02],
    // 103 'g'
    [0x0C, 0x52, 0x52, 0x52, 0x3E],
    // 104 'h'
    [0x7F, 0x08, 0x04, 0x04, 0x78],
    // 105 'i'
    [0x00, 0x44, 0x7D, 0x40, 0x00],
    // 106 'j'
    [0x20, 0x40, 0x44, 0x3D, 0x00],
    // 107 'k'
    [0x7F, 0x10, 0x28, 0x44, 0x00],
    // 108 'l'
    [0x00, 0x41, 0x7F, 0x40, 0x00],
    // 109 'm'
    [0x7C, 0x04, 0x18, 0x04, 0x78],
    // 110 'n'
    [0x7C, 0x08, 0x04, 0x04, 0x78],
    // 111 'o'
    [0x38, 0x44, 0x44, 0x44, 0x38],
    // 112 'p'
    [0x7C, 0x14, 0x14, 0x14, 0x08],
    // 113 'q'
    [0x08, 0x14, 0x14, 0x18, 0x7C],
    // 114 'r'
    [0x7C, 0x08, 0x04, 0x04, 0x08],
    // 115 's'
    [0x48, 0x54, 0x54, 0x54, 0x20],
    // 116 't'
    [0x04, 0x3F, 0x44, 0x40, 0x20],
    // 117 'u'
    [0x3C, 0x40, 0x40, 0x20, 0x7C],
    // 118 'v'
    [0x1C, 0x20, 0x40, 0x20, 0x1C],
    // 119 'w'
    [0x3C, 0x40, 0x30, 0x40, 0x3C],
    // 120 'x'
    [0x44, 0x28, 0x10, 0x28, 0x44],
    // 121 'y'
    [0x0C, 0x50, 0x50, 0x50, 0x3C],
    // 122 'z'
    [0x44, 0x64, 0x54, 0x4C, 0x44],
    // 123 '{'
    [0x00, 0x08, 0x36, 0x41, 0x00],
    // 124 '|'
    [0x00, 0x00, 0x7F, 0x00, 0x00],
    // 125 '}'
    [0x00, 0x41, 0x36, 0x08, 0x00],
    // 126 '~'
    [0x0C, 0x02, 0x06, 0x04, 0x08],
    // 127 DEL (placeholder)
    [0x00, 0x00, 0x00, 0x00, 0x00],
];
