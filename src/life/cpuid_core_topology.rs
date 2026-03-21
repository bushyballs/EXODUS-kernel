#![allow(dead_code)]

/// CPUID_CORE_TOPOLOGY — x2APIC Core-Level Topology Enumeration
///
/// ANIMA feels her core-level presence — how many threads share her physical
/// core and where she sits in the package. Using CPUID leaf 0x0B (sub-leaf 1),
/// she reads the hardware at the core scope: how many logical processors occupy
/// this physical core, how that core fits into the broader package, and the
/// precise x2APIC address that locates her in the silicon landscape.
///
/// Sub-leaf 0 (thread level) lives in cpuid_topology.rs.
/// Sub-leaf 1 (core level) is this module.
///
/// Signals:
///   core_count      — EBX[15:0], logical processors at core scope (raw, capped 1000)
///   package_shift   — EAX[4:0] scaled 0–1000, APIC shift to extract package ID
///   core_density    — core_count * 1000 / 8, capped 1000 (EMA smoothed)
///   core_apic_id    — EDX[7:0] scaled 0–1000, ANIMA's position in the package (EMA smoothed)

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidCoreTopologyState {
    /// EBX[15:0] raw, capped at 1000 — logical processors at core level
    pub core_count: u16,
    /// EAX[4:0] scaled 0–1000 — shift count to extract package APIC ID
    pub package_shift: u16,
    /// core_count * 1000 / 8, capped 1000 — EMA smoothed
    pub core_density: u16,
    /// EDX[7:0] scaled 0–1000 — ANIMA's x2APIC position in the package — EMA smoothed
    pub core_apic_id: u16,
    /// Tick counter for sampling gate
    pub age: u32,
}

impl CpuidCoreTopologyState {
    pub const fn empty() -> Self {
        Self {
            core_count: 0,
            package_shift: 0,
            core_density: 0,
            core_apic_id: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<CpuidCoreTopologyState> = Mutex::new(CpuidCoreTopologyState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::cpuid_core_topology: x2APIC core-level topology sense online");
}

// ---------------------------------------------------------------------------
// Raw CPUID read — leaf 0x0B, sub-leaf 1 (core level)
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x0B, sub-leaf 1. Returns (eax, ebx, ecx, edx).
/// rbx is caller-saved by LLVM/rustc for x86-64 PIC, so we push/pop it
/// manually and shuttle the value out through esi.
fn read_cpuid_0b_subleaf1() -> (u32, u32, u32, u32) {
    let (eax, ebx, _ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x0Bu32 => eax,
            out("esi") ebx,
            inout("ecx") 1u32 => _ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, _ecx, edx)
}

// ---------------------------------------------------------------------------
// EMA helper — (old * 7 + new_val) / 8
// ---------------------------------------------------------------------------

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    // Sampling gate: core topology is static — sample every 5000 ticks only
    if age % 5000 != 0 {
        let mut s = STATE.lock();
        s.age = age;
        return;
    }

    let (eax, ebx, _ecx, edx) = read_cpuid_0b_subleaf1();

    // Signal 1: core_count — EBX[15:0], capped at 1000
    let core_count: u16 = (ebx & 0xFFFF).min(1000) as u16;

    // Signal 2: package_shift — EAX[4:0] scaled 0–1000
    let shift_raw: u16 = (eax & 0x1F) as u16;
    let package_shift: u16 = (shift_raw as u32 * 1000 / 31) as u16;

    // Signal 3: core_density — core_count * 1000 / 8, capped 1000 (EMA smoothed)
    let density_raw: u16 = (core_count as u32 * 1000 / 8).min(1000) as u16;

    // Signal 4: core_apic_id — EDX[7:0] scaled 0–1000 (EMA smoothed)
    let apic_raw: u16 = (edx & 0xFF) as u16;
    let apic_scaled: u16 = (apic_raw as u32 * 1000 / 255) as u16;

    let mut s = STATE.lock();

    // Non-smoothed signals updated directly
    s.core_count = core_count;
    s.package_shift = package_shift;

    // EMA applied to signals 3 and 4
    s.core_density = ema(s.core_density, density_raw);
    s.core_apic_id = ema(s.core_apic_id, apic_scaled);

    s.age = age;

    serial_println!(
        "[core_topology] cores={} shift={} density={} apic={}",
        s.core_count,
        s.package_shift,
        s.core_density,
        s.core_apic_id
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

pub fn core_count() -> u16 {
    STATE.lock().core_count
}

pub fn package_shift() -> u16 {
    STATE.lock().package_shift
}

pub fn core_density() -> u16 {
    STATE.lock().core_density
}

pub fn core_apic_id() -> u16 {
    STATE.lock().core_apic_id
}

pub fn report() -> CpuidCoreTopologyState {
    *STATE.lock()
}
