use crate::sync::Mutex;
/// Hardware/software watchdog timer framework for Genesis — no-heap, fixed-size arrays
///
/// Manages up to MAX_WATCHDOGS independent watchdog instances (software or
/// hardware-backed).  Each watchdog has its own timeout, keepalive tracking,
/// and expiration counter.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for all counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy + have const fn empty()
use crate::{serial_print, serial_println};
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered watchdog instances
pub const MAX_WATCHDOGS: usize = 4;

/// Simulated WDOG I/O port base address
pub const WDOG_IO_BASE: u16 = 0x460;

/// Default watchdog timeout in milliseconds (60 seconds)
pub const WDOG_DEFAULT_TIMEOUT_MS: u32 = 60_000;

/// Magic value used to validate watchdog operations
pub const WDT_MAGIC: u32 = 0x5A7E_3C1D;

/// Minimum allowed timeout in milliseconds (1 second)
const WDOG_MIN_TIMEOUT_MS: u32 = 1_000;

/// Maximum allowed timeout in milliseconds (1 hour)
const WDOG_MAX_TIMEOUT_MS: u32 = 3_600_000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Watchdog hardware/software type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogType {
    /// Pure software watchdog — no hardware interaction
    Software,
    /// Hardware watchdog — triggers physical reset on expiration
    Hardware,
}

/// Watchdog lifecycle state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogState {
    /// Watchdog is registered but not started
    Inactive,
    /// Watchdog is running and counting down
    Running,
    /// Watchdog has expired (keepalive was not received in time)
    Expired,
}

/// A single watchdog instance
#[derive(Clone, Copy)]
pub struct Watchdog {
    /// Unique numeric identifier assigned at registration
    pub id: u32,
    /// Hardware or software watchdog
    pub wdog_type: WatchdogType,
    /// Current lifecycle state
    pub state: WatchdogState,
    /// Configured timeout in milliseconds
    pub timeout_ms: u32,
    /// Minimum timeout accepted by this watchdog
    pub min_timeout_ms: u32,
    /// Maximum timeout accepted by this watchdog
    pub max_timeout_ms: u32,
    /// Timestamp (ms) of the most recent keepalive (or start)
    pub last_keepalive_ms: u64,
    /// Number of times this watchdog has expired
    pub expire_count: u32,
    /// True when this table slot is occupied
    pub active: bool,
}

impl Watchdog {
    /// Return a zeroed, inactive watchdog slot suitable for static initialisation
    pub const fn empty() -> Self {
        Watchdog {
            id: 0,
            wdog_type: WatchdogType::Software,
            state: WatchdogState::Inactive,
            timeout_ms: WDOG_DEFAULT_TIMEOUT_MS,
            min_timeout_ms: WDOG_MIN_TIMEOUT_MS,
            max_timeout_ms: WDOG_MAX_TIMEOUT_MS,
            last_keepalive_ms: 0,
            expire_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Table of all registered watchdog instances
static WATCHDOGS: Mutex<[Watchdog; MAX_WATCHDOGS]> = Mutex::new([Watchdog::empty(); MAX_WATCHDOGS]);

/// Monotonic millisecond counter — updated every call to watchdog_tick()
pub static WDOG_TICK_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Simulate a write to the WDOG I/O port (hardware stub).
///
/// On real hardware this would be:  `unsafe { crate::io::outl(WDOG_IO_BASE, val); }`
/// For simulation we just record the intent via serial output at trace level.
#[inline(always)]
fn wdog_io_write(_val: u32) {
    // Stub: bare-metal systems would do `crate::io::outl(WDOG_IO_BASE, _val);`
}

/// Simulate reading from the WDOG I/O port (hardware stub).
#[inline(always)]
fn wdog_io_read() -> u32 {
    // Stub: bare-metal systems would do `crate::io::inl(WDOG_IO_BASE)`
    0
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new watchdog.
///
/// `wdog_type`  — `Software` or `Hardware`.
/// `timeout_ms` — desired timeout; clamped to `[1_000, 3_600_000]` ms.
///
/// Returns the assigned `id` on success, or `None` if the watchdog table is
/// full.
pub fn watchdog_register(wdog_type: WatchdogType, timeout_ms: u32) -> Option<u32> {
    let clamped = if timeout_ms < WDOG_MIN_TIMEOUT_MS {
        WDOG_MIN_TIMEOUT_MS
    } else if timeout_ms > WDOG_MAX_TIMEOUT_MS {
        WDOG_MAX_TIMEOUT_MS
    } else {
        timeout_ms
    };

    let mut wdogs = WATCHDOGS.lock();
    for i in 0..MAX_WATCHDOGS {
        if !wdogs[i].active {
            let id = i as u32;
            wdogs[i] = Watchdog {
                id,
                wdog_type,
                state: WatchdogState::Inactive,
                timeout_ms: clamped,
                min_timeout_ms: WDOG_MIN_TIMEOUT_MS,
                max_timeout_ms: WDOG_MAX_TIMEOUT_MS,
                last_keepalive_ms: 0,
                expire_count: 0,
                active: true,
            };
            return Some(id);
        }
    }
    None
}

/// Start a registered watchdog.
///
/// Records `WDOG_TICK_MS` as the initial keepalive timestamp.
/// Returns `true` on success, `false` if `id` is invalid or the slot is
/// inactive.
pub fn watchdog_start(id: u32) -> bool {
    if id as usize >= MAX_WATCHDOGS {
        return false;
    }
    let current_ms = WDOG_TICK_MS.load(Ordering::Relaxed);
    let mut wdogs = WATCHDOGS.lock();
    let slot = &mut wdogs[id as usize];
    if !slot.active {
        return false;
    }
    slot.state = WatchdogState::Running;
    slot.last_keepalive_ms = current_ms;
    // For hardware watchdogs: arm the hardware timer (stub)
    if slot.wdog_type == WatchdogType::Hardware {
        wdog_io_write(WDT_MAGIC);
    }
    true
}

/// Stop a running watchdog (set state to Inactive).
///
/// Returns `true` on success, `false` if `id` is invalid or the slot is
/// inactive.
pub fn watchdog_stop(id: u32) -> bool {
    if id as usize >= MAX_WATCHDOGS {
        return false;
    }
    let mut wdogs = WATCHDOGS.lock();
    let slot = &mut wdogs[id as usize];
    if !slot.active {
        return false;
    }
    slot.state = WatchdogState::Inactive;
    // For hardware watchdogs: disarm the hardware timer (stub)
    if slot.wdog_type == WatchdogType::Hardware {
        wdog_io_write(0);
    }
    true
}

/// Feed/keepalive a watchdog.
///
/// Updates `last_keepalive_ms` to `current_ms`.  Only effective when the
/// watchdog is in the `Running` state.
/// Returns `true` on success, `false` if `id` is invalid, inactive, or not
/// running.
pub fn watchdog_keepalive(id: u32, current_ms: u64) -> bool {
    if id as usize >= MAX_WATCHDOGS {
        return false;
    }
    let mut wdogs = WATCHDOGS.lock();
    let slot = &mut wdogs[id as usize];
    if !slot.active || slot.state != WatchdogState::Running {
        return false;
    }
    slot.last_keepalive_ms = current_ms;
    // For hardware watchdogs: kick the hardware counter (stub)
    if slot.wdog_type == WatchdogType::Hardware {
        wdog_io_write(WDT_MAGIC);
    }
    true
}

/// Change the timeout of a watchdog.
///
/// Only permitted when the watchdog is in the `Inactive` state.  The new
/// value is clamped to `[min_timeout_ms, max_timeout_ms]`.
/// Returns `true` on success, `false` if `id` is invalid, inactive, or
/// currently running.
pub fn watchdog_set_timeout(id: u32, timeout_ms: u32) -> bool {
    if id as usize >= MAX_WATCHDOGS {
        return false;
    }
    let mut wdogs = WATCHDOGS.lock();
    let slot = &mut wdogs[id as usize];
    if !slot.active {
        return false;
    }
    // Only allowed when stopped
    if slot.state != WatchdogState::Inactive {
        return false;
    }
    let clamped = if timeout_ms < slot.min_timeout_ms {
        slot.min_timeout_ms
    } else if timeout_ms > slot.max_timeout_ms {
        slot.max_timeout_ms
    } else {
        timeout_ms
    };
    slot.timeout_ms = clamped;
    true
}

/// Periodic watchdog tick — call with the current system uptime in
/// milliseconds (typically from the timer interrupt handler).
///
/// For every `Running` watchdog whose deadline has elapsed:
///   1. Sets state to `Expired`.
///   2. Increments `expire_count` with saturating addition.
///   3. Prints an expiration notice via `serial_println!`.
///   4. For `Hardware` watchdogs: the hardware will independently trigger a
///      system reset; we record a comment-stub for that path here.
pub fn watchdog_tick(current_ms: u64) {
    // Update the global tick counter so other functions can read "now"
    WDOG_TICK_MS.store(current_ms, Ordering::Relaxed);

    // We intentionally hold the lock for the full scan.  The tick handler is
    // brief (≤ MAX_WATCHDOGS iterations) and must not drop+reacquire between
    // iterations to avoid missed expirations.
    let mut wdogs = WATCHDOGS.lock();
    for i in 0..MAX_WATCHDOGS {
        if !wdogs[i].active {
            continue;
        }
        if wdogs[i].state != WatchdogState::Running {
            continue;
        }
        let elapsed = current_ms.saturating_sub(wdogs[i].last_keepalive_ms);
        if elapsed >= wdogs[i].timeout_ms as u64 {
            let id = wdogs[i].id;
            wdogs[i].state = WatchdogState::Expired;
            wdogs[i].expire_count = wdogs[i].expire_count.saturating_add(1);

            serial_println!("[watchdog] WATCHDOG {} EXPIRED — system reset stub", id);

            if wdogs[i].wdog_type == WatchdogType::Hardware {
                // Hardware watchdog reset stub:
                // A real implementation would call:
                //   crate::power_mgmt::reset::system_reboot();
                // The hardware timer will independently fire a reset pulse
                // via WDOG_IO_BASE if armed.
            }
        }
    }
}

/// Query how many milliseconds remain before a watchdog expires.
///
/// Returns `Some(ms_remaining)` for a `Running` watchdog, or `None` if `id`
/// is invalid, inactive, or not running.
pub fn watchdog_get_timeleft(id: u32, current_ms: u64) -> Option<u64> {
    if id as usize >= MAX_WATCHDOGS {
        return None;
    }
    let wdogs = WATCHDOGS.lock();
    let slot = &wdogs[id as usize];
    if !slot.active || slot.state != WatchdogState::Running {
        return None;
    }
    let elapsed = current_ms.saturating_sub(slot.last_keepalive_ms);
    let timeout = slot.timeout_ms as u64;
    if elapsed >= timeout {
        Some(0)
    } else {
        Some(timeout.saturating_sub(elapsed))
    }
}

/// Get the current state and expiration count of a watchdog.
///
/// Returns `Some((state, expire_count))` for a valid, active slot, or `None`
/// if `id` is invalid or inactive.
pub fn watchdog_get_stats(id: u32) -> Option<(WatchdogState, u32)> {
    if id as usize >= MAX_WATCHDOGS {
        return None;
    }
    let wdogs = WATCHDOGS.lock();
    let slot = &wdogs[id as usize];
    if !slot.active {
        return None;
    }
    Some((slot.state, slot.expire_count))
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the watchdog framework.
///
/// Registers a default software watchdog with `WDOG_DEFAULT_TIMEOUT_MS` and
/// prints a boot message.
pub fn init() {
    if let Some(id) = watchdog_register(WatchdogType::Software, WDOG_DEFAULT_TIMEOUT_MS) {
        serial_println!(
            "[watchdog] framework initialized (default sw watchdog id={}, timeout={}ms)",
            id,
            WDOG_DEFAULT_TIMEOUT_MS
        );
    } else {
        serial_println!("[watchdog] framework initialized (default watchdog registration failed)");
    }
}
