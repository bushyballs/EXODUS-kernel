/// Nearby sharing for Genesis
///
/// Device discovery, file transfer via WiFi Direct/BLE,
/// contact-based sharing, and transfer progress.
///
/// Inspired by: Android Nearby Share, AirDrop. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Discovery mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    Off,
    ContactsOnly,
    Everyone,
    Hidden,
}

/// Transfer state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Discovering,
    Connecting,
    Sending,
    Receiving,
    Complete,
    Failed,
    Cancelled,
}

/// A nearby device
pub struct NearbyDevice {
    pub id: u32,
    pub name: String,
    pub device_type: String,
    pub signal_strength: i8,
    pub supports_wifi_direct: bool,
}

/// A file transfer
pub struct Transfer {
    pub id: u32,
    pub peer_name: String,
    pub file_name: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub state: TransferState,
    pub speed_bps: u64,
}

impl Transfer {
    pub fn progress_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        ((self.transferred_bytes * 100) / self.total_bytes) as u8
    }
}

/// Nearby share manager
pub struct NearbyShare {
    pub discovery_mode: DiscoveryMode,
    pub device_name: String,
    pub nearby_devices: Vec<NearbyDevice>,
    pub transfers: Vec<Transfer>,
    pub next_transfer_id: u32,
    pub auto_accept_contacts: bool,
}

impl NearbyShare {
    const fn new() -> Self {
        NearbyShare {
            discovery_mode: DiscoveryMode::ContactsOnly,
            device_name: String::new(),
            nearby_devices: Vec::new(),
            transfers: Vec::new(),
            next_transfer_id: 1,
            auto_accept_contacts: false,
        }
    }

    pub fn set_discovery(&mut self, mode: DiscoveryMode) {
        self.discovery_mode = mode;
    }

    pub fn start_scan(&mut self) {
        self.nearby_devices.clear();
        // In real implementation: send BLE advertisements, listen for peers
    }

    pub fn send_file(&mut self, peer_id: u32, file_name: &str, size: u64) -> u32 {
        let peer_name = self
            .nearby_devices
            .iter()
            .find(|d| d.id == peer_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| String::from("Unknown"));

        let id = self.next_transfer_id;
        self.next_transfer_id = self.next_transfer_id.saturating_add(1);
        self.transfers.push(Transfer {
            id,
            peer_name,
            file_name: String::from(file_name),
            total_bytes: size,
            transferred_bytes: 0,
            state: TransferState::Connecting,
            speed_bps: 0,
        });
        id
    }

    pub fn update_transfer(&mut self, id: u32, bytes: u64) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.transferred_bytes = bytes;
            if bytes >= t.total_bytes {
                t.state = TransferState::Complete;
            } else {
                t.state = TransferState::Sending;
            }
        }
    }

    pub fn cancel_transfer(&mut self, id: u32) {
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
            t.state = TransferState::Cancelled;
        }
    }

    pub fn active_transfers(&self) -> Vec<&Transfer> {
        self.transfers
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    TransferState::Sending | TransferState::Receiving | TransferState::Connecting
                )
            })
            .collect()
    }
}

static NEARBY: Mutex<NearbyShare> = Mutex::new(NearbyShare::new());

pub fn init() {
    NEARBY.lock().device_name = String::from("Hoags Device");
    crate::serial_println!("  [crossdevice] Nearby Share initialized");
}
