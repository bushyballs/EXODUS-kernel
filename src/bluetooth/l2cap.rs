/// Logical Link Control and Adaptation Protocol (L2CAP).
///
/// L2CAP provides multiplexed data channels over the HCI ACL link.
/// Responsibilities:
///   - Channel multiplexing via Channel IDs (CIDs)
///   - Protocol/Service Multiplexer (PSM) based connection
///   - Segmentation and reassembly (SAR) for large PDUs
///   - Flow control (basic, enhanced retransmission, LE credit-based)
///   - MTU negotiation via signaling commands
///
/// Fixed channels:
///   - CID 0x0001: L2CAP Signaling (BR/EDR)
///   - CID 0x0002: Connectionless reception
///   - CID 0x0003: AMP Manager (unused here)
///   - CID 0x0004: ATT (BLE)
///   - CID 0x0005: LE Signaling
///   - CID 0x0006: Security Manager
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Well-known CIDs.
const CID_SIGNALING: u16 = 0x0001;
const CID_CONNECTIONLESS: u16 = 0x0002;
const CID_ATT: u16 = 0x0004;
const CID_LE_SIGNALING: u16 = 0x0005;
const CID_SMP: u16 = 0x0006;

/// First dynamically allocated CID.
const CID_DYNAMIC_START: u16 = 0x0040;

/// Well-known PSMs.
const PSM_SDP: u16 = 0x0001;
const PSM_RFCOMM: u16 = 0x0003;
const PSM_AVDTP: u16 = 0x0019;
const PSM_AVCTP: u16 = 0x0017;

/// Default MTU for L2CAP channels.
const DEFAULT_MTU: u16 = 672;

/// L2CAP signaling command codes.
const SIG_COMMAND_REJECT: u8 = 0x01;
const SIG_CONN_REQ: u8 = 0x02;
const SIG_CONN_RSP: u8 = 0x03;
const SIG_CONFIG_REQ: u8 = 0x04;
const SIG_CONFIG_RSP: u8 = 0x05;
const SIG_DISCONN_REQ: u8 = 0x06;
const SIG_DISCONN_RSP: u8 = 0x07;
const SIG_INFO_REQ: u8 = 0x0A;
const SIG_INFO_RSP: u8 = 0x0B;

/// Global L2CAP manager state.
static L2CAP: Mutex<Option<L2capManager>> = Mutex::new(None);

/// Flow control mode for a channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlowControlMode {
    Basic,
    EnhancedRetransmission,
    LeCreditBased,
}

/// Channel state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ChannelState {
    Closed,
    WaitConnect,
    WaitConfig,
    Open,
    WaitDisconnect,
}

/// Internal per-channel state.
struct ChannelInner {
    local_cid: u16,
    remote_cid: u16,
    psm: u16,
    local_mtu: u16,
    remote_mtu: u16,
    flow_mode: FlowControlMode,
    state: ChannelState,
    rx_queue: VecDeque<Vec<u8>>,
    tx_queue: VecDeque<Vec<u8>>,
    credits: u16,
}

/// The L2CAP connection manager.
struct L2capManager {
    channels: BTreeMap<u16, ChannelInner>,
    next_cid: u16,
    signal_id: u8,
}

impl L2capManager {
    fn new() -> Self {
        Self {
            channels: BTreeMap::new(),
            next_cid: CID_DYNAMIC_START,
            signal_id: 1,
        }
    }

    /// Allocate the next dynamic CID.
    fn alloc_cid(&mut self) -> u16 {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1);
        if self.next_cid < CID_DYNAMIC_START {
            self.next_cid = CID_DYNAMIC_START;
        }
        cid
    }

    /// Get the next signaling identifier.
    fn next_signal_id(&mut self) -> u8 {
        let id = self.signal_id;
        self.signal_id = self.signal_id.wrapping_add(1);
        if self.signal_id == 0 {
            self.signal_id = 1;
        }
        id
    }

    /// Register a fixed channel (no PSM, always open).
    fn register_fixed_channel(&mut self, cid: u16) {
        let ch = ChannelInner {
            local_cid: cid,
            remote_cid: cid,
            psm: 0,
            local_mtu: DEFAULT_MTU,
            remote_mtu: DEFAULT_MTU,
            flow_mode: FlowControlMode::Basic,
            state: ChannelState::Open,
            rx_queue: VecDeque::new(),
            tx_queue: VecDeque::new(),
            credits: 0,
        };
        self.channels.insert(cid, ch);
    }

    /// Open a dynamic channel for a given PSM.
    fn open_channel(&mut self, psm: u16) -> u16 {
        let cid = self.alloc_cid();
        let ch = ChannelInner {
            local_cid: cid,
            remote_cid: 0,
            psm,
            local_mtu: DEFAULT_MTU,
            remote_mtu: DEFAULT_MTU,
            flow_mode: FlowControlMode::Basic,
            state: ChannelState::WaitConnect,
            rx_queue: VecDeque::new(),
            tx_queue: VecDeque::new(),
            credits: 0,
        };
        self.channels.insert(cid, ch);
        serial_println!("    [l2cap] Opened channel CID={:#06x} PSM={:#06x}", cid, psm);
        cid
    }

    /// Handle an incoming connection request signaling packet.
    fn handle_conn_request(&mut self, psm: u16, source_cid: u16) -> u16 {
        let local_cid = self.alloc_cid();
        let ch = ChannelInner {
            local_cid,
            remote_cid: source_cid,
            psm,
            local_mtu: DEFAULT_MTU,
            remote_mtu: DEFAULT_MTU,
            flow_mode: FlowControlMode::Basic,
            state: ChannelState::WaitConfig,
            rx_queue: VecDeque::new(),
            tx_queue: VecDeque::new(),
            credits: 0,
        };
        self.channels.insert(local_cid, ch);
        serial_println!("    [l2cap] Incoming connection PSM={:#06x} remote_cid={:#06x} -> local_cid={:#06x}",
            psm, source_cid, local_cid);
        local_cid
    }

    /// Mark a channel as configured and open.
    fn configure_channel(&mut self, cid: u16, remote_mtu: u16) {
        if let Some(ch) = self.channels.get_mut(&cid) {
            ch.remote_mtu = remote_mtu;
            ch.state = ChannelState::Open;
            serial_println!("    [l2cap] Channel CID={:#06x} configured, remote_mtu={}", cid, remote_mtu);
        }
    }

    /// Close a channel.
    fn close_channel(&mut self, cid: u16) {
        if let Some(ch) = self.channels.get_mut(&cid) {
            ch.state = ChannelState::Closed;
            serial_println!("    [l2cap] Channel CID={:#06x} closed", cid);
        }
        // Only remove dynamic channels.
        if cid >= CID_DYNAMIC_START {
            self.channels.remove(&cid);
        }
    }

    /// Queue data to send on a channel with basic-mode segmentation.
    fn send_on_channel(&mut self, cid: u16, data: &[u8]) {
        if let Some(ch) = self.channels.get_mut(&cid) {
            if ch.state != ChannelState::Open {
                serial_println!("    [l2cap] Cannot send on CID={:#06x}: not open", cid);
                return;
            }

            let mtu = ch.remote_mtu as usize;
            // Segment the data into MTU-sized L2CAP PDUs.
            let mut offset = 0;
            while offset < data.len() {
                let end = core::cmp::min(offset + mtu, data.len());
                let segment = data[offset..end].to_vec();

                // Build L2CAP basic header: length (2) + CID (2) + payload
                let length = segment.len() as u16;
                let remote_cid = ch.remote_cid;
                let mut pdu = Vec::with_capacity(4 + segment.len());
                pdu.push((length & 0xFF) as u8);
                pdu.push((length >> 8) as u8);
                pdu.push((remote_cid & 0xFF) as u8);
                pdu.push((remote_cid >> 8) as u8);
                pdu.extend_from_slice(&segment);

                ch.tx_queue.push_back(pdu);
                offset = end;
            }
        }
    }

    /// Receive data from a channel's receive queue.
    fn recv_from_channel(&mut self, cid: u16) -> Vec<u8> {
        if let Some(ch) = self.channels.get_mut(&cid) {
            ch.rx_queue.pop_front().unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// Deliver an incoming L2CAP PDU (after HCI ACL reassembly).
    fn deliver_pdu(&mut self, raw_pdu: &[u8]) {
        if raw_pdu.len() < 4 {
            return;
        }
        let length = raw_pdu[0] as u16 | ((raw_pdu[1] as u16) << 8);
        let cid = raw_pdu[2] as u16 | ((raw_pdu[3] as u16) << 8);
        let payload_end = core::cmp::min(4 + length as usize, raw_pdu.len());
        let payload = raw_pdu[4..payload_end].to_vec();

        if cid == CID_SIGNALING || cid == CID_LE_SIGNALING {
            self.handle_signaling(&payload);
            return;
        }

        if let Some(ch) = self.channels.get_mut(&cid) {
            ch.rx_queue.push_back(payload);
        }
    }

    /// Process a signaling command.
    fn handle_signaling(&mut self, data: &[u8]) {
        if data.len() < 4 {
            return;
        }
        let code = data[0];
        let _id = data[1];
        let _length = data[2] as u16 | ((data[3] as u16) << 8);

        match code {
            SIG_CONN_REQ => {
                if data.len() >= 8 {
                    let psm = data[4] as u16 | ((data[5] as u16) << 8);
                    let source_cid = data[6] as u16 | ((data[7] as u16) << 8);
                    self.handle_conn_request(psm, source_cid);
                }
            }
            SIG_CONFIG_REQ => {
                if data.len() >= 6 {
                    let dest_cid = data[4] as u16 | ((data[5] as u16) << 8);
                    // Extract MTU option if present (type=0x01, len=2, value).
                    let mut mtu = DEFAULT_MTU;
                    let mut i = 8; // skip flags
                    while i + 3 < data.len() {
                        let opt_type = data[i];
                        let opt_len = data[i + 1] as usize;
                        if opt_type == 0x01 && opt_len == 2 && i + 4 <= data.len() {
                            mtu = data[i + 2] as u16 | ((data[i + 3] as u16) << 8);
                        }
                        i += 2 + opt_len;
                    }
                    self.configure_channel(dest_cid, mtu);
                }
            }
            SIG_DISCONN_REQ => {
                if data.len() >= 8 {
                    let dest_cid = data[4] as u16 | ((data[5] as u16) << 8);
                    self.close_channel(dest_cid);
                }
            }
            _ => {
                serial_println!("    [l2cap] Unhandled signaling code={:#04x}", code);
            }
        }
    }
}

/// An L2CAP channel for multiplexed data transfer.
pub struct L2capChannel {
    cid: u16,
    mtu: u16,
    psm: u16,
    flow_mode: FlowControlMode,
}

impl L2capChannel {
    pub fn new(cid: u16) -> Self {
        Self {
            cid,
            mtu: DEFAULT_MTU,
            psm: 0,
            flow_mode: FlowControlMode::Basic,
        }
    }

    /// Send data over this L2CAP channel.
    pub fn send(&mut self, data: &[u8]) {
        if let Some(mgr) = L2CAP.lock().as_mut() {
            mgr.send_on_channel(self.cid, data);
        }
    }

    /// Receive data from this L2CAP channel.
    pub fn recv(&mut self) -> Vec<u8> {
        if let Some(mgr) = L2CAP.lock().as_mut() {
            mgr.recv_from_channel(self.cid)
        } else {
            Vec::new()
        }
    }
}

/// Open a dynamic L2CAP channel for a given PSM.
pub fn open_channel(psm: u16) -> L2capChannel {
    let cid = if let Some(mgr) = L2CAP.lock().as_mut() {
        mgr.open_channel(psm)
    } else {
        0
    };
    L2capChannel {
        cid,
        mtu: DEFAULT_MTU,
        psm,
        flow_mode: FlowControlMode::Basic,
    }
}

/// Deliver an incoming L2CAP PDU from the HCI layer.
pub fn deliver(raw_pdu: &[u8]) {
    if let Some(mgr) = L2CAP.lock().as_mut() {
        mgr.deliver_pdu(raw_pdu);
    }
}

pub fn init() {
    let mut mgr = L2capManager::new();

    serial_println!("    [l2cap] Initializing L2CAP layer");

    // Register fixed channels.
    mgr.register_fixed_channel(CID_SIGNALING);
    mgr.register_fixed_channel(CID_CONNECTIONLESS);
    mgr.register_fixed_channel(CID_ATT);
    mgr.register_fixed_channel(CID_LE_SIGNALING);
    mgr.register_fixed_channel(CID_SMP);

    serial_println!("    [l2cap] Registered fixed channels (signaling, connectionless, ATT, LE sig, SMP)");

    *L2CAP.lock() = Some(mgr);
    serial_println!("    [l2cap] L2CAP initialized");
}
