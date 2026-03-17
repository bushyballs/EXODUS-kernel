// compositor/layer.rs - Layer management for the compositor

use crate::compositor::{
    buffer_queue::{BufferQueue, GraphicsBuffer},
    types::{Rect, BlendMode, Transform, PixelFormat, BufferUsage, DirtyRegion, Color},
};

/// Layer flags
#[derive(Debug, Clone, Copy)]
pub struct LayerFlags {
    pub hidden: bool,
    pub opaque: bool,
    pub secure: bool,
    pub cursor: bool,
}

impl LayerFlags {
    pub const fn default() -> Self {
        Self {
            hidden: false,
            opaque: false,
            secure: false,
            cursor: false,
        }
    }
}

/// A single compositable layer
pub struct Layer {
    pub id: usize,
    pub name: [u8; 64],
    pub name_len: usize,

    // Geometry
    pub bounds: Rect,
    pub display_frame: Rect,
    pub source_crop: Rect,
    pub z_order: i32,

    // Visual properties
    pub alpha: f32,
    pub color: Color,
    pub blend_mode: BlendMode,
    pub transform: Transform,
    pub flags: LayerFlags,

    // Buffer management
    buffer_queue: BufferQueue,
    current_buffer: Option<usize>,
    previous_buffer: Option<usize>,

    // Dirty tracking
    dirty: bool,
    damage_region: DirtyRegion,
}

impl Layer {
    /// Create a new layer
    pub fn new(name: &str, width: u32, height: u32) -> Self {
        let mut name_bytes = [0u8; 64];
        let name_len = name.len().min(64);
        name_bytes[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);

        let bounds = Rect::new(0, 0, width, height);
        let buffer_queue = BufferQueue::new(
            width,
            height,
            PixelFormat::RGBA8888,
            BufferUsage::default(),
        );

        Self {
            id: 0, // Will be set by LayerStack
            name: name_bytes,
            name_len,
            bounds,
            display_frame: bounds,
            source_crop: bounds,
            z_order: 0,
            alpha: 1.0,
            color: Color::transparent(),
            blend_mode: BlendMode::Premultiplied,
            transform: Transform::IDENTITY,
            flags: LayerFlags::default(),
            buffer_queue,
            current_buffer: None,
            previous_buffer: None,
            dirty: true,
            damage_region: DirtyRegion::new(),
        }
    }

    /// Get layer name as string
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("invalid")
    }

    /// Set layer position
    pub fn set_position(&mut self, x: i32, y: i32) {
        if self.bounds.x != x || self.bounds.y != y {
            self.bounds.x = x;
            self.bounds.y = y;
            self.display_frame.x = x;
            self.display_frame.y = y;
            self.mark_dirty();
        }
    }

    /// Set layer alpha
    pub fn set_alpha(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        if (self.alpha - alpha).abs() > 0.001 {
            self.alpha = alpha;
            self.mark_dirty();
        }
    }

    /// Set layer color (for solid color layers)
    pub fn set_color(&mut self, color: Color) {
        if self.color != color {
            self.color = color;
            self.mark_dirty();
        }
    }

    /// Set blend mode
    pub fn set_blend_mode(&mut self, mode: BlendMode) {
        if self.blend_mode != mode {
            self.blend_mode = mode;
            self.mark_dirty();
        }
    }

    /// Set transform
    pub fn set_transform(&mut self, transform: Transform) {
        if self.transform != transform {
            self.transform = transform;
            self.mark_dirty();
        }
    }

    /// Set visibility
    pub fn set_visible(&mut self, visible: bool) {
        if self.flags.hidden == visible {
            self.flags.hidden = !visible;
            self.mark_dirty();
        }
    }

    /// Mark layer as dirty
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
        self.damage_region.add(self.display_frame);
    }

    /// Get buffer queue
    pub fn get_buffer_queue_mut(&mut self) -> &mut BufferQueue {
        &mut self.buffer_queue
    }

    /// Latch the next available buffer
    pub fn latch_buffer(&mut self) -> bool {
        match self.buffer_queue.acquire_buffer() {
            Ok((slot_idx, _buffer)) => {
                // Release previous buffer if we have one
                if let Some(prev) = self.previous_buffer.take() {
                    let _ = self.buffer_queue.release_buffer(prev);
                }

                self.previous_buffer = self.current_buffer.take();
                self.current_buffer = Some(slot_idx);
                self.mark_dirty();
                true
            }
            Err(_) => false,
        }
    }

    /// Release current buffer
    pub fn release_buffer(&mut self) {
        if let Some(slot) = self.previous_buffer.take() {
            let _ = self.buffer_queue.release_buffer(slot);
        }
    }

    /// Get the slot index of the currently acquired buffer (for release).
    pub fn get_current_buffer(&self) -> Option<usize> {
        self.current_buffer
    }

    /// Borrow the current `GraphicsBuffer` for read-only pixel sampling.
    ///
    /// Returns `None` when no buffer has been latched yet.
    pub fn current_buffer_ref(&self) -> Option<&crate::compositor::buffer_queue::GraphicsBuffer> {
        let slot_idx = self.current_buffer?;
        // BufferQueue stores slots as an array; we need read access.
        // Delegate through the buffer_queue helper.
        self.buffer_queue.slot_buffer(slot_idx)
    }

    /// Check if layer is visible
    pub fn is_visible(&self) -> bool {
        !self.flags.hidden && self.alpha > 0.0
    }

    /// Get damage region
    pub fn damage_region(&self) -> &DirtyRegion {
        &self.damage_region
    }

    /// Clear damage region
    pub fn clear_damage(&mut self) {
        self.dirty = false;
        self.damage_region.clear();
    }
}

/// Stack of layers for composition
pub struct LayerStack {
    layers: [Option<Layer>; 64],
    count: usize,
    next_id: usize,
}

impl LayerStack {
    /// Create a new layer stack
    pub fn new() -> Self {
        const INIT: Option<Layer> = None;
        Self {
            layers: [INIT; 64],
            count: 0,
            next_id: 1,
        }
    }

    /// Add a layer to the stack
    pub fn add_layer(&mut self, mut layer: Layer) -> usize {
        if self.count >= 64 {
            return 0;
        }

        layer.id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let id = layer.id;
        self.layers[self.count] = Some(layer);
        self.count = self.count.saturating_add(1);

        // Sort by z-order
        self.sort_layers();

        id
    }

    /// Remove a layer from the stack
    pub fn remove_layer(&mut self, layer_id: usize) {
        for i in 0..self.count {
            if let Some(ref layer) = self.layers[i] {
                if layer.id == layer_id {
                    // Shift remaining layers down
                    for j in i..self.count - 1 {
                        self.layers[j] = self.layers[j + 1].take();
                    }
                    self.layers[self.count - 1] = None;
                    self.count = self.count.saturating_sub(1);
                    return;
                }
            }
        }
    }

    /// Get a layer by ID
    pub fn get_layer_mut(&mut self, layer_id: usize) -> Option<&mut Layer> {
        for i in 0..self.count {
            if let Some(ref mut layer) = self.layers[i] {
                if layer.id == layer_id {
                    return Some(layer);
                }
            }
        }
        None
    }

    /// Set layer z-order
    pub fn set_z_order(&mut self, layer_id: usize, z: i32) {
        if let Some(layer) = self.get_layer_mut(layer_id) {
            layer.z_order = z;
            self.sort_layers();
        }
    }

    /// Sort layers by z-order (bubble sort for simplicity)
    fn sort_layers(&mut self) {
        for i in 0..self.count {
            for j in 0..self.count - 1 - i {
                let swap = if let (Some(ref a), Some(ref b)) = (&self.layers[j], &self.layers[j + 1]) {
                    a.z_order > b.z_order
                } else {
                    false
                };

                if swap {
                    self.layers.swap(j, j + 1);
                }
            }
        }
    }

    /// Latch buffers from all layers
    pub fn latch_buffers(&mut self) {
        for i in 0..self.count {
            if let Some(ref mut layer) = self.layers[i] {
                layer.latch_buffer();
            }
        }
    }

    /// Release buffers from all layers
    pub fn release_buffers(&mut self) {
        for i in 0..self.count {
            if let Some(ref mut layer) = self.layers[i] {
                layer.release_buffer();
            }
        }
    }

    /// Compute dirty regions for all layers
    pub fn compute_dirty_regions(&self) -> DirtyRegion {
        let mut combined = DirtyRegion::new();

        for i in 0..self.count {
            if let Some(ref layer) = self.layers[i] {
                if layer.dirty {
                    let damage = layer.damage_region();
                    for rect in damage.rects() {
                        if let Some(r) = rect {
                            combined.add(*r);
                        }
                    }
                }
            }
        }

        combined
    }

    /// Get iterator over visible layers (bottom to top)
    pub fn iter_visible(&self) -> impl Iterator<Item = &Layer> {
        self.layers[..self.count]
            .iter()
            .filter_map(|opt| opt.as_ref())
            .filter(|layer| layer.is_visible())
    }

    /// Get mutable iterator over visible layers
    pub fn iter_visible_mut(&mut self) -> impl Iterator<Item = &mut Layer> {
        self.layers[..self.count]
            .iter_mut()
            .filter_map(|opt| opt.as_mut())
            .filter(|layer| layer.is_visible())
    }

    /// Get layer count
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}
