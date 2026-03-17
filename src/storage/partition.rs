/// Partition table parsing (GPT, MBR)
///
/// Part of the AIOS storage layer.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub enum PartitionScheme {
    Mbr,
    Gpt,
}

pub struct PartitionEntry {
    pub start_lba: u64,
    pub size_lba: u64,
    pub partition_type: u8,
    pub bootable: bool,
}

pub struct PartitionTable {
    scheme: PartitionScheme,
    table_entries: Vec<PartitionEntry>,
    disk_signature: u32,
}

impl PartitionTable {
    /// Parse a partition table from raw disk bytes.
    /// Expects at least 512 bytes for MBR; checks for GPT protective MBR
    /// and parses GPT header + entries if present.
    pub fn parse(disk: &[u8]) -> Result<Self, ()> {
        if disk.len() < 512 {
            return Err(());
        }

        // Check MBR boot signature at bytes 510-511
        if disk[510] != 0x55 || disk[511] != 0xAA {
            serial_println!("  [partition] Invalid MBR signature");
            return Err(());
        }

        // Read disk signature from MBR offset 440
        let disk_signature = u32::from_le_bytes([disk[440], disk[441], disk[442], disk[443]]);

        // Check if this is a GPT protective MBR:
        // First partition entry type == 0xEE indicates GPT
        let first_type = disk[0x1BE + 4];
        if first_type == 0xEE && disk.len() >= 1024 {
            return Self::parse_gpt(disk, disk_signature);
        }

        // Parse MBR partition table (4 entries at offset 0x1BE)
        let mut entries = Vec::new();
        for i in 0..4 {
            let base = 0x1BE + i * 16;
            let status = disk[base];
            let ptype = disk[base + 4];

            // Skip empty entries
            if ptype == 0x00 {
                continue;
            }

            let start_lba = u32::from_le_bytes([
                disk[base + 8],
                disk[base + 9],
                disk[base + 10],
                disk[base + 11],
            ]) as u64;

            let size_lba = u32::from_le_bytes([
                disk[base + 12],
                disk[base + 13],
                disk[base + 14],
                disk[base + 15],
            ]) as u64;

            entries.push(PartitionEntry {
                start_lba,
                size_lba,
                partition_type: ptype,
                bootable: status == 0x80,
            });
        }

        serial_println!("  [partition] Parsed MBR with {} partitions", entries.len());

        Ok(PartitionTable {
            scheme: PartitionScheme::Mbr,
            table_entries: entries,
            disk_signature,
        })
    }

    /// Parse a GPT partition table.
    fn parse_gpt(disk: &[u8], disk_signature: u32) -> Result<Self, ()> {
        // GPT header starts at LBA 1 (byte 512)
        if disk.len() < 1024 {
            return Err(());
        }

        let hdr = &disk[512..];

        // Check GPT signature: "EFI PART" = 0x5452415020494645
        if hdr.len() < 92 {
            return Err(());
        }
        let sig = u64::from_le_bytes([
            hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
        ]);
        if sig != 0x5452415020494645 {
            serial_println!("  [partition] Invalid GPT signature");
            return Err(());
        }

        // Number of partition entries (offset 80 in header)
        let num_entries = u32::from_le_bytes([hdr[80], hdr[81], hdr[82], hdr[83]]);

        // Size of each partition entry (offset 84)
        let entry_size = u32::from_le_bytes([hdr[84], hdr[85], hdr[86], hdr[87]]);

        // Partition entries start at LBA 2 (byte 1024)
        let entries_start = 1024usize;
        let mut entries = Vec::new();

        let max_entries = num_entries.min(128) as usize;
        for i in 0..max_entries {
            let offset = entries_start + i * (entry_size as usize);
            if offset + 128 > disk.len() {
                break;
            }
            let entry = &disk[offset..];

            // Check if entry is used: partition type GUID must not be all zeros
            let mut all_zero = true;
            for j in 0..16 {
                if entry[j] != 0 {
                    all_zero = false;
                    break;
                }
            }
            if all_zero {
                continue;
            }

            // Starting LBA at offset 32 in entry
            let start_lba = u64::from_le_bytes([
                entry[32], entry[33], entry[34], entry[35], entry[36], entry[37], entry[38],
                entry[39],
            ]);

            // Ending LBA at offset 40
            let end_lba = u64::from_le_bytes([
                entry[40], entry[41], entry[42], entry[43], entry[44], entry[45], entry[46],
                entry[47],
            ]);

            // Attributes at offset 48
            let attrs = u64::from_le_bytes([
                entry[48], entry[49], entry[50], entry[51], entry[52], entry[53], entry[54],
                entry[55],
            ]);

            // Map the type GUID to a simple u8 type code
            // Use the first byte of the partition type GUID as a shorthand
            let partition_type = entry[0];

            entries.push(PartitionEntry {
                start_lba,
                size_lba: end_lba.saturating_sub(start_lba) + 1,
                partition_type,
                bootable: (attrs & 0x04) != 0, // legacy BIOS bootable attribute
            });
        }

        serial_println!("  [partition] Parsed GPT with {} partitions", entries.len());

        Ok(PartitionTable {
            scheme: PartitionScheme::Gpt,
            table_entries: entries,
            disk_signature,
        })
    }

    pub fn entries(&self) -> &[PartitionEntry] {
        &self.table_entries
    }

    /// Return the partition scheme.
    pub fn scheme(&self) -> &PartitionScheme {
        &self.scheme
    }

    /// Return the disk signature (MBR) or 0 for GPT.
    pub fn disk_signature(&self) -> u32 {
        self.disk_signature
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PARTITION_SUBSYSTEM: Mutex<bool> = Mutex::new(false);

pub fn init() {
    let mut guard = PARTITION_SUBSYSTEM.lock();
    *guard = true;
    serial_println!("  [storage] Partition table parser initialized");
}
