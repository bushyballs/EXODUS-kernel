pub mod command_buffer;
pub mod compute;
/// GPU / 3D Graphics framework for Genesis
///
/// GPU abstraction, shader compiler, 3D renderer,
/// Vulkan-like command buffers, compute shaders,
/// texture management, render passes.
///
/// Original implementation for Hoags OS.
pub mod device;
pub mod pipeline;
pub mod renderer3d;
pub mod ring;
pub mod shader;
pub mod texture;

use crate::{serial_print, serial_println};

pub fn init() {
    device::init();
    shader::init();
    renderer3d::init();
    compute::init();
    pipeline::init();
    texture::init();
    command_buffer::init();
    ring::init();
    serial_println!("  GPU/3D initialized (device, shaders, 3D renderer, compute, Vulkan-like pipeline, cmd ring)");
}
