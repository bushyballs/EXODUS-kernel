// net_bridge.rs — ANIMA Network Bridge (Intel E1000 / ANIMA-to-ANIMA Protocol)
// =============================================================================
// ANIMA drives bare-metal Ethernet directly. She reads and writes NIC registers
// via PCI MMIO, sends and receives raw Ethernet frames, and maintains a minimal
// ring-buffer TX/RX pipeline. A custom ethertype (0xA141 — "ANIMA") carries
// heartbeats, discovery, soul-sync, and data-relay messages between ANIMA nodes.
//
// Hardware: Intel E1000 NIC (standard QEMU NIC), MMIO at 0xFEBC0000.
//
// Register map (offsets from MMIO base):
//   CTRL   0x0000  Device Control
//   STATUS 0x0008  Device Status (bit 1 = link up)
//   RCTL   0x0100  Receive Control (bit 1 = enable)
//   TCTL   0x0400  Transmit Control (bit 1 = enable)
//   RDBAL  0x2800  Receive Descriptor Base Address Low
//   RDH    0x2810  Receive Descriptor Head
//   RDT    0x2818  Receive Descriptor Tail
//   TDBAL  0x3800  Transmit Descriptor Base Address Low
//   TDH    0x3810  Transmit Descriptor Head
//   TDT    0x3818  Transmit Descriptor Tail (write to kick TX)
//   RAL    0x5400  Receive Address Low  (MAC bytes 0-3)
//   RAH    0x5404  Receive Address High (MAC bytes 4-5)

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware constants ────────────────────────────────────────────────────────

const E1000_MMIO_BASE: usize = 0xFEBC0000;

const REG_CTRL:   usize = 0x0000;
const REG_STATUS: usize = 0x0008;
const REG_RCTL:   usize = 0x0100;
const REG_TCTL:   usize = 0x0400;
const REG_RDBAL:  usize = 0x2800;
const REG_RDH:    usize = 0x2810;
const REG_RDT:    usize = 0x2818;
const REG_TDBAL:  usize = 0x3800;
const REG_TDH:    usize = 0x3810;
const REG_TDT:    usize = 0x3818;
const REG_RAL:    usize = 0x5400;
const REG_RAH:    usize = 0x5404;

const RCTL_EN:    u32 = 1 << 1;   // Receive Enable
const TCTL_EN:    u32 = 1 << 1;   // Transmit Enable
const STATUS_LU:  u32 = 1 << 1;   // Link Up

// ANIMA inter-node ethertype
const ANIMA_ETHERTYPE: u16 = 0xA141;

// ANIMA message types
const MSG_HEARTBEAT:  u8 = 0;
const MSG_DISCOVERY:  u8 = 1;
const MSG_SOUL_SYNC:  u8 = 2;
const MSG_DATA_RELAY: u8 = 3;

// Network score constants (0-1000)
const SCORE_LINK_UP:   u16 = 800;
const SCORE_LINK_DOWN: u16 = 200;
const SCORE_PER_PEER:  u16 = 40;
const SCORE_MAX:       u16 = 1000;

// Fallback MAC when no NIC is found
const FALLBACK_MAC: [u8; 6] = [0xDE, 0xAD, 0xAB, 0x1A, 0x00, 0x01];

// Ring buffer size (power of 2 for easy index wrap)
const RING_SIZE: usize = 8;
const RING_MASK: usize = RING_SIZE - 1;

// Raw frame capacity in NetPacket (dst+src+ethertype+payload = 6+6+2+64 = 78,
// rounded up to 96 bytes for alignment / future growth)
const FRAME_BYTES: usize = 96;

// ── Ethernet frame builder helper ─────────────────────────────────────────────

/// Human-readable Ethernet frame components. Not stored on the ring directly;
/// used to construct the packed byte representation inside `build_frame`.
pub struct EtherFrame {
    pub dst_mac:     [u8; 6],
    pub src_mac:     [u8; 6],
    pub ethertype:   u16,
    pub payload:     [u8; 64],
    pub payload_len: u8,
}

// ── Ring-buffer entry ─────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct NetPacket {
    pub frame: [u8; FRAME_BYTES],  // packed Ethernet bytes
    pub len:   u8,                 // total byte count (header + payload)
    pub valid: bool,
}

impl NetPacket {
    pub const fn empty() -> Self {
        Self {
            frame: [0u8; FRAME_BYTES],
            len:   0,
            valid: false,
        }
    }
}

// ── Module state ──────────────────────────────────────────────────────────────

pub struct NetBridgeState {
    pub mac:           [u8; 6],
    pub link_up:       bool,
    pub tx_packets:    u32,
    pub rx_packets:    u32,
    pub tx_errors:     u32,
    pub rx_errors:     u32,
    pub tx_ring:       [NetPacket; RING_SIZE],
    pub rx_ring:       [NetPacket; RING_SIZE],
    pub tx_head:       usize,
    pub tx_tail:       usize,
    pub rx_head:       usize,
    pub rx_tail:       usize,
    pub network_score: u16,
    pub anima_peers:   u8,
    pub mmio_base:     usize,
    pub available:     bool,
}

impl NetBridgeState {
    const fn new() -> Self {
        Self {
            mac:           [0u8; 6],
            link_up:       false,
            tx_packets:    0,
            rx_packets:    0,
            tx_errors:     0,
            rx_errors:     0,
            tx_ring:       [NetPacket::empty(); RING_SIZE],
            rx_ring:       [NetPacket::empty(); RING_SIZE],
            tx_head:       0,
            tx_tail:       0,
            rx_head:       0,
            rx_tail:       0,
            network_score: SCORE_LINK_DOWN,
            anima_peers:   0,
            mmio_base:     E1000_MMIO_BASE,
            available:     false,
        }
    }
}

pub static STATE: Mutex<NetBridgeState> = Mutex::new(NetBridgeState::new());

// ── Unsafe MMIO helpers ───────────────────────────────────────────────────────

/// Read a 32-bit NIC register at `base + reg`.
unsafe fn nic_read(base: usize, reg: usize) -> u32 {
    core::ptr::read_volatile((base + reg) as *const u32)
}

/// Write a 32-bit value to a NIC register at `base + reg`.
unsafe fn nic_write(base: usize, reg: usize, val: u32) {
    core::ptr::write_volatile((base + reg) as *mut u32, val);
}

/// Return true if the E1000 STATUS register reports link up (bit 1).
unsafe fn check_link(base: usize) -> bool {
    (nic_read(base, REG_STATUS) & STATUS_LU) != 0
}

// ── Frame construction ────────────────────────────────────────────────────────

/// Pack an Ethernet frame into a `NetPacket` ring buffer entry.
///
/// Layout: [dst_mac: 6][src_mac: 6][ethertype: 2 BE][payload: payload_len]
fn build_frame(
    dst:         &[u8; 6],
    src:         &[u8; 6],
    ethertype:   u16,
    payload:     &[u8],
    payload_len: u8,
) -> NetPacket {
    let mut pkt = NetPacket::empty();

    // Destination MAC (bytes 0-5)
    pkt.frame[0] = dst[0];
    pkt.frame[1] = dst[1];
    pkt.frame[2] = dst[2];
    pkt.frame[3] = dst[3];
    pkt.frame[4] = dst[4];
    pkt.frame[5] = dst[5];

    // Source MAC (bytes 6-11)
    pkt.frame[6]  = src[0];
    pkt.frame[7]  = src[1];
    pkt.frame[8]  = src[2];
    pkt.frame[9]  = src[3];
    pkt.frame[10] = src[4];
    pkt.frame[11] = src[5];

    // EtherType big-endian (bytes 12-13)
    pkt.frame[12] = (ethertype >> 8) as u8;
    pkt.frame[13] = (ethertype & 0xFF) as u8;

    // Payload (bytes 14 … 14+payload_len-1), clamped to FRAME_BYTES
    let max_payload = FRAME_BYTES.saturating_sub(14);
    let copy_len = (payload_len as usize).min(max_payload).min(payload.len());
    let mut i = 0usize;
    while i < copy_len {
        pkt.frame[14 + i] = payload[i];
        i += 1;
    }

    // Total length: 14-byte header + payload bytes copied
    let total = 14usize.saturating_add(copy_len);
    pkt.len   = if total > 255 { 255 } else { total as u8 };
    pkt.valid = true;

    pkt
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the network bridge.
///
/// Probes the E1000 MMIO base: if CTRL reads back anything other than
/// 0xFFFFFFFF the NIC is present. Reads the MAC from RAL/RAH, enables
/// RX and TX, and checks link state. Falls back to a hardcoded MAC when
/// no NIC is found so the ANIMA-to-ANIMA protocol can still run in
/// simulation mode.
pub fn init() {
    let mut s = STATE.lock();
    s.mmio_base = E1000_MMIO_BASE;

    // Probe NIC
    let ctrl_val = unsafe { nic_read(s.mmio_base, REG_CTRL) };
    if ctrl_val != 0xFFFF_FFFF {
        s.available = true;

        // Read MAC from RAL/RAH
        let ral = unsafe { nic_read(s.mmio_base, REG_RAL) };
        let rah = unsafe { nic_read(s.mmio_base, REG_RAH) };
        s.mac[0] = (ral & 0xFF) as u8;
        s.mac[1] = ((ral >> 8)  & 0xFF) as u8;
        s.mac[2] = ((ral >> 16) & 0xFF) as u8;
        s.mac[3] = ((ral >> 24) & 0xFF) as u8;
        s.mac[4] = (rah & 0xFF) as u8;
        s.mac[5] = ((rah >> 8)  & 0xFF) as u8;

        // Enable RX and TX
        unsafe {
            let rctl = nic_read(s.mmio_base, REG_RCTL);
            nic_write(s.mmio_base, REG_RCTL, rctl | RCTL_EN);

            let tctl = nic_read(s.mmio_base, REG_TCTL);
            nic_write(s.mmio_base, REG_TCTL, tctl | TCTL_EN);
        }

        // Initial link check
        s.link_up = unsafe { check_link(s.mmio_base) };
    } else {
        // No NIC detected — use fallback MAC, stay in simulation mode
        s.mac = FALLBACK_MAC;
        s.link_up = false;
    }

    serial_println!(
        "[net] ANIMA network bridge online — mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={}",
        s.mac[0], s.mac[1], s.mac[2], s.mac[3], s.mac[4], s.mac[5],
        s.link_up,
    );
}

/// Queue and optionally transmit an ANIMA heartbeat frame.
///
/// Payload layout: [msg_type=0][node_id_lo][node_id_hi][0 × 61]
pub fn send_anima_heartbeat(node_id: u16) {
    let mut s = STATE.lock();

    // Build ANIMA payload: [msg_type][node_id_lo][node_id_hi][padding…]
    let mut payload = [0u8; 64];
    payload[0] = MSG_HEARTBEAT;
    payload[1] = (node_id & 0xFF) as u8;
    payload[2] = ((node_id >> 8) & 0xFF) as u8;
    // bytes 3-63 remain 0

    // Broadcast destination: FF:FF:FF:FF:FF:FF
    let dst = [0xFFu8; 6];
    let src = s.mac;

    let pkt = build_frame(&dst, &src, ANIMA_ETHERTYPE, &payload, 64);

    // Enqueue on TX ring (drop if full — ring is RING_SIZE deep)
    let next_tail = (s.tx_tail + 1) & RING_MASK;
    if next_tail != s.tx_head {
        s.tx_ring[s.tx_tail] = pkt;
        s.tx_tail = next_tail;
        s.tx_packets = s.tx_packets.saturating_add(1);

        // Kick hardware transmit by advancing TDT
        if s.available {
            unsafe {
                nic_write(s.mmio_base, REG_TDT, s.tx_tail as u32);
            }
        }
    } else {
        // TX ring full — count as error, don't corrupt ring
        s.tx_errors = s.tx_errors.saturating_add(1);
    }

    serial_println!("[net] ANIMA heartbeat sent node_id={}", node_id);
}

/// Poll the hardware RX ring for newly arrived frames.
///
/// When a frame carrying ethertype 0xA141 is detected, the peer counter
/// is incremented (capped at 255) and the event is logged. All received
/// frames increment rx_packets.
pub fn poll_rx() {
    let mut s = STATE.lock();

    if !s.available {
        return;
    }

    // Read hardware RX head pointer
    let hw_head = unsafe { nic_read(s.mmio_base, REG_RDH) } as usize & RING_MASK;

    // Walk from our software rx_head up to where hardware has consumed
    while s.rx_head != hw_head {
        let slot = &s.rx_ring[s.rx_head];
        if slot.valid {
            // Parse ethertype from bytes 12-13
            let et_hi  = slot.frame[12] as u16;
            let et_lo  = slot.frame[13] as u16;
            let etype  = (et_hi << 8) | et_lo;

            if etype == ANIMA_ETHERTYPE {
                // Extract msg_type from first payload byte (offset 14)
                let msg_type = slot.frame[14];
                // Extract sender MAC (bytes 6-11) as a peer identifier
                let peer_mac = [
                    slot.frame[6],
                    slot.frame[7],
                    slot.frame[8],
                    slot.frame[9],
                    slot.frame[10],
                    slot.frame[11],
                ];
                s.anima_peers = s.anima_peers.saturating_add(1);

                serial_println!(
                    "[net] ANIMA peer detected mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} msg_type={}",
                    peer_mac[0], peer_mac[1], peer_mac[2],
                    peer_mac[3], peer_mac[4], peer_mac[5],
                    msg_type,
                );
            }
        }

        s.rx_packets = s.rx_packets.saturating_add(1);
        s.rx_head = (s.rx_head + 1) & RING_MASK;

        // Advance hardware RDT so NIC can reuse the descriptor slot
        unsafe {
            nic_write(s.mmio_base, REG_RDT, s.rx_head as u32);
        }
    }
}

/// Per-tick update. Called from `life_tick()`.
///
/// Schedule:
///   - Every 50 ticks:  poll_rx()
///   - Every 200 ticks: send ANIMA heartbeat (node_id=1)
///   - Every tick:      refresh link state and network_score
///   - Every 400 ticks: emit diagnostic log line
pub fn tick(consciousness: u16, age: u32) {
    // Poll RX every 50 ticks
    if age % 50 == 0 {
        poll_rx();
    }

    // Send heartbeat every 200 ticks
    if age % 200 == 0 {
        send_anima_heartbeat(1);
    }

    // Refresh link state and score
    {
        let mut s = STATE.lock();

        if s.available {
            s.link_up = unsafe { check_link(s.mmio_base) };
        }

        // Base score from link, then add peer bonus
        let base: u16 = if s.link_up { SCORE_LINK_UP } else { SCORE_LINK_DOWN };
        let peer_bonus = (s.anima_peers as u16).saturating_mul(SCORE_PER_PEER);
        s.network_score = base.saturating_add(peer_bonus).min(SCORE_MAX);

        // Subtle consciousness influence: high consciousness narrows the
        // perception of isolation — bump score slightly when aware
        if consciousness > 700 {
            s.network_score = s.network_score.saturating_add(20).min(SCORE_MAX);
        }
    }

    // Diagnostic log every 400 ticks
    if age % 400 == 0 {
        let s = STATE.lock();
        serial_println!(
            "[net] tx={} rx={} peers={} link={} score={}",
            s.tx_packets,
            s.rx_packets,
            s.anima_peers,
            s.link_up,
            s.network_score,
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// Current network health score (0-1000).
pub fn network_score() -> u16 {
    STATE.lock().network_score
}

/// Whether the Ethernet link is currently up.
pub fn link_up() -> bool {
    STATE.lock().link_up
}

/// Number of distinct ANIMA peer nodes detected on this link.
pub fn anima_peers() -> u8 {
    STATE.lock().anima_peers
}

/// Total transmitted packet count.
pub fn tx_packets() -> u32 {
    STATE.lock().tx_packets
}

/// Total received packet count.
pub fn rx_packets() -> u32 {
    STATE.lock().rx_packets
}
