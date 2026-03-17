use crate::sync::Mutex;
/// Vehicle HAL (Hardware Abstraction Layer) for Genesis
///
/// CAN bus interface, vehicle sensors, speed/RPM,
/// fuel/EV battery, gear, steering, ADAS data.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum VehicleProperty {
    Speed,
    Rpm,
    FuelLevel,
    EvBatteryLevel,
    EvRange,
    Gear,
    SteeringAngle,
    Odometer,
    TirePressure,
    EngineTemp,
    AmbientTemp,
    Headlights,
    DoorOpen,
    SeatBelt,
}

#[derive(Clone, Copy)]
struct VehicleSensor {
    property: VehicleProperty,
    value: i32,
    timestamp: u64,
    unit: SensorUnit,
}

#[derive(Clone, Copy, PartialEq)]
enum SensorUnit {
    Kph,
    Rpm,
    Percent,
    Km,
    Celsius,
    Degrees,
    Psi,
    Boolean,
}

struct VehicleHal {
    sensors: Vec<VehicleSensor>,
    connected: bool,
    vehicle_id: [u8; 17], // VIN
    make_model: [u8; 32],
    make_len: usize,
}

static VEHICLE_HAL: Mutex<Option<VehicleHal>> = Mutex::new(None);

impl VehicleHal {
    fn new() -> Self {
        VehicleHal {
            sensors: Vec::new(),
            connected: false,
            vehicle_id: [0; 17],
            make_model: [0; 32],
            make_len: 0,
        }
    }

    fn update_sensor(&mut self, property: VehicleProperty, value: i32, timestamp: u64) {
        let unit = match property {
            VehicleProperty::Speed => SensorUnit::Kph,
            VehicleProperty::Rpm => SensorUnit::Rpm,
            VehicleProperty::FuelLevel | VehicleProperty::EvBatteryLevel => SensorUnit::Percent,
            VehicleProperty::EvRange | VehicleProperty::Odometer => SensorUnit::Km,
            VehicleProperty::EngineTemp | VehicleProperty::AmbientTemp => SensorUnit::Celsius,
            VehicleProperty::SteeringAngle => SensorUnit::Degrees,
            VehicleProperty::TirePressure => SensorUnit::Psi,
            VehicleProperty::Gear => SensorUnit::Rpm, // using as generic int
            _ => SensorUnit::Boolean,
        };
        if let Some(s) = self.sensors.iter_mut().find(|s| s.property == property) {
            s.value = value;
            s.timestamp = timestamp;
        } else {
            self.sensors.push(VehicleSensor {
                property,
                value,
                timestamp,
                unit,
            });
        }
    }

    fn get_sensor(&self, property: VehicleProperty) -> Option<i32> {
        self.sensors
            .iter()
            .find(|s| s.property == property)
            .map(|s| s.value)
    }
}

pub fn init() {
    let mut h = VEHICLE_HAL.lock();
    *h = Some(VehicleHal::new());
    serial_println!("    Automotive: vehicle HAL (CAN bus, sensors) ready");
}
