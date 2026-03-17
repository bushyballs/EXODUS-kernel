/// Device model — bus/device/driver hierarchy for Genesis
///
/// Unified device model that organizes all hardware into a tree structure.
/// Buses discover devices, drivers claim devices, and the model manages
/// binding, power management, and hotplug events (uevents).
///
/// Inspired by: Linux device model (drivers/base/). All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Device types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Platform,
    Pci,
    Usb,
    I2c,
    Spi,
    Virtio,
    Block,
    Char,
    Net,
    Input,
    Sound,
    Graphics,
    Tty,
}

/// Device power state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    D0Active,
    D1Sleep,
    D2DeepSleep,
    D3Suspended,
    D3Cold,
}

/// A device in the device model
pub struct Device {
    /// Unique device ID
    pub id: u32,
    /// Device name (e.g., "eth0", "sda", "input0")
    pub name: String,
    /// Device type
    pub dev_type: DeviceType,
    /// Bus this device is on
    pub bus: String,
    /// Driver bound to this device (if any)
    pub driver: Option<String>,
    /// Parent device ID (0 = root)
    pub parent: u32,
    /// Children device IDs
    pub children: Vec<u32>,
    /// Power state
    pub power_state: PowerState,
    /// Device class (e.g., "net", "block", "input")
    pub class: String,
    /// Sysfs path
    pub sysfs_path: String,
    /// Devfs path (e.g., "/dev/sda")
    pub devfs_path: Option<String>,
    /// Major/minor numbers (for char/block devices)
    pub major: u32,
    pub minor: u32,
    /// Properties (key-value pairs, for uevent)
    pub properties: Vec<(String, String)>,
    /// Active flag
    pub active: bool,
}

/// A bus type
pub struct BusType {
    pub name: String,
    pub devices: Vec<u32>,
    pub drivers: Vec<String>,
}

/// A driver
pub struct Driver {
    pub name: String,
    pub bus: String,
    pub devices: Vec<u32>,
    pub probe_fn: Option<fn(u32) -> bool>,
}

/// Uevent action
#[derive(Debug, Clone, Copy)]
pub enum UeventAction {
    Add,
    Remove,
    Change,
    Move,
    Online,
    Offline,
    Bind,
    Unbind,
}

/// Uevent
pub struct Uevent {
    pub action: UeventAction,
    pub devpath: String,
    pub subsystem: String,
    pub properties: Vec<(String, String)>,
}

/// Device model registry
pub struct DeviceModel {
    devices: Vec<Device>,
    buses: Vec<BusType>,
    drivers: Vec<Driver>,
    next_id: u32,
    uevent_queue: Vec<Uevent>,
}

impl DeviceModel {
    const fn new() -> Self {
        DeviceModel {
            devices: Vec::new(),
            buses: Vec::new(),
            drivers: Vec::new(),
            next_id: 1,
            uevent_queue: Vec::new(),
        }
    }

    /// Register a bus type
    pub fn register_bus(&mut self, name: &str) {
        self.buses.push(BusType {
            name: String::from(name),
            devices: Vec::new(),
            drivers: Vec::new(),
        });
        crate::fs::sysfs::register_device("", name);
    }

    /// Register a device
    pub fn register_device(
        &mut self,
        name: &str,
        dev_type: DeviceType,
        bus: &str,
        class: &str,
        parent: u32,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let sysfs_path = format!("/sys/devices/{}/{}", bus, name);
        let dev = Device {
            id,
            name: String::from(name),
            dev_type,
            bus: String::from(bus),
            driver: None,
            parent,
            children: Vec::new(),
            power_state: PowerState::D0Active,
            class: String::from(class),
            sysfs_path: sysfs_path.clone(),
            devfs_path: None,
            major: 0,
            minor: id,
            properties: Vec::new(),
            active: true,
        };

        self.devices.push(dev);

        // Add to bus
        if let Some(b) = self.buses.iter_mut().find(|b| b.name == bus) {
            b.devices.push(id);
        }

        // Register in sysfs
        crate::fs::sysfs::register_device(bus, name);
        if !class.is_empty() {
            crate::fs::sysfs::register_class_device(class, name);
        }

        // Add to parent's children
        if parent > 0 {
            if let Some(p) = self.devices.iter_mut().find(|d| d.id == parent) {
                p.children.push(id);
            }
        }

        // Generate uevent
        self.uevent_queue.push(Uevent {
            action: UeventAction::Add,
            devpath: sysfs_path,
            subsystem: String::from(class),
            properties: alloc::vec![
                (String::from("DEVNAME"), String::from(name)),
                (String::from("DEVTYPE"), format!("{:?}", dev_type)),
            ],
        });

        id
    }

    /// Register a driver
    pub fn register_driver(&mut self, name: &str, bus: &str) {
        self.drivers.push(Driver {
            name: String::from(name),
            bus: String::from(bus),
            devices: Vec::new(),
            probe_fn: None,
        });

        if let Some(b) = self.buses.iter_mut().find(|b| b.name == bus) {
            b.drivers.push(String::from(name));
        }
    }

    /// Bind a driver to a device
    pub fn bind(&mut self, device_id: u32, driver_name: &str) -> bool {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == device_id) {
            dev.driver = Some(String::from(driver_name));
            if let Some(drv) = self.drivers.iter_mut().find(|d| d.name == driver_name) {
                drv.devices.push(device_id);
            }

            self.uevent_queue.push(Uevent {
                action: UeventAction::Bind,
                devpath: dev.sysfs_path.clone(),
                subsystem: dev.class.clone(),
                properties: alloc::vec![(String::from("DRIVER"), String::from(driver_name)),],
            });
            true
        } else {
            false
        }
    }

    /// Set device power state
    pub fn set_power_state(&mut self, device_id: u32, state: PowerState) {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == device_id) {
            dev.power_state = state;
        }
    }

    /// List all devices
    pub fn list_devices(&self) -> Vec<(u32, String, DeviceType, String, Option<String>)> {
        self.devices
            .iter()
            .filter(|d| d.active)
            .map(|d| {
                (
                    d.id,
                    d.name.clone(),
                    d.dev_type,
                    d.bus.clone(),
                    d.driver.clone(),
                )
            })
            .collect()
    }

    /// Get device tree (formatted string)
    pub fn device_tree(&self) -> String {
        let mut s = String::new();
        for dev in &self.devices {
            if !dev.active {
                continue;
            }
            let driver = dev.driver.as_deref().unwrap_or("(none)");
            s.push_str(&format!(
                "{} [{:?}] bus={} driver={} power={:?}\n",
                dev.name, dev.dev_type, dev.bus, driver, dev.power_state
            ));
        }
        s
    }

    /// Pop next uevent
    pub fn pop_uevent(&mut self) -> Option<Uevent> {
        if self.uevent_queue.is_empty() {
            None
        } else {
            Some(self.uevent_queue.remove(0))
        }
    }
}

static DEVICE_MODEL: Mutex<DeviceModel> = Mutex::new(DeviceModel::new());

pub fn init() {
    let mut dm = DEVICE_MODEL.lock();
    // Register standard buses
    dm.register_bus("platform");
    dm.register_bus("pci");
    dm.register_bus("usb");
    dm.register_bus("i2c");
    dm.register_bus("spi");
    dm.register_bus("virtio");

    // Register built-in devices
    dm.register_device("tty0", DeviceType::Tty, "platform", "tty", 0);
    dm.register_device("console", DeviceType::Char, "platform", "tty", 0);
    dm.register_device("mem", DeviceType::Char, "platform", "", 0);

    crate::serial_println!("  [devmodel] Device model initialized (6 buses)");
}

pub fn register_device(name: &str, dev_type: DeviceType, bus: &str, class: &str) -> u32 {
    DEVICE_MODEL
        .lock()
        .register_device(name, dev_type, bus, class, 0)
}
pub fn register_driver(name: &str, bus: &str) {
    DEVICE_MODEL.lock().register_driver(name, bus);
}
pub fn bind(dev_id: u32, driver: &str) -> bool {
    DEVICE_MODEL.lock().bind(dev_id, driver)
}
pub fn list_devices() -> Vec<(u32, String, DeviceType, String, Option<String>)> {
    DEVICE_MODEL.lock().list_devices()
}
pub fn device_tree() -> String {
    DEVICE_MODEL.lock().device_tree()
}
