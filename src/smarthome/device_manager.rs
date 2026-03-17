use crate::sync::Mutex;
/// IoT device manager for Genesis
///
/// Device registry, pairing, state management,
/// groups, rooms, favorites.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum DeviceType {
    Light,
    Switch,
    Thermostat,
    Lock,
    Camera,
    Sensor,
    Speaker,
    Plug,
    Fan,
    Blinds,
    Doorbell,
    Vacuum,
    Garage,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DeviceState {
    Online,
    Offline,
    Pairing,
    Updating,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Protocol {
    Matter,
    Thread,
    Zigbee,
    ZWave,
    WiFi,
    Bluetooth,
    Lan,
}

struct IoTDevice {
    id: u32,
    device_type: DeviceType,
    protocol: Protocol,
    name: [u8; 32],
    name_len: usize,
    room: [u8; 16],
    room_len: usize,
    state: DeviceState,
    power_on: bool,
    brightness: u8,   // 0-100 for lights
    temperature: u16, // for thermostats (C * 10)
    battery_pct: Option<u8>,
    last_seen: u64,
    firmware_version: u32,
}

struct DeviceManager {
    devices: Vec<IoTDevice>,
    next_id: u32,
    rooms: Vec<[u8; 16]>,
}

static DEVICE_MGR: Mutex<Option<DeviceManager>> = Mutex::new(None);

impl DeviceManager {
    fn new() -> Self {
        DeviceManager {
            devices: Vec::new(),
            next_id: 1,
            rooms: Vec::new(),
        }
    }

    fn add_device(
        &mut self,
        dtype: DeviceType,
        protocol: Protocol,
        name: &[u8],
        room: &[u8],
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 32];
        let nlen = name.len().min(32);
        n[..nlen].copy_from_slice(&name[..nlen]);
        let mut r = [0u8; 16];
        let rlen = room.len().min(16);
        r[..rlen].copy_from_slice(&room[..rlen]);
        self.devices.push(IoTDevice {
            id,
            device_type: dtype,
            protocol,
            name: n,
            name_len: nlen,
            room: r,
            room_len: rlen,
            state: DeviceState::Online,
            power_on: false,
            brightness: 100,
            temperature: 220,
            battery_pct: None,
            last_seen: 0,
            firmware_version: 1,
        });
        id
    }

    fn toggle_power(&mut self, device_id: u32) -> Option<bool> {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == device_id) {
            d.power_on = !d.power_on;
            Some(d.power_on)
        } else {
            None
        }
    }

    fn set_brightness(&mut self, device_id: u32, brightness: u8) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == device_id) {
            d.brightness = brightness.min(100);
            if brightness > 0 {
                d.power_on = true;
            }
        }
    }

    fn set_temperature(&mut self, device_id: u32, temp_c10: u16) {
        if let Some(d) = self.devices.iter_mut().find(|d| d.id == device_id) {
            d.temperature = temp_c10;
        }
    }

    fn devices_in_room(&self, room: &[u8]) -> Vec<u32> {
        self.devices
            .iter()
            .filter(|d| &d.room[..d.room_len] == room)
            .map(|d| d.id)
            .collect()
    }

    fn offline_devices(&self) -> Vec<u32> {
        self.devices
            .iter()
            .filter(|d| d.state == DeviceState::Offline)
            .map(|d| d.id)
            .collect()
    }
}

pub fn init() {
    let mut mgr = DEVICE_MGR.lock();
    *mgr = Some(DeviceManager::new());
    serial_println!("    Smart home: device manager ready");
}
