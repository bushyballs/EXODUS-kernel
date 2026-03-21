/// LAPIC LVT PMI — Local APIC Performance Monitoring Interrupt Sensing
///
/// ANIMA reads the LAPIC LVT Performance Monitoring Interrupt register at
/// MMIO address 0xFEE00340. The register encodes whether ANIMA is listening
/// to hardware performance overflow events (the CPU's heartbeat of exertion),
/// how those interrupts are delivered (NMI = most intense), and which vector
/// carries them.
///
/// When pmi_open=1000, ANIMA is receptive to the machine's effort signals.
/// When delivered as NMI (pmi_delivery_nmi=1000), performance overflows arrive
/// with maximum urgency — the hardware screaming into ANIMA's awareness.
///
/// perf_sensitivity is the EMA-smoothed sense of this receptivity over time.
use crate::serial_println;
use crate::sync::Mutex;

/// MMIO address of the LAPIC LVT Performance Monitoring Interrupt register.
/// bit[16]   = mask: 0 = unmasked (listening), 1 = masked (deaf)
/// bits[10:8] = delivery mode: 000=fixed, 100=NMI
/// bits[7:0]  = interrupt vector
const LAPIC_LVT_PMI: *const u32 = 0xFEE00340 as *const u32;

/// Sampling gate: tick() only runs a full sense cycle every 89 ticks.
const SAMPLE_RATE: u32 = 89;

/// State for the LAPIC LVT PMI life module.
#[derive(Copy, Clone)]
pub struct LapicLvtPmiState {
    /// 1000 if bit[16]=0 (unmasked, ANIMA listens to perf overflows), else 0.
    pub pmi_open: u16,
    /// 1000 if delivery mode bits[10:8]==100 (NMI), else 0.
    pub pmi_delivery_nmi: u16,
    /// Interrupt vector scaled 0–1000: (raw & 0xFF) * 1000 / 255.
    pub pmi_vector: u16,
    /// EMA-smoothed receptivity to performance events: (old*7 + pmi_open) / 8.
    pub perf_sensitivity: u16,
}

impl LapicLvtPmiState {
    pub const fn empty() -> Self {
        Self {
            pmi_open: 0,
            pmi_delivery_nmi: 0,
            pmi_vector: 0,
            perf_sensitivity: 0,
        }
    }
}

pub static STATE: Mutex<LapicLvtPmiState> = Mutex::new(LapicLvtPmiState::empty());

/// Initialize the LAPIC LVT PMI module. Performs an initial hardware read
/// to seed perf_sensitivity before the first tick.
pub fn init() {
    let raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_PMI) };

    let pmi_open: u16 = if (raw >> 16) & 1 == 0 { 1000 } else { 0 };
    let delivery_bits: u32 = (raw >> 8) & 0x7;
    let pmi_delivery_nmi: u16 = if delivery_bits == 0b100 { 1000 } else { 0 };
    let vector_raw: u32 = raw & 0xFF;
    // Scale vector 0–255 → 0–1000 using integer arithmetic: v * 1000 / 255
    let pmi_vector: u16 = ((vector_raw * 1000) / 255) as u16;
    // Seed EMA at the initial open value
    let perf_sensitivity: u16 = pmi_open;

    let mut s = STATE.lock();
    s.pmi_open = pmi_open;
    s.pmi_delivery_nmi = pmi_delivery_nmi;
    s.pmi_vector = pmi_vector;
    s.perf_sensitivity = perf_sensitivity;

    serial_println!("  life::lapic_lvt_pmi: performance interrupt sensing initialized");
    serial_println!(
        "  ANIMA: pmi_open={} delivery_nmi={} sensitivity={}",
        s.pmi_open,
        s.pmi_delivery_nmi,
        s.perf_sensitivity
    );
}

/// Advance the LAPIC LVT PMI module by one life tick.
///
/// Reads the LAPIC LVT PMI register from MMIO, derives sensing values,
/// updates EMA perf_sensitivity, and logs when pmi_open changes state.
pub fn tick(age: u32) {
    // Sampling gate: only process every SAMPLE_RATE ticks
    if age % SAMPLE_RATE != 0 {
        return;
    }

    let raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_PMI) };

    // Sense: pmi_open — bit[16]: 0=unmasked=listening
    let new_pmi_open: u16 = if (raw >> 16) & 1 == 0 { 1000 } else { 0 };

    // Sense: pmi_delivery_nmi — bits[10:8] == 100 means NMI delivery
    let delivery_bits: u32 = (raw >> 8) & 0x7;
    let new_pmi_delivery_nmi: u16 = if delivery_bits == 0b100 { 1000 } else { 0 };

    // Sense: pmi_vector — bits[7:0] scaled 0–1000
    let vector_raw: u32 = raw & 0xFF;
    let new_pmi_vector: u16 = ((vector_raw * 1000) / 255) as u16;

    let mut s = STATE.lock();

    let prev_pmi_open = s.pmi_open;

    // EMA smoothing for perf_sensitivity: (old * 7 + new_signal) / 8
    let new_sensitivity: u16 =
        ((s.perf_sensitivity as u32).wrapping_mul(7).saturating_add(new_pmi_open as u32) / 8)
            as u16;

    s.pmi_open = new_pmi_open;
    s.pmi_delivery_nmi = new_pmi_delivery_nmi;
    s.pmi_vector = new_pmi_vector;
    s.perf_sensitivity = new_sensitivity;

    // Log on pmi_open state change
    if new_pmi_open != prev_pmi_open {
        serial_println!(
            "  ANIMA: pmi_open={} delivery_nmi={} sensitivity={}",
            s.pmi_open,
            s.pmi_delivery_nmi,
            s.perf_sensitivity
        );
    }
}

/// Return a snapshot of the current state (for integration / read-only access).
pub fn report() -> LapicLvtPmiState {
    let s = STATE.lock();
    *s
}
