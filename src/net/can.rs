use crate::sync::Mutex;
/// Controller Area Network (CAN) bus networking
///
/// Provides CAN 2.0A (11-bit) and CAN 2.0B (29-bit extended) frame
/// encoding/decoding, acceptance filtering (mask+match), error
/// counters, bus state management, and interface registry.
///
/// Inspired by: CAN 2.0 specification, SocketCAN. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// CAN constants
// ---------------------------------------------------------------------------

/// Maximum data length for classic CAN
const CAN_MAX_DLC: u8 = 8;
/// Maximum data length for CAN FD
const CANFD_MAX_DLC: u8 = 64;

/// Standard frame flag (11-bit ID)
pub const CAN_SFF_MASK: u32 = 0x000007FF;
/// Extended frame flag (29-bit ID)
pub const CAN_EFF_MASK: u32 = 0x1FFFFFFF;
/// Extended frame format bit
pub const CAN_EFF_FLAG: u32 = 0x80000000;
/// Remote transmission request bit
pub const CAN_RTR_FLAG: u32 = 0x40000000;
/// Error frame flag
pub const CAN_ERR_FLAG: u32 = 0x20000000;

// ---------------------------------------------------------------------------
// CAN frame
// ---------------------------------------------------------------------------

/// CAN frame type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Standard data frame (11-bit ID)
    Standard,
    /// Extended data frame (29-bit ID)
    Extended,
    /// Remote transmission request
    Remote,
    /// Error frame
    Error,
}

/// CAN frame (classic CAN 2.0)
#[derive(Debug, Clone)]
pub struct CanFrame {
    /// Arbitration ID (11 or 29 bits, plus flags in upper bits)
    pub can_id: u32,
    /// Data length code (0-8 for classic CAN, 0-64 for CAN FD)
    pub dlc: u8,
    /// Frame data
    pub data: [u8; 8],
    /// Frame type
    pub frame_type: FrameType,
    /// Timestamp (tick counter when received)
    pub timestamp: u64,
}

impl CanFrame {
    /// Create a new standard CAN frame
    pub fn new_standard(id: u16, data: &[u8]) -> Self {
        let mut frame_data = [0u8; 8];
        let len = data.len().min(8);
        frame_data[..len].copy_from_slice(&data[..len]);
        CanFrame {
            can_id: (id as u32) & CAN_SFF_MASK,
            dlc: len as u8,
            data: frame_data,
            frame_type: FrameType::Standard,
            timestamp: 0,
        }
    }

    /// Create a new extended CAN frame
    pub fn new_extended(id: u32, data: &[u8]) -> Self {
        let mut frame_data = [0u8; 8];
        let len = data.len().min(8);
        frame_data[..len].copy_from_slice(&data[..len]);
        CanFrame {
            can_id: (id & CAN_EFF_MASK) | CAN_EFF_FLAG,
            dlc: len as u8,
            data: frame_data,
            frame_type: FrameType::Extended,
            timestamp: 0,
        }
    }

    /// Create a remote transmission request
    pub fn new_rtr(id: u32, dlc: u8, extended: bool) -> Self {
        let can_id = if extended {
            (id & CAN_EFF_MASK) | CAN_EFF_FLAG | CAN_RTR_FLAG
        } else {
            (id & CAN_SFF_MASK) | CAN_RTR_FLAG
        };
        CanFrame {
            can_id,
            dlc: dlc.min(CAN_MAX_DLC),
            data: [0u8; 8],
            frame_type: FrameType::Remote,
            timestamp: 0,
        }
    }

    /// Check if this is an extended frame
    pub fn is_extended(&self) -> bool {
        self.can_id & CAN_EFF_FLAG != 0
    }

    /// Check if this is a remote transmission request
    pub fn is_rtr(&self) -> bool {
        self.can_id & CAN_RTR_FLAG != 0
    }

    /// Check if this is an error frame
    pub fn is_error(&self) -> bool {
        self.can_id & CAN_ERR_FLAG != 0
    }

    /// Get the raw arbitration ID (without flags)
    pub fn raw_id(&self) -> u32 {
        if self.is_extended() {
            self.can_id & CAN_EFF_MASK
        } else {
            self.can_id & CAN_SFF_MASK
        }
    }

    /// Encode frame to wire format (13 bytes: 4 ID + 1 DLC + 8 data)
    pub fn encode(&self) -> [u8; 13] {
        let mut buf = [0u8; 13];
        let id_bytes = self.can_id.to_be_bytes();
        buf[0..4].copy_from_slice(&id_bytes);
        buf[4] = self.dlc;
        let len = (self.dlc as usize).min(8);
        buf[5..5 + len].copy_from_slice(&self.data[..len]);
        buf
    }

    /// Decode frame from wire format
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < 5 {
            return None;
        }
        let can_id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let dlc = data[4].min(CAN_MAX_DLC);
        let len = dlc as usize;
        if data.len() < 5 + len {
            return None;
        }
        let mut frame_data = [0u8; 8];
        frame_data[..len].copy_from_slice(&data[5..5 + len]);

        let frame_type = if can_id & CAN_ERR_FLAG != 0 {
            FrameType::Error
        } else if can_id & CAN_RTR_FLAG != 0 {
            FrameType::Remote
        } else if can_id & CAN_EFF_FLAG != 0 {
            FrameType::Extended
        } else {
            FrameType::Standard
        };

        Some(CanFrame {
            can_id,
            dlc,
            data: frame_data,
            frame_type,
            timestamp: 0,
        })
    }
}

// ---------------------------------------------------------------------------
// CAN acceptance filter
// ---------------------------------------------------------------------------

/// CAN acceptance filter (mask + match)
///
/// A frame passes the filter if: (frame.can_id & mask) == (match_id & mask)
#[derive(Debug, Clone, Copy)]
pub struct CanFilter {
    /// ID to match against
    pub match_id: u32,
    /// Mask (1 = must match, 0 = don't care)
    pub mask: u32,
}

impl CanFilter {
    /// Create a filter that matches a single standard ID exactly
    pub fn exact_standard(id: u16) -> Self {
        CanFilter {
            match_id: id as u32,
            mask: CAN_SFF_MASK | CAN_EFF_FLAG | CAN_RTR_FLAG,
        }
    }

    /// Create a filter that matches a single extended ID exactly
    pub fn exact_extended(id: u32) -> Self {
        CanFilter {
            match_id: (id & CAN_EFF_MASK) | CAN_EFF_FLAG,
            mask: CAN_EFF_MASK | CAN_EFF_FLAG | CAN_RTR_FLAG,
        }
    }

    /// Create a filter that matches a range of standard IDs
    pub fn range_standard(base_id: u16, mask: u16) -> Self {
        CanFilter {
            match_id: base_id as u32,
            mask: (mask as u32) | CAN_EFF_FLAG,
        }
    }

    /// Create a pass-all filter
    pub fn pass_all() -> Self {
        CanFilter {
            match_id: 0,
            mask: 0,
        }
    }

    /// Test whether a frame passes this filter
    pub fn matches(&self, frame: &CanFrame) -> bool {
        (frame.can_id & self.mask) == (self.match_id & self.mask)
    }
}

// ---------------------------------------------------------------------------
// CAN bus error counters
// ---------------------------------------------------------------------------

/// CAN bus state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusState {
    /// Error active (normal operation)
    ErrorActive,
    /// Error warning (TEC or REC >= 96)
    ErrorWarning,
    /// Error passive (TEC or REC >= 128)
    ErrorPassive,
    /// Bus off (TEC >= 256)
    BusOff,
}

/// CAN error counters
#[derive(Debug, Clone, Copy)]
pub struct ErrorCounters {
    /// Transmit error counter
    pub tec: u16,
    /// Receive error counter
    pub rec: u16,
    /// Total error frames received
    pub error_frames: u64,
    /// Bus-off events
    pub bus_off_count: u32,
}

impl ErrorCounters {
    fn new() -> Self {
        ErrorCounters {
            tec: 0,
            rec: 0,
            error_frames: 0,
            bus_off_count: 0,
        }
    }

    /// Get the current bus state from error counters
    pub fn bus_state(&self) -> BusState {
        if self.tec >= 256 {
            BusState::BusOff
        } else if self.tec >= 128 || self.rec >= 128 {
            BusState::ErrorPassive
        } else if self.tec >= 96 || self.rec >= 96 {
            BusState::ErrorWarning
        } else {
            BusState::ErrorActive
        }
    }

    /// Record a transmit error
    pub fn tx_error(&mut self) {
        self.tec = self.tec.saturating_add(8);
        self.error_frames = self.error_frames.saturating_add(1);
        if self.tec >= 256 {
            self.bus_off_count = self.bus_off_count.saturating_add(1);
        }
    }

    /// Record a receive error
    pub fn rx_error(&mut self) {
        self.rec = self.rec.saturating_add(1);
        self.error_frames = self.error_frames.saturating_add(1);
    }

    /// Record a successful transmission
    pub fn tx_success(&mut self) {
        self.tec = self.tec.saturating_sub(1);
    }

    /// Record a successful reception
    pub fn rx_success(&mut self) {
        if self.rec > 0 {
            self.rec = self.rec.saturating_sub(1);
        }
    }

    /// Reset counters (bus-off recovery)
    pub fn reset(&mut self) {
        self.tec = 0;
        self.rec = 0;
    }
}

// ---------------------------------------------------------------------------
// CAN bus interface
// ---------------------------------------------------------------------------

/// CAN bus bitrate
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bitrate {
    Kbps10,
    Kbps20,
    Kbps50,
    Kbps100,
    Kbps125,
    Kbps250,
    Kbps500,
    Kbps800,
    Kbps1000,
}

impl Bitrate {
    /// Get the bitrate in bits per second
    pub fn bps(self) -> u32 {
        match self {
            Bitrate::Kbps10 => 10_000,
            Bitrate::Kbps20 => 20_000,
            Bitrate::Kbps50 => 50_000,
            Bitrate::Kbps100 => 100_000,
            Bitrate::Kbps125 => 125_000,
            Bitrate::Kbps250 => 250_000,
            Bitrate::Kbps500 => 500_000,
            Bitrate::Kbps800 => 800_000,
            Bitrate::Kbps1000 => 1_000_000,
        }
    }
}

/// CAN bus interface
pub struct CanBus {
    pub id: u32,
    pub name: String,
    pub bitrate: Bitrate,
    pub loopback: bool,
    pub listen_only: bool,
    filters: Vec<CanFilter>,
    tx_queue: Vec<CanFrame>,
    rx_queue: Vec<CanFrame>,
    pub errors: ErrorCounters,
    pub tx_frames: u64,
    pub rx_frames: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    tick: u64,
}

impl CanBus {
    pub fn new(id: u32, name: &str, bitrate: Bitrate) -> Self {
        CanBus {
            id,
            name: String::from(name),
            bitrate,
            loopback: false,
            listen_only: false,
            filters: Vec::new(),
            tx_queue: Vec::new(),
            rx_queue: Vec::new(),
            errors: ErrorCounters::new(),
            tx_frames: 0,
            rx_frames: 0,
            tx_bytes: 0,
            rx_bytes: 0,
            tick: 0,
        }
    }

    /// Add an acceptance filter
    pub fn add_filter(&mut self, filter: CanFilter) {
        self.filters.push(filter);
    }

    /// Clear all acceptance filters
    pub fn clear_filters(&mut self) {
        self.filters.clear();
    }

    /// Check if a frame passes acceptance filters
    /// If no filters are set, all frames pass.
    fn accept_frame(&self, frame: &CanFrame) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        self.filters.iter().any(|f| f.matches(frame))
    }

    /// Queue a frame for transmission
    pub fn send(&mut self, frame: &CanFrame) -> Result<(), CanError> {
        if self.listen_only {
            return Err(CanError::ListenOnly);
        }
        if self.errors.bus_state() == BusState::BusOff {
            return Err(CanError::BusOff);
        }
        if frame.dlc > CAN_MAX_DLC {
            return Err(CanError::InvalidDlc);
        }
        // Priority insertion: lower ID = higher priority
        let pos = self
            .tx_queue
            .iter()
            .position(|f| f.raw_id() > frame.raw_id())
            .unwrap_or(self.tx_queue.len());
        self.tx_queue.insert(pos, frame.clone());
        self.tx_frames = self.tx_frames.saturating_add(1);
        self.tx_bytes = self.tx_bytes.saturating_add(frame.dlc as u64);
        self.errors.tx_success();

        // Loopback: also place in rx_queue
        if self.loopback {
            let mut lb = frame.clone();
            lb.timestamp = self.tick;
            self.rx_queue.push(lb);
        }

        Ok(())
    }

    /// Dequeue the next frame to transmit (highest priority first)
    pub fn dequeue_tx(&mut self) -> Option<CanFrame> {
        if self.tx_queue.is_empty() {
            None
        } else {
            Some(self.tx_queue.remove(0))
        }
    }

    /// Feed a received frame (from the bus controller)
    pub fn on_receive(&mut self, mut frame: CanFrame) {
        frame.timestamp = self.tick;
        if self.accept_frame(&frame) {
            self.rx_frames = self.rx_frames.saturating_add(1);
            self.rx_bytes = self.rx_bytes.saturating_add(frame.dlc as u64);
            self.errors.rx_success();
            self.rx_queue.push(frame);
        }
    }

    /// Read the next received frame
    pub fn recv(&mut self) -> Option<CanFrame> {
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

    /// Advance tick (for timestamping)
    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1);
    }

    /// Get bus state
    pub fn bus_state(&self) -> BusState {
        self.errors.bus_state()
    }
}

// ---------------------------------------------------------------------------
// CAN error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanError {
    NotInitialized,
    InterfaceNotFound,
    BusOff,
    ListenOnly,
    InvalidDlc,
    TxQueueFull,
}

// ---------------------------------------------------------------------------
// Global CAN subsystem
// ---------------------------------------------------------------------------

struct CanSubsystem {
    buses: Vec<CanBus>,
    next_id: u32,
}

static CAN: Mutex<Option<CanSubsystem>> = Mutex::new(None);

pub fn init() {
    *CAN.lock() = Some(CanSubsystem {
        buses: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: CAN bus subsystem initialized");
}

/// Create a new CAN bus interface
pub fn create_bus(name: &str, bitrate: Bitrate) -> Result<u32, CanError> {
    let mut guard = CAN.lock();
    let sys = guard.as_mut().ok_or(CanError::NotInitialized)?;
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.buses.push(CanBus::new(id, name, bitrate));
    Ok(id)
}

/// Send a frame on a CAN bus
pub fn send_frame(bus_id: u32, frame: &CanFrame) -> Result<(), CanError> {
    let mut guard = CAN.lock();
    let sys = guard.as_mut().ok_or(CanError::NotInitialized)?;
    let bus = sys
        .buses
        .iter_mut()
        .find(|b| b.id == bus_id)
        .ok_or(CanError::InterfaceNotFound)?;
    bus.send(frame)
}

/// Receive a frame from a CAN bus
pub fn recv_frame(bus_id: u32) -> Result<Option<CanFrame>, CanError> {
    let mut guard = CAN.lock();
    let sys = guard.as_mut().ok_or(CanError::NotInitialized)?;
    let bus = sys
        .buses
        .iter_mut()
        .find(|b| b.id == bus_id)
        .ok_or(CanError::InterfaceNotFound)?;
    Ok(bus.recv())
}

/// Add an acceptance filter to a CAN bus
pub fn add_filter(bus_id: u32, filter: CanFilter) -> Result<(), CanError> {
    let mut guard = CAN.lock();
    let sys = guard.as_mut().ok_or(CanError::NotInitialized)?;
    let bus = sys
        .buses
        .iter_mut()
        .find(|b| b.id == bus_id)
        .ok_or(CanError::InterfaceNotFound)?;
    bus.add_filter(filter);
    Ok(())
}
