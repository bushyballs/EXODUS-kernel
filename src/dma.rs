//! DMA Subsystem
//!
//! Provides DMA channel allocation, physical address mapping, and transfer
//! management for Genesis AIOS.  Identity-mapped (phys == virt) on x86-64
//! without an IOMMU; cache-coherent so sync stubs are no-ops.
//!
//! Rules: no_std, no heap, no floats, no panic, saturating/wrapping counters,
//! MMIO via read_volatile/write_volatile, Copy + const-fn empty() on all
//! static structs.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

pub const MAX_DMA_CHANNELS: usize = 16;
pub const MAX_DMA_MAPPINGS: usize = 64;

pub const DMA_DIRECTION_TO_DEVICE: u8 = 1;
pub const DMA_DIRECTION_FROM_DEVICE: u8 = 2;
pub const DMA_DIRECTION_BIDIRECTIONAL: u8 = 3;

/// Physical/bus address type (64-bit DMA).
pub type DmaAddr = u64;

// ---------------------------------------------------------------------------
// DmaMapping
// ---------------------------------------------------------------------------

/// A single DMA memory mapping (virtual→physical address binding).
#[derive(Copy, Clone)]
pub struct DmaMapping {
    /// Kernel virtual address of the mapped region.
    pub virt_addr: u64,
    /// Physical/bus address handed to the device.
    pub phys_addr: DmaAddr,
    /// Size of the mapped region in bytes.
    pub size: usize,
    /// Transfer direction (TO_DEVICE / FROM_DEVICE / BIDIRECTIONAL).
    pub direction: u8,
    /// Device that owns this mapping.
    pub dev_id: u32,
    /// Whether this slot is occupied.
    pub active: bool,
}

impl DmaMapping {
    pub const fn empty() -> Self {
        DmaMapping {
            virt_addr: 0,
            phys_addr: 0,
            size: 0,
            direction: 0,
            dev_id: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// DmaChannel
// ---------------------------------------------------------------------------

/// A DMA engine channel.
#[derive(Copy, Clone)]
pub struct DmaChannel {
    /// Hardware channel identifier.
    pub id: u32,
    /// Device that has claimed this channel.
    pub dev_id: u32,
    /// True while a transfer is in progress.
    pub busy: bool,
    /// Transfer direction for the current (or last) transfer.
    pub direction: u8,
    /// Index into `DMA_MAPPINGS` for the active transfer.
    pub current_mapping: u32,
    /// Running total of bytes transferred (saturating).
    pub bytes_transferred: u64,
    /// Whether this slot is allocated.
    pub active: bool,
}

impl DmaChannel {
    pub const fn empty() -> Self {
        DmaChannel {
            id: 0,
            dev_id: 0,
            busy: false,
            direction: 0,
            current_mapping: 0,
            bytes_transferred: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static tables
// ---------------------------------------------------------------------------

static DMA_MAPPINGS: Mutex<[DmaMapping; MAX_DMA_MAPPINGS]> =
    Mutex::new([DmaMapping::empty(); MAX_DMA_MAPPINGS]);

static DMA_CHANNELS: Mutex<[DmaChannel; MAX_DMA_CHANNELS]> =
    Mutex::new([DmaChannel::empty(); MAX_DMA_CHANNELS]);

// ---------------------------------------------------------------------------
// Mapping API
// ---------------------------------------------------------------------------

/// Map a kernel virtual address for DMA by a device.
///
/// On x86-64 with identity paging (no IOMMU) the physical address equals the
/// virtual address.  Records the mapping in the global table and returns the
/// bus address on success, or `None` if the table is full.
pub fn dma_map_single(dev_id: u32, virt_addr: u64, size: usize, dir: u8) -> Option<DmaAddr> {
    if size == 0 {
        return None;
    }
    let mut mappings = DMA_MAPPINGS.lock();
    // Find a free slot.
    for slot in mappings.iter_mut() {
        if !slot.active {
            // Identity mapping: physical address == virtual address.
            let phys = virt_addr;
            slot.virt_addr = virt_addr;
            slot.phys_addr = phys;
            slot.size = size;
            slot.direction = dir;
            slot.dev_id = dev_id;
            slot.active = true;
            return Some(phys);
        }
    }
    None // Table full.
}

/// Unmap a previously-mapped DMA region.
///
/// Locates the mapping by physical address + device-id and marks it inactive.
pub fn dma_unmap_single(dev_id: u32, phys_addr: DmaAddr, size: usize, _dir: u8) {
    if size == 0 {
        return;
    }
    let mut mappings = DMA_MAPPINGS.lock();
    for slot in mappings.iter_mut() {
        if slot.active && slot.dev_id == dev_id && slot.phys_addr == phys_addr {
            *slot = DmaMapping::empty();
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Channel API
// ---------------------------------------------------------------------------

/// Allocate a DMA channel for a device.
///
/// Returns the channel id on success, or `None` if all channels are busy.
pub fn dma_alloc_channel(dev_id: u32) -> Option<u32> {
    let mut channels = DMA_CHANNELS.lock();
    for (idx, ch) in channels.iter_mut().enumerate() {
        if !ch.active {
            ch.id = idx as u32;
            ch.dev_id = dev_id;
            ch.busy = false;
            ch.active = true;
            return Some(idx as u32);
        }
    }
    None
}

/// Release a DMA channel back to the pool.
pub fn dma_free_channel(chan_id: u32) {
    if (chan_id as usize) >= MAX_DMA_CHANNELS {
        return;
    }
    let mut channels = DMA_CHANNELS.lock();
    channels[chan_id as usize] = DmaChannel::empty();
}

/// Submit a DMA transfer on a channel.
///
/// Marks the channel busy and records the mapping index.  The `bytes_transferred`
/// counter is updated with saturating addition.  Returns `false` if the channel
/// index is out of range, the channel is not active, or it is already busy.
pub fn dma_submit(chan_id: u32, mapping_idx: u32, size: usize) -> bool {
    if (chan_id as usize) >= MAX_DMA_CHANNELS {
        return false;
    }
    if (mapping_idx as usize) >= MAX_DMA_MAPPINGS {
        return false;
    }
    let mut channels = DMA_CHANNELS.lock();
    let ch = &mut channels[chan_id as usize];
    if !ch.active || ch.busy {
        return false;
    }
    ch.busy = true;
    ch.current_mapping = mapping_idx;
    ch.bytes_transferred = ch.bytes_transferred.saturating_add(size as u64);
    true
}

/// Check whether a DMA transfer has completed.
///
/// In simulation DMA completes instantaneously, so this always returns `true`
/// for any active channel and clears the busy flag.
pub fn dma_is_complete(chan_id: u32) -> bool {
    if (chan_id as usize) >= MAX_DMA_CHANNELS {
        return false;
    }
    let mut channels = DMA_CHANNELS.lock();
    let ch = &mut channels[chan_id as usize];
    if !ch.active {
        return false;
    }
    // Stub: DMA is always instantly complete.
    ch.busy = false;
    true
}

// ---------------------------------------------------------------------------
// Cache coherency stubs
// ---------------------------------------------------------------------------

/// Flush CPU caches before a device read (TO_DEVICE direction).
///
/// x86-64 has hardware-coherent DMA — this is a no-op.
#[inline(always)]
pub fn dma_sync_for_device(_phys_addr: DmaAddr, _size: usize, _dir: u8) {
    // No-op: x86-64 DMA is cache-coherent.
}

/// Invalidate CPU caches after a device write (FROM_DEVICE direction).
///
/// x86-64 has hardware-coherent DMA — this is a no-op.
#[inline(always)]
pub fn dma_sync_for_cpu(_phys_addr: DmaAddr, _size: usize, _dir: u8) {
    // No-op: x86-64 DMA is cache-coherent.
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the DMA subsystem.  Clears all channel and mapping tables.
pub fn init() {
    {
        let mut mappings = DMA_MAPPINGS.lock();
        for slot in mappings.iter_mut() {
            *slot = DmaMapping::empty();
        }
    }
    {
        let mut channels = DMA_CHANNELS.lock();
        for (idx, ch) in channels.iter_mut().enumerate() {
            *ch = DmaChannel::empty();
            ch.id = idx as u32;
        }
    }
    crate::serial_println!(
        "  [dma] subsystem initialized ({} channels, {} mapping slots)",
        MAX_DMA_CHANNELS,
        MAX_DMA_MAPPINGS
    );
}
