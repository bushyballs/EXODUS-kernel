/// regulator — voltage/current regulator framework
///
/// Manages power supplies (LDOs, buck converters, boost converters).
/// Consumers request enable/disable; last consumer to release disables.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

const MAX_REGULATORS: usize = 32;

#[derive(Copy, Clone, PartialEq)]
pub enum RegulatorMode {
    On,
    Off,
    Eco,
}

#[derive(Copy, Clone)]
pub struct RegulatorConstraints {
    pub min_uV: u32,
    pub max_uV: u32,
    pub min_uA: u32,
    pub max_uA: u32,
    pub always_on: bool,
    pub boot_on: bool,
}

impl RegulatorConstraints {
    pub const fn default() -> Self {
        RegulatorConstraints {
            min_uV: 0,
            max_uV: 0,
            min_uA: 0,
            max_uA: 0,
            always_on: false,
            boot_on: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct Regulator {
    pub id: u32,
    pub name: [u8; 32],
    pub name_len: u8,
    pub voltage_uV: u32,
    pub current_uA: u32,
    pub mode: RegulatorMode,
    pub constraints: RegulatorConstraints,
    pub consumer_count: u32,
    pub active: bool,
}

impl Regulator {
    pub const fn empty() -> Self {
        Regulator {
            id: 0,
            name: [0u8; 32],
            name_len: 0,
            voltage_uV: 0,
            current_uA: 0,
            mode: RegulatorMode::Off,
            constraints: RegulatorConstraints::default(),
            consumer_count: 0,
            active: false,
        }
    }
}

const EMPTY_REG: Regulator = Regulator::empty();
static REGULATORS: Mutex<[Regulator; MAX_REGULATORS]> = Mutex::new([EMPTY_REG; MAX_REGULATORS]);
static REG_NEXT_ID: AtomicU32 = AtomicU32::new(1);

fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let len = src.len().min(31);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

pub fn regulator_register(name: &[u8], min_uV: u32, max_uV: u32) -> Option<u32> {
    let id = REG_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if !regs[i].active {
            regs[i] = Regulator::empty();
            regs[i].id = id;
            regs[i].name_len = copy_name(&mut regs[i].name, name);
            regs[i].constraints.min_uV = min_uV;
            regs[i].constraints.max_uV = max_uV;
            regs[i].voltage_uV = min_uV;
            regs[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn regulator_unregister(id: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            regs[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn regulator_enable(id: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            regs[i].mode = RegulatorMode::On;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn regulator_disable(id: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            if regs[i].consumer_count == 0 {
                regs[i].mode = RegulatorMode::Off;
                return true;
            }
            return false; // still has consumers
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn regulator_set_voltage(id: u32, target_uV: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            let clamped = target_uV
                .max(regs[i].constraints.min_uV)
                .min(regs[i].constraints.max_uV);
            regs[i].voltage_uV = clamped;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn regulator_get_voltage(id: u32) -> Option<u32> {
    let regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            return Some(regs[i].voltage_uV);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn regulator_get_mode(id: u32) -> Option<RegulatorMode> {
    let regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            return Some(regs[i].mode);
        }
        i = i.saturating_add(1);
    }
    None
}

pub fn regulator_add_consumer(id: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            regs[i].consumer_count = regs[i].consumer_count.saturating_add(1);
            regs[i].mode = RegulatorMode::On;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn regulator_remove_consumer(id: u32) -> bool {
    let mut regs = REGULATORS.lock();
    let mut i = 0usize;
    while i < MAX_REGULATORS {
        if regs[i].active && regs[i].id == id {
            regs[i].consumer_count = regs[i].consumer_count.saturating_sub(1);
            if regs[i].consumer_count == 0 && !regs[i].constraints.always_on {
                regs[i].mode = RegulatorMode::Off;
            }
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    // Register 4 standard power supplies
    let supplies: &[(&[u8], u32, u32, bool)] = &[
        (b"vdd-core", 800_000, 1_200_000, true),
        (b"vdd-io", 1_800_000, 3_300_000, true),
        (b"vdd-usb", 3_300_000, 3_300_000, true),
        (b"vdd-pll", 1_200_000, 1_200_000, true),
    ];
    let mut k = 0usize;
    while k < supplies.len() {
        if let Some(id) = regulator_register(supplies[k].0, supplies[k].1, supplies[k].2) {
            let mut regs = REGULATORS.lock();
            let mut i = 0usize;
            while i < MAX_REGULATORS {
                if regs[i].active && regs[i].id == id {
                    regs[i].constraints.boot_on = supplies[k].3;
                    regs[i].constraints.always_on = supplies[k].3;
                    regs[i].mode = RegulatorMode::On;
                    regs[i].voltage_uV = supplies[k].1;
                    break;
                }
                i = i.saturating_add(1);
            }
        }
        k = k.saturating_add(1);
    }
    serial_println!("[regulator] voltage regulator framework initialized");
}
