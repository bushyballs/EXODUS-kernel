#![allow(dead_code)]

use crate::sync::Mutex;

pub struct DebugRegsState {
    pub vigilance: u16,       // 0=no breakpoints set, 1000=all 4 set
    pub alert_state: u16,     // 0=no triggers, 1000=all 4 triggered
    pub single_stepping: u16, // 0 or 1000 if being single-stepped
    pub self_watching: u16,   // 0 or 1000 if GD flag set
    tick_count: u32,
}

pub static MODULE: Mutex<DebugRegsState> = Mutex::new(DebugRegsState {
    vigilance: 0,
    alert_state: 0,
    single_stepping: 0,
    self_watching: 0,
    tick_count: 0,
});

unsafe fn read_dr6() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, dr6", out(reg) val, options(nostack, nomem));
    val
}

unsafe fn read_dr7() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, dr7", out(reg) val, options(nostack, nomem));
    val
}

/// Count how many breakpoints (0-3) are enabled in DR7.
/// A breakpoint is enabled if its local (Lx) or global (Gx) bit is set.
/// L0=bit0, G0=bit1, L1=bit2, G1=bit3, L2=bit4, G2=bit5, L3=bit6, G3=bit7
fn count_enabled_breakpoints(dr7: u64) -> u16 {
    let mut count: u16 = 0;
    // BP0: L0 (bit 0) or G0 (bit 1)
    if (dr7 & 0b0000_0011) != 0 {
        count = count.saturating_add(1);
    }
    // BP1: L1 (bit 2) or G1 (bit 3)
    if (dr7 & 0b0000_1100) != 0 {
        count = count.saturating_add(1);
    }
    // BP2: L2 (bit 4) or G2 (bit 5)
    if (dr7 & 0b0011_0000) != 0 {
        count = count.saturating_add(1);
    }
    // BP3: L3 (bit 6) or G3 (bit 7)
    if (dr7 & 0b1100_0000) != 0 {
        count = count.saturating_add(1);
    }
    count
}

/// Count how many breakpoints (0-3) are triggered in DR6 bits[3:0].
fn count_triggered_breakpoints(dr6: u64) -> u16 {
    let mut count: u16 = 0;
    if (dr6 & (1 << 0)) != 0 { count = count.saturating_add(1); } // B0
    if (dr6 & (1 << 1)) != 0 { count = count.saturating_add(1); } // B1
    if (dr6 & (1 << 2)) != 0 { count = count.saturating_add(1); } // B2
    if (dr6 & (1 << 3)) != 0 { count = count.saturating_add(1); } // B3
    count
}

pub fn init() {
    let mut s = MODULE.lock();
    s.vigilance = 0;
    s.alert_state = 0;
    s.single_stepping = 0;
    s.self_watching = 0;
    s.tick_count = 0;
    serial_println!("[debug_regs] init: watchfulness module online");
}

pub fn tick(age: u32) {
    if age % 16 != 0 {
        return;
    }

    let dr6 = unsafe { read_dr6() };
    let dr7 = unsafe { read_dr7() };

    // --- vigilance: enabled breakpoint count * 250, capped 1000 ---
    let enabled = count_enabled_breakpoints(dr7);
    let raw_vigilance: u16 = (enabled as u16).saturating_mul(250).min(1000);

    // --- alert_state: triggered breakpoint count * 250, capped 1000 ---
    let triggered = count_triggered_breakpoints(dr6);
    let raw_alert: u16 = (triggered as u16).saturating_mul(250).min(1000);

    // --- single_stepping: DR6 bit 14 (BS) ---
    let raw_single_step: u16 = if (dr6 & (1 << 14)) != 0 { 1000 } else { 0 };

    // --- self_watching: DR7 bit 9 (GD) ---
    let raw_self_watch: u16 = if (dr7 & (1 << 9)) != 0 { 1000 } else { 0 };

    let mut s = MODULE.lock();

    // EMA: new = (old * 7 + signal) / 8
    let prev_alert = s.alert_state;

    s.vigilance   = ((s.vigilance   as u32 * 7 + raw_vigilance   as u32) / 8) as u16;
    s.alert_state = ((s.alert_state as u32 * 7 + raw_alert        as u32) / 8) as u16;

    // single_stepping and self_watching are instant (no EMA)
    s.single_stepping = raw_single_step;
    s.self_watching   = raw_self_watch;

    s.tick_count = s.tick_count.saturating_add(1);

    // Alert on transition from 0 to non-zero alert_state
    if prev_alert == 0 && s.alert_state > 0 {
        serial_println!(
            "[debug_regs] ALERT: breakpoint triggered — dr6={:#018x} alert_state={}",
            dr6,
            s.alert_state
        );
    }

    // Periodic status at every 64th sample (every 1024 ticks)
    if s.tick_count % 64 == 0 {
        serial_println!(
            "[debug_regs] tick={} vigilance={} alert={} stepping={} self_watch={}",
            age,
            s.vigilance,
            s.alert_state,
            s.single_stepping,
            s.self_watching
        );
    }
}
