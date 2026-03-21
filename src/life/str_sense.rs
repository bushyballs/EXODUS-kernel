#![allow(dead_code)]

use crate::sync::Mutex;

pub static STR_SENSE: Mutex<StrState> = Mutex::new(StrState::new());

pub struct StrState {
    pub tss_selector: u16,
    pub tss_index: u16,
    pub tss_privilege: u16,
    pub task_sense: u16,
}

impl StrState {
    pub const fn new() -> Self {
        Self {
            tss_selector: 0,
            tss_index: 0,
            tss_privilege: 0,
            task_sense: 0,
        }
    }
}

pub fn init() {
    serial_println!("str_sense: init");
}

pub fn tick(age: u32) {
    if age % 50 != 0 {
        return;
    }

    let tr: u16;
    unsafe {
        core::arch::asm!(
            "str {tr:x}",
            tr = out(reg) tr,
            options(nostack, nomem)
        );
    }

    let tss_selector: u16 = (tr as u32 * 1000 / 65535) as u16;
    let tss_index: u16 = ((tr >> 3) as u32 * 1000 / 8191) as u16;
    let tss_privilege: u16 = (tr & 0x3) as u16 * 333;

    let mut state = STR_SENSE.lock();

    let task_sense: u16 = (state.task_sense as u32 * 7 + tss_index as u32) as u16 / 8;

    state.tss_selector = tss_selector;
    state.tss_index = tss_index;
    state.tss_privilege = tss_privilege;
    state.task_sense = task_sense;

    serial_println!(
        "str_sense | selector:{} index:{} privilege:{} sense:{}",
        tss_selector,
        tss_index,
        tss_privilege,
        task_sense
    );
}
