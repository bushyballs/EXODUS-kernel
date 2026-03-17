/// Volume management for Genesis
///
/// Disk volume discovery, mount/unmount, filesystem detection,
/// encryption status, and storage health monitoring.
///
/// Inspired by: Android Vold, Linux udisks. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Volume type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeType {
    Internal,
    SdCard,
    UsbDrive,
    NetworkShare,
    RamDisk,
    Loop,
}

/// Volume state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    Unmounted,
    Checking,
    Mounted,
    Ejecting,
    Formatting,
    Error,
}

/// Filesystem type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {
    HoagsFs,
    Ext4,
    Fat32,
    ExFat,
    Ntfs,
    Btrfs,
    Tmpfs,
    Unknown,
}

/// A storage volume
pub struct Volume {
    pub id: u32,
    pub label: String,
    pub device_path: String,
    pub mount_point: String,
    pub volume_type: VolumeType,
    pub state: VolumeState,
    pub fs_type: FsType,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub encrypted: bool,
    pub read_only: bool,
    pub removable: bool,
}

impl Volume {
    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }

    pub fn usage_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        ((self.used_bytes * 100) / self.total_bytes) as u8
    }

    pub fn is_mounted(&self) -> bool {
        self.state == VolumeState::Mounted
    }
}

/// Volume manager
pub struct VolumeManager {
    pub volumes: Vec<Volume>,
    pub next_id: u32,
    pub auto_mount: bool,
    pub auto_check: bool,
}

impl VolumeManager {
    const fn new() -> Self {
        VolumeManager {
            volumes: Vec::new(),
            next_id: 1,
            auto_mount: true,
            auto_check: true,
        }
    }

    pub fn add_volume(
        &mut self,
        label: &str,
        device: &str,
        vtype: VolumeType,
        fs: FsType,
        total: u64,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.volumes.push(Volume {
            id,
            label: String::from(label),
            device_path: String::from(device),
            mount_point: String::new(),
            volume_type: vtype,
            state: VolumeState::Unmounted,
            fs_type: fs,
            total_bytes: total,
            used_bytes: 0,
            encrypted: false,
            read_only: false,
            removable: matches!(vtype, VolumeType::SdCard | VolumeType::UsbDrive),
        });
        id
    }

    pub fn mount(&mut self, id: u32, mount_point: &str) -> bool {
        if let Some(vol) = self.volumes.iter_mut().find(|v| v.id == id) {
            if vol.state != VolumeState::Unmounted {
                return false;
            }
            vol.mount_point = String::from(mount_point);
            vol.state = VolumeState::Mounted;
            crate::serial_println!("  [volume] Mounted {} at {}", vol.label, mount_point);
            true
        } else {
            false
        }
    }

    pub fn unmount(&mut self, id: u32) -> bool {
        if let Some(vol) = self.volumes.iter_mut().find(|v| v.id == id) {
            if vol.state != VolumeState::Mounted {
                return false;
            }
            vol.state = VolumeState::Ejecting;
            // Sync pending writes
            vol.state = VolumeState::Unmounted;
            vol.mount_point.clear();
            true
        } else {
            false
        }
    }

    pub fn get(&self, id: u32) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.id == id)
    }

    pub fn mounted_volumes(&self) -> Vec<&Volume> {
        self.volumes
            .iter()
            .filter(|v| v.state == VolumeState::Mounted)
            .collect()
    }

    pub fn total_storage(&self) -> u64 {
        self.volumes
            .iter()
            .filter(|v| v.volume_type == VolumeType::Internal)
            .map(|v| v.total_bytes)
            .sum()
    }
}

static VOLUMES: Mutex<VolumeManager> = Mutex::new(VolumeManager::new());

pub fn init() {
    let mut mgr = VOLUMES.lock();
    // Register the internal storage
    let id = mgr.add_volume(
        "System",
        "/dev/sda1",
        VolumeType::Internal,
        FsType::HoagsFs,
        64 * 1024 * 1024 * 1024,
    ); // 64 GB
    mgr.mount(id, "/");

    let data_id = mgr.add_volume(
        "Data",
        "/dev/sda2",
        VolumeType::Internal,
        FsType::HoagsFs,
        128 * 1024 * 1024 * 1024,
    ); // 128 GB
    mgr.mount(data_id, "/data");

    crate::serial_println!(
        "  [volume] Volume manager initialized ({} volumes)",
        mgr.volumes.len()
    );
}
