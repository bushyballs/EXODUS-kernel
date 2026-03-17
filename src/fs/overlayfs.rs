/// overlayfs -- union mount filesystem for Genesis
///
/// Layers a read-write "upper" directory on top of one or more read-only
/// "lower" directories.  Reads fall through to lower layers when the upper
/// does not contain the file.  Writes are always directed to the upper layer.
/// Copy-up: when a lower-layer file is opened for writing it is first copied
/// (as a stub) to the upper layer.
///
/// Design:
///   All state lives in fixed-size static arrays guarded by a Mutex.
///   No heap allocation is used anywhere (no Vec, Box, String, alloc::*).
///
/// SAFETY RULES (must never be violated):
///   - NO as f32 / as f64
///   - NO Vec, Box, String, alloc::*
///   - NO unwrap(), expect(), panic!()
///   - saturating_add / saturating_sub for counters
///   - wrapping_add for sequence numbers
///   - read_volatile / write_volatile for all MMIO
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

pub const MAX_OVERLAY_MOUNTS: usize = 4;
pub const MAX_OVERLAY_LAYERS: usize = 8; // max lower layers per mount

// ============================================================================
// OverlayLayerType
// ============================================================================

#[derive(Copy, Clone, PartialEq)]
pub enum OverlayLayerType {
    Lower, // read-only
    Upper, // read-write
    Work,  // work dir (needed by overlayfs for atomicity)
}

// ============================================================================
// OverlayLayer
// ============================================================================

/// One layer within an overlay mount.
#[derive(Copy, Clone)]
pub struct OverlayLayer {
    pub fs_type: u8,    // 1=tmpfs, 2=ext2, 3=xfs, 4=btrfs
    pub mount_idx: u32, // index into that filesystem's mount table
    pub layer_type: OverlayLayerType,
    pub active: bool,
}

impl OverlayLayer {
    pub const fn empty() -> Self {
        OverlayLayer {
            fs_type: 0,
            mount_idx: 0,
            layer_type: OverlayLayerType::Lower,
            active: false,
        }
    }
}

// ============================================================================
// OverlayMount
// ============================================================================

/// One overlay mount combining an upper layer with zero or more lower layers.
#[derive(Copy, Clone)]
pub struct OverlayMount {
    pub layers: [OverlayLayer; MAX_OVERLAY_LAYERS],
    pub nlayers: u8,
    pub upper_idx: u8, // which layer slot holds the upper (writable) layer
    pub active: bool,
}

impl OverlayMount {
    pub const fn empty() -> Self {
        OverlayMount {
            layers: [OverlayLayer::empty(); MAX_OVERLAY_LAYERS],
            nlayers: 0,
            upper_idx: 0,
            active: false,
        }
    }
}

// ============================================================================
// Static storage
// ============================================================================

static OVERLAY_MOUNTS: Mutex<[OverlayMount; MAX_OVERLAY_MOUNTS]> =
    Mutex::new([OverlayMount::empty(); MAX_OVERLAY_MOUNTS]);

// ============================================================================
// Public API
// ============================================================================

/// Create an overlay mount with the specified upper (writable) layer.
/// Returns the mount index, or None if the table is full.
pub fn overlay_create(upper_fs: u8, upper_mount: u32) -> Option<u32> {
    let mut mounts = OVERLAY_MOUNTS.lock();

    let mut slot = MAX_OVERLAY_MOUNTS;
    let mut i = 0usize;
    while i < MAX_OVERLAY_MOUNTS {
        if !mounts[i].active {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == MAX_OVERLAY_MOUNTS {
        return None;
    }

    let mut mount = OverlayMount::empty();

    // Layer 0 = upper
    mount.layers[0] = OverlayLayer {
        fs_type: upper_fs,
        mount_idx: upper_mount,
        layer_type: OverlayLayerType::Upper,
        active: true,
    };
    mount.nlayers = 1;
    mount.upper_idx = 0;
    mount.active = true;

    mounts[slot] = mount;
    Some(slot as u32)
}

/// Add a lower (read-only) layer to an existing overlay mount.
/// Lower layers are consulted in the order they are added (most recently
/// added = checked last in the current implementation).
/// Returns false if the mount index is invalid or the layer table is full.
pub fn overlay_add_lower(overlay_idx: u32, lower_fs: u8, lower_mount: u32) -> bool {
    let idx = overlay_idx as usize;
    if idx >= MAX_OVERLAY_MOUNTS {
        return false;
    }

    let mut mounts = OVERLAY_MOUNTS.lock();
    if !mounts[idx].active {
        return false;
    }

    let nlayers = mounts[idx].nlayers as usize;
    if nlayers >= MAX_OVERLAY_LAYERS {
        return false;
    }

    mounts[idx].layers[nlayers] = OverlayLayer {
        fs_type: lower_fs,
        mount_idx: lower_mount,
        layer_type: OverlayLayerType::Lower,
        active: true,
    };
    mounts[idx].nlayers = (nlayers as u8).saturating_add(1);

    true
}

/// Destroy an overlay mount.
/// Returns false if the index is invalid or not active.
pub fn overlay_destroy(overlay_idx: u32) -> bool {
    let idx = overlay_idx as usize;
    if idx >= MAX_OVERLAY_MOUNTS {
        return false;
    }

    let mut mounts = OVERLAY_MOUNTS.lock();
    if !mounts[idx].active {
        return false;
    }

    mounts[idx] = OverlayMount::empty();
    true
}

/// Look up a path in the overlay mount.
/// Returns (fs_type, mount_idx, ino) of the layer that owns the path.
///
/// Strategy:
///   1. Check the upper layer first.
///   2. If not found in upper, check lower layers in order (most recently
///      added first, i.e., highest layer index first among lower layers).
///
/// Stub: always returns the upper layer if it exists and is active.
/// A real implementation would delegate to each layer's own lookup.
pub fn overlay_lookup(overlay_idx: u32, _path: &[u8]) -> Option<(u8, u32, u64)> {
    let idx = overlay_idx as usize;
    if idx >= MAX_OVERLAY_MOUNTS {
        return None;
    }

    let mounts = OVERLAY_MOUNTS.lock();
    if !mounts[idx].active {
        return None;
    }

    let upper_slot = mounts[idx].upper_idx as usize;
    let upper = &mounts[idx].layers[upper_slot];
    if upper.active {
        // Stub: return upper layer with ino=0 (real impl would call into
        // the target filesystem's lookup function)
        return Some((upper.fs_type, upper.mount_idx, 0));
    }

    // Fall through to lower layers (most recently added = highest index)
    let nlayers = mounts[idx].nlayers as usize;
    if nlayers == 0 {
        return None;
    }
    let mut layer_idx = nlayers.saturating_sub(1);
    loop {
        let layer = &mounts[idx].layers[layer_idx];
        if layer.active && layer.layer_type == OverlayLayerType::Lower {
            return Some((layer.fs_type, layer.mount_idx, 0));
        }
        if layer_idx == 0 {
            break;
        }
        layer_idx = layer_idx.saturating_sub(1);
    }

    None
}

/// Copy-up: bring a file from a lower layer into the upper layer so it can
/// be written.  Returns the inode number of the new upper-layer file.
///
/// Stub: creates an empty file in the upper layer using the same conceptual
/// inode number as the lower layer file (passed as `lower_ino`).
/// A real implementation would copy the file content via block I/O.
pub fn overlay_open_write(
    overlay_idx: u32,
    lower_ino: u64,
    _lower_fs: u8,
    _lower_mount: u32,
) -> Option<u64> {
    let idx = overlay_idx as usize;
    if idx >= MAX_OVERLAY_MOUNTS {
        return None;
    }

    let mounts = OVERLAY_MOUNTS.lock();
    if !mounts[idx].active {
        return None;
    }

    let upper_slot = mounts[idx].upper_idx as usize;
    if !mounts[idx].layers[upper_slot].active {
        return None;
    }

    // Stub: return the lower_ino as the "new" upper-layer ino.
    // A real implementation would call tmpfs_create (or the appropriate
    // upper-layer fs) and copy data from the lower file.
    Some(lower_ino)
}

// ============================================================================
// init
// ============================================================================

/// Initialize the overlayfs subsystem.
pub fn init() {
    serial_println!("[overlayfs] overlay filesystem initialized");
}
