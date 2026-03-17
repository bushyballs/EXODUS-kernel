/// RFCOMM -- serial port emulation over Bluetooth.
///
/// RFCOMM provides RS-232 serial port emulation over L2CAP, enabling
/// legacy serial protocols (AT commands, OBEX, etc.) to run over Bluetooth.
/// Key concepts:
///   - Multiplexer session over a single L2CAP channel (PSM 0x0003)
///   - Up to 30 Data Link Connection Identifiers (DLCIs) per session
///   - Credit-based flow control to prevent buffer overruns
///   - UIH frames for data, SABM/UA for connection, DISC for disconnect
///   - Modem status signals (RTS/CTS/DTR/DSR emulation)
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// L2CAP PSM for RFCOMM.
const PSM_RFCOMM: u16 = 0x0003;

/// Maximum number of RFCOMM channels (DLCIs 2..31, each direction).
const MAX_DLCI: u8 = 31;

/// Default initial credits for flow control.
const DEFAULT_CREDITS: u8 = 7;

/// Default maximum frame size.
const DEFAULT_MAX_FRAME_SIZE: u16 = 127;

/// RFCOMM frame types.
const FRAME_SABM: u8 = 0x2F; // Set Asynchronous Balanced Mode
const FRAME_UA: u8 = 0x63;   // Unnumbered Acknowledgment
const FRAME_DM: u8 = 0x0F;   // Disconnected Mode
const FRAME_DISC: u8 = 0x43; // Disconnect
const FRAME_UIH: u8 = 0xEF;  // Unnumbered Information with Header check

/// RFCOMM multiplexer commands (sent on DLCI 0).
const MUX_PN: u8 = 0x20;     // Parameter Negotiation
const MUX_MSC: u8 = 0x38;    // Modem Status Command
const MUX_RPN: u8 = 0x24;    // Remote Port Negotiation
const MUX_TEST: u8 = 0x08;   // Test
const MUX_FCON: u8 = 0x28;   // Flow Control On
const MUX_FCOFF: u8 = 0x18;  // Flow Control Off

/// Global RFCOMM manager.
static RFCOMM: Mutex<Option<RfcommManager>> = Mutex::new(None);

/// RFCOMM channel state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DlcState {
    Closed,
    Opening,
    Open,
    Closing,
}

/// Modem signals for a channel.
#[derive(Debug, Clone, Copy)]
struct ModemSignals {
    rts: bool,
    cts: bool,
    dtr: bool,
    dsr: bool,
    ri: bool,
    dcd: bool,
}

impl ModemSignals {
    fn new() -> Self {
        Self {
            rts: true,
            cts: true,
            dtr: true,
            dsr: true,
            ri: false,
            dcd: false,
        }
    }

    /// Encode modem status into a single byte (MSC format).
    fn to_byte(&self) -> u8 {
        let mut v: u8 = 0;
        if self.dtr { v |= 0x04; }  // bit 2: RTC (maps to DTR)
        if self.rts { v |= 0x08; }  // bit 3: RTR (maps to RTS)
        if self.ri  { v |= 0x40; }  // bit 6: IC (ring indicator)
        if self.dcd { v |= 0x80; }  // bit 7: DV (data valid / DCD)
        v | 0x01 // EA bit always set
    }
}

/// Per-DLCI channel data.
struct DlcInfo {
    dlci: u8,
    state: DlcState,
    local_credits: u8,
    remote_credits: u8,
    max_frame_size: u16,
    modem: ModemSignals,
    rx_buffer: VecDeque<u8>,
    tx_buffer: VecDeque<u8>,
}

impl DlcInfo {
    fn new(dlci: u8) -> Self {
        Self {
            dlci,
            state: DlcState::Closed,
            local_credits: DEFAULT_CREDITS,
            remote_credits: DEFAULT_CREDITS,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            modem: ModemSignals::new(),
            rx_buffer: VecDeque::new(),
            tx_buffer: VecDeque::new(),
        }
    }
}

/// The RFCOMM multiplexer session manager.
struct RfcommManager {
    channels: BTreeMap<u8, DlcInfo>,
    mux_started: bool,
    initiator: bool,
}

impl RfcommManager {
    fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            mux_started: false,
            initiator: false,
        }
    }

    /// Compute the FCS (Frame Check Sequence) over a byte slice.
    /// RFCOMM uses a reversed CRC-8 table lookup.
    fn compute_fcs(data: &[u8]) -> u8 {
        // CRC-8 polynomial 0xE0 (reversed) -- RFCOMM uses ITU-T CRC table.
        // We compute this directly since we cannot use external crates.
        let mut fcs: u8 = 0xFF;
        for &byte in data {
            // Process each bit (LSB-first CRC-8).
            let mut val = byte;
            for _ in 0..8 {
                if (fcs ^ val) & 0x01 != 0 {
                    fcs = (fcs >> 1) ^ 0xE0;
                } else {
                    fcs >>= 1;
                }
                val >>= 1;
            }
        }
        0xFF - fcs
    }

    /// Build an RFCOMM frame.
    fn build_frame(dlci: u8, frame_type: u8, cr: bool, data: &[u8], credits: Option<u8>) -> Vec<u8> {
        let ea = 1u8; // address EA bit
        let cr_bit = if cr { 1u8 } else { 0u8 };
        let address = (dlci << 2) | (cr_bit << 1) | ea;

        let control = frame_type;
        let pf_bit = if credits.is_some() || frame_type == FRAME_SABM || frame_type == FRAME_DISC { 1u8 } else { 0u8 };
        let control_with_pf = control | (pf_bit << 4);

        let mut frame = Vec::new();
        frame.push(address);
        frame.push(control_with_pf);

        // Length field (EA encoding).
        let length = data.len();
        if length <= 127 {
            frame.push(((length as u8) << 1) | 1); // EA=1, single byte
        } else {
            frame.push((length as u8) << 1); // EA=0, first byte
            frame.push((length >> 7) as u8); // second byte
        }

        // Credits byte for UIH with PF=1.
        if let Some(c) = credits {
            if frame_type == FRAME_UIH {
                frame.push(c);
            }
        }

        // Payload.
        frame.extend_from_slice(data);

        // FCS: for SABM/DISC/UA/DM, FCS covers address + control.
        // For UIH, FCS covers address + control only.
        let fcs = Self::compute_fcs(&frame[0..2]);
        frame.push(fcs);

        frame
    }

    /// Start the multiplexer by sending SABM on DLCI 0.
    fn start_mux(&mut self) {
        if self.mux_started {
            return;
        }
        let frame = Self::build_frame(0, FRAME_SABM, true, &[], None);
        // Frame would be sent via L2CAP in a real stack.
        let _ = frame;
        self.mux_started = true;
        self.initiator = true;
        serial_println!("    [rfcomm] Multiplexer session started (initiator)");
    }

    /// Open a channel (DLCI = channel * 2 + direction).
    fn open_channel(&mut self, channel: u8) -> u8 {
        if channel == 0 || channel > MAX_DLCI {
            serial_println!("    [rfcomm] Invalid channel number {}", channel);
            return 0;
        }

        // DLCI encoding: for initiator, DLCI = channel * 2.
        let dlci = if self.initiator { channel << 1 } else { (channel << 1) | 1 };

        if !self.mux_started {
            self.start_mux();
        }

        let mut info = DlcInfo::new(dlci);
        info.state = DlcState::Opening;

        // Send SABM on this DLCI.
        let _frame = Self::build_frame(dlci, FRAME_SABM, true, &[], None);
        // Would be transmitted over L2CAP.

        // For a kernel driver we immediately transition to Open
        // since we handle the UA response in the event loop.
        info.state = DlcState::Open;

        serial_println!("    [rfcomm] Opened DLCI {} (channel {})", dlci, channel);
        self.channels.insert(dlci, info);
        dlci
    }

    /// Close a channel.
    fn close_channel(&mut self, dlci: u8) {
        if let Some(ch) = self.channels.get_mut(&dlci) {
            ch.state = DlcState::Closing;
            let _frame = Self::build_frame(dlci, FRAME_DISC, true, &[], None);
            ch.state = DlcState::Closed;
            serial_println!("    [rfcomm] Closed DLCI {}", dlci);
        }
        self.channels.remove(&dlci);
    }

    /// Write data to a channel.
    fn write_channel(&mut self, dlci: u8, data: &[u8]) {
        if let Some(ch) = self.channels.get_mut(&dlci) {
            if ch.state != DlcState::Open {
                serial_println!("    [rfcomm] Cannot write to DLCI {}: not open", dlci);
                return;
            }

            if ch.remote_credits == 0 {
                // No credits: buffer the data for later.
                for &b in data {
                    ch.tx_buffer.push_back(b);
                }
                serial_println!("    [rfcomm] DLCI {} no credits, buffered {} bytes", dlci, data.len());
                return;
            }

            // Segment data into max_frame_size chunks and send UIH frames.
            let mfs = ch.max_frame_size as usize;
            let mut offset = 0;
            while offset < data.len() && ch.remote_credits > 0 {
                let end = core::cmp::min(offset + mfs, data.len());
                let _frame = Self::build_frame(dlci, FRAME_UIH, true, &data[offset..end], Some(0));
                ch.remote_credits = ch.remote_credits.saturating_sub(1);
                offset = end;
            }
            // Buffer remainder if we ran out of credits.
            if offset < data.len() {
                for &b in &data[offset..] {
                    ch.tx_buffer.push_back(b);
                }
            }
        }
    }

    /// Read data from a channel's receive buffer.
    fn read_channel(&mut self, dlci: u8, buf: &mut [u8]) -> usize {
        if let Some(ch) = self.channels.get_mut(&dlci) {
            let count = core::cmp::min(buf.len(), ch.rx_buffer.len());
            for i in 0..count {
                buf[i] = ch.rx_buffer.pop_front().unwrap_or(0);
            }

            // If we consumed data, grant more credits.
            if count > 0 {
                ch.local_credits = ch.local_credits.saturating_add(count as u8);
            }
            count
        } else {
            0
        }
    }

    /// Handle an incoming RFCOMM frame (from L2CAP).
    fn handle_frame(&mut self, frame: &[u8]) {
        if frame.len() < 4 {
            return;
        }

        let address = frame[0];
        let dlci = address >> 2;
        let _cr = (address >> 1) & 1;
        let control = frame[1] & 0xEF; // mask out P/F bit
        let pf = (frame[1] >> 4) & 1;

        match control {
            FRAME_SABM => {
                // Respond with UA.
                let _ua = Self::build_frame(dlci, FRAME_UA, false, &[], None);
                if dlci == 0 {
                    self.mux_started = true;
                    serial_println!("    [rfcomm] MUX SABM received, responded UA");
                } else {
                    let mut info = DlcInfo::new(dlci);
                    info.state = DlcState::Open;
                    self.channels.insert(dlci, info);
                    serial_println!("    [rfcomm] DLCI {} opened by remote", dlci);
                }
            }
            FRAME_UA => {
                if let Some(ch) = self.channels.get_mut(&dlci) {
                    if ch.state == DlcState::Opening {
                        ch.state = DlcState::Open;
                    } else if ch.state == DlcState::Closing {
                        ch.state = DlcState::Closed;
                    }
                }
            }
            FRAME_DM => {
                self.channels.remove(&dlci);
            }
            FRAME_DISC => {
                let _ua = Self::build_frame(dlci, FRAME_UA, false, &[], None);
                self.channels.remove(&dlci);
                serial_println!("    [rfcomm] DLCI {} disconnected by remote", dlci);
            }
            FRAME_UIH => {
                // Parse length.
                if frame.len() < 3 {
                    return;
                }
                let (length, hdr_len) = if frame[2] & 1 != 0 {
                    ((frame[2] >> 1) as usize, 3usize)
                } else if frame.len() >= 4 {
                    (((frame[2] >> 1) as usize) | ((frame[3] as usize) << 7), 4usize)
                } else {
                    return;
                };

                let mut data_start = hdr_len;
                // Credit byte present when PF=1.
                let mut granted_credits: u8 = 0;
                if pf == 1 && data_start < frame.len() {
                    granted_credits = frame[data_start];
                    data_start += 1;
                }

                let data_end = core::cmp::min(data_start + length, frame.len().saturating_sub(1));
                let payload = &frame[data_start..data_end];

                if dlci == 0 {
                    // Multiplexer command.
                    self.handle_mux_command(payload);
                } else if let Some(ch) = self.channels.get_mut(&dlci) {
                    // Grant credits from remote.
                    ch.remote_credits = ch.remote_credits.saturating_add(granted_credits);
                    // Deliver data.
                    for &b in payload {
                        ch.rx_buffer.push_back(b);
                    }
                }
            }
            _ => {
                serial_println!("    [rfcomm] Unknown frame type {:#04x} on DLCI {}", control, dlci);
            }
        }
    }

    /// Handle multiplexer commands on DLCI 0.
    fn handle_mux_command(&mut self, data: &[u8]) {
        if data.len() < 2 {
            return;
        }
        let cmd_type = data[0] & 0xFC; // mask EA and C/R bits
        match cmd_type {
            MUX_MSC => {
                serial_println!("    [rfcomm] Modem status command received");
            }
            MUX_PN => {
                serial_println!("    [rfcomm] Parameter negotiation received");
            }
            _ => {
                serial_println!("    [rfcomm] MUX command {:#04x}", cmd_type);
            }
        }
    }
}

/// An RFCOMM channel providing serial port semantics.
pub struct RfcommChannel {
    dlci: u8,
    channel: u8,
    credits: u8,
    max_frame_size: u16,
}

impl RfcommChannel {
    pub fn new(channel: u8) -> Self {
        let dlci = if let Some(mgr) = RFCOMM.lock().as_mut() {
            mgr.open_channel(channel)
        } else {
            0
        };

        Self {
            dlci,
            channel,
            credits: DEFAULT_CREDITS,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    /// Write serial data to the RFCOMM channel.
    pub fn write(&mut self, data: &[u8]) {
        if let Some(mgr) = RFCOMM.lock().as_mut() {
            mgr.write_channel(self.dlci, data);
        }
    }

    /// Read serial data from the RFCOMM channel.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        if let Some(mgr) = RFCOMM.lock().as_mut() {
            mgr.read_channel(self.dlci, buf)
        } else {
            0
        }
    }
}

/// Deliver an incoming RFCOMM frame from L2CAP.
pub fn deliver(frame: &[u8]) {
    if let Some(mgr) = RFCOMM.lock().as_mut() {
        mgr.handle_frame(frame);
    }
}

pub fn init() {
    let mut mgr = RfcommManager::new();
    serial_println!("    [rfcomm] Initializing RFCOMM multiplexer");

    // Start the multiplexer session.
    mgr.start_mux();

    *RFCOMM.lock() = Some(mgr);
    serial_println!("    [rfcomm] RFCOMM initialized (PSM {:#06x})", PSM_RFCOMM);
}
