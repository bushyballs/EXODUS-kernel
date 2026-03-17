/// AF_UNIX (Unix domain socket) subsystem — no-alloc, no-heap implementation
///
/// Provides local IPC via filesystem-path-addressed sockets using only
/// fixed-size static arrays. Supports SOCK_STREAM, SOCK_DGRAM, SOCK_SEQPACKET.
///
/// Design constraints:
///   - NO heap: no Vec, Box, String, alloc::*
///   - NO floats: no `as f32` / `as f64`
///   - NO panics: no unwrap(), expect(), panic!()
///   - All counters use saturating_add / saturating_sub
///   - All sequence numbers use wrapping_add
///   - Structs in static Mutex are Copy + have const fn empty()
///
/// All code is original.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// AF_UNIX address family
pub const AF_UNIX: u16 = 1;

/// Socket type: connection-oriented byte stream
pub const SOCK_STREAM: u16 = 1;

/// Socket type: connectionless datagram
pub const SOCK_DGRAM: u16 = 2;

/// Socket type: sequenced-packet (message boundaries preserved)
pub const SOCK_SEQPACKET: u16 = 5;

/// Base file-descriptor value for all unix sockets
pub const UNIX_FD_BASE: i32 = 2000;

/// SCM_RIGHTS: pass file descriptors as ancillary data
pub const SCM_RIGHTS: i32 = 1;

/// SCM_CREDENTIALS: pass process credentials as ancillary data
pub const SCM_CREDENTIALS: i32 = 2;

/// Maximum path length (matches UNIX_PATH_MAX in Linux)
const UNIX_PATH_MAX: usize = 108;

/// Maximum number of concurrent unix sockets
const MAX_UNIX_SOCKETS: usize = 64;

/// Maximum number of path→fd binding entries
const MAX_UNIX_BINDINGS: usize = 128;

/// Ring-buffer capacity for message queues (must be a power of 2)
const QUEUE_DEPTH: usize = 16;

/// Mask used to wrap ring-buffer indices
const QUEUE_MASK: u8 = (QUEUE_DEPTH - 1) as u8;

/// Maximum payload per message
const MSG_DATA_MAX: usize = 4096;

/// Maximum pending-connect slots per listening socket
const MAX_PENDING: usize = 8;

// ---------------------------------------------------------------------------
// UnixSocketState
// ---------------------------------------------------------------------------

/// Lifecycle state of a unix socket
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixSocketState {
    Unconnected,
    Listening,
    Connected,
    Closed,
}

// ---------------------------------------------------------------------------
// UnixMessage
// ---------------------------------------------------------------------------

/// A single message stored in a ring-buffer queue
#[derive(Clone, Copy)]
pub struct UnixMessage {
    /// Payload bytes
    pub data: [u8; MSG_DATA_MAX],
    /// Number of valid bytes in `data`
    pub len: u16,
    /// For DGRAM: fd of the sender socket; -1 if unknown
    pub sender_fd: i32,
}

impl UnixMessage {
    #[inline]
    pub const fn empty() -> Self {
        UnixMessage {
            data: [0u8; MSG_DATA_MAX],
            len: 0,
            sender_fd: -1,
        }
    }
}

// ---------------------------------------------------------------------------
// UnixSocket
// ---------------------------------------------------------------------------

/// One Unix domain socket slot
#[derive(Clone, Copy)]
pub struct UnixSocket {
    /// Assigned file descriptor (UNIX_FD_BASE + slot_index)
    pub fd: i32,
    /// SOCK_STREAM / SOCK_DGRAM / SOCK_SEQPACKET
    pub sock_type: u16,
    /// Current lifecycle state
    pub state: UnixSocketState,
    /// Bound filesystem path (NUL-terminated if path_len < 108)
    pub path: [u8; UNIX_PATH_MAX],
    /// Number of valid bytes in `path`
    pub path_len: u8,
    /// For STREAM/SEQPACKET: fd of the connected peer; -1 if none
    pub peer_fd: i32,

    // ── Receive ring buffer ────────────────────────────────────────────────
    pub rx_queue: [UnixMessage; QUEUE_DEPTH],
    /// Index of the next message to dequeue (consumer pointer)
    pub rx_head: u8,
    /// Index of the next free slot to enqueue (producer pointer)
    pub rx_tail: u8,

    // ── Transmit ring buffer (mirrors rx for the local send path) ──────────
    pub tx_queue: [UnixMessage; QUEUE_DEPTH],
    pub tx_head: u8,
    pub tx_tail: u8,

    // ── Listen backlog ─────────────────────────────────────────────────────
    /// Maximum number of pending connects honoured before refusing
    pub backlog: u8,
    /// Fds of clients waiting to be accept()ed
    pub pending_connects: [i32; MAX_PENDING],
    /// Number of valid entries in pending_connects
    pub pending_count: u8,

    // ── Credentials of the owning process ─────────────────────────────────
    pub uid: u32,
    pub gid: u32,
    pub pid: u32,

    /// Whether this slot is in use
    pub active: bool,
}

impl UnixSocket {
    /// Construct an empty (inactive) slot — safe for use in a static array
    pub const fn empty() -> Self {
        UnixSocket {
            fd: -1,
            sock_type: 0,
            state: UnixSocketState::Unconnected,
            path: [0u8; UNIX_PATH_MAX],
            path_len: 0,
            peer_fd: -1,
            rx_queue: [UnixMessage::empty(); QUEUE_DEPTH],
            rx_head: 0,
            rx_tail: 0,
            tx_queue: [UnixMessage::empty(); QUEUE_DEPTH],
            tx_head: 0,
            tx_tail: 0,
            backlog: 0,
            pending_connects: [-1i32; MAX_PENDING],
            pending_count: 0,
            uid: 0,
            gid: 0,
            pid: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// BindingEntry — path → fd mapping
// ---------------------------------------------------------------------------

/// One entry in the global binding table
#[derive(Clone, Copy)]
struct BindingEntry {
    path: [u8; UNIX_PATH_MAX],
    path_len: u8,
    fd: i32,
    active: bool,
}

impl BindingEntry {
    const fn empty() -> Self {
        BindingEntry {
            path: [0u8; UNIX_PATH_MAX],
            path_len: 0,
            fd: -1,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// All unix socket slots
static UNIX_SOCKETS: Mutex<[UnixSocket; MAX_UNIX_SOCKETS]> =
    Mutex::new([UnixSocket::empty(); MAX_UNIX_SOCKETS]);

/// Path → fd binding table
static UNIX_BINDINGS: Mutex<[BindingEntry; MAX_UNIX_BINDINGS]> =
    Mutex::new([BindingEntry::empty(); MAX_UNIX_BINDINGS]);

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the AF_UNIX subsystem (called from net::init)
pub fn init() {
    // Nothing dynamic to initialise — statics are already zero-initialised.
    // We call this so callers have a clear hook point.
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the slot index for the given fd, if it exists and is active
fn find_slot_by_fd(fd: i32, sockets: &[UnixSocket; MAX_UNIX_SOCKETS]) -> Option<usize> {
    if fd < UNIX_FD_BASE {
        return None;
    }
    for i in 0..MAX_UNIX_SOCKETS {
        if sockets[i].active && sockets[i].fd == fd {
            return Some(i);
        }
    }
    None
}

/// Return the first free (inactive) slot index, or None if the table is full
fn find_free_slot(sockets: &[UnixSocket; MAX_UNIX_SOCKETS]) -> Option<usize> {
    for i in 0..MAX_UNIX_SOCKETS {
        if !sockets[i].active {
            return Some(i);
        }
    }
    None
}

/// Look up a bound fd by path; returns the fd or -1 if not found
fn find_binding(path: &[u8]) -> i32 {
    let bindings = UNIX_BINDINGS.lock();
    for i in 0..MAX_UNIX_BINDINGS {
        if !bindings[i].active {
            continue;
        }
        let elen = bindings[i].path_len as usize;
        if elen != path.len() {
            continue;
        }
        if bindings[i].path[..elen] == path[..elen] {
            return bindings[i].fd;
        }
    }
    -1
}

/// Return true if a path is already bound
fn path_is_bound(path: &[u8], bindings: &[BindingEntry; MAX_UNIX_BINDINGS]) -> bool {
    let plen = path.len();
    for i in 0..MAX_UNIX_BINDINGS {
        if !bindings[i].active {
            continue;
        }
        let elen = bindings[i].path_len as usize;
        if elen == plen && bindings[i].path[..elen] == path[..plen] {
            return true;
        }
    }
    false
}

/// Enqueue a message into a ring buffer.
/// Returns true on success, false if the queue is full.
fn enqueue(
    queue: &mut [UnixMessage; QUEUE_DEPTH],
    head: u8,
    tail: &mut u8,
    msg: UnixMessage,
) -> bool {
    let next_tail = tail.wrapping_add(1) & QUEUE_MASK;
    if next_tail == head {
        // Queue full
        return false;
    }
    queue[(*tail) as usize] = msg;
    *tail = next_tail;
    true
}

/// Dequeue a message from a ring buffer.
/// Returns Some(message) or None if empty.
fn dequeue(queue: &mut [UnixMessage; QUEUE_DEPTH], head: &mut u8, tail: u8) -> Option<UnixMessage> {
    if *head == tail {
        return None;
    }
    let msg = queue[(*head) as usize];
    *head = head.wrapping_add(1) & QUEUE_MASK;
    Some(msg)
}

/// Return true if the rx ring buffer has at least one message
#[inline]
fn rx_has_data(head: u8, tail: u8) -> bool {
    head != tail
}

/// Return true if there is room for one more message in a ring buffer
#[inline]
fn queue_has_room(head: u8, tail: u8) -> bool {
    tail.wrapping_add(1) & QUEUE_MASK != head
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new AF_UNIX socket of the given type.
///
/// Returns a non-negative file descriptor (>= UNIX_FD_BASE) on success,
/// or -1 if the socket table is full or the type is unrecognised.
pub fn unix_socket(sock_type: u16) -> i32 {
    if sock_type != SOCK_STREAM && sock_type != SOCK_DGRAM && sock_type != SOCK_SEQPACKET {
        return -1;
    }

    let mut sockets = UNIX_SOCKETS.lock();
    let slot = match find_free_slot(&sockets) {
        Some(s) => s,
        None => return -1,
    };

    let fd = UNIX_FD_BASE.saturating_add(slot as i32);
    sockets[slot] = UnixSocket::empty();
    sockets[slot].fd = fd;
    sockets[slot].sock_type = sock_type;
    sockets[slot].state = UnixSocketState::Unconnected;
    sockets[slot].active = true;
    fd
}

/// Bind a socket to a filesystem path.
///
/// `path` must be 1–107 bytes long.  Returns 0 on success,
/// -98  (EADDRINUSE)  if the path is already bound,
/// -22  (EINVAL)      if the path length is out of range, and
/// -9   (EBADF)       if the fd is not a valid unix socket.
pub fn unix_bind(fd: i32, path: &[u8]) -> i32 {
    let plen = path.len();
    if plen == 0 || plen >= UNIX_PATH_MAX {
        return -22; // EINVAL
    }

    let mut bindings = UNIX_BINDINGS.lock();
    if path_is_bound(path, &bindings) {
        return -98; // EADDRINUSE
    }

    let mut sockets = UNIX_SOCKETS.lock();
    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Store path in socket
    sockets[slot].path[..plen].copy_from_slice(path);
    sockets[slot].path_len = plen as u8;

    // Store in binding table
    for i in 0..MAX_UNIX_BINDINGS {
        if !bindings[i].active {
            bindings[i].path[..plen].copy_from_slice(path);
            bindings[i].path_len = plen as u8;
            bindings[i].fd = fd;
            bindings[i].active = true;
            return 0;
        }
    }

    -12 // ENOMEM — binding table full
}

/// Mark a socket as listening for incoming connections.
///
/// Only valid for SOCK_STREAM and SOCK_SEQPACKET sockets.
/// `backlog` is capped at MAX_PENDING (8).
///
/// Returns 0 on success, -1 on failure.
pub fn unix_listen(fd: i32, backlog: i32) -> i32 {
    let mut sockets = UNIX_SOCKETS.lock();
    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -1,
    };

    let st = sockets[slot].sock_type;
    if st != SOCK_STREAM && st != SOCK_SEQPACKET {
        return -1;
    }

    let capped: u8 = if backlog < 1 {
        1
    } else if backlog > MAX_PENDING as i32 {
        MAX_PENDING as u8
    } else {
        backlog as u8
    };

    sockets[slot].state = UnixSocketState::Listening;
    sockets[slot].backlog = capped;
    0
}

/// Connect a socket to a server identified by its bound path.
///
/// For SOCK_STREAM / SOCK_SEQPACKET: queues this fd in the server's
/// pending_connects list. The caller transitions to Connected only after
/// unix_accept() is called on the server side.  We optimistically mark the
/// client Connected here to match the Linux behaviour of a non-blocking connect
/// on a local socket (the kernel completes the handshake immediately).
///
/// For SOCK_DGRAM: records the peer path so subsequent unix_send calls reach
/// the right target without needing an explicit destination.
///
/// Returns:
///  0    — success
/// -111  — ECONNREFUSED (server not listening / table full)
/// -22   — EINVAL (path too long / unknown fd)
pub fn unix_connect(fd: i32, path: &[u8]) -> i32 {
    let plen = path.len();
    if plen == 0 || plen >= UNIX_PATH_MAX {
        return -22;
    }

    let server_fd = find_binding(path);
    if server_fd < 0 {
        return -111; // ECONNREFUSED — no server bound at that path
    }

    let mut sockets = UNIX_SOCKETS.lock();

    let client_slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -22,
    };

    let server_slot = match find_slot_by_fd(server_fd, &sockets) {
        Some(s) => s,
        None => return -111,
    };

    let sock_type = sockets[client_slot].sock_type;

    if sock_type == SOCK_DGRAM {
        // For DGRAM, just record the peer path for subsequent sends
        sockets[client_slot].path[..plen].copy_from_slice(path);
        sockets[client_slot].peer_fd = server_fd;
        return 0;
    }

    // STREAM / SEQPACKET — add client to server's pending_connects
    if sockets[server_slot].state != UnixSocketState::Listening {
        return -111; // ECONNREFUSED
    }

    let count = sockets[server_slot].pending_count as usize;
    let bl = sockets[server_slot].backlog as usize;
    if count >= bl {
        return -111; // ECONNREFUSED — backlog full
    }

    sockets[server_slot].pending_connects[count] = fd;
    sockets[server_slot].pending_count = sockets[server_slot].pending_count.saturating_add(1);

    // Optimistically mark client as connected (peer_fd resolved in accept)
    sockets[client_slot].state = UnixSocketState::Connected;
    sockets[client_slot].peer_fd = server_fd; // temporary; overwritten on accept

    0
}

/// Accept the next pending connection from a listening socket.
///
/// Creates a new socket representing the server-side endpoint of the
/// accepted connection, wires up peer_fd on both sides, and marks both
/// as Connected.
///
/// Returns the new file descriptor on success, or:
/// -11   — EAGAIN (no pending connections)
/// -9    — EBADF  (fd is not a valid listening unix socket)
pub fn unix_accept(fd: i32) -> i32 {
    let mut sockets = UNIX_SOCKETS.lock();

    let server_slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9,
    };

    if sockets[server_slot].state != UnixSocketState::Listening {
        return -9;
    }

    let count = sockets[server_slot].pending_count as usize;
    if count == 0 {
        return -11; // EAGAIN
    }

    // Dequeue the first pending client
    let client_fd = sockets[server_slot].pending_connects[0];

    // Shift remaining entries left
    let mut i = 0usize;
    while i.saturating_add(1) < MAX_PENDING {
        sockets[server_slot].pending_connects[i] =
            sockets[server_slot].pending_connects[i.saturating_add(1)];
        i = i.saturating_add(1);
    }
    sockets[server_slot].pending_connects[MAX_PENDING - 1] = -1;
    sockets[server_slot].pending_count = sockets[server_slot].pending_count.saturating_sub(1);

    // Allocate a new server-side connected socket
    let new_slot = match find_free_slot(&sockets) {
        Some(s) => s,
        None => return -24, // EMFILE
    };

    let server_type = sockets[server_slot].sock_type;
    let new_fd = UNIX_FD_BASE.saturating_add(new_slot as i32);

    sockets[new_slot] = UnixSocket::empty();
    sockets[new_slot].fd = new_fd;
    sockets[new_slot].sock_type = server_type;
    sockets[new_slot].state = UnixSocketState::Connected;
    sockets[new_slot].peer_fd = client_fd;
    sockets[new_slot].active = true;

    // Wire the client back to the new server-side socket
    if let Some(client_slot) = find_slot_by_fd(client_fd, &sockets) {
        sockets[client_slot].peer_fd = new_fd;
        sockets[client_slot].state = UnixSocketState::Connected;
    }

    new_fd
}

/// Send data on a connected socket.
///
/// For SOCK_STREAM / SOCK_SEQPACKET: routes to the peer's rx_queue.
/// For SOCK_DGRAM: if peer_fd is set (connected dgram), routes to that peer;
///   otherwise returns -107 (ENOTCONN).
///
/// Sends at most MSG_DATA_MAX bytes per call; any excess is silently clamped.
///
/// Returns bytes sent, or a negative errno.
pub fn unix_send(fd: i32, data: &[u8]) -> isize {
    let mut sockets = UNIX_SOCKETS.lock();

    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    if sockets[slot].state == UnixSocketState::Closed {
        return -32; // EPIPE
    }

    let peer_fd = sockets[slot].peer_fd;
    if peer_fd < 0 {
        return -107; // ENOTCONN
    }

    let peer_slot = match find_slot_by_fd(peer_fd, &sockets) {
        Some(s) => s,
        None => return -32, // EPIPE — peer closed
    };

    if sockets[peer_slot].state == UnixSocketState::Closed {
        return -32; // EPIPE
    }

    let copy_len = if data.len() > MSG_DATA_MAX {
        MSG_DATA_MAX
    } else {
        data.len()
    };

    let mut msg = UnixMessage::empty();
    msg.data[..copy_len].copy_from_slice(&data[..copy_len]);
    msg.len = copy_len as u16;
    msg.sender_fd = fd;

    {
        let sock = &mut sockets[peer_slot];
        let head = sock.rx_head;
        if !enqueue(&mut sock.rx_queue, head, &mut sock.rx_tail, msg) {
            return -11; // EAGAIN — peer rx queue full
        }
    }

    copy_len as isize
}

/// Receive data from a socket's rx_queue into `buf`.
///
/// Returns bytes copied, or:
/// -11  — EAGAIN (no data available)
/// -9   — EBADF
pub fn unix_recv(fd: i32, buf: &mut [u8]) -> isize {
    let mut sockets = UNIX_SOCKETS.lock();

    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    let msg = {
        let sock = &mut sockets[slot];
        let tail = sock.rx_tail;
        match dequeue(&mut sock.rx_queue, &mut sock.rx_head, tail) {
            Some(m) => m,
            None => return -11, // EAGAIN
        }
    };

    let copy_len = if buf.len() < msg.len as usize {
        buf.len()
    } else {
        msg.len as usize
    };
    buf[..copy_len].copy_from_slice(&msg.data[..copy_len]);
    copy_len as isize
}

/// Send a message with optional ancillary (control) data.
///
/// `cmsg_type` may be SCM_RIGHTS or SCM_CREDENTIALS.
/// For SCM_CREDENTIALS, `cmsg_data` should be 12 bytes: uid(u32) gid(u32) pid(u32)
/// packed little-endian; the values are stored in the socket's credential fields.
///
/// SCM_RIGHTS fd-passing is logged as an intent stub — the actual fd table
/// duplication is deferred to the process/fd subsystem.
///
/// Returns the same value as unix_send on the payload.
pub fn unix_sendmsg(fd: i32, data: &[u8], cmsg_type: i32, cmsg_data: &[u8]) -> isize {
    // Handle control messages before the send
    if cmsg_type == SCM_CREDENTIALS {
        // Parse uid/gid/pid from cmsg_data (3 × u32, little-endian, 12 bytes)
        if cmsg_data.len() >= 12 {
            let uid = u32::from_le_bytes([cmsg_data[0], cmsg_data[1], cmsg_data[2], cmsg_data[3]]);
            let gid = u32::from_le_bytes([cmsg_data[4], cmsg_data[5], cmsg_data[6], cmsg_data[7]]);
            let pid =
                u32::from_le_bytes([cmsg_data[8], cmsg_data[9], cmsg_data[10], cmsg_data[11]]);
            let mut sockets = UNIX_SOCKETS.lock();
            if let Some(slot) = find_slot_by_fd(fd, &sockets) {
                sockets[slot].uid = uid;
                sockets[slot].gid = gid;
                sockets[slot].pid = pid;
            }
        }
    }
    // SCM_RIGHTS: intent is logged; actual duplication deferred to fd subsystem.
    // (No heap allocation needed — just proceed with the send.)

    unix_send(fd, data)
}

/// Close a unix socket.
///
/// If the socket is connected, the peer's state is set to Closed.
/// Any binding in UNIX_BINDINGS is removed.
/// The socket slot is cleared and marked inactive.
///
/// Returns 0 on success, -9 (EBADF) if fd is not found.
pub fn unix_close(fd: i32) -> i32 {
    let mut sockets = UNIX_SOCKETS.lock();

    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    // Notify peer
    let peer_fd = sockets[slot].peer_fd;
    if peer_fd >= 0 {
        if let Some(peer_slot) = find_slot_by_fd(peer_fd, &sockets) {
            sockets[peer_slot].state = UnixSocketState::Closed;
            sockets[peer_slot].peer_fd = -1;
        }
    }

    // Copy path info before clearing
    let path_len = sockets[slot].path_len as usize;
    let mut path_copy = [0u8; UNIX_PATH_MAX];
    path_copy[..path_len].copy_from_slice(&sockets[slot].path[..path_len]);

    // Clear the slot
    sockets[slot] = UnixSocket::empty();

    drop(sockets); // release lock before taking bindings lock

    // Remove binding if the socket had one
    if path_len > 0 {
        let mut bindings = UNIX_BINDINGS.lock();
        for i in 0..MAX_UNIX_BINDINGS {
            if bindings[i].active && bindings[i].fd == fd {
                bindings[i] = BindingEntry::empty();
                break;
            }
        }
    }

    0
}

/// Copy the socket's own bound path into `out`.
///
/// Returns the path length on success (1–107), or -9 (EBADF) if not found,
/// or -22 (EINVAL) if the socket is not bound.
pub fn unix_getsockname(fd: i32, out: &mut [u8; UNIX_PATH_MAX]) -> i32 {
    let sockets = UNIX_SOCKETS.lock();
    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9,
    };

    let plen = sockets[slot].path_len as usize;
    if plen == 0 {
        return -22; // EINVAL — not bound
    }
    out[..plen].copy_from_slice(&sockets[slot].path[..plen]);
    plen as i32
}

/// Copy the peer's bound path into `out`.
///
/// Returns the path length on success, or a negative errno.
pub fn unix_getpeername(fd: i32, out: &mut [u8; UNIX_PATH_MAX]) -> i32 {
    let sockets = UNIX_SOCKETS.lock();
    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return -9, // EBADF
    };

    let peer_fd = sockets[slot].peer_fd;
    if peer_fd < 0 {
        return -107; // ENOTCONN
    }

    let peer_slot = match find_slot_by_fd(peer_fd, &sockets) {
        Some(s) => s,
        None => return -107,
    };

    let plen = sockets[peer_slot].path_len as usize;
    if plen == 0 {
        return -22; // peer is not bound (valid for anonymous accepted sockets)
    }
    out[..plen].copy_from_slice(&sockets[peer_slot].path[..plen]);
    plen as i32
}

/// Returns true if `fd` belongs to the unix socket subsystem
#[inline]
pub fn unix_is_fd(fd: i32) -> bool {
    if fd < UNIX_FD_BASE {
        return false;
    }
    let sockets = UNIX_SOCKETS.lock();
    find_slot_by_fd(fd, &sockets).is_some()
}

/// Returns true if the socket has at least one message in its rx_queue
pub fn unix_can_read(fd: i32) -> bool {
    let sockets = UNIX_SOCKETS.lock();
    match find_slot_by_fd(fd, &sockets) {
        Some(slot) => rx_has_data(sockets[slot].rx_head, sockets[slot].rx_tail),
        None => false,
    }
}

/// Returns true if the peer exists and its rx_queue has room for one more message
pub fn unix_can_write(fd: i32) -> bool {
    let sockets = UNIX_SOCKETS.lock();
    let slot = match find_slot_by_fd(fd, &sockets) {
        Some(s) => s,
        None => return false,
    };

    let peer_fd = sockets[slot].peer_fd;
    if peer_fd < 0 {
        return false;
    }

    match find_slot_by_fd(peer_fd, &sockets) {
        Some(peer_slot) => queue_has_room(sockets[peer_slot].rx_head, sockets[peer_slot].rx_tail),
        None => false,
    }
}
