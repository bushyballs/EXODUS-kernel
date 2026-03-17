// compositor/surface.rs - Surface API for clients

use crate::compositor::{
    buffer_queue::{BufferQueue, GraphicsBuffer},
    types::{PixelFormat, BufferUsage, Rect, Transform, BlendMode},
};

/// Surface handle for client applications
pub struct Surface {
    pub id: usize,
    layer_id: usize,
    width: u32,
    height: u32,
    buffer_queue: BufferQueue,
}

impl Surface {
    /// Create a new surface
    pub fn new(id: usize, layer_id: usize, width: u32, height: u32) -> Self {
        let buffer_queue = BufferQueue::new(
            width,
            height,
            PixelFormat::RGBA8888,
            BufferUsage::default(),
        );

        Self {
            id,
            layer_id,
            width,
            height,
            buffer_queue,
        }
    }

    /// Dequeue a buffer for rendering
    pub fn dequeue_buffer(&mut self) -> Result<(usize, &mut GraphicsBuffer), &'static str> {
        self.buffer_queue.dequeue_buffer()
    }

    /// Queue a buffer after rendering
    pub fn queue_buffer(&mut self, slot: usize) -> Result<(), &'static str> {
        self.buffer_queue.queue_buffer(slot, 0)
    }

    /// Cancel a dequeued buffer
    pub fn cancel_buffer(&mut self, slot: usize) -> Result<(), &'static str> {
        self.buffer_queue.cancel_buffer(slot)
    }

    /// Resize the surface
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            self.buffer_queue.resize(width, height);
        }
    }

    /// Get surface dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get layer ID
    pub fn layer_id(&self) -> usize {
        self.layer_id
    }
}

/// Surface transaction for atomic updates
pub struct SurfaceTransaction {
    layer_id: usize,
    position: Option<(i32, i32)>,
    size: Option<(u32, u32)>,
    crop: Option<Rect>,
    alpha: Option<f32>,
    transform: Option<Transform>,
    z_order: Option<i32>,
    blend_mode: Option<BlendMode>,
    visible: Option<bool>,
}

impl SurfaceTransaction {
    /// Create a new transaction for a surface
    pub fn new(layer_id: usize) -> Self {
        Self {
            layer_id,
            position: None,
            size: None,
            crop: None,
            alpha: None,
            transform: None,
            z_order: None,
            blend_mode: None,
            visible: None,
        }
    }

    /// Set surface position
    pub fn set_position(mut self, x: i32, y: i32) -> Self {
        self.position = Some((x, y));
        self
    }

    /// Set surface size
    pub fn set_size(mut self, width: u32, height: u32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Set source crop rectangle
    pub fn set_crop(mut self, crop: Rect) -> Self {
        self.crop = Some(crop);
        self
    }

    /// Set surface alpha
    pub fn set_alpha(mut self, alpha: f32) -> Self {
        self.alpha = Some(alpha);
        self
    }

    /// Set surface transform
    pub fn set_transform(mut self, transform: Transform) -> Self {
        self.transform = Some(transform);
        self
    }

    /// Set Z-order
    pub fn set_z_order(mut self, z: i32) -> Self {
        self.z_order = Some(z);
        self
    }

    /// Set blend mode
    pub fn set_blend_mode(mut self, mode: BlendMode) -> Self {
        self.blend_mode = Some(mode);
        self
    }

    /// Set visibility
    pub fn set_visible(mut self, visible: bool) -> Self {
        self.visible = Some(visible);
        self
    }

    /// Apply transaction (in real impl, this would send to SurfaceFlinger)
    pub fn apply(self) {
        // In a real implementation, this would queue the transaction
        // to be applied atomically on the next VSYNC
        log::debug!("Transaction applied for layer {}", self.layer_id);
    }
}

/// Surface client interface
///
/// This is what client applications use to interact with the compositor
pub struct SurfaceClient {
    next_surface_id: usize,
}

impl SurfaceClient {
    /// Create a new surface client
    pub fn new() -> Self {
        Self {
            next_surface_id: 1,
        }
    }

    /// Create a new surface
    pub fn create_surface(&mut self, width: u32, height: u32, name: &str) -> Result<Surface, &'static str> {
        // In real implementation, this would communicate with SurfaceFlinger
        // via IPC to create a layer and get back a surface handle

        let surface_id = self.next_surface_id;
        self.next_surface_id = self.next_surface_id.saturating_add(1);

        // For now, create a surface directly
        let layer_id = surface_id; // In real impl, returned from SurfaceFlinger
        let surface = Surface::new(surface_id, layer_id, width, height);

        log::info!("Created surface '{}' ({}x{})", name, width, height);

        Ok(surface)
    }

    /// Destroy a surface
    pub fn destroy_surface(&mut self, _surface: Surface) {
        // In real implementation, notify SurfaceFlinger to remove layer
        log::info!("Surface destroyed");
    }

    /// Begin a transaction
    pub fn begin_transaction(&self, layer_id: usize) -> SurfaceTransaction {
        SurfaceTransaction::new(layer_id)
    }
}

/// Simple drawing helper for surfaces
pub struct SurfaceCanvas<'a> {
    buffer: &'a mut [u8],
    width: u32,
    height: u32,
    stride: u32,
    format: PixelFormat,
}

impl<'a> SurfaceCanvas<'a> {
    /// Create a canvas from a buffer
    pub fn new(buffer: &'a mut GraphicsBuffer) -> Result<Self, &'static str> {
        let slice = buffer.map()?;

        Ok(Self {
            buffer: slice,
            width: buffer.width,
            height: buffer.height,
            stride: buffer.stride,
            format: buffer.format,
        })
    }

    /// Clear canvas to a color
    pub fn clear(&mut self, color: crate::compositor::types::Color) {
        match self.format {
            PixelFormat::RGBA8888 => {
                for chunk in self.buffer.chunks_exact_mut(4) {
                    chunk[0] = color.r;
                    chunk[1] = color.g;
                    chunk[2] = color.b;
                    chunk[3] = color.a;
                }
            }
            PixelFormat::BGRA8888 => {
                for chunk in self.buffer.chunks_exact_mut(4) {
                    chunk[0] = color.b;
                    chunk[1] = color.g;
                    chunk[2] = color.r;
                    chunk[3] = color.a;
                }
            }
            _ => {}
        }
    }

    /// Draw a pixel
    pub fn draw_pixel(&mut self, x: u32, y: u32, color: crate::compositor::types::Color) {
        if x >= self.width || y >= self.height {
            return;
        }

        let offset = (y * self.stride + x * self.format.bytes_per_pixel() as u32) as usize;

        match self.format {
            PixelFormat::RGBA8888 => {
                self.buffer[offset] = color.r;
                self.buffer[offset + 1] = color.g;
                self.buffer[offset + 2] = color.b;
                self.buffer[offset + 3] = color.a;
            }
            PixelFormat::BGRA8888 => {
                self.buffer[offset] = color.b;
                self.buffer[offset + 1] = color.g;
                self.buffer[offset + 2] = color.r;
                self.buffer[offset + 3] = color.a;
            }
            _ => {}
        }
    }

    /// Draw a filled rectangle
    pub fn fill_rect(&mut self, rect: &Rect, color: crate::compositor::types::Color) {
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                if x >= 0 && y >= 0 {
                    self.draw_pixel(x as u32, y as u32, color);
                }
            }
        }
    }

    /// Draw a line (Bresenham's algorithm)
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: crate::compositor::types::Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;

        let mut x = x0;
        let mut y = y0;

        loop {
            if x >= 0 && y >= 0 {
                self.draw_pixel(x as u32, y as u32, color);
            }

            if x == x1 && y == y1 {
                break;
            }

            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw a circle (Midpoint circle algorithm)
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: crate::compositor::types::Color) {
        let mut x = radius;
        let mut y = 0;
        let mut err = 0;

        while x >= y {
            self.draw_pixel_safe(cx + x, cy + y, color);
            self.draw_pixel_safe(cx + y, cy + x, color);
            self.draw_pixel_safe(cx - y, cy + x, color);
            self.draw_pixel_safe(cx - x, cy + y, color);
            self.draw_pixel_safe(cx - x, cy - y, color);
            self.draw_pixel_safe(cx - y, cy - x, color);
            self.draw_pixel_safe(cx + y, cy - x, color);
            self.draw_pixel_safe(cx + x, cy - y, color);

            y += 1;
            err += 1 + 2 * y;
            if 2 * (err - x) + 1 > 0 {
                x -= 1;
                err += 1 - 2 * x;
            }
        }
    }

    fn draw_pixel_safe(&mut self, x: i32, y: i32, color: crate::compositor::types::Color) {
        if x >= 0 && y >= 0 {
            self.draw_pixel(x as u32, y as u32, color);
        }
    }
}
