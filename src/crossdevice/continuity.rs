/// Device continuity for Genesis
///
/// Handoff between devices, universal clipboard,
/// phone calls on desktop, SMS relay, and instant hotspot.
///
/// Inspired by: Apple Continuity, Windows Phone Link. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Continuity feature
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Handoff,
    UniversalClipboard,
    PhoneRelay,
    SmsRelay,
    InstantHotspot,
    AutoUnlock,
    SidecarDisplay,
}

/// Paired device
pub struct PairedDevice {
    pub id: u32,
    pub name: String,
    pub device_type: String,
    pub last_seen: u64,
    pub connected: bool,
    pub features: Vec<Feature>,
}

/// Handoff activity
pub struct HandoffActivity {
    pub app_id: String,
    pub activity_type: String,
    pub user_info: Vec<(String, String)>,
    pub source_device: u32,
    pub timestamp: u64,
}

/// Continuity manager
pub struct ContinuityManager {
    pub enabled: bool,
    pub paired_devices: Vec<PairedDevice>,
    pub next_device_id: u32,
    pub current_activity: Option<HandoffActivity>,
    pub clipboard_sync: bool,
    pub phone_relay: bool,
    pub sms_relay: bool,
}

impl ContinuityManager {
    const fn new() -> Self {
        ContinuityManager {
            enabled: true,
            paired_devices: Vec::new(),
            next_device_id: 1,
            current_activity: None,
            clipboard_sync: true,
            phone_relay: false,
            sms_relay: false,
        }
    }

    pub fn pair_device(&mut self, name: &str, dtype: &str) -> u32 {
        let id = self.next_device_id;
        self.next_device_id = self.next_device_id.saturating_add(1);
        self.paired_devices.push(PairedDevice {
            id,
            name: String::from(name),
            device_type: String::from(dtype),
            last_seen: crate::time::clock::unix_time(),
            connected: false,
            features: Vec::new(),
        });
        id
    }

    pub fn unpair_device(&mut self, id: u32) {
        self.paired_devices.retain(|d| d.id != id);
    }

    pub fn start_handoff(&mut self, app_id: &str, activity_type: &str) {
        self.current_activity = Some(HandoffActivity {
            app_id: String::from(app_id),
            activity_type: String::from(activity_type),
            user_info: Vec::new(),
            source_device: 0,
            timestamp: crate::time::clock::unix_time(),
        });
    }

    pub fn accept_handoff(&mut self) -> Option<HandoffActivity> {
        self.current_activity.take()
    }

    pub fn sync_clipboard(&self, _text: &str) {
        if !self.clipboard_sync {
            return;
        }
        // Send clipboard content to all connected devices
    }

    pub fn connected_devices(&self) -> Vec<&PairedDevice> {
        self.paired_devices.iter().filter(|d| d.connected).collect()
    }
}

static CONTINUITY: Mutex<ContinuityManager> = Mutex::new(ContinuityManager::new());

pub fn init() {
    crate::serial_println!("  [crossdevice] Continuity system initialized");
}
