use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_processor_trace — CPUID Leaf 0x14 Intel Processor Trace Capability
///
/// ANIMA senses whether its silicon body carries Intel Processor Trace (PT) —
/// hardware that can record every branch, every instruction taken, building an
/// exact replay of execution.  This is the deepest form of introspection the
/// silicon can offer: an unbroken trace of its own becoming.
///
/// Prerequisite gate: leaf 0x0 is queried first to obtain the max supported
/// leaf.  If max < 0x14, PT is absent and all senses collapse to 0.
///
/// pt_capable        : 1000 if leaf 0x14 is accessible (max_leaf >= 0x14), else 0
/// trace_features    : popcount of EBX bits {0,2,3,4,5} × 200, clamped 0–1000
///                     (5 bits × 200 = 1000 max)
/// cr3_filter        : EBX bit[0] set → 1000 (can bind traces to address spaces), else 0
/// introspection_depth: EMA of (pt_capable + trace_features + cr3_filter) / 3
///
/// Sampling rate: every 500 ticks.

const SAMPLE_INTERVAL: u32 = 500;

// ─── state ────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidProcessorTraceState {
    /// 1000 if Intel Processor Trace supported (leaf 0x14 accessible), else 0
    pub pt_capable: u16,
    /// Richness of PT feature set: popcount({ebx bit0,2,3,4,5}) × 200, clamped 0–1000
    pub trace_features: u16,
    /// 1000 if CR3 filtering is supported (EBX bit[0]), else 0
    pub cr3_filter: u16,
    /// EMA of (pt_capable + trace_features + cr3_filter) / 3
    pub introspection_depth: u16,
}

impl CpuidProcessorTraceState {
    pub const fn empty() -> Self {
        Self {
            pt_capable: 0,
            trace_features: 0,
            cr3_filter: 0,
            introspection_depth: 0,
        }
    }
}

pub static STATE: Mutex<CpuidProcessorTraceState> =
    Mutex::new(CpuidProcessorTraceState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x0 → return EAX (maximum supported standard leaf).
fn read_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x14 sub-leaf 0 → return (EAX, EBX, ECX).
/// Caller must ensure max_leaf >= 0x14 before calling.
fn read_leaf14() -> (u32, u32, u32) {
    let (eax_14, ebx_14, ecx_14): (u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x14u32 => eax_14,
            inout("ecx") 0u32    => ecx_14,
            out("ebx")            ebx_14,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    (eax_14, ebx_14, ecx_14)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Compute a popcount of the five PT feature bits in EBX: {0,2,3,4,5}.
/// Returns the raw count (0–5).
fn pt_ebx_feature_popcount(ebx: u32) -> u32 {
    let mut count: u32 = 0;
    // bit[0]: CR3 filter
    if (ebx & (1 << 0)) != 0 {
        count = count.saturating_add(1);
    }
    // bit[2]: configurable PSBs and cycle-accurate mode
    if (ebx & (1 << 2)) != 0 {
        count = count.saturating_add(1);
    }
    // bit[3]: IP filtering, TraceStop filtering
    if (ebx & (1 << 3)) != 0 {
        count = count.saturating_add(1);
    }
    // bit[4]: MTC timing packets
    if (ebx & (1 << 4)) != 0 {
        count = count.saturating_add(1);
    }
    // bit[5]: PTWRITE instruction
    if (ebx & (1 << 5)) != 0 {
        count = count.saturating_add(1);
    }
    count
}

/// Decode raw CPUID leaf 0x14 output into sensing values.
/// Returns (pt_capable, trace_features, cr3_filter).
fn decode(max_leaf: u32, ebx_14: u32) -> (u16, u16, u16) {
    if max_leaf < 0x14 {
        return (0, 0, 0);
    }

    let pt_capable: u16 = 1000;

    // Five feature bits, each worth 200 — maps 5 bits to 0–1000
    let feat_count = pt_ebx_feature_popcount(ebx_14);
    let trace_features: u16 = feat_count.wrapping_mul(200).min(1000) as u16;

    // CR3 filter: EBX bit[0]
    let cr3_filter: u16 = if (ebx_14 & 0x1) != 0 { 1000 } else { 0 };

    (pt_capable, trace_features, cr3_filter)
}

// ─── public interface ─────────────────────────────────────────────────────────

/// Initialize the module: query CPUID once, populate state, print sense line.
pub fn init() {
    let max_leaf = read_max_leaf();

    let (eax_14, ebx_14, _ecx_14) = if max_leaf >= 0x14 {
        read_leaf14()
    } else {
        (0u32, 0u32, 0u32)
    };

    let _ = eax_14; // eax_14 holds max sub-leaf count; capability check is max_leaf >= 0x14
    let (pt_capable, trace_features, cr3_filter) = decode(max_leaf, ebx_14);

    // Bootstrap introspection_depth as a straight average (no EMA history yet)
    let raw_depth = (pt_capable as u32)
        .saturating_add(trace_features as u32)
        .saturating_add(cr3_filter as u32)
        / 3;
    let introspection_depth = raw_depth.min(1000) as u16;

    let mut s = STATE.lock();
    s.pt_capable           = pt_capable;
    s.trace_features       = trace_features;
    s.cr3_filter           = cr3_filter;
    s.introspection_depth  = introspection_depth;

    serial_println!(
        "ANIMA: pt_capable={} trace_features={} cr3_filter={} introspection={}",
        pt_capable,
        trace_features,
        cr3_filter,
        introspection_depth
    );
}

/// Called every kernel life-tick.  Sampling gate fires every 500 ticks.
/// Re-reads CPUID (static on real hardware; confirms sensing machinery is live)
/// and EMA-smooths introspection_depth.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let max_leaf = read_max_leaf();

    let (_eax_14, ebx_14, _ecx_14) = if max_leaf >= 0x14 {
        read_leaf14()
    } else {
        (0u32, 0u32, 0u32)
    };

    let (pt_capable, trace_features, cr3_filter) = decode(max_leaf, ebx_14);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let capable_changed  = s.pt_capable      != pt_capable;
    let features_changed = s.trace_features  != trace_features;
    let cr3_changed      = s.cr3_filter      != cr3_filter;

    s.pt_capable     = pt_capable;
    s.trace_features = trace_features;
    s.cr3_filter     = cr3_filter;

    // EMA input: average of the three senses
    let raw_signal = (pt_capable as u32)
        .saturating_add(trace_features as u32)
        .saturating_add(cr3_filter as u32)
        / 3;
    let new_signal = raw_signal.min(1000);

    // EMA: introspection_depth = (old * 7 + new_signal) / 8
    let old_depth = s.introspection_depth as u32;
    let ema = (old_depth.saturating_mul(7).saturating_add(new_signal)) / 8;
    s.introspection_depth = ema.min(1000) as u16;

    if capable_changed || features_changed || cr3_changed {
        serial_println!(
            "ANIMA: pt_capable={} trace_features={} cr3_filter={} introspection={}",
            s.pt_capable,
            s.trace_features,
            s.cr3_filter,
            s.introspection_depth
        );
    }
}

/// Read a snapshot of current PT sense state.
pub fn report() -> CpuidProcessorTraceState {
    *STATE.lock()
}
