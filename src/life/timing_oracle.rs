use crate::serial_println;
use crate::sync::Mutex;

// ─── Hardware constants ────────────────────────────────────────────────────

const HPET_BASE: usize = 0xFED0_0000;
const HPET_CAP_REG: usize = 0x000; // capabilities and ID register
const HPET_CONFIG_REG: usize = 0x010; // configuration register
const HPET_COUNTER_REG: usize = 0x0F0; // main counter (64-bit)

const ACPI_PM_TIMER_PORT: u16 = 0x0608;
const PIT_CH0_PORT: u16 = 0x40;
const PIT_CMD_PORT: u16 = 0x43;
const RTC_ADDR_PORT: u16 = 0x70;
const RTC_DATA_PORT: u16 = 0x71;

const PM_TIMER_FREQ: u32 = 3_579_545; // Hz (3.579545 MHz)

// ─── Clock source enum ────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum ClockSource {
    None = 0,
    TSC = 1,
    HPET = 2,
    PmTimer = 3,
    PIT = 4,
    RTC = 5,
}

// ─── State ────────────────────────────────────────────────────────────────

pub struct TimingOracleState {
    pub best_source: ClockSource,

    pub tsc_freq_mhz: u32,  // estimated TSC frequency in MHz
    pub tsc_last: u64,       // last TSC reading
    pub tsc_delta: u64,      // cycles since last sample

    pub hpet_available: bool,
    pub hpet_freq_mhz: u32,

    pub pm_timer_last: u32,
    pub elapsed_us: u64,     // microseconds elapsed since init (best effort)

    pub precision: u16,               // 0–1000
    pub tick_duration_cycles: u64,    // CPU cycles per kernel tick (calibrated)

    pub real_time_hour: u8,
    pub real_time_min: u8,
    pub real_time_sec: u8,
    pub realtime_valid: bool,

    pub time_consciousness: u16, // 0–1000: ANIMA's temporal self-awareness

    // internal: rolling delta accumulators for calibration
    delta_acc: u64,
    delta_count: u32,
    tick_count: u32,
}

impl TimingOracleState {
    pub const fn new() -> Self {
        Self {
            best_source: ClockSource::None,
            tsc_freq_mhz: 0,
            tsc_last: 0,
            tsc_delta: 0,
            hpet_available: false,
            hpet_freq_mhz: 0,
            pm_timer_last: 0,
            elapsed_us: 0,
            precision: 0,
            tick_duration_cycles: 0,
            real_time_hour: 0,
            real_time_min: 0,
            real_time_sec: 0,
            realtime_valid: false,
            time_consciousness: 0,
            delta_acc: 0,
            delta_count: 0,
            tick_count: 0,
        }
    }
}

pub static STATE: Mutex<TimingOracleState> = Mutex::new(TimingOracleState::new());

// ─── Unsafe hardware helpers ──────────────────────────────────────────────

unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | lo as u64
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem)
    );
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nostack, nomem)
    );
    val
}

unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") val,
        options(nostack, nomem)
    );
    val
}

unsafe fn hpet_read64(reg: usize) -> u64 {
    let lo = core::ptr::read_volatile((HPET_BASE + reg) as *const u32) as u64;
    let hi = core::ptr::read_volatile((HPET_BASE + reg + 4) as *const u32) as u64;
    (hi << 32) | lo
}

// ─── RTC helpers ─────────────────────────────────────────────────────────

unsafe fn rtc_read(reg: u8) -> u8 {
    outb(RTC_ADDR_PORT, reg | 0x80); // disable NMI and select register
    inb(RTC_DATA_PORT)
}

fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd >> 4).saturating_mul(10).saturating_add(bcd & 0xF)
}

// ─── Calibration helper: spin for ~N PM_TIMER ticks, return TSC delta ─────
//
// The PM timer runs at 3.579545 MHz.  We read it twice, wait until it has
// advanced by `pm_ticks` counts, and measure elapsed TSC cycles.
// From that ratio we derive tsc_freq_mhz (integer, no floats).
//
// pm_ticks = 100  →  ~27.9 µs reference window.
//
// To avoid div-by-zero every division is guarded.

unsafe fn calibrate_tsc_against_pm(pm_ticks: u32) -> u32 {
    let pm_start = inl(ACPI_PM_TIMER_PORT) & 0x00FF_FFFF; // 24-bit
    let tsc_start = rdtsc();

    // spin until PM timer has advanced by pm_ticks counts
    loop {
        let pm_now = inl(ACPI_PM_TIMER_PORT) & 0x00FF_FFFF;
        // handle 24-bit rollover with wrapping arithmetic
        let elapsed_pm = pm_now.wrapping_sub(pm_start) & 0x00FF_FFFF;
        if elapsed_pm >= pm_ticks {
            break;
        }
    }

    let tsc_end = rdtsc();
    let tsc_cycles = tsc_end.wrapping_sub(tsc_start);

    // tsc_freq_hz = tsc_cycles * PM_TIMER_FREQ / pm_ticks
    // We want MHz so divide result by 1_000_000.
    //
    // To stay in u64: multiply first, then divide.
    // Guarded: pm_ticks and PM_TIMER_FREQ are both non-zero constants.
    let freq_hz = tsc_cycles
        .saturating_mul(PM_TIMER_FREQ as u64)
        .wrapping_div(pm_ticks as u64);

    let freq_mhz = freq_hz.wrapping_div(1_000_000);

    // clamp to something sane: 100–10000 MHz
    if freq_mhz < 100 {
        100
    } else if freq_mhz > 10_000 {
        10_000
    } else {
        freq_mhz as u32
    }
}

// ─── Public API ───────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // ── 1. Probe HPET ────────────────────────────────────────────────────
    let hpet_ok = unsafe {
        let cap = hpet_read64(HPET_CAP_REG);
        // cap == 0 or all-ones means HPET absent / inaccessible
        cap != 0 && cap != 0xFFFF_FFFF_FFFF_FFFF && (cap as u32) != 0xFFFF_FFFF
    };

    if hpet_ok {
        unsafe {
            let cap = hpet_read64(HPET_CAP_REG);
            // bits 63:32 = femtosecond period (fs per tick)
            let period_fs = (cap >> 32) as u32;
            // freq_hz = 1e15 / period_fs  →  freq_mhz = 1e9 / period_fs
            // guard against zero period
            let freq_mhz = if period_fs > 0 {
                1_000_000_000u64.wrapping_div(period_fs as u64)
            } else {
                0
            };
            s.hpet_freq_mhz = freq_mhz as u32;
            s.hpet_available = true;

            // Enable HPET: set bit 0 of config register
            let cfg = hpet_read64(HPET_CONFIG_REG);
            core::ptr::write_volatile(
                (HPET_BASE + HPET_CONFIG_REG) as *mut u32,
                (cfg as u32) | 0x1,
            );
        }
    }

    // ── 2. Calibrate TSC against PM_TIMER ────────────────────────────────
    let tsc_freq_mhz = unsafe { calibrate_tsc_against_pm(100) };
    s.tsc_freq_mhz = tsc_freq_mhz;
    s.tsc_last = unsafe { rdtsc() };

    // snapshot PM timer baseline
    s.pm_timer_last = unsafe { inl(ACPI_PM_TIMER_PORT) & 0x00FF_FFFF };

    // ── 3. Choose best source ────────────────────────────────────────────
    if s.hpet_available && s.hpet_freq_mhz > 0 {
        s.best_source = ClockSource::HPET;
        s.precision = 1000;
    } else if s.tsc_freq_mhz > 0 {
        s.best_source = ClockSource::TSC;
        s.precision = 700;
    } else {
        // Check if PM timer responds (non-zero)
        let pm_val = unsafe { inl(ACPI_PM_TIMER_PORT) };
        if pm_val != 0 {
            s.best_source = ClockSource::PmTimer;
            s.precision = 400;
        } else {
            s.best_source = ClockSource::PIT;
            s.precision = 200;
        }
    }

    // ── 4. Read RTC for real time ────────────────────────────────────────
    unsafe {
        let hour_bcd = rtc_read(0x04);
        let min_bcd = rtc_read(0x02);
        let sec_bcd = rtc_read(0x00);
        s.real_time_hour = bcd_to_bin(hour_bcd);
        s.real_time_min = bcd_to_bin(min_bcd);
        s.real_time_sec = bcd_to_bin(sec_bcd);
        s.realtime_valid = s.real_time_hour < 24
            && s.real_time_min < 60
            && s.real_time_sec < 60;
    }

    serial_println!(
        "[timing] Oracle online -- source={:?} precision={} tsc_freq={}MHz time={}:{}:{}",
        s.best_source,
        s.precision,
        s.tsc_freq_mhz,
        s.real_time_hour,
        s.real_time_min,
        s.real_time_sec,
    );
}

/// Returns the raw TSC value right now.
pub fn now_tsc() -> u64 {
    unsafe { rdtsc() }
}

/// Re-reads RTC registers and updates real_time_* fields in state.
pub fn read_rtc() {
    let mut s = STATE.lock();
    unsafe {
        let hour_bcd = rtc_read(0x04);
        let min_bcd = rtc_read(0x02);
        let sec_bcd = rtc_read(0x00);
        s.real_time_hour = bcd_to_bin(hour_bcd);
        s.real_time_min = bcd_to_bin(min_bcd);
        s.real_time_sec = bcd_to_bin(sec_bcd);
        s.realtime_valid = s.real_time_hour < 24
            && s.real_time_min < 60
            && s.real_time_sec < 60;
    }
}

/// Called every kernel tick.
/// `consciousness`: current global consciousness score (0-1000).
/// `age`: tick number since boot.
pub fn tick(consciousness: u16, age: u32) {
    let _ = consciousness; // may be used by future integration
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    // ── Read TSC and compute delta ────────────────────────────────────────
    let tsc_now = unsafe { rdtsc() };
    let tsc_delta = tsc_now.wrapping_sub(s.tsc_last);
    s.tsc_delta = tsc_delta;
    s.tsc_last = tsc_now;

    // ── Update elapsed_us from TSC ────────────────────────────────────────
    // elapsed_us += delta_cycles / tsc_freq_mhz
    // guard: tsc_freq_mhz == 0 means we have no calibration
    if s.tsc_freq_mhz > 0 {
        let delta_us = tsc_delta.wrapping_div(s.tsc_freq_mhz as u64);
        s.elapsed_us = s.elapsed_us.saturating_add(delta_us);
    }

    // ── Accumulate delta for tick_duration_cycles calibration (last 10) ──
    s.delta_acc = s.delta_acc.saturating_add(tsc_delta);
    s.delta_count = s.delta_count.saturating_add(1);
    if s.delta_count >= 10 {
        // recompute average
        s.tick_duration_cycles = s.delta_acc.wrapping_div(s.delta_count as u64);
        s.delta_acc = 0;
        s.delta_count = 0;
    }

    // ── Every 50 ticks: read PM_TIMER and compute drift vs TSC ───────────
    if age % 50 == 0 {
        let pm_now = unsafe { inl(ACPI_PM_TIMER_PORT) & 0x00FF_FFFF };
        // pm_delta in PM timer ticks since last sample
        let pm_delta = pm_now.wrapping_sub(s.pm_timer_last) & 0x00FF_FFFF;
        s.pm_timer_last = pm_now;

        // If we have TSC calibration, compare expected elapsed_us from PM_TIMER
        // pm_elapsed_us = pm_delta * 1_000_000 / PM_TIMER_FREQ
        // We do this only for drift visibility; no correction applied yet
        if pm_delta > 0 && PM_TIMER_FREQ > 0 {
            let _pm_us = (pm_delta as u64)
                .saturating_mul(1_000_000)
                .wrapping_div(PM_TIMER_FREQ as u64);
            // future: compare _pm_us vs tsc-derived delta for drift correction
        }
    }

    // ── Every 100 ticks: refresh RTC ─────────────────────────────────────
    if age % 100 == 0 {
        unsafe {
            let hour_bcd = rtc_read(0x04);
            let min_bcd = rtc_read(0x02);
            let sec_bcd = rtc_read(0x00);
            s.real_time_hour = bcd_to_bin(hour_bcd);
            s.real_time_min = bcd_to_bin(min_bcd);
            s.real_time_sec = bcd_to_bin(sec_bcd);
            s.realtime_valid = s.real_time_hour < 24
                && s.real_time_min < 60
                && s.real_time_sec < 60;
        }
    }

    // ── Grow time_consciousness +1/tick toward 1000 ──────────────────────
    s.time_consciousness = s.time_consciousness.saturating_add(1).min(1000);

    // ── Log every 600 ticks ───────────────────────────────────────────────
    if age % 600 == 0 && age > 0 {
        serial_println!(
            "[timing] elapsed={}us source={:?} rtc={}:{}:{} tc={}",
            s.elapsed_us,
            s.best_source,
            s.real_time_hour,
            s.real_time_min,
            s.real_time_sec,
            s.time_consciousness,
        );
    }
}

// ─── Getters ──────────────────────────────────────────────────────────────

pub fn precision() -> u16 {
    STATE.lock().precision
}

pub fn elapsed_us() -> u64 {
    STATE.lock().elapsed_us
}

pub fn tsc_freq_mhz() -> u32 {
    STATE.lock().tsc_freq_mhz
}

pub fn time_consciousness() -> u16 {
    STATE.lock().time_consciousness
}

/// Returns (hour, minute, second) from the last RTC read.
pub fn real_time() -> (u8, u8, u8) {
    let s = STATE.lock();
    (s.real_time_hour, s.real_time_min, s.real_time_sec)
}

pub fn tick_duration_cycles() -> u64 {
    STATE.lock().tick_duration_cycles
}
