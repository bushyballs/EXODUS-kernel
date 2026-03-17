/// VirtIO transport layer for Genesis — no-heap, static-buffer implementation
///
/// Implements the VirtIO legacy PCI interface (spec 1.0, transitional devices).
/// All memory is static; no heap allocation, no Vec, no Box.
///
/// VirtIO devices are discovered via PCI scan: vendor=0x1AF4.
///   0x1000 — virtio-net
///   0x1001 — virtio-blk
///   0x1005 — virtio-rng
///   0x105A — virtio-fs
///
/// Virtqueue memory layout (each queue): 3 contiguous 4096-byte pages
///   Page 0: descriptor table  (QUEUE_SIZE * 16 bytes)
///   Page 1: available ring    (4 + QUEUE_SIZE*2 + 2 bytes, padded to 4096)
///   Page 2: used ring         (4 + QUEUE_SIZE*8 + 2 bytes, padded to 4096)
///
/// Identity mapping assumed (virt addr == phys addr for kernel statics).
///
/// SAFETY RULES (kernel-wide):
///   - No float casts (as f32 / as f64)
///   - saturating_add/saturating_sub for counters
///   - wrapping_add for ring indices
///   - read_volatile/write_volatile for all shared rings
///   - No panic — early returns on error
use crate::serial_println;
use core::sync::atomic::{fence, Ordering};

// ============================================================================
// PCI IDs
// ============================================================================

pub const VIRTIO_PCI_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_PCI_DEV_NET: u16 = 0x1000;
pub const VIRTIO_PCI_DEV_BLK: u16 = 0x1001;
pub const VIRTIO_PCI_DEV_RNG: u16 = 0x1005;
pub const VIRTIO_PCI_DEV_FS: u16 = 0x105A;

// ============================================================================
// VirtIO PCI legacy register offsets (from I/O BAR0)
// ============================================================================

/// Device features offered by host (R, 4 bytes)
pub const VIRTIO_REG_DEV_FEATURES: u16 = 0x00;
/// Driver features accepted by guest (W, 4 bytes)
pub const VIRTIO_REG_DRV_FEATURES: u16 = 0x04;
/// Queue page frame number (W, 4 bytes) — physical addr >> 12
pub const VIRTIO_REG_QUEUE_ADDR: u16 = 0x08;
/// Queue size in descriptors (R, 2 bytes)
pub const VIRTIO_REG_QUEUE_SIZE: u16 = 0x0C;
/// Queue selector (W, 2 bytes)
pub const VIRTIO_REG_QUEUE_SEL: u16 = 0x0E;
/// Queue notify — write queue index to kick device (W, 2 bytes)
pub const VIRTIO_REG_QUEUE_NOTIFY: u16 = 0x10;
/// Device status byte (RW, 1 byte)
pub const VIRTIO_REG_DEV_STATUS: u16 = 0x12;
/// ISR status — read clears interrupt (R, 1 byte)
pub const VIRTIO_REG_ISR_STATUS: u16 = 0x13;
/// Device-specific config space starts here
pub const VIRTIO_REG_CONFIG: u16 = 0x14;

// ============================================================================
// Device status bits
// ============================================================================

pub const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
pub const VIRTIO_STATUS_FAILED: u8 = 128;

// ============================================================================
// Virtqueue descriptor flags
// ============================================================================

/// Descriptor continues in `.next` field
pub const VRING_DESC_F_NEXT: u16 = 1;
/// Buffer is device-writable (driver-readable)
pub const VRING_DESC_F_WRITE: u16 = 2;
/// Buffer contains a list of indirect descriptors
pub const VRING_DESC_F_INDIRECT: u16 = 4;

// ============================================================================
// Virtqueue structures
// ============================================================================

/// Fixed queue depth. Must be <= what the device reports via QUEUE_SIZE.
/// 256 is the standard QEMU default.
pub const QUEUE_SIZE: usize = 256;

/// Virtqueue descriptor (16 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqDesc {
    /// Physical address of the buffer
    pub addr: u64,
    /// Length of the buffer in bytes
    pub len: u32,
    /// VRING_DESC_F_* flags
    pub flags: u16,
    /// Next descriptor index (valid only when VRING_DESC_F_NEXT is set)
    pub next: u16,
}

impl VirtqDesc {
    pub const fn zeroed() -> Self {
        VirtqDesc {
            addr: 0,
            len: 0,
            flags: 0,
            next: 0,
        }
    }
}

/// Available ring (driver -> device).
/// Driver places descriptor-chain heads here.
#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; QUEUE_SIZE],
    pub used_event: u16,
}

impl VirtqAvail {
    pub const fn zeroed() -> Self {
        VirtqAvail {
            flags: 0,
            idx: 0,
            ring: [0u16; QUEUE_SIZE],
            used_event: 0,
        }
    }
}

/// One element of the used ring.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqUsedElem {
    /// Descriptor chain head index returned by the device
    pub id: u32,
    /// Total bytes written by the device into write-only descriptors
    pub len: u32,
}

impl VirtqUsedElem {
    pub const fn zeroed() -> Self {
        VirtqUsedElem { id: 0, len: 0 }
    }
}

/// Used ring (device -> driver).
/// Device places completed chain heads here.
#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; QUEUE_SIZE],
    pub avail_event: u16,
}

impl VirtqUsed {
    pub const fn zeroed() -> Self {
        VirtqUsed {
            flags: 0,
            idx: 0,
            ring: [VirtqUsedElem::zeroed(); QUEUE_SIZE],
            avail_event: 0,
        }
    }
}

/// Aligned backing store for one VirtQueue.
///
/// Layout: [desc table: 4096][avail ring: 4096][used ring: 4096]
/// Each section is a full page so that the legacy PFN register (which
/// points to the start of the descriptor table) works correctly and
/// the used ring starts on a page boundary as required by the spec.
///
/// Total: 12 KiB per queue, statically allocated.
#[repr(C, align(4096))]
pub struct VirtqBuf {
    pub descs: [VirtqDesc; QUEUE_SIZE],
    /// Pad descs up to a full 4096-byte page.
    /// VirtqDesc is 16 bytes; QUEUE_SIZE=256 → 256*16 = 4096 bytes exactly.
    pub avail: VirtqAvail,
    /// VirtqAvail: 4 + 256*2 + 2 = 518 bytes → pad to 4096
    _avail_pad: [u8; 4096 - core::mem::size_of::<VirtqAvail>()],
    pub used: VirtqUsed,
    /// VirtqUsed: 4 + 256*8 + 2 = 2054 bytes → pad to 4096
    _used_pad: [u8; 4096 - core::mem::size_of::<VirtqUsed>()],
}

impl VirtqBuf {
    pub const fn zeroed() -> Self {
        VirtqBuf {
            descs: [VirtqDesc::zeroed(); QUEUE_SIZE],
            avail: VirtqAvail::zeroed(),
            _avail_pad: [0u8; 4096 - core::mem::size_of::<VirtqAvail>()],
            used: VirtqUsed::zeroed(),
            _used_pad: [0u8; 4096 - core::mem::size_of::<VirtqUsed>()],
        }
    }
}

// Safety: VirtqBuf contains no interior mutability beyond raw pointers
// managed by VirtQueue; it is placed in a static and accessed under a Mutex.
unsafe impl Send for VirtqBuf {}
unsafe impl Sync for VirtqBuf {}

/// Runtime state for one VirtQueue.
///
/// Holds raw pointers into the corresponding `VirtqBuf` static, plus the
/// driver-side bookkeeping (free list, last-seen used index).
/// Does NOT own any memory — the caller provides the backing `VirtqBuf`.
pub struct VirtQueue {
    /// I/O BAR0 base of the owning device
    pub io_base: u16,
    /// Which virtqueue index (0 = requestq for blk/rng, 0/1 for net)
    pub queue_idx: u16,
    /// Pointer to the descriptor table (inside VirtqBuf.descs)
    desc: *mut VirtqDesc,
    /// Pointer to the available ring (inside VirtqBuf.avail)
    avail: *mut VirtqAvail,
    /// Pointer to the used ring (inside VirtqBuf.used)
    used: *const VirtqUsed,
    /// Driver's last-seen used.idx (tracks how many completions we've consumed)
    pub last_used: u16,
    /// Head of the free descriptor list
    pub free_head: u16,
    /// Number of free descriptors remaining
    pub num_free: u16,
}

// Safety: VirtQueue is only ever accessed under a Mutex.
unsafe impl Send for VirtQueue {}
unsafe impl Sync for VirtQueue {}

impl VirtQueue {
    /// Construct a VirtQueue pointing into `buf`.
    ///
    /// `buf` must be a statically-allocated, page-aligned `VirtqBuf`.
    /// Caller is responsible for writing the PFN to the device before use.
    pub fn new(buf: &mut VirtqBuf, io_base: u16, queue_idx: u16) -> Self {
        // Zero-initialise descriptor free list: desc[i].next = i+1
        for i in 0..(QUEUE_SIZE as u16).saturating_sub(1) {
            buf.descs[i as usize].next = i.wrapping_add(1);
            buf.descs[i as usize].flags = 0;
            buf.descs[i as usize].addr = 0;
            buf.descs[i as usize].len = 0;
        }
        // Last descriptor points nowhere (no NEXT flag set, next=0 unused)
        buf.descs[QUEUE_SIZE - 1].next = 0;
        buf.descs[QUEUE_SIZE - 1].flags = 0;
        // Clear rings
        buf.avail.idx = 0;
        buf.avail.flags = 0;
        buf.used = VirtqUsed::zeroed();

        VirtQueue {
            io_base,
            queue_idx,
            desc: buf.descs.as_mut_ptr(),
            avail: &mut buf.avail,
            used: &buf.used,
            last_used: 0,
            free_head: 0,
            num_free: QUEUE_SIZE as u16,
        }
    }

    /// Allocate one descriptor from the free list.
    /// Returns `None` if the queue is full.
    pub fn alloc_desc(&mut self) -> Option<u16> {
        if self.num_free == 0 {
            return None;
        }
        let idx = self.free_head;
        // Bounds-check: free_head must be a valid descriptor index
        if idx as usize >= QUEUE_SIZE {
            return None;
        }
        let next = unsafe { (*self.desc.add(idx as usize)).next };
        self.free_head = next;
        self.num_free = self.num_free.saturating_sub(1);
        Some(idx)
    }

    /// Return a descriptor chain starting at `head` to the free list.
    /// Walks the NEXT chain; bounded by QUEUE_SIZE to handle corruption.
    pub fn free_chain(&mut self, head: u16) {
        let mut idx = head;
        for _ in 0..QUEUE_SIZE {
            if idx as usize >= QUEUE_SIZE {
                break; // corrupted chain — stop
            }
            let desc = unsafe { &mut *self.desc.add(idx as usize) };
            let has_next = desc.flags & VRING_DESC_F_NEXT != 0;
            let next_idx = desc.next;
            // Return to free list
            desc.flags = 0;
            desc.next = self.free_head;
            self.free_head = idx;
            self.num_free = self.num_free.saturating_add(1);
            if !has_next {
                break;
            }
            idx = next_idx;
        }
    }

    /// Get a mutable reference to descriptor `idx`.
    ///
    /// # Safety
    /// `idx` must be < QUEUE_SIZE and must be allocated (not in free list).
    #[inline]
    pub fn desc_mut(&mut self, idx: u16) -> &mut VirtqDesc {
        // Saturating keeps us in-bounds on bad input
        let clamped = (idx as usize).min(QUEUE_SIZE - 1);
        unsafe { &mut *self.desc.add(clamped) }
    }

    /// Place a descriptor chain head into the available ring and notify device.
    ///
    /// `head` must be the first descriptor of an already-built chain.
    pub fn submit(&mut self, head: u16) {
        // Write ring entry (volatile: shared with device)
        let avail = unsafe { &mut *self.avail };
        let ring_idx = (avail.idx as usize) % QUEUE_SIZE;
        unsafe {
            core::ptr::write_volatile(&mut avail.ring[ring_idx] as *mut u16, head);
        }
        // Ensure the ring entry is visible before we advance idx
        fence(Ordering::SeqCst);
        unsafe {
            core::ptr::write_volatile(&mut avail.idx as *mut u16, avail.idx.wrapping_add(1));
        }
        // Ensure idx is visible before the doorbell write
        fence(Ordering::SeqCst);
        // Doorbell: write queue index to QUEUE_NOTIFY
        crate::io::outw(self.io_base + VIRTIO_REG_QUEUE_NOTIFY, self.queue_idx);
    }

    /// Poll the used ring for a completed descriptor chain.
    ///
    /// Returns `Some((chain_head_id, bytes_written))` if a completion is
    /// available, or `None` if the ring is empty.
    pub fn poll(&mut self) -> Option<(u16, u32)> {
        fence(Ordering::SeqCst);
        let used_idx = unsafe { core::ptr::read_volatile(&(*self.used).idx as *const u16) };
        if self.last_used == used_idx {
            return None;
        }
        let ring_idx = (self.last_used as usize) % QUEUE_SIZE;
        let elem = unsafe {
            core::ptr::read_volatile(&(*self.used).ring[ring_idx] as *const VirtqUsedElem)
        };
        self.last_used = self.last_used.wrapping_add(1);
        Some((elem.id as u16, elem.len))
    }

    /// Add a scatter-gather chain and submit it to the device in one call.
    ///
    /// `chain` is a slice of `(phys_addr, len, device_writable)` entries.
    /// Returns the head descriptor index on success, or `None` if the queue
    /// doesn't have enough free descriptors.
    pub fn add_chain(&mut self, chain: &[(u64, u32, bool)]) -> Option<u16> {
        if chain.is_empty() {
            return None;
        }
        if (self.num_free as usize) < chain.len() {
            return None;
        }

        let mut head = 0u16;
        let mut prev = 0u16;
        let mut is_first = true;

        for (i, &(addr, len, writable)) in chain.iter().enumerate() {
            let idx = self.alloc_desc()?;
            if is_first {
                head = idx;
                is_first = false;
            } else {
                // Link previous descriptor to this one
                let pdesc = self.desc_mut(prev);
                pdesc.next = idx;
                pdesc.flags |= VRING_DESC_F_NEXT;
            }
            let is_last = i == chain.len() - 1;
            let d = self.desc_mut(idx);
            d.addr = addr;
            d.len = len;
            d.flags = if writable { VRING_DESC_F_WRITE } else { 0 };
            if !is_last {
                d.flags |= VRING_DESC_F_NEXT;
            }
            d.next = 0;
            prev = idx;
        }

        self.submit(head);
        Some(head)
    }
}

// ============================================================================
// Public helpers — PCI probing
// ============================================================================

/// Compute the physical page-frame number for a static VirtqBuf reference.
///
/// Under identity mapping virt == phys, so we just shift the address.
/// The PFN is what gets written to VIRTIO_REG_QUEUE_ADDR (legacy).
#[inline]
pub fn buf_pfn(buf: &VirtqBuf) -> u32 {
    // Cast to raw pointer, then to usize (the physical address under identity map)
    let phys = buf as *const VirtqBuf as usize;
    // Page size = 4096 = 2^12
    (phys >> 12) as u32
}

/// Perform the VirtIO legacy device-status handshake up to DRIVER step.
///
/// Call this after BAR0 is known and before queue setup:
///   1. RESET
///   2. ACKNOWLEDGE
///   3. DRIVER
///
/// Returns the device's feature bits.
pub fn device_begin_init(io_base: u16) -> u32 {
    // 1. Reset device
    crate::io::outb(io_base + VIRTIO_REG_DEV_STATUS, 0);
    // 2. Acknowledge existence
    crate::io::outb(io_base + VIRTIO_REG_DEV_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
    // 3. Tell device we are its driver
    crate::io::outb(
        io_base + VIRTIO_REG_DEV_STATUS,
        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
    );
    // Read offered features
    crate::io::inl(io_base + VIRTIO_REG_DEV_FEATURES)
}

/// Write accepted features and set FEATURES_OK.
///
/// Returns `true` if the device accepted the feature set (FEATURES_OK bit
/// stays set after writing), `false` otherwise.
pub fn device_set_features(io_base: u16, features: u32) -> bool {
    crate::io::outl(io_base + VIRTIO_REG_DRV_FEATURES, features);
    crate::io::outb(
        io_base + VIRTIO_REG_DEV_STATUS,
        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
    );
    let status = crate::io::inb(io_base + VIRTIO_REG_DEV_STATUS);
    status & VIRTIO_STATUS_FEATURES_OK != 0
}

/// Select a queue, read its size, write its PFN, and return the size.
///
/// Returns `None` if the device reports size=0 (queue not present).
pub fn setup_queue(io_base: u16, queue_idx: u16, pfn: u32) -> Option<u16> {
    crate::io::outw(io_base + VIRTIO_REG_QUEUE_SEL, queue_idx);
    let size = crate::io::inw(io_base + VIRTIO_REG_QUEUE_SIZE);
    if size == 0 {
        return None;
    }
    crate::io::outl(io_base + VIRTIO_REG_QUEUE_ADDR, pfn);
    Some(size)
}

/// Finalise device initialisation — write DRIVER_OK.
pub fn device_driver_ok(io_base: u16) {
    crate::io::outb(
        io_base + VIRTIO_REG_DEV_STATUS,
        VIRTIO_STATUS_ACKNOWLEDGE
            | VIRTIO_STATUS_DRIVER
            | VIRTIO_STATUS_FEATURES_OK
            | VIRTIO_STATUS_DRIVER_OK,
    );
}

/// Mark device as FAILED.
pub fn device_fail(io_base: u16) {
    crate::io::outb(io_base + VIRTIO_REG_DEV_STATUS, VIRTIO_STATUS_FAILED);
}

/// Scan PCI bus 0 for a VirtIO device matching `target_vendor` / `target_device`.
///
/// Returns `(io_base, bus, dev, func)` on success, or `None` if not found.
/// Also enables PCI bus mastering + I/O space on the found slot.
pub fn pci_find_virtio(target_vendor: u16, target_device: u16) -> Option<(u16, u8, u8, u8)> {
    for dev in 0..32u8 {
        for func in 0..8u8 {
            let vendor = crate::drivers::pci::config_read_u16(0, dev, func, 0);
            if vendor != target_vendor {
                continue;
            }
            let device_id = crate::drivers::pci::config_read_u16(0, dev, func, 2);
            if device_id != target_device {
                continue;
            }
            // BAR0 — must be an I/O BAR (bit 0 set)
            let bar0 = crate::drivers::pci::config_read(0, dev, func, 0x10);
            if bar0 & 1 == 0 {
                // Not an I/O BAR — skip (modern VirtIO uses MMIO, but
                // QEMU legacy emulation always presents I/O BAR0)
                continue;
            }
            let io_base = (bar0 & 0xFFFC) as u16;
            if io_base == 0 {
                continue;
            }
            // Enable I/O space + bus master
            crate::drivers::pci::enable_bus_master(0, dev, func);
            serial_println!(
                "  VirtIO: found {:04x}:{:04x} at {:02x}:{:02x}.{} io={:#x}",
                target_vendor,
                target_device,
                0,
                dev,
                func,
                io_base
            );
            return Some((io_base, 0, dev, func));
        }
    }
    None
}

// ============================================================================
// Global init — called from drivers::init()
// ============================================================================

/// Initialise all VirtIO devices found on the PCI bus.
///
/// Currently delegates to `virtio_blk::init()`. The net driver has been
/// separated into its own module stub and will be added in a later sprint.
pub fn init() {
    super::virtio_blk::init();
}
