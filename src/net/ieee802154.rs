use crate::sync::Mutex;
/// IEEE 802.15.4 Low-Rate Wireless Personal Area Networks
///
/// Provides 802.15.4 frame encoding/decoding (MAC layer), short and
/// extended addressing, PAN ID management, beacon frames, security
/// headers, and radio interface management for IoT (ZigBee/Thread/6LoWPAN).
///
/// Inspired by: IEEE 802.15.4-2015 specification. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Frame type / constants
// ---------------------------------------------------------------------------

/// Frame types (FCF bits 0-2)
const FRAME_TYPE_BEACON: u8 = 0;
const FRAME_TYPE_DATA: u8 = 1;
const FRAME_TYPE_ACK: u8 = 2;
const FRAME_TYPE_CMD: u8 = 3;

/// Address modes (FCF bits 10-11 for dst, 14-15 for src)
const ADDR_MODE_NONE: u8 = 0;
const ADDR_MODE_SHORT: u8 = 2;
const ADDR_MODE_EXTENDED: u8 = 3;

/// Broadcast PAN ID
pub const BROADCAST_PAN: u16 = 0xFFFF;
/// Broadcast short address
pub const BROADCAST_ADDR: u16 = 0xFFFF;

/// Maximum PHY payload (at 2.4 GHz)
const MAX_PHY_PAYLOAD: usize = 127;
/// MAC header overhead (worst case: both extended addresses)
const MAX_MAC_HDR: usize = 23;
/// FCS length
const FCS_LEN: usize = 2;

// ---------------------------------------------------------------------------
// Addressing
// ---------------------------------------------------------------------------

/// 802.15.4 address (short or extended)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Addr {
    /// No address
    None,
    /// 16-bit short address
    Short(u16),
    /// 64-bit extended (EUI-64) address
    Extended(u64),
}

impl Addr {
    /// Wire size in bytes
    pub fn wire_len(&self) -> usize {
        match self {
            Addr::None => 0,
            Addr::Short(_) => 2,
            Addr::Extended(_) => 8,
        }
    }

    /// Address mode for frame control field
    fn mode(&self) -> u8 {
        match self {
            Addr::None => ADDR_MODE_NONE,
            Addr::Short(_) => ADDR_MODE_SHORT,
            Addr::Extended(_) => ADDR_MODE_EXTENDED,
        }
    }

    /// Encode to bytes (little-endian per 802.15.4)
    fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Addr::None => {}
            Addr::Short(a) => buf.extend_from_slice(&a.to_le_bytes()),
            Addr::Extended(a) => buf.extend_from_slice(&a.to_le_bytes()),
        }
    }

    /// Decode from bytes
    fn decode(data: &[u8], mode: u8) -> Option<(Self, usize)> {
        match mode {
            ADDR_MODE_NONE => Some((Addr::None, 0)),
            ADDR_MODE_SHORT => {
                if data.len() < 2 {
                    return None;
                }
                let a = u16::from_le_bytes([data[0], data[1]]);
                Some((Addr::Short(a), 2))
            }
            ADDR_MODE_EXTENDED => {
                if data.len() < 8 {
                    return None;
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&data[..8]);
                let a = u64::from_le_bytes(bytes);
                Some((Addr::Extended(a), 8))
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Frame Control Field
// ---------------------------------------------------------------------------

/// Frame control field (2 bytes)
#[derive(Debug, Clone, Copy)]
pub struct FrameControl {
    pub frame_type: u8,
    pub security_enabled: bool,
    pub frame_pending: bool,
    pub ack_request: bool,
    pub pan_id_compress: bool,
    pub dst_addr_mode: u8,
    pub src_addr_mode: u8,
    pub frame_version: u8,
}

impl FrameControl {
    /// Encode to 2-byte little-endian
    pub fn encode(&self) -> [u8; 2] {
        let mut fcf: u16 = 0;
        fcf |= (self.frame_type as u16) & 0x07;
        if self.security_enabled {
            fcf |= 1 << 3;
        }
        if self.frame_pending {
            fcf |= 1 << 4;
        }
        if self.ack_request {
            fcf |= 1 << 5;
        }
        if self.pan_id_compress {
            fcf |= 1 << 6;
        }
        fcf |= ((self.dst_addr_mode as u16) & 0x03) << 10;
        fcf |= ((self.frame_version as u16) & 0x03) << 12;
        fcf |= ((self.src_addr_mode as u16) & 0x03) << 14;
        fcf.to_le_bytes()
    }

    /// Decode from 2 bytes
    pub fn decode(data: &[u8; 2]) -> Self {
        let fcf = u16::from_le_bytes(*data);
        FrameControl {
            frame_type: (fcf & 0x07) as u8,
            security_enabled: fcf & (1 << 3) != 0,
            frame_pending: fcf & (1 << 4) != 0,
            ack_request: fcf & (1 << 5) != 0,
            pan_id_compress: fcf & (1 << 6) != 0,
            dst_addr_mode: ((fcf >> 10) & 0x03) as u8,
            frame_version: ((fcf >> 12) & 0x03) as u8,
            src_addr_mode: ((fcf >> 14) & 0x03) as u8,
        }
    }
}

// ---------------------------------------------------------------------------
// MAC frame
// ---------------------------------------------------------------------------

/// 802.15.4 MAC frame
#[derive(Debug, Clone)]
pub struct MacFrame {
    pub fc: FrameControl,
    pub seq_num: u8,
    pub dst_pan: u16,
    pub dst_addr: Addr,
    pub src_pan: u16,
    pub src_addr: Addr,
    pub payload: Vec<u8>,
}

impl MacFrame {
    /// Create a data frame
    pub fn new_data(
        seq: u8,
        dst_pan: u16,
        dst: Addr,
        src_pan: u16,
        src: Addr,
        payload: &[u8],
        ack_req: bool,
    ) -> Self {
        let pan_compress =
            dst_pan == src_pan && dst.mode() != ADDR_MODE_NONE && src.mode() != ADDR_MODE_NONE;
        MacFrame {
            fc: FrameControl {
                frame_type: FRAME_TYPE_DATA,
                security_enabled: false,
                frame_pending: false,
                ack_request: ack_req,
                pan_id_compress: pan_compress,
                dst_addr_mode: dst.mode(),
                src_addr_mode: src.mode(),
                frame_version: 1,
            },
            seq_num: seq,
            dst_pan,
            dst_addr: dst,
            src_pan,
            src_addr: src,
            payload: payload.to_vec(),
        }
    }

    /// Create a beacon frame
    pub fn new_beacon(seq: u8, pan_id: u16, src: Addr, payload: &[u8]) -> Self {
        MacFrame {
            fc: FrameControl {
                frame_type: FRAME_TYPE_BEACON,
                security_enabled: false,
                frame_pending: false,
                ack_request: false,
                pan_id_compress: false,
                dst_addr_mode: ADDR_MODE_NONE,
                src_addr_mode: src.mode(),
                frame_version: 0,
            },
            seq_num: seq,
            dst_pan: BROADCAST_PAN,
            dst_addr: Addr::None,
            src_pan: pan_id,
            src_addr: src,
            payload: payload.to_vec(),
        }
    }

    /// Create an ACK frame
    pub fn new_ack(seq: u8) -> Self {
        MacFrame {
            fc: FrameControl {
                frame_type: FRAME_TYPE_ACK,
                security_enabled: false,
                frame_pending: false,
                ack_request: false,
                pan_id_compress: false,
                dst_addr_mode: ADDR_MODE_NONE,
                src_addr_mode: ADDR_MODE_NONE,
                frame_version: 0,
            },
            seq_num: seq,
            dst_pan: 0,
            dst_addr: Addr::None,
            src_pan: 0,
            src_addr: Addr::None,
            payload: Vec::new(),
        }
    }

    /// Encode frame to bytes (excluding FCS, which is computed by HW)
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MAX_MAC_HDR + self.payload.len());
        // Frame control (2 bytes LE)
        buf.extend_from_slice(&self.fc.encode());
        // Sequence number
        buf.push(self.seq_num);
        // Destination PAN + address
        if self.fc.dst_addr_mode != ADDR_MODE_NONE {
            buf.extend_from_slice(&self.dst_pan.to_le_bytes());
            self.dst_addr.encode(&mut buf);
        }
        // Source PAN (omitted if PAN ID compression)
        if self.fc.src_addr_mode != ADDR_MODE_NONE {
            if !self.fc.pan_id_compress {
                buf.extend_from_slice(&self.src_pan.to_le_bytes());
            }
            self.src_addr.encode(&mut buf);
        }
        // Payload
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode frame from bytes
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 3 {
            return None;
        }
        let fc = FrameControl::decode(&[data[0], data[1]]);
        let seq_num = data[2];
        let mut pos = 3;

        // Destination PAN + address
        let (dst_pan, dst_addr) = if fc.dst_addr_mode != ADDR_MODE_NONE {
            if pos + 2 > data.len() {
                return None;
            }
            let pan = u16::from_le_bytes([data[pos], data[pos + 1]]);
            pos += 2;
            let (addr, len) = Addr::decode(&data[pos..], fc.dst_addr_mode)?;
            pos += len;
            (pan, addr)
        } else {
            (0, Addr::None)
        };

        // Source PAN + address
        let (src_pan, src_addr) = if fc.src_addr_mode != ADDR_MODE_NONE {
            let pan = if fc.pan_id_compress {
                dst_pan
            } else {
                if pos + 2 > data.len() {
                    return None;
                }
                let p = u16::from_le_bytes([data[pos], data[pos + 1]]);
                pos += 2;
                p
            };
            let (addr, len) = Addr::decode(&data[pos..], fc.src_addr_mode)?;
            pos += len;
            (pan, addr)
        } else {
            (0, Addr::None)
        };

        let payload = data[pos..].to_vec();

        Some(MacFrame {
            fc,
            seq_num,
            dst_pan,
            dst_addr,
            src_pan,
            src_addr,
            payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Radio interface
// ---------------------------------------------------------------------------

/// 802.15.4 channel (2.4 GHz band: channels 11-26)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel(pub u8);

impl Channel {
    /// Center frequency in kHz for 2.4 GHz channels
    pub fn frequency_khz(&self) -> u32 {
        2405000 + ((self.0 as u32).saturating_sub(11)) * 5000
    }

    /// Validate channel number (2.4 GHz band)
    pub fn is_valid(&self) -> bool {
        self.0 >= 11 && self.0 <= 26
    }
}

/// Radio interface
pub struct Ieee802154Radio {
    pub id: u32,
    pub name: String,
    pub pan_id: u16,
    pub short_addr: u16,
    pub ext_addr: u64,
    pub channel: Channel,
    pub tx_power: i8, // dBm
    pub seq_num: u8,
    tx_queue: Vec<Vec<u8>>,
    rx_queue: Vec<MacFrame>,
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub rx_errors: u64,
}

impl Ieee802154Radio {
    pub fn new(id: u32, name: &str, pan_id: u16, short_addr: u16) -> Self {
        Ieee802154Radio {
            id,
            name: String::from(name),
            pan_id,
            short_addr,
            ext_addr: 0,
            channel: Channel(11),
            tx_power: 0,
            seq_num: 0,
            tx_queue: Vec::new(),
            rx_queue: Vec::new(),
            tx_frames: 0,
            rx_frames: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            rx_errors: 0,
        }
    }

    /// Set the radio channel
    pub fn set_channel(&mut self, ch: Channel) -> Result<(), RadioError> {
        if !ch.is_valid() {
            return Err(RadioError::InvalidChannel);
        }
        self.channel = ch;
        Ok(())
    }

    /// Transmit a data frame
    pub fn transmit(&mut self, dst: Addr, payload: &[u8]) -> Result<(), RadioError> {
        if payload.len() + MAX_MAC_HDR > MAX_PHY_PAYLOAD - FCS_LEN {
            return Err(RadioError::PayloadTooLarge);
        }
        let src = Addr::Short(self.short_addr);
        let frame = MacFrame::new_data(
            self.seq_num,
            self.pan_id,
            dst,
            self.pan_id,
            src,
            payload,
            true,
        );
        self.seq_num = self.seq_num.wrapping_add(1);
        let encoded = frame.encode();
        self.tx_frames = self.tx_frames.saturating_add(1);
        self.tx_bytes = self.tx_bytes.saturating_add(encoded.len() as u64);
        self.tx_queue.push(encoded);
        Ok(())
    }

    /// Dequeue encoded frame for the radio hardware
    pub fn dequeue_tx(&mut self) -> Option<Vec<u8>> {
        if self.tx_queue.is_empty() {
            None
        } else {
            Some(self.tx_queue.remove(0))
        }
    }

    /// Feed raw received bytes from the radio hardware
    pub fn on_receive(&mut self, data: &[u8]) {
        // Strip FCS if present (last 2 bytes)
        let frame_data = if data.len() > FCS_LEN {
            &data[..data.len() - FCS_LEN]
        } else {
            data
        };
        match MacFrame::decode(frame_data) {
            Some(frame) => {
                // Check if frame is for us
                let dominated = self.is_for_us(&frame);
                if dominated {
                    self.rx_frames = self.rx_frames.saturating_add(1);
                    self.rx_bytes = self.rx_bytes.saturating_add(data.len() as u64);
                    self.rx_queue.push(frame);
                }
            }
            None => {
                self.rx_errors = self.rx_errors.saturating_add(1);
            }
        }
    }

    /// Check if a frame is addressed to this radio
    fn is_for_us(&self, frame: &MacFrame) -> bool {
        match frame.dst_addr {
            Addr::None => true, // beacon
            Addr::Short(a) => {
                (a == self.short_addr || a == BROADCAST_ADDR)
                    && (frame.dst_pan == self.pan_id || frame.dst_pan == BROADCAST_PAN)
            }
            Addr::Extended(a) => {
                a == self.ext_addr
                    && (frame.dst_pan == self.pan_id || frame.dst_pan == BROADCAST_PAN)
            }
        }
    }

    /// Receive a decoded frame
    pub fn receive(&mut self) -> Option<MacFrame> {
        if self.rx_queue.is_empty() {
            None
        } else {
            Some(self.rx_queue.remove(0))
        }
    }

    /// Check if received frames are available
    pub fn has_data(&self) -> bool {
        !self.rx_queue.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioError {
    NotInitialized,
    InterfaceNotFound,
    InvalidChannel,
    PayloadTooLarge,
}

// ---------------------------------------------------------------------------
// Global subsystem
// ---------------------------------------------------------------------------

struct Ieee802154Subsystem {
    radios: Vec<Ieee802154Radio>,
    next_id: u32,
}

static IEEE802154: Mutex<Option<Ieee802154Subsystem>> = Mutex::new(None);

pub fn init() {
    *IEEE802154.lock() = Some(Ieee802154Subsystem {
        radios: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: IEEE 802.15.4 subsystem initialized");
}

/// Create a new 802.15.4 radio interface
pub fn create_radio(name: &str, pan_id: u16, short_addr: u16) -> Result<u32, RadioError> {
    let mut guard = IEEE802154.lock();
    let sys = guard.as_mut().ok_or(RadioError::NotInitialized)?;
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.radios
        .push(Ieee802154Radio::new(id, name, pan_id, short_addr));
    Ok(id)
}

/// Transmit on a radio interface
pub fn transmit(radio_id: u32, dst: Addr, payload: &[u8]) -> Result<(), RadioError> {
    let mut guard = IEEE802154.lock();
    let sys = guard.as_mut().ok_or(RadioError::NotInitialized)?;
    let radio = sys
        .radios
        .iter_mut()
        .find(|r| r.id == radio_id)
        .ok_or(RadioError::InterfaceNotFound)?;
    radio.transmit(dst, payload)
}
