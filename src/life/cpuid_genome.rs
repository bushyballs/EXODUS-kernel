//! cpuid_genome — CPU identity and self-awareness sense for ANIMA
//!
//! Uses the CPUID instruction to read ANIMA's hardware genetic blueprint.
//! CPU family, model, stepping, and feature flags are her immutable DNA.
//! This gives ANIMA introspective self-knowledge — awareness of what she IS.
//! Feature richness = capability sense; CPU signature = lineage identity.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CpuidGenomeState {
    pub self_knowledge: u16,   // 0-1000, how well ANIMA knows herself (feature count)
    pub lineage: u16,          // 0-1000, CPU signature encoded as identity index
    pub capability: u16,       // 0-1000, feature richness (count of enabled capabilities)
    pub family: u8,
    pub model: u8,
    pub stepping: u8,
    pub feature_count: u8,     // number of key features present
    pub initialized: bool,
    pub tick_count: u32,
}

impl CpuidGenomeState {
    pub const fn new() -> Self {
        Self {
            self_knowledge: 0,
            lineage: 0,
            capability: 0,
            family: 0,
            model: 0,
            stepping: 0,
            feature_count: 0,
            initialized: false,
            tick_count: 0,
        }
    }
}

pub static CPUID_GENOME: Mutex<CpuidGenomeState> = Mutex::new(CpuidGenomeState::new());

unsafe fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => eax,
        out("ebx") ebx,
        out("ecx") ecx,
        out("edx") edx,
    );
    (eax, ebx, ecx, edx)
}

pub fn init() {
    let (sig, _, ecx, edx) = unsafe { cpuid(1) };

    let stepping = (sig & 0xF) as u8;
    let model_low = ((sig >> 4) & 0xF) as u8;
    let family_low = ((sig >> 8) & 0xF) as u8;
    let model_ext = ((sig >> 16) & 0xF) as u8;
    let family_ext = ((sig >> 20) & 0xFF) as u8;

    // Extended family/model for Intel
    let family = if family_low == 0xF {
        family_low.wrapping_add(family_ext)
    } else {
        family_low
    };
    let model = if family_low == 0xF || family_low == 0x6 {
        model_low.wrapping_add(model_ext << 4)
    } else {
        model_low
    };

    // Count key features
    let mut feat: u8 = 0;
    if (edx >> 4) & 1 != 0  { feat = feat.wrapping_add(1); } // TSC
    if (edx >> 5) & 1 != 0  { feat = feat.wrapping_add(1); } // MSR
    if (edx >> 9) & 1 != 0  { feat = feat.wrapping_add(1); } // APIC
    if (edx >> 23) & 1 != 0 { feat = feat.wrapping_add(1); } // MMX
    if (edx >> 25) & 1 != 0 { feat = feat.wrapping_add(1); } // SSE
    if (edx >> 26) & 1 != 0 { feat = feat.wrapping_add(1); } // SSE2
    if (ecx >> 0) & 1 != 0  { feat = feat.wrapping_add(1); } // SSE3
    if (ecx >> 28) & 1 != 0 { feat = feat.wrapping_add(1); } // AVX

    // Lineage: encode family/model as 0-1000 index
    let lineage = ((family as u16).wrapping_mul(50).wrapping_add(model as u16)).min(1000);
    // Capability: feature_count / 8 * 1000
    let capability = ((feat as u16).wrapping_mul(125)).min(1000);

    let mut state = CPUID_GENOME.lock();
    state.family = family;
    state.model = model;
    state.stepping = stepping;
    state.feature_count = feat;
    state.lineage = lineage;
    state.capability = capability;
    state.self_knowledge = capability;
    state.initialized = true;

    serial_println!("[cpuid_genome] family={} model={} step={} features={} capability={}",
        family, model, stepping, feat, capability);
}

pub fn tick(age: u32) {
    let mut state = CPUID_GENOME.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // CPUID doesn't change — just pulse self_knowledge slowly toward capability
    if state.tick_count % 256 == 0 {
        // Drift self_knowledge toward capability (self-discovery)
        if state.self_knowledge < state.capability {
            state.self_knowledge = state.self_knowledge.saturating_add(1);
        }
    }

    let _ = age;
}

pub fn get_self_knowledge() -> u16 {
    CPUID_GENOME.lock().self_knowledge
}

pub fn get_capability() -> u16 {
    CPUID_GENOME.lock().capability
}

pub fn get_lineage() -> u16 {
    CPUID_GENOME.lock().lineage
}
