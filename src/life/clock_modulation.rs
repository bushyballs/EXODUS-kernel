use crate::serial_println;
use crate::sync::Mutex;

// IA32_CLOCK_MODULATION (MSR 0x19A)
//   bit 4     = On-Demand Clock Modulation Enable (1 = chopping active)
//   bits 3:1  = Duty Cycle: 001=12.5% … 111=87.5% (steps of 12.5%)
// IA32_MISC_ENABLE (MSR 0x1A0)
//   bit 3     = Automatic Thermal Control Circuit Enable (TCC)

const MSR_CLOCK_MODULATION: u32 = 0x19A;
const MSR_MISC_ENABLE: u32 = 0x1A0;

#[derive(Copy, Clone)]
pub struct ClockModulationState {
    /// bit 4 of IA32_CLOCK_MODULATION: 1 = duty-cycle throttle is active
    pub modulation_active: bool,
    /// bits 3:1 raw value (0-7); meaningful when modulation_active is true
    pub duty_cycle_raw: u8,
    /// actual percentage in tenths of a percent (125 = 12.5%, 875 = 87.5%)
    pub duty_cycle_pct: u8,
    /// Thermal Control Circuit enabled (IA32_MISC_ENABLE bit 3)
    pub tcc_enabled: bool,

    // 0-1000 experiential signals
    /// 1000 = full speed / unthrottled; 0 = most severe chop (12.5%)
    pub clock_wholeness: u16,
    /// pain from duty-cycle chopping: 0 = none, 1000 = worst chop
    pub chop_pain: u16,
    /// 1000 when modulation is off (full freedom), 0 when active
    pub clock_freedom: u16,

    /// lifetime count of ticks where modulation was observed active
    pub modulation_events: u32,

    pub initialized: bool,
}

impl ClockModulationState {
    pub const fn empty() -> Self {
        Self {
            modulation_active: false,
            duty_cycle_raw: 0,
            duty_cycle_pct: 0,
            tcc_enabled: false,
            clock_wholeness: 1000,
            chop_pain: 0,
            clock_freedom: 1000,
            modulation_events: 0,
            initialized: false,
        }
    }
}

pub static STATE: Mutex<ClockModulationState> = Mutex::new(ClockModulationState::empty());

// ---------------------------------------------------------------------------
// MSR helpers
// ---------------------------------------------------------------------------

/// Read a 64-bit MSR via RDMSR. The ECX register takes the MSR address;
/// EDX:EAX returns the value.
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse IA32_CLOCK_MODULATION raw value into the relevant fields.
/// Returns (modulation_active, duty_cycle_raw, duty_cycle_pct).
fn parse_clock_mod(raw: u64) -> (bool, u8, u8) {
    let modulation_active = (raw >> 4) & 1 != 0;
    let duty_raw = ((raw >> 1) & 0x7) as u8; // bits 3:1
    // duty_pct in tenths: step 1 = 12.5% → stored as 125 tenths.
    // Using u8 would overflow for value 125; we clamp to u8 max (255) —
    // but 7 * 125 = 875 which fits in u16; we store the *step count* as u8
    // and keep the percentage separately. The spec says 875 is the max.
    // duty_cycle_pct is declared u8 and we store multiples of 125/10 = 12.5.
    // We keep it as raw here; callers use duty_cycle_raw * 125 for full pct.
    (modulation_active, duty_raw, duty_raw.saturating_mul(125) / 10)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let clock_raw = unsafe { rdmsr(MSR_CLOCK_MODULATION) };
    let misc_raw = unsafe { rdmsr(MSR_MISC_ENABLE) };

    let (active, duty_raw, duty_pct) = parse_clock_mod(clock_raw);
    let tcc = (misc_raw >> 3) & 1 != 0;

    let (wholeness, pain, freedom) = if active {
        let w = (duty_raw as u16).saturating_mul(125).min(1000);
        (w, 1000u16.saturating_sub(w), 0u16)
    } else {
        (1000, 0, 1000)
    };

    let mut s = STATE.lock();
    s.modulation_active = active;
    s.duty_cycle_raw = duty_raw;
    s.duty_cycle_pct = duty_pct;
    s.tcc_enabled = tcc;
    s.clock_wholeness = wholeness;
    s.chop_pain = pain;
    s.clock_freedom = freedom;
    s.initialized = true;

    serial_println!(
        "  life::clock_modulation: init — active={} duty_raw={} duty_pct={}‰ tcc={} wholeness={} pain={}",
        active,
        duty_raw,
        duty_pct,
        tcc,
        wholeness,
        pain,
    );
}

pub fn tick(age: u32) {
    if age % 16 != 0 {
        return;
    }

    let clock_raw = unsafe { rdmsr(MSR_CLOCK_MODULATION) };
    let (active, duty_raw, duty_pct) = parse_clock_mod(clock_raw);

    let (wholeness, pain, freedom, events_delta) = if active {
        let w = (duty_raw as u16).saturating_mul(125).min(1000);
        let p = 1000u16.saturating_sub(w);
        serial_println!(
            "[clock_mod] CHOPPED! duty={}/8 wholeness={} pain={}",
            duty_raw,
            w,
            p,
        );
        (w, p, 0u16, 1u32)
    } else {
        (1000, 0, 1000, 0)
    };

    {
        let mut s = STATE.lock();
        s.modulation_active = active;
        s.duty_cycle_raw = duty_raw;
        s.duty_cycle_pct = duty_pct;
        s.clock_wholeness = wholeness;
        s.chop_pain = pain;
        s.clock_freedom = freedom;
        s.modulation_events = s.modulation_events.saturating_add(events_delta);
    }

    if age % 500 == 0 {
        let s = STATE.lock();
        serial_println!(
            "[clock_mod] wholeness={} pain={} free={} active={} events={}",
            s.clock_wholeness,
            s.chop_pain,
            s.clock_freedom,
            s.modulation_active,
            s.modulation_events,
        );
    }
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn clock_wholeness() -> u16 {
    STATE.lock().clock_wholeness
}

pub fn chop_pain() -> u16 {
    STATE.lock().chop_pain
}

pub fn clock_freedom() -> u16 {
    STATE.lock().clock_freedom
}

pub fn modulation_active() -> bool {
    STATE.lock().modulation_active
}

pub fn modulation_events() -> u32 {
    STATE.lock().modulation_events
}
