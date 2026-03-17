// compositor/hwc.rs - Hardware Composer (HWC) interface

use crate::compositor::{
    layer::LayerStack,
    types::{DirtyRegion, CompositionType, Rect},
};

/// Hardware composer capability flags
#[derive(Debug, Clone, Copy)]
pub struct HwcCapabilities {
    pub max_overlays: usize,
    pub supports_cursor: bool,
    pub supports_rotation: bool,
    pub supports_scaling: bool,
    pub supports_color_transform: bool,
    pub supports_hdr: bool,
}

impl HwcCapabilities {
    pub fn default() -> Self {
        Self {
            max_overlays: 4,
            supports_cursor: true,
            supports_rotation: false,
            supports_scaling: true,
            supports_color_transform: false,
            supports_hdr: false,
        }
    }
}

/// Hardware layer assignment
#[derive(Debug, Clone, Copy)]
pub struct HwcLayer {
    pub layer_id: usize,
    pub composition_type: CompositionType,
    pub overlay_index: Option<usize>,
}

/// Hardware composition plan
pub struct CompositionPlan {
    pub hwc_layers: [Option<HwcLayer>; 64],
    pub layer_count: usize,
    pub overlay_count: usize,
    pub needs_client_composition: bool,
}

impl CompositionPlan {
    pub fn new() -> Self {
        const INIT: Option<HwcLayer> = None;
        Self {
            hwc_layers: [INIT; 64],
            layer_count: 0,
            overlay_count: 0,
            needs_client_composition: false,
        }
    }

    pub fn add_layer(&mut self, layer: HwcLayer) {
        if self.layer_count < 64 {
            self.hwc_layers[self.layer_count] = Some(layer);
            self.layer_count = self.layer_count.saturating_add(1);

            if layer.composition_type == CompositionType::Device {
                self.overlay_count = self.overlay_count.saturating_add(1);
            } else if layer.composition_type == CompositionType::Client {
                self.needs_client_composition = true;
            }
        }
    }
}

/// Hardware Composer
///
/// Determines which layers can be handled by hardware overlays
/// and which need client (GPU/CPU) composition
pub struct HardwareComposer {
    capabilities: HwcCapabilities,
    enabled: bool,
}

impl HardwareComposer {
    /// Create a new hardware composer
    pub fn new() -> Self {
        Self {
            capabilities: HwcCapabilities::default(),
            enabled: true,
        }
    }

    /// Initialize and detect hardware capabilities
    pub fn init(&mut self) -> Result<(), &'static str> {
        // Detect GPU/display hardware
        // Query capabilities
        // Initialize overlay planes

        log::info!("HWC: Initialized with {} overlays", self.capabilities.max_overlays);
        Ok(())
    }

    /// Prepare composition plan for the layer stack
    pub fn prepare(&mut self, layer_stack: &LayerStack, dirty_regions: &DirtyRegion) -> Result<CompositionPlan, &'static str> {
        if !self.enabled {
            return Err("Hardware composer disabled");
        }

        let mut plan = CompositionPlan::new();

        // Count visible layers
        let visible_count = layer_stack.iter_visible().count();

        // If we have too many layers, fall back to client composition
        if visible_count > self.capabilities.max_overlays {
            return Err("Too many layers for hardware composition");
        }

        // Analyze each layer and assign composition strategy
        for layer in layer_stack.iter_visible() {
            let comp_type = self.determine_composition_type(layer);

            let hwc_layer = match comp_type {
                CompositionType::Device => {
                    // Can use hardware overlay
                    let overlay_idx = plan.overlay_count;
                    if overlay_idx < self.capabilities.max_overlays {
                        HwcLayer {
                            layer_id: layer.id,
                            composition_type: CompositionType::Device,
                            overlay_index: Some(overlay_idx),
                        }
                    } else {
                        // No more overlays available
                        HwcLayer {
                            layer_id: layer.id,
                            composition_type: CompositionType::Client,
                            overlay_index: None,
                        }
                    }
                }
                CompositionType::Cursor => {
                    // Use hardware cursor if available
                    HwcLayer {
                        layer_id: layer.id,
                        composition_type: CompositionType::Cursor,
                        overlay_index: None,
                    }
                }
                CompositionType::SolidColor => {
                    // Can be drawn directly
                    HwcLayer {
                        layer_id: layer.id,
                        composition_type: CompositionType::SolidColor,
                        overlay_index: None,
                    }
                }
                CompositionType::Client => {
                    // Needs GPU/CPU composition
                    HwcLayer {
                        layer_id: layer.id,
                        composition_type: CompositionType::Client,
                        overlay_index: None,
                    }
                }
            };

            plan.add_layer(hwc_layer);
        }

        Ok(plan)
    }

    /// Determine the best composition type for a layer
    fn determine_composition_type(&self, layer: &crate::compositor::layer::Layer) -> CompositionType {
        // Cursor layers
        if layer.flags.cursor && self.capabilities.supports_cursor {
            return CompositionType::Cursor;
        }

        // Solid color layers (no buffer)
        if layer.get_current_buffer().is_none() && layer.color.a > 0 {
            return CompositionType::SolidColor;
        }

        // Check if layer can use hardware overlay
        let can_use_hw =
            layer.alpha >= 0.99 && // Nearly opaque
            layer.transform == crate::compositor::types::Transform::IDENTITY && // No rotation
            !layer.flags.secure; // Not a secure layer

        if can_use_hw {
            CompositionType::Device
        } else {
            CompositionType::Client
        }
    }

    /// Commit the composition plan to hardware
    pub fn commit(&mut self, plan: CompositionPlan) -> Result<(), &'static str> {
        if !self.enabled {
            return Err("Hardware composer disabled");
        }

        // Program hardware overlays
        for i in 0..plan.layer_count {
            if let Some(hwc_layer) = plan.hwc_layers[i] {
                match hwc_layer.composition_type {
                    CompositionType::Device => {
                        if let Some(overlay_idx) = hwc_layer.overlay_index {
                            self.program_overlay(overlay_idx, hwc_layer.layer_id)?;
                        }
                    }
                    CompositionType::Cursor => {
                        self.program_cursor(hwc_layer.layer_id)?;
                    }
                    CompositionType::SolidColor => {
                        // Hardware can draw solid color directly
                    }
                    CompositionType::Client => {
                        // Already composited by client
                    }
                }
            }
        }

        // Trigger hardware composition
        self.present()?;

        Ok(())
    }

    /// Program a hardware overlay plane
    fn program_overlay(&mut self, overlay_idx: usize, layer_id: usize) -> Result<(), &'static str> {
        // In real implementation:
        // 1. Get buffer from layer
        // 2. Program DMA controller to scan out buffer
        // 3. Set overlay position, size, format
        // 4. Enable overlay plane

        log::debug!("HWC: Programming overlay {} for layer {}", overlay_idx, layer_id);
        Ok(())
    }

    /// Program hardware cursor
    fn program_cursor(&mut self, layer_id: usize) -> Result<(), &'static str> {
        // In real implementation:
        // 1. Upload cursor image to cursor buffer
        // 2. Set cursor position
        // 3. Enable cursor

        log::debug!("HWC: Programming cursor for layer {}", layer_id);
        Ok(())
    }

    /// Present the frame to display
    fn present(&mut self) -> Result<(), &'static str> {
        // In real implementation:
        // 1. Wait for VSYNC
        // 2. Flip display buffers
        // 3. Signal frame completion

        Ok(())
    }

    /// Enable or disable hardware composition
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            log::info!("HWC: Enabled");
        } else {
            log::info!("HWC: Disabled (falling back to software)");
        }
    }

    /// Get capabilities
    pub fn capabilities(&self) -> &HwcCapabilities {
        &self.capabilities
    }
}

/// Hardware overlay plane
struct OverlayPlane {
    index: usize,
    enabled: bool,
    position: Rect,
    z_order: i32,
    buffer_addr: usize,
    stride: u32,
    format: crate::compositor::types::PixelFormat,
}

impl OverlayPlane {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            enabled: false,
            position: Rect::new(0, 0, 0, 0),
            z_order: 0,
            buffer_addr: 0,
            stride: 0,
            format: crate::compositor::types::PixelFormat::RGBA8888,
        }
    }

    pub fn configure(
        &mut self,
        position: Rect,
        z_order: i32,
        buffer_addr: usize,
        stride: u32,
        format: crate::compositor::types::PixelFormat,
    ) {
        self.position = position;
        self.z_order = z_order;
        self.buffer_addr = buffer_addr;
        self.stride = stride;
        self.format = format;
    }

    pub fn enable(&mut self) {
        self.enabled = true;
        // Program hardware registers
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        // Program hardware registers
    }
}
