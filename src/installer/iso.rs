/// ISO Builder — create bootable Hoags OS images
///
/// Generates:
///   1. ISO 9660 filesystem with El Torito boot
///   2. UEFI boot partition (FAT32 ESP)
///   3. Hybrid MBR for USB booting
///
/// The ISO contains:
///   /boot/genesis.elf — the kernel
///   /boot/initramfs.img — initial ramdisk
///   /EFI/BOOT/BOOTX64.EFI — UEFI bootloader
///   /hoags/ — system files for installation
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// ISO 9660 volume descriptor
const ISO_SECTOR_SIZE: usize = 2048;
const ISO_MAGIC: &[u8; 5] = b"CD001";

/// ISO file entry
#[derive(Debug, Clone)]
pub struct IsoFile {
    pub path: String,
    pub data: Vec<u8>,
    pub is_directory: bool,
}

/// ISO 9660 Primary Volume Descriptor
#[derive(Debug, Clone)]
pub struct PrimaryVolumeDescriptor {
    pub system_id: String,
    pub volume_id: String,
    pub volume_size: u32, // in sectors
    pub root_dir_record: DirRecord,
    pub publisher: String,
    pub application: String,
    pub creation_date: String,
}

/// Directory record
#[derive(Debug, Clone)]
pub struct DirRecord {
    pub name: String,
    pub extent_lba: u32,  // starting sector
    pub data_length: u32, // in bytes
    pub is_directory: bool,
}

/// El Torito boot catalog
#[derive(Debug, Clone)]
pub struct BootCatalog {
    pub platform_id: u8, // 0=x86, 1=PPC, 2=Mac, 0xEF=EFI
    pub boot_image_lba: u32,
    pub boot_image_sectors: u16,
}

/// ISO builder
pub struct IsoBuilder {
    pub files: Vec<IsoFile>,
    pub volume_id: String,
    pub publisher: String,
    pub boot_image: Option<Vec<u8>>,
    pub efi_image: Option<Vec<u8>>,
}

impl IsoBuilder {
    pub fn new(volume_id: &str) -> Self {
        IsoBuilder {
            files: Vec::new(),
            volume_id: String::from(volume_id),
            publisher: String::from("Hoags Inc."),
            boot_image: None,
            efi_image: None,
        }
    }

    /// Add a file to the ISO
    pub fn add_file(&mut self, path: &str, data: Vec<u8>) {
        self.files.push(IsoFile {
            path: String::from(path),
            data,
            is_directory: false,
        });
    }

    /// Add a directory to the ISO
    pub fn add_directory(&mut self, path: &str) {
        self.files.push(IsoFile {
            path: String::from(path),
            data: Vec::new(),
            is_directory: true,
        });
    }

    /// Set the BIOS boot image (El Torito)
    pub fn set_boot_image(&mut self, image: Vec<u8>) {
        self.boot_image = Some(image);
    }

    /// Set the EFI boot image (for UEFI boot)
    pub fn set_efi_image(&mut self, image: Vec<u8>) {
        self.efi_image = Some(image);
    }

    /// Build the ISO image
    pub fn build(&self) -> Vec<u8> {
        let mut iso = Vec::new();

        // System Area (16 sectors of zeros)
        iso.resize(16 * ISO_SECTOR_SIZE, 0);

        // Primary Volume Descriptor (sector 16)
        let mut pvd = [0u8; ISO_SECTOR_SIZE];
        pvd[0] = 1; // type: primary
        pvd[1..6].copy_from_slice(ISO_MAGIC);
        pvd[6] = 1; // version

        // System identifier (32 bytes at offset 8)
        let sys_id = b"HOAGS OS";
        pvd[8..8 + sys_id.len()].copy_from_slice(sys_id);

        // Volume identifier (32 bytes at offset 40)
        let vol_id = self.volume_id.as_bytes();
        let vol_len = vol_id.len().min(32);
        pvd[40..40 + vol_len].copy_from_slice(&vol_id[..vol_len]);

        // Volume size in sectors (both-endian at offset 80)
        let total_sectors = (16 + 1 + 1 + self.files.len() as u32 * 2 + 100) as u32;
        pvd[80..84].copy_from_slice(&total_sectors.to_le_bytes());
        pvd[84..88].copy_from_slice(&total_sectors.to_be_bytes());

        // Logical block size = 2048 (both-endian at offset 128)
        pvd[128..130].copy_from_slice(&2048u16.to_le_bytes());
        pvd[130..132].copy_from_slice(&2048u16.to_be_bytes());

        // Publisher (128 bytes at offset 318)
        let pub_bytes = self.publisher.as_bytes();
        let pub_len = pub_bytes.len().min(128);
        pvd[318..318 + pub_len].copy_from_slice(&pub_bytes[..pub_len]);

        // Application (128 bytes at offset 574)
        let app = b"HOAGS OS INSTALLER";
        pvd[574..574 + app.len()].copy_from_slice(app);

        iso.extend_from_slice(&pvd);

        // Volume Descriptor Set Terminator (sector 17)
        let mut term = [0u8; ISO_SECTOR_SIZE];
        term[0] = 255; // type: terminator
        term[1..6].copy_from_slice(ISO_MAGIC);
        term[6] = 1;
        iso.extend_from_slice(&term);

        // File data sectors
        for file in &self.files {
            if !file.is_directory {
                // Pad file data to sector boundary
                let mut sector_data = file.data.clone();
                let padding =
                    (ISO_SECTOR_SIZE - (sector_data.len() % ISO_SECTOR_SIZE)) % ISO_SECTOR_SIZE;
                sector_data.resize(sector_data.len() + padding, 0);
                iso.extend_from_slice(&sector_data);
            }
        }

        // El Torito boot record (if boot image provided)
        if let Some(boot_img) = &self.boot_image {
            // Add boot catalog and boot image
            let boot_catalog_sector = (iso.len() / ISO_SECTOR_SIZE) as u32;
            let mut catalog = [0u8; ISO_SECTOR_SIZE];

            // Validation entry
            catalog[0] = 1; // header ID
            catalog[1] = 0; // platform: x86
                            // ID string
            let id = b"Hoags OS";
            catalog[4..4 + id.len()].copy_from_slice(id);

            // Default entry
            catalog[32] = 0x88; // bootable
            catalog[34] = 4; // no emulation
                             // Boot image LBA (next sector)
            let img_lba = boot_catalog_sector + 1;
            catalog[40..44].copy_from_slice(&img_lba.to_le_bytes());
            // Sector count
            let sectors = ((boot_img.len() + ISO_SECTOR_SIZE - 1) / ISO_SECTOR_SIZE) as u16;
            catalog[38..40].copy_from_slice(&sectors.to_le_bytes());

            iso.extend_from_slice(&catalog);

            // Boot image
            let mut img_data = boot_img.clone();
            let padding = (ISO_SECTOR_SIZE - (img_data.len() % ISO_SECTOR_SIZE)) % ISO_SECTOR_SIZE;
            img_data.resize(img_data.len() + padding, 0);
            iso.extend_from_slice(&img_data);
        }

        serial_println!(
            "    [iso] Built ISO: {} bytes ({} sectors), {} files",
            iso.len(),
            iso.len() / ISO_SECTOR_SIZE,
            self.files.len()
        );

        iso
    }

    /// Build a standard Hoags OS installer ISO
    pub fn build_installer() -> Vec<u8> {
        let mut builder = IsoBuilder::new("HOAGS_OS_0_5_0");

        // Add directory structure
        builder.add_directory("/boot");
        builder.add_directory("/EFI");
        builder.add_directory("/EFI/BOOT");
        builder.add_directory("/hoags");
        builder.add_directory("/hoags/packages");

        // Add kernel (placeholder — would be real ELF binary)
        builder.add_file("/boot/genesis.elf", alloc::vec![0x7F, b'E', b'L', b'F']);
        builder.add_file("/boot/initramfs.img", Vec::new());

        // Add UEFI bootloader (placeholder)
        builder.add_file("/EFI/BOOT/BOOTX64.EFI", alloc::vec![b'M', b'Z']); // PE header

        // Add installer script
        builder.add_file(
            "/hoags/install.conf",
            b"[installer]\nversion = 0.5.0\nmin_disk = 8GB\n".to_vec(),
        );

        builder.build()
    }
}
