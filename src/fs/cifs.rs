use crate::serial_println;
/// CIFS/SMB2 client stub for Genesis
///
/// Minimal SMB2 client framework for network filesystem access.
/// This is a protocol stub — no actual TCP I/O.  Network stack
/// integration will wire in real transport later.
///
/// Supports: session negotiation, tree connect, file open/close/read/write.
/// All state lives in fixed-size static arrays (no heap).
///
/// Inspired by: Linux CIFS client (fs/cifs), SMB2 RFC 5652. All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

pub const SMB2_MAGIC: u32 = 0xFE534D42; // "\xFESMB"
pub const SMB2_NEGOTIATE: u16 = 0x0000;
pub const SMB2_SESSION_SETUP: u16 = 0x0001;
pub const SMB2_LOGOFF: u16 = 0x0002;
pub const SMB2_TREE_CONNECT: u16 = 0x0003;
pub const SMB2_TREE_DISCONNECT: u16 = 0x0004;
pub const SMB2_CREATE: u16 = 0x0005;
pub const SMB2_CLOSE: u16 = 0x0006;
pub const SMB2_READ: u16 = 0x0008;
pub const SMB2_WRITE: u16 = 0x0009;
pub const SMB2_QUERY_INFO: u16 = 0x0010;

pub const MAX_CIFS_SESSIONS: usize = 4;
pub const MAX_CIFS_SHARES: usize = 16;
pub const MAX_CIFS_HANDLES: usize = 64;
pub const CIFS_MAX_PATH: usize = 256;

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum CifsState {
    Disconnected,
    Negotiating,
    Authenticating,
    Connected,
    Error,
}

#[derive(Copy, Clone)]
pub struct CifsSession {
    pub session_id: u64,
    pub state: CifsState,
    pub server_ip: [u8; 4],
    pub server_port: u16,
    pub username: [u8; 64],
    pub username_len: u8,
    pub dialect: u16,    // 0x0200=SMB2.0, 0x0210=SMB2.1, 0x0300=SMB3.0
    pub message_id: u64, // wrapping sequence number
    pub active: bool,
}

impl CifsSession {
    pub const fn empty() -> Self {
        CifsSession {
            session_id: 0,
            state: CifsState::Disconnected,
            server_ip: [0u8; 4],
            server_port: 0,
            username: [0u8; 64],
            username_len: 0,
            dialect: 0,
            message_id: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct CifsShare {
    pub tree_id: u32,
    pub session_id: u64,
    pub share_name: [u8; 128],
    pub share_len: u8,
    pub share_type: u8, // 1=disk, 2=pipe, 3=print
    pub active: bool,
}

impl CifsShare {
    pub const fn empty() -> Self {
        CifsShare {
            tree_id: 0,
            session_id: 0,
            share_name: [0u8; 128],
            share_len: 0,
            share_type: 0,
            active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct CifsHandle {
    pub file_id: u64,
    pub tree_id: u32,
    pub session_id: u64,
    pub path: [u8; CIFS_MAX_PATH],
    pub path_len: u16,
    pub access_mask: u32,
    pub file_size: u64,
    pub pos: u64,
    pub active: bool,
}

impl CifsHandle {
    pub const fn empty() -> Self {
        CifsHandle {
            file_id: 0,
            tree_id: 0,
            session_id: 0,
            path: [0u8; CIFS_MAX_PATH],
            path_len: 0,
            access_mask: 0,
            file_size: 0,
            pos: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state (no heap — fixed-size static arrays)
// ---------------------------------------------------------------------------

static CIFS_SESSIONS: Mutex<[CifsSession; MAX_CIFS_SESSIONS]> =
    Mutex::new([CifsSession::empty(); MAX_CIFS_SESSIONS]);

static CIFS_SHARES: Mutex<[CifsShare; MAX_CIFS_SHARES]> =
    Mutex::new([CifsShare::empty(); MAX_CIFS_SHARES]);

static CIFS_HANDLES: Mutex<[CifsHandle; MAX_CIFS_HANDLES]> =
    Mutex::new([CifsHandle::empty(); MAX_CIFS_HANDLES]);

/// Monotonically increasing session ID counter (wrapping).
static SESSION_ID_CTR: Mutex<u64> = Mutex::new(1);
/// Monotonically increasing tree ID counter (wrapping).
static TREE_ID_CTR: Mutex<u32> = Mutex::new(1);
/// Monotonically increasing file ID counter (wrapping).
static FILE_ID_CTR: Mutex<u64> = Mutex::new(1);

// ---------------------------------------------------------------------------
// Helper: copy bytes into a fixed array, returning length stored
// ---------------------------------------------------------------------------

#[inline]
fn copy_bytes<const N: usize>(dst: &mut [u8; N], src: &[u8]) -> usize {
    let n = src.len().min(N);
    dst[..n].copy_from_slice(&src[..n]);
    n
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new session in `Negotiating` state.
/// Returns the session_id on success, None if the table is full.
pub fn cifs_session_create(server_ip: [u8; 4], port: u16, username: &[u8]) -> Option<u64> {
    let mut sessions = CIFS_SESSIONS.lock();

    // Find a free slot
    let slot = sessions.iter().position(|s| !s.active)?;

    // Assign a new session ID
    let sid = {
        let mut ctr = SESSION_ID_CTR.lock();
        let id = *ctr;
        *ctr = ctr.wrapping_add(1);
        id
    };

    let s = &mut sessions[slot];
    *s = CifsSession::empty();
    s.session_id = sid;
    s.state = CifsState::Negotiating;
    s.server_ip = server_ip;
    s.server_port = port;
    let ulen = copy_bytes(&mut s.username, username);
    s.username_len = ulen.min(255) as u8;
    s.active = true;

    Some(sid)
}

/// Simulate negotiation + authentication: set state → Connected, dialect = SMB3.
/// Returns true on success, false if session not found.
pub fn cifs_session_connect(session_id: u64) -> bool {
    let mut sessions = CIFS_SESSIONS.lock();
    for s in sessions.iter_mut() {
        if s.active && s.session_id == session_id {
            s.state = CifsState::Connected;
            s.dialect = 0x0300; // SMB3.0
            s.message_id = s.message_id.wrapping_add(1);
            return true;
        }
    }
    false
}

/// Destroy a session, also removing associated shares and handles.
pub fn cifs_session_destroy(session_id: u64) -> bool {
    // Remove handles
    {
        let mut handles = CIFS_HANDLES.lock();
        for h in handles.iter_mut() {
            if h.active && h.session_id == session_id {
                *h = CifsHandle::empty();
            }
        }
    }
    // Remove shares
    {
        let mut shares = CIFS_SHARES.lock();
        for sh in shares.iter_mut() {
            if sh.active && sh.session_id == session_id {
                *sh = CifsShare::empty();
            }
        }
    }
    // Remove session
    let mut sessions = CIFS_SESSIONS.lock();
    for s in sessions.iter_mut() {
        if s.active && s.session_id == session_id {
            *s = CifsSession::empty();
            return true;
        }
    }
    false
}

/// Connect a share on an existing session.
/// Returns the tree_id on success.
pub fn cifs_tree_connect(session_id: u64, share_name: &[u8]) -> Option<u32> {
    // Verify session exists and is Connected
    {
        let sessions = CIFS_SESSIONS.lock();
        let found = sessions
            .iter()
            .any(|s| s.active && s.session_id == session_id && s.state == CifsState::Connected);
        if !found {
            return None;
        }
    }

    let mut shares = CIFS_SHARES.lock();
    let slot = shares.iter().position(|sh| !sh.active)?;

    let tid = {
        let mut ctr = TREE_ID_CTR.lock();
        let id = *ctr;
        *ctr = ctr.wrapping_add(1);
        id
    };

    let sh = &mut shares[slot];
    *sh = CifsShare::empty();
    sh.tree_id = tid;
    sh.session_id = session_id;
    let nlen = copy_bytes(&mut sh.share_name, share_name);
    sh.share_len = nlen.min(255) as u8;
    sh.share_type = 1; // disk
    sh.active = true;

    Some(tid)
}

/// Disconnect a tree.
pub fn cifs_tree_disconnect(tree_id: u32) -> bool {
    // Remove associated handles
    {
        let mut handles = CIFS_HANDLES.lock();
        for h in handles.iter_mut() {
            if h.active && h.tree_id == tree_id {
                *h = CifsHandle::empty();
            }
        }
    }

    let mut shares = CIFS_SHARES.lock();
    for sh in shares.iter_mut() {
        if sh.active && sh.tree_id == tree_id {
            *sh = CifsShare::empty();
            return true;
        }
    }
    false
}

/// Open a file on a connected tree.
/// Returns the file_id on success.
pub fn cifs_open(tree_id: u32, path: &[u8], access: u32) -> Option<u64> {
    // Verify tree exists
    {
        let shares = CIFS_SHARES.lock();
        let found = shares.iter().any(|sh| sh.active && sh.tree_id == tree_id);
        if !found {
            return None;
        }
    }

    let mut handles = CIFS_HANDLES.lock();
    let slot = handles.iter().position(|h| !h.active)?;

    let fid = {
        let mut ctr = FILE_ID_CTR.lock();
        let id = *ctr;
        *ctr = ctr.wrapping_add(1);
        id
    };

    // Determine session_id from the share
    let session_id = {
        let shares = CIFS_SHARES.lock();
        let mut sid = 0u64;
        for sh in shares.iter() {
            if sh.active && sh.tree_id == tree_id {
                sid = sh.session_id;
                break;
            }
        }
        sid
    };

    let h = &mut handles[slot];
    *h = CifsHandle::empty();
    h.file_id = fid;
    h.tree_id = tree_id;
    h.session_id = session_id;
    let plen = copy_bytes(&mut h.path, path);
    h.path_len = plen.min(65535) as u16;
    h.access_mask = access;
    h.file_size = 0;
    h.pos = 0;
    h.active = true;

    Some(fid)
}

/// Close an open file handle.
pub fn cifs_close(file_id: u64) -> bool {
    let mut handles = CIFS_HANDLES.lock();
    for h in handles.iter_mut() {
        if h.active && h.file_id == file_id {
            *h = CifsHandle::empty();
            return true;
        }
    }
    false
}

/// Read from a file.  Stub: returns 0 unless file_size > 0.
/// (No real data storage in this stub — caller gets zeros up to min(buf.len(), file_size - offset))
pub fn cifs_read(file_id: u64, buf: &mut [u8], offset: u64) -> usize {
    let handles = CIFS_HANDLES.lock();
    for h in handles.iter() {
        if h.active && h.file_id == file_id {
            if h.file_size == 0 {
                return 0;
            }
            if offset >= h.file_size {
                return 0;
            }
            let available = h.file_size - offset;
            let to_read = (available as usize).min(buf.len());
            // No backing store in stub — fill with 0
            for b in buf[..to_read].iter_mut() {
                *b = 0;
            }
            return to_read;
        }
    }
    0
}

/// Write to a file.  Stub: updates file_size metadata; returns data.len().
pub fn cifs_write(file_id: u64, data: &[u8], offset: u64) -> usize {
    let mut handles = CIFS_HANDLES.lock();
    for h in handles.iter_mut() {
        if h.active && h.file_id == file_id {
            // Guard: prevent overflow in offset + data.len()
            if data.is_empty() {
                return 0;
            }
            let end = offset.saturating_add(data.len() as u64);
            if end > h.file_size {
                h.file_size = end;
            }
            return data.len();
        }
    }
    0
}

/// Get file attributes: (file_size, access_mask).
pub fn cifs_getattr(file_id: u64) -> Option<(u64, u32)> {
    let handles = CIFS_HANDLES.lock();
    for h in handles.iter() {
        if h.active && h.file_id == file_id {
            return Some((h.file_size, h.access_mask));
        }
    }
    None
}

/// Initialize the CIFS/SMB2 subsystem.
pub fn init() {
    serial_println!("    [cifs] SMB2/3 client stub initialized");
}
