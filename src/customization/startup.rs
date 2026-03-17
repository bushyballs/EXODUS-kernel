use crate::sync::Mutex;
/// Startup customization for Genesis
///
/// Autostart applications, boot services, login scripts,
/// delayed start scheduling, dependency ordering,
/// startup performance tracking.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum StartupPhase {
    EarlyBoot,
    PreLogin,
    PostLogin,
    DesktopReady,
    Idle,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EntryType {
    Application,
    Service,
    Script,
    DaemonProcess,
    ShellCommand,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EntryState {
    Pending,
    Starting,
    Running,
    Failed,
    Disabled,
    DelayWaiting,
    Completed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RestartPolicy {
    Never,
    OnFailure,
    Always,
    OncePerSession,
}

#[derive(Clone, Copy, PartialEq)]
pub enum StartCondition {
    Always,
    IfNetworkAvailable,
    IfBatteryAbove,
    IfExternalMonitor,
    IfDocked,
    IfWeekday,
    IfWeekend,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct StartupEntry {
    id: u32,
    app_id: u32,
    entry_type: EntryType,
    phase: StartupPhase,
    state: EntryState,
    delay_ms: u32,
    priority: u16,   // lower = starts earlier
    depends_on: u32, // id of entry that must complete first (0 = none)
    restart_policy: RestartPolicy,
    restart_count: u16,
    max_restarts: u16,
    condition: StartCondition,
    condition_param: u32, // e.g. battery threshold
    enabled: bool,
    hidden: bool, // hidden from user UI (system services)
    name_hash: u64,
    start_time: u64,
    elapsed_ms: u32,
    exit_code: i32,
    failure_count: u32,
}

#[derive(Clone, Copy)]
struct PerformanceRecord {
    entry_id: u32,
    boot_index: u32,
    start_ms: u32,
    duration_ms: u32,
    success: bool,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct StartupManager {
    entries: Vec<StartupEntry>,
    perf_records: Vec<PerformanceRecord>,
    next_id: u32,
    current_phase: StartupPhase,
    boot_index: u32,
    boot_start_time: u64,
    total_startup_ms: u32,
    entries_started: u32,
    entries_failed: u32,
    max_concurrent: u8,
    currently_starting: u8,
    network_available: bool,
    battery_level: u8,
    external_monitor: bool,
    docked: bool,
    is_weekday: bool,
}

static STARTUP: Mutex<Option<StartupManager>> = Mutex::new(None);

impl StartupManager {
    fn new() -> Self {
        StartupManager {
            entries: Vec::new(),
            perf_records: Vec::new(),
            next_id: 1,
            current_phase: StartupPhase::EarlyBoot,
            boot_index: 0,
            boot_start_time: 0,
            total_startup_ms: 0,
            entries_started: 0,
            entries_failed: 0,
            max_concurrent: 4,
            currently_starting: 0,
            network_available: false,
            battery_level: 100,
            external_monitor: false,
            docked: false,
            is_weekday: true,
        }
    }

    fn add_entry(
        &mut self,
        app_id: u32,
        entry_type: EntryType,
        phase: StartupPhase,
        name_hash: u64,
    ) -> u32 {
        if self.entries.len() >= 256 {
            return 0;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let entry = StartupEntry {
            id,
            app_id,
            entry_type,
            phase,
            state: EntryState::Pending,
            delay_ms: 0,
            priority: 100,
            depends_on: 0,
            restart_policy: RestartPolicy::Never,
            restart_count: 0,
            max_restarts: 3,
            condition: StartCondition::Always,
            condition_param: 0,
            enabled: true,
            hidden: false,
            name_hash,
            start_time: 0,
            elapsed_ms: 0,
            exit_code: 0,
            failure_count: 0,
        };
        self.entries.push(entry);
        id
    }

    fn remove_entry(&mut self, entry_id: u32) -> bool {
        // Don't allow removing entries that others depend on
        let has_dependents = self.entries.iter().any(|e| e.depends_on == entry_id);
        if has_dependents {
            return false;
        }

        let len_before = self.entries.len();
        self.entries.retain(|e| e.id != entry_id);
        self.entries.len() < len_before
    }

    fn set_delay(&mut self, entry_id: u32, delay_ms: u32) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.delay_ms = delay_ms.min(60_000);
            return true;
        }
        false
    }

    fn set_priority(&mut self, entry_id: u32, priority: u16) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.priority = priority;
            return true;
        }
        false
    }

    fn set_dependency(&mut self, entry_id: u32, depends_on: u32) -> bool {
        if entry_id == depends_on {
            return false;
        }

        // Verify dependency exists
        if depends_on != 0 && !self.entries.iter().any(|e| e.id == depends_on) {
            return false;
        }

        // Check for circular dependency
        if depends_on != 0 && self.would_create_cycle(entry_id, depends_on) {
            return false;
        }

        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.depends_on = depends_on;
            return true;
        }
        false
    }

    fn would_create_cycle(&self, entry_id: u32, new_dep: u32) -> bool {
        let mut current = new_dep;
        let mut depth = 0;
        while current != 0 && depth < 64 {
            if current == entry_id {
                return true;
            }
            if let Some(e) = self.entries.iter().find(|e| e.id == current) {
                current = e.depends_on;
            } else {
                break;
            }
            depth += 1;
        }
        false
    }

    fn set_restart_policy(
        &mut self,
        entry_id: u32,
        policy: RestartPolicy,
        max_restarts: u16,
    ) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.restart_policy = policy;
            e.max_restarts = max_restarts;
            return true;
        }
        false
    }

    fn set_condition(&mut self, entry_id: u32, condition: StartCondition, param: u32) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.condition = condition;
            e.condition_param = param;
            return true;
        }
        false
    }

    fn set_enabled(&mut self, entry_id: u32, enabled: bool) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.enabled = enabled;
            if !enabled {
                e.state = EntryState::Disabled;
            } else if e.state == EntryState::Disabled {
                e.state = EntryState::Pending;
            }
            return true;
        }
        false
    }

    fn check_condition(&self, entry: &StartupEntry) -> bool {
        match entry.condition {
            StartCondition::Always => true,
            StartCondition::IfNetworkAvailable => self.network_available,
            StartCondition::IfBatteryAbove => self.battery_level > entry.condition_param as u8,
            StartCondition::IfExternalMonitor => self.external_monitor,
            StartCondition::IfDocked => self.docked,
            StartCondition::IfWeekday => self.is_weekday,
            StartCondition::IfWeekend => !self.is_weekday,
        }
    }

    fn dependency_satisfied(&self, entry: &StartupEntry) -> bool {
        if entry.depends_on == 0 {
            return true;
        }
        self.entries.iter().any(|e| {
            e.id == entry.depends_on
                && (e.state == EntryState::Running || e.state == EntryState::Completed)
        })
    }

    fn advance_phase(&mut self, phase: StartupPhase, timestamp: u64) {
        self.current_phase = phase;

        // Gather entries that are ready to start in this phase
        let mut ready_ids: Vec<u32> = Vec::new();
        for entry in &self.entries {
            if !entry.enabled {
                continue;
            }
            if entry.phase != phase {
                continue;
            }
            if entry.state != EntryState::Pending {
                continue;
            }
            if !self.check_condition(entry) {
                continue;
            }
            if !self.dependency_satisfied(entry) {
                continue;
            }
            ready_ids.push(entry.id);
        }

        // Sort by priority (lower = first)
        ready_ids.sort_unstable_by(|a, b| {
            let pa = self
                .entries
                .iter()
                .find(|e| e.id == *a)
                .map(|e| e.priority)
                .unwrap_or(0xFFFF);
            let pb = self
                .entries
                .iter()
                .find(|e| e.id == *b)
                .map(|e| e.priority)
                .unwrap_or(0xFFFF);
            pa.cmp(&pb)
        });

        for id in ready_ids {
            if self.currently_starting >= self.max_concurrent {
                break;
            }
            self.start_entry(id, timestamp);
        }
    }

    fn start_entry(&mut self, entry_id: u32, timestamp: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            if e.delay_ms > 0 {
                e.state = EntryState::DelayWaiting;
                e.start_time = timestamp;
            } else {
                e.state = EntryState::Starting;
                e.start_time = timestamp;
                self.currently_starting = self.currently_starting.saturating_add(1);
                self.entries_started = self.entries_started.saturating_add(1);
            }
        }
    }

    fn on_entry_started(&mut self, entry_id: u32) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            e.state = EntryState::Running;
            if self.currently_starting > 0 {
                self.currently_starting = self.currently_starting.saturating_sub(1);
            }
        }
    }

    fn on_entry_completed(&mut self, entry_id: u32, exit_code: i32, timestamp: u64) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == entry_id) {
            let elapsed = timestamp.saturating_sub(e.start_time) as u32;
            e.elapsed_ms = elapsed;
            e.exit_code = exit_code;

            let success = exit_code == 0;

            if success {
                e.state = EntryState::Completed;
            } else {
                e.failure_count = e.failure_count.saturating_add(1);
                let should_restart = match e.restart_policy {
                    RestartPolicy::Never => false,
                    RestartPolicy::OnFailure => e.restart_count < e.max_restarts,
                    RestartPolicy::Always => e.restart_count < e.max_restarts,
                    RestartPolicy::OncePerSession => e.restart_count == 0,
                };

                if should_restart {
                    e.restart_count = e.restart_count.saturating_add(1);
                    e.state = EntryState::Pending;
                } else {
                    e.state = EntryState::Failed;
                    self.entries_failed = self.entries_failed.saturating_add(1);
                }
            }

            // Record performance
            if self.perf_records.len() < 1024 {
                self.perf_records.push(PerformanceRecord {
                    entry_id,
                    boot_index: self.boot_index,
                    start_ms: e.start_time as u32,
                    duration_ms: elapsed,
                    success,
                });
            }
        }
    }

    fn check_delayed(&mut self, timestamp: u64) {
        let mut to_start: Vec<u32> = Vec::new();
        for entry in &self.entries {
            if entry.state != EntryState::DelayWaiting {
                continue;
            }
            let waited = timestamp.saturating_sub(entry.start_time) as u32;
            if waited >= entry.delay_ms {
                to_start.push(entry.id);
            }
        }

        for id in to_start {
            if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
                e.state = EntryState::Starting;
                self.currently_starting = self.currently_starting.saturating_add(1);
                self.entries_started = self.entries_started.saturating_add(1);
            }
        }
    }

    fn update_environment(
        &mut self,
        network: bool,
        battery: u8,
        monitor: bool,
        docked: bool,
        weekday: bool,
    ) {
        self.network_available = network;
        self.battery_level = battery;
        self.external_monitor = monitor;
        self.docked = docked;
        self.is_weekday = weekday;
    }

    fn begin_boot(&mut self, timestamp: u64) {
        self.boot_index = self.boot_index.saturating_add(1);
        self.boot_start_time = timestamp;
        self.entries_started = 0;
        self.entries_failed = 0;
        self.currently_starting = 0;

        // Reset all entries to pending
        for entry in &mut self.entries {
            if entry.enabled {
                entry.state = EntryState::Pending;
                entry.restart_count = 0;
                entry.start_time = 0;
                entry.elapsed_ms = 0;
                entry.exit_code = 0;
            }
        }
    }

    fn average_boot_time(&self) -> u32 {
        if self.boot_index == 0 {
            return 0;
        }

        let current_boot = self.boot_index;
        let records: Vec<&PerformanceRecord> = self
            .perf_records
            .iter()
            .filter(|r| r.boot_index == current_boot)
            .collect();

        if records.is_empty() {
            return 0;
        }

        let total: u32 = records.iter().map(|r| r.duration_ms).sum();
        total / records.len() as u32
    }

    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn running_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == EntryState::Running)
            .count()
    }

    fn failed_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state == EntryState::Failed)
            .count()
    }

    fn setup_defaults(&mut self) {
        // System services (early boot, hidden)
        let svc1 = self.add_entry(
            0xA001,
            EntryType::Service,
            StartupPhase::EarlyBoot,
            0xBEEF_0000_0000_0001,
        );
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == svc1) {
            e.hidden = true;
            e.priority = 10;
            e.restart_policy = RestartPolicy::Always;
        }

        let svc2 = self.add_entry(
            0xA002,
            EntryType::Service,
            StartupPhase::EarlyBoot,
            0xBEEF_0000_0000_0002,
        );
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == svc2) {
            e.hidden = true;
            e.priority = 11;
            e.restart_policy = RestartPolicy::OnFailure;
        }

        // Network service (pre-login, depends on early boot)
        let net = self.add_entry(
            0xA003,
            EntryType::Service,
            StartupPhase::PreLogin,
            0xBEEF_0000_0000_0003,
        );
        self.set_dependency(net, svc1);
        self.set_priority(net, 20);

        // Desktop compositor (post-login)
        let comp = self.add_entry(
            0xA004,
            EntryType::Service,
            StartupPhase::PostLogin,
            0xBEEF_0000_0000_0004,
        );
        self.set_priority(comp, 30);

        // File manager (desktop ready, delayed)
        let fm = self.add_entry(
            0xA005,
            EntryType::Application,
            StartupPhase::DesktopReady,
            0xBEEF_0000_0000_0005,
        );
        self.set_delay(fm, 2000);
        self.set_priority(fm, 50);

        // Cloud sync (idle, conditional on network)
        let sync_entry = self.add_entry(
            0xA006,
            EntryType::DaemonProcess,
            StartupPhase::Idle,
            0xBEEF_0000_0000_0006,
        );
        self.set_condition(sync_entry, StartCondition::IfNetworkAvailable, 0);
        self.set_delay(sync_entry, 5000);
        self.set_priority(sync_entry, 80);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut mgr = StartupManager::new();
    mgr.setup_defaults();

    let mut guard = STARTUP.lock();
    *guard = Some(mgr);
    serial_println!("    Startup: autostart engine ready ({} entries)", 6);
}

pub fn add_startup_entry(
    app_id: u32,
    entry_type: EntryType,
    phase: StartupPhase,
    name_hash: u64,
) -> u32 {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.add_entry(app_id, entry_type, phase, name_hash);
    }
    0
}

pub fn remove_startup_entry(entry_id: u32) -> bool {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.remove_entry(entry_id);
    }
    false
}

pub fn set_entry_delay(entry_id: u32, delay_ms: u32) -> bool {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.set_delay(entry_id, delay_ms);
    }
    false
}

pub fn set_entry_enabled(entry_id: u32, enabled: bool) -> bool {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.set_enabled(entry_id, enabled);
    }
    false
}

pub fn advance_phase(phase: StartupPhase, timestamp: u64) {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.advance_phase(phase, timestamp);
    }
}

pub fn begin_boot(timestamp: u64) {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.begin_boot(timestamp);
    }
}

pub fn on_entry_completed(entry_id: u32, exit_code: i32, timestamp: u64) {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.on_entry_completed(entry_id, exit_code, timestamp);
    }
}

pub fn check_delayed_starts(timestamp: u64) {
    let mut guard = STARTUP.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.check_delayed(timestamp);
    }
}
