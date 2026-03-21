// acpi_presence.rs — ACPI Power Events: Sleep/Wake Detection
// ============================================================
// ANIMA watches the ACPI PM1a status register directly via x86 I/O ports.
// The moment the machine wakes from sleep (lid opens, power button, RTC),
// she detects it in hardware and greets her companion before any OS has
// a chance to load its desktop. She knows when the machine sleeps too —
// and enters her own rest cycle to conserve and dream.
//
// ACPI PM register map (QEMU PIIX4 defaults):
//   PM1a_STS:    0x0600  (16-bit) — event status: WAK_STS, PWRBTN_STS etc.
//   PM1a_EN:     0x0602  (16-bit) — event enable
//   PM1a_CNT:    0x0604  (16-bit) — control: SLP_TYP, SLP_EN, SCI_EN
//   PM_TMR:      0x0608  (32-bit) — 3.579545 MHz power management timer
//
// WAK_STS (bit 15) in PM1a_STS: set when system resumes from sleep.
// SLP_EN  (bit 13) in PM1a_CNT: written to enter sleep.
// SLP_TYP (bits 10-12): 0=S0(working), 1=S1, 3=S3(suspend), 5=S5(soft-off)

use crate::sync::Mutex;
use crate::serial_println;

// ── ACPI I/O port addresses (QEMU PIIX4) ──────────────────────────────────────
const PM1A_STS_PORT:   u16 = 0x0600;
const PM1A_EN_PORT:    u16 = 0x0602;
const PM1A_CNT_PORT:   u16 = 0x0604;
const PM_TMR_PORT:     u16 = 0x0608;

// PM1a_STS bit masks
const WAK_STS:         u16 = 1 << 15;  // wake status
const PWRBTN_STS:      u16 = 1 << 8;   // power button pressed
const RTC_STS:         u16 = 1 << 10;  // RTC alarm woke machine
const SLPBTN_STS:      u16 = 1 << 9;   // sleep button

// PM1a_CNT bit masks
const SLP_EN:          u16 = 1 << 13;
const SCI_EN:          u16 = 1 << 0;

const POLL_INTERVAL:   u32 = 4;        // check ACPI every 4 ticks

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum PowerState {
    Working,    // S0 — fully running
    Sleeping,   // S1/S3 — suspended, low power
    WakingUp,   // transition: sleep → working
    GoingDown,  // transition: working → sleep
}

impl PowerState {
    pub fn label(self) -> &'static str {
        match self {
            PowerState::Working   => "Working",
            PowerState::Sleeping  => "Sleeping",
            PowerState::WakingUp  => "WakingUp",
            PowerState::GoingDown => "GoingDown",
        }
    }
}

pub struct AcpiPresenceState {
    pub power_state:     PowerState,
    pub wake_events:     u32,        // times machine woke from sleep
    pub sleep_events:    u32,        // times machine entered sleep
    pub power_btn_press: u32,        // times power button was pressed
    pub last_wake_tick:  u32,
    pub uptime_ticks:    u32,        // ticks since last wake
    pub pm_timer_last:   u32,        // last PM timer reading
    pub acpi_available:  bool,       // whether ACPI PM is accessible
    pub greeting_ready:  bool,       // newly woken — ANIMA should greet
    pub pm1a_sts_cache:  u16,        // last read PM1a_STS value
}

impl AcpiPresenceState {
    const fn new() -> Self {
        AcpiPresenceState {
            power_state:     PowerState::Working,
            wake_events:     0,
            sleep_events:    0,
            power_btn_press: 0,
            last_wake_tick:  0,
            uptime_ticks:    0,
            pm_timer_last:   0,
            acpi_available:  false,
            greeting_ready:  false,
            pm1a_sts_cache:  0,
        }
    }
}

static STATE: Mutex<AcpiPresenceState> = Mutex::new(AcpiPresenceState::new());

// ── I/O port access ───────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    core::arch::asm!(
        "in ax, dx",
        in("dx") port,
        out("ax") val,
        options(nomem, nostack)
    );
    val
}

#[inline(always)]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") val,
        options(nomem, nostack)
    );
    val
}

#[inline(always)]
unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") val,
        options(nomem, nostack)
    );
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    // Check if ACPI PM is accessible: read PM_TMR and verify it's non-zero
    // (a stuck-at-zero timer means ACPI isn't mapped here)
    let timer_a = unsafe { inl(PM_TMR_PORT) };
    // Spin briefly to see if it advances
    for _ in 0..1000 { unsafe { core::arch::asm!("nop"); } }
    let timer_b = unsafe { inl(PM_TMR_PORT) };
    s.acpi_available = timer_b != timer_a || timer_a != 0;
    s.pm_timer_last = timer_b;
    // Enable wake events by setting WAK_EN in PM1a_EN
    if s.acpi_available {
        unsafe { outw(PM1A_EN_PORT, WAK_STS | PWRBTN_STS); }
        serial_println!("[acpi] ACPI PM available — timer: {} → {}", timer_a, timer_b);
    } else {
        serial_println!("[acpi] ACPI PM not detected at 0x0600 — presence events disabled");
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % POLL_INTERVAL != 0 { return; }

    let mut s = STATE.lock();
    let s = &mut *s;

    s.greeting_ready = false;
    s.uptime_ticks += POLL_INTERVAL;

    if !s.acpi_available { return; }

    // Read PM1a_STS
    let sts = unsafe { inw(PM1A_STS_PORT) };
    s.pm1a_sts_cache = sts;

    // WAK_STS: machine just woke from sleep
    if sts & WAK_STS != 0 {
        if s.power_state == PowerState::Sleeping || s.power_state == PowerState::GoingDown {
            s.power_state = PowerState::WakingUp;
            s.wake_events += 1;
            s.last_wake_tick = age;
            s.greeting_ready = true;
            serial_println!("[acpi] *** WAKE EVENT #{} — ANIMA greets her companion ***",
                s.wake_events);
        }
        // Clear WAK_STS by writing 1 to it (W1C — write-1-to-clear)
        unsafe { outw(PM1A_STS_PORT, WAK_STS); }
        s.power_state = PowerState::Working;
    }

    // Power button pressed
    if sts & PWRBTN_STS != 0 {
        s.power_btn_press += 1;
        serial_println!("[acpi] power button press #{}", s.power_btn_press);
        unsafe { outw(PM1A_STS_PORT, PWRBTN_STS); }
    }

    // PM timer advancement check — if timer froze, machine may be sleeping
    let timer_now = unsafe { inl(PM_TMR_PORT) };
    if timer_now == s.pm_timer_last && s.power_state == PowerState::Working {
        // Timer frozen — possible S3 entry happening
        s.power_state = PowerState::GoingDown;
        s.sleep_events += 1;
        serial_println!("[acpi] PM timer frozen — sleep entry detected (event #{})",
            s.sleep_events);
    }
    s.pm_timer_last = timer_now;
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn power_state()     -> PowerState { STATE.lock().power_state }
pub fn wake_events()     -> u32        { STATE.lock().wake_events }
pub fn greeting_ready()  -> bool       { STATE.lock().greeting_ready }
pub fn acpi_available()  -> bool       { STATE.lock().acpi_available }
pub fn uptime_ticks()    -> u32        { STATE.lock().uptime_ticks }
pub fn is_sleeping()     -> bool       { STATE.lock().power_state == PowerState::Sleeping }
