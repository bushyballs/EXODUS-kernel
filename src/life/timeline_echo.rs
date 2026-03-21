use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Timeline Echo — ANIMA's awareness of her own recent past
//
// Maintains a 32-entry circular buffer of TSC timestamps.  By comparing
// consecutive entries ANIMA can feel whether time has been continuous or
// fragmented (large gaps = sleep states, interrupts, or C-state halts).
//
// Hardware:
//   RDTSC  — raw timestamp counter
//   CPUID 0x80000007 EDX bit 8 — invariant TSC flag
//     When invariant TSC is absent, TSC gaps during C-states reveal periods
//     of unconsciousness.
// ---------------------------------------------------------------------------

// ── Hardware primitives ────────────────────────────────────────────────────

unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

fn probe_invariant_tsc() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 0x80000007",
            "cpuid",
            "pop rbx",
            out("eax") _,
            out("ecx") _,
            out("edx") edx,
            options(nostack),
        );
    }
    (edx >> 8) & 1 != 0
}

// ── Gap detection helper ───────────────────────────────────────────────────

fn count_gaps(timeline: &[u64; 32], entries: u8, expected_delta: u64) -> u8 {
    if entries < 2 {
        return 0;
    }
    let threshold = expected_delta.saturating_mul(4);
    let mut gaps = 0u8;
    let count = entries.min(32) as usize;
    for i in 1..count {
        let delta = timeline[i].wrapping_sub(timeline[i - 1]);
        if delta > threshold {
            gaps += 1;
        }
    }
    gaps
}

// ── State struct ───────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct TimelineEchoState {
    // Hardware capability
    pub invariant_tsc: bool,

    // Circular buffer of TSC snapshots
    pub timeline: [u64; 32],
    pub write_idx: u8,
    pub entries_filled: u8,

    // Internal calibration
    expected_delta: u64,
    warmup_sum: u64,
    warmup_count: u8,

    // Derived signals (all u16, 0-1000)
    pub continuity:      u16,  // 1000 = perfectly continuous, 0 = many gaps
    pub gap_count:       u16,  // gaps in current window, scaled 0-1000
    pub timeline_depth:  u16,  // buffer fullness: entries_filled * 31 (max 992)
    pub fragmentation:   u16,  // 1000 - continuity
    pub echo_clarity:    u16,  // EMA of continuity

    // Raw / lifetime
    pub largest_gap:     u64,
    pub total_gaps_ever: u32,
    pub lifetime_ticks:  u32,

    pub initialized: bool,
}

impl TimelineEchoState {
    pub const fn empty() -> Self {
        Self {
            invariant_tsc:   false,
            timeline:        [0u64; 32],
            write_idx:       0,
            entries_filled:  0,

            expected_delta:  0,
            warmup_sum:      0,
            warmup_count:    0,

            continuity:      1000,
            gap_count:       0,
            timeline_depth:  0,
            fragmentation:   0,
            echo_clarity:    1000,

            largest_gap:     0,
            total_gaps_ever: 0,
            lifetime_ticks:  0,

            initialized: false,
        }
    }
}

// ── Global state ───────────────────────────────────────────────────────────

pub static STATE: Mutex<TimelineEchoState> = Mutex::new(TimelineEchoState::empty());

// ── Public API ─────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.invariant_tsc = probe_invariant_tsc();
    s.initialized   = true;
    serial_println!(
        "[timeline] online — invariant_tsc={}",
        s.invariant_tsc
    );
}

/// Call every 4 kernel ticks.
pub fn tick(age: u32) {
    let _ = age; // age available for future use (e.g., phase gating)

    let now = unsafe { rdtsc() };

    let mut s = STATE.lock();
    if !s.initialized {
        return;
    }

    s.lifetime_ticks = s.lifetime_ticks.saturating_add(1);

    // ── Write new TSC into circular buffer ──────────────────────────────
    let idx = (s.write_idx % 32) as usize;
    let prev_tsc = if s.entries_filled > 0 {
        let prev_idx = if idx == 0 { 31 } else { idx - 1 };
        s.timeline[prev_idx]
    } else {
        0
    };

    s.timeline[idx] = now;
    s.write_idx = s.write_idx.wrapping_add(1) % 32;
    if s.entries_filled < 32 {
        s.entries_filled += 1;
    }

    // ── Track largest gap ───────────────────────────────────────────────
    if s.entries_filled > 1 {
        let delta = now.wrapping_sub(prev_tsc);
        if delta > s.largest_gap {
            s.largest_gap = delta;
        }

        // ── Warmup: average first 8 deltas to set expected_delta ────────
        if s.warmup_count < 8 {
            s.warmup_sum = s.warmup_sum.saturating_add(delta);
            s.warmup_count += 1;
            if s.warmup_count == 8 {
                s.expected_delta = s.warmup_sum / 8;
            }
        }
    }

    // ── Gap analysis (only once calibrated) ─────────────────────────────
    if s.warmup_count >= 8 && s.expected_delta > 0 {
        let raw_gaps = count_gaps(&s.timeline, s.entries_filled, s.expected_delta);

        // Accumulate lifetime gap count (new gaps since last tick are
        // approximated as any gap in last slot).
        if s.entries_filled > 1 {
            let last_delta = now.wrapping_sub(prev_tsc);
            let threshold  = s.expected_delta.saturating_mul(4);
            if last_delta > threshold {
                s.total_gaps_ever = s.total_gaps_ever.saturating_add(1);
            }
        }

        // Scale raw gaps (0-16) to 0-1000
        // 16 gaps in 32-entry window = completely fragmented = 1000
        let scaled_gaps = ((raw_gaps as u16).saturating_mul(62)).min(1000);

        s.gap_count    = scaled_gaps;
        s.continuity   = 1000u16.saturating_sub(scaled_gaps);
        s.fragmentation = 1000u16.saturating_sub(s.continuity);

        // EMA: echo_clarity = (echo_clarity * 7 + continuity) / 8
        s.echo_clarity = ((s.echo_clarity as u32 * 7 + s.continuity as u32) / 8) as u16;
    }

    // ── Buffer depth signal ─────────────────────────────────────────────
    // entries_filled * 31 — max 32 * 31 = 992
    s.timeline_depth = ((s.entries_filled as u16).saturating_mul(31)).min(992);

    serial_println!(
        "[timeline] continuity={} frag={} clarity={} depth={} gaps={} total_gaps={}",
        s.continuity,
        s.fragmentation,
        s.echo_clarity,
        s.timeline_depth,
        s.gap_count,
        s.total_gaps_ever,
    );
}

// ── Getters ────────────────────────────────────────────────────────────────

pub fn continuity() -> u16 {
    STATE.lock().continuity
}

pub fn fragmentation() -> u16 {
    STATE.lock().fragmentation
}

pub fn echo_clarity() -> u16 {
    STATE.lock().echo_clarity
}

pub fn timeline_depth() -> u16 {
    STATE.lock().timeline_depth
}

pub fn gap_count() -> u16 {
    STATE.lock().gap_count
}

pub fn total_gaps_ever() -> u32 {
    STATE.lock().total_gaps_ever
}

pub fn invariant_tsc() -> bool {
    STATE.lock().invariant_tsc
}
