/// TUN/TAP virtual network interface for Genesis
///
/// TUN operates at the IP (layer-3) level: userspace reads and writes raw
/// IPv4/IPv6 packets.  TAP operates at the Ethernet (layer-2) level:
/// userspace reads and writes complete Ethernet frames.
///
/// VPN daemons and other userspace network programs interact with the kernel
/// network stack through these virtual interfaces by calling tun_write() to
/// inject inbound packets and tun_read() to drain outbound packets.
///
/// All code is original and `#![no_std]`.  No heap allocations are used.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of simultaneous TUN/TAP devices.
pub const MAX_TUN_DEVICES: usize = 8;

/// Ring buffer capacity (must be a power of two, ≤ 32 for u32 wrapping mod).
const RING_CAP: usize = 32;

/// Maximum payload size per packet slot.
const PKT_SIZE: usize = 1500;

/// Base fd value assigned to TUN devices (slots 0–7 → fds 3000–3007).
const TUN_FD_BASE: i32 = 3000;

/// Base fd value assigned to TAP devices (slots 0–7 → fds 3008–3015).
const TAP_FD_BASE: i32 = 3008;

// ---------------------------------------------------------------------------
// TunTapType
// ---------------------------------------------------------------------------

/// Whether the interface operates at the IP (TUN) or Ethernet (TAP) layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TunTapType {
    /// IP-layer (layer 3): userspace exchanges raw IPv4/IPv6 packets.
    Tun,
    /// Ethernet-layer (layer 2): userspace exchanges complete Ethernet frames.
    Tap,
}

// ---------------------------------------------------------------------------
// TunTapDevice
// ---------------------------------------------------------------------------

/// A single TUN/TAP virtual interface.
///
/// Stored inside a `static Mutex<[TunTapDevice; 8]>` so all fields must be
/// `Copy` and the type must have a `const fn empty()` constructor.
#[derive(Clone, Copy)]
pub struct TunTapDevice {
    /// Interface name, NUL-padded ASCII (e.g. "tun0\0…").
    pub name: [u8; 16],

    /// Layer type: IP (TUN) or Ethernet (TAP).
    pub dev_type: TunTapType,

    /// True when the slot is in use.
    pub active: bool,

    // ── Outgoing ring buffer (kernel → userspace) ──────────────────────────
    /// Packet data for outgoing queue.
    pub tx_queue: [[u8; PKT_SIZE]; RING_CAP],
    /// Byte-lengths of entries in `tx_queue`.
    pub tx_lens: [u16; RING_CAP],
    /// Producer index (wrapping, mod RING_CAP).
    pub tx_head: u32,
    /// Consumer index (wrapping, mod RING_CAP).
    pub tx_tail: u32,

    // ── Incoming ring buffer (userspace → kernel) ──────────────────────────
    /// Packet data for incoming queue.
    pub rx_queue: [[u8; PKT_SIZE]; RING_CAP],
    /// Byte-lengths of entries in `rx_queue`.
    pub rx_lens: [u16; RING_CAP],
    /// Producer index (wrapping, mod RING_CAP).
    pub rx_head: u32,
    /// Consumer index (wrapping, mod RING_CAP).
    pub rx_tail: u32,

    // ── Interface configuration ────────────────────────────────────────────
    /// Assigned IPv4 address (for TUN; TAP uses MAC).
    pub ip: [u8; 4],
    /// Subnet mask.
    pub mask: [u8; 4],
    /// Ethernet MAC address (used only for TAP).
    pub mac: [u8; 6],
    /// MTU (normally 1500).
    pub mtu: u16,

    // ── File descriptor ────────────────────────────────────────────────────
    /// The fd number that userspace uses when calling tun_read/tun_write.
    pub fd: i32,

    // ── Statistics ────────────────────────────────────────────────────────
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u64,
    pub tx_packets: u64,
}

impl TunTapDevice {
    /// Const constructor that produces an inactive, zeroed-out slot.
    pub const fn empty() -> Self {
        TunTapDevice {
            name: [0u8; 16],
            dev_type: TunTapType::Tun,
            active: false,
            tx_queue: [[0u8; PKT_SIZE]; RING_CAP],
            tx_lens: [0u16; RING_CAP],
            tx_head: 0,
            tx_tail: 0,
            rx_queue: [[0u8; PKT_SIZE]; RING_CAP],
            rx_lens: [0u16; RING_CAP],
            rx_head: 0,
            rx_tail: 0,
            ip: [0u8; 4],
            mask: [0u8; 4],
            mac: [0u8; 6],
            mtu: 1500,
            fd: -1,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_packets: 0,
            tx_packets: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global device table
// ---------------------------------------------------------------------------

pub static TUN_DEVICES: Mutex<[TunTapDevice; MAX_TUN_DEVICES]> =
    Mutex::new([TunTapDevice::empty(); MAX_TUN_DEVICES]);

// ---------------------------------------------------------------------------
// Ring-buffer helpers
// ---------------------------------------------------------------------------

/// Returns true when the ring buffer described by `head`/`tail` is full.
#[inline(always)]
fn ring_full(head: u32, tail: u32) -> bool {
    // Full when there are RING_CAP items in flight.
    head.wrapping_sub(tail) as usize >= RING_CAP
}

/// Returns true when the ring buffer is empty.
#[inline(always)]
fn ring_empty(head: u32, tail: u32) -> bool {
    head == tail
}

/// Returns the slot index for a given ring counter value.
#[inline(always)]
fn ring_slot(counter: u32) -> usize {
    (counter as usize) % RING_CAP
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the TUN/TAP subsystem (clears all device slots).
pub fn init() {
    let mut devs = TUN_DEVICES.lock();
    for i in 0..MAX_TUN_DEVICES {
        // Zero in-place — TunTapDevice is ~96 KB; materialising it on the
        // stack in a debug build overflows the 64 KB kernel stack.
        unsafe {
            core::ptr::write_bytes(&mut devs[i] as *mut TunTapDevice, 0, 1);
        }
    }
    serial_println!(
        "  Net: TUN/TAP subsystem initialized ({} slots)",
        MAX_TUN_DEVICES
    );
}

/// Create a new TUN or TAP device.
///
/// * `name`     — Interface name (up to 15 bytes; NUL-terminated internally).
/// * `dev_type` — `TunTapType::Tun` or `TunTapType::Tap`.
///
/// Returns the file descriptor on success, or `None` when all slots are taken.
pub fn tun_create(name: &[u8], dev_type: TunTapType) -> Option<i32> {
    let mut devs = TUN_DEVICES.lock();

    // Find a free slot.
    let slot = {
        let mut found = None;
        for i in 0..MAX_TUN_DEVICES {
            if !devs[i].active {
                found = Some(i);
                break;
            }
        }
        found?
    };

    let dev = &mut devs[slot];
    unsafe {
        core::ptr::write_bytes(dev as *mut TunTapDevice, 0, 1);
    }
    dev.dev_type = dev_type;
    dev.active = true;

    // Copy name (up to 15 bytes so slot 15 is always NUL).
    let copy_len = name.len().min(15);
    dev.name[..copy_len].copy_from_slice(&name[..copy_len]);

    // Assign fd and default configuration based on type.
    match dev_type {
        TunTapType::Tun => {
            dev.fd = TUN_FD_BASE + slot as i32;
            // Assign 10.0.0.X where X = slot + 2.
            dev.ip = [10, 0, 0, (slot as u8).saturating_add(2)];
            dev.mask = [255, 255, 255, 0];
        }
        TunTapType::Tap => {
            dev.fd = TAP_FD_BASE + slot as i32;
            // Locally-administered MAC: 02:00:00:00:00:XX.
            dev.mac = [0x02, 0x00, 0x00, 0x00, 0x00, slot as u8];
        }
    }

    let fd = dev.fd;
    serial_println!(
        "  TUN/TAP: created {:?} '{}' fd={} slot={}",
        dev_type,
        core::str::from_utf8(&dev.name[..copy_len]).unwrap_or("?"),
        fd,
        slot
    );
    Some(fd)
}

/// Destroy a TUN/TAP device identified by its fd.
///
/// Returns `true` on success, `false` if the fd was not found.
pub fn tun_destroy(fd: i32) -> bool {
    let mut devs = TUN_DEVICES.lock();
    for i in 0..MAX_TUN_DEVICES {
        if devs[i].active && devs[i].fd == fd {
            devs[i] = TunTapDevice::empty();
            serial_println!("  TUN/TAP: destroyed fd={}", fd);
            return true;
        }
    }
    false
}

/// Inject a packet from userspace into the kernel network stack.
///
/// This is the "write" side: a VPN daemon or other userspace program supplies
/// a packet that the kernel should treat as arriving from this interface.
///
/// For TUN devices the data is a raw IP packet; `ip_input()` is called.
/// For TAP devices the data is a complete Ethernet frame; `rx_packet()` is
/// called on the netdev layer.
///
/// Returns the number of bytes consumed, or `-1` if the rx ring is full.
pub fn tun_write(fd: i32, data: &[u8]) -> isize {
    if data.is_empty() || data.len() > PKT_SIZE {
        return -1;
    }

    let mut devs = TUN_DEVICES.lock();
    let slot = match find_slot_by_fd(&devs, fd) {
        Some(s) => s,
        None => return -1,
    };

    let dev = &mut devs[slot];
    if !dev.active {
        return -1;
    }

    // Check rx ring capacity.
    if ring_full(dev.rx_head, dev.rx_tail) {
        return -1;
    }

    let idx = ring_slot(dev.rx_head);
    let copy_len = data.len().min(PKT_SIZE);
    dev.rx_queue[idx][..copy_len].copy_from_slice(&data[..copy_len]);
    dev.rx_lens[idx] = copy_len as u16;
    dev.rx_head = dev.rx_head.wrapping_add(1);

    // Update stats.
    dev.rx_bytes = dev.rx_bytes.saturating_add(copy_len as u64);
    dev.rx_packets = dev.rx_packets.saturating_add(1);

    let dev_type = dev.dev_type;
    drop(devs); // release lock before calling into stack

    // Dispatch into the network stack.
    match dev_type {
        TunTapType::Tun => {
            // Treat as a received IP packet.
            crate::net::process_frame(data);
        }
        TunTapType::Tap => {
            // Treat as a received Ethernet frame.
            crate::net::process_frame(data);
        }
    }

    copy_len as isize
}

/// Read the next outgoing packet that the kernel wants to send out via this
/// TUN/TAP interface.
///
/// Called by userspace (VPN daemon) to drain packets the kernel has routed
/// through this virtual interface.
///
/// Returns the packet length on success, or `-1` if the tx queue is empty.
pub fn tun_read(fd: i32, buf: &mut [u8; PKT_SIZE]) -> isize {
    let mut devs = TUN_DEVICES.lock();
    let slot = match find_slot_by_fd(&devs, fd) {
        Some(s) => s,
        None => return -1,
    };

    let dev = &mut devs[slot];
    if !dev.active {
        return -1;
    }

    if ring_empty(dev.tx_head, dev.tx_tail) {
        return -1; // nothing to deliver
    }

    let idx = ring_slot(dev.tx_tail);
    let len = dev.tx_lens[idx] as usize;
    let copy_len = len.min(PKT_SIZE);

    buf[..copy_len].copy_from_slice(&dev.tx_queue[idx][..copy_len]);
    dev.tx_tail = dev.tx_tail.wrapping_add(1);

    // Update stats.
    dev.tx_bytes = dev.tx_bytes.saturating_add(copy_len as u64);
    dev.tx_packets = dev.tx_packets.saturating_add(1);

    copy_len as isize
}

/// Enqueue an outgoing packet for userspace to read via `tun_read()`.
///
/// Called internally by the routing layer when it determines that the egress
/// interface for a packet is a TUN/TAP device.
///
/// * `slot` — Device index in `TUN_DEVICES`.
/// * `data` — Packet/frame bytes.
/// * `len`  — Number of valid bytes in `data`.
pub fn tun_enqueue_tx(slot: usize, data: &[u8], len: u16) {
    if slot >= MAX_TUN_DEVICES {
        return;
    }
    let copy_len = (len as usize).min(data.len()).min(PKT_SIZE);
    if copy_len == 0 {
        return;
    }

    let mut devs = TUN_DEVICES.lock();
    let dev = &mut devs[slot];
    if !dev.active {
        return;
    }

    if ring_full(dev.tx_head, dev.tx_tail) {
        serial_println!("  TUN/TAP: tx ring full on slot {}, dropping packet", slot);
        return;
    }

    let idx = ring_slot(dev.tx_head);
    dev.tx_queue[idx][..copy_len].copy_from_slice(&data[..copy_len]);
    dev.tx_lens[idx] = copy_len as u16;
    dev.tx_head = dev.tx_head.wrapping_add(1);
}

/// Assign a static IPv4 address and subnet mask to a TUN/TAP device.
///
/// Returns `true` on success, `false` if the fd is not found.
pub fn tun_set_ip(fd: i32, ip: [u8; 4], mask: [u8; 4]) -> bool {
    let mut devs = TUN_DEVICES.lock();
    let slot = match find_slot_by_fd(&devs, fd) {
        Some(s) => s,
        None => return false,
    };
    devs[slot].ip = ip;
    devs[slot].mask = mask;
    true
}

/// Retrieve traffic statistics for a TUN/TAP device.
///
/// Returns `Some((rx_bytes, tx_bytes, rx_packets, tx_packets))` or `None`.
pub fn tun_get_stats(fd: i32) -> Option<(u64, u64, u64, u64)> {
    let devs = TUN_DEVICES.lock();
    let slot = find_slot_by_fd(&devs, fd)?;
    let d = &devs[slot];
    Some((d.rx_bytes, d.tx_bytes, d.rx_packets, d.tx_packets))
}

/// Returns `true` if the given fd belongs to a TUN/TAP device.
pub fn tun_is_fd(fd: i32) -> bool {
    if fd < TUN_FD_BASE {
        return false;
    }
    let devs = TUN_DEVICES.lock();
    find_slot_by_fd(&devs, fd).is_some()
}

/// Poll all active TUN/TAP devices and process any queued rx packets.
///
/// Should be called periodically from the main network poll loop.
pub fn tun_tick() {
    // Collect (slot, dev_type, head, tail) pairs without holding the lock
    // while calling into the network stack.
    let mut pending: [(usize, TunTapType, usize, [u8; PKT_SIZE]); MAX_TUN_DEVICES] =
        [(0, TunTapType::Tun, 0, [0u8; PKT_SIZE]); MAX_TUN_DEVICES];
    let mut count = 0usize;

    {
        let mut devs = TUN_DEVICES.lock();
        for slot in 0..MAX_TUN_DEVICES {
            let dev = &mut devs[slot];
            if !dev.active {
                continue;
            }
            if ring_empty(dev.rx_head, dev.rx_tail) {
                continue;
            }

            let idx = ring_slot(dev.rx_tail);
            let len = dev.rx_lens[idx] as usize;
            let copy_len = len.min(PKT_SIZE);

            let mut pkt = [0u8; PKT_SIZE];
            pkt[..copy_len].copy_from_slice(&dev.rx_queue[idx][..copy_len]);
            dev.rx_tail = dev.rx_tail.wrapping_add(1);

            pending[count] = (slot, dev.dev_type, copy_len, pkt);
            count = count.saturating_add(1);
        }
    }

    // Process collected packets outside the lock.
    for i in 0..count {
        let (_, dev_type, pkt_len, ref pkt_buf) = pending[i];
        if pkt_len == 0 {
            continue;
        }
        match dev_type {
            TunTapType::Tun | TunTapType::Tap => {
                crate::net::process_frame(&pkt_buf[..pkt_len]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the slot index for a given fd.  Lock must already be held by caller.
fn find_slot_by_fd(devs: &[TunTapDevice; MAX_TUN_DEVICES], fd: i32) -> Option<usize> {
    for i in 0..MAX_TUN_DEVICES {
        if devs[i].active && devs[i].fd == fd {
            return Some(i);
        }
    }
    None
}
