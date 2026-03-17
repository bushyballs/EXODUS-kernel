/// Network-operation syscall handlers for Genesis
///
/// Implements: sys_socket, sys_bind, sys_listen, sys_connect, sys_accept,
///             sys_send, sys_recv, sys_shutdown_socket, sys_setsockopt,
///             sys_getsockopt, sys_getsockname, sys_getpeername, sys_sendto,
///             sys_recvfrom, sys_select, sys_poll, sys_close_socket,
///             sys_unix_sendmsg
///
/// AF_UNIX sockets are dispatched through crate::net::unix_socket.
/// AF_INET/AF_INET6 stubs return ENOSYS until the TCP/IP stack is wired.
///
/// All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::errno;
use crate::net::unix_socket;

// ─── Socket table ─────────────────────────────────────────────────────────────

/// Minimal socket state (used once the real network stack is wired)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SockState {
    /// Freshly created
    Created,
    /// bind() has been called
    Bound,
    /// listen() has been called
    Listening,
    /// connect() has completed (TCP) or sendto() target set (UDP)
    Connected,
    /// accept() returned this socket
    Accepted,
    /// shutdown()/close() called
    Closed,
}

/// Address family
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrFamily {
    Unix = 1,
    Inet = 2,
    Inet6 = 10,
}

/// Socket type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SockType {
    Stream = 1,
    Datagram = 2,
    Raw = 3,
    SeqPacket = 5,
}

pub struct Socket {
    pub id: u32,
    pub domain: AddrFamily,
    pub sock_type: SockType,
    pub protocol: u32,
    pub state: SockState,
    /// Local port (0 = not bound)
    pub local_port: u16,
    /// Remote port (0 = not connected)
    pub remote_port: u16,
    /// Pending receive data
    pub rx_buf: Vec<u8>,
}

static SOCKETS: Mutex<BTreeMap<u32, Socket>> = Mutex::new(BTreeMap::new());
static NEXT_SOCK_ID: Mutex<u32> = Mutex::new(1);

fn alloc_sock_id() -> u32 {
    let mut id = NEXT_SOCK_ID.lock();
    let v = *id;
    *id = id.saturating_add(1);
    v
}

// ─── SYS_SOCKET ───────────────────────────────────────────────────────────────

/// SYS_SOCKET: create an endpoint for communication.
///
/// domain: 1=AF_UNIX, 2=AF_INET, 10=AF_INET6
/// sock_type: 1=STREAM, 2=DGRAM, 5=SEQPACKET
/// protocol: normally 0
///
/// Returns: socket file descriptor on success, errno on failure.
pub fn sys_socket(domain: u32, sock_type: u32, protocol: u32) -> u64 {
    let af = match domain {
        1 => AddrFamily::Unix,
        2 => AddrFamily::Inet,
        10 => AddrFamily::Inet6,
        _ => return errno::EAFNOSUPPORT,
    };

    let st_raw = sock_type & 0xF;
    let st = match st_raw {
        1 => SockType::Stream,
        2 => SockType::Datagram,
        3 => SockType::Raw,
        5 => SockType::SeqPacket,
        _ => return errno::EINVAL,
    };

    // ── AF_UNIX: delegate to unix_socket subsystem ───────────────────────────
    if af == AddrFamily::Unix {
        let unix_type = st_raw as u16;
        let fd = unix_socket::unix_socket(unix_type);
        if fd < 0 {
            return errno::ENFILE;
        }
        return fd as u64;
    }

    // ── AF_INET / AF_INET6: track in local table, return ENOSYS until wired ──
    let id = alloc_sock_id();
    SOCKETS.lock().insert(
        id,
        Socket {
            id,
            domain: af,
            sock_type: st,
            protocol,
            state: SockState::Created,
            local_port: 0,
            remote_port: 0,
            rx_buf: Vec::new(),
        },
    );
    let _ = id; // stored in SOCKETS
    errno::ENOSYS
}

// ─── SYS_BIND ─────────────────────────────────────────────────────────────────

/// SYS_BIND: bind a socket to a local address.
///
/// For AF_UNIX: `addr` points to a sockaddr_un; bytes [2..] are the path.
/// `addrlen` must be at least 3 (2-byte sa_family + at least 1 path byte).
///
/// For other families: stub returning ENOSYS.
pub fn sys_bind(sockfd: u32, addr: *const u8, addrlen: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        // sockaddr_un layout: sa_family (u16) + sun_path ([u8; 108])
        // We require at least 3 bytes: 2 for sa_family + 1 path byte.
        if addrlen < 3 || addr.is_null() {
            return errno::EINVAL;
        }
        // Path starts at offset 2
        let path_len = (addrlen as usize).saturating_sub(2).min(107);
        let path = unsafe { core::slice::from_raw_parts(addr.add(2), path_len) };
        let ret = unix_socket::unix_bind(fd, path);
        if ret < 0 {
            return (-ret) as u64;
        }
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_LISTEN ───────────────────────────────────────────────────────────────

/// SYS_LISTEN: mark a socket as passive (ready to accept connections).
pub fn sys_listen(sockfd: u32, backlog: i32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        let ret = unix_socket::unix_listen(fd, backlog);
        if ret < 0 {
            return (-ret) as u64;
        }
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_CONNECT ──────────────────────────────────────────────────────────────

/// SYS_CONNECT: initiate a connection on a socket.
///
/// For AF_UNIX: `addr` is a sockaddr_un; path starts at offset 2.
pub fn sys_connect(sockfd: u32, addr: *const u8, addrlen: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if addrlen < 3 || addr.is_null() {
            return errno::EINVAL;
        }
        let path_len = (addrlen as usize).saturating_sub(2).min(107);
        let path = unsafe { core::slice::from_raw_parts(addr.add(2), path_len) };
        let ret = unix_socket::unix_connect(fd, path);
        if ret < 0 {
            return (-ret) as u64;
        }
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_ACCEPT ───────────────────────────────────────────────────────────────

/// SYS_ACCEPT: accept an incoming connection on a listening socket.
///
/// For AF_UNIX: `addr` and `addrlen` are ignored in this implementation
/// (peer address retrieval is available via sys_getpeername).
/// Returns the new socket file descriptor on success.
pub fn sys_accept(sockfd: u32, _addr: *mut u8, _addrlen: *mut u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        let new_fd = unix_socket::unix_accept(fd);
        if new_fd < 0 {
            return (-new_fd) as u64;
        }
        return new_fd as u64;
    }

    errno::ENOSYS
}

// ─── SYS_SEND ─────────────────────────────────────────────────────────────────

/// SYS_SEND: send data on a connected socket.
pub fn sys_send(sockfd: u32, buf: *const u8, len: usize, _flags: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if buf.is_null() || len == 0 {
            return errno::EINVAL;
        }
        let data = unsafe { core::slice::from_raw_parts(buf, len) };
        let ret = unix_socket::unix_send(fd, data);
        if ret < 0 {
            return (-ret) as u64;
        }
        return ret as u64;
    }

    errno::ENOSYS
}

// ─── SYS_RECV ─────────────────────────────────────────────────────────────────

/// SYS_RECV: receive data from a connected socket.
pub fn sys_recv(sockfd: u32, buf: *mut u8, len: usize, _flags: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if buf.is_null() || len == 0 {
            return errno::EINVAL;
        }
        let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
        let ret = unix_socket::unix_recv(fd, slice);
        if ret < 0 {
            return (-ret) as u64;
        }
        return ret as u64;
    }

    errno::ENOSYS
}

// ─── SYS_SENDTO ───────────────────────────────────────────────────────────────

/// SYS_SENDTO: send data, with optional destination address.
///
/// For AF_UNIX DGRAM sockets with a destination address, the path from
/// `dest_addr` (sockaddr_un, offset 2) is used to find the target.
/// For connected sockets (STREAM/SEQPACKET or DGRAM with peer set),
/// the destination is ignored and unix_send is called directly.
pub fn sys_sendto(
    sockfd: u32,
    buf: *const u8,
    len: usize,
    _flags: u32,
    dest_addr: *const u8,
    addrlen: u32,
) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if buf.is_null() || len == 0 {
            return errno::EINVAL;
        }
        let data = unsafe { core::slice::from_raw_parts(buf, len) };

        // If a destination address is provided, treat as unconnected DGRAM send.
        // We connect the socket temporarily to the target path then send.
        // For simplicity in this implementation we route to unix_send which uses
        // peer_fd; callers that need true unconnected DGRAM should use the
        // unix_socket public API directly.
        if !dest_addr.is_null() && addrlen >= 3 {
            let path_len = (addrlen as usize).saturating_sub(2).min(107);
            let path = unsafe { core::slice::from_raw_parts(dest_addr.add(2), path_len) };
            // Temporarily connect to target, send, then leave peer set
            // (unconnected DGRAM socket retains last peer as per Linux behaviour)
            unix_socket::unix_connect(fd, path);
        }

        let ret = unix_socket::unix_send(fd, data);
        if ret < 0 {
            return (-ret) as u64;
        }
        return ret as u64;
    }

    errno::ENOSYS
}

// ─── SYS_RECVFROM ─────────────────────────────────────────────────────────────

/// SYS_RECVFROM: receive data, optionally retrieving the source address.
///
/// For AF_UNIX: src_addr is populated with a sockaddr_un if non-null
/// (family = 1, path = sender's bound path).
pub fn sys_recvfrom(
    sockfd: u32,
    buf: *mut u8,
    len: usize,
    _flags: u32,
    _src_addr: *mut u8,
    _addrlen: *mut u32,
) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if buf.is_null() || len == 0 {
            return errno::EINVAL;
        }
        let slice = unsafe { core::slice::from_raw_parts_mut(buf, len) };
        let ret = unix_socket::unix_recv(fd, slice);
        // TODO: populate src_addr with sender's bound path when non-null.
        if ret < 0 {
            return (-ret) as u64;
        }
        return ret as u64;
    }

    errno::ENOSYS
}

// ─── SYS_SHUTDOWN_SOCKET ──────────────────────────────────────────────────────

/// SYS_SHUTDOWN: shut down part of a full-duplex connection.
///
/// For AF_UNIX: closes the socket fully (equivalent to close).
pub fn sys_shutdown_socket(sockfd: u32, _how: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        unix_socket::unix_close(fd);
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_CLOSE_SOCKET ─────────────────────────────────────────────────────────

/// SYS_CLOSE for a socket file descriptor.
///
/// Routes to unix_close for AF_UNIX fds; stubs ENOSYS for others.
pub fn sys_close_socket(sockfd: u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        let ret = unix_socket::unix_close(fd);
        if ret < 0 {
            return (-ret) as u64;
        }
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_UNIX_SENDMSG ─────────────────────────────────────────────────────────

/// SYS_SENDMSG wrapper for AF_UNIX ancillary data (SCM_RIGHTS / SCM_CREDENTIALS).
///
/// `cmsg_type`: unix_socket::SCM_RIGHTS or unix_socket::SCM_CREDENTIALS
/// `cmsg_buf` / `cmsg_len`: raw ancillary data bytes
pub fn sys_unix_sendmsg(
    sockfd: u32,
    buf: *const u8,
    len: usize,
    cmsg_type: i32,
    cmsg_buf: *const u8,
    cmsg_len: usize,
) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if buf.is_null() {
            return errno::EINVAL;
        }
        let data = unsafe { core::slice::from_raw_parts(buf, len) };
        let cmsg_data = if cmsg_buf.is_null() || cmsg_len == 0 {
            &[][..]
        } else {
            unsafe { core::slice::from_raw_parts(cmsg_buf, cmsg_len) }
        };
        let ret = unix_socket::unix_sendmsg(fd, data, cmsg_type, cmsg_data);
        if ret < 0 {
            return (-ret) as u64;
        }
        return ret as u64;
    }

    errno::ENOSYS
}

// ─── SYS_SETSOCKOPT / GETSOCKOPT ──────────────────────────────────────────────

/// SYS_SETSOCKOPT: set socket options.
///
/// AF_UNIX sockets acknowledge but do not act on options at this stage.
pub fn sys_setsockopt(
    sockfd: u32,
    _level: u32,
    _optname: u32,
    _optval: *const u8,
    _optlen: u32,
) -> u64 {
    let fd = sockfd as i32;
    if unix_socket::unix_is_fd(fd) {
        return 0; // acknowledged, no-op
    }
    errno::ENOSYS
}

/// SYS_GETSOCKOPT: retrieve socket options (stub).
pub fn sys_getsockopt(
    sockfd: u32,
    _level: u32,
    _optname: u32,
    _optval: *mut u8,
    _optlen: *mut u32,
) -> u64 {
    let fd = sockfd as i32;
    if unix_socket::unix_is_fd(fd) {
        return errno::EINVAL; // ENOPROTOOPT — option not supported
    }
    errno::ENOSYS
}

// ─── SYS_GETSOCKNAME / GETPEERNAME ───────────────────────────────────────────

/// SYS_GETSOCKNAME: get the local address of a socket.
///
/// For AF_UNIX: fills `addr` with a sockaddr_un (sa_family=1, path).
pub fn sys_getsockname(sockfd: u32, addr: *mut u8, addrlen: *mut u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if addr.is_null() || addrlen.is_null() {
            return errno::EINVAL;
        }
        let mut path_buf = [0u8; 108];
        let ret = unix_socket::unix_getsockname(fd, &mut path_buf);
        if ret < 0 {
            return (-ret) as u64;
        }
        let plen = ret as usize;
        // Write sockaddr_un: 2-byte family (little-endian 1) + path
        let total = 2usize.saturating_add(plen);
        unsafe {
            let out = core::slice::from_raw_parts_mut(addr, total.min(110));
            if out.len() >= 2 {
                out[0] = 1; // AF_UNIX low byte
                out[1] = 0; // AF_UNIX high byte
            }
            let copy = plen.min(out.len().saturating_sub(2));
            out[2..2 + copy].copy_from_slice(&path_buf[..copy]);
            *addrlen = total as u32;
        }
        return 0;
    }

    errno::ENOSYS
}

/// SYS_GETPEERNAME: get the remote address of a connected socket.
pub fn sys_getpeername(sockfd: u32, addr: *mut u8, addrlen: *mut u32) -> u64 {
    let fd = sockfd as i32;

    if unix_socket::unix_is_fd(fd) {
        if addr.is_null() || addrlen.is_null() {
            return errno::EINVAL;
        }
        let mut path_buf = [0u8; 108];
        let ret = unix_socket::unix_getpeername(fd, &mut path_buf);
        if ret < 0 {
            return (-ret) as u64;
        }
        let plen = ret as usize;
        let total = 2usize.saturating_add(plen);
        unsafe {
            let out = core::slice::from_raw_parts_mut(addr, total.min(110));
            if out.len() >= 2 {
                out[0] = 1;
                out[1] = 0;
            }
            let copy = plen.min(out.len().saturating_sub(2));
            out[2..2 + copy].copy_from_slice(&path_buf[..copy]);
            *addrlen = total as u32;
        }
        return 0;
    }

    errno::ENOSYS
}

// ─── SYS_SELECT ───────────────────────────────────────────────────────────────

/// SYS_SELECT: synchronous I/O multiplexing (stub — reports all fds ready).
///
/// Future: iterate fd sets, check readiness via unix_can_read/unix_can_write
/// for unix fds and the socket/pipe/file subsystems for others.
pub fn sys_select(nfds: u32) -> u64 {
    // Stub: claim all nfds are ready so programs do not block indefinitely.
    nfds as u64
}

// ─── SYS_POLL ─────────────────────────────────────────────────────────────────

/// SYS_POLL: wait for events on file descriptors (stub — reports all ready).
///
/// Future: walk the pollfd array; for unix fds use unix_can_read /
/// unix_can_write; return POLLIN/POLLOUT/POLLERR in revents.
pub fn sys_poll(nfds: u32) -> u64 {
    nfds as u64
}
