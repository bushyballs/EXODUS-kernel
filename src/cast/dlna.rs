use crate::sync::Mutex;
/// DLNA media streaming for Genesis
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum DlnaDeviceType {
    MediaServer,
    MediaRenderer,
    MediaController,
}

#[derive(Clone, Copy)]
struct DlnaDevice {
    id: u32,
    name_hash: u64,
    device_type: DlnaDeviceType,
    ip_hash: u64,
    port: u16,
    capabilities: u16,
}

struct DlnaManager {
    devices: Vec<DlnaDevice>,
    active_renderer: Option<u32>,
    total_streams: u32,
    next_id: u32,
}

static DLNA: Mutex<Option<DlnaManager>> = Mutex::new(None);

impl DlnaManager {
    fn new() -> Self {
        DlnaManager {
            devices: Vec::new(),
            active_renderer: None,
            total_streams: 0,
            next_id: 1,
        }
    }

    fn discover(&mut self) {
        // SSDP discovery would go here
    }

    fn play_to_device(&mut self, device_id: u32) -> bool {
        if self
            .devices
            .iter()
            .any(|d| d.id == device_id && d.device_type == DlnaDeviceType::MediaRenderer)
        {
            self.active_renderer = Some(device_id);
            self.total_streams = self.total_streams.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn stop(&mut self) {
        self.active_renderer = None;
    }
}

pub fn init() {
    let mut d = DLNA.lock();
    *d = Some(DlnaManager::new());
    serial_println!("    DLNA media streaming ready");
}
