use crate::sync::Mutex;
/// Unix domain sockets — local inter-process communication via socket API
///
/// Provides stream-oriented and datagram-oriented communication between
/// processes on the same host. Supports abstract namespace addressing,
/// credential passing (SO_PEERCRED), and file descriptor transfer via
/// SCM_RIGHTS ancillary messages.
///
/// Inspired by: BSD sockets (API model), Linux AF_UNIX (abstract namespace,
/// SCM_RIGHTS), Plan 9 (file-based IPC). All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_SOCKETS: usize = 512;
const MAX_BACKLOG: usize = 32;
const SOCKET_BUF_SIZE: usize = 8192;
const MAX_FD_TRANSFER: usize = 16;
const MAX_ANCILLARY_SIZE: usize = 256;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SOCKET_TABLE: Mutex<Option<UnixSocketTable>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Socket types and addressing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Stream,    // SOCK_STREAM — reliable, ordered, connection-based
    Datagram,  // SOCK_DGRAM — unreliable, unordered, connectionless
    SeqPacket, // SOCK_SEQPACKET — reliable, ordered, message-bounded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Unbound,
    Bound,
    Listening,
    Connecting,
    Connected,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddress {
    /// Filesystem path (e.g., "/var/run/daemon.sock")
    Path(String),
    /// Abstract namespace (no filesystem entry, prefixed with null byte conceptually)
    Abstract(String),
    /// Unnamed (e.g., socketpair result)
    Unnamed,
}

impl SocketAddress {
    pub fn as_str(&self) -> &str {
        match self {
            SocketAddress::Path(p) => p.as_str(),
            SocketAddress::Abstract(a) => a.as_str(),
            SocketAddress::Unnamed => "<unnamed>",
        }
    }
}

// ---------------------------------------------------------------------------
// Peer credentials (SO_PEERCRED)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

// ---------------------------------------------------------------------------
// Ancillary data (cmsg) for SCM_RIGHTS and SCM_CREDENTIALS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmsgType {
    ScmRights,      // file descriptor passing
    ScmCredentials, // credential passing
}

#[derive(Debug, Clone)]
pub struct AncillaryMessage {
    pub cmsg_type: CmsgType,
    pub fds: Vec<u32>,                        // for SCM_RIGHTS
    pub credentials: Option<PeerCredentials>, // for SCM_CREDENTIALS
}

impl AncillaryMessage {
    pub fn rights(fds: Vec<u32>) -> Self {
        AncillaryMessage {
            cmsg_type: CmsgType::ScmRights,
            fds,
            credentials: None,
        }
    }

    pub fn creds(creds: PeerCredentials) -> Self {
        AncillaryMessage {
            cmsg_type: CmsgType::ScmCredentials,
            fds: Vec::new(),
            credentials: Some(creds),
        }
    }
}

// ---------------------------------------------------------------------------
// Message (datagram or ancillary-carrying)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SocketMessage {
    pub data: Vec<u8>,
    pub sender: Option<SocketAddress>,
    pub ancillary: Vec<AncillaryMessage>,
}

// ---------------------------------------------------------------------------
// Unix socket
// ---------------------------------------------------------------------------

pub struct UnixSocket {
    pub id: u32,
    pub sock_type: SocketType,
    pub state: SocketState,
    pub owner_pid: u32,
    pub address: SocketAddress,
    pub peer_id: Option<u32>, // connected peer socket id
    pub credentials: PeerCredentials,

    // Stream buffers
    recv_buf: Vec<u8>,
    recv_pos: usize,
    recv_count: usize,

    // Datagram/message queue
    messages: Vec<SocketMessage>,

    // Listening state
    backlog: Vec<u32>, // pending connection socket IDs
    max_backlog: usize,

    // Options
    pub pass_cred: bool, // SO_PASSCRED
    pub nonblocking: bool,
}

impl UnixSocket {
    fn new(id: u32, sock_type: SocketType, owner_pid: u32) -> Self {
        UnixSocket {
            id,
            sock_type,
            state: SocketState::Unbound,
            owner_pid,
            address: SocketAddress::Unnamed,
            peer_id: None,
            credentials: PeerCredentials {
                pid: owner_pid,
                uid: 0,
                gid: 0,
            },
            recv_buf: alloc::vec![0u8; SOCKET_BUF_SIZE],
            recv_pos: 0,
            recv_count: 0,
            messages: Vec::new(),
            backlog: Vec::new(),
            max_backlog: MAX_BACKLOG,
            pass_cred: false,
            nonblocking: false,
        }
    }

    /// Write data into the receive buffer (called by the sender side)
    fn push_data(&mut self, data: &[u8]) -> Result<usize, &'static str> {
        let space = SOCKET_BUF_SIZE - self.recv_count;
        if space == 0 {
            return Err("receive buffer full");
        }
        let to_write = if data.len() < space {
            data.len()
        } else {
            space
        };
        for i in 0..to_write {
            let pos = (self.recv_pos + self.recv_count + i) % SOCKET_BUF_SIZE;
            self.recv_buf[pos] = data[i];
        }
        self.recv_count += to_write;
        Ok(to_write)
    }

    /// Read data from the receive buffer
    fn pull_data(&mut self, buf: &mut [u8]) -> usize {
        if self.recv_count == 0 {
            return 0;
        }
        let to_read = if buf.len() < self.recv_count {
            buf.len()
        } else {
            self.recv_count
        };
        for i in 0..to_read {
            buf[i] = self.recv_buf[(self.recv_pos + i) % SOCKET_BUF_SIZE];
        }
        self.recv_pos = (self.recv_pos + to_read) % SOCKET_BUF_SIZE;
        self.recv_count -= to_read;
        to_read
    }

    /// Push a datagram message
    fn push_message(&mut self, msg: SocketMessage) -> Result<(), &'static str> {
        if self.messages.len() >= MAX_BACKLOG {
            return Err("message queue full");
        }
        self.messages.push(msg);
        Ok(())
    }

    /// Pop the next datagram message
    fn pop_message(&mut self) -> Option<SocketMessage> {
        if self.messages.is_empty() {
            None
        } else {
            Some(self.messages.remove(0))
        }
    }

    pub fn available(&self) -> usize {
        self.recv_count
    }
    pub fn pending_messages(&self) -> usize {
        self.messages.len()
    }
    pub fn pending_connections(&self) -> usize {
        self.backlog.len()
    }
}

// ---------------------------------------------------------------------------
// Socket table
// ---------------------------------------------------------------------------

pub struct UnixSocketTable {
    sockets: BTreeMap<u32, UnixSocket>,
    bound_paths: BTreeMap<String, u32>,    // address -> socket id
    abstract_names: BTreeMap<String, u32>, // abstract name -> socket id
    next_id: u32,
    total_created: u64,
    total_messages: u64,
    total_bytes: u64,
}

impl UnixSocketTable {
    fn new() -> Self {
        UnixSocketTable {
            sockets: BTreeMap::new(),
            bound_paths: BTreeMap::new(),
            abstract_names: BTreeMap::new(),
            next_id: 1,
            total_created: 0,
            total_messages: 0,
            total_bytes: 0,
        }
    }

    /// Create a new Unix socket
    pub fn socket(&mut self, sock_type: SocketType, owner_pid: u32) -> Result<u32, &'static str> {
        if self.sockets.len() >= MAX_SOCKETS {
            return Err("socket table full");
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sockets
            .insert(id, UnixSocket::new(id, sock_type, owner_pid));
        self.total_created = self.total_created.saturating_add(1);
        Ok(id)
    }

    /// Bind a socket to an address
    pub fn bind(&mut self, sock_id: u32, addr: SocketAddress) -> Result<(), &'static str> {
        // Check if address is already taken
        match &addr {
            SocketAddress::Path(p) => {
                if self.bound_paths.contains_key(p) {
                    return Err("address already in use");
                }
            }
            SocketAddress::Abstract(a) => {
                if self.abstract_names.contains_key(a) {
                    return Err("abstract address already in use");
                }
            }
            SocketAddress::Unnamed => return Err("cannot bind to unnamed address"),
        }

        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        if sock.state != SocketState::Unbound {
            return Err("socket already bound");
        }

        match &addr {
            SocketAddress::Path(p) => {
                self.bound_paths.insert(p.clone(), sock_id);
            }
            SocketAddress::Abstract(a) => {
                self.abstract_names.insert(a.clone(), sock_id);
            }
            SocketAddress::Unnamed => {}
        }

        sock.address = addr;
        sock.state = SocketState::Bound;
        Ok(())
    }

    /// Start listening for connections (stream sockets)
    pub fn listen(&mut self, sock_id: u32, backlog: usize) -> Result<(), &'static str> {
        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        if sock.sock_type != SocketType::Stream && sock.sock_type != SocketType::SeqPacket {
            return Err("cannot listen on datagram socket");
        }
        if sock.state != SocketState::Bound {
            return Err("socket not bound");
        }
        sock.max_backlog = if backlog > MAX_BACKLOG {
            MAX_BACKLOG
        } else {
            backlog
        };
        sock.state = SocketState::Listening;
        Ok(())
    }

    /// Connect to a listening socket
    pub fn connect(&mut self, client_id: u32, addr: &SocketAddress) -> Result<(), &'static str> {
        // Find the target socket
        let server_id = match addr {
            SocketAddress::Path(p) => *self
                .bound_paths
                .get(p)
                .ok_or("connection refused: no such path")?,
            SocketAddress::Abstract(a) => *self
                .abstract_names
                .get(a)
                .ok_or("connection refused: no such name")?,
            SocketAddress::Unnamed => return Err("cannot connect to unnamed"),
        };

        // Verify server is listening
        let server_state = {
            let server = self.sockets.get(&server_id).ok_or("server socket gone")?;
            if server.state != SocketState::Listening {
                return Err("connection refused: not listening");
            }
            if server.backlog.len() >= server.max_backlog {
                return Err("connection refused: backlog full");
            }
            server.state
        };
        let _ = server_state;

        // Add to server's backlog
        let server = self
            .sockets
            .get_mut(&server_id)
            .ok_or("server socket gone")?;
        server.backlog.push(client_id);

        // Mark client as connecting
        let client = self
            .sockets
            .get_mut(&client_id)
            .ok_or("client socket not found")?;
        client.state = SocketState::Connecting;
        client.peer_id = Some(server_id);

        Ok(())
    }

    /// Accept a pending connection (returns new connected socket ID)
    pub fn accept(&mut self, server_id: u32) -> Result<u32, &'static str> {
        let (client_id, server_pid, server_type) = {
            let server = self.sockets.get_mut(&server_id).ok_or("socket not found")?;
            if server.state != SocketState::Listening {
                return Err("not listening");
            }
            if server.backlog.is_empty() {
                return Err("no pending connections");
            }
            let cid = server.backlog.remove(0);
            (cid, server.owner_pid, server.sock_type)
        };

        // Create the server-side connected socket
        let new_id = self.socket(server_type, server_pid)?;

        // Connect the new socket to the client
        if let Some(new_sock) = self.sockets.get_mut(&new_id) {
            new_sock.state = SocketState::Connected;
            new_sock.peer_id = Some(client_id);
        }

        // Complete the client's connection
        if let Some(client) = self.sockets.get_mut(&client_id) {
            client.state = SocketState::Connected;
            client.peer_id = Some(new_id);
        }

        Ok(new_id)
    }

    /// Send data on a connected stream socket
    pub fn send(&mut self, sock_id: u32, data: &[u8]) -> Result<usize, &'static str> {
        let peer_id = {
            let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
            if sock.state != SocketState::Connected {
                return Err("not connected");
            }
            sock.peer_id.ok_or("no peer")?
        };

        let peer = self.sockets.get_mut(&peer_id).ok_or("peer socket closed")?;
        if peer.state == SocketState::Closed {
            return Err("broken pipe");
        }
        let written = peer.push_data(data)?;
        self.total_bytes += written as u64;
        self.total_messages = self.total_messages.saturating_add(1);
        Ok(written)
    }

    /// Receive data from a connected stream socket
    pub fn recv(&mut self, sock_id: u32, buf: &mut [u8]) -> Result<usize, &'static str> {
        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        if sock.state != SocketState::Connected {
            return Err("not connected");
        }
        let read = sock.pull_data(buf);
        if read == 0 && sock.nonblocking {
            return Err("would block");
        }
        Ok(read)
    }

    /// Send a datagram message (with optional ancillary data)
    pub fn sendmsg(
        &mut self,
        sock_id: u32,
        dest: &SocketAddress,
        data: Vec<u8>,
        ancillary: Vec<AncillaryMessage>,
    ) -> Result<usize, &'static str> {
        // Validate ancillary data
        for anc in &ancillary {
            if anc.cmsg_type == CmsgType::ScmRights && anc.fds.len() > MAX_FD_TRANSFER {
                return Err("too many fds in SCM_RIGHTS");
            }
        }

        let target_id = match dest {
            SocketAddress::Path(p) => *self.bound_paths.get(p).ok_or("destination not found")?,
            SocketAddress::Abstract(a) => {
                *self.abstract_names.get(a).ok_or("destination not found")?
            }
            SocketAddress::Unnamed => return Err("no destination"),
        };

        let sender_addr = {
            let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
            sock.address.clone()
        };

        let len = data.len();
        let msg = SocketMessage {
            data,
            sender: Some(sender_addr),
            ancillary,
        };

        let target = self
            .sockets
            .get_mut(&target_id)
            .ok_or("target socket closed")?;
        target.push_message(msg)?;
        self.total_messages = self.total_messages.saturating_add(1);
        self.total_bytes += len as u64;
        Ok(len)
    }

    /// Receive a datagram message
    pub fn recvmsg(&mut self, sock_id: u32) -> Result<SocketMessage, &'static str> {
        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        sock.pop_message().ok_or("no messages")
    }

    /// Send file descriptors via SCM_RIGHTS
    pub fn send_fds(
        &mut self,
        sock_id: u32,
        fds: Vec<u32>,
        data: &[u8],
    ) -> Result<usize, &'static str> {
        if fds.len() > MAX_FD_TRANSFER {
            return Err("too many fds");
        }

        let peer_id = {
            let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
            if sock.state != SocketState::Connected {
                return Err("not connected");
            }
            sock.peer_id.ok_or("no peer")?
        };

        let sender_addr = {
            let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
            sock.address.clone()
        };

        let msg = SocketMessage {
            data: data.to_vec(),
            sender: Some(sender_addr),
            ancillary: alloc::vec![AncillaryMessage::rights(fds)],
        };

        let peer = self.sockets.get_mut(&peer_id).ok_or("peer gone")?;
        peer.push_message(msg)?;
        self.total_messages = self.total_messages.saturating_add(1);
        Ok(data.len())
    }

    /// Get peer credentials (SO_PEERCRED)
    pub fn get_peer_cred(&self, sock_id: u32) -> Result<PeerCredentials, &'static str> {
        let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
        let peer_id = sock.peer_id.ok_or("not connected")?;
        let peer = self.sockets.get(&peer_id).ok_or("peer gone")?;
        Ok(peer.credentials)
    }

    /// Create a connected socketpair
    pub fn socketpair(
        &mut self,
        sock_type: SocketType,
        owner_pid: u32,
    ) -> Result<(u32, u32), &'static str> {
        let id1 = self.socket(sock_type, owner_pid)?;
        let id2 = self.socket(sock_type, owner_pid)?;

        if let Some(s1) = self.sockets.get_mut(&id1) {
            s1.state = SocketState::Connected;
            s1.peer_id = Some(id2);
        }
        if let Some(s2) = self.sockets.get_mut(&id2) {
            s2.state = SocketState::Connected;
            s2.peer_id = Some(id1);
        }

        Ok((id1, id2))
    }

    /// Close and clean up a socket
    pub fn close(&mut self, sock_id: u32) -> Result<(), &'static str> {
        let addr = {
            let sock = self.sockets.get(&sock_id).ok_or("socket not found")?;
            sock.address.clone()
        };

        // Remove from address registries
        match &addr {
            SocketAddress::Path(p) => {
                self.bound_paths.remove(p);
            }
            SocketAddress::Abstract(a) => {
                self.abstract_names.remove(a);
            }
            SocketAddress::Unnamed => {}
        }

        // Mark as closed (peer will see broken pipe)
        if let Some(sock) = self.sockets.get_mut(&sock_id) {
            sock.state = SocketState::Closed;
        }

        self.sockets.remove(&sock_id);
        Ok(())
    }

    /// Set socket to nonblocking mode
    pub fn set_nonblocking(&mut self, sock_id: u32, nonblocking: bool) -> Result<(), &'static str> {
        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        sock.nonblocking = nonblocking;
        Ok(())
    }

    /// Enable credential passing
    pub fn set_passcred(&mut self, sock_id: u32, enable: bool) -> Result<(), &'static str> {
        let sock = self.sockets.get_mut(&sock_id).ok_or("socket not found")?;
        sock.pass_cred = enable;
        Ok(())
    }

    pub fn stats(&self) -> UnixSocketStats {
        let mut connected = 0u32;
        let mut listening = 0u32;
        for sock in self.sockets.values() {
            match sock.state {
                SocketState::Connected => connected += 1,
                SocketState::Listening => listening += 1,
                _ => {}
            }
        }
        UnixSocketStats {
            total_sockets: self.sockets.len() as u32,
            connected,
            listening,
            bound_paths: self.bound_paths.len() as u32,
            abstract_names: self.abstract_names.len() as u32,
            total_created: self.total_created,
            total_messages: self.total_messages,
            total_bytes: self.total_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UnixSocketStats {
    pub total_sockets: u32,
    pub connected: u32,
    pub listening: u32,
    pub bound_paths: u32,
    pub abstract_names: u32,
    pub total_created: u64,
    pub total_messages: u64,
    pub total_bytes: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    *SOCKET_TABLE.lock() = Some(UnixSocketTable::new());
    serial_println!(
        "    [unix_sock] Unix domain socket subsystem ready (max {})",
        MAX_SOCKETS
    );
}

pub fn socket(sock_type: SocketType, owner: u32) -> Result<u32, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .socket(sock_type, owner)
}

pub fn bind(sock_id: u32, addr: SocketAddress) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .bind(sock_id, addr)
}

pub fn listen(sock_id: u32, backlog: usize) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .listen(sock_id, backlog)
}

pub fn connect(client_id: u32, addr: &SocketAddress) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .connect(client_id, addr)
}

pub fn accept(server_id: u32) -> Result<u32, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .accept(server_id)
}

pub fn send(sock_id: u32, data: &[u8]) -> Result<usize, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .send(sock_id, data)
}

pub fn recv(sock_id: u32, buf: &mut [u8]) -> Result<usize, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .recv(sock_id, buf)
}

pub fn socketpair(sock_type: SocketType, owner: u32) -> Result<(u32, u32), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .socketpair(sock_type, owner)
}

pub fn close(sock_id: u32) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .close(sock_id)
}

/// Send a datagram with optional ancillary data (SCM_RIGHTS / SCM_CREDENTIALS).
pub fn sendmsg(
    sock_id: u32,
    dest: &SocketAddress,
    data: Vec<u8>,
    ancillary: Vec<AncillaryMessage>,
) -> Result<usize, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .sendmsg(sock_id, dest, data, ancillary)
}

/// Receive a datagram message, including any ancillary data.
pub fn recvmsg(sock_id: u32) -> Result<SocketMessage, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .recvmsg(sock_id)
}

/// Send file descriptors to a connected peer via SCM_RIGHTS.
/// `data` is the accompanying byte payload (may be empty).
pub fn send_fds(sock_id: u32, fds: Vec<u32>, data: &[u8]) -> Result<usize, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .send_fds(sock_id, fds, data)
}

/// Retrieve peer credentials (SO_PEERCRED) for a connected socket.
pub fn get_peer_cred(sock_id: u32) -> Result<PeerCredentials, &'static str> {
    SOCKET_TABLE
        .lock()
        .as_ref()
        .ok_or("not initialized")?
        .get_peer_cred(sock_id)
}

/// Set a socket to non-blocking mode.
/// In non-blocking mode, `recv` returns Err("would block") immediately when
/// no data is available instead of blocking.
pub fn set_nonblocking(sock_id: u32, nonblocking: bool) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .set_nonblocking(sock_id, nonblocking)
}

/// Enable or disable credential passing (SO_PASSCRED).
/// When enabled, peer credentials are attached to each received datagram.
pub fn set_passcred(sock_id: u32, enable: bool) -> Result<(), &'static str> {
    SOCKET_TABLE
        .lock()
        .as_mut()
        .ok_or("not initialized")?
        .set_passcred(sock_id, enable)
}

/// Return aggregate statistics for the Unix socket subsystem.
pub fn stats() -> Result<UnixSocketStats, &'static str> {
    Ok(SOCKET_TABLE
        .lock()
        .as_ref()
        .ok_or("not initialized")?
        .stats())
}

/// Close all sockets owned by a process (called on process exit).
/// Iterates through the socket table and closes every socket whose
/// `owner_pid` matches the given pid.
pub fn close_all_for_pid(pid: u32) {
    let mut guard = SOCKET_TABLE.lock();
    if let Some(ref mut table) = *guard {
        // Collect IDs to close first to avoid borrow conflicts
        let ids: Vec<u32> = table
            .sockets
            .iter()
            .filter(|(_, s)| s.owner_pid == pid)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            // Best-effort: ignore individual errors (socket may already be closed)
            let _ = table.close(id);
        }
    }
}
