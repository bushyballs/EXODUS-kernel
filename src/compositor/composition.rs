// compositor/composition.rs - Software compositor implementation

use crate::compositor::{
    layer::LayerStack,
    types::{Color, Rect, DirtyRegion, PixelFormat, BlendMode},
    buffer_queue::GraphicsBuffer,
};

/// Framebuffer for composition
///
/// Pixel data is owned by a heap-allocated `Vec<u8>`.  `data` is a raw
/// pointer into that Vec and remains valid for the lifetime of the struct.
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: *mut u8,
    pub size: usize,
    // Backing heap allocation — keeps `data` valid.
    _backing: alloc::vec::Vec<u8>,
}

impl Framebuffer {
    /// Create a new framebuffer backed by zeroed heap memory.
    ///
    /// If width or height is zero the framebuffer is created with a null data
    /// pointer and size 0 (all pixel ops are guarded by the `is_null` checks
    /// that already exist throughout this file).
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let bpp = format.bytes_per_pixel() as u32;
        let stride = width.saturating_mul(bpp);
        let size = (stride as usize).saturating_mul(height as usize);

        if size == 0 {
            return Self {
                width, height, stride, format,
                data: core::ptr::null_mut(),
                size: 0,
                _backing: alloc::vec::Vec::new(),
            };
        }

        // Zeroed pixel storage (transparent black).
        let mut backing = alloc::vec![0u8; size];
        let data = backing.as_mut_ptr();

        Self {
            width,
            height,
            stride,
            format,
            data,
            size,
            _backing: backing,
        }
    }

    /// Clear framebuffer to a color
    pub fn clear(&mut self, color: Color) {
        if self.data.is_null() {
            return;
        }

        unsafe {
            let slice = core::slice::from_raw_parts_mut(self.data, self.size);
            match self.format {
                PixelFormat::RGBA8888 | PixelFormat::BGRA8888 => {
                    for chunk in slice.chunks_exact_mut(4) {
                        chunk[0] = color.r;
                        chunk[1] = color.g;
                        chunk[2] = color.b;
                        chunk[3] = color.a;
                    }
                }
                PixelFormat::RGBX8888 => {
                    for chunk in slice.chunks_exact_mut(4) {
                        chunk[0] = color.r;
                        chunk[1] = color.g;
                        chunk[2] = color.b;
                        chunk[3] = 0xff;
                    }
                }
                PixelFormat::RGB888 => {
                    for chunk in slice.chunks_exact_mut(3) {
                        chunk[0] = color.r;
                        chunk[1] = color.g;
                        chunk[2] = color.b;
                    }
                }
                PixelFormat::RGB565 => {
                    let rgb565 = ((color.r as u16 & 0xF8) << 8) |
                                 ((color.g as u16 & 0xFC) << 3) |
                                 ((color.b as u16 & 0xF8) >> 3);
                    for chunk in slice.chunks_exact_mut(2) {
                        chunk[0] = (rgb565 & 0xFF) as u8;
                        chunk[1] = ((rgb565 >> 8) & 0xFF) as u8;
                    }
                }
            }
        }
    }

    /// Get pixel at position
    pub unsafe fn get_pixel(&self, x: u32, y: u32) -> Color {
        if x >= self.width || y >= self.height || self.data.is_null() {
            return Color::transparent();
        }

        let offset = (y * self.stride + x * self.format.bytes_per_pixel() as u32) as usize;
        let slice = core::slice::from_raw_parts(self.data, self.size);

        match self.format {
            PixelFormat::RGBA8888 => {
                Color::new(slice[offset], slice[offset + 1], slice[offset + 2], slice[offset + 3])
            }
            PixelFormat::BGRA8888 => {
                Color::new(slice[offset + 2], slice[offset + 1], slice[offset], slice[offset + 3])
            }
            PixelFormat::RGBX8888 => {
                Color::rgb(slice[offset], slice[offset + 1], slice[offset + 2])
            }
            PixelFormat::RGB888 => {
                Color::rgb(slice[offset], slice[offset + 1], slice[offset + 2])
            }
            PixelFormat::RGB565 => {
                let val = slice[offset] as u16 | ((slice[offset + 1] as u16) << 8);
                let r = ((val >> 11) & 0x1F) as u8;
                let g = ((val >> 5) & 0x3F) as u8;
                let b = (val & 0x1F) as u8;
                Color::rgb((r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2))
            }
        }
    }

    /// Set pixel at position
    pub unsafe fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        if x >= self.width || y >= self.height || self.data.is_null() {
            return;
        }

        let offset = (y * self.stride + x * self.format.bytes_per_pixel() as u32) as usize;
        let slice = core::slice::from_raw_parts_mut(self.data, self.size);

        match self.format {
            PixelFormat::RGBA8888 => {
                slice[offset] = color.r;
                slice[offset + 1] = color.g;
                slice[offset + 2] = color.b;
                slice[offset + 3] = color.a;
            }
            PixelFormat::BGRA8888 => {
                slice[offset] = color.b;
                slice[offset + 1] = color.g;
                slice[offset + 2] = color.r;
                slice[offset + 3] = color.a;
            }
            PixelFormat::RGBX8888 => {
                slice[offset] = color.r;
                slice[offset + 1] = color.g;
                slice[offset + 2] = color.b;
                slice[offset + 3] = 0xff;
            }
            PixelFormat::RGB888 => {
                slice[offset] = color.r;
                slice[offset + 1] = color.g;
                slice[offset + 2] = color.b;
            }
            PixelFormat::RGB565 => {
                let rgb565 = ((color.r as u16 & 0xF8) << 8) |
                             ((color.g as u16 & 0xFC) << 3) |
                             ((color.b as u16 & 0xF8) >> 3);
                slice[offset] = (rgb565 & 0xFF) as u8;
                slice[offset + 1] = ((rgb565 >> 8) & 0xFF) as u8;
            }
        }
    }
}

/// Software compositor
pub struct Compositor {
    framebuffer: Option<Framebuffer>,
}

impl Compositor {
    /// Create a new compositor
    pub fn new() -> Self {
        Self {
            framebuffer: None,
        }
    }

    /// Initialize framebuffer
    pub fn init_framebuffer(&mut self, width: u32, height: u32, format: PixelFormat) {
        self.framebuffer = Some(Framebuffer::new(width, height, format));
    }

    /// Compose all layers into framebuffer
    pub fn compose(&mut self, layer_stack: &LayerStack, dirty_regions: &DirtyRegion) -> &Framebuffer {
        let fb = self.framebuffer.as_mut().expect("Framebuffer not initialized");

        // If we have dirty regions, only update those areas
        // Otherwise, do a full composition
        if dirty_regions.is_empty() {
            return fb;
        }

        // For each dirty region
        for dirty_rect in dirty_regions.rects() {
            if let Some(rect) = dirty_rect {
                self.compose_region(fb, layer_stack, rect);
            }
        }

        self.framebuffer.as_ref().unwrap()
    }

    /// Compose a specific region
    fn compose_region(&self, fb: &mut Framebuffer, layer_stack: &LayerStack, region: &Rect) {
        // Start with background color
        let bg_color = Color::rgb(0, 0, 0);

        // Iterate through all pixels in the region
        for y in region.y..region.y + region.height as i32 {
            for x in region.x..region.x + region.width as i32 {
                if x < 0 || y < 0 || x >= fb.width as i32 || y >= fb.height as i32 {
                    continue;
                }

                let mut pixel = bg_color;

                // Composite layers bottom to top.
                // layer.alpha is an f32 in [0.0, 1.0] (legacy field type).
                // Convert to u8 at the boundary so blend_pixel uses integer math.
                for layer in layer_stack.iter_visible() {
                    if let Some(layer_color) = self.sample_layer(layer, x, y) {
                        // Clamp to [0.0, 1.0] then scale to 0-255.
                        let a_clamped = if layer.alpha < 0.0 { 0.0 } else if layer.alpha > 1.0 { 1.0 } else { layer.alpha };
                        let a_u8 = (a_clamped * 255.0 + 0.5) as u8;
                        pixel = self.blend_pixel(layer_color, pixel, layer.blend_mode, a_u8);
                    }
                }

                unsafe {
                    fb.set_pixel(x as u32, y as u32, pixel);
                }
            }
        }
    }

    /// Sample a color from a layer at screen coordinates.
    ///
    /// When the layer has a valid `GraphicsBuffer` the pixel is read directly
    /// from the buffer's heap-backed data.  When no buffer has been latched
    /// (solid-color layer) `layer.color` is returned unchanged.
    ///
    /// All five pixel formats are handled: RGBA8888, BGRA8888, RGBX8888,
    /// RGB888, and RGB565.
    fn sample_layer(&self, layer: &crate::compositor::layer::Layer, x: i32, y: i32) -> Option<Color> {
        // Bounds check: is the screen coordinate inside the layer's display frame?
        if !layer.display_frame.contains_point(x, y) {
            return None;
        }

        // Map screen → layer-local coordinates.
        let local_x = (x - layer.display_frame.x) as u32;
        let local_y = (y - layer.display_frame.y) as u32;

        // Try to read from the layer's current acquired buffer.
        // `current_buffer_ref` returns `Option<&GraphicsBuffer>` — None means
        // no buffer has been latched yet (solid-color or newly-created layer).
        if let Some(buf) = layer.current_buffer_ref() {
            // Guard: null data pointer means the buffer was not yet allocated.
            if buf.data.is_null() {
                return Some(layer.color);
            }
            // Guard: local coordinates must be within buffer dimensions.
            if local_x >= buf.width || local_y >= buf.height {
                return Some(layer.color);
            }

            let bpp = buf.format.bytes_per_pixel() as u32;
            let offset = (local_y.saturating_mul(buf.stride)
                .saturating_add(local_x.saturating_mul(bpp))) as usize;

            // Guard: offset + bpp must fit within the allocation.
            if offset.saturating_add(bpp as usize) > buf.size {
                return Some(layer.color);
            }

            // Safety: buf.data is valid for `buf.size` bytes (heap-allocated
            // Vec<u8> in GraphicsBuffer::allocate), offset is bounds-checked above.
            let color = unsafe {
                let base = buf.data.add(offset);
                match buf.format {
                    PixelFormat::RGBA8888 => {
                        Color::new(*base, *base.add(1), *base.add(2), *base.add(3))
                    }
                    PixelFormat::BGRA8888 => {
                        // Stored B, G, R, A — reorder to RGBA.
                        Color::new(*base.add(2), *base.add(1), *base, *base.add(3))
                    }
                    PixelFormat::RGBX8888 => {
                        // X byte (padding) is discarded; alpha is fully opaque.
                        Color::rgb(*base, *base.add(1), *base.add(2))
                    }
                    PixelFormat::RGB888 => {
                        Color::rgb(*base, *base.add(1), *base.add(2))
                    }
                    PixelFormat::RGB565 => {
                        let lo = *base as u16;
                        let hi = *base.add(1) as u16;
                        let val = lo | (hi << 8);
                        let r5 = ((val >> 11) & 0x1F) as u8;
                        let g6 = ((val >> 5)  & 0x3F) as u8;
                        let b5 = ( val         & 0x1F) as u8;
                        // Expand 5/6-bit channels to full 8-bit range by
                        // replicating the high bits into the low bits.
                        Color::rgb(
                            (r5 << 3) | (r5 >> 2),
                            (g6 << 2) | (g6 >> 4),
                            (b5 << 3) | (b5 >> 2),
                        )
                    }
                }
            };
            return Some(color);
        }

        // No buffer — fall back to the layer's solid fill color.
        Some(layer.color)
    }

    /// Blend two pixels based on blend mode.
    ///
    /// `alpha` is the layer-level opacity as a u8 fraction of 255 (255 = fully opaque).
    /// All arithmetic is integer-only — no floats.
    ///
    /// Porter-Duff SRC_OVER is used as the final compositing step for all
    /// modes after the photographic operation is applied.
    fn blend_pixel(&self, src: Color, dst: Color, mode: BlendMode, alpha: u8) -> Color {
        match mode {
            BlendMode::None => {
                // Fully opaque copy — src replaces dst regardless of alpha.
                src
            }

            BlendMode::Premultiplied => {
                // Source channels are already premultiplied; scale by layer alpha,
                // then SRC_OVER.
                let a = alpha as u16;
                let src_scaled = Color::new(
                    ((src.r as u16 * a) / 255) as u8,
                    ((src.g as u16 * a) / 255) as u8,
                    ((src.b as u16 * a) / 255) as u8,
                    ((src.a as u16 * a) / 255) as u8,
                );
                src_scaled.blend_over(dst)
            }

            BlendMode::Coverage => {
                // Straight-alpha SRC_OVER.
                // combined_alpha = src.a × layer_alpha / 255
                let combined = ((src.a as u16 * alpha as u16) / 255) as u8;
                let src_final = Color::new(src.r, src.g, src.b, combined);
                src_final.blend_over(dst)
            }

            BlendMode::Multiply => {
                // Photographic Multiply: result_channel = (dst × src) / 255
                // Then SRC_OVER with layer alpha.
                let mr = ((dst.r as u16 * src.r as u16) / 255) as u8;
                let mg = ((dst.g as u16 * src.g as u16) / 255) as u8;
                let mb = ((dst.b as u16 * src.b as u16) / 255) as u8;
                // Blend the photographic result toward dst using layer alpha.
                let a = alpha as u16;
                let out_r = ((mr as u16 * a + dst.r as u16 * (255 - a)) / 255) as u8;
                let out_g = ((mg as u16 * a + dst.g as u16 * (255 - a)) / 255) as u8;
                let out_b = ((mb as u16 * a + dst.b as u16 * (255 - a)) / 255) as u8;
                Color::rgb(out_r, out_g, out_b)
            }

            BlendMode::Screen => {
                // Photographic Screen: result = 255 − (255−dst)×(255−src)/255
                let sr = 255u16 - ((255 - dst.r as u16) * (255 - src.r as u16) / 255);
                let sg = 255u16 - ((255 - dst.g as u16) * (255 - src.g as u16) / 255);
                let sb = 255u16 - ((255 - dst.b as u16) * (255 - src.b as u16) / 255);
                let a = alpha as u16;
                let out_r = ((sr * a + dst.r as u16 * (255 - a)) / 255) as u8;
                let out_g = ((sg * a + dst.g as u16 * (255 - a)) / 255) as u8;
                let out_b = ((sb * a + dst.b as u16 * (255 - a)) / 255) as u8;
                Color::rgb(out_r, out_g, out_b)
            }

            BlendMode::Overlay => {
                // Photographic Overlay: Multiply for dark dst, Screen for bright dst.
                // Threshold at 128 (half of 255).
                let overlay_channel = |d: u8, s: u8| -> u8 {
                    if d < 128 {
                        // Multiply branch: 2*d*s / 255
                        ((2 * d as u16 * s as u16) / 255) as u8
                    } else {
                        // Screen branch: 255 − 2*(255−d)*(255−s)/255
                        (255u16 - (2 * (255 - d as u16) * (255 - s as u16) / 255)) as u8
                    }
                };
                let or_ = overlay_channel(dst.r, src.r);
                let og  = overlay_channel(dst.g, src.g);
                let ob  = overlay_channel(dst.b, src.b);
                let a = alpha as u16;
                let out_r = ((or_ as u16 * a + dst.r as u16 * (255 - a)) / 255) as u8;
                let out_g = ((og  as u16 * a + dst.g as u16 * (255 - a)) / 255) as u8;
                let out_b = ((ob  as u16 * a + dst.b as u16 * (255 - a)) / 255) as u8;
                Color::rgb(out_r, out_g, out_b)
            }
        }
    }

    /// Get framebuffer reference
    pub fn framebuffer(&self) -> Option<&Framebuffer> {
        self.framebuffer.as_ref()
    }
}

/// Optimized blitter for common operations
pub struct Blitter;

impl Blitter {
    /// Fast blit from source to destination
    pub fn blit(
        src: &GraphicsBuffer,
        src_rect: &Rect,
        dst: &mut Framebuffer,
        dst_x: i32,
        dst_y: i32,
    ) {
        // Validate buffers
        if src.data.is_null() || dst.data.is_null() {
            return;
        }

        // Clip to destination bounds
        let copy_width = src_rect.width.min(dst.width.saturating_sub(dst_x as u32));
        let copy_height = src_rect.height.min(dst.height.saturating_sub(dst_y as u32));

        if copy_width == 0 || copy_height == 0 {
            return;
        }

        unsafe {
            // Simple scanline copy for matching formats
            if src.format == dst.format {
                let bpp = src.format.bytes_per_pixel();
                for y in 0..copy_height {
                    let src_offset = ((src_rect.y as u32 + y) * src.stride +
                                     src_rect.x as u32 * bpp as u32) as usize;
                    let dst_offset = ((dst_y as u32 + y) * dst.stride +
                                     dst_x as u32 * bpp as u32) as usize;

                    let src_slice = core::slice::from_raw_parts(src.data.add(src_offset), copy_width as usize * bpp);
                    let dst_slice = core::slice::from_raw_parts_mut(dst.data.add(dst_offset), copy_width as usize * bpp);

                    dst_slice.copy_from_slice(src_slice);
                }
            }
        }
    }

    /// Fill a rectangle with a solid color
    pub fn fill_rect(fb: &mut Framebuffer, rect: &Rect, color: Color) {
        if fb.data.is_null() {
            return;
        }

        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                if x >= 0 && y >= 0 && x < fb.width as i32 && y < fb.height as i32 {
                    unsafe {
                        fb.set_pixel(x as u32, y as u32, color);
                    }
                }
            }
        }
    }
}
