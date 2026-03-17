use super::virtio::{
    buf_pfn, device_begin_init, device_driver_ok, device_fail, device_set_features,
    pci_find_virtio, setup_queue, VirtQueue, VirtqBuf,
};
/// VirtIO Input Device Driver — no-heap, static-buffer implementation
///
/// VirtIO input device (PCI vendor 0x1AF4, device 0x1052) provides keyboard,
/// mouse, and tablet events using the Linux input_event ABI over a virtqueue.
///
/// This driver uses the event ring as an in-kernel software buffer: events
/// pushed via `virtio_input_push_event` are queued in a fixed-size ring buffer
/// (256 entries) and consumed by `virtio_input_poll` / `virtio_input_flush`.
///
/// Ring semantics:
///   head = write pointer (wrapping u16)
///   tail = read  pointer (wrapping u16)
///   Full:  head.wrapping_sub(tail) >= INPUT_RING_SIZE as u16
///   Write: ring[head % INPUT_RING_SIZE] = ev; head = head.wrapping_add(1)
///   Read:  ev = ring[tail % INPUT_RING_SIZE]; tail = tail.wrapping_add(1)
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add / saturating_sub for counters
///   - wrapping_add for ring indices
///   - read_volatile / write_volatile for all MMIO accesses
///   - No panic — return Option<T> / bool on error
///   - No Vec, Box, String, alloc::*
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// PCI IDs
// ============================================================================

pub const VIRTIO_INPUT_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_INPUT_DEV_ID: u16 = 0x1052;

// ============================================================================
// Linux input event types (from linux/input.h)
// ============================================================================

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;

// ============================================================================
// Key codes (subset)
// ============================================================================

pub const KEY_ESC: u16 = 1;
pub const KEY_ENTER: u16 = 28;
pub const KEY_SPACE: u16 = 57;
pub const KEY_UP: u16 = 103;
pub const KEY_DOWN: u16 = 108;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_F1: u16 = 59;
pub const KEY_F10: u16 = 68;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
pub const BTN_MIDDLE: u16 = 0x112;

// ============================================================================
// Relative axis codes
// ============================================================================

pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;

// ============================================================================
// Key state values
// ============================================================================

pub const KEY_RELEASE: i32 = 0;
pub const KEY_PRESS: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

// ============================================================================
// InputEvent — 8-byte compact form
// ============================================================================

/// Linux input_event struct (compact 8-byte kernel form).
#[derive(Copy, Clone)]
pub struct InputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub const fn zero() -> Self {
        Self {
            event_type: 0,
            code: 0,
            value: 0,
        }
    }
}

// ============================================================================
// Ring buffer size
// ============================================================================

pub const INPUT_RING_SIZE: usize = 256;

// ============================================================================
// Driver state
// ============================================================================

pub struct VirtioInput {
    io_base: u16,
    event_ring: [InputEvent; INPUT_RING_SIZE],
    ring_head: u16, // write pointer (wrapping)
    ring_tail: u16, // read  pointer (wrapping)
    present: bool,
    // Keyboard state: bitmask of pressed keys (512 bits = 64 bytes, keys 0..511)
    key_state: [u8; 64],
    // Mouse state
    mouse_x: i32,
    mouse_y: i32,
    mouse_buttons: u8, // bit0=left, bit1=right, bit2=middle
}

impl VirtioInput {
    const fn empty() -> Self {
        Self {
            io_base: 0,
            event_ring: [InputEvent::zero(); INPUT_RING_SIZE],
            ring_head: 0,
            ring_tail: 0,
            present: false,
            key_state: [0u8; 64],
            mouse_x: 0,
            mouse_y: 0,
            mouse_buttons: 0,
        }
    }
}

static VIRTIO_INPUT: Mutex<VirtioInput> = Mutex::new(VirtioInput::empty());

// ============================================================================
// Virtqueue backing store (one event VQ)
// ============================================================================

// SAFETY: zeroed() is a valid initial state; accessed only before VQ is handed
// to the Mutex<Option<VirtQueue>>, which serialises all later accesses.
static mut EVENT_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();

static EVENT_VQ: Mutex<Option<VirtQueue>> = Mutex::new(None);

// ============================================================================
// Internal key-state bitmask helper
// ============================================================================

/// Update the 64-byte key-state bitmask for `key` (0..511).
///
/// Each key occupies one bit: byte = key >> 3, bit = key & 7.
/// Keys >= 512 are silently ignored (out of array bounds).
fn set_key_state(state: &mut VirtioInput, key: u16, pressed: bool) {
    let byte = (key >> 3) as usize;
    let bit = (key & 7) as u8;
    if byte < 64 {
        if pressed {
            state.key_state[byte] |= 1 << bit;
        } else {
            state.key_state[byte] &= !(1 << bit);
        }
    }
}

// ============================================================================
// Probe and initialise
// ============================================================================

/// Probe the PCI bus for a VirtIO input device and perform the VirtIO legacy
/// handshake (RESET → ACKNOWLEDGE → DRIVER → negotiate features → DRIVER_OK).
///
/// Returns `true` if a device was found and initialised successfully.
pub fn virtio_input_init() -> bool {
    // Locate PCI device (vendor=0x1AF4, device=0x1052)
    let (io_base, _bus, _dev, _func) =
        match pci_find_virtio(VIRTIO_INPUT_VENDOR, VIRTIO_INPUT_DEV_ID) {
            Some(v) => v,
            None => return false,
        };

    // --- VirtIO handshake ---
    // device_begin_init: RESET → ACKNOWLEDGE → DRIVER, returns host feature bits
    let _dev_features = device_begin_init(io_base);

    // VirtIO input has no mandatory feature bits we need to negotiate; accept
    // whatever the device advertises (pass 0 driver features for safety).
    if !device_set_features(io_base, 0) {
        serial_println!("  virtio-input: FEATURES_OK not accepted — aborting");
        device_fail(io_base);
        return false;
    }

    // --- Set up event virtqueue (VQ 0) ---
    let event_pfn = unsafe { buf_pfn(&EVENT_VQ_BUF) };
    if setup_queue(io_base, 0, event_pfn).is_none() {
        serial_println!("  virtio-input: event VQ size=0 — aborting");
        device_fail(io_base);
        return false;
    }
    let event_vq = unsafe { VirtQueue::new(&mut EVENT_VQ_BUF, io_base, 0) };
    *EVENT_VQ.lock() = Some(event_vq);

    // --- DRIVER_OK ---
    device_driver_ok(io_base);

    // --- Store state under the device Mutex ---
    {
        let mut dev = VIRTIO_INPUT.lock();
        dev.io_base = io_base;
        dev.present = true;
    }

    serial_println!("  virtio-input: ready  io={:#x}", io_base);
    super::register("virtio-input", super::DeviceType::Other);
    true
}

// ============================================================================
// Ring buffer operations
// ============================================================================

/// Enqueue a single `InputEvent` into the software ring buffer.
///
/// If the ring is full the event is silently dropped (no panic).
pub fn virtio_input_push_event(ev: InputEvent) {
    let mut dev = VIRTIO_INPUT.lock();
    // Full: head.wrapping_sub(tail) >= INPUT_RING_SIZE as u16
    let used = dev.ring_head.wrapping_sub(dev.ring_tail);
    if used >= INPUT_RING_SIZE as u16 {
        // Ring full — drop event rather than panic
        return;
    }
    let idx = (dev.ring_head as usize) % INPUT_RING_SIZE;
    dev.event_ring[idx] = ev;
    dev.ring_head = dev.ring_head.wrapping_add(1);
}

/// Dequeue the next `InputEvent` from the software ring buffer.
///
/// Returns `None` if the ring is empty.
pub fn virtio_input_poll() -> Option<InputEvent> {
    let mut dev = VIRTIO_INPUT.lock();
    if dev.ring_head == dev.ring_tail {
        return None;
    }
    let idx = (dev.ring_tail as usize) % INPUT_RING_SIZE;
    let ev = dev.event_ring[idx];
    dev.ring_tail = dev.ring_tail.wrapping_add(1);
    Some(ev)
}

// ============================================================================
// Key state query
// ============================================================================

/// Returns `true` if `key` is currently held down (bitmask check).
pub fn virtio_input_key_down(key: u16) -> bool {
    let dev = VIRTIO_INPUT.lock();
    let byte = (key >> 3) as usize;
    let bit = (key & 7) as u8;
    if byte < 64 {
        (dev.key_state[byte] >> bit) & 1 != 0
    } else {
        false
    }
}

// ============================================================================
// Flush — drain all pending events and update state
// ============================================================================

/// Drain all pending events from the ring buffer, updating keyboard bitmask
/// and mouse position for each event.
///
/// Event handling per type:
///   - `EV_KEY` → update `key_state` bitmask (value 0 = release, 1/2 = press)
///   - `EV_REL` + `REL_X` → `mouse_x` saturating_add(value)
///   - `EV_REL` + `REL_Y` → `mouse_y` saturating_add(value)
///
/// Returns the count of events processed.
pub fn virtio_input_flush() -> usize {
    let mut count = 0usize;
    loop {
        // Poll one event at a time (releases Mutex between iterations to avoid
        // holding it across the whole drain loop — matches the module's lock
        // granularity pattern used everywhere else in this codebase).
        let ev = {
            let mut dev = VIRTIO_INPUT.lock();
            if dev.ring_head == dev.ring_tail {
                break;
            }
            let idx = (dev.ring_tail as usize) % INPUT_RING_SIZE;
            let e = dev.event_ring[idx];
            dev.ring_tail = dev.ring_tail.wrapping_add(1);
            e
        };

        // Process the event under a fresh lock acquire so state updates are
        // serialised without keeping the ring-read lock held.
        {
            let mut dev = VIRTIO_INPUT.lock();
            match ev.event_type {
                EV_KEY => {
                    let pressed = ev.value != KEY_RELEASE;
                    set_key_state(&mut dev, ev.code, pressed);
                }
                EV_REL => match ev.code {
                    REL_X => {
                        dev.mouse_x = dev.mouse_x.saturating_add(ev.value);
                    }
                    REL_Y => {
                        dev.mouse_y = dev.mouse_y.saturating_add(ev.value);
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        count = count.saturating_add(1);
    }
    count
}

// ============================================================================
// Test helpers — inject synthetic events without real hardware
// ============================================================================

/// Inject a synthetic key press or release event into the ring buffer.
///
/// Useful for testing shell navigation without a physical keyboard attached.
pub fn virtio_input_simulate_key(key: u16, pressed: bool) {
    let value = if pressed { KEY_PRESS } else { KEY_RELEASE };
    virtio_input_push_event(InputEvent {
        event_type: EV_KEY,
        code: key,
        value,
    });
}

/// Inject a synthetic relative mouse movement into the ring buffer.
///
/// `dx` / `dy` are signed pixel deltas.
pub fn virtio_input_simulate_mouse_move(dx: i32, dy: i32) {
    virtio_input_push_event(InputEvent {
        event_type: EV_REL,
        code: REL_X,
        value: dx,
    });
    virtio_input_push_event(InputEvent {
        event_type: EV_REL,
        code: REL_Y,
        value: dy,
    });
}

// ============================================================================
// Public accessors for mouse state
// ============================================================================

/// Returns the current accumulated mouse X position.
pub fn virtio_input_mouse_x() -> i32 {
    VIRTIO_INPUT.lock().mouse_x
}

/// Returns the current accumulated mouse Y position.
pub fn virtio_input_mouse_y() -> i32 {
    VIRTIO_INPUT.lock().mouse_y
}

/// Returns the current mouse button bitmask (bit0=left, bit1=right, bit2=middle).
pub fn virtio_input_mouse_buttons() -> u8 {
    VIRTIO_INPUT.lock().mouse_buttons
}

/// Returns `true` if the VirtIO input device was successfully initialised.
pub fn virtio_input_is_present() -> bool {
    VIRTIO_INPUT.lock().present
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Probe and initialise the VirtIO input device.
/// Logs result to the serial port. Called once during kernel boot.
pub fn init() {
    if virtio_input_init() {
        serial_println!("[virtio_input] input device initialized");
    } else {
        serial_println!(
            "[virtio_input] no VirtIO input device found (io={:#x})",
            VIRTIO_INPUT.lock().io_base
        );
    }
}
