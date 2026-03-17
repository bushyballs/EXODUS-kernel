// compositor/mod.rs - SurfaceFlinger-equivalent compositor for Genesis OS
// Main compositor module exposing the public API

pub mod buffer_queue;
pub mod layer;
pub mod composition;
pub mod hwc;
pub mod vsync;
pub mod display;
pub mod surface;
pub mod types;

#[cfg(test)]
mod tests;

use crate::compositor::{
    buffer_queue::BufferQueue,
    layer::{Layer, LayerStack},
    composition::Compositor,
    hwc::HardwareComposer,
    vsync::VSyncManager,
    display::DisplayManager,
};

/// Main SurfaceFlinger entry point
pub struct SurfaceFlinger {
    display_manager: DisplayManager,
    layer_stack: LayerStack,
    compositor: Compositor,
    hwc: HardwareComposer,
    vsync_manager: VSyncManager,
    running: bool,
}

impl SurfaceFlinger {
    /// Initialize the SurfaceFlinger compositor
    pub fn new() -> Self {
        let display_manager = DisplayManager::new();
        let layer_stack = LayerStack::new();
        let hwc = HardwareComposer::new();
        let compositor = Compositor::new();
        let vsync_manager = VSyncManager::new();

        Self {
            display_manager,
            layer_stack,
            compositor,
            hwc,
            vsync_manager,
            running: false,
        }
    }

    /// Start the compositor event loop
    pub fn run(&mut self) {
        self.running = true;
        log::info!("SurfaceFlinger: Starting compositor");

        // Initialize displays
        self.display_manager.init();

        // Start VSYNC
        self.vsync_manager.start();

        // Main composition loop
        while self.running {
            self.vsync_manager.wait_for_vsync();
            self.compose_frame();
        }
    }

    /// Compose a single frame
    fn compose_frame(&mut self) {
        // 1. Latch buffers from all layers
        self.layer_stack.latch_buffers();

        // 2. Calculate dirty regions
        let dirty_regions = self.layer_stack.compute_dirty_regions();

        // 3. Try hardware composition first
        let hwc_result = self.hwc.prepare(&self.layer_stack, &dirty_regions);

        // 4. Compose using HWC or fallback to software
        match hwc_result {
            Ok(composition_plan) => {
                // Hardware can handle it
                self.hwc.commit(composition_plan);
            }
            Err(_) => {
                // Fallback to software composition
                let framebuffer = self.compositor.compose(&self.layer_stack, &dirty_regions);
                self.display_manager.present(framebuffer);
            }
        }

        // 5. Signal completion and release buffers
        self.layer_stack.release_buffers();
    }

    /// Stop the compositor
    pub fn stop(&mut self) {
        self.running = false;
        self.vsync_manager.stop();
        log::info!("SurfaceFlinger: Stopped");
    }

    /// Create a new surface for a client
    pub fn create_surface(&mut self, name: &str, width: u32, height: u32) -> usize {
        let layer = Layer::new(name, width, height);
        self.layer_stack.add_layer(layer)
    }

    /// Remove a surface
    pub fn remove_surface(&mut self, layer_id: usize) {
        self.layer_stack.remove_layer(layer_id);
    }

    /// Set layer Z-order
    pub fn set_layer_z_order(&mut self, layer_id: usize, z: i32) {
        self.layer_stack.set_z_order(layer_id, z);
    }

    /// Set layer position
    pub fn set_layer_position(&mut self, layer_id: usize, x: i32, y: i32) {
        if let Some(layer) = self.layer_stack.get_layer_mut(layer_id) {
            layer.set_position(x, y);
        }
    }

    /// Set layer alpha
    pub fn set_layer_alpha(&mut self, layer_id: usize, alpha: f32) {
        if let Some(layer) = self.layer_stack.get_layer_mut(layer_id) {
            layer.set_alpha(alpha);
        }
    }

    /// Get buffer queue for a layer
    pub fn get_buffer_queue(&mut self, layer_id: usize) -> Option<&mut BufferQueue> {
        self.layer_stack.get_layer_mut(layer_id)
            .map(|layer| layer.get_buffer_queue_mut())
    }
}
