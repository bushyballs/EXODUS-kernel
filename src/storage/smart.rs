/// SMART disk health monitoring
///
/// Part of the AIOS storage layer.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

pub struct SmartAttribute {
    pub id: u8,
    pub value: u8,
    pub worst: u8,
    pub threshold: u8,
}

pub struct SmartMonitor {
    drive: usize,
    attributes: Vec<SmartAttribute>,
    healthy: bool,
    current_temperature: Option<u8>,
    power_on_hours: u64,
    reallocated_sectors: u32,
}

impl SmartMonitor {
    pub fn new(drive: usize) -> Self {
        // Populate default SMART attributes for the drive.
        // In a real system, these would be read via ATA SMART READ DATA (command 0xB0).
        let mut attrs = Vec::new();

        // ID 1: Read Error Rate
        attrs.push(SmartAttribute {
            id: 1,
            value: 100,
            worst: 100,
            threshold: 6,
        });
        // ID 5: Reallocated Sectors Count
        attrs.push(SmartAttribute {
            id: 5,
            value: 100,
            worst: 100,
            threshold: 36,
        });
        // ID 9: Power-On Hours
        attrs.push(SmartAttribute {
            id: 9,
            value: 100,
            worst: 100,
            threshold: 0,
        });
        // ID 12: Power Cycle Count
        attrs.push(SmartAttribute {
            id: 12,
            value: 100,
            worst: 100,
            threshold: 0,
        });
        // ID 194: Temperature
        attrs.push(SmartAttribute {
            id: 194,
            value: 35,
            worst: 45,
            threshold: 0,
        });
        // ID 197: Current Pending Sector Count
        attrs.push(SmartAttribute {
            id: 197,
            value: 100,
            worst: 100,
            threshold: 0,
        });
        // ID 198: Offline Uncorrectable Sector Count
        attrs.push(SmartAttribute {
            id: 198,
            value: 100,
            worst: 100,
            threshold: 0,
        });

        serial_println!("  [smart] Monitor created for drive {}", drive);

        SmartMonitor {
            drive,
            attributes: attrs,
            healthy: true,
            current_temperature: Some(35),
            power_on_hours: 0,
            reallocated_sectors: 0,
        }
    }

    pub fn read_attributes(&self) -> Result<Vec<SmartAttribute>, ()> {
        // Return a copy of the current attribute table.
        let mut result = Vec::with_capacity(self.attributes.len());
        for attr in &self.attributes {
            result.push(SmartAttribute {
                id: attr.id,
                value: attr.value,
                worst: attr.worst,
                threshold: attr.threshold,
            });
        }
        Ok(result)
    }

    pub fn is_healthy(&self) -> bool {
        // A drive is healthy if no attribute's current value has fallen
        // below its threshold (for attributes with non-zero thresholds).
        for attr in &self.attributes {
            if attr.threshold > 0 && attr.value < attr.threshold {
                return false;
            }
        }
        self.healthy
    }

    pub fn temperature(&self) -> Option<u8> {
        // Return the temperature from attribute ID 194 if present,
        // otherwise fall back to stored temperature.
        for attr in &self.attributes {
            if attr.id == 194 {
                return Some(attr.value);
            }
        }
        self.current_temperature
    }

    /// Update a SMART attribute value (simulates drive reporting new data).
    pub fn update_attribute(&mut self, id: u8, value: u8) {
        for attr in &mut self.attributes {
            if attr.id == id {
                if value < attr.worst {
                    attr.worst = value;
                }
                attr.value = value;

                // Update convenience fields
                if id == 194 {
                    self.current_temperature = Some(value);
                }
                if id == 5 {
                    // Reallocated sectors: lower value means more reallocations
                    self.reallocated_sectors = (100u32).saturating_sub(value as u32);
                }

                // Check health after update
                if attr.threshold > 0 && value < attr.threshold {
                    self.healthy = false;
                    serial_println!(
                        "  [smart] Drive {} attribute {} below threshold: {} < {}",
                        self.drive,
                        id,
                        value,
                        attr.threshold
                    );
                }
                return;
            }
        }
    }

    /// Return the drive index this monitor is tracking.
    pub fn drive_index(&self) -> usize {
        self.drive
    }

    /// Return reallocated sector count.
    pub fn reallocated_sectors(&self) -> u32 {
        self.reallocated_sectors
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

pub struct SmartSubsystem {
    monitors: Vec<SmartMonitor>,
}

impl SmartSubsystem {
    const fn new() -> Self {
        SmartSubsystem {
            monitors: Vec::new(),
        }
    }
}

static SMART_SUBSYSTEM: Mutex<Option<SmartSubsystem>> = Mutex::new(None);

pub fn init() {
    let mut guard = SMART_SUBSYSTEM.lock();
    *guard = Some(SmartSubsystem::new());
    serial_println!("  [storage] SMART health monitoring initialized");
}

/// Access the SMART subsystem under lock.
pub fn with_smart<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut SmartSubsystem) -> R,
{
    let mut guard = SMART_SUBSYSTEM.lock();
    guard.as_mut().map(f)
}
