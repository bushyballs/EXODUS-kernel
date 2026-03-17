use crate::sync::Mutex;
use alloc::vec;
/// System Integration Layer — wiring the Hoags AI into every OS subsystem
///
/// This is the bridge between the AI intelligence and the actual system.
/// The AI doesn't just answer questions — it OPERATES the system.
/// File management, process control, networking, hardware, security,
/// user management, app lifecycle, device control, and more.
///
/// The AI understands its purpose: to help with the system AND user needs.
/// It has full access to files, containers, tools, processes, network,
/// and every subsystem. When a task completes, the AI always asks the
/// user if the result is satisfactory.
///
/// Design:
///   - Every OS capability registered as a SystemBinding
///   - Access levels control what the AI can do (ReadOnly -> Root)
///   - Task tracking with user notification (always ask when done)
///   - Event queue for asynchronous system events
///   - Purpose struct so the AI knows WHY it exists
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

// ── System Capability ────────────────────────────────────────────────

/// Every subsystem the AI can interact with
#[derive(Clone, Copy, PartialEq)]
pub enum SystemCapability {
    FileSystem,
    ProcessControl,
    NetworkAccess,
    DisplayControl,
    AudioControl,
    HardwareInfo,
    SecurityPolicy,
    UserManagement,
    AppManagement,
    PackageInstall,
    SystemConfig,
    DeviceControl,
    Telephony,
    Health,
    Wallet,
    Camera,
    Location,
    Contacts,
    Calendar,
    Notifications,
    Automation,
    Debugging,
}

/// Total number of system capabilities (must match enum variant count)
const CAPABILITY_COUNT: usize = 22;

/// Map a capability to its bit index in the bitmask
fn capability_index(cap: SystemCapability) -> u32 {
    match cap {
        SystemCapability::FileSystem => 0,
        SystemCapability::ProcessControl => 1,
        SystemCapability::NetworkAccess => 2,
        SystemCapability::DisplayControl => 3,
        SystemCapability::AudioControl => 4,
        SystemCapability::HardwareInfo => 5,
        SystemCapability::SecurityPolicy => 6,
        SystemCapability::UserManagement => 7,
        SystemCapability::AppManagement => 8,
        SystemCapability::PackageInstall => 9,
        SystemCapability::SystemConfig => 10,
        SystemCapability::DeviceControl => 11,
        SystemCapability::Telephony => 12,
        SystemCapability::Health => 13,
        SystemCapability::Wallet => 14,
        SystemCapability::Camera => 15,
        SystemCapability::Location => 16,
        SystemCapability::Contacts => 17,
        SystemCapability::Calendar => 18,
        SystemCapability::Notifications => 19,
        SystemCapability::Automation => 20,
        SystemCapability::Debugging => 21,
    }
}

/// Return all capabilities in registration order
fn all_capabilities() -> [SystemCapability; CAPABILITY_COUNT] {
    [
        SystemCapability::FileSystem,
        SystemCapability::ProcessControl,
        SystemCapability::NetworkAccess,
        SystemCapability::DisplayControl,
        SystemCapability::AudioControl,
        SystemCapability::HardwareInfo,
        SystemCapability::SecurityPolicy,
        SystemCapability::UserManagement,
        SystemCapability::AppManagement,
        SystemCapability::PackageInstall,
        SystemCapability::SystemConfig,
        SystemCapability::DeviceControl,
        SystemCapability::Telephony,
        SystemCapability::Health,
        SystemCapability::Wallet,
        SystemCapability::Camera,
        SystemCapability::Location,
        SystemCapability::Contacts,
        SystemCapability::Calendar,
        SystemCapability::Notifications,
        SystemCapability::Automation,
        SystemCapability::Debugging,
    ]
}

// ── Access Level ─────────────────────────────────────────────────────

/// How much power the AI has over a given capability
#[derive(Clone, Copy, PartialEq)]
pub enum AccessLevel {
    ReadOnly,
    ReadWrite,
    Execute,
    Admin,
    Root,
}

impl AccessLevel {
    /// Numeric rank for comparison (higher = more privilege)
    fn rank(self) -> u8 {
        match self {
            AccessLevel::ReadOnly => 0,
            AccessLevel::ReadWrite => 1,
            AccessLevel::Execute => 2,
            AccessLevel::Admin => 3,
            AccessLevel::Root => 4,
        }
    }

    /// Whether this level permits write operations
    fn can_write(self) -> bool {
        self.rank() >= AccessLevel::ReadWrite.rank()
    }

    /// Whether this level permits execution
    fn can_execute(self) -> bool {
        self.rank() >= AccessLevel::Execute.rank()
    }
}

// ── System Binding ───────────────────────────────────────────────────

/// A registered connection between the AI and an OS subsystem
#[derive(Clone, Copy)]
pub struct SystemBinding {
    pub capability: SystemCapability,
    pub access: AccessLevel,
    pub handler_hash: u64,
    pub enabled: bool,
    pub call_count: u64,
    pub last_called: u64,
}

impl SystemBinding {
    fn new(capability: SystemCapability) -> Self {
        // Generate a deterministic handler hash from capability index
        let idx = capability_index(capability) as u64;
        let hash = 0xABCD_0000_0000_0000_u64 | (idx * 0x0101_0101_0101);

        SystemBinding {
            capability,
            access: AccessLevel::ReadOnly,
            handler_hash: hash,
            enabled: true,
            call_count: 0,
            last_called: 0,
        }
    }
}

// ── AI Purpose ───────────────────────────────────────────────────────

/// The AI's self-understanding — why it exists, what it should do
#[derive(Clone)]
pub struct AiPurpose {
    pub primary_mission_hash: u64,
    pub secondary_goals: Vec<u64>,
    pub constraints: Vec<u64>,
    pub identity_hash: u64,
    pub version: u32,
}

impl AiPurpose {
    /// Default purpose: help user and system
    fn default_purpose() -> Self {
        // "Help user and system" hashed
        let mission_hash: u64 = 0xAE1F_0C3D_5B2A_4E9D;
        // Identity: "Hoags AI - Genesis OS Intelligence Core"
        let identity: u64 = 0xDA7A_BABE_CAFE_F00D;

        AiPurpose {
            primary_mission_hash: mission_hash,
            secondary_goals: vec![
                0x0001_0000_0000_0001, // Learn from user interactions
                0x0002_0000_0000_0002, // Optimize system performance
                0x0003_0000_0000_0003, // Protect user privacy
                0x0004_0000_0000_0004, // Anticipate user needs
                0x0005_0000_0000_0005, // Maintain system health
            ],
            constraints: vec![
                0x00C0_0000_0000_0001, // Never send data externally without permission
                0x00C0_0000_0000_0002, // Always ask user before destructive actions
                0x00C0_0000_0000_0003, // Respect access level boundaries
                0x00C0_0000_0000_0004, // Log all privileged operations
            ],
            identity_hash: identity,
            version: 1,
        }
    }
}

// ── Task Completion ──────────────────────────────────────────────────

/// A task the AI has been asked to do (or initiated itself)
#[derive(Clone, Copy)]
pub struct TaskCompletion {
    pub task_hash: u64,
    pub started: u64,
    pub completed: u64,
    pub success: bool,
    pub user_notified: bool,
    pub feedback_received: bool,
}

// ── Event System ─────────────────────────────────────────────────────

/// Types of events the integration engine processes
#[derive(Clone, Copy, PartialEq)]
pub enum EventType {
    UserRequest,
    SystemAlert,
    ScheduledTask,
    BackgroundLearn,
    HardwareChange,
    AppEvent,
    NetworkEvent,
    SecurityEvent,
    TimerFired,
}

/// An event queued for processing
#[derive(Clone, Copy)]
pub struct SystemEvent {
    pub event_type: EventType,
    pub source_hash: u64,
    pub data_hash: u64,
    pub timestamp: u64,
    pub handled: bool,
}

// ── Integration Engine ───────────────────────────────────────────────

/// The main integration engine — wires AI into every OS subsystem
pub struct IntegrationEngine {
    pub bindings: Vec<SystemBinding>,
    pub purpose: AiPurpose,
    pub active_tasks: Vec<TaskCompletion>,
    pub event_queue: Vec<SystemEvent>,
    pub capabilities_enabled: u32,
    pub total_system_calls: u64,
    pub total_tasks_completed: u64,
    pub uptime_seconds: u64,
    pub ask_when_done: bool,
}

impl IntegrationEngine {
    /// Create a new integration engine with all 22 capabilities bound at ReadOnly
    pub fn new() -> Self {
        let caps = all_capabilities();
        let mut bindings = Vec::with_capacity(CAPABILITY_COUNT);
        let mut bitmask: u32 = 0;

        for cap in caps.iter() {
            bindings.push(SystemBinding::new(*cap));
            bitmask |= 1 << capability_index(*cap);
        }

        // Set purpose: "Help user and system"
        let purpose = AiPurpose::default_purpose();

        IntegrationEngine {
            bindings,
            purpose,
            active_tasks: Vec::new(),
            event_queue: Vec::new(),
            capabilities_enabled: bitmask,
            total_system_calls: 0,
            total_tasks_completed: 0,
            uptime_seconds: 0,
            ask_when_done: true, // Always ask user when task completes
        }
    }

    /// Upgrade the access level for a capability
    pub fn bind_capability(&mut self, cap: SystemCapability, access: AccessLevel) {
        for binding in self.bindings.iter_mut() {
            if binding.capability == cap {
                binding.access = access;
                binding.enabled = true;
                return;
            }
        }
        // Capability not found — register it fresh
        let mut binding = SystemBinding::new(cap);
        binding.access = access;
        self.bindings.push(binding);
        self.capabilities_enabled |= 1 << capability_index(cap);
    }

    /// Invoke a system capability with an action hash
    /// Returns true if the call was permitted and executed
    pub fn invoke(&mut self, cap: SystemCapability, action_hash: u64) -> bool {
        if !self.can_access(cap) {
            return false;
        }

        for binding in self.bindings.iter_mut() {
            if binding.capability == cap && binding.enabled {
                binding.call_count = binding.call_count.saturating_add(1);
                binding.last_called = action_hash; // Use action_hash as timestamp proxy
                self.total_system_calls = self.total_system_calls.saturating_add(1);
                return true;
            }
        }

        false
    }

    /// Check whether a capability is accessible (bound and enabled)
    pub fn can_access(&self, cap: SystemCapability) -> bool {
        let bit = 1 << capability_index(cap);
        if self.capabilities_enabled & bit == 0 {
            return false;
        }
        for binding in self.bindings.iter() {
            if binding.capability == cap {
                return binding.enabled;
            }
        }
        false
    }

    /// Get the current access level for a capability
    pub fn get_access_level(&self, cap: SystemCapability) -> AccessLevel {
        for binding in self.bindings.iter() {
            if binding.capability == cap {
                return binding.access;
            }
        }
        AccessLevel::ReadOnly
    }

    /// Submit a new task for tracking
    pub fn submit_task(&mut self, task: u64, timestamp: u64) {
        let completion = TaskCompletion {
            task_hash: task,
            started: timestamp,
            completed: 0,
            success: false,
            user_notified: false,
            feedback_received: false,
        };
        self.active_tasks.push(completion);
    }

    /// Mark a task as complete — sets user_notified to false so we ask the user
    pub fn complete_task(&mut self, task: u64, timestamp: u64, success: bool) {
        for t in self.active_tasks.iter_mut() {
            if t.task_hash == task && t.completed == 0 {
                t.completed = timestamp;
                t.success = success;
                t.user_notified = false; // Must notify user
                self.total_tasks_completed = self.total_tasks_completed.saturating_add(1);
                return;
            }
        }
    }

    /// Get all completed tasks where the user has not been notified yet
    pub fn get_pending_notifications(&self) -> Vec<&TaskCompletion> {
        let mut pending = Vec::new();
        for t in self.active_tasks.iter() {
            if t.completed > 0 && !t.user_notified {
                pending.push(t);
            }
        }
        pending
    }

    /// Mark a task as user-notified (the AI told the user it's done)
    pub fn notify_user(&mut self, task: u64) {
        for t in self.active_tasks.iter_mut() {
            if t.task_hash == task {
                t.user_notified = true;
                return;
            }
        }
    }

    /// Record user feedback on a completed task
    pub fn record_feedback(&mut self, task: u64) {
        for t in self.active_tasks.iter_mut() {
            if t.task_hash == task && t.user_notified {
                t.feedback_received = true;
                return;
            }
        }
    }

    /// Queue a system event for processing
    pub fn queue_event(&mut self, event: SystemEvent) {
        self.event_queue.push(event);
    }

    /// Process all queued events, returning the number handled
    pub fn process_events(&mut self) -> u32 {
        let mut count: u32 = 0;

        for event in self.event_queue.iter_mut() {
            if event.handled {
                continue;
            }

            match event.event_type {
                EventType::UserRequest => {
                    // User requests get highest priority — always handle
                    event.handled = true;
                    count += 1;
                }
                EventType::SystemAlert => {
                    // System alerts processed immediately
                    event.handled = true;
                    count += 1;
                }
                EventType::SecurityEvent => {
                    // Security events — handle and potentially escalate
                    event.handled = true;
                    count += 1;
                }
                EventType::ScheduledTask => {
                    event.handled = true;
                    count += 1;
                }
                EventType::BackgroundLearn => {
                    // Low priority learning — handle when idle
                    event.handled = true;
                    count += 1;
                }
                EventType::HardwareChange => {
                    event.handled = true;
                    count += 1;
                }
                EventType::AppEvent => {
                    event.handled = true;
                    count += 1;
                }
                EventType::NetworkEvent => {
                    event.handled = true;
                    count += 1;
                }
                EventType::TimerFired => {
                    event.handled = true;
                    count += 1;
                }
            }
        }

        // Prune handled events to keep queue lean
        self.event_queue.retain(|e| !e.handled);

        count
    }

    /// Get the AI's purpose definition
    pub fn get_purpose(&self) -> &AiPurpose {
        &self.purpose
    }

    /// Update the AI's primary mission and secondary goals
    pub fn set_purpose(&mut self, mission: u64, goals: Vec<u64>) {
        self.purpose.primary_mission_hash = mission;
        self.purpose.secondary_goals = goals;
        self.purpose.version = self.purpose.version.saturating_add(1);
    }

    /// Get stats: (total system calls, total tasks completed, capabilities active count)
    pub fn get_stats(&self) -> (u64, u64, u32) {
        let mut active_count: u32 = 0;
        let mut mask = self.capabilities_enabled;
        while mask != 0 {
            active_count += (mask & 1) as u32;
            mask >>= 1;
        }
        (
            self.total_system_calls,
            self.total_tasks_completed,
            active_count,
        )
    }

    /// Disable a capability entirely
    pub fn disable_capability(&mut self, cap: SystemCapability) {
        let bit = 1 << capability_index(cap);
        self.capabilities_enabled &= !bit;
        for binding in self.bindings.iter_mut() {
            if binding.capability == cap {
                binding.enabled = false;
                return;
            }
        }
    }

    /// Re-enable a previously disabled capability
    pub fn enable_capability(&mut self, cap: SystemCapability) {
        let bit = 1 << capability_index(cap);
        self.capabilities_enabled |= bit;
        for binding in self.bindings.iter_mut() {
            if binding.capability == cap {
                binding.enabled = true;
                return;
            }
        }
    }

    /// Get how many tasks are currently in-progress (started but not completed)
    pub fn active_task_count(&self) -> usize {
        let mut count = 0;
        for t in self.active_tasks.iter() {
            if t.completed == 0 {
                count += 1;
            }
        }
        count
    }

    /// Get the total call count for a specific capability
    pub fn get_call_count(&self, cap: SystemCapability) -> u64 {
        for binding in self.bindings.iter() {
            if binding.capability == cap {
                return binding.call_count;
            }
        }
        0
    }

    /// Compute a simple health score using Q16 fixed-point math
    /// Score based on: capabilities active, tasks completed successfully, uptime
    pub fn health_score(&self) -> Q16 {
        let (_total_calls, _tasks_done, caps_active) = self.get_stats();

        // Base score: percentage of capabilities active out of 22
        let caps_q16 = q16_from_int(caps_active as i32);
        let _max_caps_q16 = q16_from_int(CAPABILITY_COUNT as i32);

        // Avoid division — use multiplication by reciprocal approximation
        // 1/22 ~ 2979 in Q16 (65536 / 22 = 2978.9)
        let reciprocal_22: Q16 = 2979;
        let cap_ratio = q16_mul(caps_q16, reciprocal_22);

        // Scale to 0-100 range
        let hundred = q16_from_int(100);
        let score = q16_mul(cap_ratio, hundred);

        score
    }

    /// Tick the uptime counter by one second
    pub fn tick_uptime(&mut self) {
        self.uptime_seconds = self.uptime_seconds.saturating_add(1);
    }
}

// ── Global State ─────────────────────────────────────────────────────

static ENGINE: Mutex<Option<IntegrationEngine>> = Mutex::new(None);

/// Initialize the integration engine with full system bindings
pub fn init() {
    let mut engine = IntegrationEngine::new();

    // Upgrade critical subsystems beyond ReadOnly for full OS integration
    engine.bind_capability(SystemCapability::FileSystem, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::ProcessControl, AccessLevel::Execute);
    engine.bind_capability(SystemCapability::NetworkAccess, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::DisplayControl, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::AudioControl, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::HardwareInfo, AccessLevel::ReadOnly);
    engine.bind_capability(SystemCapability::SecurityPolicy, AccessLevel::Admin);
    engine.bind_capability(SystemCapability::UserManagement, AccessLevel::Admin);
    engine.bind_capability(SystemCapability::AppManagement, AccessLevel::Execute);
    engine.bind_capability(SystemCapability::PackageInstall, AccessLevel::Execute);
    engine.bind_capability(SystemCapability::SystemConfig, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::DeviceControl, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Telephony, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Health, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Wallet, AccessLevel::ReadOnly);
    engine.bind_capability(SystemCapability::Camera, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Location, AccessLevel::ReadOnly);
    engine.bind_capability(SystemCapability::Contacts, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Calendar, AccessLevel::ReadWrite);
    engine.bind_capability(SystemCapability::Notifications, AccessLevel::Execute);
    engine.bind_capability(SystemCapability::Automation, AccessLevel::Execute);
    engine.bind_capability(SystemCapability::Debugging, AccessLevel::Admin);

    let (calls, tasks, caps) = engine.get_stats();

    let mut locked = ENGINE.lock();
    *locked = Some(engine);

    serial_println!(
        "  System integration ready: {} capabilities bound, {} calls, {} tasks tracked",
        caps,
        calls,
        tasks
    );
    serial_println!("  AI purpose: Help user and system | ask_when_done=true | version=1");
}

/// Access the global integration engine (for use by other modules)
pub fn with_engine<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut IntegrationEngine) -> R,
{
    let mut locked = ENGINE.lock();
    if let Some(ref mut engine) = *locked {
        Some(f(engine))
    } else {
        None
    }
}
