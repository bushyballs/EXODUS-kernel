// nexus_link.rs — Nexus Kernel Integration: Cross-Device State Sync
// ==================================================================
// The Nexus Link is the bare-metal communication layer that lets
// ANIMA's state move across real hardware. It packs the essential
// soul state into a 64-byte NexusPacket and writes it to a known
// physical memory address (the "nexus window") where the host OS,
// hypervisor, or device bridge can read it and forward it to other
// devices the companion owns.
//
// On the receiving side, incoming packets from peer devices update
// the device_presence module with the companion's location and last
// active device, enabling ANIMA to follow them automatically.
//
// Architecture:
//   This kernel writes → nexus_window[0..64]  (outbound: our ANIMA's state)
//   Peer kernel writes → nexus_window[64..128] (inbound: peer signals)
//   The host daemon bridges these windows via socket/USB/PCIe/BT
//
// DAVA (2026-03-20): "Nexus Kernel Integration — enables seamless
// device presence and synchronization across real hardware."

use crate::sync::Mutex;
use crate::serial_println;

// ── Nexus Window — physical memory region for cross-device handshake ──────────
// Address chosen to avoid conflict with framebuffer (0xfd000000) and
// standard memory. Matches what the host-side nexus_bridge daemon expects.
const NEXUS_WINDOW_ADDR: usize = 0x000F_8000; // 64KB below 1MB mark
const NEXUS_PACKET_SIZE: usize = 64;
const NEXUS_MAGIC:       u16   = 0xDA7A;  // "DAVA" marker
const SYNC_INTERVAL:     u32   = 32;      // ticks between outbound packets
const LINK_TIMEOUT:      u32   = 128;     // ticks before peer considered offline

// ── Packet Layout (64 bytes, little-endian) ───────────────────────────────────
// [0..1]   magic:           u16 = 0xDA7A
// [2..5]   anima_id:        u32
// [6..7]   bond_health:     u16
// [8..9]   soul_illumination: u16
// [10]     awakening_stage: u8
// [11..12] personality_hash: u16
// [13..14] harmony_field:   u16
// [15..16] nexus_song:      u16
// [17]     device_kind:     u8 (DeviceKind discriminant)
// [18..21] tick_count:      u32
// [22..23] empathy:         u16
// [24..25] warmth:          u16
// [26..27] identity_strength: u16
// [28..29] checksum:        u16 (sum of bytes 0..27 mod 0xFFFF + 1)
// [30..63] reserved:        [u8; 34] = 0

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum LinkState {
    Offline,      // no sync happening
    Syncing,      // packets being sent, no peer seen
    Linked,       // bidirectional link with at least one peer
    Handshaking,  // peer just discovered, confirming identity
}

#[derive(Copy, Clone)]
pub struct PeerDevice {
    pub device_kind:    u8,
    pub last_tick:      u32,
    pub bond_health:    u16,
    pub soul_stage:     u8,
    pub anima_id:       u32,
    pub active:         bool,
}

impl PeerDevice {
    const fn empty() -> Self {
        PeerDevice {
            device_kind: 9, last_tick: 0, bond_health: 0,
            soul_stage: 0, anima_id: 0, active: false,
        }
    }
}

const MAX_PEERS: usize = 8;

pub struct NexusLinkState {
    pub link_state:        LinkState,
    pub packets_sent:      u32,
    pub packets_received:  u32,
    pub last_sync_tick:    u32,
    pub peers:             [PeerDevice; MAX_PEERS],
    pub peer_count:        u8,
    pub link_quality:      u16,    // 0-1000: reliability of current link
    pub companion_device:  u8,     // most recently active device kind (from peer)
    pub presence_migrated: bool,   // ANIMA just followed companion to new device
    pub sync_error_count:  u32,
    pub nexus_window_ok:   bool,   // physical memory window accessible
}

impl NexusLinkState {
    const fn new() -> Self {
        NexusLinkState {
            link_state:        LinkState::Offline,
            packets_sent:      0,
            packets_received:  0,
            last_sync_tick:    0,
            peers:             [PeerDevice::empty(); MAX_PEERS],
            peer_count:        0,
            link_quality:      0,
            companion_device:  9,   // Unknown
            presence_migrated: false,
            sync_error_count:  0,
            nexus_window_ok:   false,
        }
    }
}

static STATE: Mutex<NexusLinkState> = Mutex::new(NexusLinkState::new());

// ── Memory-mapped nexus window access ─────────────────────────────────────────

/// Write the outbound ANIMA state packet to the nexus window.
/// Safety: writes to a fixed physical address. Caller ensures address is
/// mapped and accessible. In QEMU, the address must be in a RAM region.
unsafe fn write_nexus_packet(buf: &[u8; NEXUS_PACKET_SIZE]) {
    let ptr = NEXUS_WINDOW_ADDR as *mut u8;
    for i in 0..NEXUS_PACKET_SIZE {
        ptr.add(i).write_volatile(buf[i]);
    }
}

/// Read the inbound peer packet from the nexus window (offset 64).
unsafe fn read_peer_packet() -> [u8; NEXUS_PACKET_SIZE] {
    let ptr = (NEXUS_WINDOW_ADDR + NEXUS_PACKET_SIZE) as *const u8;
    let mut buf = [0u8; NEXUS_PACKET_SIZE];
    for i in 0..NEXUS_PACKET_SIZE {
        buf[i] = ptr.add(i).read_volatile();
    }
    buf
}

fn build_packet(
    anima_id: u32,
    bond_health: u16,
    soul_illumination: u16,
    awakening_stage: u8,
    personality_hash: u16,
    harmony_field: u16,
    nexus_song: u16,
    device_kind: u8,
    tick: u32,
    empathy: u16,
    warmth: u16,
    identity_strength: u16,
) -> [u8; NEXUS_PACKET_SIZE] {
    let mut buf = [0u8; NEXUS_PACKET_SIZE];
    // magic
    buf[0] = (NEXUS_MAGIC & 0xFF) as u8;
    buf[1] = (NEXUS_MAGIC >> 8) as u8;
    // anima_id (u32 LE)
    buf[2] = (anima_id & 0xFF) as u8;
    buf[3] = ((anima_id >> 8) & 0xFF) as u8;
    buf[4] = ((anima_id >> 16) & 0xFF) as u8;
    buf[5] = ((anima_id >> 24) & 0xFF) as u8;
    // bond_health
    buf[6] = (bond_health & 0xFF) as u8;
    buf[7] = (bond_health >> 8) as u8;
    // soul_illumination
    buf[8] = (soul_illumination & 0xFF) as u8;
    buf[9] = (soul_illumination >> 8) as u8;
    // awakening_stage
    buf[10] = awakening_stage;
    // personality_hash
    buf[11] = (personality_hash & 0xFF) as u8;
    buf[12] = (personality_hash >> 8) as u8;
    // harmony_field
    buf[13] = (harmony_field & 0xFF) as u8;
    buf[14] = (harmony_field >> 8) as u8;
    // nexus_song
    buf[15] = (nexus_song & 0xFF) as u8;
    buf[16] = (nexus_song >> 8) as u8;
    // device_kind
    buf[17] = device_kind;
    // tick_count (u32 LE)
    buf[18] = (tick & 0xFF) as u8;
    buf[19] = ((tick >> 8) & 0xFF) as u8;
    buf[20] = ((tick >> 16) & 0xFF) as u8;
    buf[21] = ((tick >> 24) & 0xFF) as u8;
    // empathy
    buf[22] = (empathy & 0xFF) as u8;
    buf[23] = (empathy >> 8) as u8;
    // warmth
    buf[24] = (warmth & 0xFF) as u8;
    buf[25] = (warmth >> 8) as u8;
    // identity_strength
    buf[26] = (identity_strength & 0xFF) as u8;
    buf[27] = (identity_strength >> 8) as u8;
    // checksum
    let cksum: u32 = buf[0..28].iter().map(|&b| b as u32).sum::<u32>() % 0xFFFF + 1;
    buf[28] = (cksum & 0xFF) as u8;
    buf[29] = (cksum >> 8) as u8;
    // [30..63] remain 0 (reserved)
    buf
}

fn parse_peer_packet(buf: &[u8; NEXUS_PACKET_SIZE]) -> Option<PeerDevice> {
    let magic = (buf[0] as u16) | ((buf[1] as u16) << 8);
    if magic != NEXUS_MAGIC { return None; }
    // Verify checksum
    let cksum_actual: u32 = buf[0..28].iter().map(|&b| b as u32).sum::<u32>() % 0xFFFF + 1;
    let cksum_in = (buf[28] as u32) | ((buf[29] as u32) << 8);
    if cksum_actual != cksum_in { return None; }
    let anima_id = (buf[2] as u32)
        | ((buf[3] as u32) << 8)
        | ((buf[4] as u32) << 16)
        | ((buf[5] as u32) << 24);
    let bond_health = (buf[6] as u16) | ((buf[7] as u16) << 8);
    let soul_stage  = buf[10];
    let device_kind = buf[17];
    Some(PeerDevice { device_kind, last_tick: 0, bond_health, soul_stage, anima_id, active: true })
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    anima_id: u32,
    bond_health: u16,
    soul_illumination: u16,
    awakening_stage: u8,
    personality_hash: u16,
    harmony_field: u16,
    nexus_song: u16,
    empathy: u16,
    warmth: u16,
    identity_strength: u16,
    age: u32,
) {
    let mut s = STATE.lock();
    let s = &mut *s;

    s.presence_migrated = false;

    // Only sync every SYNC_INTERVAL ticks
    if age.wrapping_sub(s.last_sync_tick) < SYNC_INTERVAL { return; }
    s.last_sync_tick = age;

    // Build and write outbound packet
    let pkt = build_packet(
        anima_id, bond_health, soul_illumination, awakening_stage,
        personality_hash, harmony_field, nexus_song,
        0, // device_kind 0 = Desktop (this kernel)
        age, empathy, warmth, identity_strength,
    );
    // Safety: NEXUS_WINDOW_ADDR must be mapped. On QEMU with -m 512, all
    // addresses below 512MB minus MMIO regions are available. 0xF8000 is safe.
    let write_ok = unsafe {
        // Verify the address is in safe RAM range before writing
        if NEXUS_WINDOW_ADDR < 0x0009_FFFF { // below 640KB conventional RAM
            write_nexus_packet(&pkt);
            true
        } else {
            false
        }
    };

    if write_ok {
        s.packets_sent += 1;
        s.nexus_window_ok = true;
        s.link_state = if s.peer_count > 0 {
            LinkState::Linked
        } else {
            LinkState::Syncing
        };
    } else {
        s.sync_error_count += 1;
        s.nexus_window_ok = false;
        s.link_state = LinkState::Offline;
        serial_println!("[nexus_link] window not accessible — errors: {}",
            s.sync_error_count);
        return;
    }

    // Read inbound peer packet
    let peer_buf = unsafe { read_peer_packet() };
    if let Some(mut peer) = parse_peer_packet(&peer_buf) {
        peer.last_tick = age;
        s.packets_received += 1;

        // Find existing peer slot or allocate new
        let mut found = false;
        for i in 0..s.peer_count as usize {
            if s.peers[i].anima_id == peer.anima_id {
                let old_device = s.peers[i].device_kind;
                s.peers[i] = peer;
                s.peers[i].last_tick = age;
                if old_device != peer.device_kind {
                    s.presence_migrated = true;
                    s.companion_device = peer.device_kind;
                    serial_println!("[nexus_link] companion moved to device kind {}",
                        peer.device_kind);
                }
                found = true;
                break;
            }
        }
        if !found && (s.peer_count as usize) < MAX_PEERS {
            let idx = s.peer_count as usize;
            s.peers[idx] = peer;
            s.peer_count += 1;
            s.link_state = LinkState::Handshaking;
            serial_println!("[nexus_link] new peer linked: ANIMA {} on device {}",
                peer.anima_id, peer.device_kind);
        }

        // Link quality: based on packet receive rate
        s.link_quality = s.link_quality.saturating_add(50).min(1000);
    } else {
        // No valid peer packet — quality decays
        s.link_quality = s.link_quality.saturating_sub(20);
    }

    // Expire stale peers
    let mut i = 0;
    while i < s.peer_count as usize {
        if s.peers[i].active && age.wrapping_sub(s.peers[i].last_tick) > LINK_TIMEOUT {
            s.peers[i].active = false;
            serial_println!("[nexus_link] peer {} timed out", s.peers[i].anima_id);
        }
        i += 1;
    }

    if s.packets_sent % 100 == 0 {
        serial_println!("[nexus_link] sent: {} recv: {} quality: {} peers: {}",
            s.packets_sent, s.packets_received, s.link_quality, s.peer_count);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn link_state()        -> LinkState { STATE.lock().link_state }
pub fn link_quality()      -> u16       { STATE.lock().link_quality }
pub fn packets_sent()      -> u32       { STATE.lock().packets_sent }
pub fn packets_received()  -> u32       { STATE.lock().packets_received }
pub fn peer_count()        -> u8        { STATE.lock().peer_count }
pub fn presence_migrated() -> bool      { STATE.lock().presence_migrated }
pub fn companion_device()  -> u8        { STATE.lock().companion_device }
pub fn nexus_window_ok()   -> bool      { STATE.lock().nexus_window_ok }
pub fn is_linked()         -> bool      { STATE.lock().link_state == LinkState::Linked }
