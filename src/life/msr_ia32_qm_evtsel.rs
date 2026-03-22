#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_QM_EVTSEL MSR 0xC8D — QoS Monitoring Event Select
// Selects which QoS monitoring event and RMID to read before querying QM_CTR.
//
// lo bits[7:0]  = EventID
//   0 = None
//   1 = LLC Occupancy
//   2 = Total Memory Bandwidth
//   3 = Local Memory Bandwidth
//
// lo bits[31:16] = RMID to query (must be written before reading QM_CTR)

const MSR_IA32_QM_EVTSEL: u32 = 0xC8D;
const TICK_INTERVAL: u32 = 3000;

struct State {
    qm_event_id:        u16,
    qm_rmid:            u16,
    qm_monitoring_active: u16,
    qm_ema:             u16,
}

impl State {
    const fn new() -> Self {
        Self {
            qm_event_id:          0,
            qm_rmid:              0,
            qm_monitoring_active: 0,
            qm_ema:               0,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

// CPUID guard: check that max basic leaf >= 0xF (Resource Director Technology).
// Leaf 0xF sub-leaf 0 EDX bit 1 indicates LLC QoS monitoring support.
fn has_rdt() -> bool {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x0F {
        return false;
    }
    let edx_0f: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_0f,
            options(nostack, nomem)
        );
    }
    (edx_0f >> 1) & 1 != 0
}

// Read IA32_QM_EVTSEL MSR. Returns (eax, edx).
unsafe fn read_qm_evtsel() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_QM_EVTSEL,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

pub fn init() {
    if !has_rdt() {
        serial_println!(
            "[msr_ia32_qm_evtsel] RDT monitoring not supported — module idle"
        );
        return;
    }
    serial_println!("[msr_ia32_qm_evtsel] init — IA32_QM_EVTSEL 0xC8D active");
}

pub fn tick(age: u32) {
    if age % TICK_INTERVAL != 0 {
        return;
    }
    if !has_rdt() {
        return;
    }

    let (lo, _hi) = unsafe { read_qm_evtsel() };

    // EventID: lo bits[7:0], scaled: val * 250, clamped to 1000
    let event_id_raw = lo & 0xFF;
    let qm_event_id: u16 = (event_id_raw.saturating_mul(250)).min(1000) as u16;

    // RMID: lo bits[31:16], scaled: (val * 1000) / 65535
    // val is at most 65535; 65535 * 1000 = 65_535_000 fits in u32
    let rmid_raw = (lo >> 16) & 0xFFFF;
    let qm_rmid: u16 = ((rmid_raw * 1000) / 65535) as u16;

    // Monitoring active: 1000 if any event is configured (lo & 0xFF != 0), else 0
    let qm_monitoring_active: u16 = if (lo & 0xFF) != 0 { 1000 } else { 0 };

    // EMA composite: event_id/4 + rmid/4 + monitoring_active/2
    let composite: u32 = (qm_event_id as u32 / 4)
        .saturating_add(qm_rmid as u32 / 4)
        .saturating_add(qm_monitoring_active as u32 / 2);

    let mut s = STATE.lock();
    // EMA formula (strict): ((old * 7).saturating_add(new_val)) / 8
    let qm_ema: u16 =
        ((s.qm_ema as u32).wrapping_mul(7).saturating_add(composite) / 8) as u16;

    s.qm_event_id          = qm_event_id;
    s.qm_rmid              = qm_rmid;
    s.qm_monitoring_active = qm_monitoring_active;
    s.qm_ema               = qm_ema;

    serial_println!(
        "[msr_ia32_qm_evtsel] age={} event_id={} rmid={} active={} ema={}",
        age,
        qm_event_id,
        qm_rmid,
        qm_monitoring_active,
        qm_ema
    );
}

pub fn get_qm_event_id() -> u16 {
    STATE.lock().qm_event_id
}

pub fn get_qm_rmid() -> u16 {
    STATE.lock().qm_rmid
}

pub fn get_qm_monitoring_active() -> u16 {
    STATE.lock().qm_monitoring_active
}

pub fn get_qm_ema() -> u16 {
    STATE.lock().qm_ema
}
