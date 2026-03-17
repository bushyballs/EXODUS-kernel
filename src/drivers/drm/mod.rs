pub mod connector;
pub mod crtc;
/// DRM/KMS modesetting subsystem for Genesis — built from scratch
///
/// Implements a minimal Kernel Mode Setting (KMS) stack:
///
///   modesetting — core KMS: ModeInfo, mode constants, PLL calc, register apply
///   connector   — physical display outputs (VGA/DVI/HDMI/DP/LVDS) and status
///   plane       — framebuffer scanout planes (primary, overlay, cursor)
///   crtc        — CRT controllers: mode programming, vblank tracking
///
/// Entry point: `drm::init()` is called from `drivers::init()`.
///
/// Design constraints (mirrors kernel-wide rules):
///   - No heap (Vec/Box/String/alloc)
///   - No floats (no as f32 / as f64)
///   - No panics (no unwrap/expect/panic)
///   - All counters: saturating_add / saturating_sub
///   - All sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile only
///   - All I/O port accesses in unsafe blocks
///
/// No external crates. All code is original.
pub mod modesetting;
pub mod plane;

use crate::serial_println;

/// Initialize the full DRM/KMS subsystem.
///
/// Initialisation order matters:
///   1. modesetting — sets up VBE and VGA helpers (no side-effects on HW yet)
///   2. connector   — detects attached displays via DAC sense
///   3. plane       — initialises the 8-plane table
///   4. crtc        — initialises the 2-CRTC table
///
/// After init(), callers can use crtc_enable() / plane_set_framebuffer() /
/// plane_enable() to bring up a display pipeline.
pub fn init() {
    serial_println!("  DRM/KMS: initializing subsystem...");
    modesetting::init();
    connector::init();
    plane::init();
    crtc::init();
    serial_println!("  DRM/KMS: subsystem ready");
}
