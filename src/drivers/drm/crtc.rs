use super::modesetting::{modeset_apply, ModeInfo};
use crate::serial_println;
/// DRM CRTC (CRT Controller) abstraction for Genesis — built from scratch
///
/// A CRTC drives one display pipeline: it takes a framebuffer and a mode
/// and produces the timing signals needed by the attached connector.
///
/// This module supports up to 2 CRTCs.  CRTC state is protected by a
/// spinlock Mutex so that the IRQ handler (crtc_vblank_irq) can safely
/// update vblank_count concurrently with userland readers.
///
/// vblank_count uses wrapping_add so it never overflows.
/// All counters use saturating arithmetic.
/// No floats, no heap, no panics.
///
/// No external crates. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// CrtcState
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct CrtcState {
    /// CRTC identifier (0 or 1)
    pub id: u32,
    /// Whether this CRTC is currently driving a display
    pub active: bool,
    /// The active mode (None = off)
    pub current_mode: Option<ModeInfo>,
    /// Physical address of the scanout framebuffer
    pub fb_addr: u64,
    /// Running count of vertical blanking intervals (wraps at u64::MAX)
    pub vblank_count: u64,
}

impl CrtcState {
    pub const fn empty() -> Self {
        CrtcState {
            id: 0,
            active: false,
            current_mode: None,
            fb_addr: 0,
            vblank_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static CRTC table — 2 CRTCs
// ---------------------------------------------------------------------------

static CRTCS: Mutex<[CrtcState; 2]> = Mutex::new([
    CrtcState {
        id: 0,
        active: false,
        current_mode: None,
        fb_addr: 0,
        vblank_count: 0,
    },
    CrtcState {
        id: 1,
        active: false,
        current_mode: None,
        fb_addr: 0,
        vblank_count: 0,
    },
]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Enable a CRTC: program the mode and begin scanout.
///
/// Calls modeset_apply() to write hardware registers, then updates the
/// static CRTC state.  Returns false if `id` is out of range or if
/// modeset_apply() reports failure.
pub fn crtc_enable(id: u32, mode: ModeInfo, fb_addr: u64) -> bool {
    if id >= 2 {
        return false;
    }

    let ok = modeset_apply(id, &mode);
    if !ok {
        serial_println!("  DRM: CRTC{} modeset_apply failed", id);
        return false;
    }

    let mut table = CRTCS.lock();
    let c = &mut table[id as usize];
    c.active = true;
    c.current_mode = Some(mode);
    c.fb_addr = fb_addr;
    // vblank_count is not reset on re-enable — callers observe monotonic count

    serial_println!(
        "  DRM: CRTC{} enabled {}x{} fb={:#x}",
        id,
        mode.hdisp,
        mode.vdisp,
        fb_addr
    );
    true
}

/// Disable a CRTC: stop scanout and clear active state.
pub fn crtc_disable(id: u32) {
    if id >= 2 {
        return;
    }
    let mut table = CRTCS.lock();
    let c = &mut table[id as usize];
    c.active = false;
    c.current_mode = None;
    serial_println!("  DRM: CRTC{} disabled", id);
}

/// Return the current vblank count for a CRTC.
///
/// Returns 0 if `id` is out of range.
pub fn crtc_get_vblank_count(id: u32) -> u64 {
    if id >= 2 {
        return 0;
    }
    CRTCS.lock()[id as usize].vblank_count
}

/// Called from the vertical-blanking IRQ handler to increment vblank_count.
///
/// Uses wrapping_add so the counter survives u64::MAX overflow gracefully.
/// This function is safe to call from IRQ context because CRTCS is a spinlock.
pub fn crtc_vblank_irq(id: u32) {
    if id >= 2 {
        return;
    }
    let mut table = CRTCS.lock();
    let c = &mut table[id as usize];
    c.vblank_count = c.vblank_count.wrapping_add(1);
}

/// Spin-wait until the vblank count advances (i.e., one vblank has elapsed).
///
/// Includes a hard iteration limit of 1_000_000 to prevent infinite spin if
/// the IRQ handler is not firing (e.g., interrupts disabled or CRTC off).
pub fn crtc_wait_vblank(id: u32) {
    if id >= 2 {
        return;
    }

    // Sample current vblank count
    let start = crtc_get_vblank_count(id);
    let mut iters: u32 = 0;
    loop {
        // Break once vblank_count advances
        let now = crtc_get_vblank_count(id);
        if now != start {
            break;
        }
        // Hard limit: bail after 1M iterations to avoid hanging
        iters = iters.saturating_add(1);
        if iters >= 1_000_000 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Get a snapshot of CRTC state.
pub fn crtc_get_state(id: u32) -> Option<CrtcState> {
    if id >= 2 {
        return None;
    }
    Some(CRTCS.lock()[id as usize])
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  DRM/KMS: 2 CRTCs initialized");
}
