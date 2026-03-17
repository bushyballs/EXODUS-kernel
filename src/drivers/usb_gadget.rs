/// usb_gadget — USB gadget framework
///
/// Provides the core abstractions for USB device-mode (gadget) operation:
///   - GadgetDriver registration
///   - Endpoint allocation and transfer management
///   - Control request dispatch (Setup packets)
///   - Composite gadget support (multiple functions)
///
/// Inspired by: Linux USB gadget framework (gadget.h / composite.c).
/// All code is original. Rules: no_std, no heap, no floats, no panics.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_GADGETS: usize = 4;
const MAX_ENDPOINTS: usize = 16; // per gadget
const MAX_XFER_BUF: usize = 512;

pub const EP_DIR_IN: u8 = 0x80;
pub const EP_DIR_OUT: u8 = 0x00;
pub const EP_TYPE_CTRL: u8 = 0;
pub const EP_TYPE_ISOC: u8 = 1;
pub const EP_TYPE_BULK: u8 = 2;
pub const EP_TYPE_INTR: u8 = 3;

// USB standard request codes
pub const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
pub const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
pub const USB_REQ_SET_INTERFACE: u8 = 0x0B;

// ---------------------------------------------------------------------------
// Endpoint descriptor (simplified)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct GadgetEndpoint {
    pub ep_addr: u8, // e.g. 0x81 = EP1 IN
    pub ep_type: u8, // EP_TYPE_*
    pub max_packet: u16,
    pub interval: u8, // for ISOC/INTR (frames)
    pub enabled: bool,
    pub stalled: bool,
    pub xfer_buf: [u8; MAX_XFER_BUF],
    pub xfer_len: u16,
}

impl GadgetEndpoint {
    pub const fn empty() -> Self {
        GadgetEndpoint {
            ep_addr: 0,
            ep_type: EP_TYPE_BULK,
            max_packet: 64,
            interval: 1,
            enabled: false,
            stalled: false,
            xfer_buf: [0u8; MAX_XFER_BUF],
            xfer_len: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Setup packet (USB 2.0 §9.3)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct UsbSetupPacket {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}

impl UsbSetupPacket {
    pub const fn zero() -> Self {
        UsbSetupPacket {
            bm_request_type: 0,
            b_request: 0,
            w_value: 0,
            w_index: 0,
            w_length: 0,
        }
    }
    pub fn from_bytes(b: &[u8; 8]) -> Self {
        UsbSetupPacket {
            bm_request_type: b[0],
            b_request: b[1],
            w_value: ((b[3] as u16) << 8) | b[2] as u16,
            w_index: ((b[5] as u16) << 8) | b[4] as u16,
            w_length: ((b[7] as u16) << 8) | b[6] as u16,
        }
    }
}

// ---------------------------------------------------------------------------
// Gadget state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum GadgetSpeed {
    Unknown,
    LowSpeed,
    FullSpeed,
    HighSpeed,
    SuperSpeed,
}

#[derive(Copy, Clone, PartialEq)]
pub enum GadgetState {
    Unconnected,
    Connected,
    Configured,
    Suspended,
}

#[derive(Copy, Clone)]
pub struct UsbGadget {
    pub id: u32,
    pub name: [u8; 32],
    pub name_len: u8,
    pub vendor_id: u16,
    pub product_id: u16,
    pub speed: GadgetSpeed,
    pub state: GadgetState,
    pub config: u8, // active configuration number
    pub endpoints: [GadgetEndpoint; MAX_ENDPOINTS],
    pub ep_count: u8,
    pub active: bool,
}

impl UsbGadget {
    pub const fn empty() -> Self {
        const EMPTY_EP: GadgetEndpoint = GadgetEndpoint::empty();
        UsbGadget {
            id: 0,
            name: [0u8; 32],
            name_len: 0,
            vendor_id: 0x1D6B,
            product_id: 0x0001, // Linux Foundation defaults
            speed: GadgetSpeed::Unknown,
            state: GadgetState::Unconnected,
            config: 0,
            endpoints: [EMPTY_EP; MAX_ENDPOINTS],
            ep_count: 0,
            active: false,
        }
    }
}

fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let len = src.len().min(31);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

const EMPTY_GADGET: UsbGadget = UsbGadget::empty();
static GADGETS: Mutex<[UsbGadget; MAX_GADGETS]> = Mutex::new([EMPTY_GADGET; MAX_GADGETS]);
static GADGET_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new USB gadget. Returns gadget id.
pub fn gadget_register(name: &[u8], vendor_id: u16, product_id: u16) -> Option<u32> {
    let id = GADGET_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if !gs[i].active {
            gs[i] = UsbGadget::empty();
            gs[i].id = id;
            gs[i].name_len = copy_name(&mut gs[i].name, name);
            gs[i].vendor_id = vendor_id;
            gs[i].product_id = product_id;
            gs[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Add an endpoint to a gadget. Returns endpoint index, or None if full.
pub fn gadget_add_endpoint(id: u32, ep_addr: u8, ep_type: u8, max_packet: u16) -> Option<u8> {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            let idx = gs[i].ep_count as usize;
            if idx >= MAX_ENDPOINTS {
                return None;
            }
            gs[i].endpoints[idx].ep_addr = ep_addr;
            gs[i].endpoints[idx].ep_type = ep_type;
            gs[i].endpoints[idx].max_packet = max_packet;
            gs[i].endpoints[idx].enabled = true;
            gs[i].ep_count = gs[i].ep_count.saturating_add(1);
            return Some(idx as u8);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Handle a USB Setup packet. Returns Ok(response_len) or Err(()).
pub fn gadget_handle_setup(
    id: u32,
    pkt: &UsbSetupPacket,
    resp: &mut [u8; 64],
) -> Result<usize, ()> {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            match pkt.b_request {
                USB_REQ_SET_CONFIGURATION => {
                    gs[i].config = (pkt.w_value & 0xFF) as u8;
                    gs[i].state = if gs[i].config != 0 {
                        GadgetState::Configured
                    } else {
                        GadgetState::Connected
                    };
                    return Ok(0);
                }
                USB_REQ_GET_DESCRIPTOR => {
                    // Return a minimal device descriptor (18 bytes)
                    let desc = [
                        18u8, // bLength
                        0x01, // bDescriptorType = DEVICE
                        0x00,
                        0x02, // bcdUSB = 2.00
                        0x00, // bDeviceClass
                        0x00, // bDeviceSubClass
                        0x00, // bDeviceProtocol
                        64,   // bMaxPacketSize0
                        (gs[i].vendor_id & 0xFF) as u8,
                        (gs[i].vendor_id >> 8) as u8,
                        (gs[i].product_id & 0xFF) as u8,
                        (gs[i].product_id >> 8) as u8,
                        0x00,
                        0x01, // bcdDevice
                        0x00, // iManufacturer
                        0x00, // iProduct
                        0x00, // iSerialNumber
                        0x01, // bNumConfigurations
                    ];
                    let len = desc.len().min(pkt.w_length as usize).min(64);
                    let mut k = 0usize;
                    while k < len {
                        resp[k] = desc[k];
                        k = k.saturating_add(1);
                    }
                    return Ok(len);
                }
                _ => return Err(()),
            }
        }
        i = i.saturating_add(1);
    }
    Err(())
}

/// Queue data for IN transfer on an endpoint.
pub fn gadget_ep_write(id: u32, ep_addr: u8, data: &[u8]) -> bool {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            let mut j = 0usize;
            while j < gs[i].ep_count as usize {
                if gs[i].endpoints[j].ep_addr == ep_addr {
                    let len = data.len().min(MAX_XFER_BUF);
                    let mut k = 0usize;
                    while k < len {
                        gs[i].endpoints[j].xfer_buf[k] = data[k];
                        k = k.saturating_add(1);
                    }
                    gs[i].endpoints[j].xfer_len = len as u16;
                    return true;
                }
                j = j.saturating_add(1);
            }
        }
        i = i.saturating_add(1);
    }
    false
}

/// Simulate connect event.
pub fn gadget_connect(id: u32, speed: GadgetSpeed) -> bool {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            gs[i].speed = speed;
            gs[i].state = GadgetState::Connected;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Simulate disconnect event.
pub fn gadget_disconnect(id: u32) -> bool {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            gs[i].state = GadgetState::Unconnected;
            gs[i].config = 0;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn gadget_unregister(id: u32) -> bool {
    let mut gs = GADGETS.lock();
    let mut i = 0usize;
    while i < MAX_GADGETS {
        if gs[i].active && gs[i].id == id {
            gs[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    serial_println!(
        "[usb_gadget] USB gadget framework initialized (max {} gadgets)",
        MAX_GADGETS
    );
}
