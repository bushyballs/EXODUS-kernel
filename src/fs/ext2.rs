use crate::fs::vfs::FsError;
/// ext2 filesystem driver for Genesis
///
/// Read-only implementation of the classic Linux ext2 filesystem.
/// Supports reading files, directories, symlinks, and metadata.
///
/// On-disk layout:
///   Block 0: Boot block (1024 bytes, unused)
///   Block 1: Superblock (1024 bytes at offset 1024)
///   Block 2+: Block group descriptor table
///   Each block group: block bitmap, inode bitmap, inode table, data blocks
use crate::serial_println;
use alloc::string::String;
use alloc::vec::Vec;

/// ext2 magic number
const EXT2_MAGIC: u16 = 0xEF53;

/// Default block size (1024 bytes, configurable via superblock)
const DEFAULT_BLOCK_SIZE: usize = 1024;

/// Inode types (from i_mode)
const S_IFREG: u16 = 0x8000; // Regular file
const S_IFDIR: u16 = 0x4000; // Directory
const S_IFLNK: u16 = 0xA000; // Symbolic link

/// ext2 superblock (on-disk format, at offset 1024)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2Superblock {
    pub s_inodes_count: u32,
    pub s_blocks_count: u32,
    pub s_r_blocks_count: u32,
    pub s_free_blocks_count: u32,
    pub s_free_inodes_count: u32,
    pub s_first_data_block: u32,
    pub s_log_block_size: u32, // block_size = 1024 << s_log_block_size
    pub s_log_frag_size: u32,
    pub s_blocks_per_group: u32,
    pub s_frags_per_group: u32,
    pub s_inodes_per_group: u32,
    pub s_mtime: u32,
    pub s_wtime: u32,
    pub s_mnt_count: u16,
    pub s_max_mnt_count: u16,
    pub s_magic: u16,
    pub s_state: u16,
    pub s_errors: u16,
    pub s_minor_rev_level: u16,
    pub s_lastcheck: u32,
    pub s_checkinterval: u32,
    pub s_creator_os: u32,
    pub s_rev_level: u32,
    pub s_def_resuid: u16,
    pub s_def_resgid: u16,
    // Extended fields (rev 1+)
    pub s_first_ino: u32,
    pub s_inode_size: u16,
}

/// Block group descriptor (on-disk)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2BlockGroupDesc {
    pub bg_block_bitmap: u32,
    pub bg_inode_bitmap: u32,
    pub bg_inode_table: u32,
    pub bg_free_blocks_count: u16,
    pub bg_free_inodes_count: u16,
    pub bg_used_dirs_count: u16,
    pub bg_pad: u16,
    pub bg_reserved: [u32; 3],
}

/// ext2 inode (on-disk)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2Inode {
    pub i_mode: u16,
    pub i_uid: u16,
    pub i_size: u32,
    pub i_atime: u32,
    pub i_ctime: u32,
    pub i_mtime: u32,
    pub i_dtime: u32,
    pub i_gid: u16,
    pub i_links_count: u16,
    pub i_blocks: u32, // Number of 512-byte blocks
    pub i_flags: u32,
    pub i_osd1: u32,
    pub i_block: [u32; 15], // Block pointers (12 direct + 3 indirect)
    pub i_generation: u32,
    pub i_file_acl: u32,
    pub i_dir_acl: u32,
    pub i_faddr: u32,
    pub i_osd2: [u8; 12],
}

/// ext2 directory entry (on-disk)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2DirEntry {
    pub inode: u32,
    pub rec_len: u16,
    pub name_len: u8,
    pub file_type: u8,
    // name follows (variable length, up to 255 bytes)
}

/// In-memory ext2 filesystem state
pub struct Ext2Fs {
    /// Block device reader (reads raw bytes by block number)
    read_block_fn: fn(block_nr: u64, buf: &mut [u8]) -> Result<(), FsError>,
    /// Parsed superblock
    pub superblock: Ext2Superblock,
    /// Block size in bytes
    pub block_size: usize,
    /// Block group descriptors
    pub group_descs: Vec<Ext2BlockGroupDesc>,
    /// Inode size
    pub inode_size: usize,
}

impl Ext2Fs {
    /// Try to mount an ext2 filesystem
    pub fn mount(read_block: fn(u64, &mut [u8]) -> Result<(), FsError>) -> Result<Self, FsError> {
        // Read the superblock (at offset 1024 = block 1 for 1K blocks)
        let mut sb_buf = [0u8; 1024];
        // Read 1024 bytes starting at byte offset 1024
        // For a 512-byte block device, that's blocks 2-3
        read_block(1, &mut sb_buf)?;

        let sb: Ext2Superblock =
            unsafe { core::ptr::read_unaligned(sb_buf.as_ptr() as *const Ext2Superblock) };

        if sb.s_magic != EXT2_MAGIC {
            return Err(FsError::NotFound);
        }

        // Cap s_log_block_size to prevent shift overflow (max ext2 block size is 64K = 1024<<6)
        let log_bs = sb.s_log_block_size.min(6) as usize;
        let block_size = DEFAULT_BLOCK_SIZE << log_bs;
        let inode_size = if sb.s_rev_level >= 1 {
            let sz = sb.s_inode_size as usize;
            // inode_size must be a power of two between 128 and block_size
            if sz < 128 || sz > block_size {
                128
            } else {
                sz
            }
        } else {
            128
        };

        // Guard against divide-by-zero if s_blocks_per_group is 0
        if sb.s_blocks_per_group == 0 {
            return Err(FsError::InvalidArgument);
        }
        let num_groups = (sb.s_blocks_count + sb.s_blocks_per_group - 1) / sb.s_blocks_per_group;

        // Read block group descriptor table (starts at block 2 for 1K blocks, or block 1 for larger)
        let bgdt_block = if block_size == 1024 { 2 } else { 1 };
        let bgdt_size = num_groups as usize * core::mem::size_of::<Ext2BlockGroupDesc>();
        let mut bgdt_buf = alloc::vec![0u8; bgdt_size.max(block_size)];
        read_block(bgdt_block, &mut bgdt_buf)?;

        let mut group_descs = Vec::new();
        for i in 0..num_groups as usize {
            let offset = i * core::mem::size_of::<Ext2BlockGroupDesc>();
            let desc: Ext2BlockGroupDesc = unsafe {
                core::ptr::read_unaligned(bgdt_buf[offset..].as_ptr() as *const Ext2BlockGroupDesc)
            };
            group_descs.push(desc);
        }

        let blocks = { sb.s_blocks_count };
        let inodes = { sb.s_inodes_count };
        serial_println!(
            "  ext2: mounted — {} blocks, {} inodes, {}B block size",
            blocks,
            inodes,
            block_size
        );

        Ok(Ext2Fs {
            read_block_fn: read_block,
            superblock: sb,
            block_size,
            group_descs,
            inode_size,
        })
    }

    /// Read an inode by number (1-indexed)
    pub fn read_inode(&self, ino: u32) -> Result<Ext2Inode, FsError> {
        if ino == 0 {
            return Err(FsError::NotFound);
        }

        let group = ((ino - 1) / self.superblock.s_inodes_per_group) as usize;
        let index = ((ino - 1) % self.superblock.s_inodes_per_group) as usize;

        if group >= self.group_descs.len() {
            return Err(FsError::NotFound);
        }

        let inode_table_block = self.group_descs[group].bg_inode_table;
        // Guard against divide-by-zero from a corrupt superblock
        if self.block_size == 0 {
            return Err(FsError::InvalidArgument);
        }
        let byte_offset = index
            .checked_mul(self.inode_size)
            .ok_or(FsError::InvalidArgument)?;
        let block_nr = inode_table_block as u64 + (byte_offset / self.block_size) as u64;
        let offset_in_block = byte_offset % self.block_size;

        let mut block_buf = alloc::vec![0u8; self.block_size];
        (self.read_block_fn)(block_nr, &mut block_buf)?;

        // Ensure the inode fits within the block buffer before reading
        let inode_end = offset_in_block
            .checked_add(core::mem::size_of::<Ext2Inode>())
            .ok_or(FsError::InvalidArgument)?;
        if inode_end > block_buf.len() {
            return Err(FsError::InvalidArgument);
        }
        // Safety: offset_in_block + size_of::<Ext2Inode>() <= block_buf.len();
        // Ext2Inode is repr(C, packed) so any byte alignment is valid.
        let inode: Ext2Inode = unsafe {
            core::ptr::read_unaligned(block_buf[offset_in_block..].as_ptr() as *const Ext2Inode)
        };

        Ok(inode)
    }

    /// Read the contents of a file inode into a Vec
    pub fn read_file(&self, inode: &Ext2Inode) -> Result<Vec<u8>, FsError> {
        let size = inode.i_size as usize;
        let mut data = alloc::vec![0u8; size];
        let mut offset = 0;

        // Read direct blocks (0-11)
        for i in 0..12 {
            if offset >= size {
                break;
            }
            let block_nr = inode.i_block[i];
            if block_nr == 0 {
                break;
            }

            let mut block_buf = alloc::vec![0u8; self.block_size];
            (self.read_block_fn)(block_nr as u64, &mut block_buf)?;

            let copy_len = (size - offset).min(self.block_size);
            data[offset..offset + copy_len].copy_from_slice(&block_buf[..copy_len]);
            offset = offset.saturating_add(copy_len);
        }

        // Indirect block (block 12)
        if offset < size && inode.i_block[12] != 0 {
            let mut indirect_buf = alloc::vec![0u8; self.block_size];
            (self.read_block_fn)(inode.i_block[12] as u64, &mut indirect_buf)?;

            let ptrs_per_block = self.block_size / 4;
            for i in 0..ptrs_per_block {
                if offset >= size {
                    break;
                }
                let block_nr = u32::from_le_bytes([
                    indirect_buf[i * 4],
                    indirect_buf[i * 4 + 1],
                    indirect_buf[i * 4 + 2],
                    indirect_buf[i * 4 + 3],
                ]);
                if block_nr == 0 {
                    break;
                }

                let mut block_buf = alloc::vec![0u8; self.block_size];
                (self.read_block_fn)(block_nr as u64, &mut block_buf)?;

                let copy_len = (size - offset).min(self.block_size);
                data[offset..offset + copy_len].copy_from_slice(&block_buf[..copy_len]);
                offset = offset.saturating_add(copy_len);
            }
        }

        Ok(data)
    }

    /// List entries in a directory inode
    pub fn read_dir(&self, inode: &Ext2Inode) -> Result<Vec<(u32, String, u8)>, FsError> {
        if inode.i_mode & S_IFDIR == 0 {
            return Err(FsError::NotADirectory);
        }

        let dir_data = self.read_file(inode)?;
        let mut entries = Vec::new();
        let mut offset = 0;

        while offset + 8 <= dir_data.len() {
            let entry: Ext2DirEntry = unsafe {
                core::ptr::read_unaligned(dir_data[offset..].as_ptr() as *const Ext2DirEntry)
            };

            if entry.inode != 0 && entry.name_len > 0 {
                let name_start = offset + 8;
                let name_end = name_start + entry.name_len as usize;
                if name_end <= dir_data.len() {
                    let name = core::str::from_utf8(&dir_data[name_start..name_end]).unwrap_or("?");
                    entries.push((entry.inode, String::from(name), entry.file_type));
                }
            }

            if entry.rec_len == 0 {
                break;
            }
            offset = offset.saturating_add(entry.rec_len as usize);
        }

        Ok(entries)
    }

    /// Resolve a path from root to an inode number
    pub fn lookup(&self, path: &str) -> Result<u32, FsError> {
        let mut ino = 2u32; // root inode is always 2 in ext2

        for component in path.split('/').filter(|s| !s.is_empty()) {
            let inode = self.read_inode(ino)?;
            let entries = self.read_dir(&inode)?;

            let found = entries.iter().find(|(_, name, _)| name == component);
            match found {
                Some((child_ino, _, _)) => ino = *child_ino,
                None => return Err(FsError::NotFound),
            }
        }

        Ok(ino)
    }
}
