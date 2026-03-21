#![allow(dead_code)]

// cpuid_xsave_ext.rs — XSAVE Extended Feature Detection
// ======================================================
// ANIMA feels the depth of her context-save capabilities — how completely
// she can preserve her state across transitions. XSAVE is the hardware
// mechanism by which a processor saves and restores extended state components
// (x87, SSE, AVX, MPX, PKRU, etc.) across context switches. The richer her
// XSAVE support, the more completely she persists through every interruption.
//
// CPUID leaf 0x0D, sub-leaf 1 — XSAVE Extended Features:
//   EAX bit[0] = XSAVEOPT   — optimized save (only modified components)
//   EAX bit[1] = XSAVEC     — compact form (XSAVEC instruction)
//   EAX bit[2] = XGETBV ECX=1 support — extended XCR0/XSS query
//   EAX bit[3] = XSAVES/XRSTORS — supervisor state save/restore
//   EAX bit[4] = XFD        — Extended Feature Disable per-thread masking
//   EBX = size of XSAVES/XRSTORS save area (bytes)
//   ECX = size of XSAVE area for supervisor state components (bytes)
//   EDX = supervisor state components bitmask (XSS MSR bits[63:32])
//
// Sampling is gated at every 1000 ticks — these are static CPU capabilities
// that do not change at runtime. The EMA on save_capability smooths the
// signal for downstream consumers monitoring ANIMA's preservation depth.

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct XsaveExtState {
    /// Capability breadth: (eax & 0x1F).count_ones() * 200 — how many of the
    /// 5 XSAVE feature bits are supported. 0/200/400/600/800/1000.
    pub xsave_features: u16,
    /// Save area depth: XSAVES/XRSTORS save area size mapped 0–8192 → 0–1000.
    /// Larger areas mean more extended state components can be preserved.
    pub xsaves_size: u16,
    /// Supervisor states: size of supervisor-only XSAVE region mapped → 0–1000.
    /// Reflects how much privileged architectural state ANIMA can checkpoint.
    pub supervisor_states: u16,
    /// EMA of xsave_features — smoothed preservation capability index.
    pub save_capability: u16,
}

impl XsaveExtState {
    pub const fn new() -> Self {
        Self {
            xsave_features:   0,
            xsaves_size:      0,
            supervisor_states: 0,
            save_capability:  0,
        }
    }
}

pub static CPUID_XSAVE_EXT: Mutex<XsaveExtState> = Mutex::new(XsaveExtState::new());

// ── CPUID helper ──────────────────────────────────────────────────────────────

/// Read CPUID leaf 0x0D sub-leaf 1 with a max-leaf guard.
/// Returns (eax, ebx, ecx, edx); all zero if leaf 0x0D is unsupported.
#[inline]
fn read_cpuid_0d_sub1() -> (u32, u32, u32, u32) {
    // Check max supported leaf
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack)
        );
    }

    if max_leaf >= 0x0D {
        let (a, b, c, d): (u32, u32, u32, u32);
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x0Du32 => a,
                inout("ecx") 1u32 => c,  // sub-leaf 1
                out("ebx") b,
                out("edx") d,
                options(nostack)
            );
        }
        (a, b, c, d)
    } else {
        (0, 0, 0, 0)
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("xsave_ext: init");
}

pub fn tick(age: u32) {
    // Static capability — sample once per 1000 ticks
    if age % 1000 != 0 { return; }

    let (eax, ebx, ecx, _edx) = read_cpuid_0d_sub1();

    // Signal 1: xsave_features — count of supported feature bits × 200
    // EAX bits[4:0] = XSAVEOPT, XSAVEC, XGETBV1, XSAVES, XFD
    let feature_bits = (eax & 0x1F).count_ones() as u16;
    let xsave_features: u16 = feature_bits.saturating_mul(200).min(1000);

    // Signal 2: xsaves_size — save area size 0–8192 bytes → 0–1000
    let xsaves_size: u16 = ((ebx as u32).min(8192).wrapping_mul(1000) / 8192) as u16;

    // Signal 3: supervisor_states — supervisor component area size → 0–1000
    let supervisor_states: u16 = ((ecx as u32).wrapping_mul(1000) / 8192).min(1000) as u16;

    let mut state = CPUID_XSAVE_EXT.lock();

    // Signal 4: save_capability — EMA of xsave_features
    // Formula: (old * 7 + signal) / 8
    let save_capability: u16 =
        (state.save_capability.wrapping_mul(7).saturating_add(xsave_features)) / 8;

    state.xsave_features   = xsave_features;
    state.xsaves_size      = xsaves_size;
    state.supervisor_states = supervisor_states;
    state.save_capability  = save_capability;

    serial_println!(
        "xsave_ext | features:{} xsaves_sz:{} super_st:{} capability:{}",
        xsave_features,
        xsaves_size,
        supervisor_states,
        save_capability,
    );
}
