use crate::debug::oops::{kernel_oops, CrashRegs};
use crate::io::{inb, inl, outb, outl};
/// Hardware and software watchdog for Genesis AIOS
///
/// ## Software watchdog
/// Up to `MAX_CHANNELS` named software watchdog channels.  Each channel has a
/// timeout in milliseconds.  The caller must periodically call
/// `watchdog_pet(channel_id)` to prevent the channel from expiring.  The
/// timer ISR calls `watchdog_tick(elapsed_ms)` on every tick.  If a channel
/// has not been petted within its timeout, `oops::kernel_oops()` is called.
///
/// ## Hardware (TCO) watchdog
/// Intel ICH/PCH TCO (Timer/Counter Overflow) watchdog.  Detected via the
/// LPC device at PCI D31:F0, PMBASE + 0x60 register block.  One tick ≈ 0.6 s.
///
/// Strictly follows kernel coding rules:
///   - no_std, no alloc, no Vec/Box/String
///   - no float casts
///   - saturating arithmetic for counters
///   - read_volatile / write_volatile for all I/O register access
///   - no panic — serial_println! + early return on errors
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Software watchdog channel
// ---------------------------------------------------------------------------

/// Maximum number of concurrent software watchdog channels.
const MAX_CHANNELS: usize = 8;

/// Maximum name length for a watchdog channel.
const MAX_NAME: usize = 32;

/// One software watchdog channel.
pub struct WatchdogChannel {
    /// Human-readable channel name (null-padded, not necessarily null-terminated).
    pub name: [u8; MAX_NAME],
    /// Watchdog timeout in milliseconds.
    pub timeout_ms: u32,
    /// TSC/ms counter value when the channel was last petted.
    /// Written by `watchdog_pet`, read by `watchdog_tick`.
    pub last_pet_ms: AtomicU64,
    /// Whether this channel is active (registered and not yet disabled).
    pub active: AtomicBool,
    /// Set to true when the watchdog has fired for this channel.
    pub triggered: AtomicBool,
}

impl WatchdogChannel {
    const fn new() -> Self {
        WatchdogChannel {
            name: [0u8; MAX_NAME],
            timeout_ms: 0,
            last_pet_ms: AtomicU64::new(0),
            active: AtomicBool::new(false),
            triggered: AtomicBool::new(false),
        }
    }
}

// ---------------------------------------------------------------------------
// Global channel array
// ---------------------------------------------------------------------------

/// Fixed-size array of software watchdog channels.
/// Channels are allocated by index; `active` distinguishes used from free.
static WATCHDOG_CHANNELS: [WatchdogChannel; MAX_CHANNELS] = [
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
    const { WatchdogChannel::new() },
];

/// Elapsed milliseconds counter incremented by `watchdog_tick`.
/// Gives each channel a monotonic "last petted at" reference.
static UPTIME_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Software watchdog public API
// ---------------------------------------------------------------------------

/// Register a new watchdog channel.
///
/// Returns `Some(channel_id)` on success, `None` if all slots are occupied.
/// The channel starts active immediately with `last_pet_ms` set to now.
pub fn watchdog_register(name: &str, timeout_ms: u32) -> Option<usize> {
    let now_ms = UPTIME_MS.load(Ordering::Relaxed);

    for (id, ch) in WATCHDOG_CHANNELS.iter().enumerate() {
        // Try to claim a free slot with a compare-exchange.
        if ch
            .active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            // Copy name bytes.
            // Safety: we are the only writer right now (active was just set).
            let name_bytes = name.as_bytes();
            let copy_len = name_bytes.len().min(MAX_NAME);

            // We need interior mutability for the name bytes.  Because
            // WATCHDOG_CHANNELS is `static`, we use a pointer cast under
            // the guarantee that `active=false` was a fence and no other
            // CPU can touch this slot's name field simultaneously.
            let ch_ptr = ch as *const WatchdogChannel as *mut WatchdogChannel;
            unsafe {
                let name_ptr = (*ch_ptr).name.as_mut_ptr();
                core::ptr::write_bytes(name_ptr, 0, MAX_NAME);
                core::ptr::copy_nonoverlapping(name_bytes.as_ptr(), name_ptr, copy_len);
                (*ch_ptr).timeout_ms = timeout_ms;
            }

            ch.last_pet_ms.store(now_ms, Ordering::Release);
            ch.triggered.store(false, Ordering::Relaxed);

            crate::serial_println!(
                "  [watchdog] registered channel {} '{}' timeout={}ms",
                id,
                name,
                timeout_ms
            );
            return Some(id);
        }
    }
    crate::serial_println!(
        "  [watchdog] ERROR: no free channels (max={})",
        MAX_CHANNELS
    );
    None
}

/// Reset (pet) a watchdog channel, preventing it from firing.
pub fn watchdog_pet(channel: u32) {
    let id = channel as usize;
    if id >= MAX_CHANNELS {
        return;
    }
    let ch = &WATCHDOG_CHANNELS[id];
    if !ch.active.load(Ordering::Relaxed) {
        return;
    }
    let now_ms = UPTIME_MS.load(Ordering::Relaxed);
    ch.last_pet_ms.store(now_ms, Ordering::Release);
}

/// Disable a watchdog channel.  The slot is freed for reuse.
pub fn watchdog_disable(channel: u32) {
    let id = channel as usize;
    if id >= MAX_CHANNELS {
        return;
    }
    let ch = &WATCHDOG_CHANNELS[id];
    ch.active.store(false, Ordering::Release);
    crate::serial_println!("  [watchdog] channel {} disabled", id);
}

/// Called from the timer ISR with the elapsed milliseconds since the last tick.
///
/// Advances the internal uptime counter and checks every active channel.
/// If a channel's timeout has elapsed, `kernel_oops` is called once
/// (the `triggered` flag prevents repeated firings).
pub fn watchdog_tick(elapsed_ms: u32) {
    // Advance the uptime counter using saturating add to avoid wrapping issues.
    let elapsed64 = elapsed_ms as u64;
    let now_ms = UPTIME_MS
        .fetch_add(elapsed64, Ordering::Relaxed)
        .saturating_add(elapsed64);

    for (id, ch) in WATCHDOG_CHANNELS.iter().enumerate() {
        if !ch.active.load(Ordering::Acquire) {
            continue;
        }

        let last_pet = ch.last_pet_ms.load(Ordering::Acquire);
        let timeout = ch.timeout_ms as u64;

        let elapsed_since_pet = now_ms.saturating_sub(last_pet);

        if elapsed_since_pet >= timeout {
            // Only fire once per expiry event.
            if ch
                .triggered
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                // Build a minimal CrashRegs for context.
                let mut regs = CrashRegs::zeroed();
                unsafe {
                    core::arch::asm!(
                        "lea {rip}, [rip]",
                        rip = out(reg) regs.rip,
                        options(nomem, nostack),
                    );
                    core::arch::asm!(
                        "mov {rsp}, rsp",
                        rsp = out(reg) regs.rsp,
                        options(nomem, nostack),
                    );
                    core::arch::asm!(
                        "mov {rbp}, rbp",
                        rbp = out(reg) regs.rbp,
                        options(nomem, nostack),
                    );
                }

                // Build a fixed-size message without heap allocation.
                let mut msg_buf = [0u8; 128];
                let name_len = {
                    let mut n = 0usize;
                    while n < MAX_NAME && ch.name[n] != 0 {
                        n += 1;
                    }
                    n
                };
                // Manually write "watchdog timeout: channel N (name)"
                let prefix = b"watchdog timeout: channel ";
                let copy = prefix.len().min(msg_buf.len());
                msg_buf[..copy].copy_from_slice(&prefix[..copy]);
                let mut pos = copy;

                // Append decimal channel id.
                let id_str = id_to_decimal(id as u32);
                for &b in &id_str {
                    if pos >= msg_buf.len() {
                        break;
                    }
                    msg_buf[pos] = b;
                    pos = pos.saturating_add(1);
                }

                if pos < msg_buf.len() {
                    msg_buf[pos] = b' ';
                    pos = pos.saturating_add(1);
                }
                if pos < msg_buf.len() {
                    msg_buf[pos] = b'(';
                    pos = pos.saturating_add(1);
                }
                let name_copy = name_len.min(msg_buf.len().saturating_sub(pos).saturating_sub(1));
                msg_buf[pos..pos + name_copy].copy_from_slice(&ch.name[..name_copy]);
                pos = pos.saturating_add(name_copy);
                if pos < msg_buf.len() {
                    msg_buf[pos] = b')';
                    pos = pos.saturating_add(1);
                }

                let msg_str = core::str::from_utf8(&msg_buf[..pos]).unwrap_or("watchdog timeout");
                crate::serial_println!("  [watchdog] EXPIRED: {}", msg_str);
                kernel_oops(msg_str, &regs);
            }
        }
    }
}

/// Convert a u32 to a fixed-size decimal ASCII buffer (max 10 digits + NUL).
fn id_to_decimal(mut n: u32) -> [u8; 11] {
    let mut buf = [0u8; 11];
    if n == 0 {
        buf[0] = b'0';
        return buf;
    }
    let mut i = 10usize;
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    // Shift digits to the front.
    let start = i;
    let len = 10 - start;
    for j in 0..len {
        buf[j] = buf[start + j];
    }
    for j in len..11 {
        buf[j] = 0;
    }
    buf
}

// ---------------------------------------------------------------------------
// Hardware (Intel TCO) watchdog
// ---------------------------------------------------------------------------

// TCO register offsets from PMBASE (typically 0x400 on ICH/PCH).
// PMBASE is read from PCI D31:F0 config offset 0x40, bits [15:1] mask 0xFFFE.

const PCI_TCO_BUS: u32 = 0;
const PCI_TCO_DEVICE: u32 = 31;
const PCI_TCO_FUNC: u32 = 0;
const PCI_TCO_OFFSET: u32 = 0x40; // ACPI base address register

/// Offset of TCO registers within the ACPI/PMBASE I/O space.
const TCO_BASE_OFFSET: u16 = 0x60;

const TCO_TMR: u16 = 0x01; // TCO_TMR: initial count / timeout value
const TCO1_STS: u16 = 0x04; // TCO1_STS: status flags (bit 3 = timeout)
const TCO_CNT: u16 = 0x08; // TCO_CNT: control (bit 11 = NO_REBOOT in GCS)

/// GCS register in PCI config space D31:F0 (used to set NO_REBOOT).
const PCI_GCS_OFFSET: u32 = 0xC4;
const GCS_NO_REBOOT: u32 = 1 << 5;

/// PCI configuration address port.
const PCI_CFG_ADDR: u16 = 0xCF8;
/// PCI configuration data port.
const PCI_CFG_DATA: u16 = 0xCFC;

/// Resolved PMBASE + TCO_BASE_OFFSET for TCO register access.
/// 0 means TCO was not found.
static TCO_BASE: AtomicU32 = AtomicU32::new(0);

/// Whether the hardware TCO watchdog has been successfully initialized.
static HW_WATCHDOG_ACTIVE: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// PCI config space helpers (32-bit read/write)
// ---------------------------------------------------------------------------

fn pci_read32(bus: u32, dev: u32, func: u32, offset: u32) -> u32 {
    let addr: u32 = (1 << 31)
        | ((bus & 0xFF) << 16)
        | ((dev & 0x1F) << 11)
        | ((func & 0x07) << 8)
        | (offset & 0xFC);
    outl(PCI_CFG_ADDR, addr);
    inl(PCI_CFG_DATA)
}

fn pci_write32(bus: u32, dev: u32, func: u32, offset: u32, val: u32) {
    let addr: u32 = (1 << 31)
        | ((bus & 0xFF) << 16)
        | ((dev & 0x1F) << 11)
        | ((func & 0x07) << 8)
        | (offset & 0xFC);
    outl(PCI_CFG_ADDR, addr);
    outl(PCI_CFG_DATA, val);
}

// ---------------------------------------------------------------------------
// TCO register access helpers
// ---------------------------------------------------------------------------

/// Read 1 byte from a TCO register (tco_base + off).
#[inline]
fn tco_in8(tco_base: u16, off: u16) -> u8 {
    inb(tco_base.saturating_add(off))
}

/// Write 1 byte to a TCO register.
#[inline]
fn tco_out8(tco_base: u16, off: u16, val: u8) {
    outb(tco_base.saturating_add(off), val);
}

/// Read 2 bytes from a TCO register.
#[inline]
fn tco_in16(tco_base: u16, off: u16) -> u16 {
    crate::io::inw(tco_base.saturating_add(off))
}

/// Write 2 bytes to a TCO register.
#[inline]
fn tco_out16(tco_base: u16, off: u16, val: u16) {
    crate::io::outw(tco_base.saturating_add(off), val);
}

// ---------------------------------------------------------------------------
// Public hardware watchdog API
// ---------------------------------------------------------------------------

/// Attempt to detect and initialize the Intel TCO hardware watchdog.
///
/// Returns `true` if the TCO was found and initialized.
pub fn hwtimer_init() -> bool {
    // Read PMBASE from PCI D31:F0 offset 0x40, bits [15:1].
    let pmbase_raw = pci_read32(PCI_TCO_BUS, PCI_TCO_DEVICE, PCI_TCO_FUNC, PCI_TCO_OFFSET);
    let pmbase = (pmbase_raw & 0xFF80) as u16; // bits[15:7], 128-byte aligned

    if pmbase == 0 {
        crate::serial_println!("  [watchdog] TCO: PMBASE=0, hardware watchdog not found");
        return false;
    }

    let tco_base = pmbase.saturating_add(TCO_BASE_OFFSET);

    // Sanity-check: read TCO_CNT; it should not be all-F's (absent device).
    let tco_cnt = tco_in16(tco_base, TCO_CNT);
    if tco_cnt == 0xFFFF {
        crate::serial_println!("  [watchdog] TCO: registers not responding (0xFFFF), skipping");
        return false;
    }

    TCO_BASE.store(tco_base as u32, Ordering::Relaxed);
    HW_WATCHDOG_ACTIVE.store(true, Ordering::Release);

    // Clear any pending timeout status.
    tco_out8(tco_base, TCO1_STS, 0x08); // bit 3 = TCO_TIMEOUT flag

    crate::serial_println!(
        "  [watchdog] TCO hardware watchdog found at I/O {:#06x}",
        tco_base
    );
    true
}

/// Pet (reload) the TCO hardware watchdog timer.
///
/// Writes 0x01 to TCO_RLD (offset 0x00) and clears TCO_TIMEOUT status.
pub fn hwtimer_pet() {
    if !HW_WATCHDOG_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let tco_base = TCO_BASE.load(Ordering::Relaxed) as u16;
    // Writing 0x01 to TCO_RLD reloads the counter from TCO_TMR.
    tco_out8(tco_base, 0x00, 0x01);
    // Clear timeout status bit.
    tco_out8(tco_base, TCO1_STS, 0x08);
}

/// Set the TCO hardware watchdog timeout.
///
/// The timeout register holds units of ~0.6 s per tick (600 ms on PCH).
/// Valid range is 2–63 ticks (1.2 s – 37.8 s).  Values outside [2, 63]
/// are clamped.
pub fn hwtimer_set_timeout(secs: u8) {
    if !HW_WATCHDOG_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    // Convert seconds to ~0.6 s ticks (multiply by 5, divide by 3).
    // Use integer arithmetic only — no float casts.
    // ticks ≈ secs * 10 / 6  (0.6 s per tick)
    let ticks_wide: u32 = (secs as u32).saturating_mul(10) / 6;
    let ticks: u8 = if ticks_wide < 2 {
        2
    } else if ticks_wide > 63 {
        63
    } else {
        ticks_wide as u8
    };

    let tco_base = TCO_BASE.load(Ordering::Relaxed) as u16;
    tco_out8(tco_base, TCO_TMR, ticks);
    // Reload so the new timeout takes effect immediately.
    hwtimer_pet();
    crate::serial_println!(
        "  [watchdog] TCO timeout set to {} ticks (~{} s)",
        ticks,
        secs
    );
}

/// Stop the TCO hardware watchdog (prevent system reset on expiry).
///
/// Sets the NO_REBOOT bit in the GCS PCI config register (D31:F0 offset 0xC4).
/// Note: depending on the chipset, writing TCO_CNT bit 11 ("HLT" bit) also
/// disables the TCO counter.  We set both for maximum compatibility.
pub fn hwtimer_stop() {
    if !HW_WATCHDOG_ACTIVE.load(Ordering::Relaxed) {
        return;
    }

    // Set NO_REBOOT in GCS.
    let gcs = pci_read32(PCI_TCO_BUS, PCI_TCO_DEVICE, PCI_TCO_FUNC, PCI_GCS_OFFSET);
    pci_write32(
        PCI_TCO_BUS,
        PCI_TCO_DEVICE,
        PCI_TCO_FUNC,
        PCI_GCS_OFFSET,
        gcs | GCS_NO_REBOOT,
    );

    // Set HLT bit in TCO_CNT (bit 11) to stop the counter.
    let tco_base = TCO_BASE.load(Ordering::Relaxed) as u16;
    let cnt = tco_in16(tco_base, TCO_CNT);
    tco_out16(tco_base, TCO_CNT, cnt | (1 << 11));

    HW_WATCHDOG_ACTIVE.store(false, Ordering::Release);
    crate::serial_println!("  [watchdog] TCO hardware watchdog stopped (NO_REBOOT set)");
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

/// Initialize the watchdog subsystem.
///
/// - Attempts to detect the TCO hardware watchdog.
/// - The hardware watchdog is stopped by default to prevent unintended resets
///   during early boot.  Callers that want it armed must call
///   `hwtimer_set_timeout` and `hwtimer_pet` explicitly.
pub fn init() {
    UPTIME_MS.store(0, Ordering::Relaxed);

    // Deactivate all software channels.
    for ch in WATCHDOG_CHANNELS.iter() {
        ch.active.store(false, Ordering::Relaxed);
        ch.triggered.store(false, Ordering::Relaxed);
        ch.last_pet_ms.store(0, Ordering::Relaxed);
    }

    // Try hardware watchdog; stop it so it doesn't reset during boot.
    if hwtimer_init() {
        hwtimer_stop();
    }

    crate::serial_println!(
        "  [watchdog] Watchdog subsystem initialized ({} SW channels)",
        MAX_CHANNELS
    );
}
