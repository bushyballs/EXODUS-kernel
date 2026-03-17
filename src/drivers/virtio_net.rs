use super::virtio::{
    buf_pfn, device_begin_init, device_driver_ok, device_fail, device_set_features,
    pci_find_virtio, setup_queue, VirtQueue, VirtqBuf, VIRTIO_REG_CONFIG, VRING_DESC_F_NEXT,
    VRING_DESC_F_WRITE,
};
/// VirtIO Network Device Driver — no-heap, static-buffer implementation
///
/// Supports QEMU/KVM virtio-net (PCI vendor 0x1AF4, device 0x1000).
/// Uses the VirtIO legacy interface (I/O BAR0, PFN-based queue setup).
///
/// All buffers are static; no Vec, Box, String, or allocator calls.
/// Identity mapping assumed: virtual address == physical address for statics.
///
/// Public API:
///   virtio_net_init()       -> bool       probe + initialise + register netdev
///   virtio_net_send(d, l)   -> bool       transmit a raw Ethernet frame
///   virtio_net_recv_poll()               drain RX/TX used rings
///   virtio_net_get_mac()    -> Option<[u8;6]>
///   virtio_net_get_stats()  -> (u64,u64,u64,u64)
///   virtio_net_is_up()      -> bool
///   virtio_net_tick()       -> ()         called from timer interrupt
///   init()                              called by drivers::init()
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add/saturating_sub for counters
///   - wrapping_add for ring indices
///   - read_volatile/write_volatile for all shared-ring accesses
///   - No panic — use serial_println! + return false on fatal errors
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// PCI IDs
// ============================================================================

pub const VIRTIO_NET_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_NET_DEVICE: u16 = 0x1000;

// ============================================================================
// Feature bits
// ============================================================================

pub const VIRTIO_NET_F_CSUM: u64 = 1 << 0;
pub const VIRTIO_NET_F_GUEST_CSUM: u64 = 1 << 1;
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;
pub const VIRTIO_NET_F_GSO: u64 = 1 << 6;
pub const VIRTIO_NET_F_GUEST_TSO4: u64 = 1 << 7;
pub const VIRTIO_NET_F_GUEST_TSO6: u64 = 1 << 8;
pub const VIRTIO_NET_F_GUEST_ECN: u64 = 1 << 9;
pub const VIRTIO_NET_F_GUEST_UFO: u64 = 1 << 10;
pub const VIRTIO_NET_F_HOST_TSO4: u64 = 1 << 11;
pub const VIRTIO_NET_F_HOST_TSO6: u64 = 1 << 12;
pub const VIRTIO_NET_F_HOST_ECN: u64 = 1 << 13;
pub const VIRTIO_NET_F_HOST_UFO: u64 = 1 << 14;
pub const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;
pub const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
pub const VIRTIO_NET_F_CTRL_VQ: u64 = 1 << 17;
pub const VIRTIO_NET_F_CTRL_RX: u64 = 1 << 18;
pub const VIRTIO_NET_F_CTRL_VLAN: u64 = 1 << 19;
pub const VIRTIO_NET_F_CTRL_RX_EXTRA: u64 = 1 << 20;

// ============================================================================
// VirtIO-net packet header flags / GSO types
// ============================================================================

pub const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_NONE: u8 = 0;
pub const VIRTIO_NET_HDR_GSO_TCPV4: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_UDP: u8 = 3;
pub const VIRTIO_NET_HDR_GSO_TCPV6: u8 = 4;
pub const VIRTIO_NET_HDR_GSO_ECN: u8 = 0x80;

// ============================================================================
// VirtIO-net packet header (prepended to every TX and RX packet)
// ============================================================================

/// VirtIO-net header preceding each packet in the descriptor chain.
///
/// For our use (no GSO, no checksum offload) all fields are zero.
/// Repr(C, packed) matches the wire format exactly.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct VirtioNetHdr {
    pub flags: u8,        // VIRTIO_NET_HDR_F_*
    pub gso_type: u8,     // VIRTIO_NET_HDR_GSO_*
    pub hdr_len: u16,     // ethernet+IP+transport header length
    pub gso_size: u16,    // GSO segment size
    pub csum_start: u16,  // offset to start of checksum
    pub csum_offset: u16, // offset from csum_start to checksum field
}

impl VirtioNetHdr {
    pub const fn zeroed() -> Self {
        VirtioNetHdr {
            flags: 0,
            gso_type: VIRTIO_NET_HDR_GSO_NONE,
            hdr_len: 0,
            gso_size: 0,
            csum_start: 0,
            csum_offset: 0,
        }
    }
}

// ============================================================================
// Buffer constants
// ============================================================================

/// Number of RX and TX slots (must be < QUEUE_SIZE = 256).
pub const NET_RING_SIZE: usize = 32;

/// Maximum payload per slot (standard Ethernet MTU).
pub const NET_PKT_SIZE: usize = 1514;

// ============================================================================
// Static packet buffer pool — page-aligned, zero-initialised
// ============================================================================

/// Packet buffer pool: 32 RX + 32 TX slots of 1514 bytes each (~93 KiB).
///
/// Placed in its own static so it is page-aligned (align(4096)).  The RX and
/// TX halves are accessed exclusively under the VNET Mutex.
#[repr(C, align(4096))]
pub struct VirtioNetBufs {
    pub rx_hdrs: [VirtioNetHdr; NET_RING_SIZE],
    pub rx_pkts: [[u8; NET_PKT_SIZE]; NET_RING_SIZE],
    pub tx_hdrs: [VirtioNetHdr; NET_RING_SIZE],
    pub tx_pkts: [[u8; NET_PKT_SIZE]; NET_RING_SIZE],
}

impl VirtioNetBufs {
    pub const fn zeroed() -> Self {
        VirtioNetBufs {
            rx_hdrs: [VirtioNetHdr::zeroed(); NET_RING_SIZE],
            rx_pkts: [[0u8; NET_PKT_SIZE]; NET_RING_SIZE],
            tx_hdrs: [VirtioNetHdr::zeroed(); NET_RING_SIZE],
            tx_pkts: [[0u8; NET_PKT_SIZE]; NET_RING_SIZE],
        }
    }
}

// Safety: VirtioNetBufs is only accessed under VNET Mutex.
unsafe impl Send for VirtioNetBufs {}
unsafe impl Sync for VirtioNetBufs {}

// ============================================================================
// Device state
// ============================================================================

/// Runtime state for the VirtIO network device.
pub struct VirtioNetDev {
    pub present: bool,
    pub mac: [u8; 6],
    pub mtu: u16,
    pub features: u64,
    pub io_base: u16,
    pub rx_vq: VirtQueue, // virtqueue 0 — receive
    pub tx_vq: VirtQueue, // virtqueue 1 — transmit
    /// Pointer to the start of VNET_BUFS (used for physical address arithmetic)
    pub bufs_ptr: u32,
    /// Next TX slot to use (round-robin 0..NET_RING_SIZE)
    pub tx_next: u8,
    /// Driver-side index tracking how many RX slots we have pre-filled
    pub rx_pending: u8,
    // Statistics — all updated with saturating_add
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
}

// Safety: VirtioNetDev is only accessed under VNET Mutex.
unsafe impl Send for VirtioNetDev {}
unsafe impl Sync for VirtioNetDev {}

// ============================================================================
// Static allocations
// ============================================================================

static mut VNET_BUFS: VirtioNetBufs = VirtioNetBufs::zeroed();

static mut RX_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();
static mut TX_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();

/// Global VirtIO-net device state.
static VNET: Mutex<Option<VirtioNetDev>> = Mutex::new(None);

// ============================================================================
// Address helpers — identity-mapped, so virt == phys
// ============================================================================

/// Physical address of rx_hdrs[slot]
#[inline]
fn rx_hdr_phys(slot: usize) -> u64 {
    let slot = slot.min(NET_RING_SIZE - 1);
    unsafe { &VNET_BUFS.rx_hdrs[slot] as *const VirtioNetHdr as u64 }
}

/// Physical address of rx_pkts[slot]
#[inline]
fn rx_pkt_phys(slot: usize) -> u64 {
    let slot = slot.min(NET_RING_SIZE - 1);
    unsafe { &VNET_BUFS.rx_pkts[slot] as *const [u8; NET_PKT_SIZE] as u64 }
}

/// Physical address of tx_hdrs[slot]
#[inline]
fn tx_hdr_phys(slot: usize) -> u64 {
    let slot = slot.min(NET_RING_SIZE - 1);
    unsafe { &VNET_BUFS.tx_hdrs[slot] as *const VirtioNetHdr as u64 }
}

/// Physical address of tx_pkts[slot]
#[inline]
fn tx_pkt_phys(slot: usize) -> u64 {
    let slot = slot.min(NET_RING_SIZE - 1);
    unsafe { &VNET_BUFS.tx_pkts[slot] as *const [u8; NET_PKT_SIZE] as u64 }
}

// ============================================================================
// Pre-populate RX virtqueue
// ============================================================================

/// Add all NET_RING_SIZE RX buffers to the RX available ring.
///
/// Each RX entry is a 2-descriptor chain:
///   [hdr: WRITE 10 bytes][pkt: WRITE 1514 bytes]
///
/// The device fills both descriptors when a packet arrives.
fn populate_rx_queue(dev: &mut VirtioNetDev) {
    for slot in 0..NET_RING_SIZE {
        let hdr_idx = match dev.rx_vq.alloc_desc() {
            Some(i) => i,
            None => {
                serial_println!("  virtio-net: RX queue full at slot {}", slot);
                break;
            }
        };
        let pkt_idx = match dev.rx_vq.alloc_desc() {
            Some(i) => i,
            None => {
                dev.rx_vq.free_chain(hdr_idx);
                serial_println!("  virtio-net: RX queue full (pkt) at slot {}", slot);
                break;
            }
        };

        // Header descriptor — device-writable, chained to packet descriptor
        {
            let d = dev.rx_vq.desc_mut(hdr_idx);
            d.addr = rx_hdr_phys(slot);
            d.len = core::mem::size_of::<VirtioNetHdr>() as u32;
            d.flags = VRING_DESC_F_WRITE | VRING_DESC_F_NEXT;
            d.next = pkt_idx;
        }

        // Packet descriptor — device-writable, terminates chain
        {
            let d = dev.rx_vq.desc_mut(pkt_idx);
            d.addr = rx_pkt_phys(slot);
            d.len = NET_PKT_SIZE as u32;
            d.flags = VRING_DESC_F_WRITE;
            d.next = 0;
        }

        // Submit chain (writes to avail ring + kicks device via VirtQueue::submit)
        dev.rx_vq.submit(hdr_idx);
        dev.rx_pending = dev.rx_pending.saturating_add(1);
    }
}

/// Re-add one RX slot back to the available ring after processing.
///
/// Reuses the same descriptor indices (freed + reallocated) so the static
/// buffer regions remain valid.
fn replenish_rx_slot(dev: &mut VirtioNetDev, slot: usize) {
    let slot = slot.min(NET_RING_SIZE - 1);

    // Zero the header so stale data from the previous packet is cleared
    unsafe {
        core::ptr::write_volatile(
            &mut VNET_BUFS.rx_hdrs[slot] as *mut VirtioNetHdr,
            VirtioNetHdr::zeroed(),
        );
    }

    let hdr_idx = match dev.rx_vq.alloc_desc() {
        Some(i) => i,
        None => return, // queue temporarily full — drop replenish
    };
    let pkt_idx = match dev.rx_vq.alloc_desc() {
        Some(i) => i,
        None => {
            dev.rx_vq.free_chain(hdr_idx);
            return;
        }
    };

    {
        let d = dev.rx_vq.desc_mut(hdr_idx);
        d.addr = rx_hdr_phys(slot);
        d.len = core::mem::size_of::<VirtioNetHdr>() as u32;
        d.flags = VRING_DESC_F_WRITE | VRING_DESC_F_NEXT;
        d.next = pkt_idx;
    }
    {
        let d = dev.rx_vq.desc_mut(pkt_idx);
        d.addr = rx_pkt_phys(slot);
        d.len = NET_PKT_SIZE as u32;
        d.flags = VRING_DESC_F_WRITE;
        d.next = 0;
    }

    // Submit chain (writes to avail ring + kicks device via VirtQueue::submit)
    dev.rx_vq.submit(hdr_idx);
}

// ============================================================================
// Probe and initialise
// ============================================================================

/// Probe PCI bus for a VirtIO net device and initialise it.
///
/// Steps:
///   1. PCI scan for vendor=0x1AF4, device=0x1000
///   2. VirtIO legacy handshake: RESET → ACK → DRIVER → feature negotiation
///   3. Read MAC from device config space (legacy: io_base + 0x14..0x1A via inb)
///   4. Setup RX virtqueue (index 0) and TX virtqueue (index 1)
///   5. Pre-populate 32 RX buffers
///   6. DRIVER_OK
///   7. Register as "eth1" with netdev layer
///
/// Returns `true` on success.
pub fn virtio_net_init() -> bool {
    // --- PCI scan ---
    let (io_base, _bus, _dev, _func) = match pci_find_virtio(VIRTIO_NET_VENDOR, VIRTIO_NET_DEVICE) {
        Some(v) => v,
        None => {
            serial_println!("  virtio-net: no device found");
            return false;
        }
    };

    // --- VirtIO handshake: RESET → ACK → DRIVER → read device features ---
    let dev_features_lo = device_begin_init(io_base) as u64;
    // Legacy devices only expose 32 feature bits via the single DEVFEATURES register.
    // Upper 32 bits are always zero on legacy.
    let dev_features: u64 = dev_features_lo;

    // Negotiate: request F_MAC and F_STATUS (bit 5 and 16).
    // F_MAC must be set for us to read the MAC from config; F_STATUS gives
    // us the link-status field.  We deliberately avoid GSO/checksum offload
    // features to keep the driver simple.
    let mut drv_features: u64 = 0;
    if dev_features & VIRTIO_NET_F_MAC != 0 {
        drv_features |= VIRTIO_NET_F_MAC;
    }
    if dev_features & VIRTIO_NET_F_STATUS != 0 {
        drv_features |= VIRTIO_NET_F_STATUS;
    }

    // Legacy VirtIO uses a 32-bit features register — only low 32 bits sent.
    let drv_features_lo = (drv_features & 0xFFFF_FFFF) as u32;
    if !device_set_features(io_base, drv_features_lo) {
        serial_println!("  virtio-net: FEATURES_OK not acknowledged — aborting");
        device_fail(io_base);
        return false;
    }

    // --- Read MAC address from device config space ---
    // Legacy: io_base + VIRTIO_REG_CONFIG (0x14) holds byte 0 of the config.
    // The MAC is in bytes 0..5 of the net-device config, i.e. at offsets
    // io_base+0x14 through io_base+0x19.
    let mut mac = [0u8; 6];
    if drv_features & VIRTIO_NET_F_MAC != 0 {
        for i in 0..6u16 {
            mac[i as usize] = crate::io::inb(io_base + VIRTIO_REG_CONFIG + i);
        }
        serial_println!(
            "  virtio-net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5]
        );
    } else {
        // No MAC in config — generate a locally-administered address.
        mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        serial_println!("  virtio-net: F_MAC not available, using default MAC");
    }

    // --- Setup RX virtqueue (index 0) ---
    let rx_pfn = unsafe { buf_pfn(&RX_VQ_BUF) };
    let rx_qsz = match setup_queue(io_base, 0, rx_pfn) {
        Some(s) => s,
        None => {
            serial_println!("  virtio-net: RX virtqueue size=0 — aborting");
            device_fail(io_base);
            return false;
        }
    };
    serial_println!("  virtio-net: RX virtqueue size={}", rx_qsz);

    // --- Setup TX virtqueue (index 1) ---
    let tx_pfn = unsafe { buf_pfn(&TX_VQ_BUF) };
    let tx_qsz = match setup_queue(io_base, 1, tx_pfn) {
        Some(s) => s,
        None => {
            serial_println!("  virtio-net: TX virtqueue size=0 — aborting");
            device_fail(io_base);
            return false;
        }
    };
    serial_println!("  virtio-net: TX virtqueue size={}", tx_qsz);

    // Build VirtQueue state objects pointing into the static VirtqBuf statics.
    let rx_vq = unsafe { VirtQueue::new(&mut RX_VQ_BUF, io_base, 0) };
    let tx_vq = unsafe { VirtQueue::new(&mut TX_VQ_BUF, io_base, 1) };

    // --- DRIVER_OK ---
    device_driver_ok(io_base);

    // Store device state and pre-populate RX queue.
    let mut dev = VirtioNetDev {
        present: true,
        mac,
        mtu: 1500,
        features: drv_features,
        io_base,
        rx_vq,
        tx_vq,
        bufs_ptr: unsafe { &VNET_BUFS as *const VirtioNetBufs as u32 },
        tx_next: 0,
        rx_pending: 0,
        rx_packets: 0,
        tx_packets: 0,
        rx_bytes: 0,
        tx_bytes: 0,
        rx_errors: 0,
        tx_errors: 0,
    };

    populate_rx_queue(&mut dev);

    // Register with driver subsystem
    super::register("virtio-net", super::DeviceType::Network);

    // Register with the netdev layer as "eth1"
    {
        let mut net_dev = crate::net::netdev::NetDevice::zeroed();
        net_dev.set_name("eth1");
        net_dev.mac = mac;
        net_dev.driver_idx = crate::net::netdev::DRIVER_VIRTIO_NET;
        let _ = crate::net::netdev::register_device(net_dev);
    }

    *VNET.lock() = Some(dev);

    serial_println!("  virtio-net: init OK  mtu=1500");
    true
}

// ============================================================================
// Transmit
// ============================================================================

/// Transmit a raw Ethernet frame.
///
/// Builds a 2-descriptor TX chain:
///   [hdr: READ 10 bytes][pkt: READ len bytes]
///
/// Kicks the TX virtqueue and spin-polls the used ring for completion.
/// Returns `true` on success, `false` on error or timeout.
pub fn virtio_net_send(data: &[u8], len: usize) -> bool {
    if len == 0 || len > NET_PKT_SIZE {
        return false;
    }

    let mut guard = VNET.lock();
    let dev = match guard.as_mut() {
        Some(d) if d.present => d,
        _ => {
            return false;
        }
    };

    // Select TX slot (round-robin)
    let slot = dev.tx_next as usize;
    dev.tx_next = ((dev.tx_next as usize).wrapping_add(1) % NET_RING_SIZE) as u8;

    // Zero the TX header (no GSO, no checksum offload)
    unsafe {
        core::ptr::write_volatile(
            &mut VNET_BUFS.tx_hdrs[slot] as *mut VirtioNetHdr,
            VirtioNetHdr::zeroed(),
        );
    }

    // Copy packet data into the static TX buffer
    let copy_len = len.min(NET_PKT_SIZE);
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            VNET_BUFS.tx_pkts[slot].as_mut_ptr(),
            copy_len,
        );
    }

    // Allocate 2 TX descriptors
    let hdr_idx = match dev.tx_vq.alloc_desc() {
        Some(i) => i,
        None => {
            serial_println!("  virtio-net: TX queue full (hdr)");
            dev.tx_errors = dev.tx_errors.saturating_add(1);
            return false;
        }
    };
    let pkt_idx = match dev.tx_vq.alloc_desc() {
        Some(i) => i,
        None => {
            dev.tx_vq.free_chain(hdr_idx);
            serial_println!("  virtio-net: TX queue full (pkt)");
            dev.tx_errors = dev.tx_errors.saturating_add(1);
            return false;
        }
    };

    // Header descriptor — driver read-only, chained to packet
    {
        let d = dev.tx_vq.desc_mut(hdr_idx);
        d.addr = tx_hdr_phys(slot);
        d.len = core::mem::size_of::<VirtioNetHdr>() as u32;
        d.flags = VRING_DESC_F_NEXT; // device reads this (no WRITE flag)
        d.next = pkt_idx;
    }

    // Packet descriptor — driver read-only, terminates chain
    {
        let d = dev.tx_vq.desc_mut(pkt_idx);
        d.addr = tx_pkt_phys(slot);
        d.len = copy_len as u32;
        d.flags = 0; // device reads this (no WRITE flag)
        d.next = 0;
    }

    // Submit TX chain — this writes to the avail ring and kicks the device
    dev.tx_vq.submit(hdr_idx);

    // Spin-poll used ring for completion (~100K iterations)
    let mut completed = false;
    for _ in 0..100_000u32 {
        if let Some((id, _written)) = dev.tx_vq.poll() {
            dev.tx_vq.free_chain(id);
            completed = true;
            break;
        }
        core::hint::spin_loop();
    }

    if completed {
        dev.tx_packets = dev.tx_packets.saturating_add(1);
        dev.tx_bytes = dev.tx_bytes.saturating_add(copy_len as u64);
        true
    } else {
        serial_println!("  virtio-net: TX timeout slot={}", slot);
        // Attempt to free the descriptors even though device hasn't returned them.
        // This may leave the queue in an inconsistent state, but avoids leaking
        // all descriptors on a misbehaving device.
        dev.tx_vq.free_chain(hdr_idx);
        dev.tx_errors = dev.tx_errors.saturating_add(1);
        false
    }
}

// ============================================================================
// Receive poll
// ============================================================================

/// Poll the RX and TX used rings and process any completed work.
///
/// TX: reclaim completed TX descriptors (frees slots for new sends).
/// RX: for each received packet — copy frame to a stack buffer, call
///     `crate::net::process_frame`, then replenish the RX slot.
///
/// Must be called regularly (e.g., from the timer IRQ via `virtio_net_tick`).
pub fn virtio_net_recv_poll() {
    // Process up to NET_RING_SIZE packets per call to bound time in the ISR.
    for _ in 0..NET_RING_SIZE {
        // Acquire lock, attempt to dequeue one RX completion.
        // We release the lock before calling process_frame() to avoid holding
        // a spinlock while running network stack code (which may try to transmit).
        let frame_result: Option<([u8; NET_PKT_SIZE], usize)> = {
            let mut guard = VNET.lock();
            let dev = match guard.as_mut() {
                Some(d) if d.present => d,
                _ => return,
            };

            // Drain TX used ring (reclaim completed sends) while we have the lock.
            while let Some((id, _)) = dev.tx_vq.poll() {
                dev.tx_vq.free_chain(id);
            }

            // Try to pull one RX completion.
            let (chain_head, written_len) = match dev.rx_vq.poll() {
                Some(v) => v,
                None => {
                    // No RX completions available; nothing more to do this tick.
                    return;
                }
            };

            // Return the descriptor chain to the free list so replenish can reuse
            // the descriptor indices.
            dev.rx_vq.free_chain(chain_head);

            // `written_len` includes the VirtioNetHdr (10 bytes) + payload.
            // Subtract the header to get the actual Ethernet frame length.
            let hdr_size = core::mem::size_of::<VirtioNetHdr>() as u32;
            if written_len <= hdr_size {
                // Zero-length or header-only — count as error and replenish.
                dev.rx_errors = dev.rx_errors.saturating_add(1);
                replenish_rx_slot(dev, 0);
                // Continue outer loop; there may be more completions queued.
                None
            } else {
                let payload_len = ((written_len - hdr_size) as usize).min(NET_PKT_SIZE);

                // Determine which RX slot this corresponds to by looking up
                // the physical address stored in the hdr descriptor.
                let slot = {
                    let hdr_addr = {
                        let d = dev
                            .rx_vq
                            .desc_mut(chain_head.min((super::virtio::QUEUE_SIZE - 1) as u16));
                        d.addr
                    };
                    let mut found = 0usize;
                    for s in 0..NET_RING_SIZE {
                        if rx_hdr_phys(s) == hdr_addr {
                            found = s;
                            break;
                        }
                    }
                    found
                };

                // Copy received frame data into a stack buffer (no heap).
                let mut frame_buf = [0u8; NET_PKT_SIZE];
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        VNET_BUFS.rx_pkts[slot].as_ptr(),
                        frame_buf.as_mut_ptr(),
                        payload_len,
                    );
                }

                // Update statistics.
                dev.rx_packets = dev.rx_packets.saturating_add(1);
                dev.rx_bytes = dev.rx_bytes.saturating_add(payload_len as u64);

                // Replenish before releasing lock so the device can reuse the
                // slot immediately, minimising head-of-line stalls.
                replenish_rx_slot(dev, slot);

                Some((frame_buf, payload_len))
                // guard is dropped here — lock released before process_frame call
            }
        };

        // Process the frame outside the lock.
        if let Some((frame_buf, payload_len)) = frame_result {
            crate::net::process_frame(&frame_buf[..payload_len]);
        }
        // If frame_result is None (error case), we already continued the loop
        // implicitly by not returning — go around for the next completion.
    }
}

// ============================================================================
// Query functions
// ============================================================================

/// Return the hardware MAC address, or None if the device is not present.
pub fn virtio_net_get_mac() -> Option<[u8; 6]> {
    VNET.lock().as_ref().map(|d| d.mac)
}

/// Return (rx_packets, tx_packets, rx_bytes, tx_bytes) statistics.
pub fn virtio_net_get_stats() -> (u64, u64, u64, u64) {
    let guard = VNET.lock();
    match guard.as_ref() {
        Some(d) => (d.rx_packets, d.tx_packets, d.rx_bytes, d.tx_bytes),
        None => (0, 0, 0, 0),
    }
}

/// Return `true` if the virtio-net device has been successfully initialised.
pub fn virtio_net_is_up() -> bool {
    VNET.lock().as_ref().map(|d| d.present).unwrap_or(false)
}

// ============================================================================
// Periodic tick — called from timer interrupt
// ============================================================================

/// Periodic network poll hook.  Called from the timer interrupt handler.
///
/// Polls for received packets and reclaims completed TX descriptors.
/// The call is cheap when there is nothing to do (one volatile read per queue).
pub fn virtio_net_tick() {
    virtio_net_recv_poll();
}

// ============================================================================
// NetDriver adapter — called from netdev dispatch table
// ============================================================================

/// Transmit adapter used by `crate::net::netdev::driver_send`.
pub fn netdev_send(buf: &[u8]) -> bool {
    virtio_net_send(buf, buf.len())
}

/// Receive adapter used by `crate::net::netdev::driver_recv`.
///
/// Fills `buf` with the next available received frame.
/// Returns the number of bytes written (0 = no frame available).
pub fn netdev_recv(buf: &mut [u8]) -> usize {
    // Pull one frame from the RX used ring without full processing.
    // This path is used by the netdev layer's direct recv_packet() API.
    let mut guard = VNET.lock();
    let dev = match guard.as_mut() {
        Some(d) if d.present => d,
        _ => return 0,
    };

    let (chain_head, written_len) = match dev.rx_vq.poll() {
        Some(v) => v,
        None => return 0,
    };

    dev.rx_vq.free_chain(chain_head);

    let hdr_size = core::mem::size_of::<VirtioNetHdr>() as u32;
    if written_len <= hdr_size {
        dev.rx_errors = dev.rx_errors.saturating_add(1);
        replenish_rx_slot(dev, 0);
        return 0;
    }

    let payload_len = ((written_len - hdr_size) as usize)
        .min(NET_PKT_SIZE)
        .min(buf.len());

    // Determine slot
    let slot = {
        let hdr_addr = {
            let d = dev
                .rx_vq
                .desc_mut(chain_head.min((super::virtio::QUEUE_SIZE - 1) as u16));
            d.addr
        };
        let mut found_slot = 0usize;
        for s in 0..NET_RING_SIZE {
            if rx_hdr_phys(s) == hdr_addr {
                found_slot = s;
                break;
            }
        }
        found_slot
    };

    unsafe {
        core::ptr::copy_nonoverlapping(
            VNET_BUFS.rx_pkts[slot].as_ptr(),
            buf.as_mut_ptr(),
            payload_len,
        );
    }

    dev.rx_packets = dev.rx_packets.saturating_add(1);
    dev.rx_bytes = dev.rx_bytes.saturating_add(payload_len as u64);

    replenish_rx_slot(dev, slot);

    payload_len
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Probe and initialise the VirtIO network device.
/// Logs result to serial port.  Called once during kernel boot.
pub fn init() {
    if virtio_net_init() {
        let (rx_p, tx_p, _, _) = virtio_net_get_stats();
        serial_println!(
            "  virtio-net: registered eth1  rx_pkts={}  tx_pkts={}",
            rx_p,
            tx_p,
        );
    } else {
        serial_println!("  virtio-net: no device (or init failed)");
    }
}
