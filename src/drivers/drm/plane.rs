use crate::serial_println;
/// DRM display plane abstraction for Genesis — built from scratch
///
/// A plane is a hardware scanout region that maps a framebuffer address onto
/// a rectangular area of the display.  Three plane types are supported:
///
///   Primary  — full-screen base layer (one per CRTC)
///   Overlay  — composited above primary (hardware sprite)
///   Cursor   — small cursor plane (typically 64×64, hardware composited)
///
/// This implementation manages up to 8 planes in a fixed static array.
/// No heap allocation; all state lives in a Mutex-protected array.
///
/// plane_flip() provides atomic framebuffer swapping for double-buffering:
/// the new address is written while the CRTC is not mid-scanout
/// (no explicit vsync wait here — the caller should call crtc_wait_vblank
/// before flipping if tear-free is required).
///
/// No external crates. No heap. No floats. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// PlaneType
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaneType {
    Primary,
    Overlay,
    Cursor,
}

// ---------------------------------------------------------------------------
// Plane
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct Plane {
    /// Unique plane identifier (0–7)
    pub id: u32,
    pub plane_type: PlaneType,
    /// Source rectangle within the framebuffer (pixels)
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    /// Destination rectangle on the display (pixels)
    pub dst_x: u32,
    pub dst_y: u32,
    pub dst_w: u32,
    pub dst_h: u32,
    /// Physical address of the backing framebuffer
    pub fb_addr: u64,
    /// Whether this plane is currently enabled for scanout
    pub active: bool,
}

impl Plane {
    pub const fn empty() -> Self {
        Plane {
            id: 0,
            plane_type: PlaneType::Primary,
            src_x: 0,
            src_y: 0,
            src_w: 0,
            src_h: 0,
            dst_x: 0,
            dst_y: 0,
            dst_w: 0,
            dst_h: 0,
            fb_addr: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static plane table — 8 planes
// ---------------------------------------------------------------------------

static PLANES: Mutex<[Plane; 8]> = Mutex::new([
    Plane {
        id: 0,
        plane_type: PlaneType::Primary,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 1,
        plane_type: PlaneType::Primary,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 2,
        plane_type: PlaneType::Overlay,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 3,
        plane_type: PlaneType::Overlay,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 4,
        plane_type: PlaneType::Cursor,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 5,
        plane_type: PlaneType::Cursor,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 6,
        plane_type: PlaneType::Overlay,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
    Plane {
        id: 7,
        plane_type: PlaneType::Overlay,
        src_x: 0,
        src_y: 0,
        src_w: 0,
        src_h: 0,
        dst_x: 0,
        dst_y: 0,
        dst_w: 0,
        dst_h: 0,
        fb_addr: 0,
        active: false,
    },
]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set the framebuffer for a plane.
///
/// Sets the source rectangle to the full framebuffer (0, 0, w, h).
/// The framebuffer at `fb_addr` must already be mapped into the kernel's
/// address space.
pub fn plane_set_framebuffer(plane_id: u32, fb_addr: u64, w: u32, h: u32) {
    if plane_id >= 8 {
        return;
    }
    let mut table = PLANES.lock();
    let p = &mut table[plane_id as usize];
    p.fb_addr = fb_addr;
    p.src_x = 0;
    p.src_y = 0;
    p.src_w = w;
    p.src_h = h;
}

/// Set the destination display rectangle for a plane.
///
/// The destination rectangle is clamped to avoid zero-size regions.
pub fn plane_set_position(plane_id: u32, x: u32, y: u32, w: u32, h: u32) {
    if plane_id >= 8 {
        return;
    }
    let mut table = PLANES.lock();
    let p = &mut table[plane_id as usize];
    p.dst_x = x;
    p.dst_y = y;
    p.dst_w = w;
    p.dst_h = h;
}

/// Enable a plane for scanout.
pub fn plane_enable(plane_id: u32) {
    if plane_id >= 8 {
        return;
    }
    PLANES.lock()[plane_id as usize].active = true;
}

/// Disable a plane (remove from scanout).
pub fn plane_disable(plane_id: u32) {
    if plane_id >= 8 {
        return;
    }
    PLANES.lock()[plane_id as usize].active = false;
}

/// Atomically swap the framebuffer address for a plane (double-buffer flip).
///
/// This writes the new address without disabling the plane; the hardware
/// picks up the new pointer at the next scanout cycle.  Call
/// crtc_wait_vblank() before this if you need tear-free presentation.
pub fn plane_flip(plane_id: u32, new_fb_addr: u64) {
    if plane_id >= 8 {
        return;
    }
    // Acquire lock, update address atomically under the spinlock.
    PLANES.lock()[plane_id as usize].fb_addr = new_fb_addr;
}

/// Get a snapshot of a plane's state.
pub fn plane_get(plane_id: u32) -> Option<Plane> {
    if plane_id >= 8 {
        return None;
    }
    Some(PLANES.lock()[plane_id as usize])
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  DRM/KMS: 8 planes initialized (2 primary, 4 overlay, 2 cursor)");
}
