use crate::ml::{model, ops};
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

pub const SYSCALL_CATEGORIES: usize = 32;
pub const WINDOW_SLOTS: usize = 64;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SyscallWindow {
    pub pid: u32,
    pub histogram: [u16; SYSCALL_CATEGORIES],
    pub count: u32,
    pub last_update: u32,
}

impl SyscallWindow {
    pub const fn empty() -> Self {
        Self {
            pid: 0,
            histogram: [0; SYSCALL_CATEGORIES],
            count: 0,
            last_update: 0,
        }
    }
}

pub static WINDOWS: Mutex<[SyscallWindow; WINDOW_SLOTS]> =
    Mutex::new([SyscallWindow::empty(); WINDOW_SLOTS]);

static GLOBAL_TICK: AtomicU32 = AtomicU32::new(1);

fn clamp_i8(value: i32) -> i8 {
    if value > i8::MAX as i32 {
        i8::MAX
    } else if value < i8::MIN as i32 {
        i8::MIN
    } else {
        value as i8
    }
}

pub fn update_window(pid: u32, syscall_nr: usize) {
    let slot_idx = (pid as usize) & (WINDOW_SLOTS - 1);
    let mut windows = WINDOWS.lock();
    let slot = &mut windows[slot_idx];

    if slot.pid != pid {
        *slot = SyscallWindow::empty();
        slot.pid = pid;
    }

    let category = syscall_nr & (SYSCALL_CATEGORIES - 1);
    slot.histogram[category] = slot.histogram[category].saturating_add(1);
    slot.count = slot.count.saturating_add(1);

    let tick = GLOBAL_TICK
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    slot.last_update = tick;
}

pub fn score_process(pid: u32) -> u16 {
    let slot_idx = (pid as usize) & (WINDOW_SLOTS - 1);
    let window = {
        let windows = WINDOWS.lock();
        let w = windows[slot_idx];
        if w.pid == pid {
            w
        } else {
            SyscallWindow::empty()
        }
    };

    if window.count == 0 {
        return 0;
    }

    let mut input = [0i8; SYSCALL_CATEGORIES];
    let mut i = 0usize;
    while i < SYSCALL_CATEGORIES {
        let count_i = window.histogram[i] as u32;
        let ratio_q8_8 = count_i.saturating_mul(256) / window.count;
        let centered = (ratio_q8_8 as i32).saturating_sub(128);
        input[i] = clamp_i8(centered);
        i = i.saturating_add(1);
    }

    let mut logits = [0i8; 8];
    let produced = model::inference(
        &input,
        &model::ANOMALY_MODEL,
        &model::ANOMALY_WEIGHTS,
        &mut logits,
    );
    if produced < 2 {
        return 0;
    }

    let mut probs = [0u16; 2];
    ops::softmax_fixed(&logits[..2], &mut probs, 2);
    let score = (probs[1] as u32).saturating_mul(256) / 65535;
    let score_u16 = if score > u16::MAX as u32 {
        u16::MAX
    } else {
        score as u16
    };

    if score_u16 >= 200 {
        serial_println!(
            "[ml/anomaly] high anomaly pid={} score_q8_8={}",
            pid,
            score_u16
        );
    }

    score_u16
}
