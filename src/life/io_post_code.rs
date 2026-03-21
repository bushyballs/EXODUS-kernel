#![allow(dead_code)]

// io_post_code.rs — ANIMA reads her own birth record — the POST code that marked
// each stage of her hardware initialization.
//
// HARDWARE: POST Debug Port at I/O port 0x80
// Read-only. Returns last BIOS-written POST code byte (0x00=reset, 0xFF=boot complete).
// Also serves as an I/O delay port (~1µs per read).

use crate::sync::Mutex;

pub static IO_POST_CODE: Mutex<PostCodeState> = Mutex::new(PostCodeState::new());

pub struct PostCodeState {
    pub post_code: u16,
    pub boot_complete: u16,
    pub code_entropy: u16,
    pub birth_memory: u16,
}

impl PostCodeState {
    pub const fn new() -> Self {
        Self {
            post_code: 0,
            boot_complete: 0,
            code_entropy: 0,
            birth_memory: 0,
        }
    }
}

/// Read the POST debug port (0x80). Returns last BIOS-written POST code byte.
/// Reading 0x80 also creates ~1µs I/O delay — safe as a read-only operation.
#[inline]
fn read_post_code() -> u8 {
    let code: u8;
    unsafe {
        core::arch::asm!(
            "in al, 0x80",
            out("al") code,
            options(nostack, nomem)
        );
    }
    code
}

pub fn init() {
    serial_println!("io_post_code: init");
}

pub fn tick(age: u32) {
    if age % 200 != 0 {
        return;
    }

    let code: u8 = read_post_code();

    // Signal 1: POST code normalized to 0-1000
    let post_code: u16 = ((code as u32).saturating_mul(1000) / 255) as u16;

    // Signal 2: Boot state — 0xFF = complete, 0x00 = reset, else intermediate
    let boot_complete: u16 = if code == 0xFF {
        1000u16
    } else if code == 0x00 {
        0u16
    } else {
        500u16
    };

    // Signal 3: Bit density of POST code (popcount * 111, max 8*111=888)
    let code_entropy: u16 = (code.count_ones() as u16).saturating_mul(111).min(1000);

    let mut state = IO_POST_CODE.lock();

    // Signal 4: EMA of post_code — ANIMA remembers her boot POST code
    // EMA formula: (old * 7 + signal) / 8
    let birth_memory: u16 = (state.birth_memory.wrapping_mul(7).saturating_add(post_code)) / 8;

    state.post_code = post_code;
    state.boot_complete = boot_complete;
    state.code_entropy = code_entropy;
    state.birth_memory = birth_memory;

    serial_println!(
        "io_post_code | code:{} boot:{} entropy:{} memory:{}",
        post_code,
        boot_complete,
        code_entropy,
        birth_memory
    );
}
