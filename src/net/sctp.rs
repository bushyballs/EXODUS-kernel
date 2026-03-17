use crate::net::NetworkDriver;
/// Stream Control Transmission Protocol (RFC 4960)
///
/// Multi-homed, multi-streamed reliable transport. Fixed-size, no-heap
/// implementation using static arrays. No Vec, Box, String, or alloc::*.
///
/// Supports: association setup (4-way handshake), DATA/SACK/HEARTBEAT/
/// SHUTDOWN chunks, CRC32c checksum, multi-streaming, send/recv ring queues.
///
/// Inspired by: RFC 4960, Linux SCTP. All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const IPPROTO_SCTP: u8 = 132;
pub const SCTP_FD_BASE: i32 = 5000;

// Chunk types
pub const SCTP_DATA: u8 = 0;
pub const SCTP_INIT: u8 = 1;
pub const SCTP_INIT_ACK: u8 = 2;
pub const SCTP_SACK: u8 = 3;
pub const SCTP_HEARTBEAT: u8 = 4;
pub const SCTP_HEARTBEAT_ACK: u8 = 5;
pub const SCTP_ABORT: u8 = 6;
pub const SCTP_SHUTDOWN: u8 = 7;
pub const SCTP_SHUTDOWN_ACK: u8 = 8;
pub const SCTP_ERROR: u8 = 9;
pub const SCTP_COOKIE_ECHO: u8 = 10;
pub const SCTP_COOKIE_ACK: u8 = 11;
pub const SCTP_SHUTDOWN_COMPLETE: u8 = 14;

/// Maximum number of concurrent SCTP associations
const MAX_SCTP_ASSOCS: usize = 16;

/// Receive data ring: slots per association
const RX_SLOTS: usize = 16;

/// Maximum data size per RX slot (bytes)
const RX_SLOT_SIZE: usize = 1024;

/// Maximum outbound packet scratch buffer size
const TX_BUF_SIZE: usize = 1500;

// ---------------------------------------------------------------------------
// CRC32c (Castagnoli) lookup table — precomputed, no float
// ---------------------------------------------------------------------------

const CRC32C_TABLE: [u32; 256] = [
    0x00000000, 0xF26B8303, 0xE13B70F7, 0x1350F3F4, 0xC79A971F, 0x35F1141C, 0x26A1E7E8, 0xD4CA64EB,
    0x8AD958CF, 0x78B2DBCC, 0x6BE22838, 0x9989AB3B, 0x4D43CFD0, 0xBF284CD3, 0xAC78BF27, 0x5E133C24,
    0x105EC76F, 0xE235446C, 0xF165B798, 0x030E349B, 0xD7C45070, 0x25AFD373, 0x36FF2087, 0xC494A384,
    0x9A879FA0, 0x68EC1CA3, 0x7BBCEF57, 0x89D76C54, 0x5D1D08BF, 0xAF768BBC, 0xBC267848, 0x4E4DFB4B,
    0x20BD8EDE, 0xD2D60DDD, 0xC186FE29, 0x33ED7D2A, 0xE72719C1, 0x154C9AC2, 0x061C6936, 0xF477EA35,
    0xAA64D611, 0x580F5512, 0x4B5FA6E6, 0xB93425E5, 0x6DFE410E, 0x9F95C20D, 0x8CC531F9, 0x7EAEB2FA,
    0x30E349B1, 0xC288CAB2, 0xD1D83946, 0x23B3BA45, 0xF779DEAE, 0x05125DAD, 0x1642AE59, 0xE4292D5A,
    0xBA3A117E, 0x4851927D, 0x5B016189, 0xA96AE28A, 0x7DA08661, 0x8FCB0562, 0x9C9BF696, 0x6EF07595,
    0x417B1DBC, 0xB3109EBF, 0xA0406D4B, 0x0522BEEE, 0xD6E18A05, 0x248A0906, 0x37DAFAF2, 0xC5B179F1,
    0x9BA245D5, 0x69C9C6D6, 0x7A993522, 0x88F2B621, 0x5C38D2CA, 0xAE5351C9, 0xBD03A23D, 0x4F68213E,
    0x0125DA75, 0xF34E5976, 0xE01EAA82, 0x12752981, 0xC6BF4D6A, 0x34D4CE69, 0x27843D9D, 0xD5EFBE9E,
    0x8BFC82BA, 0x799701B9, 0x6AC7F24D, 0x98AC714E, 0x4C6615A5, 0xBE0D96A6, 0xAD5D6552, 0x5F36E651,
    0x31C693C4, 0xC3AD10C7, 0xD0FDE333, 0x22966030, 0xF65C04DB, 0x043787D8, 0x1767742C, 0xE50CF72F,
    0xBB1FCB0B, 0x49744808, 0x5A24BBFC, 0xA84F38FF, 0x7C855C14, 0x8EEEDF17, 0x9DBE2CE3, 0x6FD5AFE0,
    0x219254AB, 0xD3F9D7A8, 0xC0A9245C, 0x32C2A75F, 0xE608C3B4, 0x146340B7, 0x0733B343, 0xF5583040,
    0xAB4B0C64, 0x59208F67, 0x4A707C93, 0xB81BFF90, 0x6CD19B7B, 0x9EBA1878, 0x8DEAEB8C, 0x7F81688F,
    0x82F63B78, 0x709DB87B, 0x63CD4B8F, 0x91A6C88C, 0x456CAC67, 0xB7072F64, 0xA457DC90, 0x563C5F93,
    0x082F63B7, 0xFA44E0B4, 0xE9141340, 0x1B7F9043, 0xCFB5F4A8, 0x3DDE77AB, 0x2E8E845F, 0xDCE5075C,
    0x92A8FC17, 0x60C37F14, 0x73938CE0, 0x81F80FE3, 0x55326B08, 0xA759E80B, 0xB4091BFF, 0x466298FC,
    0x1871A4D8, 0xEA1A27DB, 0xF94AD42F, 0x0B21572C, 0xDFEB33C7, 0x2D80B0C4, 0x3ED04330, 0xCCBBC033,
    0xA24BB5A6, 0x502036A5, 0x4370C551, 0xB11B4652, 0x65D122B9, 0x97BAA1BA, 0x84EA524E, 0x7681D14D,
    0x2892ED69, 0xDAF96E6A, 0xC9A99D9E, 0x3BC21E9D, 0xEF087A76, 0x1D63F975, 0x0E330A81, 0xFC588982,
    0xB21572C9, 0x407EF1CA, 0x532E023E, 0xA145813D, 0x758FE5D6, 0x87E466D5, 0x94B49521, 0x66DF1622,
    0x38CC2A06, 0xCAA7A905, 0xD9F75AF1, 0x2B9CD9F2, 0xFF56BD19, 0x0D3D3E1A, 0x1E6DCDEE, 0xEC064EED,
    0xC38D26C4, 0x31E6A5C7, 0x22B65633, 0xD0DDD530, 0x0417B1DB, 0xF67C32D8, 0xE52CC12C, 0x1747422F,
    0x49547E0B, 0xBB3FFD08, 0xA86F0EFC, 0x5A048DFF, 0x8ECEE914, 0x7CA56A17, 0x6FF599E3, 0x9D9E1AE0,
    0xD3D3E1AB, 0x21B862A8, 0x32E8915C, 0xC083125F, 0x144976B4, 0xE622F5B7, 0xF5720643, 0x07198540,
    0x590AB964, 0xAB613A67, 0xB831C993, 0x4A5A4A90, 0x9E902E7B, 0x6CFBAD78, 0x7FAB5E8C, 0x8DC0DD8F,
    0xE330A81A, 0x115B2B19, 0x020BD8ED, 0xF0605BEE, 0x24AA3F05, 0xD6C1BC06, 0xC5914FF2, 0x37FACCF1,
    0x69E9F0D5, 0x9B8273D6, 0x88D28022, 0x7AB90321, 0xAE7367CA, 0x5C18E4C9, 0x4F48173D, 0xBD23943E,
    0xF36E6F75, 0x0105EC76, 0x12551F82, 0xE03E9C81, 0x34F4F86A, 0xC69F7B69, 0xD5CF889D, 0x27A40B9E,
    0x79B737BA, 0x8BDCB4B9, 0x988C474D, 0x6AE7C44E, 0xBE2DA0A5, 0x4C4623A6, 0x5F16D052, 0xAD7D5351,
];

/// Compute CRC32c checksum over a byte slice.
pub fn crc32c(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    let mut i = 0;
    while i < data.len() {
        let idx = ((crc ^ (data[i] as u32)) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32C_TABLE[idx];
        i = i.saturating_add(1);
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// SCTP packet structures (repr(C, packed) for wire format)
// ---------------------------------------------------------------------------

/// SCTP common header — 12 bytes
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct SctpHdr {
    pub sport: u16,
    pub dport: u16,
    pub vtag: u32,
    pub checksum: u32,
}

/// SCTP chunk header — 4 bytes
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct SctpChunkHdr {
    pub chunk_type: u8,
    pub flags: u8,
    pub length: u16, // big-endian, includes header
}

/// SCTP DATA chunk header — 16 bytes total
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct SctpDataHdr {
    pub chunk: SctpChunkHdr,
    pub tsn: u32,
    pub stream_id: u16,
    pub stream_seq: u16,
    pub ppid: u32,
    // variable data follows
}

// ---------------------------------------------------------------------------
// Association state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum SctpState {
    Closed,
    CookieWait,
    CookieEchoed,
    Established,
    ShutdownPending,
    ShutdownSent,
    ShutdownReceived,
    ShutdownAckSent,
}

// ---------------------------------------------------------------------------
// Association record — no heap, fixed ring buffers
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct SctpAssoc {
    pub fd: i32,
    pub state: SctpState,
    pub local_port: u16,
    pub remote_port: u16,
    pub remote_ip: [u8; 4],
    pub local_vtag: u32,
    pub remote_vtag: u32,
    pub local_tsn: u32,
    pub remote_tsn: u32,
    pub streams_out: u16,
    pub streams_in: u16,
    // RX ring
    pub rx_data: [[u8; RX_SLOT_SIZE]; RX_SLOTS],
    pub rx_lens: [u16; RX_SLOTS],
    pub rx_head: u8,
    pub rx_tail: u8,
    pub active: bool,
    // Heartbeat
    pub hb_timeout_ms: u32,
    pub last_hb_ms: u64,
    // Stats
    pub rx_packets: u64,
    pub tx_packets: u64,
}

impl SctpAssoc {
    pub const fn empty() -> Self {
        SctpAssoc {
            fd: -1,
            state: SctpState::Closed,
            local_port: 0,
            remote_port: 0,
            remote_ip: [0; 4],
            local_vtag: 0,
            remote_vtag: 0,
            local_tsn: 0,
            remote_tsn: 0,
            streams_out: 0,
            streams_in: 0,
            rx_data: [[0u8; RX_SLOT_SIZE]; RX_SLOTS],
            rx_lens: [0u16; RX_SLOTS],
            rx_head: 0,
            rx_tail: 0,
            active: false,
            hb_timeout_ms: 30_000,
            last_hb_ms: 0,
            rx_packets: 0,
            tx_packets: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SCTP_ASSOCS: Mutex<[SctpAssoc; MAX_SCTP_ASSOCS]> =
    Mutex::new([SctpAssoc::empty(); MAX_SCTP_ASSOCS]);

static NEXT_SCTP_VTAG: AtomicU32 = AtomicU32::new(0x1234_5678);

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // Seed vtag counter with random value if possible
    let seed = crate::crypto::random::random_u32();
    NEXT_SCTP_VTAG.store(seed | 1, Ordering::Relaxed);
    serial_println!(
        "  Net: SCTP subsystem initialized (no-heap, {} assocs)",
        MAX_SCTP_ASSOCS
    );
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find assoc index by fd
fn find_by_fd(assocs: &[SctpAssoc; MAX_SCTP_ASSOCS], fd: i32) -> Option<usize> {
    let mut i = 0;
    while i < MAX_SCTP_ASSOCS {
        if assocs[i].active && assocs[i].fd == fd {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Find assoc index by (local_port, remote_port, vtag)
fn find_by_tuple(
    assocs: &[SctpAssoc; MAX_SCTP_ASSOCS],
    local_port: u16,
    remote_port: u16,
    vtag: u32,
) -> Option<usize> {
    let mut i = 0;
    while i < MAX_SCTP_ASSOCS {
        if assocs[i].active
            && assocs[i].local_port == local_port
            && assocs[i].remote_port == remote_port
            && assocs[i].local_vtag == vtag
        {
            return Some(i);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Get the primary interface MAC and IP needed to send (no-heap).
fn get_iface() -> Option<([u8; 6], [u8; 4])> {
    let mut snaps = [super::IfaceSnapshot::empty(); 1];
    let count = super::snapshot_interfaces(&mut snaps);
    if count == 0 || !snaps[0].valid {
        return None;
    }
    let ip = snaps[0].ip;
    if ip == [0u8; 4] {
        return None;
    }
    Some((snaps[0].mac, ip))
}

/// Compute and write SCTP CRC32c into bytes 8..12 of a packet slice.
///
/// Per RFC 4960: checksum field is zeroed before computing.
fn finalize_crc(pkt: &mut [u8]) {
    if pkt.len() < 12 {
        return;
    }
    // Zero checksum field (bytes 8..12)
    pkt[8] = 0;
    pkt[9] = 0;
    pkt[10] = 0;
    pkt[11] = 0;
    let cksum = crc32c(pkt);
    pkt[8] = (cksum >> 24) as u8;
    pkt[9] = (cksum >> 16) as u8;
    pkt[10] = (cksum >> 8) as u8;
    pkt[11] = cksum as u8;
}

/// Write an SctpHdr into a buffer at offset 0 (big-endian on wire).
fn write_sctp_hdr(buf: &mut [u8; TX_BUF_SIZE], pos: usize, hdr: &SctpHdr) -> usize {
    if pos.saturating_add(12) > TX_BUF_SIZE {
        return pos;
    }
    let sp = hdr.sport.to_be_bytes();
    let dp = hdr.dport.to_be_bytes();
    let vt = hdr.vtag.to_be_bytes();
    let ck = hdr.checksum.to_be_bytes();
    buf[pos] = sp[0];
    buf[pos + 1] = sp[1];
    buf[pos + 2] = dp[0];
    buf[pos + 3] = dp[1];
    buf[pos + 4] = vt[0];
    buf[pos + 5] = vt[1];
    buf[pos + 6] = vt[2];
    buf[pos + 7] = vt[3];
    buf[pos + 8] = ck[0];
    buf[pos + 9] = ck[1];
    buf[pos + 10] = ck[2];
    buf[pos + 11] = ck[3];
    pos.saturating_add(12)
}

/// Write a chunk header into buf at pos. Returns new pos.
fn write_chunk_hdr(
    buf: &mut [u8; TX_BUF_SIZE],
    pos: usize,
    typ: u8,
    flags: u8,
    length: u16,
) -> usize {
    if pos.saturating_add(4) > TX_BUF_SIZE {
        return pos;
    }
    let lb = length.to_be_bytes();
    buf[pos] = typ;
    buf[pos + 1] = flags;
    buf[pos + 2] = lb[0];
    buf[pos + 3] = lb[1];
    pos.saturating_add(4)
}

/// Write a u32 big-endian into buf at pos. Returns new pos.
fn write_u32(buf: &mut [u8; TX_BUF_SIZE], pos: usize, val: u32) -> usize {
    if pos.saturating_add(4) > TX_BUF_SIZE {
        return pos;
    }
    let b = val.to_be_bytes();
    buf[pos] = b[0];
    buf[pos + 1] = b[1];
    buf[pos + 2] = b[2];
    buf[pos + 3] = b[3];
    pos.saturating_add(4)
}

/// Write a u16 big-endian into buf at pos. Returns new pos.
fn write_u16(buf: &mut [u8; TX_BUF_SIZE], pos: usize, val: u16) -> usize {
    if pos.saturating_add(2) > TX_BUF_SIZE {
        return pos;
    }
    let b = val.to_be_bytes();
    buf[pos] = b[0];
    buf[pos + 1] = b[1];
    pos.saturating_add(2)
}

/// Copy data bytes into buf at pos. Returns new pos.
fn write_bytes(buf: &mut [u8; TX_BUF_SIZE], pos: usize, data: &[u8]) -> usize {
    let avail = TX_BUF_SIZE.saturating_sub(pos);
    let n = data.len().min(avail);
    let mut i = 0;
    while i < n {
        buf[pos + i] = data[i];
        i = i.saturating_add(1);
    }
    pos.saturating_add(n)
}

/// Build and send an IPv4+SCTP packet using the raw NIC driver.
/// `pkt` is the SCTP common header + chunks (no IP/Ethernet header).
fn send_sctp_raw(dst_ip: [u8; 4], pkt: &mut [u8], pkt_len: usize) {
    use super::arp;
    use super::ethernet;
    use super::ipv4;
    use super::{Ipv4Addr, MacAddr};

    let (src_mac_arr, src_ip_arr) = match get_iface() {
        Some(v) => v,
        None => return,
    };
    let src_ip = Ipv4Addr(src_ip_arr);
    let dst_ip_addr = Ipv4Addr(dst_ip);
    let src_mac = MacAddr(src_mac_arr);
    let dst_mac = arp::lookup(dst_ip_addr).unwrap_or(MacAddr::BROADCAST);

    // Finalize CRC32c checksum in the SCTP header
    if pkt_len >= 12 {
        finalize_crc(&mut pkt[..pkt_len]);
    }

    // Build IPv4 header
    let ip_hdr = ipv4::build_header(
        src_ip,
        dst_ip_addr,
        IPPROTO_SCTP,
        pkt_len as u16,
        ipv4::DEFAULT_TTL,
    );
    let ip_bytes =
        unsafe { core::slice::from_raw_parts(&ip_hdr as *const ipv4::Ipv4Header as *const u8, 20) };

    // Assemble Ethernet frame into a fixed-size stack buffer
    let mut frame = [0u8; 1514];
    // Ethernet header
    frame[0..6].copy_from_slice(&dst_mac.0);
    frame[6..12].copy_from_slice(&src_mac.0);
    let et = ethernet::ETHERTYPE_IPV4.to_be_bytes();
    frame[12] = et[0];
    frame[13] = et[1];
    // IPv4
    let mut off = 14;
    let mut k = 0;
    while k < 20 && off < 1514 {
        frame[off] = ip_bytes[k];
        off = off.saturating_add(1);
        k = k.saturating_add(1);
    }
    // SCTP payload
    k = 0;
    while k < pkt_len && off < 1514 {
        frame[off] = pkt[k];
        off = off.saturating_add(1);
        k = k.saturating_add(1);
    }
    let frame_len = off.max(60);

    let driver = crate::drivers::e1000::driver().lock();
    if let Some(ref nic) = *driver {
        let _ = nic.send(&frame[..frame_len]);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new SCTP association. Returns fd >= SCTP_FD_BASE, or -1 on error.
pub fn sctp_socket() -> i32 {
    let mut assocs = SCTP_ASSOCS.lock();
    let mut i = 0;
    while i < MAX_SCTP_ASSOCS {
        if !assocs[i].active {
            assocs[i] = SctpAssoc::empty();
            assocs[i].active = true;
            assocs[i].fd = SCTP_FD_BASE + i as i32;
            return assocs[i].fd;
        }
        i = i.saturating_add(1);
    }
    -1
}

/// Bind a local port to an association. Returns 0 on success, -1 on error.
pub fn sctp_bind(fd: i32, port: u16) -> i32 {
    let mut assocs = SCTP_ASSOCS.lock();
    match find_by_fd(&*assocs, fd) {
        Some(idx) => {
            assocs[idx].local_port = port;
            0
        }
        None => -1,
    }
}

/// Connect to a remote SCTP endpoint.
/// Sends INIT, spins for INIT_ACK, then COOKIE_ECHO, then waits for COOKIE_ACK.
/// Returns 0 on success, -1 on error.
pub fn sctp_connect(fd: i32, dst_ip: [u8; 4], dst_port: u16) -> i32 {
    // Generate random vtag and initial TSN
    let local_vtag = NEXT_SCTP_VTAG.fetch_add(1, Ordering::Relaxed);
    let local_tsn = crate::crypto::random::random_u32();

    {
        let mut assocs = SCTP_ASSOCS.lock();
        let idx = match find_by_fd(&*assocs, fd) {
            Some(i) => i,
            None => return -1,
        };
        assocs[idx].local_vtag = local_vtag;
        assocs[idx].local_tsn = local_tsn;
        assocs[idx].remote_ip = dst_ip;
        assocs[idx].remote_port = dst_port;
        assocs[idx].streams_out = 1;
        assocs[idx].streams_in = 1;
        assocs[idx].state = SctpState::CookieWait;
    }

    // Build INIT chunk
    // INIT body: initiate_tag(4), a_rwnd(4), out_streams(2), in_streams(2), init_tsn(4) = 16 bytes
    // Chunk header: 4 bytes. Total chunk = 20 bytes.
    // SCTP common header: 12 bytes. Total packet = 32 bytes.
    let local_port = {
        let assocs = SCTP_ASSOCS.lock();
        match find_by_fd(&*assocs, fd) {
            Some(i) => assocs[i].local_port,
            None => return -1,
        }
    };

    let mut pkt = [0u8; TX_BUF_SIZE];
    let sctp_hdr = SctpHdr {
        sport: local_port,
        dport: dst_port,
        vtag: 0,
        checksum: 0,
    };
    let mut pos = write_sctp_hdr(&mut pkt, 0, &sctp_hdr);
    // INIT chunk header: type=1, flags=0, length=20
    pos = write_chunk_hdr(&mut pkt, pos, SCTP_INIT, 0, 20);
    pos = write_u32(&mut pkt, pos, local_vtag); // Initiate Tag
    pos = write_u32(&mut pkt, pos, 65535); // a_rwnd
    pos = write_u16(&mut pkt, pos, 1); // out streams
    pos = write_u16(&mut pkt, pos, 1); // in streams
    pos = write_u32(&mut pkt, pos, local_tsn); // Initial TSN
    let pkt_len = pos;

    send_sctp_raw(dst_ip, &mut pkt, pkt_len);

    // Spin-wait up to 3s for INIT_ACK (state transition done in sctp_input)
    let mut spin = 0u32;
    while spin < 3_000_000 {
        {
            let assocs = SCTP_ASSOCS.lock();
            if let Some(i) = find_by_fd(&*assocs, fd) {
                if assocs[i].state != SctpState::CookieWait {
                    break;
                }
            }
        }
        core::hint::spin_loop();
        spin = spin.wrapping_add(1);
    }

    // If now CookieEchoed, wait for Established
    spin = 0;
    while spin < 3_000_000 {
        {
            let assocs = SCTP_ASSOCS.lock();
            if let Some(i) = find_by_fd(&*assocs, fd) {
                match assocs[i].state {
                    SctpState::Established => return 0,
                    SctpState::Closed
                    | SctpState::ShutdownPending
                    | SctpState::ShutdownSent
                    | SctpState::ShutdownReceived
                    | SctpState::ShutdownAckSent => return -1,
                    _ => {}
                }
            } else {
                return -1;
            }
        }
        core::hint::spin_loop();
        spin = spin.wrapping_add(1);
    }
    -1
}

/// Send data on an established SCTP association.
/// Returns bytes sent (>= 0) or negative errno on error.
pub fn sctp_send(fd: i32, data: &[u8], stream: u16) -> isize {
    let (local_port, remote_port, remote_ip, tsn, stream_seq, remote_vtag) = {
        let mut assocs = SCTP_ASSOCS.lock();
        let idx = match find_by_fd(&*assocs, fd) {
            Some(i) => i,
            None => return -9, // EBADF
        };
        if assocs[idx].state != SctpState::Established {
            return -107; // ENOTCONN
        }
        let tsn = assocs[idx].local_tsn;
        assocs[idx].local_tsn = assocs[idx].local_tsn.wrapping_add(1);
        assocs[idx].tx_packets = assocs[idx].tx_packets.saturating_add(1);
        (
            assocs[idx].local_port,
            assocs[idx].remote_port,
            assocs[idx].remote_ip,
            tsn,
            0u16,
            assocs[idx].remote_vtag,
        )
    };

    // DATA chunk: 4 (chunk hdr) + 12 (tsn+sid+seq+ppid) + data
    let data_len = data.len().min(TX_BUF_SIZE.saturating_sub(12 + 4 + 12));
    let chunk_len = (4 + 12 + data_len) as u16;

    let mut pkt = [0u8; TX_BUF_SIZE];
    let sctp_hdr = SctpHdr {
        sport: local_port,
        dport: remote_port,
        vtag: remote_vtag,
        checksum: 0,
    };
    let mut pos = write_sctp_hdr(&mut pkt, 0, &sctp_hdr);
    // DATA chunk header: flags=0x03 (B+E bits = unfragmented)
    pos = write_chunk_hdr(&mut pkt, pos, SCTP_DATA, 0x03, chunk_len);
    pos = write_u32(&mut pkt, pos, tsn);
    pos = write_u16(&mut pkt, pos, stream);
    pos = write_u16(&mut pkt, pos, stream_seq);
    pos = write_u32(&mut pkt, pos, 0); // ppid
    pos = write_bytes(&mut pkt, pos, &data[..data_len]);
    // Pad to 4-byte boundary
    while pos & 3 != 0 && pos < TX_BUF_SIZE {
        pkt[pos] = 0;
        pos = pos.saturating_add(1);
    }
    let pkt_len = pos;

    send_sctp_raw(remote_ip, &mut pkt, pkt_len);
    data_len as isize
}

/// Receive data from an SCTP association (non-blocking).
/// Returns bytes received or negative errno on error.
pub fn sctp_recv(fd: i32, buf: &mut [u8; 1024]) -> isize {
    let mut assocs = SCTP_ASSOCS.lock();
    let idx = match find_by_fd(&*assocs, fd) {
        Some(i) => i,
        None => return -9, // EBADF
    };

    let head = assocs[idx].rx_head as usize;
    let tail = assocs[idx].rx_tail as usize;
    if head == tail {
        return -11; // EAGAIN
    }

    let slot = head & (RX_SLOTS - 1);
    let len = assocs[idx].rx_lens[slot] as usize;
    let copy_len = len.min(1024);
    let mut i = 0;
    while i < copy_len {
        buf[i] = assocs[idx].rx_data[slot][i];
        i = i.saturating_add(1);
    }
    assocs[idx].rx_head = ((head.saturating_add(1)) & (RX_SLOTS - 1)) as u8;
    copy_len as isize
}

/// Close an SCTP association, sending SHUTDOWN.
pub fn sctp_close(fd: i32) -> i32 {
    let (remote_ip, local_port, remote_port, remote_vtag, cum_tsn) = {
        let mut assocs = SCTP_ASSOCS.lock();
        let idx = match find_by_fd(&*assocs, fd) {
            Some(i) => i,
            None => return -1,
        };
        let state = assocs[idx].state;
        let ri = assocs[idx].remote_ip;
        let lp = assocs[idx].local_port;
        let rp = assocs[idx].remote_port;
        let rv = assocs[idx].remote_vtag;
        let ct = assocs[idx].remote_tsn;
        if state == SctpState::Established || state == SctpState::ShutdownPending {
            assocs[idx].state = SctpState::ShutdownSent;
        } else {
            assocs[idx].state = SctpState::Closed;
            assocs[idx].active = false;
            return 0;
        }
        (ri, lp, rp, rv, ct)
    };

    // Send SHUTDOWN chunk: cumulative_tsn_ack (4 bytes), chunk total = 8
    let mut pkt = [0u8; TX_BUF_SIZE];
    let sctp_hdr = SctpHdr {
        sport: local_port,
        dport: remote_port,
        vtag: remote_vtag,
        checksum: 0,
    };
    let mut pos = write_sctp_hdr(&mut pkt, 0, &sctp_hdr);
    pos = write_chunk_hdr(&mut pkt, pos, SCTP_SHUTDOWN, 0, 8);
    pos = write_u32(&mut pkt, pos, cum_tsn);
    send_sctp_raw(remote_ip, &mut pkt, pos);

    // Mark closed
    let mut assocs = SCTP_ASSOCS.lock();
    if let Some(idx) = find_by_fd(&*assocs, fd) {
        assocs[idx].state = SctpState::Closed;
        assocs[idx].active = false;
    }
    0
}

/// Returns true if fd is an SCTP fd.
pub fn sctp_is_fd(fd: i32) -> bool {
    if fd < SCTP_FD_BASE {
        return false;
    }
    let assocs = SCTP_ASSOCS.lock();
    find_by_fd(&*assocs, fd).is_some()
}

/// Process an incoming SCTP packet. Called from the IP receive path.
pub fn sctp_input(packet: &[u8], len: usize, src_ip: [u8; 4]) {
    if len < 12 {
        return;
    }
    let packet = &packet[..len];

    // Parse SCTP common header
    let sport = u16::from_be_bytes([packet[0], packet[1]]);
    let dport = u16::from_be_bytes([packet[2], packet[3]]);
    let vtag = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);

    // Verify CRC32c
    let mut pkt_copy = [0u8; TX_BUF_SIZE];
    let copy_len = len.min(TX_BUF_SIZE);
    let mut ci = 0;
    while ci < copy_len {
        pkt_copy[ci] = packet[ci];
        ci = ci.saturating_add(1);
    }
    // Zero checksum field before verification
    pkt_copy[8] = 0;
    pkt_copy[9] = 0;
    pkt_copy[10] = 0;
    pkt_copy[11] = 0;
    let computed = crc32c(&pkt_copy[..copy_len]);
    let wire_cksum = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);
    if computed != wire_cksum {
        return; // CRC mismatch — drop
    }

    // Find matching association (dport = our local port, sport = remote port)
    let assoc_idx = {
        let assocs = SCTP_ASSOCS.lock();
        // INIT chunk (vtag=0 in common header) — find by local port only
        find_by_tuple(&*assocs, dport, sport, vtag)
    };

    // Walk chunks
    let mut off: usize = 12;
    while off.saturating_add(4) <= len {
        let chunk_type = packet[off];
        let chunk_flags = packet[off + 1];
        let chunk_len = u16::from_be_bytes([packet[off + 2], packet[off + 3]]) as usize;
        if chunk_len < 4 || off.saturating_add(chunk_len) > len {
            break;
        }
        let chunk_data = &packet[off + 4..off + chunk_len];

        match chunk_type {
            SCTP_INIT_ACK => {
                // Find assoc in CookieWait state matching local/remote port
                let mut assocs = SCTP_ASSOCS.lock();
                let mut i = 0;
                while i < MAX_SCTP_ASSOCS {
                    if assocs[i].active
                        && assocs[i].local_port == dport
                        && assocs[i].remote_port == sport
                        && assocs[i].state == SctpState::CookieWait
                    {
                        if chunk_data.len() >= 4 {
                            assocs[i].remote_vtag = u32::from_be_bytes([
                                chunk_data[0],
                                chunk_data[1],
                                chunk_data[2],
                                chunk_data[3],
                            ]);
                        }
                        assocs[i].state = SctpState::CookieEchoed;
                        // Send COOKIE_ECHO (empty cookie for simplicity — 4 byte header only)
                        let fd = assocs[i].fd;
                        let lp = assocs[i].local_port;
                        let rp = assocs[i].remote_port;
                        let rv = assocs[i].remote_vtag;
                        let ri = assocs[i].remote_ip;
                        drop(assocs);
                        let mut pkt = [0u8; TX_BUF_SIZE];
                        let hdr = SctpHdr {
                            sport: lp,
                            dport: rp,
                            vtag: rv,
                            checksum: 0,
                        };
                        let mut pos = write_sctp_hdr(&mut pkt, 0, &hdr);
                        // COOKIE_ECHO with 4-byte dummy cookie
                        pos = write_chunk_hdr(&mut pkt, pos, SCTP_COOKIE_ECHO, 0, 8);
                        pos = write_u32(&mut pkt, pos, 0xC001E000u32); // dummy cookie
                        send_sctp_raw(ri, &mut pkt, pos);
                        break;
                    }
                    i = i.saturating_add(1);
                }
            }
            SCTP_COOKIE_ACK => {
                let mut assocs = SCTP_ASSOCS.lock();
                let mut i = 0;
                while i < MAX_SCTP_ASSOCS {
                    if assocs[i].active
                        && assocs[i].local_port == dport
                        && assocs[i].remote_port == sport
                        && assocs[i].state == SctpState::CookieEchoed
                    {
                        assocs[i].state = SctpState::Established;
                        break;
                    }
                    i = i.saturating_add(1);
                }
            }
            SCTP_DATA => {
                if chunk_data.len() < 12 {
                    break;
                }
                let tsn = u32::from_be_bytes([
                    chunk_data[0],
                    chunk_data[1],
                    chunk_data[2],
                    chunk_data[3],
                ]);
                let payload = &chunk_data[12..]; // after TSN(4)+SID(2)+SSN(2)+PPID(4)

                let mut assocs = SCTP_ASSOCS.lock();
                if let Some(idx) = assoc_idx {
                    if assocs[idx].state == SctpState::Established {
                        assocs[idx].remote_tsn = tsn;
                        assocs[idx].rx_packets = assocs[idx].rx_packets.saturating_add(1);

                        // Enqueue into ring
                        let tail = assocs[idx].rx_tail as usize;
                        let next_tail = (tail.saturating_add(1)) & (RX_SLOTS - 1);
                        if next_tail != assocs[idx].rx_head as usize {
                            let copy_len = payload.len().min(RX_SLOT_SIZE);
                            let mut pi = 0;
                            while pi < copy_len {
                                assocs[idx].rx_data[tail][pi] = payload[pi];
                                pi = pi.saturating_add(1);
                            }
                            assocs[idx].rx_lens[tail] = copy_len as u16;
                            assocs[idx].rx_tail = next_tail as u8;
                        }

                        // Send SACK
                        let lp = assocs[idx].local_port;
                        let rp = assocs[idx].remote_port;
                        let rv = assocs[idx].remote_vtag;
                        let ri = assocs[idx].remote_ip;
                        drop(assocs);
                        let mut pkt = [0u8; TX_BUF_SIZE];
                        let hdr = SctpHdr {
                            sport: lp,
                            dport: rp,
                            vtag: rv,
                            checksum: 0,
                        };
                        let mut pos = write_sctp_hdr(&mut pkt, 0, &hdr);
                        // SACK: cum_tsn_ack(4) + a_rwnd(4) + num_gap(2) + num_dup(2) = 12 body
                        pos = write_chunk_hdr(&mut pkt, pos, SCTP_SACK, 0, 16);
                        pos = write_u32(&mut pkt, pos, tsn); // cum tsn ack
                        pos = write_u32(&mut pkt, pos, 65535); // a_rwnd
                        pos = write_u16(&mut pkt, pos, 0); // no gap blocks
                        pos = write_u16(&mut pkt, pos, 0); // no dup tsns
                        send_sctp_raw(ri, &mut pkt, pos);
                    }
                }
            }
            SCTP_HEARTBEAT => {
                // Reflect HEARTBEAT_ACK with same chunk data
                if let Some(idx) = assoc_idx {
                    let (lp, rp, rv, ri) = {
                        let assocs = SCTP_ASSOCS.lock();
                        (
                            assocs[idx].local_port,
                            assocs[idx].remote_port,
                            assocs[idx].remote_vtag,
                            assocs[idx].remote_ip,
                        )
                    };
                    let body_len = chunk_data.len().min(TX_BUF_SIZE.saturating_sub(12 + 4));
                    let chunk_total = (4 + body_len) as u16;
                    let mut pkt = [0u8; TX_BUF_SIZE];
                    let hdr = SctpHdr {
                        sport: lp,
                        dport: rp,
                        vtag: rv,
                        checksum: 0,
                    };
                    let mut pos = write_sctp_hdr(&mut pkt, 0, &hdr);
                    pos = write_chunk_hdr(&mut pkt, pos, SCTP_HEARTBEAT_ACK, 0, chunk_total);
                    pos = write_bytes(&mut pkt, pos, &chunk_data[..body_len]);
                    send_sctp_raw(ri, &mut pkt, pos);
                }
            }
            SCTP_SHUTDOWN => {
                if let Some(idx) = assoc_idx {
                    let (lp, rp, rv, ri) = {
                        let mut assocs = SCTP_ASSOCS.lock();
                        assocs[idx].state = SctpState::ShutdownReceived;
                        (
                            assocs[idx].local_port,
                            assocs[idx].remote_port,
                            assocs[idx].remote_vtag,
                            assocs[idx].remote_ip,
                        )
                    };
                    // Send SHUTDOWN_ACK
                    let mut pkt = [0u8; TX_BUF_SIZE];
                    let hdr = SctpHdr {
                        sport: lp,
                        dport: rp,
                        vtag: rv,
                        checksum: 0,
                    };
                    let mut pos = write_sctp_hdr(&mut pkt, 0, &hdr);
                    pos = write_chunk_hdr(&mut pkt, pos, SCTP_SHUTDOWN_ACK, 0, 4);
                    send_sctp_raw(ri, &mut pkt, pos);
                    let mut assocs = SCTP_ASSOCS.lock();
                    assocs[idx].state = SctpState::ShutdownAckSent;
                }
            }
            SCTP_SHUTDOWN_COMPLETE => {
                if let Some(idx) = assoc_idx {
                    let mut assocs = SCTP_ASSOCS.lock();
                    assocs[idx].state = SctpState::Closed;
                    assocs[idx].active = false;
                }
            }
            SCTP_ABORT => {
                if let Some(idx) = assoc_idx {
                    let mut assocs = SCTP_ASSOCS.lock();
                    assocs[idx].state = SctpState::Closed;
                    assocs[idx].active = false;
                }
            }
            _ => {} // Ignore unknown chunk types
        }

        // Advance to next chunk (4-byte aligned)
        let padded = (chunk_len.saturating_add(3)) & !3;
        off = off.saturating_add(padded);
    }
}

/// Periodic heartbeat tick — send HEARTBEATs to established associations.
pub fn sctp_heartbeat_tick(current_ms: u64) {
    let mut sends: [(bool, u16, u16, [u8; 4], u32); MAX_SCTP_ASSOCS] =
        [(false, 0, 0, [0; 4], 0); MAX_SCTP_ASSOCS];

    {
        let mut assocs = SCTP_ASSOCS.lock();
        let mut i = 0;
        while i < MAX_SCTP_ASSOCS {
            if assocs[i].active && assocs[i].state == SctpState::Established {
                let elapsed = current_ms.saturating_sub(assocs[i].last_hb_ms);
                if elapsed >= assocs[i].hb_timeout_ms as u64 {
                    assocs[i].last_hb_ms = current_ms;
                    sends[i] = (
                        true,
                        assocs[i].local_port,
                        assocs[i].remote_port,
                        assocs[i].remote_ip,
                        assocs[i].remote_vtag,
                    );
                }
            }
            i = i.saturating_add(1);
        }
    }

    let mut i = 0;
    while i < MAX_SCTP_ASSOCS {
        if sends[i].0 {
            let (_, lp, rp, ri, rv) = sends[i];
            // HEARTBEAT with 8-byte info TLV (type=1, len=8, timestamp=current_ms lower 4 bytes)
            let mut pkt = [0u8; TX_BUF_SIZE];
            let hdr = SctpHdr {
                sport: lp,
                dport: rp,
                vtag: rv,
                checksum: 0,
            };
            let mut pos = write_sctp_hdr(&mut pkt, 0, &hdr);
            // HEARTBEAT chunk: 4 hdr + 8 info = 12 total
            pos = write_chunk_hdr(&mut pkt, pos, SCTP_HEARTBEAT, 0, 12);
            // Heartbeat Info TLV: type=1(2), len=8(2), data(4)
            pos = write_u16(&mut pkt, pos, 1);
            pos = write_u16(&mut pkt, pos, 8);
            pos = write_u32(&mut pkt, pos, current_ms as u32);
            send_sctp_raw(ri, &mut pkt, pos);
        }
        i = i.saturating_add(1);
    }
}
