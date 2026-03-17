use crate::io::{hlt, inb, outb, outw};
use crate::sync::Mutex;
/// Power state management — shutdown, reboot, sleep
///
/// Power states (ACPI S-states):
///   S0: Working (normal operation)
///   S1: Power on suspend (CPU stops, RAM powered)
///   S3: Suspend to RAM (low power, fast resume)
///   S4: Suspend to disk (hibernate)
///   S5: Soft off (shutdown)
use crate::{serial_print, serial_println};
use core::sync::atomic::{AtomicU32, Ordering};

static POWER_STATE: Mutex<PowerState> = Mutex::new(PowerState::Running);

/// Wake lock counter — suspend is blocked while this is > 0.
static WAKE_LOCKS: AtomicU32 = AtomicU32::new(0);

/// Notifier slot count — keep small to avoid heap in the state machine.
const MAX_NOTIFIERS: usize = 16;

/// A suspend/resume notifier entry.
struct Notifier {
    active: bool,
    /// Called before the system suspends.  Return false to abort.
    on_suspend: fn() -> bool,
    /// Called after the system resumes.
    on_resume: fn(),
}

/// Default no-op suspend callback — always permits suspend.
fn default_on_suspend() -> bool {
    true
}
/// Default no-op resume callback.
fn default_on_resume() {}

impl Notifier {
    const fn empty() -> Self {
        Notifier {
            active: false,
            on_suspend: default_on_suspend,
            on_resume: default_on_resume,
        }
    }
}

struct NotifierChain {
    entries: [Notifier; MAX_NOTIFIERS],
    count: usize,
}

impl NotifierChain {
    const fn new() -> Self {
        // const-compatible initialisation — cannot use array_init or Default here
        NotifierChain {
            entries: [
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
                Notifier::empty(),
            ],
            count: 0,
        }
    }

    /// Register a notifier pair.  Returns a handle (index) or None if full.
    fn register(&mut self, on_suspend: fn() -> bool, on_resume: fn()) -> Option<usize> {
        for (i, slot) in self.entries.iter_mut().enumerate() {
            if !slot.active {
                slot.active = true;
                slot.on_suspend = on_suspend;
                slot.on_resume = on_resume;
                if i >= self.count {
                    self.count = i + 1;
                }
                return Some(i);
            }
        }
        None
    }

    /// Unregister by handle.
    fn unregister(&mut self, handle: usize) {
        if handle < MAX_NOTIFIERS {
            self.entries[handle] = Notifier::empty();
        }
    }

    /// Fire pre-suspend callbacks in registration order.
    /// Returns false if any callback vetoed the suspend.
    fn notify_suspend(&self) -> bool {
        for entry in self.entries.iter().filter(|e| e.active) {
            if !(entry.on_suspend)() {
                return false;
            }
        }
        true
    }

    /// Fire post-resume callbacks in reverse registration order.
    fn notify_resume(&self) {
        for entry in self.entries.iter().rev().filter(|e| e.active) {
            (entry.on_resume)();
        }
    }
}

static NOTIFIERS: Mutex<NotifierChain> = Mutex::new(NotifierChain::new());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Running, // S0
    Suspending,
    Suspended,    // S3
    Hibernating,  // S4
    ShuttingDown, // S5
    Rebooting,
}

// ── Wake lock API ──────────────────────────────────────────────────────────

/// Acquire a wake lock, preventing suspend.
pub fn wake_lock_acquire() {
    WAKE_LOCKS.fetch_add(1, Ordering::SeqCst);
}

/// Release a previously acquired wake lock.
pub fn wake_lock_release() {
    // Saturating subtract: guard against underflow if release is called
    // without a matching acquire.
    let old = WAKE_LOCKS.load(Ordering::SeqCst);
    if old > 0 {
        WAKE_LOCKS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Number of active wake locks.
pub fn wake_lock_count() -> u32 {
    WAKE_LOCKS.load(Ordering::SeqCst)
}

// ── Notifier chain API ─────────────────────────────────────────────────────

/// Register a (suspend, resume) callback pair.
/// Returns a handle used to unregister later.
pub fn register_notifier(on_suspend: fn() -> bool, on_resume: fn()) -> Option<usize> {
    NOTIFIERS.lock().register(on_suspend, on_resume)
}

/// Unregister a previously registered notifier by handle.
pub fn unregister_notifier(handle: usize) {
    NOTIFIERS.lock().unregister(handle);
}

// ── Shutdown ───────────────────────────────────────────────────────────────

/// Shutdown the system (enter ACPI S5 soft-off).
pub fn shutdown() -> ! {
    serial_println!("  [power] Shutting down...");
    *POWER_STATE.lock() = PowerState::ShuttingDown;

    // Mask all interrupts so nothing races the shutdown sequence.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }

    // Notify all drivers to quiesce.
    NOTIFIERS.lock().notify_suspend();

    // Try ACPI S5 via the power/acpi module (FADT-derived PM1a/PM1b).
    if let Some(acpi) = super::acpi::get_info() {
        if acpi.pm1a_cnt_blk != 0 {
            let slp_en: u16 = 1 << 13; // SLP_EN bit
            let val: u16 = (acpi.slp_typa_s5 << 10) | slp_en;
            outw(acpi.pm1a_cnt_blk, val);

            if acpi.pm1b_cnt_blk != 0 {
                let val2: u16 = (acpi.slp_typb_s5 << 10) | slp_en;
                outw(acpi.pm1b_cnt_blk, val2);
            }
        }
    }

    // Fallback: QEMU/Bochs debug-exit port (ISA port 0x604, value 0x2000).
    outw(0x604, 0x2000);

    // Last resort: triple-fault the CPU.
    serial_println!("  [power] ACPI shutdown failed, halting CPU");
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        hlt();
    }
}

// ── Reboot ─────────────────────────────────────────────────────────────────

/// Reboot the system.
pub fn reboot() -> ! {
    serial_println!("  [power] Rebooting...");
    *POWER_STATE.lock() = PowerState::Rebooting;

    // Notify drivers (best-effort; ignore veto on reboot path).
    NOTIFIERS.lock().notify_suspend();

    // Method 1: Keyboard controller reset line (port 0x64, command 0xFE).
    // Spin until the controller input buffer is empty, then pulse the reset line.
    let mut timeout = 0xFFFFu32;
    loop {
        if inb(0x64) & 0x02 == 0 {
            break;
        }
        timeout = timeout.saturating_sub(1);
        if timeout == 0 {
            break;
        }
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }
    outb(0x64, 0xFE);

    // Short busy-wait to allow the reset line to propagate.
    for _ in 0..0x10000u32 {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }

    // Method 2: ACPI reset register (if the FADT exposes one).
    // TODO: read reset_reg / reset_val from drivers::acpi FADT extended fields
    // and write the reset value via the appropriate address space.

    // Method 3: Triple-fault — load a null IDT and trigger an exception.
    unsafe {
        let null_idtr: u128 = 0; // limit=0, base=0
        core::arch::asm!(
            "lidt [{}]",
            in(reg) &null_idtr as *const u128,
            options(nostack)
        );
        core::arch::asm!("int3", options(nostack));
    }

    loop {
        hlt();
    }
}

// ── Suspend to RAM (S3) ────────────────────────────────────────────────────

/// Suspend the system to RAM (ACPI S3).
///
/// Sequence:
///   1. Check wake locks — abort if any held.
///   2. Notify driver chain — abort if any driver vetoes.
///   3. Flush CPU caches (WBINVD).
///   4. Enter ACPI S3 via PM1a_CNT (SLP_TYP=S3, SLP_EN=1).
///   5. Execution resumes here on wakeup.
///   6. Notify driver chain of resume (reverse order).
pub fn suspend() {
    // 1. Wake-lock check.
    if WAKE_LOCKS.load(Ordering::SeqCst) > 0 {
        serial_println!(
            "  [power] Suspend blocked: {} active wake lock(s)",
            WAKE_LOCKS.load(Ordering::SeqCst)
        );
        return;
    }

    serial_println!("  [power] Suspending to RAM (S3)...");
    *POWER_STATE.lock() = PowerState::Suspending;

    // 2. Pre-suspend driver notifications.
    {
        let chain = NOTIFIERS.lock();
        if !chain.notify_suspend() {
            serial_println!("  [power] Suspend aborted by driver notifier");
            *POWER_STATE.lock() = PowerState::Running;
            return;
        }
    }

    // 3. Flush dirty cache lines back to RAM so they survive S3.
    unsafe {
        core::arch::asm!("wbinvd", options(nomem, nostack));
    }

    *POWER_STATE.lock() = PowerState::Suspended;

    // 4. Write ACPI S3 sleep entry to PM1a_CNT.
    //    SLP_TYP for S3 is platform-specific (comes from DSDT \_S3 object).
    //    We use the value stored in power/acpi if available, otherwise the
    //    typical value of 0x05 (Intel) / 0x01 (AMD).
    let (pm1a_port, slp_typ_s3) = {
        if let Some(acpi) = super::acpi::get_info() {
            // slp_typa_s5 is for S5; S3 SLP_TYP requires AML parsing.
            // TODO: extend AcpiInfo with slp_typa_s3 when AML parser is added.
            // For now we fall back to the common Intel value.
            let _ = acpi; // silence unused warning
        }
        // Common Intel/QEMU S3 SLP_TYP = 0x05; AMD = 0x03
        // Port 0x404 is the typical QEMU PM1a_CNT_BLK.
        (0x404u16, 0x05u16)
    };

    // Set SLP_TYP (bits 12:10) and SLP_EN (bit 13).
    // Read-modify-write to preserve WAK_STS and BM_STS in the control word.
    let current = crate::io::inw(pm1a_port);
    let val: u16 = (current & !0x1C00) | ((slp_typ_s3 & 0x07) << 10) | (1 << 13);
    crate::io::outw(pm1a_port, val);

    // 5. Execution resumes here when the hardware wakes up.
    //    On a real S3 resume the firmware re-POSTs and jumps to the
    //    wakeup vector saved in the FACS; control returns here.
    serial_println!("  [power] Resumed from S3");
    *POWER_STATE.lock() = PowerState::Running;

    // 6. Post-resume driver notifications (reverse order).
    NOTIFIERS.lock().notify_resume();
    serial_println!("  [power] S3 resume complete");
}

// ── CPU idle / halt ────────────────────────────────────────────────────────

/// Put the current CPU into C1 idle (HLT) until the next interrupt.
/// Call from the scheduler when there is no runnable work.
#[inline]
pub fn cpu_idle() {
    // STI is required before HLT so the interrupt that wakes us is delivered.
    unsafe {
        core::arch::asm!("sti", "hlt", options(nomem, nostack));
    }
}

/// Permanently halt the current CPU (CLI + HLT loop).
/// Used for dead CPUs and unrecoverable error paths.
pub fn cpu_halt() -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        hlt();
    }
}

// ── Query ──────────────────────────────────────────────────────────────────

/// Get current power state.
pub fn current() -> PowerState {
    *POWER_STATE.lock()
}

pub fn init() {
    serial_println!(
        "    [power] Power state manager ready (shutdown, reboot, suspend, wake-locks, notifiers)"
    );
}
