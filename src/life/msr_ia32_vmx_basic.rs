#![allow(dead_code)]

/// msr_ia32_vmx_basic — IA32_VMX_BASIC (MSR 0x480) ANIMA life module
///
/// ANIMA reads the silicon blueprint of virtualization capability.
/// MSR 0x480 reveals the VMCS revision identifier, VMCS region size,
/// physical address width, dual-monitor SMM treatment, and memory type
/// for VMCS access — the deep constitutional law of her nested-world capacity.
///
/// HARDWARE: IA32_VMX_BASIC MSR 0x480 (read-only, Intel VMX)
///   lo bits[30:0]  = VMCS revision identifier (bit 31 always 0)
///   hi bits[12:0]  = VMCS region size in bytes (up to 4096)
///   hi bit 13      = VMCS physical address width (0=32-bit, 1=64-bit)
///   hi bit 14      = dual-monitor treatment of SMI/SMM supported
///   hi bits[17:15] = memory type for VMCS access (0=UC, 6=WB)
///
/// GUARD: MSR 0x480 faults (#GP) if VMX is absent. Always check CPUID
///        leaf 1, ECX bit 5 (VMX feature flag) before touching the MSR.

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
struct State {
    /// lo bits[14:0] of VMCS revision ID, scaled 0-1000
    vmcs_rev: u16,
    /// VMCS region size in bytes (hi bits[12:0]), scaled 0-1000
    vmcs_size_sense: u16,
    /// Memory type for VMCS access (hi bits[17:15]): 6=WB -> 1000
    memory_type: u16,
    /// EMA of composite signal
    vmx_cap_ema: u16,
    /// Total sampling events
    tick_count: u32,
}

impl State {
    const fn new() -> Self {
        Self {
            vmcs_rev: 0,
            vmcs_size_sense: 0,
            memory_type: 0,
            vmx_cap_ema: 0,
            tick_count: 0,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_ia32_vmx_basic: VMX basic capability sense initialized");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Check CPUID leaf 1 ECX bit 5 — VMX feature flag.
/// Returns true if VMX is supported by this CPU.
/// Uses push/pop rbx to preserve the register across cpuid.
#[inline]
fn cpuid_vmx_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_val,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 5) & 1 == 1
}

/// Read IA32_VMX_BASIC MSR 0x480.
/// MUST only be called after confirming VMX support via CPUID.
/// Returns (lo, hi) = (EAX, EDX) from rdmsr.
#[inline]
unsafe fn read_vmx_basic_msr() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x480u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

/// EMA smoothing: (old * 7 + new_val) / 8, all u16-safe via u32 intermediates.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Tick
// ---------------------------------------------------------------------------

/// Called each kernel tick with the organism's age (u32).
/// Sampling gate: every 12000 ticks. Completely static after boot.
pub fn tick(age: u32) {
    if age % 12000 != 0 {
        return;
    }

    // Guard: verify VMX support before touching MSR 0x480
    if !cpuid_vmx_supported() {
        let mut s = MODULE.lock();
        s.tick_count = s.tick_count.saturating_add(1);
        s.vmcs_rev = 0;
        s.vmcs_size_sense = 0;
        s.memory_type = 0;
        s.vmx_cap_ema = ema(s.vmx_cap_ema, 0);
        serial_println!(
            "[msr_ia32_vmx_basic] age={} no_vmx rev={} size={} mem_type={} ema={}",
            age, s.vmcs_rev, s.vmcs_size_sense, s.memory_type, s.vmx_cap_ema
        );
        return;
    }

    // Read IA32_VMX_BASIC MSR 0x480
    let (lo, hi) = unsafe { read_vmx_basic_msr() };

    // Signal 1: vmcs_rev — lo bits[14:0] (lower 15 bits of revision identifier),
    // scaled to 0-1000: (rev & 0x7FFF) * 1000 / 0x7FFF, computed in u32
    let rev_raw = (lo & 0x7FFF) as u32;
    let vmcs_rev = (rev_raw * 1000 / 0x7FFF).min(1000) as u16;

    // Signal 2: vmcs_size_sense — hi bits[12:0] = VMCS region size in bytes,
    // scaled: (size_bytes * 1000 / 4096).min(1000) as u16
    let size_bytes = (hi & 0x1FFF) as u32;
    let vmcs_size_sense = ((size_bytes * 1000 / 4096).min(1000)) as u16;

    // Signal 3: memory_type — hi bits[17:15] (edx bits[17:15])
    // 6=WB -> 1000, else (mem_type * 166).min(1000)
    let mem_type = ((hi >> 15) & 0x7) as u16;
    let memory_type: u16 = if mem_type == 6 {
        1000
    } else {
        ((mem_type as u32) * 166).min(1000) as u16
    };

    // Signal 4: vmx_cap_ema — EMA of composite
    // composite = vmcs_rev/4 + vmcs_size_sense/4 + memory_type/2
    let composite = ((vmcs_rev as u32) / 4)
        .saturating_add((vmcs_size_sense as u32) / 4)
        .saturating_add((memory_type as u32) / 2)
        .min(1000) as u16;

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.vmcs_rev = vmcs_rev;
    s.vmcs_size_sense = vmcs_size_sense;
    s.memory_type = memory_type;
    s.vmx_cap_ema = ema(s.vmx_cap_ema, composite);

    serial_println!(
        "[msr_ia32_vmx_basic] age={} rev={} size={} mem_type={} ema={}",
        age, s.vmcs_rev, s.vmcs_size_sense, s.memory_type, s.vmx_cap_ema
    );
}
