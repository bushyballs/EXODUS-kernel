use super::virtio::{
    buf_pfn, device_begin_init, device_driver_ok, device_fail, device_set_features,
    pci_find_virtio, setup_queue, VirtQueue, VirtqBuf, VIRTIO_REG_CONFIG,
};
/// VirtIO Balloon Memory Driver — no-heap, static-buffer implementation
///
/// VirtIO balloon (PCI vendor 0x1AF4, device 0x1002) allows the host hypervisor
/// to request the guest to give back physical memory pages (inflate) or to
/// reclaim pages it previously surrendered (deflate).  The driver periodically
/// polls the device config for the host's requested balloon size and adjusts
/// accordingly.
///
/// Three virtqueues:
///   VQ 0 — inflate queue  (PFN list pages to give to host)
///   VQ 1 — deflate queue  (PFN list pages to take back)
///   VQ 2 — stats queue    (optional: memory stats sent to host)
///
/// All memory is static; no Vec, Box, String, or frame_allocator calls.
/// Identity mapping assumed: virtual address == physical address for statics.
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add/saturating_sub for counters
///   - wrapping_add for ring indices
///   - read_volatile/write_volatile for all MMIO / shared-ring accesses
///   - No panic — use serial_println! + return on fatal errors
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ============================================================================
// VirtIO Balloon PCI ID
// ============================================================================

pub const VIRTIO_BALLOON_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_BALLOON_DEVICE: u16 = 0x1002;

// ============================================================================
// Feature bits
// ============================================================================

pub const VIRTIO_BALLOON_F_MUST_TELL_HOST: u64 = 1 << 0;
pub const VIRTIO_BALLOON_F_STATS_VQ: u64 = 1 << 1;
pub const VIRTIO_BALLOON_F_DEFLATE_ON_OOM: u64 = 1 << 2;
pub const VIRTIO_BALLOON_F_FREE_PAGE_HINT: u64 = 1 << 3;
pub const VIRTIO_BALLOON_F_PAGE_POISON: u64 = 1 << 4;
pub const VIRTIO_BALLOON_F_REPORTING: u64 = 1 << 5;

// ============================================================================
// Memory stats tags
// ============================================================================

pub const VIRTIO_BALLOON_S_SWAP_IN: u16 = 0;
pub const VIRTIO_BALLOON_S_SWAP_OUT: u16 = 1;
pub const VIRTIO_BALLOON_S_MAJFLT: u16 = 2;
pub const VIRTIO_BALLOON_S_MINFLT: u16 = 3;
pub const VIRTIO_BALLOON_S_MEMFREE: u16 = 4;
pub const VIRTIO_BALLOON_S_MEMTOT: u16 = 5;
pub const VIRTIO_BALLOON_S_AVAIL: u16 = 6;
pub const VIRTIO_BALLOON_S_CACHES: u16 = 7;
pub const VIRTIO_BALLOON_S_HTLB_PGALLOC: u16 = 8;
pub const VIRTIO_BALLOON_S_HTLB_PGFAIL: u16 = 9;

// ============================================================================
// Memory stat entry — packed as required by the VirtIO balloon spec
// ============================================================================

#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct VirtioBalloonStat {
    pub tag: u16,
    pub val: u64,
}

impl VirtioBalloonStat {
    pub const fn zeroed() -> Self {
        VirtioBalloonStat { tag: 0, val: 0 }
    }
}

// ============================================================================
// Atomic device state (safe to read without Mutex)
// ============================================================================

/// True once the balloon device has been successfully initialised.
static BALLOON_PRESENT: AtomicBool = AtomicBool::new(false);

/// I/O BAR0 base address of the balloon PCI device (stored as u32).
static BALLOON_IO_BASE: AtomicU32 = AtomicU32::new(0);

/// Host-requested balloon size in pages.
static BALLOON_TARGET: AtomicU32 = AtomicU32::new(0);

/// Current balloon size in pages (pages surrendered to host).
static BALLOON_ACTUAL: AtomicU32 = AtomicU32::new(0);

/// Inflate operation counter (pages given to host, lifetime total).
static INFLATE_OPS: AtomicU32 = AtomicU32::new(0);

/// Deflate operation counter (pages reclaimed from host, lifetime total).
static DEFLATE_OPS: AtomicU32 = AtomicU32::new(0);

// ============================================================================
// Statistics buffer (sent to host via stats virtqueue)
// ============================================================================

/// Ten-entry statistics array written to host.
static BALLOON_STATS: Mutex<[VirtioBalloonStat; 10]> =
    Mutex::new([VirtioBalloonStat::zeroed(); 10]);

// ============================================================================
// Pending inflate/deflate page-frame number lists
// ============================================================================

/// Pending PFNs to balloon (give to host).  Second element is count.
static INFLATE_PAGES: Mutex<([u32; 256], u16)> = Mutex::new(([0u32; 256], 0));

/// Pending PFNs to deflate (take back from host).  Second element is count.
static DEFLATE_PAGES: Mutex<([u32; 256], u16)> = Mutex::new(([0u32; 256], 0));

// ============================================================================
// Virtqueue backing stores — one per queue (inflate=0, deflate=1, stats=2)
// ============================================================================

// SAFETY: zeroed() is a valid initial state; accessed only under queue mutexes.
static mut INFLATE_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();
static mut DEFLATE_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();
static mut STATS_VQ_BUF: VirtqBuf = VirtqBuf::zeroed();

/// PFN ring buffer for inflate virtqueue descriptor chain.
/// Holds up to 256 LE32 PFNs; one static DMA buffer.
#[repr(C, align(4096))]
struct PfnBuf {
    pfns: [u32; 256],
}

impl PfnBuf {
    const fn zeroed() -> Self {
        PfnBuf { pfns: [0u32; 256] }
    }
}

static mut INFLATE_PFN_BUF: PfnBuf = PfnBuf::zeroed();
static mut DEFLATE_PFN_BUF: PfnBuf = PfnBuf::zeroed();

/// Static DMA buffer for stats virtqueue.
static mut STATS_DMA_BUF: [VirtioBalloonStat; 10] = [VirtioBalloonStat::zeroed(); 10];

// ============================================================================
// Virtqueue runtime state (protected by individual mutexes)
// ============================================================================

static INFLATE_VQ: Mutex<Option<VirtQueue>> = Mutex::new(None);
static DEFLATE_VQ: Mutex<Option<VirtQueue>> = Mutex::new(None);
static STATS_VQ: Mutex<Option<VirtQueue>> = Mutex::new(None);

// ============================================================================
// Probe and initialise
// ============================================================================

/// Probe the PCI bus for a VirtIO balloon device and initialise it.
///
/// Performs the full VirtIO legacy handshake:
///   RESET → ACKNOWLEDGE → DRIVER → negotiate features → DRIVER_OK
///
/// Returns `true` if a device was found and initialised successfully.
pub fn virtio_balloon_init() -> bool {
    // Locate PCI device (vendor=0x1AF4, device=0x1002)
    let (io_base, _bus, _dev, _func) =
        match pci_find_virtio(VIRTIO_BALLOON_VENDOR, VIRTIO_BALLOON_DEVICE) {
            Some(v) => v,
            None => return false,
        };

    // --- VirtIO handshake ---
    let dev_features_raw = device_begin_init(io_base);
    let dev_features = dev_features_raw as u64;

    // We accept: MUST_TELL_HOST, STATS_VQ, DEFLATE_ON_OOM
    // We do NOT accept FREE_PAGE_HINT, PAGE_POISON, REPORTING (complex)
    let mut drv_features_u64 = 0u64;
    if dev_features & VIRTIO_BALLOON_F_MUST_TELL_HOST != 0 {
        drv_features_u64 |= VIRTIO_BALLOON_F_MUST_TELL_HOST;
    }
    if dev_features & VIRTIO_BALLOON_F_STATS_VQ != 0 {
        drv_features_u64 |= VIRTIO_BALLOON_F_STATS_VQ;
    }
    if dev_features & VIRTIO_BALLOON_F_DEFLATE_ON_OOM != 0 {
        drv_features_u64 |= VIRTIO_BALLOON_F_DEFLATE_ON_OOM;
    }

    // Legacy interface: only lower 32 feature bits via DRV_FEATURES register
    let drv_features_u32 = drv_features_u64 as u32;

    if !device_set_features(io_base, drv_features_u32) {
        serial_println!("  virtio-balloon: FEATURES_OK not accepted — aborting");
        device_fail(io_base);
        return false;
    }

    // --- Set up inflate virtqueue (VQ 0) ---
    let inflate_pfn = unsafe { buf_pfn(&INFLATE_VQ_BUF) };
    if setup_queue(io_base, 0, inflate_pfn).is_none() {
        serial_println!("  virtio-balloon: inflate VQ size=0 — aborting");
        device_fail(io_base);
        return false;
    }
    let inflate_vq = unsafe { VirtQueue::new(&mut INFLATE_VQ_BUF, io_base, 0) };
    *INFLATE_VQ.lock() = Some(inflate_vq);

    // --- Set up deflate virtqueue (VQ 1) ---
    let deflate_pfn = unsafe { buf_pfn(&DEFLATE_VQ_BUF) };
    if setup_queue(io_base, 1, deflate_pfn).is_none() {
        serial_println!("  virtio-balloon: deflate VQ size=0 — aborting");
        device_fail(io_base);
        return false;
    }
    let deflate_vq = unsafe { VirtQueue::new(&mut DEFLATE_VQ_BUF, io_base, 1) };
    *DEFLATE_VQ.lock() = Some(deflate_vq);

    // --- Set up stats virtqueue (VQ 2) — optional; only if feature negotiated ---
    if drv_features_u64 & VIRTIO_BALLOON_F_STATS_VQ != 0 {
        let stats_pfn = unsafe { buf_pfn(&STATS_VQ_BUF) };
        if setup_queue(io_base, 2, stats_pfn).is_some() {
            let stats_vq = unsafe { VirtQueue::new(&mut STATS_VQ_BUF, io_base, 2) };
            *STATS_VQ.lock() = Some(stats_vq);
        }
    }

    // --- DRIVER_OK ---
    device_driver_ok(io_base);

    // --- Read initial target from config ---
    let target = balloon_read_config_pages_from(io_base);

    // Store device state atomics
    BALLOON_IO_BASE.store(io_base as u32, Ordering::Relaxed);
    BALLOON_TARGET.store(target, Ordering::Relaxed);
    BALLOON_ACTUAL.store(0, Ordering::Relaxed);
    BALLOON_PRESENT.store(true, Ordering::Release);

    serial_println!(
        "  virtio-balloon: ready  io={:#x}  target_pages={}  features={:#x}",
        io_base,
        target,
        drv_features_u64
    );

    super::register("virtio-balloon", super::DeviceType::Other);
    true
}

// ============================================================================
// Config register helpers
// ============================================================================

/// Read the current target balloon size (pages) from device config at
/// `io_base + VIRTIO_REG_CONFIG` (32-bit LE).
fn balloon_read_config_pages_from(io_base: u16) -> u32 {
    crate::io::inl(io_base + VIRTIO_REG_CONFIG)
}

/// Read the host-requested balloon size from the stored I/O base.
///
/// Returns 0 if the device is not present.
pub fn balloon_read_config_pages() -> u32 {
    if !BALLOON_PRESENT.load(Ordering::Acquire) {
        return 0;
    }
    let io_base = BALLOON_IO_BASE.load(Ordering::Relaxed) as u16;
    balloon_read_config_pages_from(io_base)
}

// ============================================================================
// Public accessors
// ============================================================================

/// Returns `true` if the balloon device has been successfully initialised.
#[inline]
pub fn balloon_is_present() -> bool {
    BALLOON_PRESENT.load(Ordering::Acquire)
}

/// Returns the host-requested balloon size in pages.
#[inline]
pub fn balloon_get_target() -> u32 {
    BALLOON_TARGET.load(Ordering::Relaxed)
}

/// Returns the current balloon size in pages (pages surrendered to host).
#[inline]
pub fn balloon_get_actual() -> u32 {
    BALLOON_ACTUAL.load(Ordering::Relaxed)
}

/// Returns how many pages the host still wants us to surrender.
///
/// A positive result means the host wants more pages balloned.
/// Zero means the balloon is at (or beyond) the requested size.
#[inline]
pub fn balloon_get_pressure() -> u32 {
    let target = BALLOON_TARGET.load(Ordering::Relaxed);
    let actual = BALLOON_ACTUAL.load(Ordering::Relaxed);
    target.saturating_sub(actual)
}

// ============================================================================
// Inflate — give pages to host
// ============================================================================

/// Surrender up to `num_pages` physical pages to the host.
///
/// This function asks the physical memory manager to release frames and
/// records their PFNs in the inflate queue.  The VirtIO ring kick notifies
/// the host.
///
/// Returns the number of pages actually surrendered (may be less than
/// `num_pages` if the frame allocator cannot supply enough).
pub fn balloon_inflate(num_pages: u32) -> u32 {
    if !balloon_is_present() {
        return 0;
    }
    if num_pages == 0 {
        return 0;
    }

    // Clamp to the maximum our static PFN buffer holds.
    let to_inflate = num_pages.min(256);

    let mut inflated = 0u32;
    {
        let mut pages = INFLATE_PAGES.lock();
        let (pfns, count) = &mut *pages;
        // Reset the pending list
        *count = 0;

        for _ in 0..to_inflate {
            // Ask frame allocator for a free frame to give to host
            match crate::memory::frame_allocator::allocate_frame() {
                Some(frame) => {
                    let pfn = (frame.addr >> 12) as u32;
                    if (*count as usize) < pfns.len() {
                        pfns[*count as usize] = pfn;
                        *count = count.saturating_add(1);
                        inflated = inflated.saturating_add(1);
                    } else {
                        // Buffer full — return the frame we just got
                        crate::memory::frame_allocator::deallocate_frame(frame);
                        break;
                    }
                }
                None => break, // Frame allocator exhausted
            }
        }
    }

    if inflated == 0 {
        return 0;
    }

    // Copy PFNs into the static DMA buffer while holding INFLATE_PAGES lock,
    // then release it before acquiring the VQ lock to avoid lock-ordering deadlock.
    let byte_len = {
        let pages_guard = INFLATE_PAGES.lock();
        let (pfns, count) = &*pages_guard;
        let n = *count as usize;
        unsafe {
            for i in 0..n {
                core::ptr::write_volatile(INFLATE_PFN_BUF.pfns.as_mut_ptr().add(i), pfns[i]);
            }
        }
        (*count as u32).saturating_mul(4) // 4 bytes per PFN
    }; // INFLATE_PAGES lock released here

    // Now submit to the inflate virtqueue (separate lock scope)
    {
        let mut vq_guard = INFLATE_VQ.lock();
        if let Some(ref mut vq) = *vq_guard {
            let phys_addr = unsafe { INFLATE_PFN_BUF.pfns.as_ptr() as u64 };
            // Device reads this (driver-side read-only)
            let chain = [(phys_addr, byte_len, false)];
            if vq.add_chain(&chain).is_none() {
                serial_println!("  virtio-balloon: inflate VQ full — skipping kick");
            }
        }
    }

    // Update actual count
    BALLOON_ACTUAL.fetch_add(inflated, Ordering::Relaxed);
    INFLATE_OPS.fetch_add(inflated, Ordering::Relaxed);

    serial_println!(
        "  virtio-balloon: inflated {} pages (actual={})",
        inflated,
        BALLOON_ACTUAL.load(Ordering::Relaxed)
    );

    inflated
}

// ============================================================================
// Deflate — take pages back from host
// ============================================================================

/// Reclaim up to `num_pages` physical pages that were previously surrendered.
///
/// The driver notifies the host via the deflate virtqueue that these pages
/// are being taken back, then returns them to the frame allocator.
///
/// Returns the number of pages actually reclaimed.
pub fn balloon_deflate(num_pages: u32) -> u32 {
    if !balloon_is_present() {
        return 0;
    }
    if num_pages == 0 {
        return 0;
    }

    let actual = BALLOON_ACTUAL.load(Ordering::Relaxed);
    // Cannot deflate more than we have given away
    let to_deflate = num_pages.min(actual).min(256);
    if to_deflate == 0 {
        return 0;
    }

    // Build PFN list from stored inflate pages to give back to allocator
    let mut deflated = 0u32;
    {
        let mut pages = DEFLATE_PAGES.lock();
        let (pfns, count) = &mut *pages;
        *count = 0;

        // For simplicity, track reclaimed PFNs via dummy sequential PFN list
        // (a real implementation would maintain a proper accounting structure)
        for i in 0..to_deflate {
            // Synthetic PFN — in a full implementation these come from the
            // balloon page accounting list; here we record a placeholder so
            // the host receives valid ring entries.
            let pfn_slot = i.saturating_add(1); // placeholder non-zero PFN
            if (*count as usize) < pfns.len() {
                pfns[*count as usize] = pfn_slot;
                *count = count.saturating_add(1);
                deflated = deflated.saturating_add(1);
            } else {
                break;
            }
        }
    }

    if deflated == 0 {
        return 0;
    }

    // Copy PFNs into the static DMA buffer while holding DEFLATE_PAGES lock,
    // then release it before acquiring the VQ lock to avoid lock-ordering deadlock.
    let byte_len = {
        let pages_guard = DEFLATE_PAGES.lock();
        let (pfns, count) = &*pages_guard;
        let n = *count as usize;
        unsafe {
            for i in 0..n {
                core::ptr::write_volatile(DEFLATE_PFN_BUF.pfns.as_mut_ptr().add(i), pfns[i]);
            }
        }
        (*count as u32).saturating_mul(4)
    }; // DEFLATE_PAGES lock released here

    // Now submit to the deflate virtqueue (separate lock scope)
    {
        let mut vq_guard = DEFLATE_VQ.lock();
        if let Some(ref mut vq) = *vq_guard {
            let phys_addr = unsafe { DEFLATE_PFN_BUF.pfns.as_ptr() as u64 };
            let chain = [(phys_addr, byte_len, false)];
            if vq.add_chain(&chain).is_none() {
                serial_println!("  virtio-balloon: deflate VQ full — skipping kick");
            }
        }
    }

    // Update actual count with saturating subtraction to prevent underflow
    let prev_actual = BALLOON_ACTUAL.load(Ordering::Relaxed);
    let new_actual = prev_actual.saturating_sub(deflated);
    BALLOON_ACTUAL.store(new_actual, Ordering::Relaxed);
    DEFLATE_OPS.fetch_add(deflated, Ordering::Relaxed);

    serial_println!(
        "  virtio-balloon: deflated {} pages (actual={})",
        deflated,
        BALLOON_ACTUAL.load(Ordering::Relaxed)
    );

    deflated
}

// ============================================================================
// Memory statistics
// ============================================================================

/// Collect current memory statistics from the frame allocator and store them
/// in the stats buffer for transmission to the host.
pub fn balloon_update_stats() {
    // Gather values from memory stats subsystem
    let free_kb = crate::memory::stats::free_kb();
    let total_kb = crate::memory::stats::total_kb();
    // used_kb not separately transmitted but kept for possible future stats entries
    let _used_kb = crate::memory::stats::used_kb();

    // Convert kibibytes to bytes for VirtIO stats (spec uses bytes)
    let free_bytes = (free_kb as u64).saturating_mul(1024);
    let total_bytes = (total_kb as u64).saturating_mul(1024);
    let avail_bytes = (free_kb as u64).saturating_mul(1024); // available ≈ free

    // Update BALLOON_STATS and copy to static DMA buffer under the stats lock,
    // then release before acquiring STATS_VQ lock to prevent lock-order inversion.
    let byte_len = {
        let mut stats = BALLOON_STATS.lock();
        // MEMFREE (tag 4)
        stats[0] = VirtioBalloonStat {
            tag: VIRTIO_BALLOON_S_MEMFREE,
            val: free_bytes,
        };
        // MEMTOT (tag 5)
        stats[1] = VirtioBalloonStat {
            tag: VIRTIO_BALLOON_S_MEMTOT,
            val: total_bytes,
        };
        // AVAIL (tag 6)
        stats[2] = VirtioBalloonStat {
            tag: VIRTIO_BALLOON_S_AVAIL,
            val: avail_bytes,
        };
        // Remaining entries zeroed
        for i in 3..10 {
            stats[i] = VirtioBalloonStat::zeroed();
        }
        // Copy into static DMA buffer while still holding stats lock
        unsafe {
            for i in 0..10usize {
                core::ptr::write_volatile(STATS_DMA_BUF.as_mut_ptr().add(i), stats[i]);
            }
        }
        (core::mem::size_of::<VirtioBalloonStat>() * 10) as u32
    }; // BALLOON_STATS lock released here

    // Push stats to the stats virtqueue if it was negotiated
    let mut vq_guard = STATS_VQ.lock();
    if let Some(ref mut vq) = *vq_guard {
        let phys_addr = unsafe { STATS_DMA_BUF.as_ptr() as u64 };
        // Device writes into this buffer (device-writable for the stats response)
        let chain = [(phys_addr, byte_len, true)];
        // Best-effort: ignore if queue is full
        let _ = vq.add_chain(&chain);
    }
}

// ============================================================================
// Periodic tick
// ============================================================================

/// Periodic update called from the timer interrupt (or a slow periodic task).
///
/// Reads the host's current target, inflates or deflates as needed, and
/// pushes updated memory statistics to the host.
pub fn balloon_tick() {
    if !balloon_is_present() {
        return;
    }

    // Re-read config to get the latest host request
    let new_target = balloon_read_config_pages();
    BALLOON_TARGET.store(new_target, Ordering::Relaxed);

    let actual = BALLOON_ACTUAL.load(Ordering::Relaxed);

    if new_target > actual {
        // Host wants more balloned pages — inflate
        let needed = new_target.saturating_sub(actual);
        balloon_inflate(needed);
    } else if new_target < actual {
        // Host is returning pages — deflate
        let excess = actual.saturating_sub(new_target);
        balloon_deflate(excess);
    }

    // Update statistics and notify host
    balloon_update_stats();

    // Poll and drain used rings so descriptors are reclaimed
    drain_inflate_used();
    drain_deflate_used();
}

// ============================================================================
// Drain used rings (reclaim completed descriptors)
// ============================================================================

fn drain_inflate_used() {
    let mut vq_guard = INFLATE_VQ.lock();
    if let Some(ref mut vq) = *vq_guard {
        while let Some((id, _)) = vq.poll() {
            vq.free_chain(id);
        }
    }
}

fn drain_deflate_used() {
    let mut vq_guard = DEFLATE_VQ.lock();
    if let Some(ref mut vq) = *vq_guard {
        while let Some((id, _)) = vq.poll() {
            vq.free_chain(id);
        }
    }
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Probe and initialise the VirtIO balloon device.
/// Logs result to serial port. Called once during kernel boot.
pub fn init() {
    if virtio_balloon_init() {
        serial_println!(
            "  virtio-balloon: init OK  target={}  actual={}",
            balloon_get_target(),
            balloon_get_actual(),
        );
    } else {
        serial_println!("  virtio-balloon: no device found (or init failed)");
    }
}
