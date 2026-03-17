/// xfrm — IPsec transform framework
///
/// Implements the Security Association (SA) database and policy engine
/// for IPsec ESP and AH processing.
///
/// Design:
///   - SA table: 64 entries, keyed by (SPI, dst_ip, proto)
///   - Policy table: 32 entries (src/dst prefix + SPI selector)
///   - Sequence number anti-replay window: 64-bit bitmap
///   - ESP: encrypt+authenticate (ChaCha20-Poly1305 placeholder)
///   - AH:  authenticate-only (HMAC-SHA256 placeholder)
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const XFRM_MAX_SA: usize = 64;
pub const XFRM_MAX_POLICY: usize = 32;
pub const XFRM_KEY_LEN: usize = 32; // 256-bit keys
pub const XFRM_AUTH_LEN: usize = 32; // 256-bit auth keys

// Protocol identifiers
pub const IPPROTO_ESP: u8 = 50;
pub const IPPROTO_AH: u8 = 51;

// SA direction
pub const XFRM_DIR_IN: u8 = 0;
pub const XFRM_DIR_OUT: u8 = 1;

// SA state
pub const XFRM_SA_VALID: u8 = 1;
pub const XFRM_SA_EXPIRED: u8 = 2;
pub const XFRM_SA_DEAD: u8 = 3;

// ---------------------------------------------------------------------------
// Security Association
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct XfrmSa {
    pub id: u32,
    pub spi: u32,    // Security Parameter Index (network byte order)
    pub dst_ip: u32, // destination IPv4
    pub src_ip: u32, // source IPv4
    pub proto: u8,   // IPPROTO_ESP or IPPROTO_AH
    pub dir: u8,     // XFRM_DIR_IN / OUT
    pub state: u8,   // XFRM_SA_*
    pub enc_key: [u8; XFRM_KEY_LEN],
    pub auth_key: [u8; XFRM_AUTH_LEN],
    pub seq_num: u32,    // outbound: next seq; inbound: last seen seq
    pub replay_win: u64, // 64-bit anti-replay window bitmap
    pub life_pkts: u64,  // packet lifetime counter (hard limit)
    pub life_bytes: u64, // byte lifetime counter (hard limit)
    pub pkts: u64,       // processed packet count
    pub bytes: u64,      // processed byte count
    pub hard_pkt: u64,   // hard packet limit (0 = unlimited)
    pub hard_byte: u64,  // hard byte limit (0 = unlimited)
}

impl XfrmSa {
    pub const fn empty() -> Self {
        XfrmSa {
            id: 0,
            spi: 0,
            dst_ip: 0,
            src_ip: 0,
            proto: 0,
            dir: 0,
            state: 0,
            enc_key: [0u8; XFRM_KEY_LEN],
            auth_key: [0u8; XFRM_AUTH_LEN],
            seq_num: 0,
            replay_win: 0,
            life_pkts: 0,
            life_bytes: 0,
            pkts: 0,
            bytes: 0,
            hard_pkt: 0,
            hard_byte: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Security Policy
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct XfrmPolicy {
    pub id: u32,
    pub src_ip: u32,
    pub src_mask: u8,
    pub dst_ip: u32,
    pub dst_mask: u8,
    pub proto: u8, // IPPROTO_ESP / AH / 0 = any
    pub dir: u8,   // XFRM_DIR_IN / OUT
    pub spi: u32,  // 0 = use first matching SA
    pub valid: bool,
}

impl XfrmPolicy {
    pub const fn empty() -> Self {
        XfrmPolicy {
            id: 0,
            src_ip: 0,
            src_mask: 0,
            dst_ip: 0,
            dst_mask: 0,
            proto: 0,
            dir: 0,
            spi: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

static SA_TABLE: Mutex<[XfrmSa; XFRM_MAX_SA]> = Mutex::new([XfrmSa::empty(); XFRM_MAX_SA]);
static POLICY_TABLE: Mutex<[XfrmPolicy; XFRM_MAX_POLICY]> =
    Mutex::new([XfrmPolicy::empty(); XFRM_MAX_POLICY]);
static SA_NEXT_ID: AtomicU32 = AtomicU32::new(1);
static POL_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// IP prefix match helper
// ---------------------------------------------------------------------------

fn ip_match(addr: u32, prefix: u32, mask_bits: u8) -> bool {
    if mask_bits == 0 {
        return true;
    }
    if mask_bits >= 32 {
        return addr == prefix;
    }
    let mask = !((1u32 << (32 - mask_bits)) - 1);
    (addr & mask) == (prefix & mask)
}

// ---------------------------------------------------------------------------
// SA management
// ---------------------------------------------------------------------------

/// Add a Security Association. Returns SA id or 0 on failure.
pub fn xfrm_sa_add(
    spi: u32,
    src_ip: u32,
    dst_ip: u32,
    proto: u8,
    dir: u8,
    enc_key: &[u8],
    auth_key: &[u8],
    hard_pkt: u64,
    hard_byte: u64,
) -> u32 {
    let mut table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].state == 0 {
            let id = SA_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i] = XfrmSa::empty();
            table[i].id = id;
            table[i].spi = spi;
            table[i].src_ip = src_ip;
            table[i].dst_ip = dst_ip;
            table[i].proto = proto;
            table[i].dir = dir;
            table[i].state = XFRM_SA_VALID;
            table[i].hard_pkt = hard_pkt;
            table[i].hard_byte = hard_byte;
            let ek = enc_key.len().min(XFRM_KEY_LEN);
            let ak = auth_key.len().min(XFRM_AUTH_LEN);
            let mut k = 0usize;
            while k < ek {
                table[i].enc_key[k] = enc_key[k];
                k = k.saturating_add(1);
            }
            let mut k = 0usize;
            while k < ak {
                table[i].auth_key[k] = auth_key[k];
                k = k.saturating_add(1);
            }
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Look up an inbound SA by (SPI, dst_ip, proto).
pub fn xfrm_sa_lookup_in(spi: u32, dst_ip: u32, proto: u8) -> u32 {
    let table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        let sa = &table[i];
        if sa.state == XFRM_SA_VALID
            && sa.spi == spi
            && sa.dst_ip == dst_ip
            && sa.proto == proto
            && sa.dir == XFRM_DIR_IN
        {
            return sa.id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Look up outbound SA by (src_ip, dst_ip, proto).
pub fn xfrm_sa_lookup_out(src_ip: u32, dst_ip: u32, proto: u8) -> u32 {
    let table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        let sa = &table[i];
        if sa.state == XFRM_SA_VALID
            && sa.src_ip == src_ip
            && sa.dst_ip == dst_ip
            && sa.proto == proto
            && sa.dir == XFRM_DIR_OUT
        {
            return sa.id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Delete an SA by id.
pub fn xfrm_sa_del(id: u32) -> bool {
    let mut table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].id == id && table[i].state != 0 {
            table[i] = XfrmSa::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Get SA stats (pkts, bytes). Returns (0,0) if not found.
pub fn xfrm_sa_stats(id: u32) -> (u64, u64) {
    let table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].id == id {
            return (table[i].pkts, table[i].bytes);
        }
        i = i.saturating_add(1);
    }
    (0, 0)
}

// ---------------------------------------------------------------------------
// Anti-replay window check (inbound)
// ---------------------------------------------------------------------------

/// Returns true if seq is acceptable (not replayed, within window).
/// Updates the window if accepted.
fn replay_check(sa: &mut XfrmSa, seq: u32) -> bool {
    let last = sa.seq_num;
    if seq > last {
        // advance window
        let diff = seq - last;
        if diff < 64 {
            sa.replay_win = (sa.replay_win << diff) | 1;
        } else {
            sa.replay_win = 1;
        }
        sa.seq_num = seq;
        return true;
    }
    let diff = last - seq;
    if diff >= 64 {
        return false;
    } // too old
    let bit = 1u64 << diff;
    if sa.replay_win & bit != 0 {
        return false;
    } // already seen
    sa.replay_win |= bit;
    true
}

// ---------------------------------------------------------------------------
// Policy management
// ---------------------------------------------------------------------------

/// Add a policy. Returns policy id or 0 on failure.
pub fn xfrm_policy_add(
    src_ip: u32,
    src_mask: u8,
    dst_ip: u32,
    dst_mask: u8,
    proto: u8,
    dir: u8,
    spi: u32,
) -> u32 {
    let mut table = POLICY_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_POLICY {
        if !table[i].valid {
            let id = POL_NEXT_ID.fetch_add(1, Ordering::Relaxed);
            table[i] = XfrmPolicy {
                id,
                src_ip,
                src_mask,
                dst_ip,
                dst_mask,
                proto,
                dir,
                spi,
                valid: true,
            };
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Delete a policy by id.
pub fn xfrm_policy_del(id: u32) -> bool {
    let mut table = POLICY_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_POLICY {
        if table[i].id == id && table[i].valid {
            table[i] = XfrmPolicy::empty();
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Find the first matching policy for a packet. Returns SPI (0 = no match).
pub fn xfrm_policy_lookup(src_ip: u32, dst_ip: u32, proto: u8, dir: u8) -> u32 {
    let table = POLICY_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_POLICY {
        let p = &table[i];
        if p.valid && p.dir == dir {
            let src_ok = ip_match(src_ip, p.src_ip, p.src_mask);
            let dst_ok = ip_match(dst_ip, p.dst_ip, p.dst_mask);
            let proto_ok = p.proto == 0 || p.proto == proto;
            if src_ok && dst_ok && proto_ok {
                return p.spi;
            }
        }
        i = i.saturating_add(1);
    }
    0
}

// ---------------------------------------------------------------------------
// ESP / AH processing stubs
// ---------------------------------------------------------------------------

/// Process an inbound ESP packet. Returns true if the packet is valid.
///
/// In a full implementation this would:
///   1. Look up SA by (SPI, dst_ip, IPPROTO_ESP)
///   2. Verify HMAC/auth tag
///   3. Decrypt payload
///   4. Anti-replay check
///   5. Strip ESP header/trailer
pub fn xfrm_esp_input(pkt: &mut [u8], pkt_len: usize, dst_ip: u32) -> bool {
    if pkt_len < 12 {
        return false;
    }
    // Extract SPI from first 4 bytes (network byte order)
    let spi = u32::from_be_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]);
    let seq = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);

    let sa_id = xfrm_sa_lookup_in(spi, dst_ip, IPPROTO_ESP);
    if sa_id == 0 {
        return false;
    }

    let mut table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].id == sa_id {
            if !replay_check(&mut table[i], seq) {
                return false;
            }
            table[i].pkts = table[i].pkts.wrapping_add(1);
            table[i].bytes = table[i].bytes.wrapping_add(pkt_len as u64);
            // Lifetime enforcement
            if table[i].hard_pkt != 0 && table[i].pkts >= table[i].hard_pkt {
                table[i].state = XFRM_SA_EXPIRED;
            }
            if table[i].hard_byte != 0 && table[i].bytes >= table[i].hard_byte {
                table[i].state = XFRM_SA_EXPIRED;
            }
            return true; // decryption stub: accept
        }
        i = i.saturating_add(1);
    }
    false
}

/// Build an outbound ESP header into `hdr` (8 bytes: SPI[4] + Seq[4]).
/// Returns the next sequence number, or 0 if no SA found.
pub fn xfrm_esp_output(src_ip: u32, dst_ip: u32, pkt_len: usize, hdr: &mut [u8; 8]) -> u32 {
    let sa_id = xfrm_sa_lookup_out(src_ip, dst_ip, IPPROTO_ESP);
    if sa_id == 0 {
        return 0;
    }

    let mut table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].id == sa_id {
            let seq = table[i].seq_num.wrapping_add(1);
            table[i].seq_num = seq;
            table[i].pkts = table[i].pkts.wrapping_add(1);
            table[i].bytes = table[i].bytes.wrapping_add(pkt_len as u64);
            let spi_b = table[i].spi.to_be_bytes();
            let seq_b = seq.to_be_bytes();
            hdr[0] = spi_b[0];
            hdr[1] = spi_b[1];
            hdr[2] = spi_b[2];
            hdr[3] = spi_b[3];
            hdr[4] = seq_b[0];
            hdr[5] = seq_b[1];
            hdr[6] = seq_b[2];
            hdr[7] = seq_b[3];
            return seq;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Process an inbound AH packet.  Returns true if auth tag is valid.
pub fn xfrm_ah_input(pkt: &mut [u8], pkt_len: usize, dst_ip: u32) -> bool {
    if pkt_len < 12 {
        return false;
    }
    let spi = u32::from_be_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]);
    let seq = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);

    let sa_id = xfrm_sa_lookup_in(spi, dst_ip, IPPROTO_AH);
    if sa_id == 0 {
        return false;
    }

    let mut table = SA_TABLE.lock();
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].id == sa_id {
            if !replay_check(&mut table[i], seq) {
                return false;
            }
            table[i].pkts = table[i].pkts.wrapping_add(1);
            table[i].bytes = table[i].bytes.wrapping_add(pkt_len as u64);
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

pub fn xfrm_sa_count() -> usize {
    let table = SA_TABLE.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < XFRM_MAX_SA {
        if table[i].state != 0 {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

pub fn xfrm_policy_count() -> usize {
    let table = POLICY_TABLE.lock();
    let mut n = 0usize;
    let mut i = 0usize;
    while i < XFRM_MAX_POLICY {
        if table[i].valid {
            n = n.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[xfrm] IPsec transform framework initialized (SA={}, policy={})",
        XFRM_MAX_SA,
        XFRM_MAX_POLICY
    );
}
