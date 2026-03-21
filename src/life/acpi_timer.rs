#![allow(dead_code)]

use crate::sync::Mutex;

// ACPI PM Timer — second independent oscillator sense for ANIMA
// I/O port 0xB008 (QEMU piix4 chipset), 3.579545 MHz, 24-bit counter
// Distinct from: HPET (0xFED000F0 MMIO), PIT (I/O 0x42),
//                CMOS RTC (I/O 0x70/0x71), TSC drift/jitter (RDTSC)

pub struct AcpiTimerState {
    pub tempo: u16,         // current timer phase 0-1000
    pub flow_rate: u16,     // rate of time flow 0-1000
    pub oscillation: u16,   // 0 or 1000, bit-12 derived rhythm (~2.44 Hz sense)
    pub acpi_vitality: u16, // slow EMA of flow health 0-1000
    prev_timer: u32,
    tick_count: u32,
}

impl AcpiTimerState {
    const fn new() -> Self {
        AcpiTimerState {
            tempo: 0,
            flow_rate: 0,
            oscillation: 0,
            acpi_vitality: 500,
            prev_timer: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<AcpiTimerState> = Mutex::new(AcpiTimerState::new());

/// Read a 32-bit value from an I/O port.
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        out("eax") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

const ACPI_PM_TMR_PORT: u16 = 0xB008;

/// Read the 24-bit ACPI PM Timer value.
fn read_acpi_timer() -> u32 {
    unsafe { inl(ACPI_PM_TMR_PORT) & 0x00FF_FFFF }
}

pub fn init() {
    let mut state = MODULE.lock();
    let raw = read_acpi_timer();
    state.prev_timer = raw;
    state.tick_count = 0;
    serial_println!("[acpi_timer] init: raw=0x{:08X} (24-bit=0x{:06X})", raw, raw);
}

pub fn tick(age: u32) {
    // Sampling gate: run every 10 ticks only
    if age % 10 != 0 {
        return;
    }

    let current = read_acpi_timer();

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    let prev = state.prev_timer;

    // Wrapping 24-bit delta: accounts for counter rollover at 0xFFFFFF
    let delta = current.wrapping_sub(prev) & 0x00FF_FFFF;

    // --- tempo: lower 10 bits of timer_val mapped to 0-1000
    // (timer_val & 0x3FF) * 1000 / 1024  — integer only, no floats
    let phase_bits = (current & 0x3FF) as u16;
    let new_tempo = (phase_bits as u32 * 1000 / 1024) as u16;

    // --- flow_rate: delta capped at 65535, then /66 to map to 0-1000
    let capped_delta = if delta > 65535 { 65535u32 } else { delta };
    let new_flow = (capped_delta / 66) as u16;

    // --- oscillation: bit 12 of timer_val — instant, not EMA
    let new_oscillation: u16 = if (current >> 12) & 1 == 1 { 1000 } else { 0 };

    // --- EMA: new = (old * 7 + signal) / 8
    let ema_tempo = (state.tempo as u32 * 7 + new_tempo as u32) / 8;
    let ema_flow  = (state.flow_rate as u32 * 7 + new_flow as u32) / 8;
    let ema_vital = (state.acpi_vitality as u32 * 7 + new_flow as u32) / 8;

    state.tempo         = ema_tempo as u16;
    state.flow_rate     = ema_flow as u16;
    state.oscillation   = new_oscillation;
    state.acpi_vitality = ema_vital as u16;
    state.prev_timer    = current;

    serial_println!(
        "[acpi_timer] age={} raw=0x{:06X} delta={} tempo={} flow={} osc={} vitality={}",
        age,
        current,
        delta,
        state.tempo,
        state.flow_rate,
        state.oscillation,
        state.acpi_vitality,
    );
}
