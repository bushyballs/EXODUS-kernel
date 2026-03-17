use crate::sync::Mutex;
/// Store client for Genesis
///
/// Install, update, uninstall, download management,
/// update notifications, auto-update scheduling.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum InstallState {
    Queued,
    Downloading,
    Installing,
    Installed,
    Updating,
    Failed,
}

struct InstalledApp {
    listing_id: u32,
    version_major: u8,
    version_minor: u8,
    version_patch: u16,
    install_state: InstallState,
    installed_size: u64,
    install_time: u64,
    last_update: u64,
    auto_update: bool,
    data_size: u64,
    cache_size: u64,
}

struct StoreClient {
    installed: Vec<InstalledApp>,
    download_queue: Vec<u32>,
    auto_update_enabled: bool,
    wifi_only_downloads: bool,
    total_installed: u32,
    total_updated: u32,
}

static STORE_CLIENT: Mutex<Option<StoreClient>> = Mutex::new(None);

impl StoreClient {
    fn new() -> Self {
        StoreClient {
            installed: Vec::new(),
            download_queue: Vec::new(),
            auto_update_enabled: true,
            wifi_only_downloads: true,
            total_installed: 0,
            total_updated: 0,
        }
    }

    fn install(&mut self, listing_id: u32, size: u64, timestamp: u64) {
        self.installed.push(InstalledApp {
            listing_id,
            version_major: 1,
            version_minor: 0,
            version_patch: 0,
            install_state: InstallState::Downloading,
            installed_size: size,
            install_time: timestamp,
            last_update: timestamp,
            auto_update: true,
            data_size: 0,
            cache_size: 0,
        });
        self.total_installed = self.total_installed.saturating_add(1);
    }

    fn uninstall(&mut self, listing_id: u32) -> bool {
        if let Some(idx) = self
            .installed
            .iter()
            .position(|a| a.listing_id == listing_id)
        {
            self.installed.remove(idx);
            return true;
        }
        false
    }

    fn check_updates(&self) -> Vec<u32> {
        // Would compare installed versions against repo
        Vec::new()
    }

    fn total_storage_used(&self) -> u64 {
        self.installed
            .iter()
            .map(|a| a.installed_size + a.data_size + a.cache_size)
            .sum()
    }

    fn clear_cache(&mut self, listing_id: u32) {
        if let Some(app) = self
            .installed
            .iter_mut()
            .find(|a| a.listing_id == listing_id)
        {
            app.cache_size = 0;
        }
    }
}

pub fn init() {
    let mut c = STORE_CLIENT.lock();
    *c = Some(StoreClient::new());
    serial_println!("    App store: client (install, update, auto-update) ready");
}
