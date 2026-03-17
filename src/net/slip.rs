use crate::sync::Mutex;
/// Serial Line IP (RFC 1055)
///
/// Provides SLIP framing for IP over serial links: END/ESC byte
/// stuffing, encoding, decoding, and interface management.
///
/// Inspired by: RFC 1055. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// SLIP special characters
// ---------------------------------------------------------------------------

/// Frame END marker
pub const END: u8 = 0xC0;
/// Escape character
pub const ESC: u8 = 0xDB;
/// Escaped END (ESC + ESC_END = literal END in data)
pub const ESC_END: u8 = 0xDC;
/// Escaped ESC (ESC + ESC_ESC = literal ESC in data)
pub const ESC_ESC: u8 = 0xDD;

/// Maximum SLIP frame payload (MTU)
const SLIP_MTU: usize = 1006;

/// Maximum encoded frame size (worst case: every byte escaped + 2 END markers)
const MAX_ENCODED_SIZE: usize = SLIP_MTU * 2 + 2;

// ---------------------------------------------------------------------------
// SLIP encoding/decoding
// ---------------------------------------------------------------------------

/// Encode an IP packet into a SLIP frame
pub fn encode_frame(packet: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(packet.len() * 2 + 2);
    // Send initial END to flush any garbage
    frame.push(END);
    for &byte in packet {
        match byte {
            END => {
                frame.push(ESC);
                frame.push(ESC_END);
            }
            ESC => {
                frame.push(ESC);
                frame.push(ESC_ESC);
            }
            _ => {
                frame.push(byte);
            }
        }
    }
    frame.push(END);
    frame
}

/// Decode a SLIP frame back to an IP packet
///
/// Returns the decoded packet, or None if the frame is invalid.
pub fn decode_frame(data: &[u8]) -> Option<Vec<u8>> {
    let mut packet = Vec::new();
    let mut in_escape = false;
    let mut started = false;

    for &byte in data {
        if in_escape {
            match byte {
                ESC_END => packet.push(END),
                ESC_ESC => packet.push(ESC),
                _ => return None, // protocol error
            }
            in_escape = false;
        } else {
            match byte {
                END => {
                    if started && !packet.is_empty() {
                        return Some(packet);
                    }
                    started = true;
                    packet.clear();
                }
                ESC => {
                    in_escape = true;
                }
                _ => {
                    started = true;
                    packet.push(byte);
                }
            }
        }
    }

    if !packet.is_empty() {
        Some(packet)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// SLIP interface
// ---------------------------------------------------------------------------

/// SLIP network interface
pub struct SlipInterface {
    pub id: u32,
    pub name: String,
    /// Partial receive buffer (for streaming byte input)
    recv_buf: Vec<u8>,
    /// Whether we're inside a frame
    in_frame: bool,
    /// Whether the previous byte was ESC
    in_escape: bool,
    /// Current frame being assembled
    current_frame: Vec<u8>,
    /// Completed received packets
    rx_queue: Vec<Vec<u8>>,
    /// Statistics
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub rx_errors: u64,
    /// MTU
    pub mtu: usize,
}

impl SlipInterface {
    pub fn new(id: u32, name: &str) -> Self {
        SlipInterface {
            id,
            name: String::from(name),
            recv_buf: Vec::new(),
            in_frame: false,
            in_escape: false,
            current_frame: Vec::new(),
            rx_queue: Vec::new(),
            tx_packets: 0,
            rx_packets: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            rx_errors: 0,
            mtu: SLIP_MTU,
        }
    }

    /// Encode a packet for transmission
    pub fn encode_packet(&mut self, packet: &[u8]) -> Vec<u8> {
        self.tx_packets = self.tx_packets.saturating_add(1);
        self.tx_bytes = self.tx_bytes.saturating_add(packet.len() as u64);
        encode_frame(packet)
    }

    /// Feed raw bytes from the serial port into the decoder
    ///
    /// Call this as bytes arrive. Completed packets are queued
    /// for retrieval via recv().
    pub fn feed_bytes(&mut self, data: &[u8]) {
        for &byte in data {
            if self.in_escape {
                match byte {
                    ESC_END => self.current_frame.push(END),
                    ESC_ESC => self.current_frame.push(ESC),
                    _ => {
                        self.rx_errors = self.rx_errors.saturating_add(1);
                        self.current_frame.clear();
                        self.in_frame = false;
                    }
                }
                self.in_escape = false;
            } else {
                match byte {
                    END => {
                        if self.in_frame && !self.current_frame.is_empty() {
                            // Complete frame
                            let pkt = core::mem::take(&mut self.current_frame);
                            self.rx_packets = self.rx_packets.saturating_add(1);
                            self.rx_bytes = self.rx_bytes.saturating_add(pkt.len() as u64);
                            self.rx_queue.push(pkt);
                        }
                        self.in_frame = true;
                        self.current_frame.clear();
                    }
                    ESC => {
                        self.in_escape = true;
                    }
                    _ => {
                        if self.current_frame.len() < MAX_ENCODED_SIZE {
                            self.current_frame.push(byte);
                        } else {
                            self.rx_errors = self.rx_errors.saturating_add(1);
                            self.current_frame.clear();
                        }
                        self.in_frame = true;
                    }
                }
            }
        }
    }

    /// Receive a decoded IP packet (non-blocking)
    pub fn recv(&mut self) -> Option<Vec<u8>> {
        if self.rx_queue.is_empty() {
            None
        } else {
            Some(self.rx_queue.remove(0))
        }
    }

    /// Check if packets are available
    pub fn has_data(&self) -> bool {
        !self.rx_queue.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct SlipSubsystem {
    interfaces: Vec<SlipInterface>,
    next_id: u32,
}

static SLIP: Mutex<Option<SlipSubsystem>> = Mutex::new(None);

pub fn init() {
    *SLIP.lock() = Some(SlipSubsystem {
        interfaces: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: SLIP subsystem initialized");
}

/// Create a SLIP interface
pub fn create_interface(name: &str) -> Result<u32, &'static str> {
    let mut guard = SLIP.lock();
    let sys = guard.as_mut().ok_or("SLIP not initialized")?;
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.interfaces.push(SlipInterface::new(id, name));
    Ok(id)
}
