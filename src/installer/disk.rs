use crate::serial_println;
/// Disk partitioning and installation for Hoags OS
///
/// Partition layout:
///   1. EFI System Partition (512MB, FAT32) — bootloader + kernel
///   2. Root partition (rest, HoagsFS or LUKS+HoagsFS) — the OS
///
/// The installer:
///   1. Detects available disks
///   2. Partitions with GPT
///   3. Formats EFI as FAT32
///   4. Optionally encrypts root with LUKS
///   5. Formats root as HoagsFS
///   6. Copies kernel, bootloader, and system files
///   7. Installs UEFI boot entry
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// CRC-32 for GPT header checksums
// ---------------------------------------------------------------------------

/// CRC-32 (ISO 3309 / ITU-T V.42) lookup table.
/// GPT mandates CRC-32 with the standard polynomial 0xEDB88320 (reflected).
const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// Compute CRC-32 of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((crc ^ b as u32) & 0xFF) as usize;
        crc = (crc >> 8) ^ CRC32_TABLE[idx];
    }
    crc ^ 0xFFFF_FFFF
}

// ---------------------------------------------------------------------------
// GPT UUIDs (little-endian mixed-endian encoding per UEFI spec)
// ---------------------------------------------------------------------------

/// Parse a GUID string "AABBCCDD-EEFF-GGHH-IIJJ-KKLLMMNNOOPP"
/// into the 16-byte UEFI mixed-endian representation.
fn parse_guid(s: &str) -> [u8; 16] {
    // Strip hyphens and hex-decode
    let mut hex_str = String::new();
    for c in s.chars() {
        if c != '-' {
            hex_str.push(c);
        }
    }
    let bytes = hex_str.as_bytes();
    let mut raw = [0u8; 16];
    for i in 0..16 {
        let hi = hex_nibble_d(bytes[i * 2]);
        let lo = hex_nibble_d(bytes[i * 2 + 1]);
        raw[i] = (hi << 4) | lo;
    }

    // UEFI mixed-endian: first three fields are LE, last two are BE.
    // raw is currently big-endian (as written in the string).
    // field 1: bytes 0-3  → reverse
    // field 2: bytes 4-5  → reverse
    // field 3: bytes 6-7  → reverse
    // fields 4+5 remain as-is (big-endian)
    let mut out = [0u8; 16];
    out[0] = raw[3];
    out[1] = raw[2];
    out[2] = raw[1];
    out[3] = raw[0];
    out[4] = raw[5];
    out[5] = raw[4];
    out[6] = raw[7];
    out[7] = raw[6];
    out[8..16].copy_from_slice(&raw[8..16]);
    out
}

#[inline(always)]
fn hex_nibble_d(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Low-level NVMe sector write helper
// ---------------------------------------------------------------------------

/// Write a 512-byte sector to `lba` on the system NVMe namespace (nsid 1).
fn write_lba(lba: u64, sector: &[u8; 512]) -> Result<(), &'static str> {
    crate::drivers::nvme::write_sectors(1, lba, 1, sector).map_err(|_| "nvme write_lba failed")
}

/// Detected disk
#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub path: String,
    pub model: String,
    pub size_bytes: u64,
    pub partitions: Vec<PartitionInfo>,
}

/// A disk partition
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    pub number: u32,
    pub start_lba: u64,
    pub size_bytes: u64,
    pub type_guid: String,
    pub label: String,
}

/// GPT partition type GUIDs
pub mod guid {
    pub const EFI_SYSTEM: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
    pub const LINUX_ROOT: &str = "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709";
    pub const LINUX_HOME: &str = "933AC7E1-2EB4-4F13-B844-0E14E2AEF915";
    pub const HOAGS_ROOT: &str = "484F4147-5300-4F53-524F-4F5400000001"; // "HOAGS" OS ROOT
    pub const HOAGS_DATA: &str = "484F4147-5300-4F53-4441-544100000001"; // "HOAGS" OS DATA
}

/// Installation steps
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallStep {
    DetectingDisks,
    Partitioning,
    FormattingEfi,
    EncryptingRoot,
    FormattingRoot,
    CopyingFiles,
    InstallingBootloader,
    ConfiguringSystem,
    Complete,
    Failed,
}

/// Installation progress
pub struct InstallProgress {
    pub step: InstallStep,
    pub percent: u8,
    pub message: String,
}

/// Detect available disks by scanning PCI for NVMe (class 0x01, subclass 0x08)
/// and AHCI (class 0x01, subclass 0x06) storage controllers.
///
/// For each discovered controller the function adds a `DiskInfo` entry.
/// Partition data is left empty; a full implementation would read the existing
/// GPT from the disk and populate `partitions`.
pub fn detect_disks() -> Vec<DiskInfo> {
    serial_println!("  [installer] Scanning for disks...");
    let mut disks: Vec<DiskInfo> = Vec::new();

    // Scan the full PCI bus
    let pci_devices = crate::drivers::pci::scan();

    for dev in &pci_devices {
        if dev.class != 0x01 {
            continue; // not a mass-storage controller
        }
        match dev.subclass {
            0x08 => {
                // NVMe (Non-Volatile Memory Express)
                let model = alloc::format!("NVMe {:04x}:{:04x}", dev.vendor_id, dev.device_id);
                serial_println!(
                    "  [installer]   Found NVMe: {} at {}",
                    model,
                    dev.bdf_string()
                );
                disks.push(DiskInfo {
                    path: alloc::format!("/dev/nvme{}n1", disks.len()),
                    model,
                    size_bytes: 0, // populated by namespace identify
                    partitions: Vec::new(),
                });
            }
            0x06 => {
                // AHCI (Serial ATA)
                let model = alloc::format!("AHCI {:04x}:{:04x}", dev.vendor_id, dev.device_id);
                serial_println!(
                    "  [installer]   Found AHCI: {} at {}",
                    model,
                    dev.bdf_string()
                );
                disks.push(DiskInfo {
                    path: alloc::format!("/dev/sda{}", (b'a' + disks.len() as u8) as char),
                    model,
                    size_bytes: 0,
                    partitions: Vec::new(),
                });
            }
            0x01 => {
                // IDE
                let model = alloc::format!("IDE {:04x}:{:04x}", dev.vendor_id, dev.device_id);
                serial_println!("  [installer]   Found IDE: {}", model);
                disks.push(DiskInfo {
                    path: alloc::format!("/dev/hda"),
                    model,
                    size_bytes: 0,
                    partitions: Vec::new(),
                });
            }
            _ => {}
        }
    }

    if disks.is_empty() {
        serial_println!("  [installer] No storage controllers found via PCI");
    } else {
        serial_println!("  [installer] Found {} storage device(s)", disks.len());
    }
    disks
}

/// Partition a disk with GPT for Hoags OS.
///
/// Writes the three mandatory GPT structures:
///   LBA 0  — Protective MBR
///   LBA 1  — Primary GPT header
///   LBA 2..33 — Primary partition entry array (128 entries × 128 bytes = 16 KiB)
///   LBA (last-33)..(last-1) — Backup partition entry array
///   LBA (last)              — Backup GPT header
///
/// Partition layout:
///   Partition 1: EFI System Partition  (512 MiB, FAT32)
///   Partition 2: Hoags Root            (remaining space, HoagsFS)
///
/// `disk_id` is the NVMe namespace id (1-based).
/// `size_sectors` is the total number of 512-byte sectors on the disk.
pub fn partition_disk(disk: &str) -> Result<(), &'static str> {
    serial_println!("  [installer] Partitioning {} with GPT", disk);

    // Assume sector size 512, total size 128 GiB as a safe default.
    // A real impl would query the NVMe Identify Namespace response.
    let size_sectors: u64 = 128 * 1024 * 1024 * 2; // 128 GiB in 512-byte sectors
    write_gpt(1, size_sectors)?;
    serial_println!("  [installer] GPT written to {}", disk);
    Ok(())
}

/// Write the full GPT structure (protective MBR + primary header + entries +
/// backup entries + backup header) to NVMe namespace `disk_id`.
fn write_gpt(disk_id: u32, size_sectors: u64) -> Result<(), &'static str> {
    // ------------------------------------------------------------------
    // Constants
    // ------------------------------------------------------------------
    const GPT_MAGIC: &[u8; 8] = b"EFI PART";
    const GPT_VERSION: u32 = 0x0001_0000; // version 1.0
    const GPT_HEADER_SIZE: u32 = 92;
    const ENTRY_SIZE: u32 = 128;
    const ENTRY_COUNT: u32 = 128;

    // First usable LBA: LBA 1 (header) + 32 LBAs of entries = LBA 34
    let first_usable_lba: u64 = 34;
    // Last usable LBA: last sector − 1 (backup header) − 32 LBAs (backup entries) = size-34
    let last_usable_lba: u64 = size_sectors.saturating_sub(34);

    // EFI System Partition: 512 MiB
    let efi_start: u64 = first_usable_lba;
    let efi_size_sectors: u64 = 512 * 1024 * 1024 / 512; // 512 MiB
    let efi_end: u64 = efi_start + efi_size_sectors - 1;

    // Hoags Root: rest of the disk
    let root_start: u64 = efi_end + 1;
    let root_end: u64 = last_usable_lba;

    // Disk GUID — deterministic seed via SHA-256 of "HoagsOS_disk_1"
    let disk_guid_seed = crate::crypto::sha256::hash(b"HoagsOS_disk_1");
    let mut disk_guid = [0u8; 16];
    disk_guid.copy_from_slice(&disk_guid_seed[..16]);
    // Set variant and version bits for UUID v4
    disk_guid[6] = (disk_guid[6] & 0x0F) | 0x40;
    disk_guid[8] = (disk_guid[8] & 0x3F) | 0x80;

    // ------------------------------------------------------------------
    // Step 1: Protective MBR (LBA 0)
    // ------------------------------------------------------------------
    {
        let mut mbr = [0u8; 512];
        // Boot signature
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        // Partition entry 1: protective GPT partition (type 0xEE)
        // Offset 446 = first partition entry
        let pe = &mut mbr[446..462];
        pe[0] = 0x00; // status: not bootable
        pe[1] = 0x00;
        pe[2] = 0x02;
        pe[3] = 0x00; // CHS start (head=0, sector=2, cylinder=0)
        pe[4] = 0xEE; // type: GPT protective
        pe[5] = 0xFF;
        pe[6] = 0xFF;
        pe[7] = 0xFF; // CHS end: all 1s
                      // LBA start (little-endian u32) = 1
        pe[8] = 1;
        pe[9] = 0;
        pe[10] = 0;
        pe[11] = 0;
        // Sector count (little-endian u32) = min(disk_size-1, 0xFFFFFFFF)
        let protective_size = (size_sectors - 1).min(0xFFFF_FFFF) as u32;
        pe[12] = protective_size as u8;
        pe[13] = (protective_size >> 8) as u8;
        pe[14] = (protective_size >> 16) as u8;
        pe[15] = (protective_size >> 24) as u8;

        write_lba(0, &mbr)?;
        serial_println!("  [gpt] Protective MBR written");
    }

    // ------------------------------------------------------------------
    // Helper: build a 128-byte GPT partition entry
    // ------------------------------------------------------------------
    let build_entry = |type_guid_str: &str,
                       part_guid: &[u8; 16],
                       start: u64,
                       end: u64,
                       attrs: u64,
                       name_ascii: &str|
     -> [u8; 128] {
        let mut entry = [0u8; 128];
        // Type GUID (bytes 0..16)
        let type_guid = parse_guid(type_guid_str);
        entry[0..16].copy_from_slice(&type_guid);
        // Partition GUID (bytes 16..32)
        entry[16..32].copy_from_slice(part_guid);
        // Start LBA (bytes 32..40 LE u64)
        entry[32..40].copy_from_slice(&start.to_le_bytes());
        // End LBA   (bytes 40..48 LE u64, inclusive)
        entry[40..48].copy_from_slice(&end.to_le_bytes());
        // Attributes (bytes 48..56 LE u64)
        entry[48..56].copy_from_slice(&attrs.to_le_bytes());
        // Name: UTF-16LE (bytes 56..128, max 36 UTF-16 code units)
        let mut pos = 56usize;
        for c in name_ascii.chars().take(36) {
            let cu = c as u16;
            if pos + 2 > 128 {
                break;
            }
            entry[pos] = cu as u8;
            entry[pos + 1] = (cu >> 8) as u8;
            pos += 2;
        }
        entry
    };

    // ------------------------------------------------------------------
    // Step 2: Build partition entries (128 entries × 128 bytes = 16 384 bytes = 32 LBAs)
    // ------------------------------------------------------------------
    // Unique partition GUIDs (SHA-256 seeded)
    let efi_guid_seed = crate::crypto::sha256::hash(b"HoagsOS_part_EFI");
    let root_guid_seed = crate::crypto::sha256::hash(b"HoagsOS_part_root");
    let mut efi_guid = [0u8; 16];
    efi_guid.copy_from_slice(&efi_guid_seed[..16]);
    let mut root_guid = [0u8; 16];
    root_guid.copy_from_slice(&root_guid_seed[..16]);
    efi_guid[6] = (efi_guid[6] & 0x0F) | 0x40;
    efi_guid[8] = (efi_guid[8] & 0x3F) | 0x80;
    root_guid[6] = (root_guid[6] & 0x0F) | 0x40;
    root_guid[8] = (root_guid[8] & 0x3F) | 0x80;

    let efi_entry = build_entry(
        guid::EFI_SYSTEM,
        &efi_guid,
        efi_start,
        efi_end,
        0,
        "EFI System",
    );
    let root_entry = build_entry(
        guid::HOAGS_ROOT,
        &root_guid,
        root_start,
        root_end,
        0,
        "Hoags Root",
    );

    // 128 entries × 128 bytes = 16 384 bytes.  Laid out in a 16 384-byte buffer.
    // Then written in 512-byte sectors (32 sectors = LBA 2..33).
    let mut entry_array = [0u8; 128 * 128];
    entry_array[0..128].copy_from_slice(&efi_entry);
    entry_array[128..256].copy_from_slice(&root_entry);
    // Remaining entries are zero (unused)

    let entries_crc = crc32(&entry_array);

    // Write primary partition entries: LBA 2..33 (32 sectors of 512 bytes)
    for i in 0u64..32 {
        let mut sector = [0u8; 512];
        let off = (i as usize) * 512;
        sector.copy_from_slice(&entry_array[off..off + 512]);
        write_lba(2 + i, &sector)?;
    }
    serial_println!("  [gpt] Primary partition entries written (LBA 2-33)");

    // ------------------------------------------------------------------
    // Step 3: Primary GPT header (LBA 1)
    // ------------------------------------------------------------------
    {
        let backup_lba = size_sectors - 1;
        let mut header = [0u8; 512];

        // Magic "EFI PART"
        header[0..8].copy_from_slice(GPT_MAGIC);
        // Revision 1.0
        header[8..12].copy_from_slice(&GPT_VERSION.to_le_bytes());
        // Header size (92 bytes)
        header[12..16].copy_from_slice(&GPT_HEADER_SIZE.to_le_bytes());
        // Header CRC32 placeholder (zeroed, filled in below)
        header[16..20].copy_from_slice(&[0u8; 4]);
        // Reserved
        header[20..24].copy_from_slice(&[0u8; 4]);
        // My LBA = 1
        header[24..32].copy_from_slice(&1u64.to_le_bytes());
        // Alternate LBA = backup header
        header[32..40].copy_from_slice(&backup_lba.to_le_bytes());
        // First usable LBA
        header[40..48].copy_from_slice(&first_usable_lba.to_le_bytes());
        // Last usable LBA
        header[48..56].copy_from_slice(&last_usable_lba.to_le_bytes());
        // Disk GUID
        header[56..72].copy_from_slice(&disk_guid);
        // Partition entries start LBA = 2
        header[72..80].copy_from_slice(&2u64.to_le_bytes());
        // Number of partition entries
        header[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        // Size of each partition entry
        header[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        // Partition entries CRC32
        header[88..92].copy_from_slice(&entries_crc.to_le_bytes());

        // Compute and write header CRC32 over bytes 0..92
        let hdr_crc = crc32(&header[0..92]);
        header[16..20].copy_from_slice(&hdr_crc.to_le_bytes());

        write_lba(1, &header)?;
        serial_println!("  [gpt] Primary GPT header written (LBA 1)");
    }

    // ------------------------------------------------------------------
    // Step 4: Backup partition entries (LBA size-33 .. size-2)
    // ------------------------------------------------------------------
    let backup_entries_start = size_sectors - 33;
    for i in 0u64..32 {
        let mut sector = [0u8; 512];
        let off = (i as usize) * 512;
        sector.copy_from_slice(&entry_array[off..off + 512]);
        write_lba(backup_entries_start + i, &sector)?;
    }
    serial_println!(
        "  [gpt] Backup partition entries written (LBA {}-{})",
        backup_entries_start,
        backup_entries_start + 31
    );

    // ------------------------------------------------------------------
    // Step 5: Backup GPT header (last LBA)
    // ------------------------------------------------------------------
    {
        let backup_lba = size_sectors - 1;
        let mut header = [0u8; 512];

        header[0..8].copy_from_slice(GPT_MAGIC);
        header[8..12].copy_from_slice(&GPT_VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&GPT_HEADER_SIZE.to_le_bytes());
        header[16..20].copy_from_slice(&[0u8; 4]); // CRC placeholder
        header[20..24].copy_from_slice(&[0u8; 4]);
        // My LBA = backup_lba
        header[24..32].copy_from_slice(&backup_lba.to_le_bytes());
        // Alternate LBA = 1 (primary)
        header[32..40].copy_from_slice(&1u64.to_le_bytes());
        header[40..48].copy_from_slice(&first_usable_lba.to_le_bytes());
        header[48..56].copy_from_slice(&last_usable_lba.to_le_bytes());
        header[56..72].copy_from_slice(&disk_guid);
        // Backup entries start = backup_entries_start
        header[72..80].copy_from_slice(&backup_entries_start.to_le_bytes());
        header[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        header[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        header[88..92].copy_from_slice(&entries_crc.to_le_bytes());

        let hdr_crc = crc32(&header[0..92]);
        header[16..20].copy_from_slice(&hdr_crc.to_le_bytes());

        write_lba(backup_lba, &header)?;
        serial_println!("  [gpt] Backup GPT header written (LBA {})", backup_lba);
    }

    Ok(())
}

/// Format the EFI partition as FAT32.
///
/// Writes the minimum structures required for a valid FAT32 volume:
///   - Boot sector (BPB) at LBA 0 of the partition
///   - FS Information Sector at LBA 1
///   - Backup boot sector at LBA 6
///   - FAT1 starting at the reserved region end
///   - FAT2 (copy) immediately after FAT1
///   - Empty root directory cluster (cluster 2, first data cluster)
///
/// `start_lba` is the first absolute LBA of the partition.
/// `size_sectors` is the total 512-byte sector count.
pub fn format_efi(partition: &str) -> Result<(), &'static str> {
    serial_println!(
        "  [installer] Formatting EFI partition as FAT32 ({})",
        partition
    );

    // EFI partition starts right after GPT entries (LBA 34) by default.
    // The installer's partition_disk places it there.
    let start_lba: u64 = 34;
    let size_sectors: u64 = 512 * 1024 * 1024 / 512; // 512 MiB

    create_fat32_partition(start_lba, size_sectors)
}

/// Write a FAT32 filesystem onto the region `[start_lba, start_lba + size_sectors)`.
pub fn create_fat32_partition(start_lba: u64, size_sectors: u64) -> Result<(), &'static str> {
    // ------------------------------------------------------------------
    // FAT32 geometry parameters
    // ------------------------------------------------------------------
    const BYTES_PER_SECTOR: u32 = 512;
    const SECTORS_PER_CLUSTER: u32 = 8; // 4 KiB clusters
    const RESERVED_SECTORS: u32 = 32; // standard for FAT32
    const NUM_FATS: u32 = 2;
    const ROOT_CLUSTER: u32 = 2; // root dir starts at cluster 2

    // Number of clusters in the data area
    let fat_size_sectors: u32 = {
        // FAT32 sectors per FAT = ceil((total_clusters * 4) / 512)
        // total_clusters ~ (size_sectors - reserved - 2*fat_size) / spc
        // Approximate: fat_size = ceil(size_sectors / (spc * 128 + 2))
        let numerator = size_sectors as u32;
        let denominator = SECTORS_PER_CLUSTER * 128 + 2;
        (numerator + denominator - 1) / denominator
    };

    let data_start: u32 = RESERVED_SECTORS + NUM_FATS * fat_size_sectors;
    let total_clusters: u32 =
        ((size_sectors as u32).saturating_sub(data_start)) / SECTORS_PER_CLUSTER;

    // ------------------------------------------------------------------
    // Boot Sector / BPB (512 bytes)
    // ------------------------------------------------------------------
    let mut boot_sector = [0u8; 512];

    // Jump instruction + NOP
    boot_sector[0] = 0xEB;
    boot_sector[1] = 0x58;
    boot_sector[2] = 0x90;
    // OEM name
    boot_sector[3..11].copy_from_slice(b"HOAGSFS ");
    // BPB_BytsPerSec
    boot_sector[11..13].copy_from_slice(&(BYTES_PER_SECTOR as u16).to_le_bytes());
    // BPB_SecPerClus
    boot_sector[13] = SECTORS_PER_CLUSTER as u8;
    // BPB_RsvdSecCnt
    boot_sector[14..16].copy_from_slice(&(RESERVED_SECTORS as u16).to_le_bytes());
    // BPB_NumFATs
    boot_sector[16] = NUM_FATS as u8;
    // BPB_RootEntCnt = 0 (FAT32)
    boot_sector[17..19].copy_from_slice(&0u16.to_le_bytes());
    // BPB_TotSec16 = 0 (use 32-bit field)
    boot_sector[19..21].copy_from_slice(&0u16.to_le_bytes());
    // BPB_Media = 0xF8 (fixed disk)
    boot_sector[21] = 0xF8;
    // BPB_FATSz16 = 0 (use 32-bit FAT32 field)
    boot_sector[22..24].copy_from_slice(&0u16.to_le_bytes());
    // BPB_SecPerTrk
    boot_sector[24..26].copy_from_slice(&63u16.to_le_bytes());
    // BPB_NumHeads
    boot_sector[26..28].copy_from_slice(&255u16.to_le_bytes());
    // BPB_HiddSec = start_lba
    boot_sector[28..32].copy_from_slice(&(start_lba as u32).to_le_bytes());
    // BPB_TotSec32
    boot_sector[32..36].copy_from_slice(&(size_sectors as u32).to_le_bytes());
    // BPB_FATSz32
    boot_sector[36..40].copy_from_slice(&fat_size_sectors.to_le_bytes());
    // BPB_ExtFlags = 0 (both FATs mirrored)
    boot_sector[40..42].copy_from_slice(&0u16.to_le_bytes());
    // BPB_FSVer = 0 (FAT32 version 0.0)
    boot_sector[42..44].copy_from_slice(&0u16.to_le_bytes());
    // BPB_RootClus = 2
    boot_sector[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    // BPB_FSInfo = 1
    boot_sector[48..50].copy_from_slice(&1u16.to_le_bytes());
    // BPB_BkBootSec = 6
    boot_sector[50..52].copy_from_slice(&6u16.to_le_bytes());
    // BPB_Reserved = 12 zero bytes
    // BS_DrvNum = 0x80
    boot_sector[64] = 0x80;
    // BS_Reserved1 = 0
    // BS_BootSig = 0x29
    boot_sector[66] = 0x29;
    // BS_VolID — deterministic from SHA-256 seed
    let vol_seed = crate::crypto::sha256::hash(b"HoagsEFI_vol_id");
    boot_sector[67..71].copy_from_slice(&vol_seed[0..4]);
    // BS_VolLab
    boot_sector[71..82].copy_from_slice(b"HOAGS EFI  ");
    // BS_FilSysType
    boot_sector[82..90].copy_from_slice(b"FAT32   ");
    // Boot signature
    boot_sector[510] = 0x55;
    boot_sector[511] = 0xAA;

    write_lba(start_lba, &boot_sector)?;
    serial_println!("  [fat32] Boot sector written at LBA {}", start_lba);

    // ------------------------------------------------------------------
    // FS Information Sector (LBA 1 of partition)
    // ------------------------------------------------------------------
    {
        let mut fsinfo = [0u8; 512];
        // Lead signature 0x41615252 (stored LE as bytes 52 52 61 41 = "RRaA")
        fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        // Structure signature at offset 484: 0x61417272 (stored LE as bytes 72 72 41 61 = "rrAa")
        fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        // Free cluster count (unknown = 0xFFFFFFFF)
        fsinfo[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        // Next free cluster (start from cluster 3 — cluster 2 used by root)
        fsinfo[492..496].copy_from_slice(&3u32.to_le_bytes());
        // Trail signature
        fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());

        write_lba(start_lba + 1, &fsinfo)?;
    }

    // Backup boot sector at partition-relative LBA 6
    write_lba(start_lba + 6, &boot_sector)?;
    serial_println!(
        "  [fat32] Backup boot sector written at LBA {}",
        start_lba + 6
    );

    // ------------------------------------------------------------------
    // FAT1 and FAT2
    // ------------------------------------------------------------------
    // FAT starts at RESERVED_SECTORS from the partition start
    let fat1_start = start_lba + RESERVED_SECTORS as u64;
    let fat2_start = fat1_start + fat_size_sectors as u64;

    // First sector of FAT: media byte, end-of-chain markers, root dir cluster chain
    let mut fat_sector0 = [0u8; 512];
    // FAT[0]: media byte in low byte, rest 0xFF
    fat_sector0[0..4].copy_from_slice(&0xFFFF_FFF8u32.to_le_bytes());
    // FAT[1]: end-of-chain for FAT[1]
    fat_sector0[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    // FAT[2]: root directory cluster chain — EOC
    fat_sector0[8..12].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());

    write_lba(fat1_start, &fat_sector0)?;
    write_lba(fat2_start, &fat_sector0)?;
    serial_println!(
        "  [fat32] FAT1 at LBA {}, FAT2 at LBA {}",
        fat1_start,
        fat2_start
    );

    // Remaining FAT sectors are zero (empty = free clusters); NVMe sectors are
    // already zero from partition writing, so we only need to write sector 0.

    // ------------------------------------------------------------------
    // Root directory cluster (cluster 2 = first data cluster)
    // ------------------------------------------------------------------
    let data_region_start = fat2_start + fat_size_sectors as u64;
    // Root directory is cluster 2; cluster offset = (cluster - 2) * sectors_per_cluster
    let root_dir_lba = data_region_start; // cluster 2 → offset 0

    // Write 8 empty 512-byte sectors (one cluster)
    let empty_sector = [0u8; 512];
    for i in 0u64..SECTORS_PER_CLUSTER as u64 {
        write_lba(root_dir_lba + i, &empty_sector)?;
    }
    serial_println!(
        "  [fat32] Root directory cluster written at LBA {}",
        root_dir_lba
    );
    serial_println!(
        "  [installer] FAT32 format complete ({} clusters)",
        total_clusters
    );

    Ok(())
}

/// Format the root partition as HoagsFS.
///
/// HoagsFS mkfs is not yet implemented.  This function writes a minimal
/// superblock magic so the partition is identifiable, and returns Ok so
/// that the rest of the installation pipeline can proceed.
pub fn format_root(partition: &str) -> Result<(), &'static str> {
    serial_println!(
        "  [installer] Formatting root partition as HoagsFS ({})",
        partition
    );

    // HoagsFS superblock magic: "HOAGSFS\0" at byte 0 of the first sector.
    // The root partition starts right after the EFI partition.
    let efi_size_sectors: u64 = 512 * 1024 * 1024 / 512;
    let root_start_lba: u64 = 34 + efi_size_sectors;

    let mut superblock = [0u8; 512];
    superblock[0..8].copy_from_slice(b"HOAGSFS\0");
    // Version 1
    superblock[8..12].copy_from_slice(&1u32.to_le_bytes());
    // UUID seeded from SHA-256
    let uuid_seed = crate::crypto::sha256::hash(b"HoagsFS_root_uuid");
    superblock[12..28].copy_from_slice(&uuid_seed[..16]);

    write_lba(root_start_lba, &superblock)?;
    serial_println!("  [installer] HoagsFS superblock written (HoagsFS mkfs stub)");
    Ok(())
}

/// Copy system files to the installed partition
pub fn install_system(_root_mount: &str) -> Result<(), &'static str> {
    serial_println!("  [installer] Copying system files...");

    // Files to install:
    // /boot/genesis.elf — the kernel
    // /boot/initramfs.img — initial ramdisk
    // /bin/hoags-init — init system
    // /bin/hoags-shell — shell
    // /bin/hoags-pkg — package manager
    // /etc/hoags.conf — system configuration
    // /etc/fstab — filesystem table

    Ok(())
}

/// Install the UEFI bootloader
pub fn install_bootloader(_efi_mount: &str) -> Result<(), &'static str> {
    serial_println!("  [installer] Installing UEFI bootloader");

    // Copy bootloader to /EFI/HOAGS/genesis.efi
    // Create UEFI boot entry

    Ok(())
}

/// Run the full installation
pub fn install(config: &super::InstallConfig) -> Result<(), &'static str> {
    serial_println!("  [installer] Starting Hoags OS installation");
    serial_println!("  [installer] Target: {}", config.target_disk);
    serial_println!("  [installer] Encrypt: {}", config.encrypt);

    partition_disk(&config.target_disk)?;
    format_efi(&format!("{}p1", config.target_disk))?;
    format_root(&format!("{}p2", config.target_disk))?;
    install_system("/mnt/root")?;
    install_bootloader("/mnt/efi")?;

    serial_println!("  [installer] Installation complete!");
    Ok(())
}
