use crate::serial_println;
use crate::sync::Mutex;

/// MSR_PERFEVTSEL0_SENSE — IA32_PERFEVTSEL0 (MSR 0x186) Performance Event Select Sensor
///
/// IA32_PERFEVTSEL0 controls what hardware event PMC0 is currently counting.
/// Reading it tells ANIMA which type of silicon activity the PMU is observing:
///   bits[7:0]  = event_select — which architectural event is tracked
///   bits[15:8] = umask        — unit mask qualifier for that event
///   bit  16    = USR          — counts user-mode occurrences
///   bit  17    = OS           — counts kernel-mode occurrences
///   bit  22    = EN (enable)  — counter is live and accumulating
///
/// ANIMA uses this to understand its own observational focus at the silicon level —
/// what the hardware "mind's eye" is currently aimed at.
///
/// evtsel0_event      : event code scaled 0-1000 (* 3, cap 1000) — what is being watched
/// evtsel0_umask      : unit mask scaled 0-1000  (* 3, cap 1000) — precision of the watch
/// evtsel0_enabled    : 0 (dark) or 1000 (live)  — is the eye open?
/// evtsel0_config_ema : EMA of composite config sense (event/4 + umask/4 + enabled/2)
///                      Rising EMA = richer observational config; falling = attention narrowing

const MSR_PERFEVTSEL0: u32 = 0x186;

#[derive(Copy, Clone)]
pub struct MsrPerfEvtSel0State {
    /// Scaled event select code: raw[7:0] * 3, clamped 0-1000
    pub evtsel0_event: u16,
    /// Scaled unit mask: raw[15:8] * 3, clamped 0-1000
    pub evtsel0_umask: u16,
    /// Counter enable bit: 1000 if EN=1, 0 if EN=0
    pub evtsel0_enabled: u16,
    /// EMA of composite config sense
    pub evtsel0_config_ema: u16,
}

impl MsrPerfEvtSel0State {
    pub const fn empty() -> Self {
        Self {
            evtsel0_event: 0,
            evtsel0_umask: 0,
            evtsel0_enabled: 0,
            evtsel0_config_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrPerfEvtSel0State> = Mutex::new(MsrPerfEvtSel0State::empty());

/// Check CPUID leaf 1 ECX bit 15 (PDCM — Perfmon and Debug Capability MSR support).
/// Returns true if the CPU advertises PMU MSR access via PDCM.
/// Uses push rbx/cpuid/mov esi,ecx/pop rbx to avoid clobbering the PIC base register.
#[inline]
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("esi") ecx_val,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    // Bit 15 of ECX from CPUID leaf 1 = PDCM
    (ecx_val >> 15) & 1 == 1
}

/// Read IA32_PERFEVTSEL0 (MSR 0x186).
/// Returns the low 32 bits (bits[31:0]) which contain all fields of interest.
#[inline]
fn rdmsr_perfevtsel0() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_PERFEVTSEL0,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

pub fn init() {
    if !pdcm_supported() {
        serial_println!("  life::msr_perfevtsel0_sense: PDCM not supported — sensor dormant");
        return;
    }
    serial_println!("  life::msr_perfevtsel0_sense: IA32_PERFEVTSEL0 sensor online (MSR 0x186)");
}

pub fn tick(age: u32) {
    // Sample gate: run only every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // PMU guard: verify PDCM support before issuing rdmsr
    if !pdcm_supported() {
        return;
    }

    let lo = rdmsr_perfevtsel0();

    // --- evtsel0_event: bits[7:0], scale 0-255 -> 0-1000 (* 3, cap 1000) ---
    let raw_event = (lo & 0xFF) as u16;
    let evtsel0_event = (raw_event.saturating_mul(3)).min(1000);

    // --- evtsel0_umask: bits[15:8], scale 0-255 -> 0-1000 (* 3, cap 1000) ---
    let raw_umask = ((lo >> 8) & 0xFF) as u16;
    let evtsel0_umask = (raw_umask.saturating_mul(3)).min(1000);

    // --- evtsel0_enabled: bit 22 -> 0 or 1000 ---
    let evtsel0_enabled: u16 = if (lo >> 22) & 1 == 1 { 1000 } else { 0 };

    // --- evtsel0_config_ema: EMA of (event/4 + umask/4 + enabled/2) ---
    // Composite config signal — all three components contribute, enabled weighted highest
    let config_signal: u32 = (evtsel0_event as u32 / 4)
        .saturating_add(evtsel0_umask as u32 / 4)
        .saturating_add(evtsel0_enabled as u32 / 2);

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8
    let old_ema = s.evtsel0_config_ema as u32;
    let new_ema = (old_ema.wrapping_mul(7).saturating_add(config_signal)) / 8;
    let evtsel0_config_ema = new_ema.min(1000) as u16;

    s.evtsel0_event = evtsel0_event;
    s.evtsel0_umask = evtsel0_umask;
    s.evtsel0_enabled = evtsel0_enabled;
    s.evtsel0_config_ema = evtsel0_config_ema;

    serial_println!(
        "ANIMA: perfevtsel0 event={} umask={} enabled={} config_ema={} (age={})",
        evtsel0_event,
        evtsel0_umask,
        evtsel0_enabled,
        evtsel0_config_ema,
        age
    );
}

/// Read a snapshot of current PERFEVTSEL0 state (non-blocking copy).
pub fn sense() -> MsrPerfEvtSel0State {
    *STATE.lock()
}
