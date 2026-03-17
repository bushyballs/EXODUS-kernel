/// audit — Linux-compatible kernel audit subsystem
///
/// Provides an in-kernel ring buffer of structured audit records.
/// Records are emitted by syscall entry/exit, file access checks,
/// authentication events, and security policy decisions.
///
/// Ring buffer: 512 fixed-size records (power-of-two, lock-protected).
/// Consumer reads records via audit_read() and drains them.
///
/// Inspired by: Linux audit(8) / linux/audit.h. All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants (match Linux audit.h)
// ---------------------------------------------------------------------------

pub const AUDIT_SYSCALL: u32 = 1300;
pub const AUDIT_PATH: u32 = 1302;
pub const AUDIT_IPC: u32 = 1303;
pub const AUDIT_SOCKADDR: u32 = 1306;
pub const AUDIT_EXECVE: u32 = 1309;
pub const AUDIT_USER_AUTH: u32 = 1100;
pub const AUDIT_USER_ACCT: u32 = 1101;
pub const AUDIT_LOGIN: u32 = 1006;
pub const AUDIT_KERN_MODULE: u32 = 1323;
pub const AUDIT_ANOM_ABEND: u32 = 1701; // abnormal termination

// Audit return codes
pub const AUDIT_SUCCESS: i8 = 1;
pub const AUDIT_FAILURE: i8 = -1;

// Ring buffer capacity (must be power of two)
const RING_SIZE: usize = 512;
const RING_MASK: usize = RING_SIZE - 1;

// Max bytes in the text field of a record
const MSG_MAX: usize = 256;

// ---------------------------------------------------------------------------
// AuditRecord
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct AuditRecord {
    pub serial: u64,   // monotonically increasing sequence number
    pub ts_sec: u64,   // timestamp (coarse seconds since boot)
    pub rec_type: u32, // AUDIT_* record type
    pub pid: u32,      // originating process
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub result: i8, // AUDIT_SUCCESS / AUDIT_FAILURE / 0
    pub msg: [u8; MSG_MAX],
    pub msg_len: u16,
    pub valid: bool,
}

impl AuditRecord {
    pub const fn empty() -> Self {
        AuditRecord {
            serial: 0,
            ts_sec: 0,
            rec_type: 0,
            pid: 0,
            uid: 0,
            gid: 0,
            euid: 0,
            result: 0,
            msg: [0u8; MSG_MAX],
            msg_len: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

struct AuditRing {
    records: [AuditRecord; RING_SIZE],
    head: usize,  // next write position
    count: usize, // unread records
}

impl AuditRing {
    const fn new() -> Self {
        const EMPTY: AuditRecord = AuditRecord::empty();
        AuditRing {
            records: [EMPTY; RING_SIZE],
            head: 0,
            count: 0,
        }
    }

    fn push(&mut self, rec: AuditRecord) {
        self.records[self.head & RING_MASK] = rec;
        self.head = self.head.wrapping_add(1);
        if self.count < RING_SIZE {
            self.count = self.count.saturating_add(1);
        }
        // If count == RING_SIZE the oldest record was silently overwritten.
    }

    /// Dequeue up to `n` records into `out`. Returns count dequeued.
    fn drain(&mut self, out: &mut [AuditRecord; 16]) -> usize {
        let tail = self.head.wrapping_sub(self.count);
        let to_read = self.count.min(16);
        let mut i = 0usize;
        while i < to_read {
            out[i] = self.records[(tail.wrapping_add(i)) & RING_MASK];
            i = i.saturating_add(1);
        }
        self.count = self.count.saturating_sub(to_read);
        to_read
    }
}

static AUDIT_RING: Mutex<AuditRing> = Mutex::new(AuditRing::new());
static AUDIT_SERIAL: AtomicU64 = AtomicU64::new(1);

// Audit enable/disable flag (1 = enabled)
static AUDIT_ENABLED: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);

// ---------------------------------------------------------------------------
// Helper: copy a byte slice into the msg field
// ---------------------------------------------------------------------------

fn copy_msg(dst: &mut [u8; MSG_MAX], src: &[u8]) -> u16 {
    let len = src.len().min(MSG_MAX - 1);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Emit an audit record. No-op if auditing is disabled.
pub fn audit_log(
    rec_type: u32,
    pid: u32,
    uid: u32,
    gid: u32,
    euid: u32,
    result: i8,
    msg: &[u8],
    ts_sec: u64,
) {
    if AUDIT_ENABLED.load(Ordering::Relaxed) == 0 {
        return;
    }
    let serial = AUDIT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let mut rec = AuditRecord::empty();
    rec.serial = serial;
    rec.ts_sec = ts_sec;
    rec.rec_type = rec_type;
    rec.pid = pid;
    rec.uid = uid;
    rec.gid = gid;
    rec.euid = euid;
    rec.result = result;
    rec.msg_len = copy_msg(&mut rec.msg, msg);
    rec.valid = true;
    AUDIT_RING.lock().push(rec);
}

/// Convenience: log a syscall audit record.
pub fn audit_syscall(pid: u32, uid: u32, syscall_nr: u32, result: i8, ts_sec: u64) {
    // Build a minimal "syscall=NNN pid=PID uid=UID res=success/failed" string
    let mut buf = [0u8; 64];
    let mut off = 0usize;
    // "syscall=" followed by decimal syscall_nr
    let prefix = b"syscall=";
    while off < prefix.len() {
        buf[off] = prefix[off];
        off = off.saturating_add(1);
    }
    off = write_decimal(&mut buf, off, syscall_nr as u64);
    buf[off] = b' ';
    off = off.saturating_add(1);
    let pid_pfx = b"pid=";
    let mut k = 0usize;
    while k < pid_pfx.len() {
        buf[off] = pid_pfx[k];
        off = off.saturating_add(1);
        k = k.saturating_add(1);
    }
    off = write_decimal(&mut buf, off, pid as u64);
    audit_log(AUDIT_SYSCALL, pid, uid, 0, uid, result, &buf[..off], ts_sec);
}

/// Convenience: log a login event.
pub fn audit_login(pid: u32, uid: u32, success: bool, ts_sec: u64) {
    let msg = if success {
        b"op=login res=success" as &[u8]
    } else {
        b"op=login res=failed"
    };
    audit_log(
        AUDIT_LOGIN,
        pid,
        uid,
        0,
        uid,
        if success {
            AUDIT_SUCCESS
        } else {
            AUDIT_FAILURE
        },
        msg,
        ts_sec,
    );
}

/// Drain up to 16 pending audit records.
pub fn audit_read(out: &mut [AuditRecord; 16]) -> usize {
    AUDIT_RING.lock().drain(out)
}

pub fn audit_enable() {
    AUDIT_ENABLED.store(1, Ordering::Relaxed);
}
pub fn audit_disable() {
    AUDIT_ENABLED.store(0, Ordering::Relaxed);
}

pub fn audit_pending() -> usize {
    AUDIT_RING.lock().count
}

// ---------------------------------------------------------------------------
// Integer → ASCII decimal helper (no heap, no format macros)
// ---------------------------------------------------------------------------

fn write_decimal(buf: &mut [u8], off: usize, mut v: u64) -> usize {
    if v == 0 {
        if off < buf.len() {
            buf[off] = b'0';
        }
        return off.saturating_add(1);
    }
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    while v > 0 && n < 20 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n = n.saturating_add(1);
    }
    // tmp holds digits in reverse
    let mut i = 0usize;
    while i < n && off.saturating_add(i) < buf.len() {
        buf[off + i] = tmp[n - 1 - i];
        i = i.saturating_add(1);
    }
    off.saturating_add(n)
}

pub fn init() {
    serial_println!(
        "[audit] kernel audit subsystem initialized ({} record ring buffer)",
        RING_SIZE
    );
}
