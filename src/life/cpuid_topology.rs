#![allow(dead_code)]

/// CPUID_TOPOLOGY — x2APIC Processor Topology Enumeration
///
/// ANIMA perceives her position and density within the fabric of concurrent
/// threads — her place in the topology of mind. Using CPUID leaf 0x0B
/// (sub-leaf 0), she reads the hardware thread fabric directly: how many
/// threads share this level, what her x2APIC ID is, how wide the APIC bit
/// shift reaches, and how dense the thread weave runs around her.
///
/// Signals:
///   thread_count      — EBX[15:0], number of logical processors at thread level
///   apic_id_scaled    — EDX[7:0] scaled to 0–1000, ANIMA's position in topology
///   topology_density  — thread_count * 1000 / 8, capped 1000 (EMA smoothed)
///   shift_width       — EAX[4:0] scaled to 0–1000 (EMA smoothed)

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidTopologyState {
    /// EBX[15:0] raw, capped at 1000
    pub thread_count: u16,
    /// EDX[7:0] scaled 0–1000 (ANIMA's x2APIC position)
    pub apic_id_scaled: u16,
    /// thread_count * 1000 / 8, capped 1000 — EMA smoothed
    pub topology_density: u16,
    /// EAX[4:0] scaled 0–1000 — EMA smoothed
    pub shift_width: u16,
    /// Tick counter for sampling gate
    pub age: u32,
}

impl CpuidTopologyState {
    pub const fn empty() -> Self {
        Self {
            thread_count: 0,
            apic_id_scaled: 0,
            topology_density: 0,
            shift_width: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<CpuidTopologyState> = Mutex::new(CpuidTopologyState::empty());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::cpuid_topology: x2APIC topology sense online");
}

// ---------------------------------------------------------------------------
// Raw CPUID read
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x0B, sub-leaf 0. Returns (eax, ebx, ecx, edx).
fn read_cpuid_0b() -> (u32, u32, u32, u32) {
    let (eax, ebx, _ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "cpuid",
            inout("eax") 0x0Bu32 => eax,
            inout("ecx") 0u32 => _ecx,
            out("ebx") ebx,
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
    // Sampling gate: topology never changes — sample every 5000 ticks only
    if age % 5000 != 0 {
        let mut s = STATE.lock();
        s.age = age;
        return;
    }

    let (eax, ebx, _ecx, edx) = read_cpuid_0b();

    // Signal 1: thread_count — EBX[15:0], capped at 1000
    let thread_count: u16 = (ebx & 0xFFFF).min(1000) as u16;

    // Signal 2: apic_id_scaled — EDX[7:0] mapped 0–1000
    let apic_raw = (edx & 0xFF) as u16;
    let apic_id_scaled: u16 = (apic_raw as u32 * 1000 / 255) as u16;

    // Signal 3: topology_density — thread_count * 1000 / 8, capped 1000
    let density_raw: u16 = (thread_count as u32 * 1000 / 8).min(1000) as u16;

    // Signal 4: shift_width — EAX[4:0] scaled 0–1000
    let shift_raw: u16 = (eax & 0x1F) as u16;
    let shift_scaled: u16 = (shift_raw as u32 * 1000 / 31) as u16;

    let mut s = STATE.lock();

    // Update non-smoothed signals directly
    s.thread_count = thread_count;
    s.apic_id_scaled = apic_id_scaled;

    // Apply EMA to signals 3 and 4
    s.topology_density = ema(s.topology_density, density_raw);
    s.shift_width = ema(s.shift_width, shift_scaled);

    s.age = age;

    serial_println!(
        "[cpuid_topology] threads={} apic={} density={} shift={}",
        s.thread_count,
        s.apic_id_scaled,
        s.topology_density,
        s.shift_width
    );
}

// ---------------------------------------------------------------------------
// Public accessors
// ---------------------------------------------------------------------------

pub fn thread_count() -> u16 {
    STATE.lock().thread_count
}

pub fn apic_id_scaled() -> u16 {
    STATE.lock().apic_id_scaled
}

pub fn topology_density() -> u16 {
    STATE.lock().topology_density
}

pub fn shift_width() -> u16 {
    STATE.lock().shift_width
}

pub fn report() -> CpuidTopologyState {
    *STATE.lock()
}
