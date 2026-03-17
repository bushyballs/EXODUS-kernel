use super::tcp;
use super::{Ipv4Addr, NetError};
use crate::sync::Mutex;
/// BSD sockets API for Genesis
///
/// Provides the standard socket interface that applications use:
///   socket(), bind(), listen(), accept(), connect(), send(), recv(), close()
///   sendto(), recvfrom() for UDP datagrams
///   setsockopt(), getsockopt() for socket options
///   shutdown() for half-close
///   select()/poll() for readiness checking
///
/// This is the user-facing API that wraps TCP/UDP internals.
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering as SAOrdering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of sockets
const MAX_SOCKETS: usize = 1024;

/// Maximum listen backlog
const MAX_BACKLOG: u32 = 128;

/// Default send buffer size
const DEFAULT_SNDBUF: usize = 65536;

/// Default receive buffer size
const DEFAULT_RCVBUF: usize = 65536;

// ---------------------------------------------------------------------------
// Socket types and addresses
// ---------------------------------------------------------------------------

/// Socket types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Stream,   // TCP
    Datagram, // UDP
    Raw,      // Raw IP
}

/// Socket address
#[derive(Debug, Clone, Copy)]
pub struct SocketAddr {
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl SocketAddr {
    pub fn new(ip: Ipv4Addr, port: u16) -> Self {
        SocketAddr { ip, port }
    }
}

/// Socket states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Unbound,
    Bound,
    Listening,
    Connected,
    Closed,
    ShutdownRead,
    ShutdownWrite,
    ShutdownBoth,
}

/// Shutdown modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownMode {
    Read,
    Write,
    Both,
}

/// Socket option identifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketOption {
    /// SO_REUSEADDR: allow binding to an address already in use
    ReuseAddr,
    /// SO_KEEPALIVE: enable TCP keep-alive probes
    KeepAlive,
    /// TCP_NODELAY: disable Nagle's algorithm
    TcpNoDelay,
    /// SO_RCVBUF: receive buffer size
    RcvBuf,
    /// SO_SNDBUF: send buffer size
    SndBuf,
    /// SO_BROADCAST: allow sending broadcast datagrams
    Broadcast,
    /// SO_LINGER: control close behavior
    Linger,
}

/// Readiness flags for poll/select
#[derive(Debug, Clone, Copy)]
pub struct PollFlags {
    /// Socket is readable (data available or connection incoming)
    pub readable: bool,
    /// Socket is writable (send buffer has space)
    pub writable: bool,
    /// Socket has an error condition
    pub error: bool,
    /// Socket has been hung up (peer closed)
    pub hangup: bool,
}

impl PollFlags {
    fn empty() -> Self {
        PollFlags {
            readable: false,
            writable: false,
            error: false,
            hangup: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Socket options storage
// ---------------------------------------------------------------------------

struct SocketOptions {
    reuse_addr: bool,
    keepalive: bool,
    tcp_nodelay: bool,
    rcv_buf: usize,
    snd_buf: usize,
    broadcast: bool,
    non_blocking: bool,
    linger_secs: Option<u32>,
}

impl SocketOptions {
    fn new() -> Self {
        SocketOptions {
            reuse_addr: false,
            keepalive: false,
            tcp_nodelay: false,
            rcv_buf: DEFAULT_RCVBUF,
            snd_buf: DEFAULT_SNDBUF,
            broadcast: false,
            non_blocking: false,
            linger_secs: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Socket struct
// ---------------------------------------------------------------------------

/// A socket handle
pub struct Socket {
    pub socket_type: SocketType,
    pub local_addr: Option<SocketAddr>,
    pub remote_addr: Option<SocketAddr>,
    pub tcp_conn_id: Option<u32>,
    pub state: SocketState,
    /// Received data buffer (for UDP: (src_addr, data) pairs)
    pub recv_queue: Vec<(SocketAddr, Vec<u8>)>,
    /// TCP stream receive buffer (accumulated from TCP layer)
    tcp_recv_buf: Vec<u8>,
    /// Socket options
    options: SocketOptions,
    /// Listen backlog: pending connections waiting for accept()
    listen_backlog: Vec<u32>, // TCP connection IDs of pending connections
    /// Maximum backlog size
    max_backlog: u32,
    /// Whether socket is in non-blocking mode
    non_blocking: bool,
    /// Error status (set by async operations)
    pending_error: Option<NetError>,
}

impl Socket {
    /// Create a new socket
    pub fn new(socket_type: SocketType) -> Self {
        Socket {
            socket_type,
            local_addr: None,
            remote_addr: None,
            tcp_conn_id: None,
            state: SocketState::Unbound,
            recv_queue: Vec::new(),
            tcp_recv_buf: Vec::new(),
            options: SocketOptions::new(),
            listen_backlog: Vec::new(),
            max_backlog: MAX_BACKLOG,
            non_blocking: false,
            pending_error: None,
        }
    }

    /// Bind socket to a local address.
    ///
    /// NOTE: The caller (sys_bind) is responsible for port-in-use checking
    /// before calling this method, because sys_bind already holds the
    /// SOCKET_TABLE lock and this method must not re-acquire it.
    pub fn bind(&mut self, addr: SocketAddr) -> Result<(), NetError> {
        self.local_addr = Some(addr);
        self.state = SocketState::Bound;

        // For UDP, register the port with the global receive queue
        if self.socket_type == SocketType::Datagram {
            super::udp_bind(addr.port);
        }

        Ok(())
    }

    /// Listen for incoming connections (TCP only)
    pub fn listen(&mut self, backlog: u32) -> Result<(), NetError> {
        if self.socket_type != SocketType::Stream {
            return Err(NetError::InvalidPacket);
        }
        if self.local_addr.is_none() {
            return Err(NetError::AddrNotAvailable);
        }

        self.max_backlog = if backlog > MAX_BACKLOG {
            MAX_BACKLOG
        } else {
            backlog
        };
        self.state = SocketState::Listening;

        // Create TCP connection in LISTEN state
        let port = match self.local_addr {
            Some(a) => a.port,
            None => return Err(NetError::AddrNotAvailable),
        };
        let conn_id = tcp::listen(port);
        self.tcp_conn_id = Some(conn_id);

        Ok(())
    }

    /// Accept an incoming connection from the listen queue.
    /// Returns a new Socket representing the connected client.
    pub fn accept(&mut self) -> Result<(Socket, SocketAddr), NetError> {
        if self.state != SocketState::Listening {
            return Err(NetError::InvalidPacket);
        }

        // Check the TCP connection table for connections that arrived on our listen port
        let local_port = match self.local_addr {
            Some(a) => a.port,
            None => return Err(NetError::AddrNotAvailable),
        };
        let mut accepted_id: Option<u32> = None;
        let mut remote_ip = Ipv4Addr::ANY;
        let mut remote_port = 0u16;

        {
            let conns = tcp::TCP_CONNECTIONS.lock();
            for (&id, conn) in conns.iter() {
                if conn.local_port == local_port
                    && conn.state == tcp::TcpState::Established
                    && Some(id) != self.tcp_conn_id
                {
                    // Check this connection hasn't been accepted already
                    let already_accepted = self.listen_backlog.contains(&id);
                    if !already_accepted {
                        accepted_id = Some(id);
                        remote_ip = conn.remote_ip;
                        remote_port = conn.remote_port;
                        break;
                    }
                }
            }
        }

        if let Some(conn_id) = accepted_id {
            // Track that this connection was accepted
            self.listen_backlog.push(conn_id);

            // Prune the backlog: remove entries for connections that have since
            // been closed or timed out.  The TCP_CONNECTIONS lock was released by
            // the inner block above, so tcp::get_state() is safe here.
            self.listen_backlog.retain(|&id| {
                tcp::get_state(id).map_or(false, |s| {
                    s != tcp::TcpState::Closed && s != tcp::TcpState::TimeWait
                })
            });

            // Create a new connected socket
            let remote_addr = SocketAddr::new(remote_ip, remote_port);
            let mut new_socket = Socket::new(SocketType::Stream);
            new_socket.local_addr = self.local_addr;
            new_socket.remote_addr = Some(remote_addr);
            new_socket.tcp_conn_id = Some(conn_id);
            new_socket.state = SocketState::Connected;

            // Copy relevant options
            new_socket.options.keepalive = self.options.keepalive;
            new_socket.options.tcp_nodelay = self.options.tcp_nodelay;
            new_socket.options.rcv_buf = self.options.rcv_buf;
            new_socket.options.snd_buf = self.options.snd_buf;

            // Apply options to TCP connection
            if new_socket.options.keepalive {
                tcp::set_keepalive(conn_id, true);
            }
            if new_socket.options.tcp_nodelay {
                tcp::set_nodelay(conn_id, true);
            }

            // Re-create the listening TCP connection for future accepts
            let new_listen_id = tcp::listen(local_port);
            self.tcp_conn_id = Some(new_listen_id);

            Ok((new_socket, remote_addr))
        } else {
            if self.non_blocking {
                Err(NetError::IoError) // EWOULDBLOCK equivalent
            } else {
                Err(NetError::Timeout) // No pending connections
            }
        }
    }

    /// Connect to a remote address (TCP only)
    pub fn connect(&mut self, addr: SocketAddr) -> Result<(), NetError> {
        if self.socket_type != SocketType::Stream {
            return Err(NetError::InvalidPacket);
        }
        self.remote_addr = Some(addr);

        // Allocate ephemeral port if not bound
        if self.local_addr.is_none() {
            let port = allocate_ephemeral_port();
            self.local_addr = Some(SocketAddr::new(Ipv4Addr::ANY, port));
        }

        let port = match self.local_addr {
            Some(a) => a.port,
            None => return Err(NetError::AddrNotAvailable),
        };
        let conn_id = tcp::connect(port, addr.ip, addr.port);
        self.tcp_conn_id = Some(conn_id);

        // Apply socket options to the TCP connection
        if self.options.tcp_nodelay {
            tcp::set_nodelay(conn_id, true);
        }
        if self.options.keepalive {
            tcp::set_keepalive(conn_id, true);
        }
        tcp::set_sndbuf(conn_id, self.options.snd_buf);
        tcp::set_rcvbuf(conn_id, self.options.rcv_buf);

        self.state = SocketState::Connected;
        Ok(())
    }

    /// Send data on a connected socket (TCP)
    pub fn send(&mut self, data: &[u8]) -> Result<usize, NetError> {
        self.check_send_allowed()?;

        match self.socket_type {
            SocketType::Stream => {
                let conn_id = self.tcp_conn_id.ok_or(NetError::ConnectionRefused)?;
                tcp::send_data(conn_id, data)
            }
            SocketType::Datagram => {
                let remote = self.remote_addr.ok_or(NetError::AddrNotAvailable)?;
                self.send_to(data, remote)
            }
            SocketType::Raw => {
                Err(NetError::InvalidPacket) // Not implemented for raw sockets
            }
        }
    }

    /// Receive data from a connected socket (TCP)
    pub fn recv(&mut self, buf: &mut [u8]) -> Result<usize, NetError> {
        self.check_recv_allowed()?;

        match self.socket_type {
            SocketType::Stream => {
                let conn_id = self.tcp_conn_id.ok_or(NetError::ConnectionRefused)?;

                // Pull data from TCP layer into our buffer
                let tcp_data = tcp::read_data(conn_id);
                if !tcp_data.is_empty() {
                    self.tcp_recv_buf.extend_from_slice(&tcp_data);
                }

                if self.tcp_recv_buf.is_empty() {
                    // Check if connection was closed by peer
                    if let Some(state) = tcp::get_state(conn_id) {
                        if state == tcp::TcpState::CloseWait
                            || state == tcp::TcpState::Closed
                            || state == tcp::TcpState::TimeWait
                        {
                            return Ok(0); // EOF
                        }
                    }
                    if self.non_blocking {
                        return Err(NetError::IoError); // EWOULDBLOCK
                    }
                    return Ok(0);
                }

                let to_copy = buf.len().min(self.tcp_recv_buf.len());
                buf[..to_copy].copy_from_slice(&self.tcp_recv_buf[..to_copy]);
                self.tcp_recv_buf.drain(..to_copy);
                Ok(to_copy)
            }
            SocketType::Datagram => {
                let mut addr = SocketAddr::new(Ipv4Addr::ANY, 0);
                self.recv_from(buf, &mut addr)
            }
            SocketType::Raw => Err(NetError::InvalidPacket),
        }
    }

    /// Send a UDP datagram to a specific destination
    pub fn send_to(&mut self, data: &[u8], dest: SocketAddr) -> Result<usize, NetError> {
        if self.socket_type != SocketType::Datagram {
            return Err(NetError::InvalidPacket);
        }
        self.check_send_allowed()?;

        // Check broadcast permission
        if dest.ip == Ipv4Addr::BROADCAST && !self.options.broadcast {
            return Err(NetError::InvalidPacket);
        }

        // Allocate ephemeral port if not bound
        if self.local_addr.is_none() {
            let port = allocate_ephemeral_port();
            self.local_addr = Some(SocketAddr::new(Ipv4Addr::ANY, port));
            self.state = SocketState::Bound;
            super::udp_bind(port);
        }

        let src_port = match self.local_addr {
            Some(a) => a.port,
            None => return Err(NetError::AddrNotAvailable),
        };
        super::send_udp(src_port, dest.ip, dest.port, data)?;
        Ok(data.len())
    }

    /// Receive a UDP datagram, storing the source address
    pub fn recv_from(
        &mut self,
        buf: &mut [u8],
        src_addr: &mut SocketAddr,
    ) -> Result<usize, NetError> {
        if self.socket_type != SocketType::Datagram {
            return Err(NetError::InvalidPacket);
        }
        self.check_recv_allowed()?;

        // Check our queued datagrams first
        if !self.recv_queue.is_empty() {
            let (addr, data) = self.recv_queue.remove(0);
            *src_addr = addr;
            let to_copy = buf.len().min(data.len());
            buf[..to_copy].copy_from_slice(&data[..to_copy]);
            return Ok(to_copy);
        }

        // Try to get from the global UDP queue
        if let Some(local) = self.local_addr {
            if let Some((ip, port, data)) = super::udp_recv(local.port) {
                *src_addr = SocketAddr::new(ip, port);
                let to_copy = buf.len().min(data.len());
                buf[..to_copy].copy_from_slice(&data[..to_copy]);
                return Ok(to_copy);
            }
        }

        if self.non_blocking {
            Err(NetError::IoError) // EWOULDBLOCK
        } else {
            Ok(0) // No data available
        }
    }

    /// Shutdown part of the socket
    pub fn shutdown(&mut self, how: ShutdownMode) -> Result<(), NetError> {
        match how {
            ShutdownMode::Read => match self.state {
                SocketState::Connected => self.state = SocketState::ShutdownRead,
                SocketState::ShutdownWrite => self.state = SocketState::ShutdownBoth,
                _ => return Err(NetError::InvalidPacket),
            },
            ShutdownMode::Write => {
                match self.state {
                    SocketState::Connected => {
                        self.state = SocketState::ShutdownWrite;
                        // Initiate TCP half-close
                        if let Some(conn_id) = self.tcp_conn_id {
                            tcp::close_connection(conn_id);
                        }
                    }
                    SocketState::ShutdownRead => self.state = SocketState::ShutdownBoth,
                    _ => return Err(NetError::InvalidPacket),
                }
            }
            ShutdownMode::Both => {
                self.state = SocketState::ShutdownBoth;
                if let Some(conn_id) = self.tcp_conn_id {
                    tcp::close_connection(conn_id);
                }
            }
        }
        Ok(())
    }

    /// Close the socket
    pub fn close(&mut self) {
        if self.socket_type == SocketType::Stream {
            if let Some(conn_id) = self.tcp_conn_id {
                tcp::close_connection(conn_id);
            }
        }
        self.state = SocketState::Closed;
    }

    /// Set a socket option
    pub fn set_option(&mut self, opt: SocketOption, value: usize) -> Result<(), NetError> {
        match opt {
            SocketOption::ReuseAddr => {
                self.options.reuse_addr = value != 0;
            }
            SocketOption::KeepAlive => {
                self.options.keepalive = value != 0;
                if let Some(conn_id) = self.tcp_conn_id {
                    tcp::set_keepalive(conn_id, value != 0);
                }
            }
            SocketOption::TcpNoDelay => {
                self.options.tcp_nodelay = value != 0;
                if let Some(conn_id) = self.tcp_conn_id {
                    tcp::set_nodelay(conn_id, value != 0);
                }
            }
            SocketOption::RcvBuf => {
                if value == 0 {
                    return Err(NetError::InvalidPacket);
                }
                self.options.rcv_buf = value;
                if let Some(conn_id) = self.tcp_conn_id {
                    tcp::set_rcvbuf(conn_id, value);
                }
            }
            SocketOption::SndBuf => {
                if value == 0 {
                    return Err(NetError::InvalidPacket);
                }
                self.options.snd_buf = value;
                if let Some(conn_id) = self.tcp_conn_id {
                    tcp::set_sndbuf(conn_id, value);
                }
            }
            SocketOption::Broadcast => {
                self.options.broadcast = value != 0;
            }
            SocketOption::Linger => {
                self.options.linger_secs = if value == 0 { None } else { Some(value as u32) };
            }
        }
        Ok(())
    }

    /// Get a socket option value
    pub fn get_option(&self, opt: SocketOption) -> Result<usize, NetError> {
        match opt {
            SocketOption::ReuseAddr => Ok(self.options.reuse_addr as usize),
            SocketOption::KeepAlive => Ok(self.options.keepalive as usize),
            SocketOption::TcpNoDelay => Ok(self.options.tcp_nodelay as usize),
            SocketOption::RcvBuf => Ok(self.options.rcv_buf),
            SocketOption::SndBuf => Ok(self.options.snd_buf),
            SocketOption::Broadcast => Ok(self.options.broadcast as usize),
            SocketOption::Linger => Ok(self.options.linger_secs.unwrap_or(0) as usize),
        }
    }

    /// Set non-blocking mode
    pub fn set_nonblocking(&mut self, nonblocking: bool) {
        self.non_blocking = nonblocking;
        self.options.non_blocking = nonblocking;
    }

    /// Check if socket is non-blocking
    pub fn is_nonblocking(&self) -> bool {
        self.non_blocking
    }

    /// Poll the socket for readiness
    pub fn poll(&mut self) -> PollFlags {
        let mut flags = PollFlags::empty();

        match self.socket_type {
            SocketType::Stream => {
                if self.state == SocketState::Listening {
                    // Check if there are pending connections
                    if let Some(local) = self.local_addr {
                        let conns = tcp::TCP_CONNECTIONS.lock();
                        for (_, conn) in conns.iter() {
                            if conn.local_port == local.port
                                && conn.state == tcp::TcpState::Established
                            {
                                flags.readable = true;
                                break;
                            }
                        }
                    }
                } else if self.state == SocketState::Connected
                    || self.state == SocketState::ShutdownWrite
                {
                    // Check for readable data
                    if let Some(conn_id) = self.tcp_conn_id {
                        let tcp_data = tcp::read_data(conn_id);
                        if !tcp_data.is_empty() {
                            self.tcp_recv_buf.extend_from_slice(&tcp_data);
                        }
                        if !self.tcp_recv_buf.is_empty() {
                            flags.readable = true;
                        }

                        // Check connection state for hangup
                        if let Some(state) = tcp::get_state(conn_id) {
                            match state {
                                tcp::TcpState::CloseWait
                                | tcp::TcpState::Closed
                                | tcp::TcpState::TimeWait => {
                                    flags.hangup = true;
                                    flags.readable = true; // EOF is readable
                                }
                                _ => {}
                            }
                        }
                    }

                    // Check for writable (send buffer has space)
                    if self.state == SocketState::Connected {
                        flags.writable = true; // simplified: always writable if connected
                    }
                }
            }
            SocketType::Datagram => {
                // Drain any datagrams from the global UDP queue into our socket's
                // recv_queue so poll() does not silently consume them.
                if let Some(local) = self.local_addr {
                    while let Some((src_ip, src_port, data)) = super::udp_recv(local.port) {
                        let addr = SocketAddr::new(src_ip, src_port);
                        self.recv_queue.push((addr, data));
                    }
                }
                if !self.recv_queue.is_empty() {
                    flags.readable = true;
                }
                flags.writable = true; // UDP is always writable
            }
            SocketType::Raw => {}
        }

        // Check pending error
        if self.pending_error.is_some() {
            flags.error = true;
        }

        flags
    }

    /// Get and clear pending error
    pub fn take_error(&mut self) -> Option<NetError> {
        self.pending_error.take()
    }

    /// Helper: check if send is allowed given shutdown state
    fn check_send_allowed(&self) -> Result<(), NetError> {
        match self.state {
            SocketState::ShutdownWrite | SocketState::ShutdownBoth | SocketState::Closed => {
                Err(NetError::ConnectionReset)
            }
            _ => Ok(()),
        }
    }

    /// Helper: check if recv is allowed given shutdown state
    fn check_recv_allowed(&self) -> Result<(), NetError> {
        match self.state {
            SocketState::ShutdownRead | SocketState::ShutdownBoth | SocketState::Closed => {
                Err(NetError::ConnectionReset)
            }
            _ => Ok(()),
        }
    }

    /// Get the local bound address
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Get the remote address (for connected sockets)
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.remote_addr
    }
}

// ---------------------------------------------------------------------------
// Global socket table
// ---------------------------------------------------------------------------

/// Socket file descriptor type
pub type SocketFd = u32;

/// Global socket table: maps fd -> Socket
static SOCKET_TABLE: Mutex<BTreeMap<SocketFd, Socket>> = Mutex::new(BTreeMap::new());

/// Next socket fd to allocate.
/// Atomic replaces Mutex<u32>: we only need a unique incrementing number;
/// no memory ordering relationship with any other variable is required.
// hot path: allocated on every sys_socket() and sys_accept() call
static NEXT_SOCKET_FD: AtomicU32 = AtomicU32::new(3); // 0=stdin, 1=stdout, 2=stderr

/// Ephemeral port range (49152-65535).
/// Atomic replaces Mutex<u16> for the same reason as NEXT_SOCKET_FD.
// hot path: allocated on every connect() and unbound sendto()
static NEXT_EPHEMERAL_PORT: AtomicU16 = AtomicU16::new(49152);

/// Allocate an ephemeral port number (lock-free, O(1)).
// hot path: called from every TCP connect() and first UDP sendto()
#[inline]
fn allocate_ephemeral_port() -> u16 {
    // Wrap back to 49152 after 65535.
    // fetch_add will overflow u16 at 65535+1=0; handle by CAS loop.
    loop {
        let cur = NEXT_EPHEMERAL_PORT.load(SAOrdering::Relaxed);
        let next = if cur >= 65535 { 49152 } else { cur + 1 };
        if NEXT_EPHEMERAL_PORT
            .compare_exchange_weak(cur, next, SAOrdering::Relaxed, SAOrdering::Relaxed)
            .is_ok()
        {
            return cur;
        }
        // Lost CAS race — retry (rare; only at wrap boundary)
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// System call-style API (operates on global socket table)
// ---------------------------------------------------------------------------

/// Create a socket and return its file descriptor.
pub fn sys_socket(socket_type: SocketType) -> Result<SocketFd, NetError> {
    let mut table = SOCKET_TABLE.lock();
    if table.len() >= MAX_SOCKETS {
        return Err(NetError::IoError);
    }
    // Lock-free fd allocation: Relaxed is sufficient since the fd value
    // itself does not synchronise any other memory.
    let fd = NEXT_SOCKET_FD.fetch_add(1, SAOrdering::Relaxed);
    table.insert(fd, Socket::new(socket_type));
    Ok(fd)
}

/// Bind a socket to an address.
pub fn sys_bind(fd: SocketFd, addr: SocketAddr) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();

    // Port-in-use check (done here rather than in Socket::bind to avoid
    // re-acquiring the SOCKET_TABLE lock, which would deadlock).
    let reuse_addr = table
        .get(&fd)
        .map(|s| s.options.reuse_addr)
        .unwrap_or(false);

    let sock_type = table
        .get(&fd)
        .map(|s| s.socket_type)
        .ok_or(NetError::InvalidPacket)?;

    if !reuse_addr {
        for (&other_fd, other_sock) in table.iter() {
            if other_fd == fd {
                continue; // skip self
            }
            if let Some(ref local) = other_sock.local_addr {
                if local.port == addr.port
                    && (local.ip == addr.ip
                        || addr.ip == Ipv4Addr::ANY
                        || local.ip == Ipv4Addr::ANY)
                    && other_sock.socket_type == sock_type
                    && other_sock.state != SocketState::Closed
                {
                    return Err(NetError::AddrInUse);
                }
            }
        }
    }

    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.bind(addr)
}

/// Listen on a socket.
pub fn sys_listen(fd: SocketFd, backlog: u32) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.listen(backlog)
}

/// Accept a connection on a listening socket.
/// Returns (new_fd, remote_addr).
pub fn sys_accept(fd: SocketFd) -> Result<(SocketFd, SocketAddr), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let listen_sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    let (new_socket, addr) = listen_sock.accept()?;
    // Lock-free fd allocation — same reasoning as sys_socket.
    let new_fd = NEXT_SOCKET_FD.fetch_add(1, SAOrdering::Relaxed);
    table.insert(new_fd, new_socket);
    Ok((new_fd, addr))
}

/// Connect a socket to a remote address.
pub fn sys_connect(fd: SocketFd, addr: SocketAddr) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.connect(addr)
}

/// Send data on a connected socket.
pub fn sys_send(fd: SocketFd, data: &[u8]) -> Result<usize, NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.send(data)
}

/// Receive data from a connected socket.
pub fn sys_recv(fd: SocketFd, buf: &mut [u8]) -> Result<usize, NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.recv(buf)
}

/// Send a datagram to a specific address (UDP).
pub fn sys_sendto(fd: SocketFd, data: &[u8], dest: SocketAddr) -> Result<usize, NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.send_to(data, dest)
}

/// Receive a datagram and the source address (UDP).
pub fn sys_recvfrom(fd: SocketFd, buf: &mut [u8]) -> Result<(usize, SocketAddr), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    let mut addr = SocketAddr::new(Ipv4Addr::ANY, 0);
    let n = sock.recv_from(buf, &mut addr)?;
    Ok((n, addr))
}

/// Set a socket option.
pub fn sys_setsockopt(fd: SocketFd, opt: SocketOption, value: usize) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.set_option(opt, value)
}

/// Get a socket option.
pub fn sys_getsockopt(fd: SocketFd, opt: SocketOption) -> Result<usize, NetError> {
    let table = SOCKET_TABLE.lock();
    let sock = table.get(&fd).ok_or(NetError::InvalidPacket)?;
    sock.get_option(opt)
}

/// Set non-blocking mode.
pub fn sys_set_nonblocking(fd: SocketFd, nonblocking: bool) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.set_nonblocking(nonblocking);
    Ok(())
}

/// Shutdown a socket.
pub fn sys_shutdown(fd: SocketFd, how: ShutdownMode) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    sock.shutdown(how)
}

/// Close a socket.
pub fn sys_close(fd: SocketFd) -> Result<(), NetError> {
    let mut table = SOCKET_TABLE.lock();
    if let Some(mut sock) = table.remove(&fd) {
        sock.close();
        Ok(())
    } else {
        Err(NetError::InvalidPacket)
    }
}

/// Poll readiness for a single socket.
pub fn sys_poll(fd: SocketFd) -> Result<PollFlags, NetError> {
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&fd).ok_or(NetError::InvalidPacket)?;
    Ok(sock.poll())
}

/// Select-style readiness check across multiple sockets.
/// Returns a list of (fd, PollFlags) for sockets that have events.
pub fn sys_select(fds: &[SocketFd]) -> Vec<(SocketFd, PollFlags)> {
    let mut table = SOCKET_TABLE.lock();
    let mut ready = Vec::new();

    for &fd in fds {
        if let Some(sock) = table.get_mut(&fd) {
            let flags = sock.poll();
            if flags.readable || flags.writable || flags.error || flags.hangup {
                ready.push((fd, flags));
            }
        }
    }

    ready
}

/// Poll multiple sockets, returning only those with any readiness.
/// `timeout_ms` is ignored in this non-blocking version.
pub fn sys_poll_multi(fds: &[(SocketFd, bool, bool)]) -> Vec<(SocketFd, PollFlags)> {
    let mut table = SOCKET_TABLE.lock();
    let mut ready = Vec::new();

    for &(fd, want_read, want_write) in fds {
        if let Some(sock) = table.get_mut(&fd) {
            let flags = sock.poll();
            let matches = (want_read && flags.readable)
                || (want_write && flags.writable)
                || flags.error
                || flags.hangup;
            if matches {
                ready.push((fd, flags));
            }
        }
    }

    ready
}

/// Get the local address of a socket.
pub fn sys_getsockname(fd: SocketFd) -> Result<SocketAddr, NetError> {
    let table = SOCKET_TABLE.lock();
    let sock = table.get(&fd).ok_or(NetError::InvalidPacket)?;
    sock.local_addr.ok_or(NetError::AddrNotAvailable)
}

/// Get the peer address of a connected socket.
pub fn sys_getpeername(fd: SocketFd) -> Result<SocketAddr, NetError> {
    let table = SOCKET_TABLE.lock();
    let sock = table.get(&fd).ok_or(NetError::InvalidPacket)?;
    sock.remote_addr.ok_or(NetError::AddrNotAvailable)
}

/// Get the number of open sockets.
pub fn socket_count() -> usize {
    SOCKET_TABLE.lock().len()
}

/// Get the SocketType for an open socket fd.
/// Returns None if the fd is not found in the socket table.
pub fn socket_type(fd: SocketFd) -> Option<SocketType> {
    let table = SOCKET_TABLE.lock();
    table.get(&fd).map(|s| s.socket_type)
}

/// List all open socket fds with their state.
pub fn list_sockets() -> Vec<(
    SocketFd,
    SocketType,
    SocketState,
    Option<SocketAddr>,
    Option<SocketAddr>,
)> {
    let table = SOCKET_TABLE.lock();
    table
        .iter()
        .map(|(&fd, sock)| {
            (
                fd,
                sock.socket_type,
                sock.state,
                sock.local_addr,
                sock.remote_addr,
            )
        })
        .collect()
}
