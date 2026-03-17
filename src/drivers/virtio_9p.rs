/// VirtIO-9P Filesystem Driver — no-heap, static-buffer implementation
///
/// Exposes a 9P filesystem to the guest via VirtIO transport.
/// Used by QEMU to share host directories with the kernel guest.
///
/// Protocol: 9P2000.u (Unix extensions of the 9P2000 protocol).
/// Transport: VirtIO legacy PCI (vendor 0x1AF4, device 0x1009).
///
/// All buffers and state are static; no Vec, Box, String, or allocator calls.
/// Identity mapping assumed: virtual address == physical address for statics.
///
/// Public API:
///   init()                         — probe device, print status
///   v9p_init()         -> bool     — probe PCI device, initialise atomics
///   p9_version()       -> bool     — negotiate 9P2000.u version
///   p9_attach(...)     -> Option<u32> — attach to root, return fid
///   p9_walk(...)       -> bool     — walk path components
///   p9_read(...)       -> u32      — read from open fid
///   p9_write(...)      -> u32      — write to open fid
///   p9_clunk(...)      -> bool     — clunk (release) a fid
///   p9_alloc_fid()     -> Option<u32>
///
/// SAFETY RULES (enforced throughout):
///   - No as f32 / as f64
///   - No unwrap() / expect() / panic!()
///   - saturating_add / saturating_sub for all counters
///   - wrapping_add for tag / fid sequence numbers
///   - bounds-checked array accesses, early return on out-of-bounds
///   - write_volatile / read_volatile for all shared-memory ring I/O
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};

// ============================================================================
// PCI IDs
// ============================================================================

/// VirtIO PCI vendor (standard for all VirtIO devices)
pub const VIRTIO_9P_VENDOR: u16 = 0x1AF4;
/// VirtIO-9P PCI device ID
pub const VIRTIO_9P_DEVICE: u16 = 0x1009;

// ============================================================================
// 9P protocol constants
// ============================================================================

/// Version string for the 9P2000.u Unix dialect
pub const P9_VERSION_UNIX: &[u8] = b"9P2000.u";
/// Special tag meaning "no tag" (used in TVERSION)
pub const P9_NOTAG: u16 = 0xFFFF;
/// Special fid meaning "no fid" (used in TATTACH for afid when not authenticating)
pub const P9_NOFID: u32 = 0xFFFF_FFFF;
/// Maximum number of path components in a single TWALK message
pub const P9_MAX_WNAMES: usize = 16;
/// Maximum message size negotiated with the server
pub const P9_MAX_MSIZE: u32 = 8192;

// ============================================================================
// 9P message type codes (T = request, R = response)
// ============================================================================

pub const P9_TVERSION: u8 = 100;
pub const P9_RVERSION: u8 = 101;
pub const P9_TATTACH: u8 = 104;
pub const P9_RATTACH: u8 = 105;
pub const P9_TWALK: u8 = 110;
pub const P9_RWALK: u8 = 111;
pub const P9_TREAD: u8 = 116;
pub const P9_RREAD: u8 = 117;
pub const P9_TWRITE: u8 = 118;
pub const P9_RWRITE: u8 = 119;
pub const P9_TCLUNK: u8 = 120;
pub const P9_RCLUNK: u8 = 121;

// ============================================================================
// 9P data types
// ============================================================================

/// 9P Qid — unique identifier for a server-side file/directory object.
///
/// qid_type: QTDIR (0x80) = directory, 0 = regular file.
/// version:  monotonically-increasing modification counter.
/// path:     unique inode-like 64-bit path identifier.
#[derive(Clone, Copy)]
pub struct P9Qid {
    pub qid_type: u8,
    pub version: u32,
    pub path: u64,
}

impl P9Qid {
    pub const fn zero() -> Self {
        P9Qid {
            qid_type: 0,
            version: 0,
            path: 0,
        }
    }
}

/// A tracked open file identifier (client side).
///
/// fid:    the numeric fid assigned by this client.
/// qid:    the server Qid associated with this fid after open/walk.
/// iounit: maximum atomic I/O size reported by the server.
/// active: true when this slot is in use.
#[derive(Clone, Copy)]
pub struct P9Fid {
    pub fid: u32,
    pub qid: P9Qid,
    pub iounit: u32,
    pub active: bool,
}

impl P9Fid {
    pub const fn empty() -> Self {
        P9Fid {
            fid: 0,
            qid: P9Qid::zero(),
            iounit: 0,
            active: false,
        }
    }
}

// ============================================================================
// Static device state — atomics for lock-free fast-path reads
// ============================================================================

/// True once a VirtIO-9P device has been successfully probed.
static P9_PRESENT: AtomicBool = AtomicBool::new(false);

/// I/O BAR0 base (legacy PCI I/O port base of the VirtIO device).
static P9_IO_BASE: AtomicU16 = AtomicU16::new(0);

/// Next tag value (wrapping sequence, P9_NOTAG is skipped).
static P9_NEXT_TAG: AtomicU16 = AtomicU16::new(1);

/// Next fid value (wrapping sequence, P9_NOFID is skipped).
static P9_NEXT_FID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Negotiated message size (stored after successful TVERSION exchange).
static P9_MSIZE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(P9_MAX_MSIZE);

// ============================================================================
// Static fid table — Mutex-guarded fixed-size array
// ============================================================================

/// Table of all currently active fids.
static P9_FIDS: Mutex<[P9Fid; 32]> = Mutex::new([P9Fid::empty(); 32]);

// ============================================================================
// Message scratch buffers — one for outgoing, one for incoming.
// Both are 8192 bytes (= P9_MAX_MSIZE).
// ============================================================================

/// Outgoing 9P message scratch buffer (single-threaded kernel: no contention).
static P9_TX_BUF: Mutex<[u8; 8192]> = Mutex::new([0u8; 8192]);

/// Incoming 9P reply scratch buffer.
static P9_RX_BUF: Mutex<[u8; 8192]> = Mutex::new([0u8; 8192]);

// ============================================================================
// Tag / fid allocation helpers
// ============================================================================

/// Allocate the next unique tag, skipping P9_NOTAG (0xFFFF).
fn alloc_tag() -> u16 {
    loop {
        let t = P9_NEXT_TAG.fetch_add(1, Ordering::Relaxed);
        if t != P9_NOTAG {
            return t;
        }
        // Skip 0xFFFF; wrapping will eventually cycle back to 1
    }
}

/// Allocate the next unique fid, skipping P9_NOFID (0xFFFF_FFFF).
fn alloc_raw_fid() -> u32 {
    loop {
        let f = P9_NEXT_FID.fetch_add(1, Ordering::Relaxed);
        if f != P9_NOFID {
            return f;
        }
    }
}

// ============================================================================
// 9P message encoding helpers
// ============================================================================

/// Write a little-endian u16 into `buf` at `*pos`, advancing `*pos` by 2.
/// Does nothing if `*pos + 2 > buf.len()`.
pub fn p9_encode_u16(buf: &mut [u8; 8192], pos: &mut usize, v: u16) {
    if *pos + 2 > buf.len() {
        return;
    }
    buf[*pos] = (v & 0xFF) as u8;
    buf[*pos + 1] = ((v >> 8) & 0xFF) as u8;
    *pos = pos.saturating_add(2);
}

/// Write a little-endian u32 into `buf` at `*pos`, advancing `*pos` by 4.
/// Does nothing if `*pos + 4 > buf.len()`.
pub fn p9_encode_u32(buf: &mut [u8; 8192], pos: &mut usize, v: u32) {
    if *pos + 4 > buf.len() {
        return;
    }
    buf[*pos] = (v & 0xFF) as u8;
    buf[*pos + 1] = ((v >> 8) & 0xFF) as u8;
    buf[*pos + 2] = ((v >> 16) & 0xFF) as u8;
    buf[*pos + 3] = ((v >> 24) & 0xFF) as u8;
    *pos = pos.saturating_add(4);
}

/// Write a little-endian u64 into `buf` at `*pos`, advancing `*pos` by 8.
/// Does nothing if `*pos + 8 > buf.len()`.
fn p9_encode_u64(buf: &mut [u8; 8192], pos: &mut usize, v: u64) {
    if *pos + 8 > buf.len() {
        return;
    }
    let lo = (v & 0xFFFF_FFFF) as u32;
    let hi = ((v >> 32) & 0xFFFF_FFFF) as u32;
    p9_encode_u32(buf, pos, lo);
    p9_encode_u32(buf, pos, hi);
}

/// Write a 9P string (2-byte length prefix + raw bytes) into `buf`.
/// If the string plus length would overflow the buffer, nothing is written.
pub fn p9_encode_str(buf: &mut [u8; 8192], pos: &mut usize, s: &[u8]) {
    let slen = s.len();
    if slen > 0xFFFF {
        return;
    }
    if *pos + 2 + slen > buf.len() {
        return;
    }
    p9_encode_u16(buf, pos, slen as u16);
    let start = *pos;
    // Copy bytes individually — no slice::copy_from_slice to avoid
    // accidentally pulling in alloc.  A raw loop is explicit and safe here.
    let mut i = 0usize;
    while i < slen {
        if start + i >= buf.len() {
            break;
        }
        buf[start + i] = s[i];
        i = i.saturating_add(1);
    }
    *pos = pos.saturating_add(slen);
}

/// Write the 7-byte 9P message header into `buf[0..7]`.
///
/// Layout:
///   [0..4]  size   — total message length including this header (little-endian u32)
///   [4]     type   — message type byte (P9_T* / P9_R*)
///   [5..7]  tag    — message tag (little-endian u16)
pub fn p9_msg_header(buf: &mut [u8; 8192], size: u32, msg_type: u8, tag: u16) {
    let mut pos = 0usize;
    p9_encode_u32(buf, &mut pos, size);
    if pos < buf.len() {
        buf[pos] = msg_type;
        pos = pos.saturating_add(1);
    }
    p9_encode_u16(buf, &mut pos, tag);
}

// ============================================================================
// 9P decoding helpers (read from reply buffer)
// ============================================================================

/// Read a little-endian u16 from `buf` at `*pos`, advancing `*pos` by 2.
/// Returns 0 on out-of-bounds.
fn p9_decode_u16(buf: &[u8; 8192], pos: &mut usize, len: usize) -> u16 {
    if *pos + 2 > len {
        return 0;
    }
    let v = (buf[*pos] as u16) | ((buf[*pos + 1] as u16) << 8);
    *pos = pos.saturating_add(2);
    v
}

/// Read a little-endian u32 from `buf` at `*pos`, advancing `*pos` by 4.
/// Returns 0 on out-of-bounds.
fn p9_decode_u32(buf: &[u8; 8192], pos: &mut usize, len: usize) -> u32 {
    if *pos + 4 > len {
        return 0;
    }
    let v = (buf[*pos] as u32)
        | ((buf[*pos + 1] as u32) << 8)
        | ((buf[*pos + 2] as u32) << 16)
        | ((buf[*pos + 3] as u32) << 24);
    *pos = pos.saturating_add(4);
    v
}

/// Read a little-endian u64 from `buf` at `*pos`, advancing `*pos` by 8.
/// Returns 0 on out-of-bounds.
fn p9_decode_u64(buf: &[u8; 8192], pos: &mut usize, len: usize) -> u64 {
    let lo = p9_decode_u32(buf, pos, len) as u64;
    let hi = p9_decode_u32(buf, pos, len) as u64;
    lo | (hi << 32)
}

/// Decode a 9P Qid (13 bytes: u8 + u32 + u64) from `buf`.
fn p9_decode_qid(buf: &[u8; 8192], pos: &mut usize, len: usize) -> P9Qid {
    if *pos + 13 > len {
        return P9Qid::zero();
    }
    let qid_type = buf[*pos];
    *pos = pos.saturating_add(1);
    let version = p9_decode_u32(buf, pos, len);
    let path = p9_decode_u64(buf, pos, len);
    P9Qid {
        qid_type,
        version,
        path,
    }
}

// ============================================================================
// VirtIO send/receive stub
// ============================================================================

/// Send a 9P message to the VirtIO device and receive its reply.
///
/// This is a **stub implementation** — it echoes the request back as the
/// reply.  A full implementation would:
///   1. Place `msg[0..msg_len]` into VirtQueue descriptor (device-readable).
///   2. Place `reply[0..8192]` into a second descriptor (device-writable).
///   3. Update the available ring and notify the device via the queue notify
///      register (`io_base + VIRTIO_REG_QUEUE_NOTIFY`).
///   4. Spin-poll the used ring until the device posts a used descriptor.
///   5. Read back `reply_len` from the used-ring element's `len` field.
///
/// For now the stub satisfies the borrow-checker and the module compiles
/// correctly so the real VirtQueue plumbing can be wired in later.
pub fn p9_send_recv(
    msg: &[u8],
    msg_len: usize,
    reply: &mut [u8; 8192],
    reply_len: &mut usize,
) -> bool {
    if !P9_PRESENT.load(Ordering::Relaxed) {
        return false;
    }
    if msg_len == 0 || msg_len > 8192 {
        return false;
    }

    // Stub: echo the message back verbatim.
    let copy_len = if msg_len < 8192 { msg_len } else { 8192 };
    let mut i = 0usize;
    while i < copy_len {
        // Safety: both slices are within their declared bounds.
        reply[i] = msg[i];
        i = i.saturating_add(1);
    }
    *reply_len = copy_len;
    true
}

// ============================================================================
// 9P high-level operations
// ============================================================================

/// Negotiate protocol version with the 9P server.
///
/// Sends TVERSION(msize=8192, version="9P2000.u") and validates the RVERSION
/// response.  On success updates the negotiated msize in P9_MSIZE.
pub fn p9_version() -> bool {
    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize; // leave room for header

    // Encode body: msize (u32) + version string
    p9_encode_u32(&mut tx, &mut pos, P9_MAX_MSIZE);
    p9_encode_str(&mut tx, &mut pos, P9_VERSION_UNIX);

    // Write header now that we know total size
    p9_msg_header(&mut tx, pos as u32, P9_TVERSION, P9_NOTAG);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return false;
    }

    // Validate reply header: size(4) + type(1) + tag(2) = 7 bytes minimum
    if rx_len < 7 {
        return false;
    }
    let reply_type = rx[4];
    if reply_type != P9_RVERSION {
        return false;
    }

    // Decode msize from reply body (offset 7)
    let mut rpos = 7usize;
    let negotiated_msize = p9_decode_u32(&rx, &mut rpos, rx_len);
    if negotiated_msize == 0 {
        return false;
    }
    let clamped = if negotiated_msize > P9_MAX_MSIZE {
        P9_MAX_MSIZE
    } else {
        negotiated_msize
    };
    P9_MSIZE.store(clamped, Ordering::Relaxed);
    true
}

/// Attach to the server's root filesystem, returning the root fid.
///
/// `afid`  — authentication fid (pass P9_NOFID to skip auth).
/// `uname` — Unix user name to authenticate as.
/// `aname` — attach name / export path (e.g. b"/" or b"share").
///
/// Returns `Some(fid)` on success, `None` on failure.
pub fn p9_attach(afid: u32, uname: &[u8], aname: &[u8]) -> Option<u32> {
    let fid = alloc_raw_fid();
    let tag = alloc_tag();

    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize;

    // Body: fid(u32) afid(u32) uname(str) aname(str) n_uname(u32)
    p9_encode_u32(&mut tx, &mut pos, fid);
    p9_encode_u32(&mut tx, &mut pos, afid);
    p9_encode_str(&mut tx, &mut pos, uname);
    p9_encode_str(&mut tx, &mut pos, aname);
    // n_uname: 0xFFFFFFFF means "no uid" (anonymous)
    p9_encode_u32(&mut tx, &mut pos, 0xFFFF_FFFF);

    p9_msg_header(&mut tx, pos as u32, P9_TATTACH, tag);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return None;
    }

    // Validate RATTACH: size(4)+type(1)+tag(2)+qid(13) = 20 bytes minimum
    if rx_len < 20 {
        return None;
    }
    if rx[4] != P9_RATTACH {
        return None;
    }

    // Decode the root qid
    let mut rpos = 7usize;
    let qid = p9_decode_qid(&rx, &mut rpos, rx_len);

    // Register fid in table
    let mut fids = P9_FIDS.lock();
    for slot in fids.iter_mut() {
        if !slot.active {
            slot.fid = fid;
            slot.qid = qid;
            slot.iounit = 0;
            slot.active = true;
            return Some(fid);
        }
    }
    // No free slot — fid table full
    None
}

/// Walk a path relative to `fid`, binding the result to `newfid`.
///
/// `path` is a slice of name components (e.g. `&[b"etc", b"passwd"]`).
/// Returns `true` if the server acknowledged all components.
pub fn p9_walk(fid: u32, newfid: u32, path: &[&[u8]]) -> bool {
    let tag = alloc_tag();
    let nwnames = if path.len() > P9_MAX_WNAMES {
        P9_MAX_WNAMES
    } else {
        path.len()
    };

    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize;

    // Body: fid(u32) newfid(u32) nwnames(u16) wnames...
    p9_encode_u32(&mut tx, &mut pos, fid);
    p9_encode_u32(&mut tx, &mut pos, newfid);
    p9_encode_u16(&mut tx, &mut pos, nwnames as u16);
    let mut i = 0usize;
    while i < nwnames {
        p9_encode_str(&mut tx, &mut pos, path[i]);
        i = i.saturating_add(1);
    }

    p9_msg_header(&mut tx, pos as u32, P9_TWALK, tag);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return false;
    }

    // Minimal validation: size(4)+type(1)+tag(2)+nwqids(2) = 9 bytes
    if rx_len < 9 {
        return false;
    }
    if rx[4] != P9_RWALK {
        return false;
    }

    // Confirm all path components were walked
    let mut rpos = 7usize;
    let nwqids = p9_decode_u16(&rx, &mut rpos, rx_len);
    nwqids as usize == nwnames
}

/// Read `count` bytes from `fid` at `offset` into `buf`.
/// Returns the number of bytes actually read (0 on error).
pub fn p9_read(fid: u32, offset: u64, count: u32, buf: &mut [u8; 8192]) -> u32 {
    let tag = alloc_tag();
    let msize = P9_MSIZE.load(Ordering::Relaxed);
    let safe_count = if count > msize { msize } else { count };

    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize;

    // Body: fid(u32) offset(u64) count(u32)
    p9_encode_u32(&mut tx, &mut pos, fid);
    p9_encode_u64(&mut tx, &mut pos, offset);
    p9_encode_u32(&mut tx, &mut pos, safe_count);

    p9_msg_header(&mut tx, pos as u32, P9_TREAD, tag);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return 0;
    }

    // RREAD: size(4)+type(1)+tag(2)+count(4)+data = 11 bytes minimum
    if rx_len < 11 {
        return 0;
    }
    if rx[4] != P9_RREAD {
        return 0;
    }

    let mut rpos = 7usize;
    let data_count = p9_decode_u32(&rx, &mut rpos, rx_len);
    if data_count == 0 {
        return 0;
    }
    let to_copy = if (data_count as usize) < rx_len.saturating_sub(rpos) {
        data_count as usize
    } else {
        rx_len.saturating_sub(rpos)
    };
    let to_copy = if to_copy > 8192 { 8192usize } else { to_copy };

    let mut i = 0usize;
    while i < to_copy {
        if rpos + i >= rx_len {
            break;
        }
        buf[i] = rx[rpos + i];
        i = i.saturating_add(1);
    }
    i as u32
}

/// Write `data` to `fid` at `offset`.
/// Returns the number of bytes accepted by the server (0 on error).
pub fn p9_write(fid: u32, offset: u64, data: &[u8]) -> u32 {
    if data.is_empty() {
        return 0;
    }
    let tag = alloc_tag();
    let msize = P9_MSIZE.load(Ordering::Relaxed);

    let write_len = {
        let candidate = data.len();
        // clamp to msize - 23 (header 7 + fid 4 + offset 8 + count 4)
        let max_payload = (msize as usize).saturating_sub(23);
        if candidate > max_payload {
            max_payload
        } else {
            candidate
        }
    };
    if write_len == 0 {
        return 0;
    }

    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize;

    // Body: fid(u32) offset(u64) count(u32) data[count]
    p9_encode_u32(&mut tx, &mut pos, fid);
    p9_encode_u64(&mut tx, &mut pos, offset);
    p9_encode_u32(&mut tx, &mut pos, write_len as u32);

    // Append raw data bytes
    let mut i = 0usize;
    while i < write_len {
        if pos >= tx.len() {
            break;
        }
        tx[pos] = data[i];
        pos = pos.saturating_add(1);
        i = i.saturating_add(1);
    }

    p9_msg_header(&mut tx, pos as u32, P9_TWRITE, tag);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return 0;
    }

    // RWRITE: size(4)+type(1)+tag(2)+count(4) = 11 bytes
    if rx_len < 11 {
        return 0;
    }
    if rx[4] != P9_RWRITE {
        return 0;
    }

    let mut rpos = 7usize;
    p9_decode_u32(&rx, &mut rpos, rx_len)
}

/// Clunk (release) `fid`, freeing server-side resources.
/// Returns `true` on success.
pub fn p9_clunk(fid: u32) -> bool {
    let tag = alloc_tag();

    let mut tx = P9_TX_BUF.lock();
    let mut rx = P9_RX_BUF.lock();
    let mut pos = 7usize;

    // Body: fid(u32)
    p9_encode_u32(&mut tx, &mut pos, fid);
    p9_msg_header(&mut tx, pos as u32, P9_TCLUNK, tag);

    let tx_slice: &[u8] = &*tx;
    let mut rx_len = 0usize;
    if !p9_send_recv(tx_slice, pos, &mut rx, &mut rx_len) {
        return false;
    }

    // RCLUNK: size(4)+type(1)+tag(2) = 7 bytes
    if rx_len < 7 {
        return false;
    }
    if rx[4] != P9_RCLUNK {
        return false;
    }

    // Mark the fid slot as inactive
    let mut fids = P9_FIDS.lock();
    for slot in fids.iter_mut() {
        if slot.active && slot.fid == fid {
            slot.active = false;
            break;
        }
    }
    true
}

/// Allocate a fresh, unused fid number.
///
/// Checks the active fid table to avoid collisions, then returns the next
/// available sequence number.  Returns `None` if the fid table is full.
pub fn p9_alloc_fid() -> Option<u32> {
    // Ensure there is at least one free slot before burning a fid number.
    let fids = P9_FIDS.lock();
    let free = fids.iter().any(|s| !s.active);
    drop(fids);
    if !free {
        return None;
    }
    Some(alloc_raw_fid())
}

// ============================================================================
// Device probe
// ============================================================================

/// Probe for a VirtIO-9P PCI device.
///
/// If found, records `io_base` in the atomic and sets `P9_PRESENT`.
/// Returns `true` if a device was found and initialised.
pub fn v9p_init() -> bool {
    match crate::drivers::virtio::pci_find_virtio(VIRTIO_9P_VENDOR, VIRTIO_9P_DEVICE) {
        None => false,
        Some((io_base, _bus, _dev, _func)) => {
            P9_IO_BASE.store(io_base, Ordering::Relaxed);
            P9_PRESENT.store(true, Ordering::Relaxed);
            true
        }
    }
}

// ============================================================================
// Module init — called from drivers::init()
// ============================================================================

/// Initialise the VirtIO-9P driver.
///
/// Probes for the device and prints status.  On success also negotiates the
/// 9P version so the device is immediately usable by callers.
pub fn init() {
    let found = v9p_init();
    serial_println!(
        "[virtio_9p] plan9 filesystem driver initialized (device found: {})",
        if found { "yes" } else { "no" }
    );
    if found {
        if p9_version() {
            serial_println!(
                "[virtio_9p] 9P2000.u version negotiated (msize={})",
                P9_MSIZE.load(Ordering::Relaxed)
            );
        } else {
            serial_println!("[virtio_9p] warning: version negotiation failed (stub mode)");
        }
    }
}
