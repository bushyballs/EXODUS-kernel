use super::vfs::{DirEntry, FileOps, FileSystem, FileType, FsError, Inode};
/// Native HoagsFS implementation for Genesis
///
/// This is the Rust port of the HoagsFS filesystem, originally written in C
/// as a Linux kernel module. Same on-disk format, pure Rust implementation.
///
/// On-disk layout:
///   Block 0: Superblock (magic, block count, inode count, free counts, root inode)
///   Block 1..N: Inode table (packed hoagsfs_inode structs)
///   Block N+1..M: Block bitmap (1 bit per data block)
///   Block M+1..K: Inode bitmap (1 bit per inode)
///   Block K+1..end: Data blocks
///
/// Design:
///   - Block size: 4096 bytes (same as page size)
///   - Max file size: ~4GB (direct + single + double indirect blocks)
///   - Directory entries: fixed 256-byte records (name + inode number)
///
/// Inspired by: ext2 (on-disk structure), Linux VFS integration, our own
/// C implementation in hoags-kernel/modules/hoagsfs/. All code is original.
use crate::serial_println;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// HoagsFS magic number (identifies a valid HoagsFS superblock)
pub const HOAGSFS_MAGIC: u32 = 0x484F4147; // "HOAG"

/// Block size (4 KB)
pub const BLOCK_SIZE: usize = 4096;

/// Maximum filename length
pub const MAX_NAME_LEN: usize = 248;

/// Number of direct block pointers in an inode
pub const DIRECT_BLOCKS: usize = 12;

/// Inode file type constants (stored on disk)
pub const HOAGSFS_FT_REG: u16 = 1;
pub const HOAGSFS_FT_DIR: u16 = 2;
pub const HOAGSFS_FT_LNK: u16 = 3;

/// On-disk superblock structure
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct HoagsSuperblock {
    pub magic: u32,
    pub block_count: u32,
    pub inode_count: u32,
    pub free_blocks: u32,
    pub free_inodes: u32,
    pub block_size: u32,
    pub root_inode: u32,
    pub inode_table_block: u32,
    pub block_bitmap_block: u32,
    pub inode_bitmap_block: u32,
    pub first_data_block: u32,
    pub label: [u8; 64],
    // Padding to fill rest of first block
}

/// On-disk inode structure (128 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct HoagsInode {
    pub mode: u16,
    pub file_type: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub nlink: u32,
    pub blocks: u32,
    pub flags: u32,
    pub ctime: u64,
    pub mtime: u64,
    pub atime: u64,
    pub direct: [u32; DIRECT_BLOCKS],
    pub indirect: u32,
    pub double_indirect: u32,
    pub _reserved: [u8; 8],
}

/// On-disk directory entry (256 bytes)
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct HoagsDirEntry {
    pub inode: u32,
    pub rec_len: u16,
    pub name_len: u8,
    pub file_type: u8,
    pub name: [u8; MAX_NAME_LEN],
}

/// Block device trait — abstracts over NVMe, AHCI, RAM disk, etc.
pub trait BlockDevice: Send + Sync {
    /// Read a block into buf. Block number is absolute.
    fn read_block(&self, block_nr: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError>;
    /// Write buf to a block.
    fn write_block(&self, block_nr: u64, buf: &[u8; BLOCK_SIZE]) -> Result<(), FsError>;
}

/// RAM block device (for testing before we have real storage drivers)
pub struct RamBlockDevice {
    data: Vec<[u8; BLOCK_SIZE]>,
}

impl RamBlockDevice {
    pub fn new(block_count: usize) -> Self {
        RamBlockDevice {
            data: alloc::vec![[0u8; BLOCK_SIZE]; block_count],
        }
    }
}

impl BlockDevice for RamBlockDevice {
    fn read_block(&self, block_nr: u64, buf: &mut [u8; BLOCK_SIZE]) -> Result<(), FsError> {
        let nr = block_nr as usize;
        if nr >= self.data.len() {
            return Err(FsError::IoError);
        }
        buf.copy_from_slice(&self.data[nr]);
        Ok(())
    }

    fn write_block(&self, _block_nr: u64, _buf: &[u8; BLOCK_SIZE]) -> Result<(), FsError> {
        // Note: need interior mutability for real use. Placeholder.
        Err(FsError::NotSupported)
    }
}

/// Mounted HoagsFS instance
pub struct HoagsFileSystem {
    pub device: Box<dyn BlockDevice>,
    pub superblock: HoagsSuperblock,
}

impl HoagsFileSystem {
    /// Mount a HoagsFS from a block device
    pub fn mount(device: Box<dyn BlockDevice>) -> Result<Self, FsError> {
        // Read superblock from block 0
        let mut block = [0u8; BLOCK_SIZE];
        device.read_block(0, &mut block)?;

        let sb = unsafe { *(block.as_ptr() as *const HoagsSuperblock) };

        // Validate magic (copy from packed struct to avoid unaligned reference)
        let magic = sb.magic;
        if magic != HOAGSFS_MAGIC {
            serial_println!(
                "  HoagsFS: invalid magic {:#x} (expected {:#x})",
                magic,
                HOAGSFS_MAGIC
            );
            return Err(FsError::InvalidArgument);
        }

        let block_count = sb.block_count;
        let inode_count = sb.inode_count;
        serial_println!(
            "  HoagsFS: mounted — {} blocks, {} inodes",
            block_count,
            inode_count
        );

        Ok(HoagsFileSystem {
            device,
            superblock: sb,
        })
    }

    /// Read an inode from disk
    pub fn read_inode(&self, ino: u32) -> Result<HoagsInode, FsError> {
        let inodes_per_block = BLOCK_SIZE / core::mem::size_of::<HoagsInode>();
        let block_nr =
            self.superblock.inode_table_block as u64 + (ino as u64 / inodes_per_block as u64);
        let offset = (ino as usize % inodes_per_block) * core::mem::size_of::<HoagsInode>();

        let mut block = [0u8; BLOCK_SIZE];
        self.device.read_block(block_nr, &mut block)?;

        let inode = unsafe { *(block.as_ptr().add(offset) as *const HoagsInode) };

        Ok(inode)
    }

    /// Read a data block
    pub fn read_data_block(&self, block_nr: u32) -> Result<[u8; BLOCK_SIZE], FsError> {
        let mut block = [0u8; BLOCK_SIZE];
        self.device.read_block(block_nr as u64, &mut block)?;
        Ok(block)
    }

    /// Read all data bytes described by an inode (direct blocks only + single indirect)
    pub fn read_inode_data(&self, inode: &HoagsInode) -> Result<Vec<u8>, FsError> {
        let size = inode.size as usize;
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut data = alloc::vec![0u8; size];
        let mut offset = 0usize;

        // Direct blocks (0..DIRECT_BLOCKS)
        for i in 0..DIRECT_BLOCKS {
            if offset >= size {
                break;
            }
            let block_nr = inode.direct[i];
            if block_nr == 0 {
                break;
            }
            let block = self.read_data_block(block_nr)?;
            let copy_len = (size - offset).min(BLOCK_SIZE);
            data[offset..offset + copy_len].copy_from_slice(&block[..copy_len]);
            offset = offset.saturating_add(copy_len);
        }

        // Single indirect block
        if offset < size && inode.indirect != 0 {
            let indirect_block = self.read_data_block(inode.indirect)?;
            let ptrs_per_block = BLOCK_SIZE / 4;
            for i in 0..ptrs_per_block {
                if offset >= size {
                    break;
                }
                let block_nr = u32::from_le_bytes([
                    indirect_block[i * 4],
                    indirect_block[i * 4 + 1],
                    indirect_block[i * 4 + 2],
                    indirect_block[i * 4 + 3],
                ]);
                if block_nr == 0 {
                    break;
                }
                let block = self.read_data_block(block_nr)?;
                let copy_len = (size - offset).min(BLOCK_SIZE);
                data[offset..offset + copy_len].copy_from_slice(&block[..copy_len]);
                offset = offset.saturating_add(copy_len);
            }
        }

        Ok(data)
    }

    /// Parse directory entries from raw data bytes
    pub fn parse_dir_entries(data: &[u8]) -> Vec<DirEntry> {
        let entry_size = core::mem::size_of::<HoagsDirEntry>();
        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset + entry_size <= data.len() {
            let de: HoagsDirEntry = unsafe {
                core::ptr::read_unaligned(data[offset..].as_ptr() as *const HoagsDirEntry)
            };
            if de.inode != 0 && de.name_len > 0 {
                let name_len = (de.name_len as usize).min(MAX_NAME_LEN);
                let name = core::str::from_utf8(&de.name[..name_len]).unwrap_or("?");
                if name != "." && name != ".." {
                    let ft = match de.file_type {
                        2 => FileType::Directory,
                        3 => FileType::Symlink,
                        _ => FileType::Regular,
                    };
                    entries.push(DirEntry {
                        name: String::from(name),
                        ino: de.inode as u64,
                        file_type: ft,
                    });
                }
            }
            let rec_len = de.rec_len as usize;
            if rec_len == 0 || offset + rec_len > data.len() {
                break;
            }
            offset = offset.saturating_add(rec_len);
        }
        entries
    }
}

/// Arc-wrapped HoagsFS — shared by all inode ops so they can call back
/// into the filesystem for block I/O.
pub struct ArcHoagsFs(Arc<HoagsFileSystem>);

impl ArcHoagsFs {
    pub fn new(fs: HoagsFileSystem) -> Self {
        ArcHoagsFs(Arc::new(fs))
    }
}

impl FileSystem for ArcHoagsFs {
    fn name(&self) -> &str {
        "hoagsfs"
    }

    fn root(&self) -> Result<Inode, FsError> {
        let ino_nr = self.0.superblock.root_inode;
        let inode = self.0.read_inode(ino_nr)?;
        inode_to_vfs_inode(&self.0, ino_nr, &inode)
    }
}

/// Convert an on-disk HoagsInode + inode number into a VFS Inode.
fn inode_to_vfs_inode(
    fs: &Arc<HoagsFileSystem>,
    ino_nr: u32,
    inode: &HoagsInode,
) -> Result<Inode, FsError> {
    let ft = match inode.file_type {
        HOAGSFS_FT_DIR => FileType::Directory,
        HOAGSFS_FT_LNK => FileType::Symlink,
        _ => FileType::Regular,
    };
    let ops: Box<dyn FileOps> = match ft {
        FileType::Directory => Box::new(HoagsDirOps {
            fs: Arc::clone(fs),
            ino_nr,
        }),
        _ => Box::new(HoagsFileOps {
            fs: Arc::clone(fs),
            ino_nr,
        }),
    };
    Ok(Inode {
        ino: ino_nr as u64,
        file_type: ft,
        size: inode.size,
        mode: inode.mode as u32,
        uid: inode.uid,
        gid: inode.gid,
        nlink: inode.nlink,
        ops,
        rdev: 0,
        blocks: inode.blocks as u64,
        atime: inode.atime,
        mtime: inode.mtime,
        ctime: inode.ctime,
        crtime: inode.ctime,
    })
}

// ---------------------------------------------------------------------------
// Directory FileOps
// ---------------------------------------------------------------------------

/// FileOps for a HoagsFS directory inode.
struct HoagsDirOps {
    fs: Arc<HoagsFileSystem>,
    ino_nr: u32,
}

impl core::fmt::Debug for HoagsDirOps {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HoagsDirOps")
            .field("ino_nr", &self.ino_nr)
            .finish()
    }
}

impl FileOps for HoagsDirOps {
    fn read(&self, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::IsADirectory)
    }

    fn size(&self) -> u64 {
        self.fs.read_inode(self.ino_nr).map(|i| i.size).unwrap_or(0)
    }

    fn readdir(&self) -> Result<Vec<DirEntry>, FsError> {
        let inode = self.fs.read_inode(self.ino_nr)?;
        let data = self.fs.read_inode_data(&inode)?;
        Ok(HoagsFileSystem::parse_dir_entries(&data))
    }

    fn lookup(&self, name: &str) -> Result<Inode, FsError> {
        let inode = self.fs.read_inode(self.ino_nr)?;
        let data = self.fs.read_inode_data(&inode)?;

        // Scan raw on-disk directory entries to find `name`
        let entry_size = core::mem::size_of::<HoagsDirEntry>();
        let mut offset = 0usize;
        while offset + entry_size <= data.len() {
            let de: HoagsDirEntry = unsafe {
                core::ptr::read_unaligned(data[offset..].as_ptr() as *const HoagsDirEntry)
            };
            if de.inode != 0 && de.name_len > 0 {
                let name_len = (de.name_len as usize).min(MAX_NAME_LEN);
                let entry_name = core::str::from_utf8(&de.name[..name_len]).unwrap_or("");
                if entry_name == name {
                    let child_inode = self.fs.read_inode(de.inode)?;
                    return inode_to_vfs_inode(&self.fs, de.inode, &child_inode);
                }
            }
            let rec_len = de.rec_len as usize;
            if rec_len == 0 {
                break;
            }
            offset = offset.saturating_add(rec_len);
        }
        Err(FsError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Regular file FileOps
// ---------------------------------------------------------------------------

/// FileOps for a HoagsFS regular file or symlink inode.
struct HoagsFileOps {
    fs: Arc<HoagsFileSystem>,
    ino_nr: u32,
}

impl core::fmt::Debug for HoagsFileOps {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HoagsFileOps")
            .field("ino_nr", &self.ino_nr)
            .finish()
    }
}

impl FileOps for HoagsFileOps {
    fn read(&self, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let inode = self.fs.read_inode(self.ino_nr)?;
        let data = self.fs.read_inode_data(&inode)?;
        let start = (offset as usize).min(data.len());
        let end = (start + buf.len()).min(data.len());
        let copy_len = end - start;
        if copy_len == 0 {
            return Ok(0);
        }
        buf[..copy_len].copy_from_slice(&data[start..end]);
        Ok(copy_len)
    }

    fn write(&self, _offset: u64, _buf: &[u8]) -> Result<usize, FsError> {
        // HoagsFS write path requires mutable block device; not yet wired.
        Err(FsError::NotSupported)
    }

    fn size(&self) -> u64 {
        self.fs.read_inode(self.ino_nr).map(|i| i.size).unwrap_or(0)
    }
}
