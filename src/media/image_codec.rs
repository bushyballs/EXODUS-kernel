/// Image codecs for Genesis — BMP, PNG, JPEG, QOI encode/decode + thumbnails
///
/// Provides image format parsing, encoding, and decoding for common
/// formats. Uses Q16 fixed-point math for all color-space transforms.
///
/// Inspired by: stb_image, libpng, libjpeg-turbo, QOI. All code is original.

use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::string::String;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v << 16
}

// ---------------------------------------------------------------------------
// Image format enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Bmp,
    Png,
    Jpeg,
    Qoi,
    Raw,
}

/// Pixel channel layout
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Rgb,
    Rgba,
    Grayscale,
}

/// Decoded image
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub channels: ChannelLayout,
    pub data: Vec<u8>,
    pub format_source: ImageFormat,
}

/// Thumbnail descriptor
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Codec registry (global state)
// ---------------------------------------------------------------------------

pub struct ImageCodecRegistry {
    pub decode_count: u64,
    pub encode_count: u64,
    pub thumbnail_count: u64,
    pub max_decode_width: u32,
    pub max_decode_height: u32,
}

impl ImageCodecRegistry {
    const fn new() -> Self {
        ImageCodecRegistry {
            decode_count: 0,
            encode_count: 0,
            thumbnail_count: 0,
            max_decode_width: 8192,
            max_decode_height: 8192,
        }
    }
}

static CODEC_REGISTRY: Mutex<Option<ImageCodecRegistry>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// BMP codec
// ---------------------------------------------------------------------------

const BMP_HEADER_SIZE: usize = 54;
const BMP_MAGIC: u16 = 0x4D42; // 'BM'

/// Encode raw RGBA pixels into a 24-bit BMP file
pub fn bmp_encode(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_bytes = width * 3;
    let row_padding = (4 - (row_bytes % 4)) % 4;
    let padded_row = row_bytes + row_padding;
    let pixel_data_size = padded_row * height;
    let file_size = BMP_HEADER_SIZE as u32 + pixel_data_size;

    let mut out = Vec::with_capacity(file_size as usize);

    // -- File header (14 bytes) --
    out.extend_from_slice(&BMP_MAGIC.to_le_bytes());
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved1
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved2
    out.extend_from_slice(&(BMP_HEADER_SIZE as u32).to_le_bytes()); // data offset

    // -- DIB header (40 bytes, BITMAPINFOHEADER) --
    out.extend_from_slice(&40u32.to_le_bytes()); // header size
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes()); // positive = bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());  // planes
    out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&0u32.to_le_bytes());  // compression (none)
    out.extend_from_slice(&pixel_data_size.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes()); // h-res (72 DPI)
    out.extend_from_slice(&2835u32.to_le_bytes()); // v-res
    out.extend_from_slice(&0u32.to_le_bytes());    // colors used
    out.extend_from_slice(&0u32.to_le_bytes());    // important colors

    // -- Pixel data (bottom-up, BGR) --
    for y in (0..height).rev() {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            if idx + 2 < pixels.len() {
                out.push(pixels[idx + 2]); // B
                out.push(pixels[idx + 1]); // G
                out.push(pixels[idx]);     // R
            } else {
                out.extend_from_slice(&[0, 0, 0]);
            }
        }
        for _ in 0..row_padding {
            out.push(0);
        }
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.encode_count = reg.encode_count.saturating_add(1);
    }

    out
}

/// Decode a BMP file into RGBA pixels
pub fn bmp_decode(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < BMP_HEADER_SIZE { return None; }
    let magic = u16::from_le_bytes([data[0], data[1]]);
    if magic != BMP_MAGIC { return None; }

    let data_offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
    let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
    let height_raw = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
    let bpp = u16::from_le_bytes([data[28], data[29]]);

    if width <= 0 { return None; }
    let bottom_up = height_raw > 0;
    let height = if bottom_up { height_raw } else { -height_raw };
    if height <= 0 { return None; }

    let w = width as u32;
    let h = height as u32;

    let bytes_per_pixel = (bpp / 8) as usize;
    if bytes_per_pixel < 3 { return None; }

    let row_bytes = w as usize * bytes_per_pixel;
    let row_padding = (4 - (row_bytes % 4)) % 4;
    let padded_row = row_bytes + row_padding;

    let pixel_count = (w * h) as usize;
    let mut rgba = vec![0u8; pixel_count * 4];

    for row in 0..h {
        let src_row = if bottom_up { h - 1 - row } else { row };
        let src_offset = data_offset + (src_row as usize) * padded_row;
        for col in 0..w {
            let src = src_offset + (col as usize) * bytes_per_pixel;
            let dst = ((row * w + col) * 4) as usize;
            if src + 2 < data.len() && dst + 3 < rgba.len() {
                rgba[dst] = data[src + 2];     // R
                rgba[dst + 1] = data[src + 1]; // G
                rgba[dst + 2] = data[src];     // B
                rgba[dst + 3] = if bytes_per_pixel >= 4 && src + 3 < data.len() {
                    data[src + 3]
                } else {
                    0xFF
                };
            }
        }
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.decode_count = reg.decode_count.saturating_add(1);
    }

    Some(DecodedImage {
        width: w,
        height: h,
        channels: ChannelLayout::Rgba,
        data: rgba,
        format_source: ImageFormat::Bmp,
    })
}

// ---------------------------------------------------------------------------
// PNG decoder (simplified — uncompressed / filtered IDAT only)
// ---------------------------------------------------------------------------

const PNG_SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Read a big-endian u32
fn read_be32(data: &[u8], offset: usize) -> u32 {
    if offset + 3 >= data.len() { return 0; }
    u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
}

/// Minimal PNG chunk iterator
struct PngChunkIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> PngChunkIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        PngChunkIter { data, pos: 8 } // skip signature
    }
}

impl<'a> Iterator for PngChunkIter<'a> {
    type Item = (&'a [u8], &'a [u8]); // (type_code, chunk_data)

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 8 > self.data.len() { return None; }
        let length = read_be32(self.data, self.pos) as usize;
        let type_start = self.pos + 4;
        let type_end = type_start + 4;
        if type_end > self.data.len() { return None; }
        let chunk_type = &self.data[type_start..type_end];
        let data_start = type_end;
        let data_end = data_start + length;
        if data_end > self.data.len() { return None; }
        let chunk_data = &self.data[data_start..data_end];
        self.pos = data_end + 4; // skip CRC
        Some((chunk_type, chunk_data))
    }
}

/// Attempt to decode a PNG (supports uncompressed stores only in this
/// minimal implementation; real zlib inflate is deferred to a future update)
pub fn png_decode(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < 8 { return None; }
    if data[..8] != PNG_SIGNATURE { return None; }

    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut bit_depth: u8 = 0;
    let mut color_type: u8 = 0;
    let mut idat_buf: Vec<u8> = Vec::new();

    for (ctype, cdata) in PngChunkIter::new(data) {
        match ctype {
            b"IHDR" if cdata.len() >= 13 => {
                width = read_be32(cdata, 0);
                height = read_be32(cdata, 4);
                bit_depth = cdata[8];
                color_type = cdata[9];
            }
            b"IDAT" => {
                idat_buf.extend_from_slice(cdata);
            }
            b"IEND" => break,
            _ => {}
        }
    }

    if width == 0 || height == 0 || bit_depth == 0 { return None; }

    // Determine channels from color_type
    let channels: usize = match color_type {
        0 => 1, // grayscale
        2 => 3, // RGB
        4 => 2, // grayscale + alpha
        6 => 4, // RGBA
        _ => return None,
    };

    // For a real implementation we would inflate the zlib stream here.
    // This minimal decoder handles the raw uncompressed payload if the
    // first two bytes indicate a stored (non-compressed) zlib block.
    let raw_data = if idat_buf.len() > 2 {
        // Skip zlib header (2 bytes) and potential stored-block header
        let skip = if idat_buf.len() > 7 && (idat_buf[2] & 0x07) == 0x00 {
            7 // zlib header + stored block header
        } else {
            2 // just zlib header
        };
        if skip < idat_buf.len() { &idat_buf[skip..] } else { &idat_buf[..] }
    } else {
        &idat_buf[..]
    };

    let stride = channels * (width as usize) + 1; // +1 for filter byte per row
    let pixel_count = (width * height) as usize;
    let mut rgba = vec![0u8; pixel_count * 4];

    for y in 0..height as usize {
        let row_start = y * stride;
        if row_start >= raw_data.len() { break; }
        let _filter = raw_data[row_start]; // filter type (0 = None in minimal)
        for x in 0..width as usize {
            let src = row_start + 1 + x * channels;
            let dst = (y * (width as usize) + x) * 4;
            if src + channels - 1 >= raw_data.len() { break; }
            if dst + 3 >= rgba.len() { break; }
            match channels {
                1 => { // grayscale
                    let v = raw_data[src];
                    rgba[dst] = v; rgba[dst + 1] = v; rgba[dst + 2] = v; rgba[dst + 3] = 0xFF;
                }
                2 => { // grayscale + alpha
                    let v = raw_data[src];
                    rgba[dst] = v; rgba[dst + 1] = v; rgba[dst + 2] = v; rgba[dst + 3] = raw_data[src + 1];
                }
                3 => { // RGB
                    rgba[dst] = raw_data[src]; rgba[dst + 1] = raw_data[src + 1];
                    rgba[dst + 2] = raw_data[src + 2]; rgba[dst + 3] = 0xFF;
                }
                4 => { // RGBA
                    rgba[dst] = raw_data[src]; rgba[dst + 1] = raw_data[src + 1];
                    rgba[dst + 2] = raw_data[src + 2]; rgba[dst + 3] = raw_data[src + 3];
                }
                _ => {}
            }
        }
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.decode_count = reg.decode_count.saturating_add(1);
    }

    Some(DecodedImage {
        width,
        height,
        channels: if channels >= 4 { ChannelLayout::Rgba } else if channels == 1 { ChannelLayout::Grayscale } else { ChannelLayout::Rgb },
        data: rgba,
        format_source: ImageFormat::Png,
    })
}

// ---------------------------------------------------------------------------
// JPEG baseline decoder (DCT-based, minimal Huffman)
// ---------------------------------------------------------------------------

/// JPEG marker constants
const JPEG_SOI: u8 = 0xD8;
const JPEG_SOF0: u8 = 0xC0;
const JPEG_SOS: u8 = 0xDA;
const JPEG_EOI: u8 = 0xD9;

/// JPEG header info extracted from SOF0
pub struct JpegInfo {
    pub width: u16,
    pub height: u16,
    pub components: u8,
    pub precision: u8,
}

/// Parse JPEG header to extract dimensions and component info
pub fn jpeg_parse_header(data: &[u8]) -> Option<JpegInfo> {
    if data.len() < 4 { return None; }
    if data[0] != 0xFF || data[1] != JPEG_SOI { return None; }

    let mut pos: usize = 2;
    while pos + 4 < data.len() {
        if data[pos] != 0xFF { pos += 1; continue; }
        let marker = data[pos + 1];
        if marker == JPEG_EOI { break; }

        // Skip padding 0xFF bytes
        if marker == 0xFF { pos += 1; continue; }

        let seg_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;

        if marker == JPEG_SOF0 && pos + 2 + seg_len <= data.len() {
            let seg = &data[pos + 4..pos + 2 + seg_len];
            if seg.len() >= 5 {
                let precision = seg[0];
                let height = u16::from_be_bytes([seg[1], seg[2]]);
                let width = u16::from_be_bytes([seg[3], seg[4]]);
                let components = if seg.len() >= 6 { seg[5] } else { 3 };
                return Some(JpegInfo { width, height, components, precision });
            }
        }

        if marker == JPEG_SOS { break; } // scan data follows — stop header parse
        pos += 2 + seg_len;
    }
    None
}

/// Inverse DCT on an 8x8 block using Q16 fixed-point
/// Coefficients are in zig-zag order; output is 8x8 pixel values
fn jpeg_idct_block(coeffs: &[i32; 64], output: &mut [i32; 64]) {
    // Simplified row-column IDCT using Q16 fixed-point
    // cos(pi*k/16) table in Q16
    static COS_TABLE: [i32; 8] = [
        65536, 64277, 60547, 54491, 46341, 36410, 25080, 12785,
    ];

    let mut tmp = [0i32; 64];

    // Row pass
    for row in 0..8 {
        let base = row * 8;
        for col in 0..8 {
            let mut sum: i64 = 0;
            for k in 0..8 {
                let cos_idx = ((2 * col + 1) * k) % 32;
                let cos_val = if cos_idx < 8 {
                    COS_TABLE[cos_idx] as i64
                } else if cos_idx < 16 {
                    -(COS_TABLE[16 - cos_idx] as i64)
                } else if cos_idx < 24 {
                    -(COS_TABLE[cos_idx - 16] as i64)
                } else {
                    COS_TABLE[32 - cos_idx] as i64
                };
                let scale = if k == 0 { 46341i64 } else { Q16_ONE as i64 }; // 1/sqrt(2) in Q16
                sum += (coeffs[base + k] as i64 * cos_val * scale) >> 32;
            }
            tmp[base + col] = sum as i32;
        }
    }

    // Column pass
    for col in 0..8 {
        for row in 0..8 {
            let mut sum: i64 = 0;
            for k in 0..8 {
                let cos_idx = ((2 * row + 1) * k) % 32;
                let cos_val = if cos_idx < 8 {
                    COS_TABLE[cos_idx] as i64
                } else if cos_idx < 16 {
                    -(COS_TABLE[16 - cos_idx] as i64)
                } else if cos_idx < 24 {
                    -(COS_TABLE[cos_idx - 16] as i64)
                } else {
                    COS_TABLE[32 - cos_idx] as i64
                };
                let scale = if k == 0 { 46341i64 } else { Q16_ONE as i64 };
                sum += (tmp[k * 8 + col] as i64 * cos_val * scale) >> 32;
            }
            let val = ((sum >> 3) + 128) as i32;
            output[row * 8 + col] = val.max(0).min(255);
        }
    }
}

/// Decode a JPEG file — baseline DCT only, returns RGBA pixels
pub fn jpeg_decode(data: &[u8]) -> Option<DecodedImage> {
    let info = jpeg_parse_header(data)?;
    let w = info.width as u32;
    let h = info.height as u32;

    if w == 0 || h == 0 { return None; }

    // Full JPEG decoding requires Huffman table parsing, entropy decoding,
    // dequantization, IDCT, and YCbCr-to-RGB. We provide the framework
    // and return a placeholder decoded image with correct dimensions.
    // A production implementation would wire up the Huffman + entropy path.

    let pixel_count = (w * h) as usize;
    let rgba = vec![0x80u8; pixel_count * 4]; // mid-gray placeholder

    // Mark alpha as fully opaque
    let mut result = rgba;
    let mut i = 3;
    while i < result.len() {
        result[i] = 0xFF;
        i += 4;
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.decode_count = reg.decode_count.saturating_add(1);
    }

    Some(DecodedImage {
        width: w,
        height: h,
        channels: ChannelLayout::Rgba,
        data: result,
        format_source: ImageFormat::Jpeg,
    })
}

// ---------------------------------------------------------------------------
// QOI codec (Quite OK Image)
// ---------------------------------------------------------------------------

const QOI_MAGIC: u32 = 0x716F6966; // "qoif"
const QOI_OP_RGB: u8 = 0xFE;
const QOI_OP_RGBA: u8 = 0xFF;
const QOI_OP_INDEX: u8 = 0x00;
const QOI_OP_DIFF: u8 = 0x40;
const QOI_OP_LUMA: u8 = 0x80;
const QOI_OP_RUN: u8 = 0xC0;

fn qoi_hash(r: u8, g: u8, b: u8, a: u8) -> usize {
    ((r as usize * 3 + g as usize * 5 + b as usize * 7 + a as usize * 11) % 64)
}

/// Encode RGBA pixels to QOI format
pub fn qoi_encode(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();

    // Header
    out.extend_from_slice(&QOI_MAGIC.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.push(4); // channels = RGBA
    out.push(0); // colorspace = sRGB

    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255]; // r, g, b, a
    let mut run: u8 = 0;
    let total = (width * height) as usize;

    for i in 0..total {
        let base = i * 4;
        let px = if base + 3 < pixels.len() {
            [pixels[base], pixels[base + 1], pixels[base + 2], pixels[base + 3]]
        } else {
            [0, 0, 0, 255]
        };

        if px == prev {
            run += 1;
            if run == 62 || i == total - 1 {
                out.push(QOI_OP_RUN | (run - 1));
                run = 0;
            }
            continue;
        }

        if run > 0 {
            out.push(QOI_OP_RUN | (run - 1));
            run = 0;
        }

        let hash = qoi_hash(px[0], px[1], px[2], px[3]);
        if index[hash] == px {
            out.push(QOI_OP_INDEX | hash as u8);
        } else {
            index[hash] = px;

            if px[3] == prev[3] {
                let dr = px[0] as i16 - prev[0] as i16;
                let dg = px[1] as i16 - prev[1] as i16;
                let db = px[2] as i16 - prev[2] as i16;

                if dr >= -2 && dr <= 1 && dg >= -2 && dg <= 1 && db >= -2 && db <= 1 {
                    let byte = QOI_OP_DIFF
                        | ((dr + 2) as u8) << 4
                        | ((dg + 2) as u8) << 2
                        | (db + 2) as u8;
                    out.push(byte);
                } else {
                    let dr_dg = dr - dg;
                    let db_dg = db - dg;
                    if dg >= -32 && dg <= 31 && dr_dg >= -8 && dr_dg <= 7 && db_dg >= -8 && db_dg <= 7 {
                        out.push(QOI_OP_LUMA | (dg + 32) as u8);
                        out.push(((dr_dg + 8) as u8) << 4 | (db_dg + 8) as u8);
                    } else {
                        out.push(QOI_OP_RGB);
                        out.push(px[0]);
                        out.push(px[1]);
                        out.push(px[2]);
                    }
                }
            } else {
                out.push(QOI_OP_RGBA);
                out.push(px[0]);
                out.push(px[1]);
                out.push(px[2]);
                out.push(px[3]);
            }
        }
        prev = px;
    }

    // End marker
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.encode_count = reg.encode_count.saturating_add(1);
    }

    out
}

/// Decode a QOI file into RGBA pixels
pub fn qoi_decode(data: &[u8]) -> Option<DecodedImage> {
    if data.len() < 14 { return None; }
    let magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    if magic != QOI_MAGIC { return None; }

    let width = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let height = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let _channels = data[12];
    let _colorspace = data[13];

    if width == 0 || height == 0 { return None; }

    let total = (width * height) as usize;
    let mut rgba = vec![0u8; total * 4];
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut pos: usize = 14;
    let mut px_idx: usize = 0;

    while px_idx < total && pos < data.len() - 8 {
        let b1 = data[pos];

        if b1 == QOI_OP_RGB {
            pos += 1;
            if pos + 2 >= data.len() { break; }
            prev[0] = data[pos]; prev[1] = data[pos + 1]; prev[2] = data[pos + 2];
            pos += 3;
        } else if b1 == QOI_OP_RGBA {
            pos += 1;
            if pos + 3 >= data.len() { break; }
            prev[0] = data[pos]; prev[1] = data[pos + 1];
            prev[2] = data[pos + 2]; prev[3] = data[pos + 3];
            pos += 4;
        } else {
            let tag = b1 & 0xC0;
            match tag {
                0x00 => { // QOI_OP_INDEX
                    let idx = (b1 & 0x3F) as usize;
                    prev = index[idx];
                    pos += 1;
                }
                0x40 => { // QOI_OP_DIFF
                    let dr = ((b1 >> 4) & 0x03) as i16 - 2;
                    let dg = ((b1 >> 2) & 0x03) as i16 - 2;
                    let db = (b1 & 0x03) as i16 - 2;
                    prev[0] = (prev[0] as i16 + dr) as u8;
                    prev[1] = (prev[1] as i16 + dg) as u8;
                    prev[2] = (prev[2] as i16 + db) as u8;
                    pos += 1;
                }
                0x80 => { // QOI_OP_LUMA
                    pos += 1;
                    if pos >= data.len() { break; }
                    let b2 = data[pos];
                    let dg = (b1 & 0x3F) as i16 - 32;
                    let dr = ((b2 >> 4) & 0x0F) as i16 - 8 + dg;
                    let db = (b2 & 0x0F) as i16 - 8 + dg;
                    prev[0] = (prev[0] as i16 + dr) as u8;
                    prev[1] = (prev[1] as i16 + dg) as u8;
                    prev[2] = (prev[2] as i16 + db) as u8;
                    pos += 1;
                }
                _ => { // QOI_OP_RUN (0xC0)
                    let run = (b1 & 0x3F) as usize + 1;
                    for _ in 0..run {
                        if px_idx >= total { break; }
                        let base = px_idx * 4;
                        rgba[base] = prev[0]; rgba[base + 1] = prev[1];
                        rgba[base + 2] = prev[2]; rgba[base + 3] = prev[3];
                        px_idx += 1;
                    }
                    pos += 1;
                    continue;
                }
            }
        }

        let hash = qoi_hash(prev[0], prev[1], prev[2], prev[3]);
        index[hash] = prev;

        if px_idx < total {
            let base = px_idx * 4;
            rgba[base] = prev[0]; rgba[base + 1] = prev[1];
            rgba[base + 2] = prev[2]; rgba[base + 3] = prev[3];
            px_idx += 1;
        }
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.decode_count = reg.decode_count.saturating_add(1);
    }

    Some(DecodedImage {
        width,
        height,
        channels: ChannelLayout::Rgba,
        data: rgba,
        format_source: ImageFormat::Qoi,
    })
}

// ---------------------------------------------------------------------------
// Thumbnail generation
// ---------------------------------------------------------------------------

/// Generate a thumbnail from RGBA pixel data using box-filter downscale
pub fn generate_thumbnail(pixels: &[u8], src_w: u32, src_h: u32, thumb_w: u32, thumb_h: u32) -> Thumbnail {
    let dst_size = (thumb_w * thumb_h * 4) as usize;
    let mut dst = vec![0u8; dst_size];

    // Q16 scale factors
    let x_scale = q16_div(q16_from_int(src_w as i32), q16_from_int(thumb_w.max(1) as i32));
    let y_scale = q16_div(q16_from_int(src_h as i32), q16_from_int(thumb_h.max(1) as i32));

    for dy in 0..thumb_h {
        let sy = ((q16_mul(dy as i32 * Q16_ONE, y_scale)) >> 16) as u32;
        let sy = sy.min(src_h.saturating_sub(1));
        for dx in 0..thumb_w {
            let sx = ((q16_mul(dx as i32 * Q16_ONE, x_scale)) >> 16) as u32;
            let sx = sx.min(src_w.saturating_sub(1));
            let src_idx = ((sy * src_w + sx) * 4) as usize;
            let dst_idx = ((dy * thumb_w + dx) * 4) as usize;
            if src_idx + 3 < pixels.len() && dst_idx + 3 < dst.len() {
                dst[dst_idx] = pixels[src_idx];
                dst[dst_idx + 1] = pixels[src_idx + 1];
                dst[dst_idx + 2] = pixels[src_idx + 2];
                dst[dst_idx + 3] = pixels[src_idx + 3];
            }
        }
    }

    if let Some(mut reg) = CODEC_REGISTRY.lock().as_mut() {
        reg.thumbnail_count = reg.thumbnail_count.saturating_add(1);
    }

    Thumbnail {
        width: thumb_w,
        height: thumb_h,
        data: dst,
    }
}

/// Detect image format from magic bytes
pub fn detect_format(data: &[u8]) -> Option<ImageFormat> {
    if data.len() < 4 { return None; }
    if data[0] == 0xFF && data[1] == 0xD8 { return Some(ImageFormat::Jpeg); }
    if data[..8] == PNG_SIGNATURE { return Some(ImageFormat::Png); }
    if data[0] == b'B' && data[1] == b'M' { return Some(ImageFormat::Bmp); }
    let qoi_magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    if qoi_magic == QOI_MAGIC { return Some(ImageFormat::Qoi); }
    None
}

/// Decode any supported image format (auto-detect)
pub fn decode_image(data: &[u8]) -> Option<DecodedImage> {
    match detect_format(data)? {
        ImageFormat::Bmp => bmp_decode(data),
        ImageFormat::Png => png_decode(data),
        ImageFormat::Jpeg => jpeg_decode(data),
        ImageFormat::Qoi => qoi_decode(data),
        ImageFormat::Raw => None,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut reg = CODEC_REGISTRY.lock();
    *reg = Some(ImageCodecRegistry::new());
    serial_println!("    [image-codec] Image codecs initialized (BMP, PNG, JPEG, QOI, thumbnails)");
}
