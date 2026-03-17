use crate::sync::Mutex;
/// Hoags Task Automation — event-driven workflow engine
///
/// Create automated tasks that fire on triggers and execute action chains.
/// Triggers include schedules, system events, file changes, app launches,
/// time-of-day, battery levels, location enter, or manual activation.
/// Actions can run scripts, open apps, send notifications, change settings,
/// make HTTP requests, play sounds, copy files, or chain sub-actions.
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: Tasker (Android), Shortcuts (iOS), IFTTT, AutoHotkey.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Q16 fixed-point: 65536 = 1.0
type Q16 = i32;
const Q16_ONE: Q16 = 65536;

/// Maximum tasks in the system
const MAX_TASKS: usize = 256;
/// Maximum actions per task
const MAX_ACTIONS: usize = 32;
/// Maximum history entries
const MAX_HISTORY: usize = 512;

// ---------------------------------------------------------------------------
// Trigger — what causes a task to fire
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    /// Fire at a specific timestamp (epoch seconds)
    Schedule(u64),
    /// Fire on a system event (event type hash)
    Event(u64),
    /// Fire when a file changes (path hash)
    FileChange(u64),
    /// Fire when an application launches (app name hash)
    AppLaunch(u64),
    /// Fire at a specific time of day (seconds since midnight, 0-86399)
    TimeOfDay(u32),
    /// Fire when battery drops to or below this level (0-100)
    BatteryLevel(u8),
    /// Fire when entering a location (location hash)
    LocationEnter(u64),
    /// Fire only when manually triggered by user
    Manual,
}

impl Trigger {
    /// Check if this trigger matches the given event context
    fn matches(&self, event_type: u64, event_value: u64, current_time: u64, battery: u8) -> bool {
        match self {
            Trigger::Schedule(ts) => current_time >= *ts,
            Trigger::Event(ev) => event_type == *ev,
            Trigger::FileChange(path_hash) => event_type == 0xF11E && event_value == *path_hash,
            Trigger::AppLaunch(app_hash) => event_type == 0xA991 && event_value == *app_hash,
            Trigger::TimeOfDay(secs) => {
                let time_of_day = (current_time % 86400) as u32;
                // Match within a 60-second window
                let diff = if time_of_day >= *secs {
                    time_of_day - *secs
                } else {
                    *secs - time_of_day
                };
                diff < 60
            }
            Trigger::BatteryLevel(level) => battery <= *level,
            Trigger::LocationEnter(loc_hash) => event_type == 0x10CA && event_value == *loc_hash,
            Trigger::Manual => false, // only fires via explicit execute_task()
        }
    }
}

// ---------------------------------------------------------------------------
// Action — what a task does when triggered
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Run a script (script hash)
    RunScript(u64),
    /// Open an application (app name hash)
    OpenApp(u64),
    /// Send a notification (message hash)
    SendNotification(u64),
    /// Change a system setting (setting hash, Q16 value)
    ChangeSettings(u64, Q16),
    /// Make an HTTP request (URL hash)
    HttpRequest(u64),
    /// Play a sound file (sound hash)
    PlaySound(u64),
    /// Copy a file (source path hash, destination path hash)
    CopyFile(u64, u64),
    /// Chain multiple sub-actions in sequence
    Chain(Vec<Action>),
}

// ---------------------------------------------------------------------------
// AutoTask
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AutoTask {
    pub id: u32,
    pub name_hash: u64,
    pub trigger: Trigger,
    pub actions: Vec<Action>,
    pub enabled: bool,
    pub run_count: u32,
    pub last_run: u64,
}

impl AutoTask {
    fn new(id: u32, name_hash: u64, trigger: Trigger) -> Self {
        AutoTask {
            id,
            name_hash,
            trigger,
            actions: Vec::new(),
            enabled: true,
            run_count: 0,
            last_run: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskHistory — record of past executions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TaskHistory {
    task_id: u32,
    timestamp: u64,
    success: bool,
    actions_executed: u32,
    duration_ms: u32,
}

// ---------------------------------------------------------------------------
// TaskAutomation — main state
// ---------------------------------------------------------------------------

struct TaskAutomationState {
    tasks: Vec<AutoTask>,
    history: Vec<TaskHistory>,
    next_id: u32,
    initialized: bool,
    total_runs: u64,
}

impl TaskAutomationState {
    fn new() -> Self {
        TaskAutomationState {
            tasks: Vec::new(),
            history: Vec::new(),
            next_id: 1,
            initialized: false,
            total_runs: 0,
        }
    }
}

static AUTOMATION: Mutex<Option<TaskAutomationState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new automated task. Returns the task ID.
pub fn create_task(name_hash: u64, trigger: Trigger, actions: Vec<Action>) -> u32 {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        if state.tasks.len() >= MAX_TASKS {
            serial_println!("[task_auto] ERROR: max tasks ({}) reached", MAX_TASKS);
            return 0;
        }
        let id = state.next_id;
        state.next_id = state.next_id.saturating_add(1);

        let mut task = AutoTask::new(id, name_hash, trigger);
        for action in actions {
            if task.actions.len() < MAX_ACTIONS {
                task.actions.push(action);
            }
        }
        serial_println!(
            "[task_auto] Created task {} (name_hash={:#018X}, {} actions)",
            id,
            name_hash,
            task.actions.len()
        );
        state.tasks.push(task);
        id
    } else {
        0
    }
}

/// Delete a task by ID. Returns true if found and deleted.
pub fn delete_task(task_id: u32) -> bool {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        let before = state.tasks.len();
        state.tasks.retain(|t| t.id != task_id);
        let deleted = state.tasks.len() < before;
        if deleted {
            serial_println!("[task_auto] Deleted task {}", task_id);
        }
        deleted
    } else {
        false
    }
}

/// Enable a task by ID.
pub fn enable(task_id: u32) -> bool {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        for task in &mut state.tasks {
            if task.id == task_id {
                task.enabled = true;
                serial_println!("[task_auto] Enabled task {}", task_id);
                return true;
            }
        }
    }
    false
}

/// Disable a task by ID.
pub fn disable(task_id: u32) -> bool {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        for task in &mut state.tasks {
            if task.id == task_id {
                task.enabled = false;
                serial_println!("[task_auto] Disabled task {}", task_id);
                return true;
            }
        }
    }
    false
}

/// Check all tasks against an event context, execute any that match.
/// Returns the number of tasks that fired.
pub fn check_triggers(event_type: u64, event_value: u64, current_time: u64, battery: u8) -> u32 {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        let mut fired = 0u32;
        let task_count = state.tasks.len();

        for i in 0..task_count {
            if !state.tasks[i].enabled {
                continue;
            }
            if state.tasks[i]
                .trigger
                .matches(event_type, event_value, current_time, battery)
            {
                // Execute the task's actions
                let task_id = state.tasks[i].id;
                let actions = state.tasks[i].actions.clone();
                let action_count = execute_actions(&actions);

                state.tasks[i].run_count = state.tasks[i].run_count.saturating_add(1);
                state.tasks[i].last_run = current_time;
                state.total_runs = state.total_runs.saturating_add(1);

                // Record history
                if state.history.len() < MAX_HISTORY {
                    state.history.push(TaskHistory {
                        task_id,
                        timestamp: current_time,
                        success: true,
                        actions_executed: action_count,
                        duration_ms: 0, // no real timing in kernel context
                    });
                }

                serial_println!(
                    "[task_auto] Task {} fired ({} actions)",
                    task_id,
                    action_count
                );
                fired += 1;
            }
        }

        fired
    } else {
        0
    }
}

/// Manually execute a specific task by ID, regardless of trigger.
pub fn execute_task(task_id: u32, current_time: u64) -> bool {
    let mut guard = AUTOMATION.lock();
    if let Some(ref mut state) = *guard {
        for task in &mut state.tasks {
            if task.id == task_id {
                let actions = task.actions.clone();
                let action_count = execute_actions(&actions);

                task.run_count = task.run_count.saturating_add(1);
                task.last_run = current_time;
                state.total_runs = state.total_runs.saturating_add(1);

                if state.history.len() < MAX_HISTORY {
                    state.history.push(TaskHistory {
                        task_id,
                        timestamp: current_time,
                        success: true,
                        actions_executed: action_count,
                        duration_ms: 0,
                    });
                }

                serial_println!(
                    "[task_auto] Manually executed task {} ({} actions)",
                    task_id,
                    action_count
                );
                return true;
            }
        }
    }
    false
}

/// Get execution history for a specific task.
/// Returns list of (timestamp, success, actions_executed).
pub fn get_history(task_id: u32) -> Vec<(u64, bool, u32)> {
    let guard = AUTOMATION.lock();
    if let Some(ref state) = *guard {
        let mut result = Vec::new();
        for entry in &state.history {
            if entry.task_id == task_id {
                result.push((entry.timestamp, entry.success, entry.actions_executed));
            }
        }
        result
    } else {
        Vec::new()
    }
}

/// Export a task as a portable representation.
/// Returns (name_hash, trigger_type_id, action_count, enabled).
pub fn export_task(task_id: u32) -> Option<(u64, u8, usize, bool)> {
    let guard = AUTOMATION.lock();
    if let Some(ref state) = *guard {
        for task in &state.tasks {
            if task.id == task_id {
                let trigger_type = match task.trigger {
                    Trigger::Schedule(_) => 0,
                    Trigger::Event(_) => 1,
                    Trigger::FileChange(_) => 2,
                    Trigger::AppLaunch(_) => 3,
                    Trigger::TimeOfDay(_) => 4,
                    Trigger::BatteryLevel(_) => 5,
                    Trigger::LocationEnter(_) => 6,
                    Trigger::Manual => 7,
                };
                return Some((
                    task.name_hash,
                    trigger_type,
                    task.actions.len(),
                    task.enabled,
                ));
            }
        }
    }
    None
}

/// Import a task from a portable representation.
/// This is a simplified import that creates a manual-trigger task with no actions.
/// The caller should add actions via create_task for full fidelity.
pub fn import_task(name_hash: u64, trigger_type: u8, enabled: bool) -> u32 {
    let trigger = match trigger_type {
        0 => Trigger::Schedule(0),
        1 => Trigger::Event(0),
        2 => Trigger::FileChange(0),
        3 => Trigger::AppLaunch(0),
        4 => Trigger::TimeOfDay(0),
        5 => Trigger::BatteryLevel(50),
        6 => Trigger::LocationEnter(0),
        _ => Trigger::Manual,
    };

    let id = create_task(name_hash, trigger, Vec::new());
    if !enabled {
        disable(id);
    }
    serial_println!(
        "[task_auto] Imported task {} (trigger_type={})",
        id,
        trigger_type
    );
    id
}

/// Get the total number of registered tasks.
pub fn task_count() -> usize {
    let guard = AUTOMATION.lock();
    if let Some(ref state) = *guard {
        state.tasks.len()
    } else {
        0
    }
}

/// Get total runs across all tasks.
pub fn total_runs() -> u64 {
    let guard = AUTOMATION.lock();
    if let Some(ref state) = *guard {
        state.total_runs
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Internal action execution
// ---------------------------------------------------------------------------

fn execute_actions(actions: &[Action]) -> u32 {
    let mut count = 0u32;
    for action in actions {
        execute_single_action(action);
        count += 1;
    }
    count
}

fn execute_single_action(action: &Action) {
    match action {
        Action::RunScript(script_hash) => {
            serial_println!("[task_auto]   -> RunScript({:#018X})", script_hash);
            // In a full implementation, this would look up and execute
            // the script via the script engine
        }
        Action::OpenApp(app_hash) => {
            serial_println!("[task_auto]   -> OpenApp({:#018X})", app_hash);
            // Would launch the application via the process subsystem
        }
        Action::SendNotification(msg_hash) => {
            serial_println!("[task_auto]   -> SendNotification({:#018X})", msg_hash);
            // Would route through the notification framework
        }
        Action::ChangeSettings(setting_hash, value) => {
            let whole = value >> 16;
            let frac = ((value & 0xFFFF) * 1000) >> 16;
            serial_println!(
                "[task_auto]   -> ChangeSettings({:#018X}, {}.{:03})",
                setting_hash,
                whole,
                frac
            );
            // Would update the system config
        }
        Action::HttpRequest(url_hash) => {
            serial_println!("[task_auto]   -> HttpRequest({:#018X})", url_hash);
            // Would create a network request via the net stack
        }
        Action::PlaySound(sound_hash) => {
            serial_println!("[task_auto]   -> PlaySound({:#018X})", sound_hash);
            // Would play via the audio subsystem
        }
        Action::CopyFile(src_hash, dst_hash) => {
            serial_println!(
                "[task_auto]   -> CopyFile({:#018X} -> {:#018X})",
                src_hash,
                dst_hash
            );
            // Would copy via the filesystem
        }
        Action::Chain(sub_actions) => {
            serial_println!("[task_auto]   -> Chain({} sub-actions)", sub_actions.len());
            for sub in sub_actions {
                execute_single_action(sub);
            }
        }
    }
}

pub fn init() {
    let mut guard = AUTOMATION.lock();
    *guard = Some(TaskAutomationState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!("    [scripting] Task automation initialized (triggers, actions, workflows)");
}
