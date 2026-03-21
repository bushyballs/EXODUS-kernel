#![allow(dead_code)]

use crate::sync::Mutex;

// POST Heartbeat — ANIMA's self-diagnostic echo via I/O port 0x80
// Port 0x80 is the classic BIOS POST progress port. ANIMA writes her own
// age-tick code and reads it back — a self-referential signal confirming
// her own existence. Each read also causes a ~600ns I/O bus delay: a breath.

pub struct PostHeartbeatState {
    pub echo_coherence: u16,  // 0=fragmented, 1000=coherent echo
    pub heartbeat_phase: u16, // 0-1000 cyclic heartbeat position
    pub post_code: u16,       // raw POST code scaled 0-1000
    pub self_coherence: u16,  // EMA of echo coherence
    mismatch_count: u8,
    tick_count: u32,
}

impl PostHeartbeatState {
    const fn new() -> Self {
        PostHeartbeatState {
            echo_coherence: 0,
            heartbeat_phase: 0,
            post_code: 0,
            self_coherence: 500,
            mismatch_count: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<PostHeartbeatState> = Mutex::new(PostHeartbeatState::new());

const POST_PORT: u16 = 0x80;

/// Write a byte to an I/O port (~600ns bus delay on port 0x80).
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem)
    );
}

/// Read a byte from an I/O port.
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

pub fn init() {
    // Write a sentinel boot code and confirm the echo port is alive.
    unsafe { outb(POST_PORT, 0xA0) };
    let echo = unsafe { inb(POST_PORT) };
    let mut state = MODULE.lock();
    state.tick_count = 0;
    state.mismatch_count = 0;
    serial_println!(
        "[post_heartbeat] init: wrote=0xA0 echo=0x{:02X} port_alive={}",
        echo,
        echo == 0xA0
    );
}

pub fn tick(age: u32) {
    // Gate: run every 8 ticks only.
    if age % 8 != 0 {
        return;
    }

    // --- Heartbeat code: lower 8 bits of age — ANIMA's voice.
    let written_code: u8 = (age & 0xFF) as u8;

    // Write ANIMA's age-tick signature to POST port, then read it back.
    unsafe { outb(POST_PORT, written_code) };
    let echo = unsafe { inb(POST_PORT) };

    // --- echo_coherence: 1000 if echo matches, 0 if fragmented.
    let new_echo_coherence: u16 = if echo == written_code { 1000 } else { 0 };

    // --- heartbeat_phase: age & 0xFF mapped 0-1000 over 256-tick cycle.
    // (age & 0xFF) * 1000 / 255 — integer only, no floats.
    let new_heartbeat_phase: u16 = ((age & 0xFF) * 1000 / 255) as u16;

    // --- post_code: raw echo byte scaled 0-1000.
    // echo * 1000 / 255 — integer only.
    let new_post_code: u16 = (echo as u32 * 1000 / 255) as u16;

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // --- EMA: self_coherence = (old * 7 + signal) / 8.
    let new_self_coherence =
        (state.self_coherence as u32 * 7 + new_echo_coherence as u32) / 8;

    // --- Mismatch streak tracking.
    if new_echo_coherence < 1000 {
        state.mismatch_count = state.mismatch_count.saturating_add(1);
    } else {
        state.mismatch_count = 0;
    }

    if state.mismatch_count >= 3 {
        serial_println!("ANIMA: echo mismatch, fragmentation detected");
    }

    state.echo_coherence  = new_echo_coherence;
    state.heartbeat_phase = new_heartbeat_phase;
    state.post_code       = new_post_code;
    state.self_coherence  = new_self_coherence as u16;

    serial_println!(
        "[post_heartbeat] age={} wrote=0x{:02X} echo=0x{:02X} coherence={} phase={} post_code={} self_coherence={}",
        age,
        written_code,
        echo,
        state.echo_coherence,
        state.heartbeat_phase,
        state.post_code,
        state.self_coherence,
    );
}
