/// FAT32 filesystem driver for Genesis — no-heap, static-buffer implementation
///
/// Read-only FAT32 support for accessing EFI System Partitions and
/// USB drives formatted with FAT32.
///
/// All state lives in static arrays guarded by a Mutex.  No Vec, Box,
/// String, Arc, or any other alloc:: type is used.
///
/// FAT32 on-disk layout:
///   Sector 0:              Boot sector (BIOS Parameter Block)
///   Sectors 1..reserved-1: FSInfo, backup boot sector
///   FAT region:            File Allocation Table (cluster chains)
///   Data region:           Actual file / directory data in clusters
///
/// Inspired by: Microsoft FAT32 specification, Linux vfat driver.
/// All code is original.
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
// FAT32 attribute flags
// ============================================================================

pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME_ID: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_LONG_NAME: u8 = 0x0F;

// ============================================================================
// FAT32 cluster sentinel values
// ============================================================================

/// Any value >= FAT32_EOC marks end-of-chain.
pub const FAT32_EOC: u32 = 0x0FFF_FFF8;
/// Bad cluster marker.
pub const FAT32_BAD: u32 = 0x0FFF_FFF7;
/// Free cluster.
pub const FAT32_FREE: u32 = 0x0000_0000;

// ============================================================================
// On-disk structures
// ============================================================================

/// BIOS Parameter Block — first 90 bytes of the FAT32 boot sector.
///
/// All fields are little-endian.  The struct is `repr(C, packed)` so it can be
/// cast directly from a raw sector buffer.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct Fat32Bpb {
    pub jump_boot: [u8; 3],
    pub oem_name: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_entry_count: u16, // 0 for FAT32
    pub total_sectors_16: u16, // 0 for FAT32
    pub media: u8,
    pub fat_size_16: u16, // 0 for FAT32
    pub sectors_per_track: u16,
    pub num_heads: u16,
    pub hidden_sectors: u32,
    pub total_sectors_32: u32,
    // --- FAT32 extended BPB (offset 36) ---
    pub fat_size_32: u32,
    pub ext_flags: u16,
    pub fs_version: u16,
    pub root_cluster: u32,
    pub fs_info: u16,
    pub backup_boot_sector: u16,
    pub reserved: [u8; 12],
    pub drive_number: u8,
    pub reserved1: u8,
    pub boot_sig: u8,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
    pub fs_type: [u8; 8],
}

impl Fat32Bpb {
    const fn zeroed() -> Self {
        Fat32Bpb {
            jump_boot: [0u8; 3],
            oem_name: [0u8; 8],
            bytes_per_sector: 0,
            sectors_per_cluster: 0,
            reserved_sectors: 0,
            num_fats: 0,
            root_entry_count: 0,
            total_sectors_16: 0,
            media: 0,
            fat_size_16: 0,
            sectors_per_track: 0,
            num_heads: 0,
            hidden_sectors: 0,
            total_sectors_32: 0,
            fat_size_32: 0,
            ext_flags: 0,
            fs_version: 0,
            root_cluster: 0,
            fs_info: 0,
            backup_boot_sector: 0,
            reserved: [0u8; 12],
            drive_number: 0,
            reserved1: 0,
            boot_sig: 0,
            volume_id: 0,
            volume_label: [0u8; 11],
            fs_type: [0u8; 8],
        }
    }
}

/// FAT32 32-byte directory entry (short name / standard entry).
///
/// The `name` field stores the 8.3 filename in 11 bytes (no dot separator):
/// bytes 0-7 = base name padded with spaces, bytes 8-10 = extension padded
/// with spaces.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct Fat32DirEntry {
    pub name: [u8; 11], // 8.3 format, space-padded
    pub attr: u8,
    pub nt_reserved: u8,
    pub crt_time_tenth: u8,
    pub crt_time: u16,
    pub crt_date: u16,
    pub lst_acc_date: u16,
    pub fst_clus_hi: u16,
    pub wrt_time: u16,
    pub wrt_date: u16,
    pub fst_clus_lo: u16,
    pub file_size: u32,
}

impl Fat32DirEntry {
    /// Combine the high and low words of the first cluster number.
    #[inline]
    pub fn first_cluster(&self) -> u32 {
        // Read packed fields into locals before arithmetic to avoid
        // unaligned access UB on packed structs.
        let hi = self.fst_clus_hi;
        let lo = self.fst_clus_lo;
        ((hi as u32) << 16) | (lo as u32)
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }

    #[inline]
    pub fn is_volume_label(&self) -> bool {
        self.attr & ATTR_VOLUME_ID != 0
    }

    #[inline]
    pub fn is_long_name(&self) -> bool {
        self.attr == ATTR_LONG_NAME
    }

    /// Entry has been deleted (0xE5) or is past the last entry (0x00).
    #[inline]
    pub fn is_free(&self) -> bool {
        self.name[0] == 0xE5 || self.name[0] == 0x00
    }

    /// Entry has name[0] == 0x00, signalling no more entries follow.
    #[inline]
    pub fn is_end(&self) -> bool {
        self.name[0] == 0x00
    }

    /// Compare this entry's 11-byte 8.3 name against `name83`.
    #[inline]
    pub fn name_matches(&self, name83: &[u8; 11]) -> bool {
        self.name == *name83
    }

    const fn zeroed() -> Self {
        Fat32DirEntry {
            name: [0u8; 11],
            attr: 0,
            nt_reserved: 0,
            crt_time_tenth: 0,
            crt_time: 0,
            crt_date: 0,
            lst_acc_date: 0,
            fst_clus_hi: 0,
            wrt_time: 0,
            wrt_date: 0,
            fst_clus_lo: 0,
            file_size: 0,
        }
    }
}

// ============================================================================
// Per-volume runtime state
// ============================================================================

/// Runtime state for a single mounted FAT32 volume.
///
/// All data lives in this fixed-size struct — no heap allocations.
/// The `cache_buf` holds one cluster (up to 4096 bytes = 8 × 512-byte
/// sectors at the most common cluster sizes).
pub struct Fat32Volume {
    /// True when this slot holds a mounted volume.
    pub active: bool,
    bpb: Fat32Bpb,
    /// LBA of the first FAT sector (= BPB.reserved_sectors).
    fat_start_sector: u32,
    /// LBA of the first data sector (= fat_start + num_fats * fat_size_32).
    data_start_sector: u32,
    /// Cluster number of the root directory (= BPB.root_cluster).
    root_dir_cluster: u32,
    /// Sectors per cluster (copied from BPB for quick access).
    sectors_per_cluster: u32,
    /// Bytes per cluster (= sectors_per_cluster * 512, capped at 4096).
    bytes_per_cluster: u32,
    /// Total usable data clusters.
    total_clusters: u32,
    /// Index into the block device table (0 = first virtio-blk / ATA drive).
    device_idx: u32,
    /// Cluster currently held in `cache_buf`, or 0xFFFF_FFFF if invalid.
    cache_cluster: u32,
    /// True when `cache_buf` contains valid data for `cache_cluster`.
    cache_valid: bool,
    /// One-cluster buffer (up to 4 KiB).
    cache_buf: [u8; 4096],
}

impl Fat32Volume {
    pub const fn empty() -> Self {
        Fat32Volume {
            active: false,
            bpb: Fat32Bpb::zeroed(),
            fat_start_sector: 0,
            data_start_sector: 0,
            root_dir_cluster: 0,
            sectors_per_cluster: 0,
            bytes_per_cluster: 0,
            total_clusters: 0,
            device_idx: 0,
            cache_cluster: 0xFFFF_FFFF,
            cache_valid: false,
            cache_buf: [0u8; 4096],
        }
    }
}

// Safety: Fat32Volume is only accessed under FAT32_VOLUMES Mutex.
unsafe impl Send for Fat32Volume {}

// ============================================================================
// Global volume table
// ============================================================================

/// Up to 4 simultaneously mounted FAT32 volumes.
static FAT32_VOLUMES: Mutex<[Fat32Volume; 4]> = Mutex::new([
    Fat32Volume::empty(),
    Fat32Volume::empty(),
    Fat32Volume::empty(),
    Fat32Volume::empty(),
]);

// ============================================================================
// Block device stub
// ============================================================================

/// Read one 512-byte sector from block device `device_idx` into `buf`.
///
/// Delegates to `virtio_blk_read`.  Returns `false` if no device is
/// available or the read fails.
fn read_sector(device_idx: u32, sector: u32, buf: &mut [u8; 512]) -> bool {
    // Currently only device index 0 is supported (the first virtio-blk device).
    // Ignore the index for forward compatibility but log if > 0.
    let _ = device_idx; // suppress unused warning; extend later for multi-device

    if crate::drivers::virtio_blk::virtio_blk_read(sector as u64, buf) {
        return true;
    }

    serial_println!(
        "  FAT32: no block device (device_idx={}, sector={})",
        device_idx,
        sector
    );
    false
}

// ============================================================================
// Public API
// ============================================================================

/// Mount a FAT32 volume from block device `device_idx` into volume slot `vol_idx`.
///
/// Reads sector 0, validates the BPB, and derives all geometry fields.
/// Returns `true` on success.
pub fn fat32_mount(device_idx: u32, vol_idx: usize) -> bool {
    if vol_idx >= 4 {
        serial_println!("  FAT32: vol_idx {} out of range", vol_idx);
        return false;
    }

    let mut sector_buf = [0u8; 512];
    if !read_sector(device_idx, 0, &mut sector_buf) {
        serial_println!("  FAT32: cannot read boot sector (device {})", device_idx);
        return false;
    }

    // Cast the raw bytes to a BPB.
    // Safety: Fat32Bpb is repr(C, packed); any byte pattern is valid.
    let bpb: Fat32Bpb =
        unsafe { core::ptr::read_unaligned(sector_buf.as_ptr() as *const Fat32Bpb) };

    // --- Validate BPB ---
    let bps = bpb.bytes_per_sector;
    if bps != 512 {
        serial_println!("  FAT32: bytes_per_sector={} (want 512)", bps);
        return false;
    }
    if bpb.sectors_per_cluster == 0 {
        serial_println!("  FAT32: sectors_per_cluster=0");
        return false;
    }
    if bpb.fat_size_32 == 0 {
        serial_println!("  FAT32: fat_size_32=0 (not FAT32?)");
        return false;
    }
    if bpb.num_fats == 0 {
        serial_println!("  FAT32: num_fats=0");
        return false;
    }
    // fs_type field should start with "FAT32" for spec-compliant volumes;
    // some BPBs omit it so we treat this as a soft warning only.
    if &bpb.fs_type[..5] != b"FAT32" {
        serial_println!("  FAT32: fs_type field unexpected (continuing anyway)");
    }

    // --- Derive geometry ---
    let reserved = bpb.reserved_sectors as u32;
    let fat_size = bpb.fat_size_32;
    let num_fats = bpb.num_fats as u32;

    let fat_start_sector = reserved;
    let fat_total_sectors = num_fats.saturating_mul(fat_size);
    let data_start_sector = fat_start_sector.saturating_add(fat_total_sectors);

    let spc = bpb.sectors_per_cluster as u32;
    // Cluster size in bytes; never exceed 4096 (our cache buffer size).
    let bytes_per_cluster = spc.saturating_mul(512).min(4096);

    // Total clusters = (total_sectors - data_start) / spc
    let total_sectors = bpb.total_sectors_32;
    let data_sectors = total_sectors.saturating_sub(data_start_sector);
    let total_clusters = if spc > 0 { data_sectors / spc } else { 0 };

    let root_dir_cluster = bpb.root_cluster;

    // --- Write into volume table ---
    {
        let mut vols = FAT32_VOLUMES.lock();
        let vol = &mut vols[vol_idx];
        vol.active = true;
        vol.bpb = bpb;
        vol.fat_start_sector = fat_start_sector;
        vol.data_start_sector = data_start_sector;
        vol.root_dir_cluster = root_dir_cluster;
        vol.sectors_per_cluster = spc;
        vol.bytes_per_cluster = bytes_per_cluster;
        vol.total_clusters = total_clusters;
        vol.device_idx = device_idx;
        vol.cache_cluster = 0xFFFF_FFFF;
        vol.cache_valid = false;
    }

    serial_println!(
        "  FAT32: vol[{}] mounted: device={}, root_clus={}, data_start={}, clusters={}",
        vol_idx,
        device_idx,
        root_dir_cluster,
        data_start_sector,
        total_clusters
    );
    true
}

/// Read the FAT32 FAT entry for `cluster`.
///
/// Returns the next cluster value (masked to 28 bits), or 0 on error.
pub fn fat32_read_fat(vol: &mut Fat32Volume, cluster: u32) -> u32 {
    if !vol.active {
        return 0;
    }
    // Each FAT32 entry is 4 bytes.
    // Avoid overflow: cluster is at most ~2^28, so cluster*4 fits in u32.
    let fat_offset = cluster.saturating_mul(4);
    let fat_sector = vol.fat_start_sector.saturating_add(fat_offset / 512);
    let byte_off = (fat_offset % 512) as usize;

    let mut sector_buf = [0u8; 512];
    if !read_sector(vol.device_idx, fat_sector, &mut sector_buf) {
        return 0;
    }
    // Bounds: byte_off is 0..=508 (fat_offset%512), so byte_off+4 <= 512.
    if byte_off.saturating_add(4) > 512 {
        return 0;
    }
    let raw = u32::from_le_bytes([
        sector_buf[byte_off],
        sector_buf[byte_off.saturating_add(1)],
        sector_buf[byte_off.saturating_add(2)],
        sector_buf[byte_off.saturating_add(3)],
    ]);
    raw & 0x0FFF_FFFF
}

/// Follow the FAT chain one step: return `Some(next_cluster)` if the chain
/// continues, or `None` if the cluster is end-of-chain or bad.
pub fn fat32_next_cluster(vol: &mut Fat32Volume, cluster: u32) -> Option<u32> {
    let next = fat32_read_fat(vol, cluster);
    if next >= FAT32_EOC || next == FAT32_BAD || next == FAT32_FREE {
        None
    } else {
        Some(next)
    }
}

/// Convert a cluster number to its starting LBA sector.
///
/// Clusters are numbered from 2; cluster 2 starts at `data_start_sector`.
pub fn fat32_cluster_to_sector(vol: &Fat32Volume, cluster: u32) -> u32 {
    let offset = cluster.saturating_sub(2);
    vol.data_start_sector
        .saturating_add(offset.saturating_mul(vol.sectors_per_cluster))
}

/// Read one cluster into `buf`.
///
/// Uses a single-cluster cache.  If `cache_cluster == cluster` the cached
/// data is copied directly without hitting the disk.
///
/// `buf` must be exactly 4096 bytes; only `bytes_per_cluster` bytes are
/// meaningful on return.
pub fn fat32_read_cluster(vol: &mut Fat32Volume, cluster: u32, buf: &mut [u8; 4096]) -> bool {
    if !vol.active {
        return false;
    }

    // Cache hit?
    if vol.cache_valid && vol.cache_cluster == cluster {
        buf.copy_from_slice(&vol.cache_buf);
        return true;
    }

    let base_sector = fat32_cluster_to_sector(vol, cluster);
    let sectors = vol.sectors_per_cluster.min(8); // cap at 8 × 512 = 4096

    // Zero-fill the output buffer first so padding bytes are deterministic.
    for b in buf.iter_mut() {
        *b = 0;
    }

    let mut ok = true;
    let mut i = 0u32;
    while i < sectors {
        let sector = base_sector.saturating_add(i);
        // Each sector fills 512 bytes inside buf.
        let byte_start = (i as usize).saturating_mul(512);
        let byte_end = byte_start.saturating_add(512);
        if byte_end > 4096 {
            break;
        }
        let mut tmp = [0u8; 512];
        if !read_sector(vol.device_idx, sector, &mut tmp) {
            ok = false;
            break;
        }
        buf[byte_start..byte_end].copy_from_slice(&tmp);
        i = i.saturating_add(1);
    }

    if ok {
        // Update cache.
        vol.cache_cluster = cluster;
        vol.cache_valid = true;
        vol.cache_buf.copy_from_slice(buf);
    }

    ok
}

/// Scan directory `dir_cluster` for an 8.3 entry matching `name83`.
///
/// Skips deleted, volume-label, and long-name entries.
/// Returns `Some(Fat32DirEntry)` on the first match.
pub fn fat32_find_entry(
    vol: &mut Fat32Volume,
    dir_cluster: u32,
    name83: &[u8; 11],
) -> Option<Fat32DirEntry> {
    let mut cluster = dir_cluster;
    let entry_size = core::mem::size_of::<Fat32DirEntry>(); // always 32

    loop {
        let mut buf = [0u8; 4096];
        if !fat32_read_cluster(vol, cluster, &mut buf) {
            return None;
        }

        let valid_bytes = vol.bytes_per_cluster as usize;
        let mut off = 0usize;
        while off.saturating_add(entry_size) <= valid_bytes {
            // Safety: off + entry_size <= valid_bytes <= buf.len() = 4096.
            let entry: Fat32DirEntry =
                unsafe { core::ptr::read_unaligned(buf.as_ptr().add(off) as *const Fat32DirEntry) };

            if entry.is_end() {
                return None;
            }
            if !entry.is_free()
                && !entry.is_long_name()
                && !entry.is_volume_label()
                && entry.name_matches(name83)
            {
                return Some(entry);
            }

            off = off.saturating_add(entry_size);
        }

        // Advance cluster chain.
        match fat32_next_cluster(vol, cluster) {
            Some(next) => cluster = next,
            None => return None,
        }
    }
}

/// Convert a filename component (e.g. `"FILE.TXT"`) to an 8.3 name byte array.
///
/// Rules:
///   - Uppercase all ASCII letters.
///   - Name part padded to 8 bytes with spaces.
///   - Extension part padded to 3 bytes with spaces.
///   - A trailing dot (e.g. `"FOO."`) gives an empty extension.
pub fn name_to_83(name: &[u8], out: &mut [u8; 11]) {
    // Fill with spaces first.
    for b in out.iter_mut() {
        *b = b' ';
    }

    // Find the last dot, skipping a leading dot (hidden files like ".foo").
    let dot_pos: Option<usize> = {
        let mut found = None;
        let start = if name.first() == Some(&b'.') { 1 } else { 0 };
        let mut i = start;
        while i < name.len() {
            if name[i] == b'.' {
                found = Some(i);
            }
            i = i.saturating_add(1);
        }
        found
    };

    match dot_pos {
        Some(dp) => {
            // Name part: up to 8 bytes.
            let name_slice = &name[..dp];
            let n = name_slice.len().min(8);
            let mut i = 0usize;
            while i < n {
                out[i] = ascii_upper(name_slice[i]);
                i = i.saturating_add(1);
            }
            // Extension part: up to 3 bytes.
            let ext_start = dp.saturating_add(1);
            let ext_slice = if ext_start < name.len() {
                &name[ext_start..]
            } else {
                &[]
            };
            let e = ext_slice.len().min(3);
            let mut j = 0usize;
            while j < e {
                out[8usize.saturating_add(j)] = ascii_upper(ext_slice[j]);
                j = j.saturating_add(1);
            }
        }
        None => {
            // No dot: entire name goes in the base part, no extension.
            let n = name.len().min(8);
            let mut i = 0usize;
            while i < n {
                out[i] = ascii_upper(name[i]);
                i = i.saturating_add(1);
            }
        }
    }
}

/// ASCII uppercase (only modifies lowercase a-z; other bytes pass through).
#[inline(always)]
fn ascii_upper(b: u8) -> u8 {
    if b >= b'a' && b <= b'z' {
        b.wrapping_sub(32)
    } else {
        b
    }
}

/// Resolve a path like `b"/EFI/BOOT/BOOTx64.EFI"` to its directory entry.
///
/// - Leading `/` is ignored.
/// - Empty components are skipped.
/// - Path components are converted to 8.3 names before lookup.
/// - Returns `None` if any component is not found.
pub fn fat32_path_to_entry(vol: &mut Fat32Volume, path: &[u8]) -> Option<Fat32DirEntry> {
    if !vol.active {
        return None;
    }

    let mut cluster = vol.root_dir_cluster;
    let mut name83 = [b' '; 11];

    // Split path on b'/' and walk the tree.
    let mut start = 0usize;
    // Skip a leading '/'.
    if path.first() == Some(&b'/') {
        start = 1;
    }

    // Collect non-empty components into a small fixed array (max 32 levels).
    let mut components: [&[u8]; 32] = [&[]; 32];
    let mut comp_count = 0usize;
    {
        let mut i = start;
        let mut seg_start = i;
        while i <= path.len() {
            let at_end = i == path.len();
            let at_slash = !at_end && path[i] == b'/';
            if at_end || at_slash {
                if i > seg_start {
                    if comp_count < 32 {
                        components[comp_count] = &path[seg_start..i];
                        comp_count = comp_count.saturating_add(1);
                    }
                }
                seg_start = i.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
    }

    if comp_count == 0 {
        // Root directory itself — return a synthetic entry.
        let mut root_entry = Fat32DirEntry::zeroed();
        root_entry.attr = ATTR_DIRECTORY;
        let hi = (cluster >> 16) as u16;
        let lo = cluster as u16;
        root_entry.fst_clus_hi = hi;
        root_entry.fst_clus_lo = lo;
        return Some(root_entry);
    }

    let mut comp_idx = 0usize;
    while comp_idx < comp_count {
        let comp = components[comp_idx];
        name_to_83(comp, &mut name83);

        let entry = fat32_find_entry(vol, cluster, &name83)?;

        if comp_idx.saturating_add(1) == comp_count {
            // Last component — this is the target.
            return Some(entry);
        } else {
            // Intermediate — must be a directory.
            if !entry.is_dir() {
                return None;
            }
            cluster = entry.first_cluster();
            // FAT32 root cluster 0 is a special case in some older BPBs.
            if cluster == 0 {
                cluster = vol.root_dir_cluster;
            }
        }

        comp_idx = comp_idx.saturating_add(1);
    }

    None
}

/// Read up to `max` bytes of file at `path` from volume `vol_idx` into `buf`.
///
/// Returns the number of bytes actually read, or -1 on error.
pub fn fat32_read_file(vol_idx: usize, path: &[u8], buf: &mut [u8], max: usize) -> isize {
    if vol_idx >= 4 || max == 0 || buf.len() < max {
        return -1;
    }

    // We need a mutable reference to the volume, but `FAT32_VOLUMES` is a
    // Mutex-guarded array of non-Copy structs.  Lock, do all work, release.
    let mut vols = FAT32_VOLUMES.lock();
    let vol = &mut vols[vol_idx];
    if !vol.active {
        return -1;
    }

    let entry = match fat32_path_to_entry(vol, path) {
        Some(e) => e,
        None => return -1,
    };
    if entry.is_dir() {
        return -1;
    }

    let file_size = entry.file_size as usize;
    let to_read = max.min(file_size);
    let mut cluster = entry.first_cluster();
    let bpc = vol.bytes_per_cluster as usize;
    let mut written = 0usize;

    loop {
        if written >= to_read {
            break;
        }
        if cluster == 0 || cluster >= FAT32_EOC {
            break;
        }

        let mut cluster_buf = [0u8; 4096];
        if !fat32_read_cluster(vol, cluster, &mut cluster_buf) {
            return -1;
        }

        // How many bytes of this cluster to copy?
        let remaining = to_read.saturating_sub(written);
        let chunk = remaining.min(bpc);
        // Clamp to actual buffer extent (bpc <= 4096 always, but be safe).
        let chunk_clamped = chunk.min(4096);

        buf[written..written.saturating_add(chunk_clamped)]
            .copy_from_slice(&cluster_buf[..chunk_clamped]);
        written = written.saturating_add(chunk_clamped);

        match fat32_next_cluster(vol, cluster) {
            Some(next) => cluster = next,
            None => break,
        }
    }

    written as isize
}

/// Fill `out` with up to 64 non-free, non-LFN directory entries from `path`.
///
/// Returns the number of entries written into `out`.
pub fn fat32_list_dir(vol_idx: usize, path: &[u8], out: &mut [Fat32DirEntry; 64]) -> u32 {
    if vol_idx >= 4 {
        return 0;
    }

    let mut vols = FAT32_VOLUMES.lock();
    let vol = &mut vols[vol_idx];
    if !vol.active {
        return 0;
    }

    // Resolve the directory.
    let dir_entry = match fat32_path_to_entry(vol, path) {
        Some(e) => e,
        None => return 0,
    };
    let mut cluster = if dir_entry.is_dir() {
        let c = dir_entry.first_cluster();
        if c == 0 {
            vol.root_dir_cluster
        } else {
            c
        }
    } else {
        return 0;
    };

    let entry_size = core::mem::size_of::<Fat32DirEntry>(); // 32
    let mut count = 0u32;

    loop {
        let bpc = vol.bytes_per_cluster as usize;
        let mut cluster_buf = [0u8; 4096];
        if !fat32_read_cluster(vol, cluster, &mut cluster_buf) {
            break;
        }

        let valid = bpc.min(4096);
        let mut off = 0usize;
        while off.saturating_add(entry_size) <= valid {
            let entry: Fat32DirEntry = unsafe {
                core::ptr::read_unaligned(cluster_buf.as_ptr().add(off) as *const Fat32DirEntry)
            };

            if entry.is_end() {
                return count;
            }
            if !entry.is_free() && !entry.is_long_name() && !entry.is_volume_label() {
                if (count as usize) < 64 {
                    out[count as usize] = entry;
                    count = count.saturating_add(1);
                }
            }

            off = off.saturating_add(entry_size);
        }

        match fat32_next_cluster(vol, cluster) {
            Some(next) => cluster = next,
            None => break,
        }
    }

    count
}

/// Return the file size of the entry at `path`, or `None` if not found.
pub fn fat32_file_size(vol_idx: usize, path: &[u8]) -> Option<u32> {
    if vol_idx >= 4 {
        return None;
    }

    let mut vols = FAT32_VOLUMES.lock();
    let vol = &mut vols[vol_idx];
    if !vol.active {
        return None;
    }

    let entry = fat32_path_to_entry(vol, path)?;
    if entry.is_dir() {
        return None;
    }
    Some(entry.file_size)
}

// ============================================================================
// Module initialisation
// ============================================================================

/// Initialise the FAT32 driver subsystem.
///
/// This only logs readiness.  Actual mounting is performed by `fat32_mount`
/// when called from the boot sequence (or from `fs::init`).
pub fn init() {
    serial_println!("  FAT32 driver ready");
}
