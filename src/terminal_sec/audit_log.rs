use crate::sync::Mutex;
/// Terminal session audit logging for Genesis OS
///
/// Records every command execution, session lifecycle event, login attempt,
/// and security-relevant action for forensic analysis. Uses a hash chain
/// for tamper detection so that log entries cannot be silently modified.
///
/// Ring buffer holds up to 100,000 events with automatic rotation.
/// Anomaly detection flags rapid-fire commands, unusual-hour activity,
/// and repeated authentication failures.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_EVENTS: usize = 4_096; // was 100_000 — 8MB alloc fragmented heap at boot
const MAX_SESSIONS: usize = 1024;
const HASH_SEED: u64 = 0xA1B2C3D4E5F60718;

/// Rapid-fire threshold: more than this many commands in RAPID_WINDOW_MS
/// from the same uid triggers an anomaly.
const RAPID_FIRE_THRESHOLD: u32 = 30;
const RAPID_WINDOW_MS: u64 = 5000;

/// Login failures from one uid within FAIL_WINDOW_MS that trigger an alert.
const FAIL_THRESHOLD: u32 = 5;
const FAIL_WINDOW_MS: u64 = 60_000;

/// Hours considered "unusual" (0-based, 24h). 0..=4 means midnight-04:59.
const UNUSUAL_HOUR_START: u64 = 0;
const UNUSUAL_HOUR_END: u64 = 4;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventType {
    Login,
    Logout,
    LoginFailed,
    CommandExec,
    CommandFailed,
    FileAccess,
    FileModify,
    FileDelete,
    PrivEscalation,
    PrivDenied,
    SessionStart,
    SessionEnd,
    NetworkAccess,
    ConfigChange,
    ServiceStart,
    ServiceStop,
    SuspiciousActivity,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct AuditEvent {
    pub id: u64,
    pub event_type: AuditEventType,
    pub uid: u32,
    pub pid: u32,
    pub tty_hash: u64,
    pub command_hash: u64,
    pub args_hash: u64,
    pub cwd_hash: u64,
    pub timestamp: u64,
    pub exit_code: i32,
    pub duration_ms: u32,
    pub risk_level: u8,
    /// Hash of the previous event for tamper detection chain.
    chain_hash: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditSession {
    pub session_id: u64,
    pub uid: u32,
    pub login_time: u64,
    pub logout_time: u64,
    pub tty_hash: u64,
    pub ip_hash: u64,
    pub commands_count: u32,
    pub violations_count: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditFilter {
    pub uid: Option<u32>,
    pub event_type: Option<AuditEventType>,
    pub time_start: u64,
    pub time_end: u64,
    pub min_risk: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct AlertThreshold {
    pub event_type: AuditEventType,
    pub max_count: u32,
    pub window_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditStatistics {
    pub total_events: u64,
    pub total_sessions: u64,
    pub active_sessions: u32,
    pub login_failures: u64,
    pub commands_executed: u64,
    pub privilege_escalations: u64,
    pub suspicious_events: u64,
    pub anomalies_detected: u64,
    pub log_rotations: u64,
}

// ---------------------------------------------------------------------------
// AuditEngine
// ---------------------------------------------------------------------------

pub struct AuditEngine {
    /// Ring buffer for events.
    event_log: Vec<Option<AuditEvent>>,
    /// Write head into event_log.
    head: usize,
    /// Total events ever written (monotonic, never reset by rotation).
    total_written: u64,
    /// Next event id.
    next_id: u64,
    /// Last chain hash for tamper-protection linking.
    last_chain_hash: u64,

    /// Session table.
    sessions: Vec<AuditSession>,
    session_count: usize,
    next_session_id: u64,

    /// Configurable alert thresholds.
    alert_thresholds: Vec<AlertThreshold>,

    /// Statistics counters.
    stats: AuditStatistics,
}

impl AuditEngine {
    pub const fn new() -> Self {
        AuditEngine {
            event_log: Vec::new(),
            head: 0,
            total_written: 0,
            next_id: 1,
            last_chain_hash: HASH_SEED,
            sessions: Vec::new(),
            session_count: 0,
            next_session_id: 1,
            alert_thresholds: Vec::new(),
            stats: AuditStatistics {
                total_events: 0,
                total_sessions: 0,
                active_sessions: 0,
                login_failures: 0,
                commands_executed: 0,
                privilege_escalations: 0,
                suspicious_events: 0,
                anomalies_detected: 0,
                log_rotations: 0,
            },
        }
    }

    /// Lazily allocate the ring buffer on first use.
    fn ensure_allocated(&mut self) {
        if self.event_log.is_empty() {
            self.event_log = vec![None; MAX_EVENTS];
        }
        if self.sessions.is_empty() {
            self.sessions = vec![
                AuditSession {
                    session_id: 0,
                    uid: 0,
                    login_time: 0,
                    logout_time: 0,
                    tty_hash: 0,
                    ip_hash: 0,
                    commands_count: 0,
                    violations_count: 0,
                };
                MAX_SESSIONS
            ];
        }
    }

    // ------------------------------------------------------------------
    // Hash helpers (FNV-1a inspired, no floats, pure integer)
    // ------------------------------------------------------------------

    fn hash_event(ev: &AuditEvent, prev_hash: u64) -> u64 {
        let mut h: u64 = prev_hash ^ 0xCBF29CE484222325;
        let mix = |h: &mut u64, v: u64| {
            *h ^= v;
            *h = h.wrapping_mul(0x100000001B3);
        };
        mix(&mut h, ev.id);
        mix(&mut h, ev.event_type as u64);
        mix(&mut h, ev.uid as u64);
        mix(&mut h, ev.pid as u64);
        mix(&mut h, ev.tty_hash);
        mix(&mut h, ev.command_hash);
        mix(&mut h, ev.args_hash);
        mix(&mut h, ev.cwd_hash);
        mix(&mut h, ev.timestamp);
        mix(&mut h, ev.exit_code as u64);
        mix(&mut h, ev.duration_ms as u64);
        mix(&mut h, ev.risk_level as u64);
        h
    }

    // ------------------------------------------------------------------
    // Core logging
    // ------------------------------------------------------------------

    /// Log a fully-specified audit event. Returns the assigned event id.
    pub fn log_event(
        &mut self,
        event_type: AuditEventType,
        uid: u32,
        pid: u32,
        tty_hash: u64,
        command_hash: u64,
        args_hash: u64,
        cwd_hash: u64,
        timestamp: u64,
        exit_code: i32,
        duration_ms: u32,
        risk_level: u8,
    ) -> u64 {
        self.ensure_allocated();

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let mut event = AuditEvent {
            id,
            event_type,
            uid,
            pid,
            tty_hash,
            command_hash,
            args_hash,
            cwd_hash,
            timestamp,
            exit_code,
            duration_ms,
            risk_level,
            chain_hash: 0,
        };

        // Compute chain hash linking to previous entry.
        let chain = Self::hash_event(&event, self.last_chain_hash);
        event.chain_hash = chain;
        self.last_chain_hash = chain;

        // Write into ring buffer.
        self.event_log[self.head] = Some(event);
        self.head = (self.head + 1) % MAX_EVENTS;
        self.total_written = self.total_written.saturating_add(1);

        // Update statistics.
        self.stats.total_events = self.stats.total_events.saturating_add(1);
        match event_type {
            AuditEventType::LoginFailed => {
                self.stats.login_failures = self.stats.login_failures.saturating_add(1);
            }
            AuditEventType::CommandExec => {
                self.stats.commands_executed = self.stats.commands_executed.saturating_add(1);
            }
            AuditEventType::CommandFailed => {
                self.stats.commands_executed = self.stats.commands_executed.saturating_add(1);
            }
            AuditEventType::PrivEscalation => {
                self.stats.privilege_escalations =
                    self.stats.privilege_escalations.saturating_add(1);
            }
            AuditEventType::SuspiciousActivity => {
                self.stats.suspicious_events = self.stats.suspicious_events.saturating_add(1);
            }
            _ => {}
        }

        // High-risk events get a serial line.
        if risk_level >= 7 {
            serial_println!(
                "  [audit_log] HIGH RISK (lvl {}) event {:?} uid={} pid={} id={}",
                risk_level,
                event_type,
                uid,
                pid,
                id
            );
        }

        id
    }

    /// Convenience: log a command execution event.
    pub fn log_command(
        &mut self,
        uid: u32,
        pid: u32,
        tty_hash: u64,
        command_hash: u64,
        args_hash: u64,
        cwd_hash: u64,
        timestamp: u64,
        exit_code: i32,
        duration_ms: u32,
    ) -> u64 {
        let event_type = if exit_code == 0 {
            AuditEventType::CommandExec
        } else {
            AuditEventType::CommandFailed
        };

        // Assign risk based on exit code and duration.
        let risk = if exit_code != 0 { 3 } else { 1 };

        let id = self.log_event(
            event_type,
            uid,
            pid,
            tty_hash,
            command_hash,
            args_hash,
            cwd_hash,
            timestamp,
            exit_code,
            duration_ms,
            risk,
        );

        // Increment session command count if the user has an active session.
        for i in 0..self.session_count {
            if self.sessions[i].uid == uid && self.sessions[i].logout_time == 0 {
                self.sessions[i].commands_count = self.sessions[i].commands_count.saturating_add(1);
                break;
            }
        }

        id
    }

    // ------------------------------------------------------------------
    // Session management
    // ------------------------------------------------------------------

    /// Start a new audit session. Returns the session id.
    pub fn start_session(&mut self, uid: u32, tty_hash: u64, ip_hash: u64, timestamp: u64) -> u64 {
        self.ensure_allocated();

        let sid = self.next_session_id;
        self.next_session_id = self.next_session_id.saturating_add(1);

        let session = AuditSession {
            session_id: sid,
            uid,
            login_time: timestamp,
            logout_time: 0,
            tty_hash,
            ip_hash,
            commands_count: 0,
            violations_count: 0,
        };

        if self.session_count < MAX_SESSIONS {
            self.sessions[self.session_count] = session;
            self.session_count += 1;
        } else {
            // Overwrite oldest completed session, or slot 0 as fallback.
            let mut slot = 0;
            for i in 0..MAX_SESSIONS {
                if self.sessions[i].logout_time != 0 {
                    slot = i;
                    break;
                }
            }
            self.sessions[slot] = session;
        }

        self.stats.total_sessions = self.stats.total_sessions.saturating_add(1);
        self.stats.active_sessions = self.stats.active_sessions.saturating_add(1);

        // Also emit a SessionStart event.
        self.log_event(
            AuditEventType::SessionStart,
            uid,
            0,
            tty_hash,
            0,
            0,
            0,
            timestamp,
            0,
            0,
            1,
        );

        serial_println!("  [audit_log] Session {} started for uid={}", sid, uid);
        sid
    }

    /// End an active session by session_id.
    pub fn end_session(&mut self, session_id: u64, timestamp: u64) -> bool {
        for i in 0..self.session_count {
            if self.sessions[i].session_id == session_id && self.sessions[i].logout_time == 0 {
                self.sessions[i].logout_time = timestamp;
                if self.stats.active_sessions > 0 {
                    self.stats.active_sessions -= 1;
                }

                let uid = self.sessions[i].uid;
                let tty_hash = self.sessions[i].tty_hash;
                self.log_event(
                    AuditEventType::SessionEnd,
                    uid,
                    0,
                    tty_hash,
                    0,
                    0,
                    0,
                    timestamp,
                    0,
                    0,
                    1,
                );

                serial_println!(
                    "  [audit_log] Session {} ended for uid={} (cmds={}, violations={})",
                    session_id,
                    uid,
                    self.sessions[i].commands_count,
                    self.sessions[i].violations_count
                );
                return true;
            }
        }
        false
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Query events matching a filter. Returns up to `limit` events.
    pub fn query_events(&self, filter: &AuditFilter, limit: usize) -> Vec<AuditEvent> {
        let mut results = Vec::new();
        if self.event_log.is_empty() {
            return results;
        }

        let cap = self.event_log.len();
        let total = if self.total_written < cap as u64 {
            self.total_written as usize
        } else {
            cap
        };

        // Walk backwards from head so newest events come first.
        for offset in 0..total {
            if results.len() >= limit {
                break;
            }
            let idx = if self.head == 0 {
                cap - 1 - offset
            } else {
                (self.head + cap - 1 - offset) % cap
            };
            if let Some(ref ev) = self.event_log[idx] {
                if self.matches_filter(ev, filter) {
                    results.push(*ev);
                }
            }
        }
        results
    }

    /// Query events for a specific user.
    pub fn query_by_user(&self, uid: u32, limit: usize) -> Vec<AuditEvent> {
        let filter = AuditFilter {
            uid: Some(uid),
            event_type: None,
            time_start: 0,
            time_end: u64::MAX,
            min_risk: 0,
        };
        self.query_events(&filter, limit)
    }

    /// Get a session by id.
    pub fn get_session(&self, session_id: u64) -> Option<AuditSession> {
        for i in 0..self.session_count {
            if self.sessions[i].session_id == session_id {
                return Some(self.sessions[i]);
            }
        }
        None
    }

    /// Get all currently active (not yet ended) sessions.
    pub fn get_active_sessions(&self) -> Vec<AuditSession> {
        let mut active = Vec::new();
        for i in 0..self.session_count {
            if self.sessions[i].logout_time == 0 && self.sessions[i].session_id != 0 {
                active.push(self.sessions[i]);
            }
        }
        active
    }

    fn matches_filter(&self, ev: &AuditEvent, f: &AuditFilter) -> bool {
        if let Some(uid) = f.uid {
            if ev.uid != uid {
                return false;
            }
        }
        if let Some(et) = f.event_type {
            if ev.event_type != et {
                return false;
            }
        }
        if ev.timestamp < f.time_start || ev.timestamp > f.time_end {
            return false;
        }
        if ev.risk_level < f.min_risk {
            return false;
        }
        true
    }

    // ------------------------------------------------------------------
    // Anomaly detection
    // ------------------------------------------------------------------

    /// Detect anomalies for a given uid at `now_ms` timestamp.
    /// Returns a risk score 0-10.  0 = nothing suspicious.
    pub fn detect_anomaly(&self, uid: u32, now_ms: u64) -> u8 {
        if self.event_log.is_empty() {
            return 0;
        }

        let mut risk: u8 = 0;

        // --- Rapid-fire command detection ---
        let window_start = now_ms.saturating_sub(RAPID_WINDOW_MS);
        let mut cmd_count: u32 = 0;

        let cap = self.event_log.len();
        let total = if self.total_written < cap as u64 {
            self.total_written as usize
        } else {
            cap
        };

        let mut fail_count: u32 = 0;
        let fail_window_start = now_ms.saturating_sub(FAIL_WINDOW_MS);

        for offset in 0..total {
            let idx = (self.head + cap - 1 - offset) % cap;
            if let Some(ref ev) = self.event_log[idx] {
                if ev.uid != uid {
                    continue;
                }

                // Stop scanning once we're well past both windows.
                if ev.timestamp < fail_window_start && ev.timestamp < window_start {
                    break;
                }

                // Rapid-fire check.
                if ev.timestamp >= window_start {
                    match ev.event_type {
                        AuditEventType::CommandExec | AuditEventType::CommandFailed => {
                            cmd_count += 1;
                        }
                        _ => {}
                    }
                }

                // Repeated login failure check.
                if ev.timestamp >= fail_window_start && ev.event_type == AuditEventType::LoginFailed
                {
                    fail_count += 1;
                }
            }
        }

        if cmd_count > RAPID_FIRE_THRESHOLD {
            risk = risk.saturating_add(4);
            serial_println!(
                "  [audit_log] ANOMALY: rapid-fire commands uid={} count={} in {}ms",
                uid,
                cmd_count,
                RAPID_WINDOW_MS
            );
        }

        if fail_count >= FAIL_THRESHOLD {
            risk = risk.saturating_add(5);
            serial_println!(
                "  [audit_log] ANOMALY: repeated login failures uid={} count={} in {}ms",
                uid,
                fail_count,
                FAIL_WINDOW_MS
            );
        }

        // --- Unusual hour detection ---
        // Convert timestamp (ms since boot) into approximate hour-of-day.
        // In a real kernel we would use RTC; here we use a simple modular estimate.
        let seconds = now_ms / 1000;
        let hour_of_day = (seconds / 3600) % 24;
        if hour_of_day >= UNUSUAL_HOUR_START && hour_of_day <= UNUSUAL_HOUR_END {
            risk = risk.saturating_add(2);
        }

        // --- Check custom alert thresholds ---
        for threshold in &self.alert_thresholds {
            let tw_start = now_ms.saturating_sub(threshold.window_ms);
            let mut count: u32 = 0;
            for offset in 0..total {
                let idx = (self.head + cap - 1 - offset) % cap;
                if let Some(ref ev) = self.event_log[idx] {
                    if ev.timestamp < tw_start {
                        break;
                    }
                    if ev.uid == uid && ev.event_type == threshold.event_type {
                        count += 1;
                    }
                }
            }
            if count > threshold.max_count {
                risk = risk.saturating_add(3);
            }
        }

        if risk > 0 {
            self.stats_mut_anomaly_inc();
        }

        if risk > 10 {
            10
        } else {
            risk
        }
    }

    /// Ugly but necessary: we need to bump anomaly counter without &mut self
    /// in detect_anomaly. We do it best-effort through a separate call path.
    fn stats_mut_anomaly_inc(&self) {
        // In a real implementation this would use an atomic counter.
        // Since we are behind a Mutex anyway, the caller holds &mut through
        // the public wrappers that call detect_anomaly_mut.
    }

    /// Mutable version that also increments the anomaly counter.
    pub fn detect_anomaly_mut(&mut self, uid: u32, now_ms: u64) -> u8 {
        let risk = self.detect_anomaly(uid, now_ms);
        if risk > 0 {
            self.stats.anomalies_detected = self.stats.anomalies_detected.saturating_add(1);
        }
        risk
    }

    // ------------------------------------------------------------------
    // Integrity verification (hash chain)
    // ------------------------------------------------------------------

    /// Verify the hash chain of logged events.
    /// Returns (verified_count, first_bad_index) -- None means chain is intact.
    pub fn verify_integrity(&self) -> (u64, Option<usize>) {
        if self.event_log.is_empty() {
            return (0, None);
        }

        let cap = self.event_log.len();
        let total = if self.total_written < cap as u64 {
            self.total_written as usize
        } else {
            cap
        };

        // Find the oldest entry by walking forward from head (oldest is at head
        // when the buffer has wrapped, or at 0 if it hasn't).
        let start = if self.total_written > cap as u64 {
            self.head
        } else {
            0
        };

        let mut prev_hash = HASH_SEED;
        let mut verified: u64 = 0;

        for i in 0..total {
            let idx = (start + i) % cap;
            if let Some(ref ev) = self.event_log[idx] {
                let expected = Self::hash_event(ev, prev_hash);
                if ev.chain_hash != expected {
                    serial_println!(
                        "  [audit_log] INTEGRITY FAIL at ring index {} (event id={})",
                        idx,
                        ev.id
                    );
                    return (verified, Some(idx));
                }
                prev_hash = ev.chain_hash;
                verified += 1;
            }
        }

        serial_println!("  [audit_log] Integrity OK: {} events verified", verified);
        (verified, None)
    }

    // ------------------------------------------------------------------
    // Export and rotation
    // ------------------------------------------------------------------

    /// Export log entries matching a filter into a Vec.
    pub fn export_log(&self, filter: &AuditFilter, limit: usize) -> Vec<AuditEvent> {
        self.query_events(filter, limit)
    }

    /// Rotate the log: clears old entries, resets chain.
    /// Returns the number of events that were discarded.
    pub fn rotate_log(&mut self) -> u64 {
        let discarded = self.total_written;

        // Reset the ring buffer.
        for slot in self.event_log.iter_mut() {
            *slot = None;
        }
        self.head = 0;
        self.total_written = 0;
        self.last_chain_hash = HASH_SEED;
        // next_id intentionally NOT reset -- ids are globally monotonic.

        self.stats.log_rotations = self.stats.log_rotations.saturating_add(1);

        serial_println!("  [audit_log] Log rotated, {} events discarded", discarded);
        discarded
    }

    // ------------------------------------------------------------------
    // Alert thresholds
    // ------------------------------------------------------------------

    /// Add or update an alert threshold for an event type.
    pub fn set_alert_threshold(
        &mut self,
        event_type: AuditEventType,
        max_count: u32,
        window_ms: u64,
    ) {
        // Update existing if present.
        for t in self.alert_thresholds.iter_mut() {
            if t.event_type == event_type {
                t.max_count = max_count;
                t.window_ms = window_ms;
                return;
            }
        }
        self.alert_thresholds.push(AlertThreshold {
            event_type,
            max_count,
            window_ms,
        });
    }

    // ------------------------------------------------------------------
    // Statistics
    // ------------------------------------------------------------------

    /// Return a snapshot of audit statistics.
    pub fn get_statistics(&self) -> AuditStatistics {
        self.stats
    }
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

static AUDIT_ENGINE: Mutex<Option<AuditEngine>> = Mutex::new(None);

fn with_engine<F, R>(f: F) -> R
where
    F: FnOnce(&mut AuditEngine) -> R,
{
    let mut guard = AUDIT_ENGINE.lock();
    let engine = guard.as_mut().expect("audit_log not initialized");
    f(engine)
}

// ---------------------------------------------------------------------------
// Public API (module-level convenience functions)
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = AUDIT_ENGINE.lock();
    *guard = Some(AuditEngine::new());
    // Force allocation now so we do not stall on the first event.
    if let Some(ref mut engine) = *guard {
        engine.ensure_allocated();
    }
    serial_println!(
        "    [audit_log] Terminal audit logging initialized (ring buffer: {} events)",
        MAX_EVENTS
    );
}

pub fn log_event(
    event_type: AuditEventType,
    uid: u32,
    pid: u32,
    tty_hash: u64,
    command_hash: u64,
    args_hash: u64,
    cwd_hash: u64,
    timestamp: u64,
    exit_code: i32,
    duration_ms: u32,
    risk_level: u8,
) -> u64 {
    with_engine(|e| {
        e.log_event(
            event_type,
            uid,
            pid,
            tty_hash,
            command_hash,
            args_hash,
            cwd_hash,
            timestamp,
            exit_code,
            duration_ms,
            risk_level,
        )
    })
}

pub fn log_command(
    uid: u32,
    pid: u32,
    tty_hash: u64,
    command_hash: u64,
    args_hash: u64,
    cwd_hash: u64,
    timestamp: u64,
    exit_code: i32,
    duration_ms: u32,
) -> u64 {
    with_engine(|e| {
        e.log_command(
            uid,
            pid,
            tty_hash,
            command_hash,
            args_hash,
            cwd_hash,
            timestamp,
            exit_code,
            duration_ms,
        )
    })
}

pub fn start_session(uid: u32, tty_hash: u64, ip_hash: u64, timestamp: u64) -> u64 {
    with_engine(|e| e.start_session(uid, tty_hash, ip_hash, timestamp))
}

pub fn end_session(session_id: u64, timestamp: u64) -> bool {
    with_engine(|e| e.end_session(session_id, timestamp))
}

pub fn query_events(filter: &AuditFilter, limit: usize) -> Vec<AuditEvent> {
    with_engine(|e| e.query_events(filter, limit))
}

pub fn query_by_user(uid: u32, limit: usize) -> Vec<AuditEvent> {
    with_engine(|e| e.query_by_user(uid, limit))
}

pub fn get_session(session_id: u64) -> Option<AuditSession> {
    with_engine(|e| e.get_session(session_id))
}

pub fn get_active_sessions() -> Vec<AuditSession> {
    with_engine(|e| e.get_active_sessions())
}

pub fn detect_anomaly(uid: u32, now_ms: u64) -> u8 {
    with_engine(|e| e.detect_anomaly_mut(uid, now_ms))
}

pub fn export_log(filter: &AuditFilter, limit: usize) -> Vec<AuditEvent> {
    with_engine(|e| e.export_log(filter, limit))
}

pub fn verify_integrity() -> (u64, Option<usize>) {
    with_engine(|e| e.verify_integrity())
}

pub fn set_alert_threshold(event_type: AuditEventType, max_count: u32, window_ms: u64) {
    with_engine(|e| e.set_alert_threshold(event_type, max_count, window_ms))
}

pub fn get_statistics() -> AuditStatistics {
    with_engine(|e| e.get_statistics())
}

pub fn rotate_log() -> u64 {
    with_engine(|e| e.rotate_log())
}
