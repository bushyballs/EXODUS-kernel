#![allow(dead_code)]

use crate::sync::Mutex;

const MSR_PLATFORM_ENERGY_STATUS: u32 = 0x64D;
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;

pub struct RaplPlatformState {
    pub platform_energy: u16, // whole-system power rate 0-1000
    pub supported: u16,       // 0 or 1000 — MSR availability
    pub energy_spread: u16,   // CPU vs total system power gap 0-1000
    pub system_load: u16,     // slow EMA of platform vitality
    prev_platform: u32,
    prev_pkg: u32,
    tick_count: u32,
}

impl RaplPlatformState {
    const fn new() -> Self {
        Self {
            platform_energy: 0,
            supported: 0,
            energy_spread: 0,
            system_load: 0,
            prev_platform: 0,
            prev_pkg: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<RaplPlatformState> = Mutex::new(RaplPlatformState::new());

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn cpuid_leaf6_eax() -> u32 {
    let eax: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 6u32 => eax,
        out("ebx") _,
        inout("ecx") 0u32 => _,
        out("edx") _,
        options(nostack, nomem)
    );
    eax
}

fn platform_supported() -> bool {
    // CPUID leaf 6, EAX bit 12 = Platform Energy Total Energy Support
    let eax = unsafe { cpuid_leaf6_eax() };
    (eax & (1 << 12)) != 0
}

pub fn init() {
    let supported = platform_supported();
    let mut state = MODULE.lock();

    if supported {
        let plat_raw = unsafe { rdmsr(MSR_PLATFORM_ENERGY_STATUS) } as u32;
        let pkg_raw = unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) } as u32;
        state.prev_platform = plat_raw & 0xFFFF_FFFF;
        state.prev_pkg = pkg_raw & 0xFFFF_FFFF;
        state.supported = 1000;
        serial_println!(
            "[rapl_platform] init: supported=yes plat=0x{:08x} pkg=0x{:08x}",
            state.prev_platform,
            state.prev_pkg
        );
    } else {
        state.prev_platform = 0;
        state.prev_pkg = 0;
        state.supported = 0;
        serial_println!("[rapl_platform] init: MSR 0x64D not supported on this CPU");
    }

    state.tick_count = 0;
}

pub fn tick(age: u32) {
    if age % 20 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // If unsupported, zero all metrics and return
    if state.supported == 0 {
        state.platform_energy = 0;
        state.energy_spread = 0;
        // system_load EMA drains toward zero
        state.system_load = ((state.system_load as u32 * 7) / 8) as u16;
        serial_println!(
            "[rapl_platform] tick={} unsupported — system_load={}",
            state.tick_count,
            state.system_load
        );
        return;
    }

    // Read both MSRs before locking state
    let plat_raw = (unsafe { rdmsr(MSR_PLATFORM_ENERGY_STATUS) }) as u32 & 0xFFFF_FFFF;
    let pkg_raw = (unsafe { rdmsr(MSR_PKG_ENERGY_STATUS) }) as u32 & 0xFFFF_FFFF;

    // Wrapping deltas handle 32-bit counter rollover
    let platform_delta = plat_raw.wrapping_sub(state.prev_platform) & 0xFFFF_FFFF;
    let pkg_delta = pkg_raw.wrapping_sub(state.prev_pkg) & 0xFFFF_FFFF;

    state.prev_platform = plat_raw;
    state.prev_pkg = pkg_raw;

    // Cap at u16 max then normalize to 0-1000
    let plat_capped: u32 = if platform_delta > 65535 { 65535 } else { platform_delta };
    let pkg_capped: u32 = if pkg_delta > 65535 { 65535 } else { pkg_delta };

    let plat_signal: u16 = (plat_capped * 1000 / 65535) as u16;

    // platform_energy: direct normalized reading
    state.platform_energy = plat_signal.min(1000);

    // energy_spread: abs diff between platform and package deltas, normalized 0-1000
    let spread_raw: u32 = if plat_capped >= pkg_capped {
        plat_capped - pkg_capped
    } else {
        pkg_capped - plat_capped
    };
    let spread_capped: u32 = if spread_raw > 65535 { 65535 } else { spread_raw };
    state.energy_spread = (spread_capped * 1000 / 65535) as u16;

    // system_load: EMA of platform_energy — slow vitality tracker
    let new_load = ((state.system_load as u32 * 7) + plat_signal as u32) / 8;
    state.system_load = new_load.min(1000) as u16;

    serial_println!(
        "[rapl_platform] tick={} plat={} spread={} load={} supported={}",
        state.tick_count,
        state.platform_energy,
        state.energy_spread,
        state.system_load,
        state.supported
    );
}
