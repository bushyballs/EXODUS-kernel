use crate::sync::Mutex;
/// CAN bus driver for Genesis — no-heap, fixed-size static arrays
///
/// Implements a CAN 2.0 controller registry with per-controller RX ring
/// buffers, acceptance filters, loopback mode, and simulated TX.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of CAN controllers supported
pub const MAX_CAN_CONTROLLERS: usize = 4;

/// Maximum number of acceptance filters per controller
pub const MAX_CAN_FILTERS: usize = 32;

/// CAN 2.0A/B maximum data bytes per frame
pub const CAN_MAX_DLEN: usize = 8;

/// Extended frame flag (29-bit CAN ID)
pub const CAN_EFF_FLAG: u32 = 0x8000_0000;

/// Remote Transmission Request flag
pub const CAN_RTR_FLAG: u32 = 0x4000_0000;

/// Error frame flag
pub const CAN_ERR_FLAG: u32 = 0x2000_0000;

/// Standard frame (SFF) 11-bit ID mask
pub const CAN_SFF_MASK: u32 = 0x7FF;

/// Extended frame (EFF) 29-bit ID mask
pub const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

/// Simulated CAN controller base I/O port
pub const CAN_IO_BASE: u16 = 0x3C0;

/// RX ring buffer capacity (must be a power of two for the & 0xF mask)
const CAN_RX_BUF_LEN: usize = 16;

/// Mask used to wrap ring-buffer head/tail indices into [0, 15]
const CAN_RX_BUF_MASK: u8 = 0xF;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single CAN frame (CAN 2.0A / 2.0B compatible)
#[derive(Copy, Clone)]
pub struct CanFrame {
    /// CAN ID with optional EFF/RTR/ERR flags in the high bits
    pub can_id: u32,
    /// Data Length Code — number of valid bytes in `data` (0..=8)
    pub can_dlc: u8,
    /// Frame payload
    pub data: [u8; CAN_MAX_DLEN],
}

impl CanFrame {
    pub const fn empty() -> Self {
        CanFrame {
            can_id: 0,
            can_dlc: 0,
            data: [0u8; CAN_MAX_DLEN],
        }
    }
}

/// An acceptance filter entry: a frame passes if
///   `(frame_id & can_mask) == (can_id & can_mask)`
#[derive(Copy, Clone)]
pub struct CanFilter {
    /// Filter reference CAN ID (masked comparison)
    pub can_id: u32,
    /// Bitmask applied to both `can_id` and the incoming frame ID
    pub can_mask: u32,
}

impl CanFilter {
    pub const fn empty() -> Self {
        CanFilter {
            can_id: 0,
            can_mask: 0,
        }
    }

    /// Returns `true` when the given frame ID passes this filter.
    pub fn matches(&self, frame_id: u32) -> bool {
        (frame_id & self.can_mask) == (self.can_id & self.can_mask)
    }
}

/// State for a single CAN controller instance
#[derive(Copy, Clone)]
pub struct CanController {
    /// Numeric ID assigned at registration
    pub id: u32,
    /// Base I/O port for this controller
    pub io_base: u16,
    /// Configured bus bit-rate in bits/second
    pub bitrate: u32,
    /// When true, transmitted frames are looped back into the RX buffer
    pub loopback: bool,
    /// Ring-buffer head (next read position)
    pub rx_head: u8,
    /// Ring-buffer tail (next write position)
    pub rx_tail: u8,
    /// RX ring buffer
    pub rx_buf: [CanFrame; CAN_RX_BUF_LEN],
    /// Acceptance filter table
    pub filters: [CanFilter; MAX_CAN_FILTERS],
    /// Number of active filters (0 = accept-all)
    pub nfilters: u8,
    /// Count of successfully transmitted frames
    pub tx_count: u64,
    /// Count of successfully received frames
    pub rx_count: u64,
    /// Count of error events (bus errors, overflow, etc.)
    pub error_count: u64,
    /// True when this table slot is occupied
    pub active: bool,
}

impl CanController {
    pub const fn empty() -> Self {
        CanController {
            id: 0,
            io_base: 0,
            bitrate: 0,
            loopback: false,
            rx_head: 0,
            rx_tail: 0,
            rx_buf: [CanFrame::empty(); CAN_RX_BUF_LEN],
            filters: [CanFilter::empty(); MAX_CAN_FILTERS],
            nfilters: 0,
            tx_count: 0,
            rx_count: 0,
            error_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CAN_CONTROLLERS: Mutex<[CanController; MAX_CAN_CONTROLLERS]> =
    Mutex::new([CanController::empty(); MAX_CAN_CONTROLLERS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return the index into the controller array for a given `id`, or `None`
/// if no active controller with that id is found.
fn find_controller_idx(
    controllers: &[CanController; MAX_CAN_CONTROLLERS],
    id: u32,
) -> Option<usize> {
    let mut i: usize = 0;
    while i < MAX_CAN_CONTROLLERS {
        if controllers[i].active && controllers[i].id == id {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Check whether a frame passes the filter set of a controller.
///
/// If `nfilters` is 0 every frame is accepted (accept-all policy).
fn frame_passes_filters(ctrl: &CanController, frame_id: u32) -> bool {
    if ctrl.nfilters == 0 {
        return true;
    }
    let n = ctrl.nfilters as usize;
    let cap = if n < MAX_CAN_FILTERS {
        n
    } else {
        MAX_CAN_FILTERS
    };
    let mut i: usize = 0;
    while i < cap {
        if ctrl.filters[i].matches(frame_id) {
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Enqueue `frame` into the controller's RX ring buffer.
///
/// Increments `rx_count` on success.  On overflow (ring full) increments
/// `error_count` and drops the frame.
fn enqueue_rx(ctrl: &mut CanController, frame: &CanFrame) {
    let next_tail = (ctrl.rx_tail.wrapping_add(1)) & CAN_RX_BUF_MASK;
    if next_tail == ctrl.rx_head {
        // Ring buffer full — drop frame, record error
        ctrl.error_count = ctrl.error_count.saturating_add(1);
        return;
    }
    let slot = ctrl.rx_tail as usize;
    ctrl.rx_buf[slot] = *frame;
    ctrl.rx_tail = next_tail;
    ctrl.rx_count = ctrl.rx_count.saturating_add(1);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new CAN controller.
///
/// Allocates the first free slot in `CAN_CONTROLLERS`.
/// Returns the assigned controller id on success, or `None` if the table
/// is full.
pub fn can_register_controller(io_base: u16, bitrate: u32) -> Option<u32> {
    let mut controllers = CAN_CONTROLLERS.lock();
    let mut i: usize = 0;
    while i < MAX_CAN_CONTROLLERS {
        if !controllers[i].active {
            let id = i as u32;
            controllers[i] = CanController {
                id,
                io_base,
                bitrate,
                loopback: false,
                rx_head: 0,
                rx_tail: 0,
                rx_buf: [CanFrame::empty(); CAN_RX_BUF_LEN],
                filters: [CanFilter::empty(); MAX_CAN_FILTERS],
                nfilters: 0,
                tx_count: 0,
                rx_count: 0,
                error_count: 0,
                active: true,
            };
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Enable or disable loopback mode for a controller.
///
/// In loopback mode transmitted frames are immediately enqueued into the
/// controller's own RX buffer.
///
/// Returns `true` on success, `false` if the controller id is not found.
pub fn can_set_loopback(id: u32, loopback: bool) -> bool {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        controllers[idx].loopback = loopback;
        return true;
    }
    false
}

/// Add an acceptance filter to a controller.
///
/// Returns `true` on success, `false` if the controller is not found or the
/// filter table is full.
pub fn can_add_filter(id: u32, filter: CanFilter) -> bool {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        let nf = controllers[idx].nfilters as usize;
        if nf >= MAX_CAN_FILTERS {
            return false;
        }
        controllers[idx].filters[nf] = filter;
        controllers[idx].nfilters = controllers[idx].nfilters.saturating_add(1);
        return true;
    }
    false
}

/// Remove all acceptance filters from a controller (revert to accept-all).
///
/// Returns `true` on success, `false` if the controller id is not found.
pub fn can_clear_filters(id: u32) -> bool {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        controllers[idx].nfilters = 0;
        return true;
    }
    false
}

/// Transmit a CAN frame.
///
/// Behaviour:
///   - In loopback mode: frame is always accepted (no filter check) and
///     enqueued into the controller's own RX buffer; `tx_count` is also
///     incremented.
///   - Otherwise: simulated TX — `tx_count` is incremented (no real bus
///     access).
///
/// Returns `true` on success, `false` if the controller id is not found.
pub fn can_send(id: u32, frame: &CanFrame) -> bool {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        controllers[idx].tx_count = controllers[idx].tx_count.saturating_add(1);
        if controllers[idx].loopback {
            // In loopback mode enqueue directly — filters bypassed per spec
            enqueue_rx(&mut controllers[idx], frame);
        }
        // Non-loopback: simulated TX (tx_count updated above); no physical bus
        return true;
    }
    false
}

/// Receive a CAN frame from the RX ring buffer.
///
/// Dequeues the oldest frame and writes it into `frame_out`.
/// Returns `true` when a frame was available, `false` when the buffer is
/// empty or the controller id is not found.
pub fn can_recv(id: u32, frame_out: &mut CanFrame) -> bool {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        let ctrl = &mut controllers[idx];
        if ctrl.rx_head == ctrl.rx_tail {
            // Ring buffer empty
            return false;
        }
        let slot = ctrl.rx_head as usize;
        *frame_out = ctrl.rx_buf[slot];
        ctrl.rx_head = (ctrl.rx_head.wrapping_add(1)) & CAN_RX_BUF_MASK;
        return true;
    }
    false
}

/// Deliver an externally-received CAN frame to a controller.
///
/// Applies the controller's acceptance filters: if the frame passes (or there
/// are no filters), it is enqueued into the RX buffer.
///
/// This function is intended for use by interrupt handlers or simulation
/// harnesses that inject frames into the driver.
pub fn can_receive_frame(id: u32, frame: &CanFrame) {
    let mut controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        if frame_passes_filters(&controllers[idx], frame.can_id) {
            enqueue_rx(&mut controllers[idx], frame);
        }
    }
}

/// Return transmission, reception, and error counters for a controller.
///
/// Returns `Some((tx_count, rx_count, error_count))` on success, or `None`
/// if the controller id is not found.
pub fn can_get_stats(id: u32) -> Option<(u64, u64, u64)> {
    let controllers = CAN_CONTROLLERS.lock();
    if let Some(idx) = find_controller_idx(&*controllers, id) {
        let c = &controllers[idx];
        return Some((c.tx_count, c.rx_count, c.error_count));
    }
    None
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the CAN bus driver.
///
/// Registers a loopback CAN controller at `CAN_IO_BASE` with a default
/// bitrate of 500 kbit/s and loopback mode enabled.
pub fn init() {
    if let Some(id) = can_register_controller(CAN_IO_BASE, 500_000) {
        can_set_loopback(id, true);
        serial_println!("[can] CAN bus driver initialized");
    } else {
        serial_println!("[can] CAN bus driver initialized (no controller slots)");
    }
}
