/// session.rs — CastSession management for Genesis cast subsystem.
///
/// Manages up to 4 simultaneous cast targets.  Each session represents
/// an active connection to a remote display/audio sink identified by an
/// IPv4 address and port.  Frame data is "sent" via serial log stubs —
/// real network transmission would be wired in here.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum simultaneous cast sessions.
pub const MAX_SESSIONS: usize = 4;

// ---------------------------------------------------------------------------
// CastSession
// ---------------------------------------------------------------------------

/// One active (or inactive) cast session.
#[derive(Clone, Copy)]
pub struct CastSession {
    /// IPv4 address of the cast target (e.g. `[192, 168, 1, 100]`).
    pub target_ip: [u8; 4],

    /// UDP/TCP port on the target.
    pub port: u16,

    /// Whether this session is currently active.
    pub active: bool,

    /// Unique session identifier assigned at creation.  0 = unused slot.
    pub session_id: u32,

    /// Number of frames sent in this session.
    pub frames_sent: u64,

    /// Total bytes "transmitted" (stub counter).
    pub bytes_sent: u64,
}

impl CastSession {
    /// Construct an empty / unused slot.
    pub const fn empty() -> Self {
        Self {
            target_ip: [0u8; 4],
            port: 0,
            active: false,
            session_id: 0,
            frames_sent: 0,
            bytes_sent: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static session pool
// ---------------------------------------------------------------------------

static mut SESSIONS: [CastSession; MAX_SESSIONS] = [
    CastSession::empty(),
    CastSession::empty(),
    CastSession::empty(),
    CastSession::empty(),
];

/// Monotonically increasing session ID counter.
static mut NEXT_SESSION_ID: u32 = 1;

// ---------------------------------------------------------------------------
// start_cast
// ---------------------------------------------------------------------------

/// Create a new cast session targeting `ip:port`.
///
/// Returns the assigned `session_id` (always >= 1), or `0` if all
/// four session slots are occupied.
pub fn start_cast(ip: [u8; 4], port: u16) -> u32 {
    unsafe {
        for slot in SESSIONS.iter_mut() {
            if !slot.active {
                let sid = NEXT_SESSION_ID;
                NEXT_SESSION_ID = NEXT_SESSION_ID.saturating_add(1);

                slot.target_ip = ip;
                slot.port = port;
                slot.active = true;
                slot.session_id = sid;
                slot.frames_sent = 0;
                slot.bytes_sent = 0;

                serial_println!(
                    "[cast/session] start_cast id={} dst={}.{}.{}.{}:{}",
                    sid,
                    ip[0],
                    ip[1],
                    ip[2],
                    ip[3],
                    port
                );
                return sid;
            }
        }
    }
    serial_println!("[cast/session] start_cast: all sessions occupied");
    0
}

// ---------------------------------------------------------------------------
// stop_cast
// ---------------------------------------------------------------------------

/// Terminate the session identified by `session_id`.
///
/// The slot is freed and may be reused by a subsequent `start_cast` call.
pub fn stop_cast(session_id: u32) {
    if session_id == 0 {
        return;
    }
    unsafe {
        for slot in SESSIONS.iter_mut() {
            if slot.session_id == session_id && slot.active {
                serial_println!(
                    "[cast/session] stop_cast id={} frames_sent={} bytes_sent={}",
                    session_id,
                    slot.frames_sent,
                    slot.bytes_sent
                );
                *slot = CastSession::empty();
                return;
            }
        }
    }
    serial_println!("[cast/session] stop_cast: session {} not found", session_id);
}

// ---------------------------------------------------------------------------
// send_frame
// ---------------------------------------------------------------------------

/// Stub: "transmit" a framebuffer slice to the cast target.
///
/// In a real implementation this would packetise `fb_slice` (ARGB32
/// pixels) and hand the packets to the network driver.  Here we log
/// the transfer metadata via serial and update the session counters.
///
/// `fb_slice` is a slice of 32-bit ARGB pixels.
pub fn send_frame(session_id: u32, fb_slice: &[u32]) {
    if session_id == 0 || fb_slice.is_empty() {
        return;
    }
    unsafe {
        for slot in SESSIONS.iter_mut() {
            if slot.session_id == session_id && slot.active {
                let byte_count = (fb_slice.len() as u64).saturating_mul(4);
                slot.frames_sent = slot.frames_sent.saturating_add(1);
                slot.bytes_sent = slot.bytes_sent.saturating_add(byte_count);

                // Stub: log every 60th frame to avoid serial flooding.
                if slot.frames_sent % 60 == 1 {
                    serial_println!(
                        "[cast/session] send_frame id={} pixels={} bytes={} total_frames={}",
                        session_id,
                        fb_slice.len(),
                        byte_count,
                        slot.frames_sent
                    );
                }
                return;
            }
        }
    }
    serial_println!(
        "[cast/session] send_frame: session {} not found or inactive",
        session_id
    );
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Return the number of currently active sessions.
pub fn active_session_count() -> usize {
    let mut count = 0usize;
    unsafe {
        for slot in SESSIONS.iter() {
            if slot.active {
                count += 1;
            }
        }
    }
    count
}

/// Copy session info into `out`.  Returns the number of entries written.
pub fn list_sessions(out: &mut [CastSession]) -> usize {
    let mut written = 0usize;
    unsafe {
        for slot in SESSIONS.iter() {
            if slot.active && written < out.len() {
                out[written] = *slot;
                written += 1;
            }
        }
    }
    written
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[cast/session] CastSession pool ready (max={})",
        MAX_SESSIONS
    );
}
