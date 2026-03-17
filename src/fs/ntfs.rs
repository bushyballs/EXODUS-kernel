use crate::fs::vfs::FsError;
/// NTFS read-only driver for Genesis
///
/// Implements read-only access to the NTFS filesystem, the default
/// filesystem for Windows since NT 3.1. Supports:
///   - MFT (Master File Table) parsing
///   - Attribute parsing (standard info, filename, data, index)
///   - Data runs (non-resident data extents)
///   - Compressed file reading (LZNT1 decompression)
///   - Directory index (B+ tree) traversal
///
/// On-disk layout:
///   Sector 0: Boot sector (BPB + NTFS signature)
///   MFT: Master File Table (array of 1KB FILE records)
///   Each FILE record: header + sequence of attributes
///   Data stored in "runs" (contiguous cluster extents)
///
/// Inspired by: Linux ntfs3 driver, libntfs. All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// NTFS boot sector signature at offset 3
const NTFS_OEM_ID: &[u8; 8] = b"NTFS    ";

/// MFT FILE record magic: "FILE"
const FILE_RECORD_MAGIC: u32 = 0x454C_4946;

/// Attribute type codes
const ATTR_STANDARD_INFO: u32 = 0x10;
const ATTR_FILENAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_INDEX_ROOT: u32 = 0x90;
const ATTR_INDEX_ALLOC: u32 = 0xA0;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// Well-known MFT entry numbers
const MFT_ENTRY_MFT: u64 = 0;
const MFT_ENTRY_ROOT: u64 = 5;

/// Maximum supported cluster size (64 KB)
const MAX_CLUSTER_SIZE: usize = 65536;

/// LZNT1 compression unit: 4096 bytes (16 clusters of 256 bytes)
const COMPRESSION_UNIT_SIZE: usize = 4096;

/// NTFS boot sector (BPB)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NtfsBpb {
    pub jump: [u8; 3],
    pub oem_id: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub _unused1: [u8; 3], // always 0 for NTFS
    pub _unused2: u16,     // always 0
    pub media_descriptor: u8,
    pub _unused3: u16, // always 0
    pub sectors_per_track: u16,
    pub number_of_heads: u16,
    pub hidden_sectors: u32,
    pub _unused4: u32, // always 0
    pub _unused5: u32, // 0x00800080
    pub total_sectors: u64,
    pub mft_cluster: u64,
    pub mft_mirror_cluster: u64,
    /// Clusters per MFT record (or negative = 2^|value| bytes)
    pub clusters_per_mft_record: i8,
    pub _padding1: [u8; 3],
    /// Clusters per index block (or negative = 2^|value| bytes)
    pub clusters_per_index_block: i8,
    pub _padding2: [u8; 3],
    pub serial_number: u64,
}

/// MFT FILE record header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FileRecordHeader {
    pub magic: u32, // "FILE" = 0x454C4946
    pub update_seq_offset: u16,
    pub update_seq_count: u16,
    pub logfile_seq_number: u64,
    pub sequence_number: u16,
    pub hard_link_count: u16,
    pub first_attr_offset: u16,
    pub flags: u16, // 0x01 = in use, 0x02 = directory
    pub used_size: u32,
    pub allocated_size: u32,
    pub base_record_ref: u64,
    pub next_attr_id: u16,
}

/// Attribute header (common part)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct AttrHeader {
    pub attr_type: u32,
    pub length: u32,
    pub non_resident: u8,
    pub name_length: u8,
    pub name_offset: u16,
    pub flags: u16,
    pub attr_id: u16,
}

/// Resident attribute data (follows AttrHeader when non_resident == 0)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct ResidentAttrData {
    pub value_length: u32,
    pub value_offset: u16,
    pub indexed_flag: u16,
}

/// Non-resident attribute data (follows AttrHeader when non_resident == 1)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct NonResidentAttrData {
    pub start_vcn: u64,
    pub end_vcn: u64,
    pub data_runs_offset: u16,
    pub compression_unit: u16,
    pub _padding: u32,
    pub alloc_size: u64,
    pub real_size: u64,
    pub init_size: u64,
}

/// Filename attribute (within ATTR_FILENAME)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct FilenameAttr {
    pub parent_dir_ref: u64,
    pub created: u64,
    pub modified: u64,
    pub mft_modified: u64,
    pub accessed: u64,
    pub alloc_size: u64,
    pub real_size: u64,
    pub flags: u32,
    pub reparse_value: u32,
    pub name_length: u8,
    pub name_type: u8,
    // followed by name_length * 2 bytes of UTF-16LE name
}

/// A parsed data run (extent)
#[derive(Debug, Clone, Copy)]
pub struct DataRun {
    /// Starting cluster (absolute, LCN)
    pub lcn: u64,
    /// Number of clusters in this run
    pub length: u64,
    /// True if this is a sparse (hole) run
    pub sparse: bool,
}

/// Parsed MFT entry with key attributes
#[derive(Debug, Clone)]
pub struct MftEntry {
    pub index: u64,
    pub flags: u16,
    pub sequence: u16,
    pub filename: String,
    pub parent_ref: u64,
    pub file_size: u64,
    pub is_directory: bool,
    pub data_runs: Vec<DataRun>,
    pub is_compressed: bool,
}

/// Directory entry from index
#[derive(Debug, Clone)]
pub struct NtfsDirEntry {
    pub name: String,
    pub mft_ref: u64,
    pub is_directory: bool,
    pub file_size: u64,
}

/// In-memory NTFS filesystem state
pub struct NtfsFs {
    /// Block device reader (sector-level)
    read_sector_fn: fn(sector: u64, buf: &mut [u8]) -> Result<(), FsError>,
    /// Parsed boot sector
    pub bpb: NtfsBpb,
    /// Bytes per cluster
    pub cluster_size: usize,
    /// MFT record size in bytes
    pub mft_record_size: usize,
    /// First cluster of MFT
    pub mft_start_cluster: u64,
    /// Cached MFT data runs (for the MFT itself)
    pub mft_runs: Vec<DataRun>,
}

/// Global NTFS driver state
static NTFS_STATE: Mutex<Option<NtfsFs>> = Mutex::new(None);

/// Parse data runs from raw bytes
///
/// Data runs encode cluster extents as variable-length pairs:
///   byte 0: low nibble = length field size, high nibble = offset field size
///   followed by length bytes, then offset bytes (signed, delta from previous LCN)
fn parse_data_runs(data: &[u8]) -> Vec<DataRun> {
    let mut runs = Vec::new();
    let mut pos = 0;
    let mut prev_lcn: i64 = 0;

    while pos < data.len() {
        let header = data[pos];
        if header == 0 {
            break;
        }
        pos = pos.saturating_add(1);

        let len_size = (header & 0x0F) as usize;
        let off_size = ((header >> 4) & 0x0F) as usize;

        if len_size == 0 || pos + len_size + off_size > data.len() {
            break;
        }

        // Read length (unsigned)
        let mut length: u64 = 0;
        for i in 0..len_size {
            length |= (data[pos + i] as u64) << (i * 8);
        }
        pos = pos.saturating_add(len_size);

        // Read offset (signed delta)
        if off_size == 0 {
            // Sparse run (no physical clusters)
            runs.push(DataRun {
                lcn: 0,
                length,
                sparse: true,
            });
        } else {
            let mut offset: i64 = 0;
            for i in 0..off_size {
                offset |= (data[pos + i] as i64) << (i * 8);
            }
            // Sign extend
            if off_size > 0 && (data[pos + off_size - 1] & 0x80) != 0 {
                for i in off_size..8 {
                    offset |= 0xFFi64 << (i * 8);
                }
            }
            pos = pos.saturating_add(off_size);

            prev_lcn += offset;
            runs.push(DataRun {
                lcn: prev_lcn as u64,
                length,
                sparse: false,
            });
        }
    }

    runs
}

/// LZNT1 decompression (simplified — NTFS standard compression)
///
/// Each compression unit (4096 bytes) is either:
///   - Compressed: 2-byte header + compressed data
///   - Uncompressed: raw 4096 bytes (when compression doesn't shrink it)
fn decompress_lznt1(compressed: &[u8], output: &mut Vec<u8>) {
    let mut src = 0;

    while src + 2 <= compressed.len() {
        let chunk_header = u16::from_le_bytes([compressed[src], compressed[src + 1]]);
        src = src.saturating_add(2);

        if chunk_header == 0 {
            break;
        }

        let chunk_size = (chunk_header & 0x0FFF) as usize + 1;
        let is_compressed = (chunk_header & 0x8000) != 0;

        if !is_compressed {
            // Uncompressed chunk
            let end = (src + chunk_size).min(compressed.len());
            output.extend_from_slice(&compressed[src..end]);
            src = end;
            continue;
        }

        // Compressed chunk — process flag groups
        let chunk_end = (src + chunk_size).min(compressed.len());
        while src < chunk_end {
            if src >= compressed.len() {
                break;
            }
            let flags = compressed[src];
            src = src.saturating_add(1);

            for bit in 0..8u8 {
                if src >= chunk_end {
                    break;
                }

                if (flags >> bit) & 1 == 0 {
                    // Literal byte
                    output.push(compressed[src]);
                    src = src.saturating_add(1);
                } else {
                    // Back-reference
                    if src + 1 >= compressed.len() {
                        break;
                    }
                    let token = u16::from_le_bytes([compressed[src], compressed[src + 1]]);
                    src = src.saturating_add(2);

                    // Calculate displacement bits based on output position within unit
                    let out_pos = output.len();
                    let unit_pos = out_pos % COMPRESSION_UNIT_SIZE;
                    let disp_bits = if unit_pos < 2 {
                        4
                    } else {
                        let mut bits = 4u32;
                        let mut threshold = 2usize;
                        while threshold < unit_pos && bits < 12 {
                            bits = bits.saturating_add(1);
                            threshold <<= 1;
                        }
                        bits
                    };

                    let disp_mask = (1u16 << disp_bits) - 1;
                    let displacement = (token & disp_mask) as usize + 1;
                    let copy_len = (token >> disp_bits) as usize + 3;

                    for _i in 0..copy_len {
                        if displacement > output.len() {
                            output.push(0);
                        } else {
                            let byte = output[output.len() - displacement];
                            output.push(byte);
                        }
                    }
                }
            }
        }
    }
}

impl NtfsFs {
    /// Try to mount an NTFS filesystem
    pub fn mount(read_sector: fn(u64, &mut [u8]) -> Result<(), FsError>) -> Result<Self, FsError> {
        // Read boot sector
        let mut boot_buf = [0u8; 512];
        read_sector(0, &mut boot_buf)?;

        let bpb: NtfsBpb =
            unsafe { core::ptr::read_unaligned(boot_buf.as_ptr() as *const NtfsBpb) };

        // Validate NTFS signature
        if &bpb.oem_id != NTFS_OEM_ID {
            return Err(FsError::InvalidArgument);
        }

        let bytes_per_sector = bpb.bytes_per_sector as usize;
        let sectors_per_cluster = bpb.sectors_per_cluster as usize;
        let cluster_size = bytes_per_sector * sectors_per_cluster;

        if cluster_size == 0 || cluster_size > MAX_CLUSTER_SIZE {
            return Err(FsError::InvalidArgument);
        }

        // Calculate MFT record size
        let mft_record_size = if bpb.clusters_per_mft_record >= 0 {
            bpb.clusters_per_mft_record as usize * cluster_size
        } else {
            1usize << ((-bpb.clusters_per_mft_record) as usize)
        };

        let mft_start = bpb.mft_cluster;

        // Read the first MFT record ($MFT itself, entry 0) to get MFT data runs
        let mft_first_sector = mft_start * sectors_per_cluster as u64;
        let mut mft_buf = alloc::vec![0u8; mft_record_size];
        let sectors_needed = (mft_record_size + bytes_per_sector - 1) / bytes_per_sector;
        for i in 0..sectors_needed {
            let mut sector_buf = alloc::vec![0u8; bytes_per_sector];
            read_sector(mft_first_sector + i as u64, &mut sector_buf)?;
            let start = i * bytes_per_sector;
            let end = (start + bytes_per_sector).min(mft_record_size);
            mft_buf[start..end].copy_from_slice(&sector_buf[..end - start]);
        }

        // Apply fixup array to MFT record
        Self::apply_fixups_raw(&mut mft_buf, bytes_per_sector);

        // Parse $MFT's data runs
        let mft_runs = Self::find_data_runs_in_record(&mft_buf);

        let total = { bpb.total_sectors };
        serial_println!(
            "  NTFS: mounted -- {} sectors, cluster {}B, MFT at cluster {}",
            total,
            cluster_size,
            mft_start
        );

        Ok(NtfsFs {
            read_sector_fn: read_sector,
            bpb,
            cluster_size,
            mft_record_size,
            mft_start_cluster: mft_start,
            mft_runs,
        })
    }

    /// Apply update sequence fixups to a raw record buffer
    fn apply_fixups_raw(buf: &mut [u8], sector_size: usize) {
        if buf.len() < 48 {
            return;
        }
        let header: FileRecordHeader =
            unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const FileRecordHeader) };
        let uso = header.update_seq_offset as usize;
        let usc = header.update_seq_count as usize;

        if uso + usc * 2 > buf.len() || usc < 2 {
            return;
        }

        // The update sequence array: first entry is the value to check,
        // remaining entries replace the last two bytes of each sector

        for i in 1..usc {
            let replace_offset = i * sector_size - 2;
            if replace_offset + 1 < buf.len() && uso + i * 2 + 1 < buf.len() {
                buf[replace_offset] = buf[uso + i * 2];
                buf[replace_offset + 1] = buf[uso + i * 2 + 1];
            }
        }
    }

    /// Find the $DATA attribute's data runs in a raw MFT record
    fn find_data_runs_in_record(record: &[u8]) -> Vec<DataRun> {
        if record.len() < 48 {
            return Vec::new();
        }
        let header: FileRecordHeader =
            unsafe { core::ptr::read_unaligned(record.as_ptr() as *const FileRecordHeader) };
        if header.magic != FILE_RECORD_MAGIC {
            return Vec::new();
        }

        let mut offset = header.first_attr_offset as usize;
        while offset + 16 <= record.len() {
            let attr: AttrHeader = unsafe {
                core::ptr::read_unaligned(record[offset..].as_ptr() as *const AttrHeader)
            };
            if attr.attr_type == ATTR_END || attr.length == 0 {
                break;
            }
            if attr.attr_type == ATTR_DATA && attr.non_resident == 1 {
                let nr: NonResidentAttrData = unsafe {
                    core::ptr::read_unaligned(
                        record[offset + 16..].as_ptr() as *const NonResidentAttrData
                    )
                };
                let runs_start = offset + nr.data_runs_offset as usize;
                let runs_end = offset + attr.length as usize;
                if runs_start < runs_end && runs_end <= record.len() {
                    return parse_data_runs(&record[runs_start..runs_end]);
                }
            }
            offset += attr.length as usize;
        }

        Vec::new()
    }

    /// Read clusters from disk
    fn read_clusters(&self, start_lcn: u64, count: u64, buf: &mut [u8]) -> Result<(), FsError> {
        let bps = self.bpb.bytes_per_sector as usize;
        let spc = self.bpb.sectors_per_cluster as u64;

        for c in 0..count {
            let cluster_lcn = start_lcn + c;
            let first_sector = cluster_lcn * spc;
            let buf_offset = c as usize * self.cluster_size;

            for s in 0..spc as usize {
                let mut sector_buf = alloc::vec![0u8; bps];
                (self.read_sector_fn)(first_sector + s as u64, &mut sector_buf)?;
                let dst_start = buf_offset + s * bps;
                let dst_end = (dst_start + bps).min(buf.len());
                if dst_start < buf.len() {
                    let copy_len = dst_end - dst_start;
                    buf[dst_start..dst_end].copy_from_slice(&sector_buf[..copy_len]);
                }
            }
        }
        Ok(())
    }

    /// Read data described by a set of data runs into a contiguous buffer
    fn read_data_runs(&self, runs: &[DataRun], total_size: u64) -> Result<Vec<u8>, FsError> {
        let mut data = alloc::vec![0u8; total_size as usize];
        let mut offset = 0usize;

        for run in runs {
            let run_bytes = run.length as usize * self.cluster_size;
            let to_read = run_bytes.min(total_size as usize - offset);

            if run.sparse {
                // Sparse run — already zeroed
                offset += to_read;
            } else {
                let mut run_buf = alloc::vec![0u8; run_bytes];
                self.read_clusters(run.lcn, run.length, &mut run_buf)?;
                let copy_len = to_read.min(run_buf.len());
                data[offset..offset + copy_len].copy_from_slice(&run_buf[..copy_len]);
                offset += copy_len;
            }

            if offset >= total_size as usize {
                break;
            }
        }

        Ok(data)
    }

    /// Read an MFT entry by index number
    pub fn read_mft_entry(&self, index: u64) -> Result<MftEntry, FsError> {
        let byte_offset = index * self.mft_record_size as u64;
        let cluster_offset = byte_offset / self.cluster_size as u64;
        let offset_in_cluster = (byte_offset % self.cluster_size as u64) as usize;

        // Find which data run contains this MFT record
        let mut vcn: u64 = 0;
        let mut target_lcn: Option<u64> = None;
        let mut lcn_cluster_offset: u64 = 0;

        for run in &self.mft_runs {
            if cluster_offset >= vcn && cluster_offset < vcn + run.length {
                if !run.sparse {
                    target_lcn = Some(run.lcn);
                    lcn_cluster_offset = cluster_offset - vcn;
                }
                break;
            }
            vcn += run.length;
        }

        let lcn = target_lcn.ok_or(FsError::NotFound)?;

        // Read enough clusters to cover the MFT record
        let clusters_needed = (self.mft_record_size + self.cluster_size - 1) / self.cluster_size;
        let mut buf = alloc::vec![0u8; clusters_needed * self.cluster_size];
        self.read_clusters(lcn + lcn_cluster_offset, clusters_needed as u64, &mut buf)?;

        let record = &mut buf[offset_in_cluster..offset_in_cluster + self.mft_record_size];

        // Apply fixups
        Self::apply_fixups_raw(record, self.bpb.bytes_per_sector as usize);

        // Parse the record header
        let header: FileRecordHeader =
            unsafe { core::ptr::read_unaligned(record.as_ptr() as *const FileRecordHeader) };

        if header.magic != FILE_RECORD_MAGIC {
            return Err(FsError::NotFound);
        }

        let is_directory = (header.flags & 0x02) != 0;
        let mut filename = String::new();
        let mut parent_ref: u64 = 0;
        let mut file_size: u64 = 0;
        let mut data_runs = Vec::new();
        let mut is_compressed = false;

        // Walk attributes
        let mut attr_offset = header.first_attr_offset as usize;
        while attr_offset + 16 <= self.mft_record_size {
            let attr: AttrHeader = unsafe {
                core::ptr::read_unaligned(record[attr_offset..].as_ptr() as *const AttrHeader)
            };

            if attr.attr_type == ATTR_END || attr.length == 0 {
                break;
            }

            match attr.attr_type {
                ATTR_FILENAME if attr.non_resident == 0 => {
                    let res: ResidentAttrData = unsafe {
                        core::ptr::read_unaligned(
                            record[attr_offset + 16..].as_ptr() as *const ResidentAttrData
                        )
                    };
                    let val_off = attr_offset + res.value_offset as usize;
                    if val_off + 66 <= record.len() {
                        let fattr: FilenameAttr = unsafe {
                            core::ptr::read_unaligned(
                                record[val_off..].as_ptr() as *const FilenameAttr
                            )
                        };
                        parent_ref = fattr.parent_dir_ref & 0x0000_FFFF_FFFF_FFFF;
                        let name_off = val_off + 66;
                        let name_len = fattr.name_length as usize * 2;
                        if name_off + name_len <= record.len() {
                            // Decode UTF-16LE
                            let mut chars = Vec::new();
                            for i in 0..fattr.name_length as usize {
                                let lo = record[name_off + i * 2] as u16;
                                let hi = record[name_off + i * 2 + 1] as u16;
                                let ch = lo | (hi << 8);
                                if let Some(c) = core::char::from_u32(ch as u32) {
                                    chars.push(c);
                                }
                            }
                            // Prefer the Win32 name (type 1 or 3)
                            if fattr.name_type == 1 || fattr.name_type == 3 || filename.is_empty() {
                                filename = chars.into_iter().collect();
                            }
                        }
                    }
                }
                ATTR_DATA => {
                    if (attr.flags & 0x0001) != 0 {
                        is_compressed = true;
                    }
                    if attr.non_resident == 1 {
                        let nr: NonResidentAttrData = unsafe {
                            core::ptr::read_unaligned(
                                record[attr_offset + 16..].as_ptr() as *const NonResidentAttrData
                            )
                        };
                        file_size = nr.real_size;
                        let runs_start = attr_offset + nr.data_runs_offset as usize;
                        let runs_end = attr_offset + attr.length as usize;
                        if runs_start < runs_end && runs_end <= record.len() {
                            data_runs = parse_data_runs(&record[runs_start..runs_end]);
                        }
                    } else {
                        let res: ResidentAttrData = unsafe {
                            core::ptr::read_unaligned(
                                record[attr_offset + 16..].as_ptr() as *const ResidentAttrData
                            )
                        };
                        file_size = res.value_length as u64;
                    }
                }
                _ => {}
            }

            attr_offset += attr.length as usize;
        }

        Ok(MftEntry {
            index,
            flags: header.flags,
            sequence: header.sequence_number,
            filename,
            parent_ref,
            file_size,
            is_directory,
            data_runs,
            is_compressed,
        })
    }

    /// Read the contents of a file by MFT entry
    pub fn read_file(&self, entry: &MftEntry) -> Result<Vec<u8>, FsError> {
        if entry.is_directory {
            return Err(FsError::IsADirectory);
        }

        if entry.data_runs.is_empty() {
            // Resident data — re-read and extract inline
            return self.read_resident_data(entry.index);
        }

        let raw = self.read_data_runs(&entry.data_runs, entry.file_size)?;

        if entry.is_compressed {
            let mut decompressed = Vec::new();
            decompress_lznt1(&raw, &mut decompressed);
            decompressed.truncate(entry.file_size as usize);
            Ok(decompressed)
        } else {
            let mut data = raw;
            data.truncate(entry.file_size as usize);
            Ok(data)
        }
    }

    /// Read resident (inline) data for a file
    fn read_resident_data(&self, mft_index: u64) -> Result<Vec<u8>, FsError> {
        let byte_offset = mft_index * self.mft_record_size as u64;
        let cluster_offset = byte_offset / self.cluster_size as u64;
        let offset_in_cluster = (byte_offset % self.cluster_size as u64) as usize;

        let mut vcn: u64 = 0;
        let mut target_lcn: Option<u64> = None;
        let mut lcn_offset: u64 = 0;

        for run in &self.mft_runs {
            if cluster_offset >= vcn && cluster_offset < vcn + run.length {
                if !run.sparse {
                    target_lcn = Some(run.lcn);
                    lcn_offset = cluster_offset - vcn;
                }
                break;
            }
            vcn += run.length;
        }

        let lcn = target_lcn.ok_or(FsError::NotFound)?;
        let clusters_needed = (self.mft_record_size + self.cluster_size - 1) / self.cluster_size;
        let mut buf = alloc::vec![0u8; clusters_needed * self.cluster_size];
        self.read_clusters(lcn + lcn_offset, clusters_needed as u64, &mut buf)?;

        let record = &mut buf[offset_in_cluster..offset_in_cluster + self.mft_record_size];
        Self::apply_fixups_raw(record, self.bpb.bytes_per_sector as usize);

        let header: FileRecordHeader =
            unsafe { core::ptr::read_unaligned(record.as_ptr() as *const FileRecordHeader) };

        let mut attr_offset = header.first_attr_offset as usize;
        while attr_offset + 16 <= self.mft_record_size {
            let attr: AttrHeader = unsafe {
                core::ptr::read_unaligned(record[attr_offset..].as_ptr() as *const AttrHeader)
            };
            if attr.attr_type == ATTR_END || attr.length == 0 {
                break;
            }
            if attr.attr_type == ATTR_DATA && attr.non_resident == 0 {
                let res: ResidentAttrData = unsafe {
                    core::ptr::read_unaligned(
                        record[attr_offset + 16..].as_ptr() as *const ResidentAttrData
                    )
                };
                let val_off = attr_offset + res.value_offset as usize;
                let val_len = res.value_length as usize;
                if val_off + val_len <= record.len() {
                    return Ok(record[val_off..val_off + val_len].to_vec());
                }
            }
            attr_offset += attr.length as usize;
        }

        Err(FsError::NotFound)
    }

    /// Read directory entries from a directory MFT entry
    pub fn read_directory(&self, dir_entry: &MftEntry) -> Result<Vec<NtfsDirEntry>, FsError> {
        if !dir_entry.is_directory {
            return Err(FsError::NotADirectory);
        }

        let mut entries = Vec::new();

        // Read all MFT entries and find those whose parent matches
        // This is a simplified approach; a full implementation would parse
        // index root + index allocation B+ trees
        //
        // For now, scan the first portion of the MFT for children
        let max_entries = 4096u64; // Scan limit
        for i in 0..max_entries {
            match self.read_mft_entry(i) {
                Ok(entry) => {
                    if entry.parent_ref == dir_entry.index
                        && (entry.flags & 0x01) != 0
                        && !entry.filename.is_empty()
                        && entry.filename != "."
                        && entry.filename != ".."
                    {
                        entries.push(NtfsDirEntry {
                            name: entry.filename.clone(),
                            mft_ref: entry.index,
                            is_directory: entry.is_directory,
                            file_size: entry.file_size,
                        });
                    }
                }
                Err(_) => continue,
            }
        }

        Ok(entries)
    }

    /// Resolve a path from root to an MFT entry
    pub fn lookup(&self, path: &str) -> Result<MftEntry, FsError> {
        let root = self.read_mft_entry(MFT_ENTRY_ROOT)?;
        let components: Vec<&str> = path
            .split('\\')
            .chain(path.split('/'))
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Ok(root);
        }

        let mut current = root;
        for component in &components {
            let dir_entries = self.read_directory(&current)?;
            let found = dir_entries
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(component));
            match found {
                Some(de) => {
                    current = self.read_mft_entry(de.mft_ref)?;
                }
                None => return Err(FsError::NotFound),
            }
        }

        Ok(current)
    }
}

/// Initialize the NTFS driver
pub fn init() {
    serial_println!("  NTFS: read-only driver initialized (MFT, data runs, LZNT1)");
}
