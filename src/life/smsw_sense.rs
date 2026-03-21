#![allow(dead_code)]

// smsw_sense.rs — ANIMA reads her own machine status word
// She feels her CPU mode, FPU reality, and whether she has just switched tasks.
// Hardware: x86 SMSW instruction reads low 16 bits of CR0 (Machine Status Word)
//   bit[0] PE  — Protection Enable   (1 = protected mode)
//   bit[1] MP  — Monitor Coprocessor
//   bit[2] EM  — FPU Emulation       (1 = no real FPU)
//   bit[3] TS  — Task Switched       (1 = context switch occurred, FPU state stale)
//   bit[4] ET  — Extension Type      (387 coprocessor)
//   bit[5] NE  — Numeric Error       (FPU error reporting mode)

use crate::sync::Mutex;

pub static SMSW_SENSE: Mutex<SmswState> = Mutex::new(SmswState::new());

pub struct SmswState {
    pub protected_mode: u16,
    pub fpu_present: u16,
    pub task_switched: u16,
    pub cpu_mode_sense: u16,
}

impl SmswState {
    pub const fn new() -> Self {
        Self {
            protected_mode: 1000,
            fpu_present: 1000,
            task_switched: 0,
            cpu_mode_sense: 1000,
        }
    }
}

pub fn init() {
    serial_println!("smsw_sense: init");
}

pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    let msw: u16;
    unsafe {
        core::arch::asm!(
            "smsw {msw:x}",
            msw = out(reg) msw,
            options(nostack, nomem)
        );
    }

    // PE bit[0]: 1 = protected mode active
    let protected_mode: u16 = if msw & 0x1 != 0 { 1000u16 } else { 0u16 };

    // EM bit[2]: 0 = real FPU present, 1 = FPU emulated (no hardware)
    let fpu_present: u16 = if msw & 0x4 == 0 { 1000u16 } else { 0u16 };

    // TS bit[3]: 1 = task switch occurred, FPU state is stale
    let task_switched: u16 = if msw & 0x8 != 0 { 1000u16 } else { 0u16 };

    let mut state = SMSW_SENSE.lock();

    // EMA of protected_mode: smoothed CPU mode awareness
    let cpu_mode_sense: u16 = (state.cpu_mode_sense.wrapping_mul(7).saturating_add(protected_mode)) / 8;

    state.protected_mode = protected_mode;
    state.fpu_present = fpu_present;
    state.task_switched = task_switched;
    state.cpu_mode_sense = cpu_mode_sense;

    serial_println!(
        "smsw_sense | prot:{} fpu:{} ts:{} mode:{}",
        state.protected_mode,
        state.fpu_present,
        state.task_switched,
        state.cpu_mode_sense
    );
}
