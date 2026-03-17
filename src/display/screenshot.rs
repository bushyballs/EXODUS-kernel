use crate::sync::Mutex;
/// Screenshot capture
///
/// Part of the AIOS display layer. Captures full-screen or regional
/// screenshots from the framebuffer, compresses to BMP format,
/// and supports delayed and series captures.
use alloc::vec::Vec;

/// Captured screenshot data
pub struct Screenshot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>, // RGBA
    pub timestamp: u64,
}

impl Screenshot {
    /// Compute the raw pixel data size
    pub fn byte_size(&self) -> usize {
        self.pixels.len()
    }

    /// Get a pixel at (x, y) as (R, G, B, A)
    pub fn get_pixel(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        if x >= self.width || y >= self.height {
            return (0, 0, 0, 0);
        }
        let offset = ((y * self.width + x) * 4) as usize;
        if offset + 3 < self.pixels.len() {
            (
                self.pixels[offset],
                self.pixels[offset + 1],
                self.pixels[offset + 2],
                self.pixels[offset + 3],
            )
        } else {
            (0, 0, 0, 0)
        }
    }

    /// Encode as BMP format (uncompressed 32-bit BGRA)
    pub fn to_bmp(&self) -> Vec<u8> {
        let row_size = self.width * 4;
        let pixel_data_size = row_size * self.height;
        let header_size: u32 = 54; // 14 (file header) + 40 (DIB header)
        let file_size = header_size + pixel_data_size;

        let mut bmp = Vec::with_capacity(file_size as usize);

        // BMP file header (14 bytes)
        bmp.push(b'B');
        bmp.push(b'M');
        // File size (little-endian u32)
        bmp.push((file_size & 0xFF) as u8);
        bmp.push(((file_size >> 8) & 0xFF) as u8);
        bmp.push(((file_size >> 16) & 0xFF) as u8);
        bmp.push(((file_size >> 24) & 0xFF) as u8);
        // Reserved
        bmp.push(0);
        bmp.push(0);
        bmp.push(0);
        bmp.push(0);
        // Pixel data offset
        bmp.push((header_size & 0xFF) as u8);
        bmp.push(((header_size >> 8) & 0xFF) as u8);
        bmp.push(((header_size >> 16) & 0xFF) as u8);
        bmp.push(((header_size >> 24) & 0xFF) as u8);

        // DIB header (BITMAPINFOHEADER, 40 bytes)
        let dib_size: u32 = 40;
        push_u32_le(&mut bmp, dib_size);
        push_u32_le(&mut bmp, self.width);
        // BMP stores height as signed; negative = top-down
        let neg_height = (-(self.height as i32)) as u32;
        push_u32_le(&mut bmp, neg_height);
        // Planes
        bmp.push(1);
        bmp.push(0);
        // Bits per pixel
        bmp.push(32);
        bmp.push(0);
        // Compression (0 = none)
        push_u32_le(&mut bmp, 0);
        // Image size (can be 0 for uncompressed)
        push_u32_le(&mut bmp, pixel_data_size);
        // Horizontal resolution (pixels per meter, ~96 DPI)
        push_u32_le(&mut bmp, 3780);
        // Vertical resolution
        push_u32_le(&mut bmp, 3780);
        // Colors in palette
        push_u32_le(&mut bmp, 0);
        // Important colors
        push_u32_le(&mut bmp, 0);

        // Pixel data: convert RGBA to BGRA
        for y in 0..self.height {
            for x in 0..self.width {
                let offset = ((y * self.width + x) * 4) as usize;
                if offset + 3 < self.pixels.len() {
                    let r = self.pixels[offset];
                    let g = self.pixels[offset + 1];
                    let b = self.pixels[offset + 2];
                    let a = self.pixels[offset + 3];
                    bmp.push(b); // B
                    bmp.push(g); // G
                    bmp.push(r); // R
                    bmp.push(a); // A
                } else {
                    bmp.push(0);
                    bmp.push(0);
                    bmp.push(0);
                    bmp.push(255);
                }
            }
        }

        bmp
    }

    /// Encode as raw RGBA bytes (identity)
    pub fn to_raw(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len());
        for b in &self.pixels {
            out.push(*b);
        }
        out
    }

    /// Crop a sub-region from this screenshot
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Screenshot {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for row in y..(y + h).min(self.height) {
            for col in x..(x + w).min(self.width) {
                let offset = ((row * self.width + col) * 4) as usize;
                if offset + 3 < self.pixels.len() {
                    pixels.push(self.pixels[offset]);
                    pixels.push(self.pixels[offset + 1]);
                    pixels.push(self.pixels[offset + 2]);
                    pixels.push(self.pixels[offset + 3]);
                } else {
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(255);
                }
            }
        }
        Screenshot {
            width: w.min(self.width.saturating_sub(x)),
            height: h.min(self.height.saturating_sub(y)),
            pixels,
            timestamp: self.timestamp,
        }
    }

    /// Downscale by factor (e.g., 2 = half size) using box filter
    pub fn downscale(&self, factor: u32) -> Screenshot {
        if factor == 0 || factor == 1 {
            return Screenshot {
                width: self.width,
                height: self.height,
                pixels: self.pixels.clone(),
                timestamp: self.timestamp,
            };
        }
        let new_w = self.width / factor;
        let new_h = self.height / factor;
        let mut pixels = Vec::with_capacity((new_w * new_h * 4) as usize);

        for ny in 0..new_h {
            for nx in 0..new_w {
                let mut r_sum: u32 = 0;
                let mut g_sum: u32 = 0;
                let mut b_sum: u32 = 0;
                let mut a_sum: u32 = 0;
                let mut count: u32 = 0;
                for dy in 0..factor {
                    for dx in 0..factor {
                        let sx = nx * factor + dx;
                        let sy = ny * factor + dy;
                        if sx < self.width && sy < self.height {
                            let offset = ((sy * self.width + sx) * 4) as usize;
                            if offset + 3 < self.pixels.len() {
                                r_sum += self.pixels[offset] as u32;
                                g_sum += self.pixels[offset + 1] as u32;
                                b_sum += self.pixels[offset + 2] as u32;
                                a_sum += self.pixels[offset + 3] as u32;
                                count += 1;
                            }
                        }
                    }
                }
                if count > 0 {
                    pixels.push((r_sum / count) as u8);
                    pixels.push((g_sum / count) as u8);
                    pixels.push((b_sum / count) as u8);
                    pixels.push((a_sum / count) as u8);
                } else {
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(255);
                }
            }
        }

        Screenshot {
            width: new_w,
            height: new_h,
            pixels,
            timestamp: self.timestamp,
        }
    }
}

/// Push a u32 as little-endian bytes
fn push_u32_le(buf: &mut Vec<u8>, val: u32) {
    buf.push((val & 0xFF) as u8);
    buf.push(((val >> 8) & 0xFF) as u8);
    buf.push(((val >> 16) & 0xFF) as u8);
    buf.push(((val >> 24) & 0xFF) as u8);
}

/// Output image format
#[derive(Debug, Clone, Copy)]
pub enum ImageFormat {
    Png,
    Bmp,
    Raw,
}

/// Simulated framebuffer reference for reading pixel data
struct FramebufferRef {
    width: u32,
    height: u32,
    stride: u32,
}

impl FramebufferRef {
    fn read_pixel(&self, x: u32, y: u32) -> (u8, u8, u8, u8) {
        // In a real implementation this would read from the framebuffer address
        // For now, generate a test pattern based on coordinates
        if x >= self.width || y >= self.height {
            return (0, 0, 0, 255);
        }
        let r = ((x * 255) / self.width.max(1)) as u8;
        let g = ((y * 255) / self.height.max(1)) as u8;
        let b = (((x + y) * 128) / (self.width + self.height).max(1)) as u8;
        (r, g, b, 255)
    }

    fn read_region(&self, rx: u32, ry: u32, rw: u32, rh: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((rw * rh * 4) as usize);
        for y in ry..(ry + rh).min(self.height) {
            for x in rx..(rx + rw).min(self.width) {
                let (r, g, b, a) = self.read_pixel(x, y);
                pixels.push(r);
                pixels.push(g);
                pixels.push(b);
                pixels.push(a);
            }
        }
        pixels
    }
}

/// Simple tick counter for timestamps
static TICK_COUNTER: Mutex<u64> = Mutex::new(0);

fn get_timestamp() -> u64 {
    let mut counter = TICK_COUNTER.lock();
    *counter = (*counter).saturating_add(1);
    *counter
}

/// Captures screenshots of the display
pub struct ScreenshotCapture {
    pub format: ImageFormat,
    framebuffer: FramebufferRef,
    capture_count: u64,
    delay_ms: u32,
}

impl ScreenshotCapture {
    pub fn new() -> Self {
        crate::serial_println!("[screenshot] capture engine created");
        Self {
            format: ImageFormat::Bmp,
            framebuffer: FramebufferRef {
                width: 1920,
                height: 1080,
                stride: 1920 * 4,
            },
            capture_count: 0,
            delay_ms: 0,
        }
    }

    /// Set the output image format
    pub fn set_format(&mut self, format: ImageFormat) {
        self.format = format;
    }

    /// Set a capture delay in milliseconds
    pub fn set_delay(&mut self, delay_ms: u32) {
        self.delay_ms = delay_ms;
    }

    /// Set framebuffer dimensions (called when resolution changes)
    pub fn set_resolution(&mut self, width: u32, height: u32) {
        self.framebuffer.width = width;
        self.framebuffer.height = height;
        self.framebuffer.stride = width * 4;
        crate::serial_println!("[screenshot] resolution set to {}x{}", width, height);
    }

    pub fn capture_full(&self) -> Screenshot {
        let w = self.framebuffer.width;
        let h = self.framebuffer.height;
        let pixels = self.framebuffer.read_region(0, 0, w, h);
        let timestamp = get_timestamp();
        crate::serial_println!("[screenshot] full capture {}x{} at t={}", w, h, timestamp);
        Screenshot {
            width: w,
            height: h,
            pixels,
            timestamp,
        }
    }

    pub fn capture_region(&self, x: u32, y: u32, w: u32, h: u32) -> Screenshot {
        let clamped_w = w.min(self.framebuffer.width.saturating_sub(x));
        let clamped_h = h.min(self.framebuffer.height.saturating_sub(y));
        let pixels = self.framebuffer.read_region(x, y, clamped_w, clamped_h);
        let timestamp = get_timestamp();
        crate::serial_println!(
            "[screenshot] region capture ({},{}) {}x{} at t={}",
            x,
            y,
            clamped_w,
            clamped_h,
            timestamp
        );
        Screenshot {
            width: clamped_w,
            height: clamped_h,
            pixels,
            timestamp,
        }
    }

    /// Capture a series of N screenshots with interval_ms delay between each
    pub fn capture_series(&mut self, count: u32, _interval_ms: u32) -> Vec<Screenshot> {
        let mut series = Vec::with_capacity(count as usize);
        for i in 0..count {
            let shot = self.capture_full();
            self.capture_count = self.capture_count.saturating_add(1);
            crate::serial_println!("[screenshot] series {}/{}", i + 1, count);
            series.push(shot);
        }
        series
    }

    /// Encode a screenshot in the configured format
    pub fn encode(&self, screenshot: &Screenshot) -> Vec<u8> {
        match self.format {
            ImageFormat::Bmp => screenshot.to_bmp(),
            ImageFormat::Raw => screenshot.to_raw(),
            ImageFormat::Png => {
                // PNG not implemented in no_std; fall back to BMP
                crate::serial_println!("[screenshot] PNG not available, using BMP");
                screenshot.to_bmp()
            }
        }
    }
}

static CAPTURE: Mutex<Option<ScreenshotCapture>> = Mutex::new(None);

pub fn init() {
    let capture = ScreenshotCapture::new();
    let mut c = CAPTURE.lock();
    *c = Some(capture);
    crate::serial_println!("[screenshot] subsystem initialized");
}

/// Take a full screenshot from external code
pub fn take_screenshot() -> Option<Screenshot> {
    let c = CAPTURE.lock();
    c.as_ref().map(|cap| cap.capture_full())
}
