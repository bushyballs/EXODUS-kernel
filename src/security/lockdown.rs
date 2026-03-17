/// Kernel lockdown for Genesis
///
/// Restricts dangerous kernel operations even for root:
///   - No loading unsigned kernel modules
///   - No writing to /dev/mem or /dev/kmem
///   - No accessing raw I/O ports from userspace
///   - No modifying kernel memory via debugfs
///   - No disabling security features (ASLR, NX, etc.)
///   - No raw disk writes to mounted filesystems
///   - No kexec with unsigned images
///   - No hibernation image without signature
///   - No BPF writes to kernel memory
///
/// Two modes:
///   - Integrity: prevent unauthorized kernel modification
///   - Confidentiality: integrity + prevent kernel data leaks
///
/// Inspired by: Linux kernel lockdown LSM, macOS SIP.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;

static LOCKDOWN: Mutex<LockdownState> = Mutex::new(LockdownState::new());

/// Lockdown mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockdownMode {
    /// No restrictions
    None,
    /// Prevent unauthorized kernel modification
    Integrity,
    /// Integrity + prevent kernel data leaks
    Confidentiality,
}

/// Operations that lockdown restricts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockdownOp {
    /// Load a kernel module
    ModuleLoad,
    /// Write to /dev/mem or /dev/kmem
    RawMemoryWrite,
    /// Access I/O ports from userspace
    IoPortAccess,
    /// Access debugfs
    DebugfsAccess,
    /// Disable ASLR
    DisableAslr,
    /// Disable NX/SMEP/SMAP
    DisableHardening,
    /// Raw disk write to mounted filesystem
    RawDiskWrite,
    /// kexec with unsigned image
    UnsignedKexec,
    /// Read /dev/mem or /dev/kmem
    RawMemoryRead,
    /// Read kernel symbol addresses (/proc/kallsyms)
    KernelSymbols,
    /// Access performance counters
    PerfCounters,
    /// Write to ACPI tables
    AcpiWrite,
    /// Modify MSR registers
    MsrWrite,
    /// Use eBPF to access kernel memory
    BpfKernelAccess,
    /// Hibernate with unsigned image
    UnsignedHibernate,
}

/// Lockdown state
pub struct LockdownState {
    pub mode: LockdownMode,
    pub locked: bool, // Once locked, mode can only increase
    pub denied_count: u64,
    pub audit_denials: bool,
}

impl LockdownState {
    const fn new() -> Self {
        LockdownState {
            mode: LockdownMode::None,
            locked: false,
            denied_count: 0,
            audit_denials: true,
        }
    }
}

/// Check if an operation is allowed under current lockdown mode
pub fn check(op: LockdownOp) -> bool {
    let mut state = LOCKDOWN.lock();

    if state.mode == LockdownMode::None {
        return true;
    }

    let denied = match state.mode {
        LockdownMode::None => false,
        LockdownMode::Integrity => matches!(
            op,
            LockdownOp::ModuleLoad
                | LockdownOp::RawMemoryWrite
                | LockdownOp::IoPortAccess
                | LockdownOp::DebugfsAccess
                | LockdownOp::DisableAslr
                | LockdownOp::DisableHardening
                | LockdownOp::RawDiskWrite
                | LockdownOp::UnsignedKexec
                | LockdownOp::AcpiWrite
                | LockdownOp::MsrWrite
                | LockdownOp::BpfKernelAccess
                | LockdownOp::UnsignedHibernate
        ),
        LockdownMode::Confidentiality => matches!(
            op,
            LockdownOp::ModuleLoad
                | LockdownOp::RawMemoryWrite
                | LockdownOp::RawMemoryRead
                | LockdownOp::IoPortAccess
                | LockdownOp::DebugfsAccess
                | LockdownOp::DisableAslr
                | LockdownOp::DisableHardening
                | LockdownOp::RawDiskWrite
                | LockdownOp::UnsignedKexec
                | LockdownOp::KernelSymbols
                | LockdownOp::PerfCounters
                | LockdownOp::AcpiWrite
                | LockdownOp::MsrWrite
                | LockdownOp::BpfKernelAccess
                | LockdownOp::UnsignedHibernate
        ),
    };

    if denied {
        state.denied_count = state.denied_count.saturating_add(1);
        if state.audit_denials {
            serial_println!("  [lockdown] DENIED: {:?} (mode={:?})", op, state.mode);
            crate::security::audit::log(
                crate::security::audit::AuditEvent::PolicyChange,
                crate::security::audit::AuditResult::Deny,
                0,
                0,
                &alloc::format!("lockdown denied: {:?}", op),
            );
        }
    }

    !denied
}

/// Set lockdown mode (can only increase once locked)
pub fn set_mode(mode: LockdownMode) -> Result<(), &'static str> {
    let mut state = LOCKDOWN.lock();

    if state.locked {
        // Can only increase lockdown level
        let current = state.mode as u8;
        let new = mode as u8;
        if new < current {
            return Err("lockdown can only be increased once locked");
        }
    }

    state.mode = mode;
    serial_println!("  [lockdown] Mode set to {:?}", mode);
    Ok(())
}

/// Lock the lockdown — mode can only increase from here
pub fn lock() {
    let mut state = LOCKDOWN.lock();
    state.locked = true;
    serial_println!("  [lockdown] Lockdown locked (mode: {:?})", state.mode);
}

/// Check if lockdown is active
pub fn is_locked() -> bool {
    LOCKDOWN.lock().locked
}

/// Get current lockdown mode
pub fn mode() -> LockdownMode {
    LOCKDOWN.lock().mode
}

/// Initialize kernel lockdown
pub fn init(mode: LockdownMode) {
    {
        let mut state = LOCKDOWN.lock();
        state.mode = mode;
    }

    if mode != LockdownMode::None {
        // Auto-lock in non-debug builds
        lock();
    }

    serial_println!("  [lockdown] Kernel lockdown: {:?}", mode);
}
