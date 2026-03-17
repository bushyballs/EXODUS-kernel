/// System toggles backend for Genesis
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ToggleId {
    Wifi,
    Bluetooth,
    Airplane,
    Location,
    Nfc,
    AutoRotate,
    DarkMode,
    BatterySaver,
    DataSaver,
    Hotspot,
    Vpn,
    NightLight,
    DoNotDisturb,
}

#[derive(Clone, Copy)]
struct ToggleState {
    id: ToggleId,
    enabled: bool,
    last_changed: u64,
    change_count: u32,
}

struct ToggleManager {
    toggles: [ToggleState; 13],
    total_toggles: u32,
}

static TOGGLES: Mutex<Option<ToggleManager>> = Mutex::new(None);

impl ToggleManager {
    fn new() -> Self {
        use ToggleId::*;
        let ids = [
            Wifi,
            Bluetooth,
            Airplane,
            Location,
            Nfc,
            AutoRotate,
            DarkMode,
            BatterySaver,
            DataSaver,
            Hotspot,
            Vpn,
            NightLight,
            DoNotDisturb,
        ];
        let mut toggles = [ToggleState {
            id: Wifi,
            enabled: false,
            last_changed: 0,
            change_count: 0,
        }; 13];
        for (i, &tid) in ids.iter().enumerate() {
            toggles[i].id = tid;
            // Wifi and Bluetooth on by default
            toggles[i].enabled = matches!(tid, Wifi | Bluetooth | Location | AutoRotate);
        }
        ToggleManager {
            toggles,
            total_toggles: 0,
        }
    }

    fn toggle(&mut self, id: ToggleId, timestamp: u64) {
        for t in &mut self.toggles {
            if t.id == id {
                t.enabled = !t.enabled;
                t.last_changed = timestamp;
                t.change_count = t.change_count.saturating_add(1);
                self.total_toggles = self.total_toggles.saturating_add(1);
                // Airplane mode side effects
                if id == ToggleId::Airplane && t.enabled {
                    self.force_set(ToggleId::Wifi, false, timestamp);
                    self.force_set(ToggleId::Bluetooth, false, timestamp);
                    self.force_set(ToggleId::Nfc, false, timestamp);
                }
                return;
            }
        }
    }

    fn force_set(&mut self, id: ToggleId, enabled: bool, timestamp: u64) {
        for t in &mut self.toggles {
            if t.id == id {
                t.enabled = enabled;
                t.last_changed = timestamp;
                return;
            }
        }
    }

    fn is_enabled(&self, id: ToggleId) -> bool {
        self.toggles
            .iter()
            .find(|t| t.id == id)
            .map_or(false, |t| t.enabled)
    }
}

pub fn init() {
    let mut t = TOGGLES.lock();
    *t = Some(ToggleManager::new());
    serial_println!("    System toggles: 13 toggles ready");
}
