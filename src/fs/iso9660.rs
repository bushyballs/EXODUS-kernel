/// ISO 9660 (ECMA-119) CD-ROM filesystem driver
///
/// Read-only, no-heap implementation.  All state lives in static arrays
/// guarded by Mutex.  No Vec, Box, String, or any alloc:: type is used.
///
/// Reads 2048-byte sectors via virtio_blk (4 × 512-byte sub-reads).
/// Supports up to 2 mounted volumes simultaneously.
///
/// On-disk layout (spec §6.7):
///   Sector 0-15:  System area (ignored)
///   Sector 16:    Primary Volume Descriptor (PVD)
///   Sector 17+:   More Volume Descriptors (terminated by VD Set Terminator)
///   …             Path tables, directory extents, file extents
///
/// SAFETY RULES (must never be violated):
///   - NO as f32 / as f64
///   - NO Vec, Box, String, alloc::*
///   - NO unwrap(), expect(), panic!()
///   - saturating_add / saturating_sub for all arithmetic that could overflow
///   - wrapping_add for ring/sequence indices
///   - read_volatile / write_volatile for MMIO
///   - All array accesses must be bounds-checked before indexing
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

/// Size of one CD-ROM logical sector in bytes.
pub const ISO_SECTOR_SIZE: usize = 2048;

/// LBA of the Primary Volume Descriptor.
const PVD_LBA: u32 = 16;

/// Expected magic identifier in the PVD (bytes 1-5).
const CD001: [u8; 5] = *b"CD001";

/// PVD type byte value.
const VD_TYPE_PVD: u8 = 1;
/// Volume Descriptor Set Terminator type byte.
const VD_TYPE_TERMINATOR: u8 = 255;

/// Bit 1 of file_flags set → this record describes a directory.
const DIR_FLAG_DIRECTORY: u8 = 0x02;

/// Maximum number of volumes we can mount simultaneously.
pub const MAX_VOLUMES: usize = 2;

/// Maximum depth we will walk during path resolution (prevents infinite loops).
const MAX_PATH_DEPTH: usize = 16;

/// Maximum number of directory sectors scanned per directory lookup.
const MAX_DIR_SECTORS: u32 = 256; // 256 × 2048 = 512 KiB — more than enough

// ============================================================================
// On-disk structures  (repr(C, packed) — cast directly from sector bytes)
// ============================================================================

/// ISO 9660 Primary Volume Descriptor — occupies one 2048-byte sector.
///
/// All multi-byte integer fields appear in both little-endian (LE) and
/// big-endian (BE) copies.  We always use the LE copy.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct Iso9660Pvd {
    /// Volume Descriptor type: 1 = PVD.
    pub vd_type: u8,
    /// Standard identifier: "CD001".
    pub id: [u8; 5],
    /// Volume Descriptor version: must be 1.
    pub version: u8,
    _unused1: u8,
    /// Name of the system (32 bytes, space-padded).
    pub system_id: [u8; 32],
    /// Name of the volume (32 bytes, space-padded).
    pub volume_id: [u8; 32],
    _unused2: [u8; 8],
    /// Total number of logical blocks on the volume (LE copy).
    pub volume_space_size_le: u32,
    /// Total number of logical blocks on the volume (BE copy).
    pub volume_space_size_be: u32,
    _unused3: [u8; 32],
    pub volume_set_size_le: u16,
    pub volume_set_size_be: u16,
    pub volume_seq_num_le: u16,
    pub volume_seq_num_be: u16,
    /// Logical block size in bytes, usually 2048 (LE copy).
    pub logical_block_size_le: u16,
    pub logical_block_size_be: u16,
    pub path_table_size_le: u32,
    pub path_table_size_be: u32,
    pub path_table_loc_le: u32,
    pub opt_path_table_loc_le: u32,
    pub path_table_loc_be: u32,
    pub opt_path_table_loc_be: u32,
    /// Embedded Directory Record for the root directory (34 bytes).
    pub root_dir_record: [u8; 34],
    /// Remaining PVD fields — not needed for basic operation.
    _rest: [u8; 1858],
}

// Compile-time size assertion — PVD must be exactly 2048 bytes.
const _: () = assert!(core::mem::size_of::<Iso9660Pvd>() == ISO_SECTOR_SIZE);

/// ISO 9660 Directory Record header (variable-length on disk; we only
/// interpret the fixed prefix and access the name via pointer arithmetic).
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct Iso9660DirRecord {
    /// Total length of this directory record in bytes (0 = end of sector pad).
    pub len: u8,
    /// Extended attribute record length (usually 0).
    pub ext_attr_len: u8,
    /// Logical Block Address of the extent (file data / sub-directory) — LE.
    pub loc_le: u32,
    /// Same, big-endian copy.
    pub loc_be: u32,
    /// Size of the extent in bytes — LE.
    pub data_len_le: u32,
    /// Same, big-endian copy.
    pub data_len_be: u32,
    /// Recording date/time: [years-since-1900, month, day, hour, min, sec, tz].
    pub recording_date: [u8; 7],
    /// File flags: bit 1 set = directory.
    pub file_flags: u8,
    pub file_unit_size: u8,
    pub interleave_gap: u8,
    pub vol_seq_num_le: u16,
    pub vol_seq_num_be: u16,
    /// Length of the file identifier (name) that immediately follows.
    pub name_len: u8,
    // File identifier (name) follows immediately after this struct in memory.
}

impl Iso9660DirRecord {
    /// Returns `true` if the FLAG_DIRECTORY bit is set.
    #[inline]
    pub fn is_dir(&self) -> bool {
        self.file_flags & DIR_FLAG_DIRECTORY != 0
    }

    /// Returns `true` if this is a "." (current directory) entry.
    /// ISO 9660 §9.1.11: name_len==1 and name byte == 0x00.
    #[inline]
    pub fn is_dot(&self) -> bool {
        self.name_len == 1
        // Caller checks the name byte via the sector buffer.
    }

    /// Returns `true` if this is a ".." (parent directory) entry.
    /// ISO 9660 §9.1.11: name_len==1 and name byte == 0x01.
    #[inline]
    pub fn is_dotdot(&self) -> bool {
        self.name_len == 1
        // Caller discriminates dot vs dotdot by reading the first name byte.
    }
}

// Minimum meaningful directory record size (header only, no name).
const DR_HEADER_SIZE: usize = core::mem::size_of::<Iso9660DirRecord>();

// ============================================================================
// Volume state
// ============================================================================

/// Runtime state for one mounted ISO 9660 volume.
pub struct Iso9660Volume {
    /// True when this slot holds a live mount.
    pub active: bool,
    /// Index into the virtio-blk device table (currently always 0).
    pub device_idx: u32,
    /// Logical block size (almost always 2048).
    pub block_size: u32,
    /// Total number of logical blocks on the volume.
    pub total_blocks: u32,
    /// LBA of the root directory extent.
    pub root_lba: u32,
    /// Size of the root directory extent in bytes.
    pub root_len: u32,
    /// Volume identifier copied from the PVD (space-padded, not NUL-terminated).
    pub volume_id: [u8; 32],

    // ---- Sector cache (one sector) -----------------------------------------
    /// LBA currently in the sector cache, valid when cache_valid == true.
    pub cache_lba: u32,
    /// Whether cache_buf contains valid data for cache_lba.
    pub cache_valid: bool,
    /// 2048-byte cache buffer for recently read sector.
    pub cache_buf: [u8; ISO_SECTOR_SIZE],

    // ---- Directory sector cache (one sector) --------------------------------
    /// LBA currently in the directory cache, valid when dir_cache_valid == true.
    pub dir_cache_lba: u32,
    /// Whether dir_cache_buf contains valid data for dir_cache_lba.
    pub dir_cache_valid: bool,
    /// 2048-byte cache buffer for the most recently read directory sector.
    pub dir_cache_buf: [u8; ISO_SECTOR_SIZE],
}

impl Iso9660Volume {
    /// Construct a zeroed (inactive) volume slot — used to initialise the
    /// static array.  Must be `const fn` so the array can live in BSS.
    pub const fn empty() -> Self {
        Iso9660Volume {
            active: false,
            device_idx: 0,
            block_size: 0,
            total_blocks: 0,
            root_lba: 0,
            root_len: 0,
            volume_id: [0u8; 32],
            cache_lba: 0,
            cache_valid: false,
            cache_buf: [0u8; ISO_SECTOR_SIZE],
            dir_cache_lba: 0,
            dir_cache_valid: false,
            dir_cache_buf: [0u8; ISO_SECTOR_SIZE],
        }
    }
}

/// Global array of ISO 9660 volume slots.
static ISO_VOLUMES: Mutex<[Iso9660Volume; MAX_VOLUMES]> =
    Mutex::new([Iso9660Volume::empty(), Iso9660Volume::empty()]);

// ============================================================================
// Block I/O
// ============================================================================

/// Read one 2048-byte ISO sector from a block device.
///
/// ISO 9660 sectors are 2048 bytes; virtio-blk uses 512-byte sub-sectors,
/// so we issue 4 consecutive reads (sub-sectors `lba*4 .. lba*4+3`).
///
/// Returns `true` on success, `false` on any sub-read failure.
fn read_sector(_device_idx: u32, lba: u32, buf: &mut [u8; ISO_SECTOR_SIZE]) -> bool {
    // Each ISO sector = 4 × 512-byte virtio sub-sectors.
    let base_sector: u64 = (lba as u64).saturating_mul(4);
    let mut tmp = [0u8; 512];

    for i in 0..4usize {
        if !crate::drivers::virtio_blk::virtio_blk_read(
            base_sector.saturating_add(i as u64),
            &mut tmp,
        ) {
            return false;
        }
        let dst_start = i.saturating_mul(512);
        let dst_end = dst_start.saturating_add(512);
        if dst_end > ISO_SECTOR_SIZE {
            return false; // should never happen
        }
        buf[dst_start..dst_end].copy_from_slice(&tmp);
    }
    true
}

// ============================================================================
// Volume mounting
// ============================================================================

/// Mount an ISO 9660 volume.
///
/// Reads sector 16 (the Primary Volume Descriptor), validates its signature,
/// and populates `ISO_VOLUMES[vol_idx]` with parsed metadata.
///
/// # Arguments
/// * `device_idx` — index of the virtio-blk device to read from.
/// * `vol_idx`    — slot in `ISO_VOLUMES` to populate (0 or 1).
///
/// Returns `true` on success.
pub fn iso9660_mount(device_idx: u32, vol_idx: usize) -> bool {
    if vol_idx >= MAX_VOLUMES {
        serial_println!("  iso9660: mount failed — vol_idx {} out of range", vol_idx);
        return false;
    }

    // Read the Primary Volume Descriptor.
    let mut sector_buf = [0u8; ISO_SECTOR_SIZE];
    if !read_sector(device_idx, PVD_LBA, &mut sector_buf) {
        serial_println!(
            "  iso9660: mount failed — cannot read PVD at LBA {}",
            PVD_LBA
        );
        return false;
    }

    // Validate PVD: type must be 1, id must be "CD001", version must be 1.
    if sector_buf[0] != VD_TYPE_PVD {
        serial_println!("  iso9660: PVD type byte {:#x} != 1", sector_buf[0]);
        return false;
    }
    if &sector_buf[1..6] != &CD001 {
        serial_println!("  iso9660: PVD magic mismatch");
        return false;
    }
    if sector_buf[6] != 1 {
        serial_println!("  iso9660: PVD version {} != 1", sector_buf[6]);
        return false;
    }

    // Interpret the sector buffer as a PVD.
    // SAFETY: We have verified the magic and the buffer is exactly ISO_SECTOR_SIZE
    //         bytes, which equals sizeof(Iso9660Pvd) as asserted at compile time.
    //         repr(C, packed) means no padding surprises.
    let pvd: &Iso9660Pvd = unsafe { &*(sector_buf.as_ptr() as *const Iso9660Pvd) };

    // Read block_size (must be non-zero).
    // Use a local copy to avoid packed field reference UB.
    let block_size_raw: u16 = pvd.logical_block_size_le;
    let block_size: u32 = if block_size_raw == 0 {
        ISO_SECTOR_SIZE as u32
    } else {
        block_size_raw as u32
    };

    let total_blocks: u32 = pvd.volume_space_size_le;

    // Parse the embedded root directory record (34 bytes at offset 156 in PVD).
    // Offset 156 within the PVD sector.
    let root_rec_offset: usize = 156;
    if root_rec_offset.saturating_add(DR_HEADER_SIZE) > ISO_SECTOR_SIZE {
        serial_println!("  iso9660: root dir record beyond sector bounds");
        return false;
    }

    // SAFETY: Iso9660DirRecord is repr(C, packed), Copy; buffer is large enough.
    let root_dr: Iso9660DirRecord = unsafe {
        core::ptr::read_unaligned(
            sector_buf.as_ptr().add(root_rec_offset) as *const Iso9660DirRecord
        )
    };

    let root_lba: u32 = root_dr.loc_le;
    let root_len: u32 = root_dr.data_len_le;

    // Copy volume_id bytes from PVD.
    let mut volume_id = [0u8; 32];
    // volume_id lives at offset 40 in the PVD.
    let vid_offset: usize = 40;
    if vid_offset.saturating_add(32) <= ISO_SECTOR_SIZE {
        volume_id.copy_from_slice(&sector_buf[vid_offset..vid_offset.saturating_add(32)]);
    }

    serial_println!(
        "  iso9660: mounted device {} vol[{}] — block_size={} total_blocks={} root_lba={} root_len={}",
        device_idx, vol_idx, block_size, total_blocks, root_lba, root_len
    );

    // Populate the volume slot.
    let mut guard = ISO_VOLUMES.lock();
    guard[vol_idx] = Iso9660Volume {
        active: true,
        device_idx,
        block_size,
        total_blocks,
        root_lba,
        root_len,
        volume_id,
        cache_lba: 0,
        cache_valid: false,
        cache_buf: [0u8; ISO_SECTOR_SIZE],
        dir_cache_lba: 0,
        dir_cache_valid: false,
        dir_cache_buf: [0u8; ISO_SECTOR_SIZE],
    };
    true
}

// ============================================================================
// Sector cache helpers  (operate on a single Iso9660Volume, no lock needed
// because callers already hold the ISO_VOLUMES lock)
// ============================================================================

/// Return a reference to the cached sector data for `lba`, reading from the
/// device if the cache does not already hold that sector.
///
/// Returns `Some(&sector_bytes)` on success, `None` on I/O error.
///
/// # Note
/// This mutates the `cache_buf` / `cache_valid` / `cache_lba` fields of `vol`
/// through shared references.  The caller holds `ISO_VOLUMES.lock()` which
/// serialises all access.
fn iso9660_read_sector_cached<'a>(
    vol: &'a mut Iso9660Volume,
    lba: u32,
) -> Option<&'a [u8; ISO_SECTOR_SIZE]> {
    if vol.cache_valid && vol.cache_lba == lba {
        return Some(&vol.cache_buf);
    }
    // Cache miss — read from device.
    let device_idx = vol.device_idx;
    if !read_sector(device_idx, lba, &mut vol.cache_buf) {
        return None;
    }
    vol.cache_lba = lba;
    vol.cache_valid = true;
    Some(&vol.cache_buf)
}

/// Same as `iso9660_read_sector_cached` but uses the dedicated directory cache
/// slot so that directory walks do not evict recently read file-data sectors.
fn iso9660_read_dir_sector_cached<'a>(
    vol: &'a mut Iso9660Volume,
    lba: u32,
) -> Option<&'a [u8; ISO_SECTOR_SIZE]> {
    if vol.dir_cache_valid && vol.dir_cache_lba == lba {
        return Some(&vol.dir_cache_buf);
    }
    let device_idx = vol.device_idx;
    if !read_sector(device_idx, lba, &mut vol.dir_cache_buf) {
        return None;
    }
    vol.dir_cache_lba = lba;
    vol.dir_cache_valid = true;
    Some(&vol.dir_cache_buf)
}

// ============================================================================
// Name comparison helpers
// ============================================================================

/// Return `true` when `iso_name` matches `target`, using case-insensitive
/// ASCII comparison and stripping the ";1" version suffix from ISO names.
///
/// Both slices contain raw bytes — neither is NUL-terminated.
pub fn name_matches_iso(iso_name: &[u8], target: &[u8]) -> bool {
    // Strip ";1" (or any ";N") version suffix from the ISO name.
    let iso_trimmed = {
        let mut end = iso_name.len();
        // Walk backwards looking for ';'
        let mut i = iso_name.len();
        while i > 0 {
            i = i.saturating_sub(1);
            if iso_name[i] == b';' {
                end = i;
                break;
            }
        }
        &iso_name[..end]
    };

    if iso_trimmed.len() != target.len() {
        return false;
    }

    // Case-insensitive byte comparison.
    let mut i = 0usize;
    while i < iso_trimmed.len() {
        let a = iso_trimmed[i].to_ascii_uppercase();
        let b = target[i].to_ascii_uppercase();
        if a != b {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ============================================================================
// Directory walking
// ============================================================================

/// Search a directory extent for an entry whose name matches `name`.
///
/// Walks all sectors of the directory (up to `MAX_DIR_SECTORS`), parsing
/// variable-length directory records.  Skips "." and ".." entries.
///
/// Returns a copy of the matching `Iso9660DirRecord` on success.
pub fn iso9660_find_entry(
    vol: &mut Iso9660Volume,
    dir_lba: u32,
    dir_len: u32,
    name: &[u8],
) -> Option<Iso9660DirRecord> {
    // How many 2048-byte sectors does this directory span?
    let block_size = vol.block_size;
    if block_size == 0 {
        return None;
    }

    // Compute number of sectors.  Use saturating arithmetic throughout.
    let sectors_in_dir = dir_len.saturating_add(block_size.saturating_sub(1)) / block_size;
    let sectors_to_scan = if sectors_in_dir > MAX_DIR_SECTORS {
        MAX_DIR_SECTORS
    } else {
        sectors_in_dir
    };

    let mut sector_idx: u32 = 0;
    while sector_idx < sectors_to_scan {
        let lba = dir_lba.saturating_add(sector_idx);
        let sector_data: [u8; ISO_SECTOR_SIZE] = {
            let maybe = iso9660_read_dir_sector_cached(vol, lba);
            match maybe {
                Some(s) => *s,
                None => {
                    sector_idx = sector_idx.saturating_add(1);
                    continue;
                }
            }
        };

        // Walk records within this sector.
        let mut offset: usize = 0;
        loop {
            if offset >= ISO_SECTOR_SIZE {
                break;
            }
            let rec_len = sector_data[offset] as usize;
            if rec_len == 0 {
                // Padding to end of sector — advance to next sector.
                break;
            }
            if rec_len < DR_HEADER_SIZE || offset.saturating_add(rec_len) > ISO_SECTOR_SIZE {
                // Corrupt record — stop scanning this sector.
                break;
            }

            // Read the directory record header.
            // SAFETY: bounds checked above; repr(C, packed), Copy.
            let dr: Iso9660DirRecord = unsafe {
                core::ptr::read_unaligned(
                    sector_data.as_ptr().add(offset) as *const Iso9660DirRecord
                )
            };

            let name_len = dr.name_len as usize;

            // Name immediately follows the fixed-size header.
            let name_offset = offset.saturating_add(DR_HEADER_SIZE);
            let name_end = name_offset.saturating_add(name_len);

            if name_end > ISO_SECTOR_SIZE || name_end > offset.saturating_add(rec_len) {
                // Name would overflow — skip.
                offset = offset.saturating_add(rec_len);
                continue;
            }

            // Detect "." (name byte == 0x00) and ".." (name byte == 0x01).
            let is_special = name_len == 1
                && (sector_data[name_offset] == 0x00 || sector_data[name_offset] == 0x01);

            if !is_special {
                let iso_name = &sector_data[name_offset..name_end];
                if name_matches_iso(iso_name, name) {
                    return Some(dr);
                }
            }

            offset = offset.saturating_add(rec_len);
        }

        sector_idx = sector_idx.saturating_add(1);
    }

    None // not found
}

/// Resolve a slash-separated `path` starting from the root directory,
/// returning the final `Iso9660DirRecord`.
///
/// Path components are split on `/`; empty components and leading `/` are
/// skipped.  Each component is looked up in the current directory.
///
/// Returns `None` if any component is not found or a non-terminal component
/// is not a directory.
pub fn iso9660_path_to_record(vol: &mut Iso9660Volume, path: &[u8]) -> Option<Iso9660DirRecord> {
    // Start at root.
    let mut cur_lba = vol.root_lba;
    let mut cur_len = vol.root_len;

    let mut depth: usize = 0;

    // Walk through path components split on b'/'.
    let mut pos: usize = 0;
    let path_len = path.len();

    // We track whether we actually consumed any component.
    let mut any_component = false;

    // Build a synthetic "root" record to return if path is "/" or "".
    let mut last_record = Iso9660DirRecord {
        len: 0,
        ext_attr_len: 0,
        loc_le: cur_lba,
        loc_be: 0,
        data_len_le: cur_len,
        data_len_be: 0,
        recording_date: [0u8; 7],
        file_flags: DIR_FLAG_DIRECTORY,
        file_unit_size: 0,
        interleave_gap: 0,
        vol_seq_num_le: 0,
        vol_seq_num_be: 0,
        name_len: 0,
    };

    while pos < path_len {
        // Skip any leading '/' characters.
        while pos < path_len && path[pos] == b'/' {
            pos = pos.saturating_add(1);
        }
        if pos >= path_len {
            break;
        }

        // Find end of component.
        let comp_start = pos;
        while pos < path_len && path[pos] != b'/' {
            pos = pos.saturating_add(1);
        }
        let comp_end = pos;

        let component = &path[comp_start..comp_end];
        if component.is_empty() {
            continue;
        }

        // Guard against absurdly deep paths.
        if depth >= MAX_PATH_DEPTH {
            return None;
        }
        depth = depth.saturating_add(1);

        // Look up this component in the current directory.
        let found = iso9660_find_entry(vol, cur_lba, cur_len, component)?;
        any_component = true;

        // If there are more components, the found entry must be a directory.
        let remaining_has_more = {
            let mut tmp = pos;
            while tmp < path_len && path[tmp] == b'/' {
                tmp = tmp.saturating_add(1);
            }
            tmp < path_len
        };

        if remaining_has_more && !found.is_dir() {
            return None; // non-directory in mid-path
        }

        cur_lba = found.loc_le;
        cur_len = found.data_len_le;
        last_record = found;
    }

    if any_component {
        Some(last_record)
    } else {
        // Empty path — return synthetic root record.
        Some(last_record)
    }
}

// ============================================================================
// Public file / directory operations
// ============================================================================

/// Read up to `max` bytes of the file at `path` on volume `vol_idx` into `buf`.
///
/// Returns the number of bytes actually read (≥ 0) or `-1` on error.
pub fn iso9660_read_file(vol_idx: usize, path: &[u8], buf: &mut [u8], max: usize) -> isize {
    if vol_idx >= MAX_VOLUMES || max == 0 || buf.len() < max {
        return -1;
    }

    let mut guard = ISO_VOLUMES.lock();
    let vol = &mut guard[vol_idx];
    if !vol.active {
        return -1;
    }

    // Resolve the path.
    let record = match iso9660_path_to_record(vol, path) {
        Some(r) => r,
        None => return -1,
    };

    // Must be a file, not a directory.
    if record.is_dir() {
        return -1;
    }

    let file_lba = record.loc_le;
    let file_size = record.data_len_le;
    if file_size == 0 {
        return 0;
    }

    // Bytes to read: min(file_size, max).
    let to_read = {
        let fs = file_size as usize;
        if fs < max {
            fs
        } else {
            max
        }
    };

    let block_size = vol.block_size as usize;
    if block_size == 0 {
        return -1;
    }

    let mut bytes_done: usize = 0;
    let mut sector_idx: u32 = 0;

    while bytes_done < to_read {
        let lba = file_lba.saturating_add(sector_idx);

        // Read sector into the general-purpose sector cache.
        let sector_data: [u8; ISO_SECTOR_SIZE] = {
            match iso9660_read_sector_cached(vol, lba) {
                Some(s) => *s,
                None => return -1,
            }
        };

        let sector_offset: usize = 0; // reading consecutive sectors, offset always 0
        let remaining = to_read.saturating_sub(bytes_done);
        let avail = ISO_SECTOR_SIZE.saturating_sub(sector_offset);
        let chunk = if avail < remaining { avail } else { remaining };

        if bytes_done.saturating_add(chunk) > buf.len() {
            return -1; // buf too small
        }

        buf[bytes_done..bytes_done.saturating_add(chunk)]
            .copy_from_slice(&sector_data[sector_offset..sector_offset.saturating_add(chunk)]);

        bytes_done = bytes_done.saturating_add(chunk);
        sector_idx = sector_idx.wrapping_add(1);
    }

    bytes_done as isize
}

/// List up to 64 entries in the directory at `path` on volume `vol_idx`.
///
/// Each entry name is written into the next slot of `names`, NUL-padded to
/// 32 bytes.  Returns the number of entries written.
pub fn iso9660_list_dir(vol_idx: usize, path: &[u8], names: &mut [[u8; 32]; 64]) -> u32 {
    if vol_idx >= MAX_VOLUMES {
        return 0;
    }

    let mut guard = ISO_VOLUMES.lock();
    let vol = &mut guard[vol_idx];
    if !vol.active {
        return 0;
    }

    // Resolve the path.
    let record = match iso9660_path_to_record(vol, path) {
        Some(r) => r,
        None => return 0,
    };

    if !record.is_dir() {
        return 0;
    }

    let dir_lba = record.loc_le;
    let dir_len = record.data_len_le;
    let block_size = vol.block_size;
    if block_size == 0 {
        return 0;
    }

    let sectors_in_dir = dir_len.saturating_add(block_size.saturating_sub(1)) / block_size;
    let sectors_to_scan = if sectors_in_dir > MAX_DIR_SECTORS {
        MAX_DIR_SECTORS
    } else {
        sectors_in_dir
    };

    let mut count: u32 = 0;
    let max_entries: u32 = 64;

    let mut sector_idx: u32 = 0;
    while sector_idx < sectors_to_scan && count < max_entries {
        let lba = dir_lba.saturating_add(sector_idx);
        let sector_data: [u8; ISO_SECTOR_SIZE] = {
            match iso9660_read_dir_sector_cached(vol, lba) {
                Some(s) => *s,
                None => {
                    sector_idx = sector_idx.saturating_add(1);
                    continue;
                }
            }
        };

        let mut offset: usize = 0;
        loop {
            if offset >= ISO_SECTOR_SIZE || count >= max_entries {
                break;
            }

            let rec_len = sector_data[offset] as usize;
            if rec_len == 0 {
                break; // end-of-sector padding
            }
            if rec_len < DR_HEADER_SIZE || offset.saturating_add(rec_len) > ISO_SECTOR_SIZE {
                break;
            }

            // SAFETY: bounds checked above.
            let dr: Iso9660DirRecord = unsafe {
                core::ptr::read_unaligned(
                    sector_data.as_ptr().add(offset) as *const Iso9660DirRecord
                )
            };

            let name_len = dr.name_len as usize;
            let name_offset = offset.saturating_add(DR_HEADER_SIZE);
            let name_end = name_offset.saturating_add(name_len);

            if name_end <= ISO_SECTOR_SIZE && name_end <= offset.saturating_add(rec_len) {
                // Skip "." and ".."
                let is_special = name_len == 1
                    && (sector_data[name_offset] == 0x00 || sector_data[name_offset] == 0x01);

                if !is_special {
                    let raw_name = &sector_data[name_offset..name_end];
                    // Strip ";1" suffix and copy into output slot, NUL-padded.
                    let trimmed = strip_version_suffix(raw_name);
                    let copy_len = if trimmed.len() < 32 {
                        trimmed.len()
                    } else {
                        32
                    };

                    let slot_idx = count as usize;
                    if slot_idx < 64 {
                        names[slot_idx] = [0u8; 32];
                        names[slot_idx][..copy_len].copy_from_slice(&trimmed[..copy_len]);
                        count = count.saturating_add(1);
                    }
                }
            }

            offset = offset.saturating_add(rec_len);
        }

        sector_idx = sector_idx.saturating_add(1);
    }

    count
}

/// Return the size in bytes of the file at `path` on volume `vol_idx`,
/// or `None` if the path does not exist or is a directory.
pub fn iso9660_file_size(vol_idx: usize, path: &[u8]) -> Option<u32> {
    if vol_idx >= MAX_VOLUMES {
        return None;
    }

    let mut guard = ISO_VOLUMES.lock();
    let vol = &mut guard[vol_idx];
    if !vol.active {
        return None;
    }

    let record = iso9660_path_to_record(vol, path)?;
    if record.is_dir() {
        return None;
    }
    Some(record.data_len_le)
}

// ============================================================================
// Utility
// ============================================================================

/// Return a sub-slice of `name` with any trailing ";N" version suffix removed.
///
/// ISO 9660 file names look like `AUTORUN.INF;1` — the ";1" is the version
/// number, which we strip for user-facing comparisons and display.
fn strip_version_suffix(name: &[u8]) -> &[u8] {
    let mut i = name.len();
    while i > 0 {
        i = i.saturating_sub(1);
        if name[i] == b';' {
            return &name[..i];
        }
    }
    name
}

// ============================================================================
// Module entry point
// ============================================================================

/// Initialise the ISO 9660 driver.
///
/// Called from `fs::init()`.  Resets all volume slots to inactive and logs
/// a ready message.  Actual mounting is triggered by `iso9660_mount()`.
pub fn init() {
    {
        let mut guard = ISO_VOLUMES.lock();
        for slot in guard.iter_mut() {
            unsafe {
                core::ptr::write_bytes(slot as *mut Iso9660Volume, 0, 1);
            }
        }
    }
    serial_println!("  iso9660: ISO 9660 driver ready");
}
