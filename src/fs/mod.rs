pub mod acl;
pub mod ai_fs;
pub mod ai_prefetch;
pub mod aio;
pub mod block_alloc;
pub mod btrfs;
pub mod cifs;
pub mod dentry_cache;
pub mod devfs;
pub mod devtmpfs;
pub mod epoll;
pub mod ext2;
pub mod fat32;
pub mod fd;
pub mod fuse;
pub mod hoagsfs;
pub mod inode_cache;
pub mod inotify;
pub mod iso9660;
pub mod journal;
pub mod nfs;
pub mod ntfs;
pub mod overlayfs;
pub mod procfs;
pub mod quota;
pub mod ramfs;
pub mod sendfile;
pub mod splice;
pub mod squashfs;
pub mod stat_cache;
pub mod sysctl;
pub mod sysfs;
pub mod tmpfs;
/// Virtual File System for Genesis
///
/// The VFS provides a unified interface for all file operations,
/// abstracting over different filesystem implementations (HoagsFS, FAT32,
/// procfs, devfs, etc.)
///
/// Inspired by: Linux VFS (inode/dentry/superblock model), Plan 9 (everything
/// is a file + per-process namespaces), Redox (URL-based scheme routing).
/// All code is original.
pub mod vfs;
pub mod xattr;
pub mod xfs;

use crate::serial_println;

/// ATA block device adapter -- bridges our ATA driver to the BlockDevice trait
pub struct AtaBlockDevice {
    drive_idx: usize,
}

impl AtaBlockDevice {
    pub fn new(drive_idx: usize) -> Self {
        AtaBlockDevice { drive_idx }
    }
}

impl hoagsfs::BlockDevice for AtaBlockDevice {
    fn read_block(
        &self,
        block_nr: u64,
        buf: &mut [u8; hoagsfs::BLOCK_SIZE],
    ) -> Result<(), vfs::FsError> {
        // Convert 4K block to 512-byte sectors (8 sectors per block)
        let start_lba = block_nr * 8;
        // Read 8 sectors (4096 bytes)
        for i in 0..8u8 {
            let sector_buf = &mut buf[(i as usize * 512)..((i as usize + 1) * 512)];
            let mut sector = [0u8; 512];
            crate::drivers::ata::read_sectors(self.drive_idx, start_lba + i as u64, 1, &mut sector)
                .map_err(|_| vfs::FsError::IoError)?;
            sector_buf.copy_from_slice(&sector);
        }
        Ok(())
    }

    fn write_block(
        &self,
        block_nr: u64,
        buf: &[u8; hoagsfs::BLOCK_SIZE],
    ) -> Result<(), vfs::FsError> {
        let start_lba = block_nr * 8;
        for i in 0..8u8 {
            let sector_buf = &buf[(i as usize * 512)..((i as usize + 1) * 512)];
            crate::drivers::ata::write_sectors(self.drive_idx, start_lba + i as u64, 1, sector_buf)
                .map_err(|_| vfs::FsError::IoError)?;
        }
        Ok(())
    }
}

/// Initialize the filesystem subsystem
pub fn init() {
    // Register built-in filesystems
    vfs::init();

    // Initialize in-memory filesystem (RAM-backed VFS tree)
    vfs::init_memfs();

    // Mount devfs at /dev
    devfs::init();

    // Mount procfs at /proc
    procfs::init();

    // Mount sysfs at /sys
    sysfs::init();

    // Initialise FAT32 driver (no-heap static-buffer implementation).
    // Actual mounting (fat32::fat32_mount) is called here for device 0 if available.
    fat32::init();
    if fat32::fat32_mount(0, 0) {
        serial_println!("  VFS: FAT32 mounted from device 0 into vol[0]");
    } else {
        serial_println!("  VFS: no FAT32 on device 0 (or no drive)");
    }

    // Mount tmpfs at /tmp and /run
    tmpfs::init();

    // Initialize ramfs (used for /sys and /run backing)
    ramfs::ramfs_init();

    // Initialize AI filesystem intelligence
    ai_fs::init();

    // Initialize AI predictive prefetching (neural bus integration)
    ai_prefetch::init();

    // Initialise ISO 9660 CD-ROM filesystem driver.
    iso9660::init();
    if iso9660::iso9660_mount(0, 0) {
        serial_println!("  VFS: ISO 9660 mounted from device 0 into vol[0]");
    } else {
        serial_println!("  VFS: no ISO 9660 on device 0 (or no CD-ROM)");
    }

    // Initialize CIFS/SMB2 client stub
    cifs::init();

    // Initialize XFS filesystem driver stub
    xfs::init();

    // Initialize Btrfs filesystem driver stub
    btrfs::init();

    // Initialize overlayfs union mount subsystem
    overlayfs::init();

    // Initialize SquashFS read-only filesystem driver
    squashfs::init();

    // Initialize inotify filesystem event notification
    inotify::init();

    // Initialize devtmpfs (/dev virtual filesystem with standard device nodes)
    devtmpfs::init();

    // Initialize sysctl kernel parameter interface
    sysctl::init();

    serial_println!("  VFS: initialized with devfs, procfs, sysfs, tmpfs, overlayfs, squashfs, inotify, flock, dcache, mount table");
}
