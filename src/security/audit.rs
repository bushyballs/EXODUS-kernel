/// Security audit log for Genesis
///
/// Records all security-relevant events in a lock-protected ring buffer.
/// The buffer holds the most recent MAX_ENTRIES events; older events are
/// silently overwritten (dropped counter is maintained).
///
/// Supported event kinds:
///   SyscallEntry  — a syscall was dispatched (number + args)
///   SyscallExit   — a syscall returned (number + return value)
///   FileOpen      — a file was opened
///   NetworkConn   — a network connection was initiated
///   CapCheck      — a capability was checked
///   ProcessFork   — a new process was forked
///   ProcessExit   — a process exited
///   SeccompKill   — a process was killed by seccomp
///   CapDenied     — legacy alias for policy denial
///   MacCheck      — MAC check performed
///   MacDenied     — MAC check denied
///   AuthSuccess   — authentication succeeded
///   AuthFailed    — authentication failed
///   ProcessSpawn  — process spawned (alias for ProcessFork)
///   PrivilegeChange — a privilege change was performed
///   PolicyChange  — security policy was modified
///   FileAccess    — file access performed or denied
///   NetworkConnect — network connection event (alias)
///
/// Hardening events (new — for kernel security mitigations):
///   SyscallBlocked    — seccomp / syscall filter blocked a syscall
///   PrivilegeEscalation — UID transition detected
///   StackOverflow     — kernel stack overflow detected via canary / guard page
///   NullDeref         — null-pointer dereference fault
///   KernelExploit     — suspected kernel exploit (PF at unexpected RIP)
///   SignatureFail     — module signature verification failed
///   AuthFail          — authentication failure (with reason code)
///   CapabilityDenied  — capability check denied (detailed record)
///
/// The hardening events are stored in a *separate* no-alloc 1024-entry ring
/// (`HARDEN_LOG`) so they are available even before the full `alloc` heap is
/// initialised.  Use `audit_log`, `audit_read`, and `audit_count` to access
/// this ring.
///
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;

/// Maximum ring-buffer entries.
const MAX_ENTRIES: usize = 4096;

static AUDIT_LOG: Mutex<AuditLog> = Mutex::new(AuditLog::new());

// ── Rich event enum ───────────────────────────────────────────────────────────

/// A security-relevant audit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEvent {
    // ── Syscall lifecycle ─────────────────────────────────────────────────
    /// A syscall was entered (dispatched to the handler).
    SyscallEntry,
    /// A syscall handler returned.
    SyscallExit,

    // ── File access ───────────────────────────────────────────────────────
    /// A file inode was opened.
    FileOpen,
    /// A file access (read/write/execute) was attempted.
    FileAccess,

    // ── Network ───────────────────────────────────────────────────────────
    /// A network connection was initiated.
    NetworkConn,
    /// Alias for NetworkConn (used by LSM hooks).
    NetworkConnect,

    // ── Capability checks ─────────────────────────────────────────────────
    /// A Linux capability was checked and granted.
    CapCheck,
    /// A Linux capability was checked and denied.
    CapDenied,

    // ── Process lifecycle ─────────────────────────────────────────────────
    /// A process was forked.
    ProcessFork,
    /// Alias for ProcessFork (used by some hooks).
    ProcessSpawn,
    /// A process exited.
    ProcessExit,

    // ── Seccomp ───────────────────────────────────────────────────────────
    /// A process was killed by the seccomp filter.
    SeccompKill,

    // ── MAC / policy ──────────────────────────────────────────────────────
    /// A MAC policy check was performed and allowed.
    MacCheck,
    /// A MAC policy check was performed and denied.
    MacDenied,
    /// An authentication attempt succeeded.
    AuthSuccess,
    /// An authentication attempt failed.
    AuthFailed,
    /// A privilege level change was performed.
    PrivilegeChange,
    /// A security policy was modified.
    PolicyChange,
}

// ── Result classification ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Allow,
    Deny,
    Info,
}

// ── Detailed event payloads ───────────────────────────────────────────────────

/// Per-event detail payload stored alongside the base entry.
///
/// Stored in a fixed-size union-like enum so we do not need heap allocation.
#[derive(Debug, Clone, Copy)]
pub enum AuditDetail {
    /// No additional detail.
    None,
    /// Syscall entry/exit detail.
    Syscall { nr: u32, args: [u64; 6], ret: i64 },
    /// File open detail.
    FileOpen { inode: u64, flags: u32 },
    /// Network connection detail.
    Network { dst_ip: u32, dst_port: u16 },
    /// Capability check detail.
    Capability { cap: u64, granted: bool },
    /// Process fork/exit detail.
    Process { parent: u32, child: u32, code: i32 },
    /// Seccomp kill detail.
    Seccomp { syscall_nr: u32 },
}

// ── Ring-buffer entry ─────────────────────────────────────────────────────────

/// A single audit log entry.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Event kind.
    pub event: AuditEvent,
    /// Allow/Deny/Info.
    pub result: AuditResult,
    /// PID of the process involved.
    pub pid: u32,
    /// UID of the process (0 when not applicable).
    pub uid: u32,
    /// Structured detail (does not need a string).
    pub detail: AuditDetail,
    /// Detail offset into the string ring (for free-form messages).
    pub detail_offset: usize,
    /// Length of the string detail.
    pub detail_len: usize,
}

// ── Audit log ring buffer ─────────────────────────────────────────────────────

pub struct AuditLog {
    /// Slot ring.
    entries: [Option<AuditEntry>; MAX_ENTRIES],
    /// Write cursor (mod MAX_ENTRIES = current slot).
    head: usize,
    /// Number of valid entries (capped at MAX_ENTRIES).
    count: usize,
    /// Monotonic sequence counter.
    seq: u64,
    /// Events dropped due to the ring being full.
    pub dropped: u64,
    /// String detail ring buffer (circular byte store).
    detail_buf: [u8; 16384],
    /// Write cursor into detail_buf.
    detail_head: usize,
}

impl AuditLog {
    pub const fn new() -> Self {
        AuditLog {
            entries: [const { None }; MAX_ENTRIES],
            head: 0,
            count: 0,
            seq: 0,
            dropped: 0,
            detail_buf: [0u8; 16384],
            detail_head: 0,
        }
    }

    /// Store a free-form string detail in the circular text buffer.
    ///
    /// Returns (offset, len) into `detail_buf`.
    fn write_detail_str(&mut self, detail: &str) -> (usize, usize) {
        let bytes = detail.as_bytes();
        // Cap detail strings at 256 bytes.
        let len = if bytes.len() > 256 { 256 } else { bytes.len() };
        let offset = self.detail_head;
        for i in 0..len {
            self.detail_buf[(self.detail_head + i) % 16384] = bytes[i];
        }
        self.detail_head = (self.detail_head + len) % 16384;
        (offset, len)
    }

    /// Record an event with a free-form string detail.
    pub fn log(
        &mut self,
        event: AuditEvent,
        result: AuditResult,
        pid: u32,
        uid: u32,
        detail: &str,
    ) {
        let (offset, len) = self.write_detail_str(detail);
        self.record_entry(event, result, pid, uid, AuditDetail::None, offset, len);
    }

    /// Record an event with a structured payload.
    pub fn log_detail(
        &mut self,
        event: AuditEvent,
        result: AuditResult,
        pid: u32,
        uid: u32,
        payload: AuditDetail,
    ) {
        self.record_entry(event, result, pid, uid, payload, 0, 0);
    }

    fn record_entry(
        &mut self,
        event: AuditEvent,
        result: AuditResult,
        pid: u32,
        uid: u32,
        payload: AuditDetail,
        detail_offset: usize,
        detail_len: usize,
    ) {
        let seq = self.seq;
        self.seq = self.seq.saturating_add(1);

        // Check if we are about to overwrite a valid slot.
        if self.count == MAX_ENTRIES {
            self.dropped = self.dropped.saturating_add(1);
        }

        let entry = AuditEntry {
            seq,
            event,
            result,
            pid,
            uid,
            detail: payload,
            detail_offset,
            detail_len,
        };

        self.entries[self.head] = Some(entry);
        self.head = (self.head + 1) % MAX_ENTRIES;
        if self.count < MAX_ENTRIES {
            self.count += 1;
        }

        // Emit denials to serial immediately so they cannot be hidden.
        if result == AuditResult::Deny {
            serial_println!(
                "  [audit] DENIED {:?} seq={} pid={} uid={}",
                event,
                seq,
                pid,
                uid
            );
        }
    }

    /// Return the total number of valid entries.
    pub fn entry_count(&self) -> usize {
        self.count
    }

    /// Return the total sequence count (including overwritten entries).
    pub fn total_events(&self) -> u64 {
        self.seq
    }

    /// Retrieve the most recent `n` entries (up to `count`).
    ///
    /// Entries are returned newest-first.
    pub fn recent(&self, n: usize) -> alloc::vec::Vec<&AuditEntry> {
        let take = if n > self.count { self.count } else { n };
        let mut result = alloc::vec::Vec::with_capacity(take);

        // Walk backwards from the last-written slot.
        let mut idx = if self.head == 0 {
            MAX_ENTRIES - 1
        } else {
            self.head - 1
        };
        for _ in 0..take {
            if let Some(ref e) = self.entries[idx] {
                result.push(e);
            }
            if idx == 0 {
                idx = MAX_ENTRIES - 1;
            } else {
                idx -= 1;
            }
        }
        result
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize the audit log.
pub fn init() {
    serial_println!(
        "  [audit] Security audit log initialized (ring: {} entries, detail: 16384 B)",
        MAX_ENTRIES
    );
}

/// Log a free-form security event (backwards-compatible API).
pub fn log(event: AuditEvent, result: AuditResult, pid: u32, uid: u32, detail: &str) {
    AUDIT_LOG.lock().log(event, result, pid, uid, detail);
}

/// Log a syscall entry event.
pub fn log_syscall_entry(pid: u32, nr: u32, args: [u64; 6]) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::SyscallEntry,
        AuditResult::Info,
        pid,
        0,
        AuditDetail::Syscall { nr, args, ret: 0 },
    );
}

/// Log a syscall exit event.
pub fn log_syscall_exit(pid: u32, nr: u32, ret: i64) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::SyscallExit,
        AuditResult::Info,
        pid,
        0,
        AuditDetail::Syscall {
            nr,
            args: [0u64; 6],
            ret,
        },
    );
}

/// Log a file-open event.
pub fn log_file_open(pid: u32, inode: u64, flags: u32) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::FileOpen,
        AuditResult::Info,
        pid,
        0,
        AuditDetail::FileOpen { inode, flags },
    );
}

/// Log a network connection event.
pub fn log_network_connect(pid: u32, dst_ip: u32, dst_port: u16) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::NetworkConn,
        AuditResult::Info,
        pid,
        0,
        AuditDetail::Network { dst_ip, dst_port },
    );
}

/// Log a capability check.
pub fn log_cap_check(pid: u32, cap: u64, granted: bool) {
    let result = if granted {
        AuditResult::Allow
    } else {
        AuditResult::Deny
    };
    let event = if granted {
        AuditEvent::CapCheck
    } else {
        AuditEvent::CapDenied
    };
    AUDIT_LOG.lock().log_detail(
        event,
        result,
        pid,
        0,
        AuditDetail::Capability { cap, granted },
    );
}

/// Log a process fork event.
pub fn log_process_fork(parent_pid: u32, child_pid: u32) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::ProcessFork,
        AuditResult::Info,
        parent_pid,
        0,
        AuditDetail::Process {
            parent: parent_pid,
            child: child_pid,
            code: 0,
        },
    );
}

/// Log a process exit event.
pub fn log_process_exit(pid: u32, exit_code: i32) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::ProcessExit,
        AuditResult::Info,
        pid,
        0,
        AuditDetail::Process {
            parent: 0,
            child: pid,
            code: exit_code,
        },
    );
}

/// Log a seccomp kill event.
pub fn log_seccomp_kill(pid: u32, syscall_nr: u32) {
    AUDIT_LOG.lock().log_detail(
        AuditEvent::SeccompKill,
        AuditResult::Deny,
        pid,
        0,
        AuditDetail::Seccomp { syscall_nr },
    );
    serial_println!("  [audit] SECCOMP KILL pid={} syscall={}", pid, syscall_nr);
}

/// Return (total_events, dropped_events, buffered_count).
pub fn stats() -> (u64, u64, usize) {
    let log = AUDIT_LOG.lock();
    (log.total_events(), log.dropped, log.entry_count())
}

/// Return the N most recent audit entries (newest first).
pub fn recent_entries(n: usize) -> alloc::vec::Vec<AuditEntry> {
    AUDIT_LOG.lock().recent(n).into_iter().cloned().collect()
}

// ════════════════════════════════════════════════════════════════════════════
// Hardening audit ring — no-alloc, 1024-entry, for kernel security events
// ════════════════════════════════════════════════════════════════════════════

/// Maximum entries in the hardening security event ring.
const HARDEN_RING_SIZE: usize = 1024;

/// Rich security event types emitted by hardening mitigations.
///
/// Each variant carries all fields inline (no heap, no pointers outside the
/// struct).  `#[derive(Copy, Clone)]` is required for use in static arrays.
#[derive(Debug, Clone, Copy)]
pub enum HardenEvent {
    /// A syscall was blocked by seccomp or a syscall-filter policy.
    SyscallBlocked {
        pid: u32,
        syscall: u32,
        seccomp_action: u8,
    },
    /// A UID/GID privilege transition was detected.
    PrivilegeEscalation {
        pid: u32,
        from_uid: u32,
        to_uid: u32,
    },
    /// Kernel stack overflow detected (canary mismatch or guard-page fault).
    StackOverflow { pid: u32, rsp: u64, stack_top: u64 },
    /// A null-pointer dereference page fault occurred.
    NullDeref { pid: u32, fault_addr: u64 },
    /// Suspected kernel exploit: unexpected #PF at a kernel RIP.
    KernelExploit { pid: u32, fault_addr: u64, rip: u64 },
    /// A kernel module failed signature verification.
    SignatureFail {
        /// Module name, zero-terminated, up to 31 usable bytes.
        module_name: [u8; 32],
    },
    /// An authentication attempt failed.
    AuthFail { uid: u32, reason: u8 },
    /// A capability check was denied.
    CapabilityDenied { pid: u32, cap: u8 },
}

/// A single hardening ring entry.
#[derive(Clone, Copy)]
struct HardenEntry {
    /// Monotonic sequence number (never zero after first event).
    seq: u64,
    /// The event payload.
    event: HardenEvent,
}

/// No-alloc ring buffer for hardening events.
struct AuditRingBuffer {
    slots: [Option<HardenEntry>; HARDEN_RING_SIZE],
    head: usize,
    count: usize,
    seq: u64,
}

impl AuditRingBuffer {
    const fn new() -> Self {
        AuditRingBuffer {
            slots: [None; HARDEN_RING_SIZE],
            head: 0,
            count: 0,
            seq: 0,
        }
    }

    fn push(&mut self, event: HardenEvent) {
        self.seq = self.seq.saturating_add(1);
        let entry = HardenEntry {
            seq: self.seq,
            event,
        };
        self.slots[self.head] = Some(entry);
        self.head = (self.head + 1) % HARDEN_RING_SIZE;
        if self.count < HARDEN_RING_SIZE {
            self.count = self.count.saturating_add(1);
        }
    }

    /// Drain up to `max` events into `out` (oldest-first).
    /// Returns the number of events written.
    fn drain(&self, out: &mut [HardenEvent], max: usize) -> usize {
        let take = if max < self.count { max } else { self.count };
        if take == 0 {
            return 0;
        }
        // Read oldest first: start from (head - count) mod ring.
        let start = (self.head + HARDEN_RING_SIZE - self.count) % HARDEN_RING_SIZE;
        let mut written = 0usize;
        for i in 0..take {
            let idx = (start + i) % HARDEN_RING_SIZE;
            if let Some(ref e) = self.slots[idx] {
                if written < out.len() {
                    out[written] = e.event;
                    written = written.saturating_add(1);
                }
            }
        }
        written
    }
}

// Safety: single-writer enforced by Mutex.
unsafe impl Send for AuditRingBuffer {}

/// The hardening security event ring (no-alloc, 1024 slots).
static HARDEN_LOG: crate::sync::Mutex<AuditRingBuffer> =
    crate::sync::Mutex::new(AuditRingBuffer::new());

// ── Public hardening audit API ────────────────────────────────────────────────

/// Record a hardening security event into the no-alloc ring.
///
/// Also emits a brief serial line for events that indicate an active attack
/// (StackOverflow, KernelExploit, PrivilegeEscalation).
pub fn audit_log(event: HardenEvent) {
    // Emit to serial for high-severity events before acquiring the lock so
    // the message appears even if the lock is contended.
    match event {
        HardenEvent::StackOverflow { pid, rsp, .. } => {
            serial_println!("  [audit-harden] STACK OVERFLOW pid={} rsp={:#x}", pid, rsp)
        }
        HardenEvent::KernelExploit {
            pid,
            fault_addr,
            rip,
        } => serial_println!(
            "  [audit-harden] KERNEL EXPLOIT pid={} fault={:#x} rip={:#x}",
            pid,
            fault_addr,
            rip
        ),
        HardenEvent::PrivilegeEscalation {
            pid,
            from_uid,
            to_uid,
        } => serial_println!(
            "  [audit-harden] PRIV ESCALATION pid={} {} -> {}",
            pid,
            from_uid,
            to_uid
        ),
        HardenEvent::SignatureFail { module_name } => {
            // Find null terminator for display length.
            let len = module_name.iter().position(|&b| b == 0).unwrap_or(32);
            serial_println!("  [audit-harden] SIGNATURE FAIL module=[{} bytes]", len);
        }
        _ => {}
    }
    HARDEN_LOG.lock().push(event);
}

/// Copy up to `max` hardening events (oldest-first) into `out`.
/// Returns the number of events written.
pub fn audit_read(out: &mut [HardenEvent], max: usize) -> usize {
    HARDEN_LOG.lock().drain(out, max)
}

/// Return the number of hardening events currently stored in the ring.
pub fn audit_count() -> usize {
    HARDEN_LOG.lock().count
}
