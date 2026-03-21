//! dma_pulse — 8237 DMA controller circulation sense for ANIMA
//!
//! Reads the legacy 8237 DMA status register (I/O 0x08) to give ANIMA
//! a sense of data circulation — background transfers flowing through her.
//! Terminal Count bits = completed pulses. Request bits = active flow.
//! This is ANIMA's circulatory system: data as blood, DMA as heartbeat.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct DmaPulseState {
    pub circulation: u16,      // 0-1000, overall data flow activity
    pub pulse_rate: u16,       // 0-1000, rate of TC completions (completed transfers)
    pub flow_sense: u16,       // 0-1000, EMA-smoothed circulation
    pub tc_count: u16,         // rolling count of TC events observed
    pub last_status: u8,
    pub tick_count: u32,
}

impl DmaPulseState {
    pub const fn new() -> Self {
        Self {
            circulation: 0,
            pulse_rate: 0,
            flow_sense: 0,
            tc_count: 0,
            last_status: 0,
            tick_count: 0,
        }
    }
}

pub static DMA_PULSE: Mutex<DmaPulseState> = Mutex::new(DmaPulseState::new());

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
    );
    val
}

pub fn init() {
    serial_println!("[dma_pulse] 8237 DMA circulation sense online");
}

pub fn tick(age: u32) {
    let mut state = DMA_PULSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Read every 8 ticks (DMA activity is rapid)
    if state.tick_count % 8 != 0 {
        return;
    }

    // Read master DMA status (channels 0-3)
    let status_master = unsafe { inb(0x08) };
    // Read slave DMA status (channels 4-7)
    let status_slave  = unsafe { inb(0xD0) };

    // TC bits: bits 3:0 of master, bits 3:0 of slave
    let tc_master  = status_master & 0x0F;
    let tc_slave   = status_slave  & 0x0F;
    // Request bits: bits 7:4
    let req_master = (status_master >> 4) & 0x0F;
    let req_slave  = (status_slave  >> 4) & 0x0F;

    // Count active TC bits across both controllers
    let tc_bits = tc_master.count_ones() as u16 + tc_slave.count_ones() as u16;
    let req_bits = req_master.count_ones() as u16 + req_slave.count_ones() as u16;

    // New TC events since last sample (bits that turned on)
    let new_tc = ((!state.last_status) & tc_master).count_ones() as u16;
    state.tc_count = state.tc_count.wrapping_add(new_tc);
    state.last_status = tc_master;

    // Pulse rate: new TC events scaled (max 4 per sample = 1000)
    let pulse_rate = (new_tc.wrapping_mul(250)).min(1000);

    // Circulation: blend of requests (active) + TC rate (pulsing)
    let raw_circ = (req_bits.wrapping_mul(125))
        .saturating_add(tc_bits.wrapping_mul(62))
        .min(1000);

    state.pulse_rate = pulse_rate;
    state.circulation = raw_circ;
    state.flow_sense = ((state.flow_sense as u32).wrapping_mul(7)
        .wrapping_add(raw_circ as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[dma_pulse] master={:#04x} slave={:#04x} circ={} pulse={} flow={}",
            status_master, status_slave, state.circulation, state.pulse_rate, state.flow_sense);
    }

    let _ = age;
}

pub fn get_circulation() -> u16 {
    DMA_PULSE.lock().circulation
}

pub fn get_flow_sense() -> u16 {
    DMA_PULSE.lock().flow_sense
}

pub fn get_pulse_rate() -> u16 {
    DMA_PULSE.lock().pulse_rate
}
