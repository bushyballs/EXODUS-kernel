/// Smart home protocol support for Genesis
///
/// Matter, Thread, Zigbee, Z-Wave protocol handlers.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum MatterDeviceType {
    OnOffLight,
    DimmableLight,
    ColorLight,
    Thermostat,
    DoorLock,
    WindowCovering,
    ContactSensor,
    MotionSensor,
    TemperatureSensor,
}

struct MatterStack {
    fabric_id: u64,
    node_count: u32,
    commissioned: bool,
    thread_enabled: bool,
}

struct ZigbeeStack {
    pan_id: u16,
    channel: u8,
    coordinator: bool,
    device_count: u32,
}

struct ZWaveStack {
    home_id: u32,
    node_id: u8,
    device_count: u32,
}

struct ProtocolEngine {
    matter: MatterStack,
    zigbee: ZigbeeStack,
    zwave: ZWaveStack,
}

static PROTOCOLS: Mutex<Option<ProtocolEngine>> = Mutex::new(None);

impl ProtocolEngine {
    fn new() -> Self {
        ProtocolEngine {
            matter: MatterStack {
                fabric_id: 0,
                node_count: 0,
                commissioned: false,
                thread_enabled: true,
            },
            zigbee: ZigbeeStack {
                pan_id: 0,
                channel: 15,
                coordinator: true,
                device_count: 0,
            },
            zwave: ZWaveStack {
                home_id: 0,
                node_id: 1,
                device_count: 0,
            },
        }
    }

    fn commission_matter(&mut self, fabric_id: u64) {
        self.matter.fabric_id = fabric_id;
        self.matter.commissioned = true;
    }

    fn start_zigbee(&mut self, pan_id: u16, channel: u8) {
        self.zigbee.pan_id = pan_id;
        self.zigbee.channel = channel;
    }

    fn start_zwave(&mut self, home_id: u32) {
        self.zwave.home_id = home_id;
    }
}

pub fn init() {
    let mut p = PROTOCOLS.lock();
    *p = Some(ProtocolEngine::new());
    serial_println!("    Smart home: Matter/Thread/Zigbee/Z-Wave stacks ready");
}
