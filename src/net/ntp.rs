use crate::serial_println;
/// NTP client for Genesis — RFC 5905 (NTPv3/v4)
///
/// Synchronizes the system wall clock with a remote NTP server over UDP port 123.
/// All state is stored in a fixed-size static; no heap allocation is used.
///
/// Design constraints (bare-metal #![no_std]):
///   - No float casts (as f32 / as f64)
///   - No heap (no Vec / Box / String)
///   - No panic — all errors are handled with Option/Result and early returns
///   - Saturating arithmetic for all counters
///   - wrapping_add for sequence numbers / TSC differences
///
/// NTP epoch: 1900-01-01 00:00:00 UTC
/// Unix epoch: 1970-01-01 00:00:00 UTC
/// Offset: 70 years = 2,208,988,800 seconds
///
/// Reference: RFC 5905 §§7–8.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// NTP server UDP port
pub const NTP_PORT: u16 = 123;

/// Seconds between NTP epoch (1900) and Unix epoch (1970):
/// 70 years × 365.25 days/year × 86 400 s/day ≈ 2 208 988 800 s
pub const NTP_UNIX_OFFSET: u64 = 2_208_988_800;

/// Ephemeral source port used for NTP queries
const NTP_SRC_PORT: u16 = 32768;

/// Spin-poll limit while waiting for a UDP response (~100 000 iterations)
const NTP_RECV_SPIN_MAX: u32 = 100_000;

/// li_vn_mode field for a client request:
///   LI=0 (no leap warning), VN=3 (NTPv3), Mode=3 (client)
const NTP_CLIENT_BYTE: u8 = 0b00_011_011; // 0x1B

/// Mode field mask (lower 3 bits of li_vn_mode)
const NTP_MODE_MASK: u8 = 0x07;
/// Mode=4 → server response
const NTP_MODE_SERVER: u8 = 4;

// ---------------------------------------------------------------------------
// NTP timestamp
// ---------------------------------------------------------------------------

/// 64-bit NTP timestamp: upper 32 bits = seconds since 1900,
/// lower 32 bits = sub-second fraction (1 / 2^32 s per unit).
#[derive(Debug, Clone, Copy, Default)]
pub struct NtpTimestamp {
    pub seconds: u32,
    pub fraction: u32,
}

impl NtpTimestamp {
    /// Convert to Unix seconds (saturating subtraction of the epoch offset).
    pub fn to_unix_secs(&self) -> u64 {
        (self.seconds as u64).saturating_sub(NTP_UNIX_OFFSET)
    }

    /// Approximate sub-second component in milliseconds (0–999).
    /// Uses the top 10 bits of the fraction field; resolution ~1 ms.
    pub fn fraction_to_millis(&self) -> u32 {
        // fraction * 1000 / 2^32  ≈  (fraction >> 22) & 0x3FF
        (self.fraction >> 22) & 0x3FF
    }

    /// Build from a Unix second timestamp (fraction = 0).
    pub fn from_unix_secs(unix_secs: u64) -> Self {
        let ntp_secs = unix_secs.saturating_add(NTP_UNIX_OFFSET);
        NtpTimestamp {
            seconds: ntp_secs as u32,
            fraction: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// NTP packet (48 bytes, RFC 5905 §7.3)
// ---------------------------------------------------------------------------

/// Wire-format NTP packet — 48 bytes, big-endian.
/// `repr(C, packed)` so we can cast to/from `[u8; 48]` directly.
#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct NtpPacket {
    /// LI[7:6] | VN[5:3] | Mode[2:0]
    pub li_vn_mode: u8,
    pub stratum: u8,
    pub poll: u8,
    pub precision: u8, // signed log2 seconds (RFC interprets as i8, we store as u8)
    pub root_delay: u32,
    pub root_dispersion: u32,
    pub ref_id: [u8; 4],
    pub ref_ts: NtpTimestamp,
    pub orig_ts: NtpTimestamp,
    pub recv_ts: NtpTimestamp,
    pub xmit_ts: NtpTimestamp,
}

impl NtpPacket {
    /// Build a client request packet.
    fn new_request(local_unix_secs: u64) -> Self {
        NtpPacket {
            li_vn_mode: NTP_CLIENT_BYTE,
            stratum: 0,
            poll: 6,         // 2^6 = 64 s poll interval
            precision: 0xFA, // -6 as u8 (twos-complement) ≈ 15 ms precision
            root_delay: 0,
            root_dispersion: 0,
            ref_id: [0; 4],
            ref_ts: NtpTimestamp::default(),
            orig_ts: NtpTimestamp::default(),
            recv_ts: NtpTimestamp::default(),
            xmit_ts: NtpTimestamp::from_unix_secs(local_unix_secs),
        }
    }

    /// Serialize to a 48-byte array (all multi-byte fields in network byte order).
    fn to_bytes(&self) -> [u8; 48] {
        let mut buf = [0u8; 48];
        buf[0] = self.li_vn_mode;
        buf[1] = self.stratum;
        buf[2] = self.poll;
        buf[3] = self.precision;
        buf[4..8].copy_from_slice(&self.root_delay.to_be_bytes());
        buf[8..12].copy_from_slice(&self.root_dispersion.to_be_bytes());
        buf[12..16].copy_from_slice(&self.ref_id);
        // ref_ts
        buf[16..20].copy_from_slice(&self.ref_ts.seconds.to_be_bytes());
        buf[20..24].copy_from_slice(&self.ref_ts.fraction.to_be_bytes());
        // orig_ts
        buf[24..28].copy_from_slice(&self.orig_ts.seconds.to_be_bytes());
        buf[28..32].copy_from_slice(&self.orig_ts.fraction.to_be_bytes());
        // recv_ts
        buf[32..36].copy_from_slice(&self.recv_ts.seconds.to_be_bytes());
        buf[36..40].copy_from_slice(&self.recv_ts.fraction.to_be_bytes());
        // xmit_ts
        buf[40..44].copy_from_slice(&self.xmit_ts.seconds.to_be_bytes());
        buf[44..48].copy_from_slice(&self.xmit_ts.fraction.to_be_bytes());
        buf
    }

    /// Deserialize from a 48-byte slice. Returns None if slice is too short.
    fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 48 {
            return None;
        }
        let read_ts = |off: usize| -> NtpTimestamp {
            let secs = u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
            let frac = u32::from_be_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
            NtpTimestamp {
                seconds: secs,
                fraction: frac,
            }
        };
        Some(NtpPacket {
            li_vn_mode: buf[0],
            stratum: buf[1],
            poll: buf[2],
            precision: buf[3],
            root_delay: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            root_dispersion: u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
            ref_id: [buf[12], buf[13], buf[14], buf[15]],
            ref_ts: read_ts(16),
            orig_ts: read_ts(24),
            recv_ts: read_ts(32),
            xmit_ts: read_ts(40),
        })
    }
}

// ---------------------------------------------------------------------------
// NTP client state
// ---------------------------------------------------------------------------

struct NtpState {
    /// NTP server IPv4 address
    server_ip: [u8; 4],
    /// Unix timestamp of last successful sync (seconds)
    last_sync_secs: u64,
    /// Clock offset in milliseconds: positive = our clock is behind server
    offset_ms: i64,
    /// Round-trip time to server in milliseconds
    rtt_ms: u32,
    /// Server stratum (1 = primary reference, 15 = max)
    stratum: u8,
    /// True after at least one successful sync
    synchronized: bool,
}

impl NtpState {
    const fn new() -> Self {
        NtpState {
            server_ip: [216, 239, 35, 0], // time.google.com
            last_sync_secs: 0,
            offset_ms: 0,
            rtt_ms: 0,
            stratum: 0,
            synchronized: false,
        }
    }
}

static NTP_STATE: Mutex<NtpState> = Mutex::new(NtpState::new());

// ---------------------------------------------------------------------------
// Helper: read TSC and convert to milliseconds (for offset arithmetic)
// ---------------------------------------------------------------------------

/// Read the TSC counter.
#[inline]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Convert a TSC delta to milliseconds using the calibrated TSC frequency.
/// Falls back to 3 GHz if calibration has not run.
fn tsc_delta_to_ms(tsc_delta: u64) -> u64 {
    let freq = crate::time::clock::tsc_freq_hz();
    let freq = if freq == 0 { 3_000_000_000u64 } else { freq };
    // ms = delta * 1000 / freq  — avoid overflow: divide freq by 1000 first
    let freq_khz = freq / 1000;
    if freq_khz == 0 {
        return 0;
    }
    tsc_delta / freq_khz
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Set the NTP server by its raw IPv4 address bytes.
pub fn ntp_set_server(ip: [u8; 4]) {
    NTP_STATE.lock().server_ip = ip;
    serial_println!(
        "  NTP: server set to {}.{}.{}.{}",
        ip[0],
        ip[1],
        ip[2],
        ip[3]
    );
}

/// Perform one NTP query-response exchange (RFC 5905 §8).
///
/// Returns the best estimate of the current Unix timestamp in seconds,
/// or `None` on failure (no network, bad response, stratum out of range).
pub fn ntp_query() -> Option<u64> {
    let (server_ip, local_unix) = {
        let state = NTP_STATE.lock();
        let ip = state.server_ip;
        let unix = crate::time::clock::unix_time();
        (ip, unix)
    };

    let server = super::Ipv4Addr(server_ip);

    // ---- Step 1: bind ephemeral receive port --------------------------------
    // Ignore error if already bound
    let _ = super::udp::bind(NTP_SRC_PORT);

    // ---- Step 2: build request and record T1 (transmit time) ----------------
    let req = NtpPacket::new_request(local_unix);
    let t1_unix = local_unix; // seconds (coarse)
    let t1_tsc = rdtsc();

    // ---- Step 3: send UDP ---------------------------------------------------
    let pkt_bytes = req.to_bytes();
    match super::udp::send_to(NTP_SRC_PORT, server, NTP_PORT, &pkt_bytes) {
        Ok(_) => {}
        Err(_) => {
            serial_println!("  NTP: send failed");
            return None;
        }
    }

    // ---- Step 4: spin-poll for response ------------------------------------
    let mut resp_pkt: Option<NtpPacket> = None;
    for _ in 0..NTP_RECV_SPIN_MAX {
        // Drive the NIC receive path so UDP packets are queued
        super::poll();

        if let Some((_src_ip, _src_port, data)) = super::udp::recv(NTP_SRC_PORT) {
            if let Some(pkt) = NtpPacket::from_bytes(&data) {
                resp_pkt = Some(pkt);
                break;
            }
        }
        core::hint::spin_loop();
    }

    // ---- Step 5: validate response -----------------------------------------
    let resp = match resp_pkt {
        Some(p) => p,
        None => {
            serial_println!("  NTP: no response (timeout)");
            return None;
        }
    };

    // Mode must be 4 (server)
    if resp.li_vn_mode & NTP_MODE_MASK != NTP_MODE_SERVER {
        serial_println!("  NTP: unexpected mode {}", resp.li_vn_mode & NTP_MODE_MASK);
        return None;
    }
    // Stratum 1–15 (0 = unspecified, 16+ = unsynchronized)
    if resp.stratum == 0 || resp.stratum > 15 {
        serial_println!("  NTP: invalid stratum {}", resp.stratum);
        return None;
    }

    // ---- Step 6: compute offset and RTT (RFC 5905 §8) ----------------------
    // T1 = origin (when we sent), T2 = server receive, T3 = server transmit,
    // T4 = when we received the response.
    //
    // offset = ((T2 - T1) + (T3 - T4)) / 2
    // rtt    = (T4 - T1) - (T3 - T2)
    //
    // We work in integer seconds at this stage (milliseconds via fraction fields
    // would require wider arithmetic than is safe without floats).
    // For our purposes 1-second resolution is acceptable; the wall clock will
    // be corrected by NTP sync periodically.

    let t4_tsc = rdtsc();
    let elapsed_ms = tsc_delta_to_ms(t4_tsc.wrapping_sub(t1_tsc));
    let t4_unix: i64 = t1_unix as i64 + (elapsed_ms / 1000) as i64;

    let t1: i64 = t1_unix as i64;
    let orig_ts = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(resp.orig_ts)) };
    let xmit_ts = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(resp.xmit_ts)) };
    let recv_ts = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(resp.recv_ts)) };
    let t2: i64 = orig_ts.to_unix_secs() as i64; // orig_ts echoes our xmit_ts
    let t3: i64 = xmit_ts.to_unix_secs() as i64; // server transmit
    let t4: i64 = t4_unix;

    // Include millisecond sub-second component for offset (integer arithmetic)
    let t2_ms = recv_ts.fraction_to_millis() as i64;
    let t3_ms = xmit_ts.fraction_to_millis() as i64;
    let t4_sub_ms = (elapsed_ms % 1000) as i64;

    // offset in ms: ((T2-T1)*1000 + t2_ms - 0) + ((T3-T4)*1000 + t3_ms - t4_sub_ms) / 2
    let d21 = (t2 - t1).saturating_mul(1000).saturating_add(t2_ms);
    let d34 = (t3 - t4)
        .saturating_mul(1000)
        .saturating_add(t3_ms)
        .saturating_sub(t4_sub_ms);
    let offset_ms = d21.saturating_add(d34) / 2;

    // rtt in ms: (T4-T1)*1000 - (T3-T2)*1000
    let rtt_raw = (t4 - t1)
        .saturating_mul(1000)
        .saturating_sub((t3 - t2).saturating_mul(1000));
    let rtt_ms = if rtt_raw > 0 { rtt_raw as u32 } else { 0u32 };

    // ---- Step 7: update state -----------------------------------------------
    {
        let mut state = NTP_STATE.lock();
        state.offset_ms = offset_ms;
        state.rtt_ms = rtt_ms;
        state.stratum = resp.stratum;
        state.synchronized = true;
    }

    serial_println!(
        "  NTP: stratum={} rtt={}ms offset={}ms",
        resp.stratum,
        rtt_ms,
        offset_ms
    );

    // ---- Step 8: return T4 Unix seconds + one-way delay approximation -------
    // One-way delay ≈ rtt / 2 in seconds (coarse)
    let one_way_s = (rtt_ms / 2000) as u64; // rtt_ms / 2 / 1000
    let unix_now = (t4_unix as u64).saturating_add(one_way_s);
    Some(unix_now)
}

/// Synchronise the kernel wall clock with NTP.
/// Returns `true` if the query succeeded and the wall clock was updated.
pub fn ntp_sync() -> bool {
    match ntp_query() {
        Some(unix_secs) => {
            crate::kernel::wallclock::set_wallclock_secs(unix_secs);
            {
                let mut state = NTP_STATE.lock();
                state.last_sync_secs = unix_secs;
            }
            serial_println!("  NTP: wall clock synced to unix={}", unix_secs);
            true
        }
        None => false,
    }
}

/// Get the best-estimate Unix timestamp, using the last sync time plus
/// elapsed monotonic time since that sync.
pub fn get_unix_time() -> u64 {
    crate::kernel::wallclock::get_wallclock_secs()
}

/// True if at least one NTP sync has succeeded.
pub fn is_synchronized() -> bool {
    NTP_STATE.lock().synchronized
}

/// NTP server stratum from the last response (0 if never synced).
pub fn get_stratum() -> u8 {
    NTP_STATE.lock().stratum
}

/// Round-trip time to the NTP server in milliseconds (0 if never synced).
pub fn get_rtt_ms() -> u32 {
    NTP_STATE.lock().rtt_ms
}

/// Clock offset in milliseconds from the last sync
/// (positive = our clock was behind the server).
pub fn get_offset_ms() -> i64 {
    NTP_STATE.lock().offset_ms
}

/// Initialize the NTP client.
/// Sets the default server (time.google.com: 216.239.35.0).
/// Does NOT perform a query — the network stack may not be ready yet.
pub fn init() {
    // Default: time.google.com primary anycast address
    NTP_STATE.lock().server_ip = [216, 239, 35, 0];
    serial_println!(
        "  NTP: client ready (server 216.239.35.0, port {})",
        NTP_PORT
    );
}
