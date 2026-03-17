use crate::sync::Mutex;
/// Unix domain socket implementation
///
/// Local IPC via filesystem-addressed sockets. Supports stream and
/// datagram modes, abstract namespace addressing, SCM_RIGHTS
/// (file descriptor passing), and peer credential retrieval.
///
/// Inspired by: Linux AF_UNIX (net/unix/), BSD unix sockets.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum Unix sockets
const MAX_SOCKETS: usize = 256;

/// Maximum backlog for listening sockets
const MAX_BACKLOG: usize = 128;

/// Maximum receive buffer per socket
const MAX_RECV_BUF: usize = 262144; // 256KB

/// Maximum SCM_RIGHTS file descriptors per message
const MAX_SCM_RIGHTS: usize = 253;

/// Maximum datagram size
const MAX_DGRAM_SIZE: usize = 65536;

/// Maximum pending connections
const MAX_PENDING: usize = 64;

/// Abstract namespace prefix (NUL byte)
const ABSTRACT_PREFIX: u8 = 0;

// ---------------------------------------------------------------------------
// Socket type
// ---------------------------------------------------------------------------

/// Unix socket type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixSocketType {
    /// Connection-oriented byte stream (SOCK_STREAM)
    Stream,
    /// Connectionless datagram (SOCK_DGRAM)
    Datagram,
    /// Sequenced-packet (SOCK_SEQPACKET)
    SeqPacket,
}

// ---------------------------------------------------------------------------
// Socket state
// ---------------------------------------------------------------------------

/// Socket state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Unbound,
    Bound,
    Listening,
    Connecting,
    Connected,
    Closing,
    Closed,
}

// ---------------------------------------------------------------------------
// Socket address
// ---------------------------------------------------------------------------

/// Unix socket address
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnixAddr {
    /// Filesystem path (e.g., "/tmp/my.sock")
    Pathname(String),
    /// Abstract namespace (no filesystem entry)
    Abstract(String),
    /// Unnamed (auto-assigned)
    Unnamed,
}

impl UnixAddr {
    pub fn from_path(path: &str) -> Self {
        if path.starts_with('\0') {
            UnixAddr::Abstract(String::from(&path[1..]))
        } else {
            UnixAddr::Pathname(String::from(path))
        }
    }

    pub fn matches(&self, other: &UnixAddr) -> bool {
        match (self, other) {
            (UnixAddr::Pathname(a), UnixAddr::Pathname(b)) => a == b,
            (UnixAddr::Abstract(a), UnixAddr::Abstract(b)) => a == b,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Ancillary data (cmsg)
// ---------------------------------------------------------------------------

/// Control message types
#[derive(Debug, Clone)]
pub enum CmsgData {
    /// SCM_RIGHTS: pass file descriptors
    ScmRights(Vec<u32>),
    /// SCM_CREDENTIALS: pass process credentials
    ScmCredentials { pid: u32, uid: u32, gid: u32 },
}

/// A message with optional ancillary data
#[derive(Debug, Clone)]
pub struct UnixMessage {
    /// Message data
    pub data: Vec<u8>,
    /// Ancillary data
    pub cmsg: Vec<CmsgData>,
    /// Source address (for dgram)
    pub src_addr: Option<UnixAddr>,
}

// ---------------------------------------------------------------------------
// Peer credentials
// ---------------------------------------------------------------------------

/// Credentials of the peer process
#[derive(Debug, Clone, Copy)]
pub struct PeerCred {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

// ---------------------------------------------------------------------------
// Socket inner state
// ---------------------------------------------------------------------------

struct SocketInner {
    id: u32,
    sock_type: UnixSocketType,
    state: SocketState,
    addr: UnixAddr,
    /// Peer socket ID (for connected stream sockets)
    peer_id: Option<u32>,
    /// Pending incoming connections (for listening sockets)
    accept_queue: Vec<u32>,
    /// Receive buffer (stream: byte buffer, dgram: message queue)
    recv_messages: Vec<UnixMessage>,
    recv_bytes: usize,
    /// Send buffer limit
    send_buf_size: usize,
    /// Receive buffer limit
    recv_buf_size: usize,
    /// Backlog limit
    backlog: usize,
    /// Peer credentials
    peer_cred: Option<PeerCred>,
    /// Non-blocking mode
    non_blocking: bool,
    /// Pass credentials option
    pass_cred: bool,
    /// Statistics
    rx_packets: u64,
    tx_packets: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

impl SocketInner {
    fn new(id: u32, sock_type: UnixSocketType) -> Self {
        SocketInner {
            id,
            sock_type,
            state: SocketState::Unbound,
            addr: UnixAddr::Unnamed,
            peer_id: None,
            accept_queue: Vec::new(),
            recv_messages: Vec::new(),
            recv_bytes: 0,
            send_buf_size: MAX_RECV_BUF,
            recv_buf_size: MAX_RECV_BUF,
            backlog: MAX_BACKLOG,
            peer_cred: None,
            non_blocking: false,
            pass_cred: false,
            rx_packets: 0,
            tx_packets: 0,
            rx_bytes: 0,
            tx_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Public socket handle
// ---------------------------------------------------------------------------

/// Unix domain socket handle
pub struct UnixDomainSocket {
    id: u32,
}

impl UnixDomainSocket {
    /// Create a new Unix socket
    pub fn new(sock_type: UnixSocketType) -> Option<Self> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut()?;
        if sys.sockets.len() >= MAX_SOCKETS {
            return None;
        }
        let id = sys.next_id;
        sys.next_id = sys.next_id.saturating_add(1);
        sys.sockets.push(SocketInner::new(id, sock_type));
        Some(UnixDomainSocket { id })
    }

    /// Bind to an address
    pub fn bind(&self, addr: UnixAddr) -> Result<(), UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;

        // Check for address conflicts
        let addr_in_use = sys
            .sockets
            .iter()
            .any(|s| s.id != self.id && s.addr.matches(&addr));
        if addr_in_use {
            return Err(UnixError::AddrInUse);
        }

        let sock = find_sock_mut(sys, self.id)?;
        sock.addr = addr;
        sock.state = SocketState::Bound;
        Ok(())
    }

    /// Listen for incoming connections (stream only)
    pub fn listen(&self, backlog: usize) -> Result<(), UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;
        let sock = find_sock_mut(sys, self.id)?;

        if sock.sock_type != UnixSocketType::Stream && sock.sock_type != UnixSocketType::SeqPacket {
            return Err(UnixError::InvalidOp);
        }
        if sock.state != SocketState::Bound {
            return Err(UnixError::NotBound);
        }

        sock.state = SocketState::Listening;
        sock.backlog = backlog.min(MAX_BACKLOG);
        Ok(())
    }

    /// Accept a pending connection (returns new socket ID)
    pub fn accept(&self) -> Result<UnixDomainSocket, UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;
        let sock = find_sock_mut(sys, self.id)?;

        if sock.state != SocketState::Listening {
            return Err(UnixError::NotListening);
        }
        if sock.accept_queue.is_empty() {
            return Err(UnixError::WouldBlock);
        }

        let peer_id = sock.accept_queue.remove(0);
        let sock_type = sock.sock_type;

        // Read next_id before the mutable borrow from find_sock_mut is needed again
        let new_id = sys.next_id;
        sys.next_id = sys.next_id.saturating_add(1);
        let mut new_sock = SocketInner::new(new_id, sock_type);
        new_sock.state = SocketState::Connected;
        new_sock.peer_id = Some(peer_id);
        sys.sockets.push(new_sock);

        // Update the peer's connection
        if let Some(peer) = sys.sockets.iter_mut().find(|s| s.id == peer_id) {
            peer.peer_id = Some(new_id);
            peer.state = SocketState::Connected;
        }

        serial_println!(
            "  Unix: accepted connection (new sock {}, peer {})",
            new_id,
            peer_id
        );
        Ok(UnixDomainSocket { id: new_id })
    }

    /// Connect to a listening socket at the given address
    pub fn connect(&self, addr: &UnixAddr) -> Result<(), UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;

        // Find the target listening socket
        let target_id = sys
            .sockets
            .iter()
            .find(|s| s.addr.matches(addr) && s.state == SocketState::Listening)
            .map(|s| s.id)
            .ok_or(UnixError::ConnectionRefused)?;

        // Check backlog
        let target = find_sock_mut(sys, target_id)?;
        if target.accept_queue.len() >= target.backlog {
            return Err(UnixError::ConnectionRefused);
        }
        target.accept_queue.push(self.id);

        // Mark our socket as connecting
        let sock = find_sock_mut(sys, self.id)?;
        sock.state = SocketState::Connecting;
        sock.peer_id = Some(target_id);
        Ok(())
    }

    /// Send data to the connected peer
    pub fn send(&self, data: &[u8]) -> Result<usize, UnixError> {
        self.sendmsg(data, Vec::new())
    }

    /// Send data with ancillary data
    pub fn sendmsg(&self, data: &[u8], cmsg: Vec<CmsgData>) -> Result<usize, UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;

        let sock = find_sock(sys, self.id)?;
        let peer_id = sock.peer_id.ok_or(UnixError::NotConnected)?;
        let src_addr = sock.addr.clone();
        let sock_type = sock.sock_type;

        // Validate SCM_RIGHTS
        for c in &cmsg {
            if let CmsgData::ScmRights(fds) = c {
                if fds.len() > MAX_SCM_RIGHTS {
                    return Err(UnixError::TooManyFds);
                }
            }
        }

        // For datagram, check message size
        if sock_type == UnixSocketType::Datagram && data.len() > MAX_DGRAM_SIZE {
            return Err(UnixError::MsgTooLarge);
        }

        let peer = find_sock_mut(sys, peer_id)?;
        if peer.recv_bytes + data.len() > peer.recv_buf_size {
            return Err(UnixError::WouldBlock);
        }

        let msg = UnixMessage {
            data: data.to_vec(),
            cmsg,
            src_addr: Some(src_addr),
        };

        peer.recv_bytes = peer.recv_bytes.saturating_add(data.len());
        peer.rx_packets = peer.rx_packets.saturating_add(1);
        peer.rx_bytes = peer.rx_bytes.saturating_add(data.len() as u64);
        peer.recv_messages.push(msg);

        // Update sender stats
        let sock = find_sock_mut(sys, self.id)?;
        sock.tx_packets = sock.tx_packets.saturating_add(1);
        sock.tx_bytes = sock.tx_bytes.saturating_add(data.len() as u64);

        Ok(data.len())
    }

    /// Receive data from the socket
    pub fn recv(&self) -> Result<UnixMessage, UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;
        let sock = find_sock_mut(sys, self.id)?;

        if sock.recv_messages.is_empty() {
            return Err(UnixError::WouldBlock);
        }

        let msg = sock.recv_messages.remove(0);
        sock.recv_bytes = sock.recv_bytes.saturating_sub(msg.data.len());
        Ok(msg)
    }

    /// Send a datagram to a specific address (for dgram sockets)
    pub fn sendto(&self, data: &[u8], dest: &UnixAddr) -> Result<usize, UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;

        let sock = find_sock(sys, self.id)?;
        if sock.sock_type != UnixSocketType::Datagram {
            return Err(UnixError::InvalidOp);
        }
        let src_addr = sock.addr.clone();

        // Find target socket by address
        let target_id = sys
            .sockets
            .iter()
            .find(|s| s.addr.matches(dest))
            .map(|s| s.id)
            .ok_or(UnixError::ConnectionRefused)?;

        if data.len() > MAX_DGRAM_SIZE {
            return Err(UnixError::MsgTooLarge);
        }

        let target = find_sock_mut(sys, target_id)?;
        if target.recv_bytes + data.len() > target.recv_buf_size {
            return Err(UnixError::WouldBlock);
        }

        target.recv_messages.push(UnixMessage {
            data: data.to_vec(),
            cmsg: Vec::new(),
            src_addr: Some(src_addr),
        });
        target.recv_bytes = target.recv_bytes.saturating_add(data.len());
        target.rx_packets = target.rx_packets.saturating_add(1);
        target.rx_bytes = target.rx_bytes.saturating_add(data.len() as u64);

        let sock = find_sock_mut(sys, self.id)?;
        sock.tx_packets = sock.tx_packets.saturating_add(1);
        sock.tx_bytes = sock.tx_bytes.saturating_add(data.len() as u64);

        Ok(data.len())
    }

    /// Get peer credentials
    pub fn peer_cred(&self) -> Option<PeerCred> {
        let guard = SUBSYSTEM.lock();
        let sys = guard.as_ref()?;
        let sock = sys.sockets.iter().find(|s| s.id == self.id)?;
        sock.peer_cred
    }

    /// Set non-blocking mode
    pub fn set_nonblocking(&self, enabled: bool) -> Result<(), UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;
        let sock = find_sock_mut(sys, self.id)?;
        sock.non_blocking = enabled;
        Ok(())
    }

    /// Shutdown the socket
    pub fn shutdown(&self) -> Result<(), UnixError> {
        let mut guard = SUBSYSTEM.lock();
        let sys = guard.as_mut().ok_or(UnixError::NotInitialized)?;
        let sock = find_sock_mut(sys, self.id)?;
        sock.state = SocketState::Closing;
        let peer_id = sock.peer_id;
        // Notify peer (sock borrow is dropped by reading peer_id into a local)
        if let Some(pid) = peer_id {
            if let Some(peer) = sys.sockets.iter_mut().find(|s| s.id == pid) {
                peer.peer_id = None;
            }
        }
        Ok(())
    }

    /// Close and destroy the socket
    pub fn close(self) {
        let mut guard = SUBSYSTEM.lock();
        if let Some(sys) = guard.as_mut() {
            // Find peer_id before mutating
            let peer_id = sys
                .sockets
                .iter()
                .find(|s| s.id == self.id)
                .and_then(|s| s.peer_id);
            // Notify peer
            if let Some(pid) = peer_id {
                if let Some(peer) = sys.sockets.iter_mut().find(|s| s.id == pid) {
                    peer.peer_id = None;
                }
            }
            sys.sockets.retain(|s| s.id != self.id);
        }
    }

    /// Check if data is available
    pub fn has_data(&self) -> bool {
        let guard = SUBSYSTEM.lock();
        match guard.as_ref() {
            Some(sys) => sys
                .sockets
                .iter()
                .find(|s| s.id == self.id)
                .map(|s| !s.recv_messages.is_empty())
                .unwrap_or(false),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_sock(sys: &UnixSubsystem, id: u32) -> Result<&SocketInner, UnixError> {
    sys.sockets
        .iter()
        .find(|s| s.id == id)
        .ok_or(UnixError::BadSocket)
}

fn find_sock_mut(sys: &mut UnixSubsystem, id: u32) -> Result<&mut SocketInner, UnixError> {
    sys.sockets
        .iter_mut()
        .find(|s| s.id == id)
        .ok_or(UnixError::BadSocket)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixError {
    NotInitialized,
    BadSocket,
    AddrInUse,
    NotBound,
    NotListening,
    NotConnected,
    ConnectionRefused,
    WouldBlock,
    InvalidOp,
    MsgTooLarge,
    TooManyFds,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct UnixSubsystem {
    sockets: Vec<SocketInner>,
    next_id: u32,
}

static SUBSYSTEM: Mutex<Option<UnixSubsystem>> = Mutex::new(None);

/// Initialize the Unix domain socket subsystem
pub fn init() {
    *SUBSYSTEM.lock() = Some(UnixSubsystem {
        sockets: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: Unix domain socket subsystem initialized");
}
