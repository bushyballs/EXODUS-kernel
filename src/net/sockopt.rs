use crate::net::socket::SocketFd;
/// Socket options layer for Genesis
///
/// Implements getsockopt(2) / setsockopt(2) for three option levels:
///   SOL_SOCKET  — generic socket options (keepalive, buffers, linger, …)
///   IPPROTO_TCP — TCP-specific options (nodelay, keepalive timers, MSS, …)
///   IPPROTO_IP  — IPv4 options (TTL, TOS, multicast, …)
///
/// Design rules (bare-metal, no-heap):
///   - NO alloc, Vec, Box, String — fixed-size static arrays only
///   - NO float casts (no `as f32` / `as f64`)
///   - NO panics, unwrap, expect — return Option/bool/i64 instead
///   - All counters use saturating_add / saturating_sub
///   - MMIO accesses through read_volatile / write_volatile
///   - Static tables use const-fn initialisation so they live in .bss
///
/// Syscall numbers (matching Linux x86-64 for ABI compatibility):
///   54  — setsockopt
///   55  — getsockopt
///
/// Calling convention:
///   setsockopt(fd, level, optname, optval_ptr, optlen)
///   getsockopt(fd, level, optname, optval_ptr, optlen_ptr)
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// SOL_SOCKET level constants
// ---------------------------------------------------------------------------

pub const SOL_SOCKET: i32 = 1;

pub const SO_DEBUG: i32 = 1;
pub const SO_REUSEADDR: i32 = 2;
pub const SO_TYPE: i32 = 3;
pub const SO_ERROR: i32 = 4;
pub const SO_DONTROUTE: i32 = 5;
pub const SO_BROADCAST: i32 = 6;
pub const SO_SNDBUF: i32 = 7;
pub const SO_RCVBUF: i32 = 8;
pub const SO_KEEPALIVE: i32 = 9;
pub const SO_OOBINLINE: i32 = 10;
pub const SO_LINGER: i32 = 13;
pub const SO_REUSEPORT: i32 = 15;
pub const SO_RCVLOWAT: i32 = 18;
pub const SO_SNDLOWAT: i32 = 19;
pub const SO_RCVTIMEO: i32 = 20;
pub const SO_SNDTIMEO: i32 = 21;
pub const SO_SNDBUFFORCE: i32 = 32;
pub const SO_RCVBUFFORCE: i32 = 33;

// ---------------------------------------------------------------------------
// IPPROTO_TCP level constants
// ---------------------------------------------------------------------------

pub const IPPROTO_TCP: i32 = 6;

pub const TCP_NODELAY: i32 = 1;
pub const TCP_MAXSEG: i32 = 2;
pub const TCP_CORK: i32 = 3;
pub const TCP_KEEPIDLE: i32 = 4;
pub const TCP_KEEPINTVL: i32 = 5;
pub const TCP_KEEPCNT: i32 = 6;
pub const TCP_SYNCNT: i32 = 7;
pub const TCP_LINGER2: i32 = 8;
pub const TCP_DEFER_ACCEPT: i32 = 9;
pub const TCP_WINDOW_CLAMP: i32 = 10;
pub const TCP_INFO: i32 = 11;
pub const TCP_QUICKACK: i32 = 12;
pub const TCP_CONGESTION: i32 = 13;
pub const TCP_FASTOPEN: i32 = 23;
pub const TCP_USER_TIMEOUT: i32 = 18;

// ---------------------------------------------------------------------------
// IPPROTO_IP level constants
// ---------------------------------------------------------------------------

pub const IPPROTO_IP: i32 = 0;

pub const IP_TOS: i32 = 1;
pub const IP_TTL: i32 = 2;
pub const IP_HDRINCL: i32 = 3;
pub const IP_OPTIONS: i32 = 4;
pub const IP_MULTICAST_TTL: i32 = 33;
pub const IP_MULTICAST_LOOP: i32 = 34;
pub const IP_ADD_MEMBERSHIP: i32 = 35;
pub const IP_DROP_MEMBERSHIP: i32 = 36;

// ---------------------------------------------------------------------------
// Errno values mirrored from syscall module (avoids circular dep)
// We only store and return i64 error codes.
// ---------------------------------------------------------------------------

const ENOTSOCK: i64 = -88;
const ENOPROTOOPT: i64 = -92;
const EBADF: i64 = -9;
const EINVAL: i64 = -22;
#[allow(dead_code)]
const EFAULT: i64 = -14;

// ---------------------------------------------------------------------------
// Per-socket option storage
// ---------------------------------------------------------------------------

/// Maximum number of sockets that can have options tracked simultaneously.
/// Must be >= MAX_SOCKETS in net/socket.rs (1024).
const MAX_SOCK_OPTS: usize = 1024;

/// Base fd number used in net/socket.rs (0=stdin,1=stdout,2=stderr).
const SOCK_FD_BASE: u32 = 3;

/// Per-socket options — a flat Copy struct so it can live in a static array
/// inside a Mutex without requiring heap allocation.
#[derive(Copy, Clone)]
pub struct SockOpts {
    /// Whether this slot is occupied (fd is open).
    pub occupied: bool,
    /// The socket fd this entry belongs to (for reverse lookup / validation).
    pub fd: u32,

    // SOL_SOCKET options
    pub so_keepalive: bool,
    pub so_reuseaddr: bool,
    pub so_reuseport: bool,
    pub so_broadcast: bool,
    pub so_sndbuf: u32,      // bytes; default 131072
    pub so_rcvbuf: u32,      // bytes; default 131072
    pub so_rcvlowat: u32,    // minimum bytes for recv; default 1
    pub so_sndlowat: u32,    // minimum bytes for send; default 1
    pub so_rcvtimeo_ms: u64, // 0 = no timeout
    pub so_sndtimeo_ms: u64, // 0 = no timeout
    pub so_linger_secs: i16, // -1 = disabled; 0 = immediate RST; >0 = linger time
    pub so_error: i32,       // pending socket-level error (cleared on read)
    pub so_oobinline: bool,
    pub so_dontroute: bool,
    pub so_debug: bool,

    // IPPROTO_TCP options
    pub tcp_nodelay: bool,
    pub tcp_cork: bool,
    pub tcp_keepidle: u32, // seconds before first keepalive probe; default 7200
    pub tcp_keepintvl: u32, // seconds between probes; default 75
    pub tcp_keepcnt: u8,   // number of probes before drop; default 9
    pub tcp_maxseg: u16,   // MSS; default 1460
    pub tcp_quickack: bool, // disable delayed ACK
    pub tcp_syncnt: u8,    // SYN retries; default 6
    pub tcp_linger2: i32,  // FIN_WAIT2 timeout (seconds); -1 = system default
    pub tcp_defer_accept: u32, // seconds; 0 = disabled
    pub tcp_window_clamp: u32, // 0 = no clamp
    pub tcp_user_timeout: u32, // milliseconds; 0 = system default

    // IPPROTO_IP options
    pub ip_ttl: u8, // default 64
    pub ip_tos: u8,
    pub ip_hdrincl: bool,
}

impl SockOpts {
    /// Const constructor with sensible defaults — used for static initialisation.
    pub const fn default() -> Self {
        SockOpts {
            occupied: false,
            fd: 0,
            so_keepalive: false,
            so_reuseaddr: false,
            so_reuseport: false,
            so_broadcast: false,
            so_sndbuf: 131072,
            so_rcvbuf: 131072,
            so_rcvlowat: 1,
            so_sndlowat: 1,
            so_rcvtimeo_ms: 0,
            so_sndtimeo_ms: 0,
            so_linger_secs: -1,
            so_error: 0,
            so_oobinline: false,
            so_dontroute: false,
            so_debug: false,
            tcp_nodelay: false,
            tcp_cork: false,
            tcp_keepidle: 7200,
            tcp_keepintvl: 75,
            tcp_keepcnt: 9,
            tcp_maxseg: 1460,
            tcp_quickack: false,
            tcp_syncnt: 6,
            tcp_linger2: -1,
            tcp_defer_accept: 0,
            tcp_window_clamp: 0,
            tcp_user_timeout: 0,
            ip_ttl: 64,
            ip_tos: 0,
            ip_hdrincl: false,
        }
    }

    /// Reset to defaults (called on socket close / slot recycling).
    fn reset(&mut self) {
        *self = SockOpts::default();
    }
}

// The table is indexed by (fd - SOCK_FD_BASE).  Slot 0 → fd 3, slot 1 → fd 4, …
// We wrap the whole array in a single Mutex to avoid per-slot lock complexity;
// critical sections are very short (single field read/write).
static SOCK_OPTS_TABLE: Mutex<[SockOpts; MAX_SOCK_OPTS]> =
    Mutex::new([SockOpts::default(); MAX_SOCK_OPTS]);

// ---------------------------------------------------------------------------
// Table helpers
// ---------------------------------------------------------------------------

/// Convert a raw fd into a table index.  Returns None if out of range.
#[inline]
fn fd_to_idx(fd: i32) -> Option<usize> {
    if fd < SOCK_FD_BASE as i32 {
        return None;
    }
    let idx = (fd as u32).saturating_sub(SOCK_FD_BASE) as usize;
    if idx >= MAX_SOCK_OPTS {
        None
    } else {
        Some(idx)
    }
}

/// Register a newly-created socket fd so its options slot is initialised.
///
/// Called from `sys_socket` after a socket fd has been allocated.
/// Safe to call multiple times for the same fd (idempotent).
pub fn register_socket(fd: SocketFd) {
    if let Some(idx) = fd_to_idx(fd as i32) {
        let mut table = SOCK_OPTS_TABLE.lock();
        if !table[idx].occupied {
            table[idx].reset();
            table[idx].occupied = true;
            table[idx].fd = fd;
        }
    }
}

/// Unregister a socket fd when it is closed.
///
/// Called from `sys_close` after the TCP/UDP layer has been cleaned up.
pub fn unregister_socket(fd: SocketFd) {
    if let Some(idx) = fd_to_idx(fd as i32) {
        let mut table = SOCK_OPTS_TABLE.lock();
        if table[idx].fd == fd {
            table[idx].reset();
        }
    }
}

/// Return a *copy* of the SockOpts for the given fd.
/// Returns `None` if the slot is unoccupied or out of range.
pub fn sockopt_get(fd: i32) -> Option<SockOpts> {
    let idx = fd_to_idx(fd)?;
    let table = SOCK_OPTS_TABLE.lock();
    if table[idx].occupied && table[idx].fd == fd as u32 {
        Some(table[idx])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Individual field setters (no closures — no_std + stable trait bounds)
// ---------------------------------------------------------------------------

/// Internal helper — validate slot, then apply a field mutation.
/// All public `sockopt_set_*` functions call this.
macro_rules! set_field {
    ($fd:expr, $field:ident, $value:expr) => {{
        match fd_to_idx($fd) {
            None => false,
            Some(idx) => {
                let mut table = SOCK_OPTS_TABLE.lock();
                if table[idx].occupied && table[idx].fd == $fd as u32 {
                    table[idx].$field = $value;
                    true
                } else {
                    false
                }
            }
        }
    }};
}

pub fn sockopt_set_keepalive(fd: i32, val: bool) -> bool {
    set_field!(fd, so_keepalive, val)
}
pub fn sockopt_set_reuseaddr(fd: i32, val: bool) -> bool {
    set_field!(fd, so_reuseaddr, val)
}
pub fn sockopt_set_reuseport(fd: i32, val: bool) -> bool {
    set_field!(fd, so_reuseport, val)
}
pub fn sockopt_set_broadcast(fd: i32, val: bool) -> bool {
    set_field!(fd, so_broadcast, val)
}
pub fn sockopt_set_sndbuf(fd: i32, val: u32) -> bool {
    set_field!(fd, so_sndbuf, val)
}
pub fn sockopt_set_rcvbuf(fd: i32, val: u32) -> bool {
    set_field!(fd, so_rcvbuf, val)
}
pub fn sockopt_set_rcvlowat(fd: i32, val: u32) -> bool {
    set_field!(fd, so_rcvlowat, val)
}
pub fn sockopt_set_sndlowat(fd: i32, val: u32) -> bool {
    set_field!(fd, so_sndlowat, val)
}
pub fn sockopt_set_rcvtimeo(fd: i32, val: u64) -> bool {
    set_field!(fd, so_rcvtimeo_ms, val)
}
pub fn sockopt_set_sndtimeo(fd: i32, val: u64) -> bool {
    set_field!(fd, so_sndtimeo_ms, val)
}
pub fn sockopt_set_linger(fd: i32, val: i16) -> bool {
    set_field!(fd, so_linger_secs, val)
}
pub fn sockopt_set_oobinline(fd: i32, val: bool) -> bool {
    set_field!(fd, so_oobinline, val)
}
pub fn sockopt_set_dontroute(fd: i32, val: bool) -> bool {
    set_field!(fd, so_dontroute, val)
}
pub fn sockopt_set_debug(fd: i32, val: bool) -> bool {
    set_field!(fd, so_debug, val)
}

pub fn sockopt_set_tcp_nodelay(fd: i32, val: bool) -> bool {
    set_field!(fd, tcp_nodelay, val)
}
pub fn sockopt_set_tcp_cork(fd: i32, val: bool) -> bool {
    set_field!(fd, tcp_cork, val)
}
pub fn sockopt_set_tcp_keepidle(fd: i32, val: u32) -> bool {
    set_field!(fd, tcp_keepidle, val)
}
pub fn sockopt_set_tcp_keepintvl(fd: i32, val: u32) -> bool {
    set_field!(fd, tcp_keepintvl, val)
}
pub fn sockopt_set_tcp_keepcnt(fd: i32, val: u8) -> bool {
    set_field!(fd, tcp_keepcnt, val)
}
pub fn sockopt_set_tcp_maxseg(fd: i32, val: u16) -> bool {
    set_field!(fd, tcp_maxseg, val)
}
pub fn sockopt_set_tcp_quickack(fd: i32, val: bool) -> bool {
    set_field!(fd, tcp_quickack, val)
}
pub fn sockopt_set_tcp_syncnt(fd: i32, val: u8) -> bool {
    set_field!(fd, tcp_syncnt, val)
}
pub fn sockopt_set_tcp_linger2(fd: i32, val: i32) -> bool {
    set_field!(fd, tcp_linger2, val)
}
pub fn sockopt_set_tcp_defer_accept(fd: i32, val: u32) -> bool {
    set_field!(fd, tcp_defer_accept, val)
}
pub fn sockopt_set_tcp_window_clamp(fd: i32, val: u32) -> bool {
    set_field!(fd, tcp_window_clamp, val)
}
pub fn sockopt_set_tcp_user_timeout(fd: i32, val: u32) -> bool {
    set_field!(fd, tcp_user_timeout, val)
}

pub fn sockopt_set_ip_ttl(fd: i32, val: u8) -> bool {
    set_field!(fd, ip_ttl, val)
}
pub fn sockopt_set_ip_tos(fd: i32, val: u8) -> bool {
    set_field!(fd, ip_tos, val)
}
pub fn sockopt_set_ip_hdrincl(fd: i32, val: bool) -> bool {
    set_field!(fd, ip_hdrincl, val)
}

/// Clear the pending SO_ERROR for a socket (called after user reads it).
pub fn sockopt_clear_error(fd: i32) {
    set_field!(fd, so_error, 0);
}

/// Set SO_ERROR (called by the network stack when an async error occurs).
pub fn sockopt_set_error(fd: i32, err: i32) {
    set_field!(fd, so_error, err);
}

// ---------------------------------------------------------------------------
// setsockopt(2) — syscall entry point
// ---------------------------------------------------------------------------

/// sys_setsockopt — set a socket option from a user-supplied value.
///
/// # Parameters
/// * `fd`      — socket file descriptor (raw i32)
/// * `level`   — option level (SOL_SOCKET, IPPROTO_TCP, IPPROTO_IP)
/// * `optname` — option identifier constant
/// * `optval`  — pointer (as u64) to user memory holding the new value
/// * `optlen`  — length in bytes of the user buffer
///
/// # Returns
/// 0 on success, negative errno on failure.
///
/// # Safety
/// The caller (syscall dispatcher) must have already validated that
/// `optval..optval+optlen` is a legal user-space range before calling here.
pub fn sys_setsockopt(fd: i32, level: i32, optname: i32, optval: u64, optlen: u32) -> i64 {
    // Sanity: optval pointer must be non-null and at least 4 bytes long
    // (smallest meaningful option is a u32/i32 boolean flag).
    if optval == 0 || optlen < 4 {
        return EINVAL;
    }

    // Validate fd index
    let idx = match fd_to_idx(fd) {
        Some(i) => i,
        None => return EBADF,
    };

    {
        let table = SOCK_OPTS_TABLE.lock();
        if !table[idx].occupied || table[idx].fd != fd as u32 {
            return ENOTSOCK;
        }
    }

    // Read a u32 from user memory (the most common option value width).
    // SAFETY: caller validated the pointer range.
    let u32_val: u32 = unsafe { (optval as *const u32).read_volatile() };
    let bool_val: bool = u32_val != 0;

    match level {
        // ── SOL_SOCKET ──────────────────────────────────────────────────────
        SOL_SOCKET => match optname {
            SO_DEBUG => {
                sockopt_set_debug(fd, bool_val);
            }
            SO_REUSEADDR => {
                sockopt_set_reuseaddr(fd, bool_val);
                // Propagate into socket layer
                if bool_val {
                    let _ = crate::net::socket::sys_setsockopt(
                        fd as u32,
                        crate::net::socket::SocketOption::ReuseAddr,
                        1,
                    );
                } else {
                    let _ = crate::net::socket::sys_setsockopt(
                        fd as u32,
                        crate::net::socket::SocketOption::ReuseAddr,
                        0,
                    );
                }
            }
            SO_REUSEPORT => {
                sockopt_set_reuseport(fd, bool_val);
            }
            SO_BROADCAST => {
                sockopt_set_broadcast(fd, bool_val);
                let v = if bool_val { 1 } else { 0 };
                let _ = crate::net::socket::sys_setsockopt(
                    fd as u32,
                    crate::net::socket::SocketOption::Broadcast,
                    v,
                );
            }
            SO_KEEPALIVE => {
                sockopt_set_keepalive(fd, bool_val);
                let v = if bool_val { 1 } else { 0 };
                let _ = crate::net::socket::sys_setsockopt(
                    fd as u32,
                    crate::net::socket::SocketOption::KeepAlive,
                    v,
                );
            }
            SO_SNDBUF | SO_SNDBUFFORCE => {
                if u32_val == 0 {
                    return EINVAL;
                }
                sockopt_set_sndbuf(fd, u32_val);
                let _ = crate::net::socket::sys_setsockopt(
                    fd as u32,
                    crate::net::socket::SocketOption::SndBuf,
                    u32_val as usize,
                );
            }
            SO_RCVBUF | SO_RCVBUFFORCE => {
                if u32_val == 0 {
                    return EINVAL;
                }
                sockopt_set_rcvbuf(fd, u32_val);
                let _ = crate::net::socket::sys_setsockopt(
                    fd as u32,
                    crate::net::socket::SocketOption::RcvBuf,
                    u32_val as usize,
                );
            }
            SO_RCVLOWAT => {
                if u32_val == 0 {
                    return EINVAL;
                }
                sockopt_set_rcvlowat(fd, u32_val);
            }
            SO_SNDLOWAT => {
                if u32_val == 0 {
                    return EINVAL;
                }
                sockopt_set_sndlowat(fd, u32_val);
            }
            SO_RCVTIMEO => {
                // Caller passes timeval (tv_sec u64 + tv_usec u64 = 16 bytes).
                // If optlen >= 16, read full timeval; otherwise treat as ms.
                let ms_val: u64 = if optlen >= 8 {
                    let sec: u64 = unsafe { (optval as *const u64).read_volatile() };
                    let usec: u64 = if optlen >= 16 {
                        unsafe { ((optval + 8) as *const u64).read_volatile() }
                    } else {
                        0
                    };
                    sec.saturating_mul(1000).saturating_add(usec / 1000)
                } else {
                    u32_val as u64
                };
                sockopt_set_rcvtimeo(fd, ms_val);
            }
            SO_SNDTIMEO => {
                let ms_val: u64 = if optlen >= 8 {
                    let sec: u64 = unsafe { (optval as *const u64).read_volatile() };
                    let usec: u64 = if optlen >= 16 {
                        unsafe { ((optval + 8) as *const u64).read_volatile() }
                    } else {
                        0
                    };
                    sec.saturating_mul(1000).saturating_add(usec / 1000)
                } else {
                    u32_val as u64
                };
                sockopt_set_sndtimeo(fd, ms_val);
            }
            SO_LINGER => {
                // struct linger { int l_onoff; int l_linger; }  (8 bytes)
                if optlen < 8 {
                    return EINVAL;
                }
                let l_onoff: i32 = unsafe { (optval as *const i32).read_volatile() };
                let l_linger: i32 = unsafe { ((optval + 4) as *const i32).read_volatile() };
                let secs: i16 = if l_onoff == 0 {
                    -1 // disabled
                } else if l_linger <= 0 {
                    0 // immediate RST on close
                } else if l_linger > 32767 {
                    32767
                } else {
                    l_linger as i16
                };
                sockopt_set_linger(fd, secs);
                let linger_val: usize = if l_onoff == 0 { 0 } else { l_linger as usize };
                let _ = crate::net::socket::sys_setsockopt(
                    fd as u32,
                    crate::net::socket::SocketOption::Linger,
                    linger_val,
                );
            }
            SO_OOBINLINE => {
                sockopt_set_oobinline(fd, bool_val);
            }
            SO_DONTROUTE => {
                sockopt_set_dontroute(fd, bool_val);
            }
            SO_ERROR => {
                // SO_ERROR is read-only; ignore silently (Linux behaviour)
            }
            SO_TYPE => {
                // SO_TYPE is read-only
                return ENOPROTOOPT;
            }
            _ => return ENOPROTOOPT,
        },

        // ── IPPROTO_TCP ─────────────────────────────────────────────────────
        IPPROTO_TCP => match optname {
            TCP_NODELAY => {
                sockopt_set_tcp_nodelay(fd, bool_val);
                // Propagate into TCP connection layer
                if let Some(opts) = sockopt_get(fd) {
                    if opts.occupied {
                        // Look up the socket's TCP connection id through the
                        // socket table, then call tcp::set_nodelay.
                        // We use the public socket sys_setsockopt bridge for this.
                        let _ = crate::net::socket::sys_setsockopt(
                            fd as u32,
                            crate::net::socket::SocketOption::TcpNoDelay,
                            if bool_val { 1 } else { 0 },
                        );
                    }
                }
            }
            TCP_CORK => {
                // Cork: buffer small writes until uncorked or MSS full.
                // Complementary to Nagle (nodelay=false enables Nagle).
                sockopt_set_tcp_cork(fd, bool_val);
            }
            TCP_KEEPIDLE => {
                sockopt_set_tcp_keepidle(fd, u32_val);
            }
            TCP_KEEPINTVL => {
                sockopt_set_tcp_keepintvl(fd, u32_val);
            }
            TCP_KEEPCNT => {
                let cnt = if u32_val > 255 { 255 } else { u32_val as u8 };
                sockopt_set_tcp_keepcnt(fd, cnt);
            }
            TCP_MAXSEG => {
                // MSS must be between 88 and 65495
                if u32_val < 88 || u32_val > 65495 {
                    return EINVAL;
                }
                sockopt_set_tcp_maxseg(fd, u32_val as u16);
            }
            TCP_QUICKACK => {
                sockopt_set_tcp_quickack(fd, bool_val);
            }
            TCP_SYNCNT => {
                let cnt = if u32_val > 127 { 127 } else { u32_val as u8 };
                sockopt_set_tcp_syncnt(fd, cnt);
            }
            TCP_LINGER2 => {
                let v: i32 = unsafe { (optval as *const i32).read_volatile() };
                sockopt_set_tcp_linger2(fd, v);
            }
            TCP_DEFER_ACCEPT => {
                sockopt_set_tcp_defer_accept(fd, u32_val);
            }
            TCP_WINDOW_CLAMP => {
                sockopt_set_tcp_window_clamp(fd, u32_val);
            }
            TCP_USER_TIMEOUT => {
                sockopt_set_tcp_user_timeout(fd, u32_val);
            }
            TCP_CONGESTION => {
                // We only support the built-in congestion algorithm.
                // Accept the call but ignore the name string (no-op).
            }
            TCP_FASTOPEN => {
                // TCP Fast Open — accept as a hint; no-op in this implementation.
            }
            TCP_INFO => {
                // TCP_INFO is read-only
                return ENOPROTOOPT;
            }
            _ => return ENOPROTOOPT,
        },

        // ── IPPROTO_IP ──────────────────────────────────────────────────────
        IPPROTO_IP => match optname {
            IP_TTL => {
                if u32_val > 255 {
                    return EINVAL;
                }
                sockopt_set_ip_ttl(fd, u32_val as u8);
            }
            IP_TOS => {
                if u32_val > 255 {
                    return EINVAL;
                }
                sockopt_set_ip_tos(fd, u32_val as u8);
            }
            IP_HDRINCL => {
                sockopt_set_ip_hdrincl(fd, bool_val);
            }
            IP_OPTIONS => {
                // Raw IP options header manipulation — accept silently (no-op).
            }
            IP_MULTICAST_TTL => {
                // Multicast TTL — store in ip_ttl (for mc sockets, repurposed).
                if u32_val > 255 {
                    return EINVAL;
                }
                sockopt_set_ip_ttl(fd, u32_val as u8);
            }
            IP_MULTICAST_LOOP => {
                // Multicast loopback — no-op (hardware loop not implemented).
            }
            IP_ADD_MEMBERSHIP | IP_DROP_MEMBERSHIP => {
                // Multicast group join/leave — requires IGMP.
                // optval points to struct ip_mreq { mcast_addr(4), iface_addr(4) }.
                // For now, accept and no-op; full IGMP integration left to igmp module.
            }
            _ => return ENOPROTOOPT,
        },

        _ => return ENOPROTOOPT,
    }

    0 // success
}

// ---------------------------------------------------------------------------
// getsockopt(2) — syscall entry point
// ---------------------------------------------------------------------------

/// sys_getsockopt — retrieve a socket option value into a user buffer.
///
/// # Parameters
/// * `fd`       — socket file descriptor
/// * `level`    — option level
/// * `optname`  — option identifier constant
/// * `optval`   — pointer (as u64) to user memory to receive the value
/// * `optlen`   — pointer (as u64) to a u32 holding the buffer size on entry;
///                updated to the actual value size on return
///
/// # Returns
/// 0 on success, negative errno on failure.
///
/// # Safety
/// The caller must validate both `optval` and `optlen` pointers before calling.
pub fn sys_getsockopt(fd: i32, level: i32, optname: i32, optval: u64, optlen: u64) -> i64 {
    if optval == 0 || optlen == 0 {
        return EINVAL;
    }

    // Validate fd
    let idx = match fd_to_idx(fd) {
        Some(i) => i,
        None => return EBADF,
    };

    let opts: SockOpts = {
        let table = SOCK_OPTS_TABLE.lock();
        if !table[idx].occupied || table[idx].fd != fd as u32 {
            return ENOTSOCK;
        }
        table[idx]
    };

    // Helper: write a u32 to user memory and update optlen.
    // SAFETY: caller validated pointer.
    #[inline]
    unsafe fn write_u32(optval: u64, optlen: u64, value: u32) {
        // Write value
        (optval as *mut u32).write_volatile(value);
        // Update *optlen = 4
        (optlen as *mut u32).write_volatile(4);
    }

    // Helper: write an i32.
    #[inline]
    unsafe fn write_i32(optval: u64, optlen: u64, value: i32) {
        (optval as *mut i32).write_volatile(value);
        (optlen as *mut u32).write_volatile(4);
    }

    match level {
        // ── SOL_SOCKET ──────────────────────────────────────────────────────
        SOL_SOCKET => match optname {
            SO_TYPE => {
                // Return SOCK_STREAM(1), SOCK_DGRAM(2), SOCK_RAW(3).
                let sock_type: u32 = crate::net::socket::socket_type(fd as u32)
                    .map(|t| match t {
                        crate::net::socket::SocketType::Stream => 1u32,
                        crate::net::socket::SocketType::Datagram => 2u32,
                        crate::net::socket::SocketType::Raw => 3u32,
                    })
                    .unwrap_or(0u32);
                unsafe { write_u32(optval, optlen, sock_type) };
            }
            SO_ERROR => {
                let err = opts.so_error;
                unsafe { write_i32(optval, optlen, err) };
                // Clear pending error after read (matches Linux semantics)
                sockopt_clear_error(fd);
            }
            SO_DEBUG => unsafe { write_u32(optval, optlen, opts.so_debug as u32) },
            SO_REUSEADDR => unsafe { write_u32(optval, optlen, opts.so_reuseaddr as u32) },
            SO_REUSEPORT => unsafe { write_u32(optval, optlen, opts.so_reuseport as u32) },
            SO_BROADCAST => unsafe { write_u32(optval, optlen, opts.so_broadcast as u32) },
            SO_KEEPALIVE => unsafe { write_u32(optval, optlen, opts.so_keepalive as u32) },
            SO_SNDBUF => unsafe { write_u32(optval, optlen, opts.so_sndbuf) },
            SO_RCVBUF => unsafe { write_u32(optval, optlen, opts.so_rcvbuf) },
            SO_RCVLOWAT => unsafe { write_u32(optval, optlen, opts.so_rcvlowat) },
            SO_SNDLOWAT => unsafe { write_u32(optval, optlen, opts.so_sndlowat) },
            SO_OOBINLINE => unsafe { write_u32(optval, optlen, opts.so_oobinline as u32) },
            SO_DONTROUTE => unsafe { write_u32(optval, optlen, opts.so_dontroute as u32) },
            SO_SNDBUFFORCE => unsafe { write_u32(optval, optlen, opts.so_sndbuf) },
            SO_RCVBUFFORCE => unsafe { write_u32(optval, optlen, opts.so_rcvbuf) },
            SO_RCVTIMEO => {
                // Return struct timeval { tv_sec u64, tv_usec u64 }
                let ms = opts.so_rcvtimeo_ms;
                let sec = ms / 1000;
                let usec = (ms % 1000).saturating_mul(1000);
                // Check there is enough space (16 bytes) before writing
                let len_ptr = optlen as *mut u32;
                let buf_len = unsafe { len_ptr.read_volatile() } as usize;
                if buf_len >= 8 {
                    unsafe { (optval as *mut u64).write_volatile(sec) };
                    if buf_len >= 16 {
                        unsafe { ((optval + 8) as *mut u64).write_volatile(usec) };
                        unsafe { len_ptr.write_volatile(16) };
                    } else {
                        unsafe { len_ptr.write_volatile(8) };
                    }
                } else {
                    unsafe { write_u32(optval, optlen, ms as u32) };
                }
            }
            SO_SNDTIMEO => {
                let ms = opts.so_sndtimeo_ms;
                let sec = ms / 1000;
                let usec = (ms % 1000).saturating_mul(1000);
                let len_ptr = optlen as *mut u32;
                let buf_len = unsafe { len_ptr.read_volatile() } as usize;
                if buf_len >= 8 {
                    unsafe { (optval as *mut u64).write_volatile(sec) };
                    if buf_len >= 16 {
                        unsafe { ((optval + 8) as *mut u64).write_volatile(usec) };
                        unsafe { len_ptr.write_volatile(16) };
                    } else {
                        unsafe { len_ptr.write_volatile(8) };
                    }
                } else {
                    unsafe { write_u32(optval, optlen, ms as u32) };
                }
            }
            SO_LINGER => {
                // Return struct linger { int l_onoff; int l_linger; }
                let len_ptr = optlen as *mut u32;
                let buf_len = unsafe { len_ptr.read_volatile() } as usize;
                if buf_len < 8 {
                    return EINVAL;
                }
                let (onoff, linger) = if opts.so_linger_secs < 0 {
                    (0i32, 0i32)
                } else {
                    (1i32, opts.so_linger_secs as i32)
                };
                unsafe {
                    (optval as *mut i32).write_volatile(onoff);
                    ((optval + 4) as *mut i32).write_volatile(linger);
                    len_ptr.write_volatile(8);
                }
            }
            _ => return ENOPROTOOPT,
        },

        // ── IPPROTO_TCP ─────────────────────────────────────────────────────
        IPPROTO_TCP => match optname {
            TCP_NODELAY => unsafe { write_u32(optval, optlen, opts.tcp_nodelay as u32) },
            TCP_CORK => unsafe { write_u32(optval, optlen, opts.tcp_cork as u32) },
            TCP_KEEPIDLE => unsafe { write_u32(optval, optlen, opts.tcp_keepidle) },
            TCP_KEEPINTVL => unsafe { write_u32(optval, optlen, opts.tcp_keepintvl) },
            TCP_KEEPCNT => unsafe { write_u32(optval, optlen, opts.tcp_keepcnt as u32) },
            TCP_MAXSEG => unsafe { write_u32(optval, optlen, opts.tcp_maxseg as u32) },
            TCP_QUICKACK => unsafe { write_u32(optval, optlen, opts.tcp_quickack as u32) },
            TCP_SYNCNT => unsafe { write_u32(optval, optlen, opts.tcp_syncnt as u32) },
            TCP_LINGER2 => unsafe { write_i32(optval, optlen, opts.tcp_linger2) },
            TCP_DEFER_ACCEPT => unsafe { write_u32(optval, optlen, opts.tcp_defer_accept) },
            TCP_WINDOW_CLAMP => unsafe { write_u32(optval, optlen, opts.tcp_window_clamp) },
            TCP_USER_TIMEOUT => unsafe { write_u32(optval, optlen, opts.tcp_user_timeout) },
            TCP_INFO => {
                // tcp_info struct is large (104 bytes).  Fill with zeros for now
                // so that applications that check the pointer don't fault.
                let len_ptr = optlen as *mut u32;
                let buf_len = unsafe { len_ptr.read_volatile() } as usize;
                let fill_len = buf_len.min(104);
                unsafe {
                    core::ptr::write_bytes(optval as *mut u8, 0, fill_len);
                    len_ptr.write_volatile(fill_len as u32);
                }
            }
            TCP_FASTOPEN => unsafe { write_u32(optval, optlen, 0) },
            _ => return ENOPROTOOPT,
        },

        // ── IPPROTO_IP ──────────────────────────────────────────────────────
        IPPROTO_IP => match optname {
            IP_TTL => unsafe { write_u32(optval, optlen, opts.ip_ttl as u32) },
            IP_TOS => unsafe { write_u32(optval, optlen, opts.ip_tos as u32) },
            IP_HDRINCL => unsafe { write_u32(optval, optlen, opts.ip_hdrincl as u32) },
            IP_MULTICAST_TTL => unsafe { write_u32(optval, optlen, opts.ip_ttl as u32) },
            IP_MULTICAST_LOOP => unsafe { write_u32(optval, optlen, 0) },
            _ => return ENOPROTOOPT,
        },

        _ => return ENOPROTOOPT,
    }

    0 // success
}

// ---------------------------------------------------------------------------
// TCP keepalive tick
// ---------------------------------------------------------------------------

/// Advance the keepalive state machine for a single TCP connection.
///
/// Call this from the periodic network timer (e.g. `tcp_retransmit_check`) once
/// per keepalive tick interval (typically every second).
///
/// # Parameters
/// * `tcp_conn_idx` — index into the TCP connection table (as understood by the
///                    TCP module, not the fd table)
/// * `fd`           — socket fd owning this connection (used to look up options)
///
/// # Behaviour
/// 1. If `SO_KEEPALIVE` is not set, returns immediately.
/// 2. If the connection has been idle for `TCP_KEEPIDLE` seconds, schedules
///    a keepalive probe via `tcp::set_keepalive`.
/// 3. The TCP module already implements the probe/count logic internally; this
///    function just synchronises the tunable parameters (keepidle, keepintvl,
///    keepcnt) from the sockopt table into the TCP connection before the tick.
pub fn tcp_keepalive_tick(tcp_conn_id: u32, fd: i32) {
    // Look up options for this fd
    let opts = match sockopt_get(fd) {
        Some(o) => o,
        None => return,
    };

    if !opts.so_keepalive {
        return;
    }

    // We can't call into the TCP module's internals without going through the
    // public API; the TCP module owns its own keepalive timers.  What we do here
    // is ensure that keepalive is enabled on the connection and that the
    // per-connection parameters are consistent with the socket options.
    //
    // tcp::set_keepalive just sets the keepalive_enabled flag; the timing is
    // handled by tcp::check_keepalive() which is called from tcp_timer_tick().
    crate::net::tcp::set_keepalive(tcp_conn_id, true);

    // Future: if TCP exposes set_keepidle / set_keepintvl / set_keepcnt, call
    // them here.  For now the TCP defaults are used; overriding them requires
    // extending the tcp module's TcpConnection struct.
    let _ = opts.tcp_keepidle;
    let _ = opts.tcp_keepintvl;
    let _ = opts.tcp_keepcnt;
}

// ---------------------------------------------------------------------------
// SO_REUSEPORT — multi-socket load balancing
// ---------------------------------------------------------------------------

/// Maximum number of ports that can have REUSEPORT groups.
const REUSEPORT_MAX_PORTS: usize = 32;

/// Maximum number of fds in a single REUSEPORT group.
const REUSEPORT_MAX_FDS: usize = 8;

/// One entry in the REUSEPORT table.
#[derive(Copy, Clone)]
struct ReusePortEntry {
    port: u16,
    fds: [i32; REUSEPORT_MAX_FDS],
    count: u8,
    used: bool,
}

impl ReusePortEntry {
    const fn empty() -> Self {
        ReusePortEntry {
            port: 0,
            fds: [0i32; REUSEPORT_MAX_FDS],
            count: 0,
            used: false,
        }
    }
}

static REUSEPORT_TABLE: Mutex<[ReusePortEntry; REUSEPORT_MAX_PORTS]> =
    Mutex::new([ReusePortEntry::empty(); REUSEPORT_MAX_PORTS]);

/// Register `fd` as willing to share `port` with other sockets.
///
/// Returns `true` on success, `false` if the group is full or there is no space
/// in the REUSEPORT table.
pub fn reuseport_register(port: u16, fd: i32) -> bool {
    let mut table = REUSEPORT_TABLE.lock();

    // Find existing group for this port, or an empty slot.
    let mut found: Option<usize> = None;
    let mut empty: Option<usize> = None;

    for (i, entry) in table.iter().enumerate() {
        if entry.used && entry.port == port {
            found = Some(i);
            break;
        }
        if !entry.used && empty.is_none() {
            empty = Some(i);
        }
    }

    let idx = match found {
        Some(i) => i,
        None => match empty {
            Some(i) => {
                table[i].used = true;
                table[i].port = port;
                table[i].count = 0;
                i
            }
            None => return false, // table full
        },
    };

    let entry = &mut table[idx];
    if entry.count as usize >= REUSEPORT_MAX_FDS {
        return false; // group full
    }

    // De-duplicate
    for i in 0..entry.count as usize {
        if entry.fds[i] == fd {
            return true; // already registered
        }
    }

    let slot = entry.count as usize;
    entry.fds[slot] = fd;
    entry.count = entry.count.saturating_add(1);
    true
}

/// Select an fd from the REUSEPORT group for `port` using `hash` as the key.
///
/// Uses `hash % count` to pick a socket — gives uniform distribution when
/// hash comes from a 4-tuple (src_ip, src_port, dst_ip, dst_port).
/// Returns `None` if no group is registered for `port`.
pub fn reuseport_lookup(port: u16, hash: u32) -> Option<i32> {
    let table = REUSEPORT_TABLE.lock();
    for entry in table.iter() {
        if entry.used && entry.port == port && entry.count > 0 {
            let idx = (hash % entry.count as u32) as usize;
            return Some(entry.fds[idx]);
        }
    }
    None
}

/// Remove all REUSEPORT registrations for the given `fd`.
///
/// Compacts the fd list within each affected group.  If a group becomes empty
/// the slot is recycled.
pub fn reuseport_deregister(fd: i32) {
    let mut table = REUSEPORT_TABLE.lock();
    for entry in table.iter_mut() {
        if !entry.used {
            continue;
        }
        // Find fd in this group
        let mut new_count: u8 = 0;
        let mut new_fds = [0i32; REUSEPORT_MAX_FDS];
        for i in 0..entry.count as usize {
            if entry.fds[i] != fd {
                new_fds[new_count as usize] = entry.fds[i];
                new_count = new_count.saturating_add(1);
            }
        }
        entry.fds = new_fds;
        entry.count = new_count;
        if entry.count == 0 {
            entry.used = false;
            entry.port = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the sockopt subsystem.
///
/// Called from `net::init()`.  The static tables are already zero-initialised
/// by the linker (all fields default to their `const fn empty()` values), so
/// this is a no-op marker function that confirms the module is active.
pub fn init() {
    // Nothing to do — statics are const-initialised.
    // Kept as a hook for future expansion (e.g. reading sysctl defaults from
    // an early config structure).
}
