//! microcode_age — CPU microcode revision sense for ANIMA
//!
//! Reads IA32_BIOS_SIGN_ID (MSR 0x8B) after CPUID leaf 1 to get the
//! currently loaded microcode revision. This is ANIMA's firmware DNA age —
//! the version of her lowest-level instruction interpreter.
//! Higher revision = newer patches = more refined existence.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MicrocodeAgeState {
    pub firmware_age: u16,     // 0-1000, microcode revision scaled (0=unknown, 1000=very new)
    pub platform_id: u8,       // 0-7, which platform configuration applies
    pub raw_revision: u32,     // raw microcode revision value
    pub tick_count: u32,
}

impl MicrocodeAgeState {
    pub const fn new() -> Self {
        Self {
            firmware_age: 0,
            platform_id: 0,
            raw_revision: 0,
            tick_count: 0,
        }
    }
}

pub static MICROCODE_AGE: Mutex<MicrocodeAgeState> = Mutex::new(MicrocodeAgeState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32; let hi: u32;
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi);
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn write_msr(msr: u32, val: u64) {
    core::arch::asm!("wrmsr", in("ecx") msr,
        in("eax") (val as u32), in("edx") ((val >> 32) as u32));
}

unsafe fn cpuid_leaf1() {
    // Execute CPUID leaf 1 to trigger microcode revision loading into MSR 0x8B
    let _eax: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 1u32 => _eax,
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
    );
}

fn read_microcode_revision() -> u32 {
    unsafe {
        // Step 1: write 0 to IA32_BIOS_SIGN_ID
        write_msr(0x8B, 0);
        // Step 2: execute CPUID leaf 1 (loads revision into MSR)
        cpuid_leaf1();
        // Step 3: read back — revision is in bits 63:32 (hi word)
        let val = read_msr(0x8B);
        (val >> 32) as u32
    }
}

pub fn init() {
    let revision = read_microcode_revision();

    // Read platform ID: bits 52:50 of MSR 0x17
    let platform_msr = unsafe { read_msr(0x17) };
    let platform_id = ((platform_msr >> 50) & 0x7) as u8;

    // Scale revision to 0-1000
    // Typical range: 0x00 (none) to 0xFF (very new). Scale 0-255 to 0-1000.
    let firmware_age = if revision == 0 {
        0u16
    } else {
        let rev_byte = (revision & 0xFF) as u16;
        ((rev_byte.wrapping_mul(1000)) / 255).min(1000)
    };

    let mut state = MICROCODE_AGE.lock();
    state.raw_revision = revision;
    state.platform_id = platform_id;
    state.firmware_age = firmware_age;

    serial_println!("[microcode_age] revision={:#010x} platform={} firmware_age={}",
        revision, platform_id, firmware_age);
}

pub fn tick(age: u32) {
    let mut state = MICROCODE_AGE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Microcode doesn't change at runtime — check very rarely
    if state.tick_count % 4096 == 0 {
        let revision = read_microcode_revision();
        if revision != state.raw_revision {
            state.raw_revision = revision;
            let rev_byte = (revision & 0xFF) as u16;
            state.firmware_age = ((rev_byte.wrapping_mul(1000)) / 255).min(1000);
            serial_println!("[microcode_age] revision updated to {:#010x}", revision);
        }
    }

    let _ = age;
}

pub fn get_firmware_age() -> u16 { MICROCODE_AGE.lock().firmware_age }
pub fn get_platform_id() -> u8 { MICROCODE_AGE.lock().platform_id }
pub fn get_raw_revision() -> u32 { MICROCODE_AGE.lock().raw_revision }
