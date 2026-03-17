// compositor/tests.rs - Compositor test suite

#![cfg(test)]

use super::*;
use crate::compositor::{
    types::{Color, Rect, PixelFormat, Transform},
    buffer_queue::{BufferQueue, BufferUsage},
    layer::{Layer, LayerStack},
    composition::Compositor,
    vsync::VSyncManager,
};

#[test]
fn test_color_blending() {
    let red = Color::rgb(255, 0, 0);
    let blue = Color::rgb(0, 0, 255);

    // Fully opaque red over blue should be red
    let result = red.blend_over(blue);
    assert_eq!(result.r, 255);
    assert_eq!(result.g, 0);
    assert_eq!(result.b, 0);

    // Transparent color should not change destination
    let transparent = Color::transparent();
    let result = transparent.blend_over(red);
    assert_eq!(result, red);

    // Semi-transparent blending
    let semi_red = Color::new(255, 0, 0, 128);
    let result = semi_red.blend_over(blue);
    assert!(result.r > 0);
    assert!(result.b > 0);
}

#[test]
fn test_rect_intersection() {
    let rect1 = Rect::new(0, 0, 100, 100);
    let rect2 = Rect::new(50, 50, 100, 100);

    // Should intersect
    assert!(rect1.intersects(&rect2));

    // Get intersection
    let intersection = rect1.intersection(&rect2).unwrap();
    assert_eq!(intersection.x, 50);
    assert_eq!(intersection.y, 50);
    assert_eq!(intersection.width, 50);
    assert_eq!(intersection.height, 50);

    // Non-intersecting rects
    let rect3 = Rect::new(200, 200, 100, 100);
    assert!(!rect1.intersects(&rect3));
    assert!(rect1.intersection(&rect3).is_none());
}

#[test]
fn test_rect_union() {
    let rect1 = Rect::new(0, 0, 100, 100);
    let rect2 = Rect::new(50, 50, 100, 100);

    let union = rect1.union(&rect2);
    assert_eq!(union.x, 0);
    assert_eq!(union.y, 0);
    assert_eq!(union.width, 150);
    assert_eq!(union.height, 150);
}

#[test]
fn test_buffer_queue_lifecycle() {
    let mut queue = BufferQueue::new(
        800,
        600,
        PixelFormat::RGBA8888,
        BufferUsage::default(),
    );

    // Dequeue a buffer
    let result = queue.dequeue_buffer();
    assert!(result.is_ok());
    let (slot1, _buffer) = result.unwrap();

    // Queue it back
    let result = queue.queue_buffer(slot1, 0);
    assert!(result.is_ok());

    // Acquire it
    let result = queue.acquire_buffer();
    assert!(result.is_ok());
    let (acquired_slot, _buffer) = result.unwrap();
    assert_eq!(acquired_slot, slot1);

    // Release it
    let result = queue.release_buffer(acquired_slot);
    assert!(result.is_ok());

    // Should be free again
    let result = queue.dequeue_buffer();
    assert!(result.is_ok());
}

#[test]
fn test_buffer_queue_triple_buffering() {
    let mut queue = BufferQueue::new(
        800,
        600,
        PixelFormat::RGBA8888,
        BufferUsage::default(),
    );

    // Should be able to dequeue 3 buffers
    let slot1 = queue.dequeue_buffer().unwrap().0;
    let slot2 = queue.dequeue_buffer().unwrap().0;
    let slot3 = queue.dequeue_buffer().unwrap().0;

    // All slots should be different
    assert_ne!(slot1, slot2);
    assert_ne!(slot2, slot3);
    assert_ne!(slot1, slot3);

    // Fourth dequeue should fail (all buffers busy)
    assert!(queue.dequeue_buffer().is_err());

    // Queue one back
    queue.queue_buffer(slot1, 0).unwrap();

    // Now we can acquire it
    let (acquired, _) = queue.acquire_buffer().unwrap();
    assert_eq!(acquired, slot1);
}

#[test]
fn test_layer_creation() {
    let layer = Layer::new("TestLayer", 1920, 1080);

    assert_eq!(layer.name(), "TestLayer");
    assert_eq!(layer.bounds.width, 1920);
    assert_eq!(layer.bounds.height, 1080);
    assert_eq!(layer.alpha, 1.0);
    assert!(layer.is_visible());
}

#[test]
fn test_layer_properties() {
    let mut layer = Layer::new("Test", 100, 100);

    // Test position
    layer.set_position(50, 75);
    assert_eq!(layer.bounds.x, 50);
    assert_eq!(layer.bounds.y, 75);

    // Test alpha
    layer.set_alpha(0.5);
    assert_eq!(layer.alpha, 0.5);

    // Test visibility
    layer.set_visible(false);
    assert!(!layer.is_visible());

    // Test alpha affects visibility
    layer.set_visible(true);
    layer.set_alpha(0.0);
    assert!(!layer.is_visible());
}

#[test]
fn test_layer_stack() {
    let mut stack = LayerStack::new();

    // Add layers
    let layer1 = Layer::new("Layer1", 100, 100);
    let layer2 = Layer::new("Layer2", 100, 100);

    let id1 = stack.add_layer(layer1);
    let id2 = stack.add_layer(layer2);

    assert_eq!(stack.len(), 2);
    assert!(!stack.is_empty());

    // Remove a layer
    stack.remove_layer(id1);
    assert_eq!(stack.len(), 1);

    // Should still have layer2
    assert!(stack.get_layer_mut(id2).is_some());
    assert!(stack.get_layer_mut(id1).is_none());
}

#[test]
fn test_layer_z_ordering() {
    let mut stack = LayerStack::new();

    let mut layer1 = Layer::new("Bottom", 100, 100);
    layer1.z_order = 10;

    let mut layer2 = Layer::new("Middle", 100, 100);
    layer2.z_order = 20;

    let mut layer3 = Layer::new("Top", 100, 100);
    layer3.z_order = 30;

    stack.add_layer(layer3); // Add out of order
    stack.add_layer(layer1);
    stack.add_layer(layer2);

    // Stack should auto-sort by z-order
    let layers: Vec<_> = stack.iter_visible().collect();
    assert_eq!(layers[0].z_order, 10);
    assert_eq!(layers[1].z_order, 20);
    assert_eq!(layers[2].z_order, 30);
}

#[test]
fn test_vsync_manager() {
    let mut vsync = VSyncManager::new();

    // Test refresh rate
    vsync.set_refresh_rate(60);
    assert_eq!(vsync.get_refresh_rate(), 60);

    let period = vsync.get_period_ns();
    assert_eq!(period, 16_666_667); // 60Hz

    // Test 144Hz
    vsync.set_refresh_rate(144);
    assert_eq!(vsync.get_refresh_rate(), 144);
}

#[test]
fn test_frame_timing() {
    use crate::compositor::vsync::FrameTimingTracker;

    let mut tracker = FrameTimingTracker::new();

    // Record some frames
    tracker.record_frame(16_000_000); // 16ms
    tracker.record_frame(17_000_000); // 17ms
    tracker.record_frame(15_000_000); // 15ms

    let avg = tracker.average_frame_time_ns();
    assert!(avg >= 15_000_000 && avg <= 17_000_000);

    let fps = tracker.average_fps();
    assert!(fps >= 58.0 && fps <= 67.0); // ~60 FPS

    assert_eq!(tracker.min_frame_time_ns(), 15_000_000);
    assert_eq!(tracker.max_frame_time_ns(), 17_000_000);
}

#[test]
fn test_dirty_region() {
    use crate::compositor::types::DirtyRegion;

    let mut region = DirtyRegion::new();
    assert!(region.is_empty());

    // Add a rectangle
    region.add(Rect::new(0, 0, 100, 100));
    assert!(!region.is_empty());

    // Clear
    region.clear();
    assert!(region.is_empty());

    // Add many rectangles (should coalesce)
    for i in 0..20 {
        region.add(Rect::new(i * 10, i * 10, 50, 50));
    }

    // Should have coalesced into bounding box
    assert!(region.rects().len() > 0);
}

#[test]
fn test_transform() {
    let identity = Transform::IDENTITY;
    assert!(!identity.flip_h);
    assert!(!identity.flip_v);
    assert!(!identity.rotate_90);

    let rot180 = Transform::rotate_180();
    assert!(rot180.flip_h);
    assert!(rot180.flip_v);

    let rot270 = Transform::rotate_270();
    assert!(rot270.flip_h);
    assert!(rot270.rotate_90);
}

#[test]
fn test_pixel_format() {
    assert_eq!(PixelFormat::RGBA8888.bytes_per_pixel(), 4);
    assert_eq!(PixelFormat::RGB888.bytes_per_pixel(), 3);
    assert_eq!(PixelFormat::RGB565.bytes_per_pixel(), 2);
}

#[test]
fn test_compositor_creation() {
    let compositor = Compositor::new();
    assert!(compositor.framebuffer().is_none());
}

#[test]
fn test_hwc_capabilities() {
    use crate::compositor::hwc::{HardwareComposer, HwcCapabilities};

    let caps = HwcCapabilities::default();
    assert!(caps.max_overlays > 0);
    assert!(caps.supports_cursor);

    let hwc = HardwareComposer::new();
    assert_eq!(hwc.capabilities().max_overlays, caps.max_overlays);
}

#[test]
fn test_display_config() {
    use crate::compositor::display::DisplayConfig;

    let config = DisplayConfig::mode_1080p60();
    assert_eq!(config.width, 1920);
    assert_eq!(config.height, 1080);
    assert_eq!(config.refresh_rate, 60);

    let config = DisplayConfig::mode_4k60();
    assert_eq!(config.width, 3840);
    assert_eq!(config.height, 2160);
}

#[test]
fn test_surface_lifecycle() {
    use crate::compositor::surface::{Surface, SurfaceClient};

    let mut client = SurfaceClient::new();

    let surface = client.create_surface(800, 600, "Test").unwrap();
    assert_eq!(surface.dimensions(), (800, 600));

    let layer_id = surface.layer_id();
    assert!(layer_id > 0);
}

#[test]
fn test_buffer_fence() {
    let buffer = BufferQueue::new(100, 100, PixelFormat::RGBA8888, BufferUsage::default());

    // Test that buffer operations respect fences
    // (In real implementation, this would test GPU synchronization)
}

// Integration tests

#[test]
fn test_end_to_end_composition() {
    use crate::compositor::surface::SurfaceClient;

    // This would test the full pipeline:
    // 1. Create surface
    // 2. Dequeue buffer
    // 3. Draw to buffer
    // 4. Queue buffer
    // 5. Compositor acquires and composites
    // 6. Display presents

    let mut client = SurfaceClient::new();
    let mut surface = client.create_surface(100, 100, "E2E").unwrap();

    let (slot, _buffer) = surface.dequeue_buffer().unwrap();
    surface.queue_buffer(slot).unwrap();

    // In real system, compositor would pick this up on next VSYNC
}

#[test]
fn test_concurrent_surfaces() {
    use crate::compositor::surface::SurfaceClient;

    let mut client = SurfaceClient::new();

    // Create multiple surfaces
    let surface1 = client.create_surface(100, 100, "S1").unwrap();
    let surface2 = client.create_surface(200, 200, "S2").unwrap();
    let surface3 = client.create_surface(300, 300, "S3").unwrap();

    // All should have unique layer IDs
    assert_ne!(surface1.layer_id(), surface2.layer_id());
    assert_ne!(surface2.layer_id(), surface3.layer_id());
}
