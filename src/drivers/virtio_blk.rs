use super::virtio::{
    buf_pfn, device_begin_init, device_driver_ok, device_fail, device_set_features,
    pci_find_virtio, setup_queue, VirtQueue, VirtqBuf, QUEUE_SIZE, VIRTIO_PCI_DEV_BLK,
    VIRTIO_PCI_VENDOR, VIRTIO_REG_CONFIG, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE,
};
/// VirtIO Block Device Driver — no-heap, static-buffer implementation
///
/// Supports QEMU/KVM virtio-blk (PCI vendor 0x1AF4, device 0x1001).
/// Uses the VirtIO legacy interface (I/O BAR0, PFN-based queue setup).
///
/// All buffers are static; no Vec, Box, String, or frame_allocator calls.
/// Identity mapping assumed: virtual address == physical address for statics.
///
/// Public API:
///   virtio_blk_init()       -> bool          probe + initialise
///   virtio_blk_read(s, b)   -> bool          read one 512-byte sector
///   virtio_blk_write(s, b)  -> bool          write one 512-byte sector
///   virtio_blk_flush()      -> bool          flush device cache
///   virtio_blk_capacity()   -> u64           total sectors
///   virtio_blk_is_read_only()-> bool         read-only flag from device
///   init()                              called by drivers::init()
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add/saturating_sub for counters
///   - wrapping_add for ring indices
///   - read_volatile/write_volatile for all MMIO / shared-ring accesses
///   - No panic — use serial_println! + return false on fatal errors
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Feature bits
// ============================================================================

/// Maximum size of any single segment (VIRTIO_BLK_F_SIZE_MAX)
pub const VIRTIO_BLK_F_SIZE_MAX: u32 = 1 << 1;
/// Maximum number of segments (VIRTIO_BLK_F_SEG_MAX)
pub const VIRTIO_BLK_F_SEG_MAX: u32 = 1 << 2;
/// Disk geometry available (VIRTIO_BLK_F_GEOMETRY)
pub const VIRTIO_BLK_F_GEOMETRY: u32 = 1 << 4;
/// Device is read-only (VIRTIO_BLK_F_RO)
pub const VIRTIO_BLK_F_RO: u32 = 1 << 5;
/// Block size reported in config (VIRTIO_BLK_F_BLK_SIZE)
pub const VIRTIO_BLK_F_BLK_SIZE: u32 = 1 << 6;
/// Flush command supported (VIRTIO_BLK_F_FLUSH)
pub const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;

// ============================================================================
// Request type codes
// ============================================================================

/// Read sector(s) from device into driver buffer
pub const VIRTIO_BLK_T_IN: u32 = 0;
/// Write sector(s) from driver buffer to device
pub const VIRTIO_BLK_T_OUT: u32 = 1;
/// Flush volatile write cache
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
/// TRIM / discard sectors
pub const VIRTIO_BLK_T_DISCARD: u32 = 11;

// ============================================================================
// Status byte values (written by device into the status descriptor)
// ============================================================================

const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

// ============================================================================
// Request header — placed in the first (read-only) descriptor of each request
// ============================================================================

/// VirtIO block request header (16 bytes).
/// Written by driver; read by device.
#[repr(C)]
pub struct VirtioBlkReq {
    /// One of VIRTIO_BLK_T_IN / OUT / FLUSH / DISCARD
    pub req_type: u32,
    /// Must be zero
    pub reserved: u32,
    /// 512-byte logical sector to start at (unused for FLUSH)
    pub sector: u64,
}

impl VirtioBlkReq {
    const fn zeroed() -> Self {
        VirtioBlkReq {
            req_type: 0,
            reserved: 0,
            sector: 0,
        }
    }
}

// ============================================================================
// Device state
// ============================================================================

/// Runtime state for one VirtIO block device.
pub struct VirtioBlkDev {
    /// I/O BAR0 base of the PCI device
    pub io_base: u16,
    /// Total number of 512-byte sectors
    pub capacity_sectors: u64,
    /// Logical block size (usually 512; 4096 if BLK_SIZE negotiated)
    pub blk_size: u32,
    /// True if VIRTIO_BLK_F_RO was set by the device
    pub read_only: bool,
    /// True after successful init
    pub active: bool,
    /// Whether the device supports flush (BLK_F_FLUSH)
    supports_flush: bool,
    /// The virtqueue (requestq — virtio-blk has exactly one)
    queue: VirtQueue,
}

// Safety: VirtioBlkDev is only accessed under VIRTIO_BLK Mutex.
unsafe impl Send for VirtioBlkDev {}
unsafe impl Sync for VirtioBlkDev {}

// ============================================================================
// Static storage — all buffers live here, zero-initialised at load
// ============================================================================

/// Virtqueue backing store (descriptor table + available ring + used ring).
/// Must be page-aligned; uses VirtqBuf's repr(C, align(4096)).
// SAFETY: zeroed() is a valid initial state for all fields.
static mut VQ_BUF: VirtqBuf = VirtqBuf::zeroed();

/// Request header for in-flight operation (one at a time, serialised by Mutex).
// SAFETY: only accessed while holding VIRTIO_BLK lock.
static mut REQ_HDR: VirtioBlkReq = VirtioBlkReq::zeroed();

/// Status byte written by device at end of each request.
/// Initialised to 0xFF (invalid); device sets to 0 on success.
// SAFETY: only accessed while holding VIRTIO_BLK lock.
static mut STATUS_BYTE: u8 = 0xFF;

/// Global VirtIO block device instance.
static VIRTIO_BLK: Mutex<Option<VirtioBlkDev>> = Mutex::new(None);

// ============================================================================
// Probe and initialise
// ============================================================================

/// Probe PCI bus for a VirtIO block device and initialise it.
///
/// Returns `true` if a device was found and initialised successfully.
pub fn virtio_blk_init() -> bool {
    // Locate PCI device
    let (io_base, _bus, _dev, _func) = match pci_find_virtio(VIRTIO_PCI_VENDOR, VIRTIO_PCI_DEV_BLK)
    {
        Some(v) => v,
        None => return false,
    };

    // --- VirtIO handshake ---
    let dev_features = device_begin_init(io_base);

    // Negotiate features we care about
    let mut drv_features = 0u32;
    if dev_features & VIRTIO_BLK_F_BLK_SIZE != 0 {
        drv_features |= VIRTIO_BLK_F_BLK_SIZE;
    }
    if dev_features & VIRTIO_BLK_F_RO != 0 {
        drv_features |= VIRTIO_BLK_F_RO;
    }
    if dev_features & VIRTIO_BLK_F_FLUSH != 0 {
        drv_features |= VIRTIO_BLK_F_FLUSH;
    }

    if !device_set_features(io_base, drv_features) {
        serial_println!("  virtio-blk: FEATURES_OK not acknowledged — aborting");
        device_fail(io_base);
        return false;
    }

    // --- Read device config (capacity at offset 0, 8 bytes) ---
    // Config base = VIRTIO_REG_CONFIG (0x14)
    let cap_lo = crate::io::inl(io_base + VIRTIO_REG_CONFIG) as u64;
    let cap_hi = crate::io::inl(io_base + VIRTIO_REG_CONFIG.saturating_add(4)) as u64;
    let capacity_sectors = (cap_hi << 32) | cap_lo;

    // Block size (at config offset 0x14 = 20 bytes after config base — only
    // when VIRTIO_BLK_F_BLK_SIZE was negotiated)
    let blk_size = if drv_features & VIRTIO_BLK_F_BLK_SIZE != 0 {
        // blk_size is at config offset +20 (after capacity[8] + geometry[4] + status[1] + max_seg[4] + seg_cnt[4])
        // For simplicity we hard-code 512 unless we actually see a different value.
        // Config offset for blk_size in virtio-blk spec: byte 20 of device config
        let raw = crate::io::inl(io_base + VIRTIO_REG_CONFIG.saturating_add(20));
        if raw == 0 {
            512
        } else {
            raw
        }
    } else {
        512
    };

    let read_only = dev_features & VIRTIO_BLK_F_RO != 0;
    let supports_flush = drv_features & VIRTIO_BLK_F_FLUSH != 0;

    // --- Set up virtqueue 0 (requestq) ---
    // Get a reference to the static buffer and compute its PFN
    let pfn = unsafe { buf_pfn(&VQ_BUF) };

    let queue_size = match setup_queue(io_base, 0, pfn) {
        Some(s) => s,
        None => {
            serial_println!("  virtio-blk: requestq size=0 — aborting");
            device_fail(io_base);
            return false;
        }
    };

    // Warn if the device wants more descriptors than our static buffer provides
    if queue_size as usize > QUEUE_SIZE {
        serial_println!(
            "  virtio-blk: device queue size {} > static QUEUE_SIZE {} \
             — using {} (device may not like this)",
            queue_size,
            QUEUE_SIZE,
            QUEUE_SIZE
        );
    }

    // Build VirtQueue state pointing into VQ_BUF
    let vq = unsafe { VirtQueue::new(&mut VQ_BUF, io_base, 0) };

    // --- DRIVER_OK ---
    device_driver_ok(io_base);

    serial_println!(
        "  virtio-blk: ready  capacity={} sectors (~{}MiB)  \
         blk_size={}  ro={}  flush={}",
        capacity_sectors,
        capacity_sectors.saturating_mul(512) >> 20,
        blk_size,
        read_only,
        supports_flush,
    );

    // Register with driver subsystem
    super::register("virtio-blk", super::DeviceType::Storage);

    // Store device state
    *VIRTIO_BLK.lock() = Some(VirtioBlkDev {
        io_base,
        capacity_sectors,
        blk_size,
        read_only,
        active: true,
        supports_flush,
        queue: vq,
    });

    true
}

// ============================================================================
// Internal I/O engine
// ============================================================================

/// Issue a 3-descriptor virtio-blk request and spin-poll for completion.
///
/// Descriptor chain:
///   [0] request header  (driver-readable,  16 bytes)
///   [1] data buffer     (rw depends on direction)
///   [2] status byte     (device-writable,   1 byte)
///
/// Returns `true` on success (device wrote status=0).
fn do_request(
    dev: &mut VirtioBlkDev,
    req_type: u32,
    sector: u64,
    data_ptr: *mut u8, // pointer to 512-byte data buffer
    data_len: u32,
    data_write: bool, // true = device writes into data_ptr (read op)
) -> bool {
    // Build request header in static buffer
    unsafe {
        core::ptr::write_volatile(
            &mut REQ_HDR as *mut VirtioBlkReq,
            VirtioBlkReq {
                req_type,
                reserved: 0,
                sector,
            },
        );
        // Pre-set status to invalid so we can detect timeout vs device error
        core::ptr::write_volatile(&mut STATUS_BYTE as *mut u8, 0xFF);
    }

    let req_phys = unsafe { &REQ_HDR as *const VirtioBlkReq as u64 };
    let status_phys = unsafe { &STATUS_BYTE as *const u8 as u64 };
    let data_phys = data_ptr as u64;

    // Allocate 3 descriptors manually (add_chain submits automatically, but
    // we need precise control over the data-descriptor write flag)
    let hdr_idx = match dev.queue.alloc_desc() {
        Some(i) => i,
        None => {
            serial_println!("  virtio-blk: queue full (hdr)");
            return false;
        }
    };
    let data_idx = match dev.queue.alloc_desc() {
        Some(i) => i,
        None => {
            dev.queue.free_chain(hdr_idx);
            serial_println!("  virtio-blk: queue full (data)");
            return false;
        }
    };
    let status_idx = match dev.queue.alloc_desc() {
        Some(i) => i,
        None => {
            dev.queue.free_chain(hdr_idx);
            dev.queue.free_chain(data_idx);
            serial_println!("  virtio-blk: queue full (status)");
            return false;
        }
    };

    // Header descriptor — driver read-only, chained to data
    {
        let d = dev.queue.desc_mut(hdr_idx);
        d.addr = req_phys;
        d.len = core::mem::size_of::<VirtioBlkReq>() as u32;
        d.flags = VRING_DESC_F_NEXT; // device reads this
        d.next = data_idx;
    }

    // Data descriptor — direction depends on request type
    {
        let d = dev.queue.desc_mut(data_idx);
        d.addr = data_phys;
        d.len = data_len;
        d.flags = VRING_DESC_F_NEXT | if data_write { VRING_DESC_F_WRITE } else { 0 };
        d.next = status_idx;
    }

    // Status descriptor — always device-writable, terminates chain
    {
        let d = dev.queue.desc_mut(status_idx);
        d.addr = status_phys;
        d.len = 1;
        d.flags = VRING_DESC_F_WRITE;
        d.next = 0;
    }

    // Submit chain head
    dev.queue.submit(hdr_idx);

    // Spin-poll for completion (≈2M iterations ≈ ~0.5s at 4GHz)
    for _ in 0..2_000_000u32 {
        if let Some((id, _written)) = dev.queue.poll() {
            dev.queue.free_chain(id);
            let status = unsafe { core::ptr::read_volatile(&STATUS_BYTE as *const u8) };
            match status {
                VIRTIO_BLK_S_OK => return true,
                VIRTIO_BLK_S_IOERR => {
                    serial_println!("  virtio-blk: I/O error (sector {})", sector);
                    return false;
                }
                VIRTIO_BLK_S_UNSUPP => {
                    serial_println!(
                        "  virtio-blk: unsupported request type {} sector {}",
                        req_type,
                        sector
                    );
                    return false;
                }
                _ => {
                    serial_println!(
                        "  virtio-blk: unknown status {:#x} sector {}",
                        status,
                        sector
                    );
                    return false;
                }
            }
        }
        core::hint::spin_loop();
    }

    // Timeout — chain was never returned by device
    serial_println!("  virtio-blk: timeout sector {}", sector);
    // Free descriptors we allocated (chain was submitted but not completed)
    dev.queue.free_chain(hdr_idx);
    false
}

// ============================================================================
// 512-byte sector buffer for I/O operations
// ============================================================================

/// Static 512-byte aligned sector buffer shared by read/write calls.
/// Protected by VIRTIO_BLK Mutex (all public functions lock before use).
#[repr(C, align(512))]
struct SectorBuf {
    data: [u8; 512],
}

impl SectorBuf {
    const fn zeroed() -> Self {
        SectorBuf { data: [0u8; 512] }
    }
}

static mut SECTOR_BUF: SectorBuf = SectorBuf::zeroed();

// ============================================================================
// Public API
// ============================================================================

/// Read one 512-byte sector from the block device.
///
/// `sector` — zero-based LBA sector index.
/// `buf`    — caller-supplied 512-byte output buffer.
///
/// Returns `true` on success.
pub fn virtio_blk_read(sector: u64, buf: &mut [u8; 512]) -> bool {
    let mut guard = VIRTIO_BLK.lock();
    let dev = match guard.as_mut() {
        Some(d) if d.active => d,
        _ => {
            serial_println!("  virtio-blk: read called but device not ready");
            return false;
        }
    };

    if sector >= dev.capacity_sectors {
        serial_println!(
            "  virtio-blk: read sector {} beyond capacity {}",
            sector,
            dev.capacity_sectors
        );
        return false;
    }

    // Use the static sector buffer as the DMA target (avoids any stack allocation)
    let dma_ptr = unsafe { SECTOR_BUF.data.as_mut_ptr() };
    if !do_request(dev, VIRTIO_BLK_T_IN, sector, dma_ptr, 512, true) {
        return false;
    }

    // Copy result from static buffer to caller's buffer
    unsafe {
        core::ptr::copy_nonoverlapping(SECTOR_BUF.data.as_ptr(), buf.as_mut_ptr(), 512);
    }
    true
}

/// Write one 512-byte sector to the block device.
///
/// `sector` — zero-based LBA sector index.
/// `data`   — caller-supplied 512-byte data to write.
///
/// Returns `true` on success.
pub fn virtio_blk_write(sector: u64, data: &[u8; 512]) -> bool {
    let mut guard = VIRTIO_BLK.lock();
    let dev = match guard.as_mut() {
        Some(d) if d.active => d,
        _ => {
            serial_println!("  virtio-blk: write called but device not ready");
            return false;
        }
    };

    if dev.read_only {
        serial_println!("  virtio-blk: write refused — device is read-only");
        return false;
    }
    if sector >= dev.capacity_sectors {
        serial_println!(
            "  virtio-blk: write sector {} beyond capacity {}",
            sector,
            dev.capacity_sectors
        );
        return false;
    }

    // Copy caller data into static DMA buffer
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), SECTOR_BUF.data.as_mut_ptr(), 512);
    }

    let dma_ptr = unsafe { SECTOR_BUF.data.as_mut_ptr() };
    do_request(dev, VIRTIO_BLK_T_OUT, sector, dma_ptr, 512, false)
}

/// Flush the device's volatile write cache.
///
/// Only meaningful if `VIRTIO_BLK_F_FLUSH` was negotiated.
/// Returns `true` on success (or if flush is not supported — it's a no-op).
pub fn virtio_blk_flush() -> bool {
    let mut guard = VIRTIO_BLK.lock();
    let dev = match guard.as_mut() {
        Some(d) if d.active => d,
        _ => {
            serial_println!("  virtio-blk: flush called but device not ready");
            return false;
        }
    };

    if !dev.supports_flush {
        return true; // nothing to flush
    }

    // Flush request: header + status only (no data descriptor).
    // We reuse do_request with a zero-length… but do_request always chains a
    // data descriptor.  For FLUSH we build the chain manually.

    unsafe {
        core::ptr::write_volatile(
            &mut REQ_HDR as *mut VirtioBlkReq,
            VirtioBlkReq {
                req_type: VIRTIO_BLK_T_FLUSH,
                reserved: 0,
                sector: 0,
            },
        );
        core::ptr::write_volatile(&mut STATUS_BYTE as *mut u8, 0xFF);
    }

    let req_phys = unsafe { &REQ_HDR as *const VirtioBlkReq as u64 };
    let status_phys = unsafe { &STATUS_BYTE as *const u8 as u64 };

    let hdr_idx = match dev.queue.alloc_desc() {
        Some(i) => i,
        None => {
            serial_println!("  virtio-blk: flush queue full (hdr)");
            return false;
        }
    };
    let status_idx = match dev.queue.alloc_desc() {
        Some(i) => i,
        None => {
            dev.queue.free_chain(hdr_idx);
            serial_println!("  virtio-blk: flush queue full (status)");
            return false;
        }
    };

    {
        let d = dev.queue.desc_mut(hdr_idx);
        d.addr = req_phys;
        d.len = core::mem::size_of::<VirtioBlkReq>() as u32;
        d.flags = VRING_DESC_F_NEXT;
        d.next = status_idx;
    }
    {
        let d = dev.queue.desc_mut(status_idx);
        d.addr = status_phys;
        d.len = 1;
        d.flags = VRING_DESC_F_WRITE;
        d.next = 0;
    }

    dev.queue.submit(hdr_idx);

    for _ in 0..2_000_000u32 {
        if let Some((id, _)) = dev.queue.poll() {
            dev.queue.free_chain(id);
            let status = unsafe { core::ptr::read_volatile(&STATUS_BYTE as *const u8) };
            return match status {
                VIRTIO_BLK_S_OK => true,
                VIRTIO_BLK_S_IOERR => {
                    serial_println!("  virtio-blk: flush I/O error");
                    false
                }
                VIRTIO_BLK_S_UNSUPP => {
                    serial_println!("  virtio-blk: flush not supported by device");
                    false
                }
                _ => {
                    serial_println!("  virtio-blk: flush unknown status {:#x}", status);
                    false
                }
            };
        }
        core::hint::spin_loop();
    }

    serial_println!("  virtio-blk: flush timeout");
    dev.queue.free_chain(hdr_idx);
    false
}

/// Return total number of 512-byte sectors on the device.
/// Returns 0 if the device is not initialised.
pub fn virtio_blk_capacity() -> u64 {
    VIRTIO_BLK
        .lock()
        .as_ref()
        .map(|d| d.capacity_sectors)
        .unwrap_or(0)
}

/// Return `true` if the device is read-only.
pub fn virtio_blk_is_read_only() -> bool {
    VIRTIO_BLK
        .lock()
        .as_ref()
        .map(|d| d.read_only)
        .unwrap_or(false)
}

/// Return `true` if the block device has been successfully initialised.
pub fn virtio_blk_available() -> bool {
    VIRTIO_BLK
        .lock()
        .as_ref()
        .map(|d| d.active)
        .unwrap_or(false)
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Probe and initialise the VirtIO block device.
/// Logs result to serial port. Called once during kernel boot.
pub fn init() {
    if virtio_blk_init() {
        serial_println!(
            "  virtio-blk: init OK  capacity={}MiB  ro={}",
            virtio_blk_capacity().saturating_mul(512) >> 20,
            virtio_blk_is_read_only(),
        );
    } else {
        serial_println!("  virtio-blk: no device found (or init failed)");
    }
}
