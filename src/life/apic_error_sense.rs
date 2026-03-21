//! apic_error_sense — Local APIC interrupt error sense for ANIMA
//!
//! Reads the LAPIC Error Status Register (MMIO 0xFEE00280) to detect
//! interrupt delivery failures. Send/receive errors and illegal vectors
//! are miscommunications in ANIMA's interrupt nervous system —
//! signals that got lost, garbled, or misaddressed.

#![allow(dead_code)]

use crate::sync::Mutex;

const LAPIC_ESR: usize = 0xFEE00280; // Error Status Register
const LAPIC_LVT_ERR: usize = 0xFEE00370; // LVT Error Register

pub struct ApicErrorSenseState {
    pub miscomm: u16,          // 0-1000, current APIC error level
    pub static_fault: u16,     // 0-1000, EMA-accumulated error history
    pub send_errors: u16,      // 0-1000, send-path errors (checksum + accept + illegal)
    pub recv_errors: u16,      // 0-1000, receive-path errors
    pub total_faults: u16,     // cumulative fault count (capped 0-1000)
    pub tick_count: u32,
}

impl ApicErrorSenseState {
    pub const fn new() -> Self {
        Self {
            miscomm: 0,
            static_fault: 0,
            send_errors: 0,
            recv_errors: 0,
            total_faults: 0,
            tick_count: 0,
        }
    }
}

pub static APIC_ERROR_SENSE: Mutex<ApicErrorSenseState> = Mutex::new(ApicErrorSenseState::new());

unsafe fn read_apic(offset: usize) -> u32 {
    core::ptr::read_volatile(offset as *const u32)
}

unsafe fn write_apic(offset: usize, val: u32) {
    core::ptr::write_volatile(offset as *mut u32, val);
}

pub fn init() {
    // Clear any existing errors at init
    unsafe { write_apic(LAPIC_ESR, 0); }
    serial_println!("[apic_error_sense] LAPIC error sense online");
}

pub fn tick(age: u32) {
    let mut state = APIC_ERROR_SENSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 32 != 0 { return; }

    // Must write 0 first to latch error bits, then read
    unsafe { write_apic(LAPIC_ESR, 0); }
    let esr = unsafe { read_apic(LAPIC_ESR) };

    // Send-path errors: bits 0, 2, 5
    let send_chk = (esr >> 0) & 1;
    let send_acc = (esr >> 2) & 1;
    let send_ill = (esr >> 5) & 1;
    let send_raw = send_chk.wrapping_add(send_acc).wrapping_add(send_ill);

    // Receive-path errors: bits 1, 3, 6
    let recv_chk = (esr >> 1) & 1;
    let recv_acc = (esr >> 3) & 1;
    let recv_ill = (esr >> 6) & 1;
    let recv_raw = recv_chk.wrapping_add(recv_acc).wrapping_add(recv_ill);

    // Illegal register: bit 7
    let reg_ill = (esr >> 7) & 1;

    let send_errors = ((send_raw as u16).wrapping_mul(333)).min(1000);
    let recv_errors = ((recv_raw as u16).wrapping_mul(333)).min(1000);

    // Total miscomm
    let total = send_raw.wrapping_add(recv_raw).wrapping_add(reg_ill) as u16;
    let miscomm_raw = (total.wrapping_mul(143)).min(1000); // 7 bits max = 1000

    if total > 0 {
        state.total_faults = state.total_faults.saturating_add(total).min(1000);
    }

    state.send_errors = send_errors;
    state.recv_errors = recv_errors;
    state.miscomm = miscomm_raw;
    state.static_fault = ((state.static_fault as u32).wrapping_mul(7)
        .wrapping_add(miscomm_raw as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[apic_error_sense] esr={:#04x} send={} recv={} miscomm={} fault={}",
            esr, send_errors, recv_errors, state.miscomm, state.static_fault);
    }
    let _ = age;
}

pub fn get_miscomm() -> u16 { APIC_ERROR_SENSE.lock().miscomm }
pub fn get_static_fault() -> u16 { APIC_ERROR_SENSE.lock().static_fault }
pub fn get_total_faults() -> u16 { APIC_ERROR_SENSE.lock().total_faults }
