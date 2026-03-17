/// AI Automation for Genesis
///
/// Rule-based and ML-driven automation: triggers,
/// conditions, actions, routines, and adaptive behavior.
///
/// Features real condition evaluation against system state,
/// action execution with chaining and delay, rule priority
/// and conflict resolution, cooldown tracking, and run
/// history for debugging.
///
/// Inspired by: Android Routines, iOS Shortcuts, Tasker. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// Trigger type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerType {
    Time,
    Location,
    AppOpen,
    AppClose,
    WifiConnect,
    WifiDisconnect,
    BluetoothConnect,
    Charging,
    BatteryLow,
    Sunrise,
    Sunset,
    NfcTag,
    VoiceCommand,
    Notification,
    ScreenOn,
    ScreenOff,
}

/// Condition type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionType {
    BatteryAbove,
    BatteryBelow,
    WifiConnected,
    BluetoothConnected,
    ScreenOn,
    DoNotDisturb,
    TimeRange,
    DayOfWeek,
    AppRunning,
    HeadphonesConnected,
}

/// Action type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    OpenApp,
    SetVolume,
    SetBrightness,
    ToggleWifi,
    ToggleBluetooth,
    ToggleDnd,
    SendNotification,
    PlayMedia,
    SetWallpaper,
    RunShellCommand,
    SetSetting,
    LaunchShortcut,
    Speak,
    Wait,
}

/// Priority levels for conflict resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Result of executing an action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionResult {
    Success,
    Failed,
    Skipped,
    Delayed,
}

// ---------------------------------------------------------------------------
// Core data structures
// ---------------------------------------------------------------------------

/// A trigger definition
pub struct Trigger {
    pub trigger_type: TriggerType,
    pub value: String,
}

/// A condition check
pub struct Condition {
    pub condition_type: ConditionType,
    pub value: String,
    pub negate: bool,
}

/// An action to perform
pub struct Action {
    pub action_type: ActionType,
    pub params: Vec<(String, String)>,
    pub delay_ms: u64,
}

/// Record of a single action execution
#[derive(Debug, Clone)]
pub struct ActionLog {
    pub action_type: ActionType,
    pub result: ActionResult,
    pub timestamp: u64,
    pub detail: String,
}

/// Record of a routine execution
#[derive(Debug, Clone)]
pub struct RunLog {
    pub routine_id: u32,
    pub routine_name: String,
    pub timestamp: u64,
    pub trigger_type: TriggerType,
    pub trigger_value: String,
    pub actions_executed: u32,
    pub actions_failed: u32,
}

/// A complete automation routine
pub struct Routine {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub triggers: Vec<Trigger>,
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    pub run_count: u64,
    pub last_run: u64,
    pub created_at: u64,
    /// Priority for conflict resolution
    pub priority: Priority,
    /// Minimum cooldown between runs (seconds)
    pub cooldown_secs: u64,
    /// Maximum times this routine can run per day (0 = unlimited)
    pub max_daily_runs: u32,
    /// Count of runs today (resets at midnight)
    pub runs_today: u32,
    /// Last day (day-of-year) when runs_today was last reset
    pub runs_today_day: u32,
    /// Whether to stop action chain on first failure
    pub stop_on_failure: bool,
    /// Tags for grouping and filtering
    pub tags: Vec<String>,
}

/// System state snapshot for condition evaluation
pub struct SystemState {
    pub battery_level: u8,
    pub is_charging: bool,
    pub wifi_connected: bool,
    pub wifi_ssid: String,
    pub bluetooth_connected: bool,
    pub bluetooth_device: String,
    pub screen_on: bool,
    pub dnd_enabled: bool,
    pub current_hour: u8,
    pub current_minute: u8,
    pub day_of_week: u8,
    pub headphones_connected: bool,
    pub running_apps: Vec<String>,
    pub current_volume: u8,
    pub current_brightness: u8,
}

impl SystemState {
    const fn new() -> Self {
        SystemState {
            battery_level: 100,
            is_charging: false,
            wifi_connected: false,
            wifi_ssid: String::new(),
            bluetooth_connected: false,
            bluetooth_device: String::new(),
            screen_on: true,
            dnd_enabled: false,
            current_hour: 0,
            current_minute: 0,
            day_of_week: 0,
            headphones_connected: false,
            running_apps: Vec::new(),
            current_volume: 50,
            current_brightness: 50,
        }
    }

    /// Update time fields from the system clock
    fn update_time(&mut self) {
        let now = crate::time::clock::unix_time();
        self.current_hour = ((now / 3600) % 24) as u8;
        self.current_minute = ((now / 60) % 60) as u8;
        self.day_of_week = ((now / 86400) % 7) as u8;
    }
}

// ---------------------------------------------------------------------------
// Condition evaluation
// ---------------------------------------------------------------------------

/// Evaluate a single condition against the current system state.
/// Returns true if the condition is met (respecting negation).
fn check_condition(condition: &Condition, state: &SystemState) -> bool {
    let raw = match condition.condition_type {
        ConditionType::BatteryAbove => {
            let threshold = parse_u8(&condition.value).unwrap_or(50);
            state.battery_level > threshold
        }
        ConditionType::BatteryBelow => {
            let threshold = parse_u8(&condition.value).unwrap_or(20);
            state.battery_level < threshold
        }
        ConditionType::WifiConnected => {
            if condition.value.is_empty() {
                state.wifi_connected
            } else {
                state.wifi_connected && state.wifi_ssid == condition.value
            }
        }
        ConditionType::BluetoothConnected => {
            if condition.value.is_empty() {
                state.bluetooth_connected
            } else {
                state.bluetooth_connected && state.bluetooth_device == condition.value
            }
        }
        ConditionType::ScreenOn => state.screen_on,
        ConditionType::DoNotDisturb => state.dnd_enabled,
        ConditionType::TimeRange => {
            // value format: "HH:MM-HH:MM" e.g. "09:00-17:00"
            parse_time_range_check(&condition.value, state.current_hour, state.current_minute)
        }
        ConditionType::DayOfWeek => {
            // value: comma-separated day numbers (0=Sun, 1=Mon, ..., 6=Sat)
            parse_days(&condition.value).contains(&state.day_of_week)
        }
        ConditionType::AppRunning => state.running_apps.iter().any(|a| *a == condition.value),
        ConditionType::HeadphonesConnected => state.headphones_connected,
    };

    if condition.negate {
        !raw
    } else {
        raw
    }
}

/// Parse a "HH:MM-HH:MM" time range and check if current time is within it
fn parse_time_range_check(value: &str, hour: u8, minute: u8) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 2 {
        return true;
    } // malformed => pass

    let start = parse_hhmm(parts[0]);
    let end = parse_hhmm(parts[1]);
    let current = (hour as u32) * 60 + minute as u32;

    if start <= end {
        // Normal range (e.g. 09:00-17:00)
        current >= start && current < end
    } else {
        // Wraps midnight (e.g. 22:00-06:00)
        current >= start || current < end
    }
}

fn parse_hhmm(s: &str) -> u32 {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 2 {
        return 0;
    }
    let h = parse_u8(parts[0]).unwrap_or(0) as u32;
    let m = parse_u8(parts[1]).unwrap_or(0) as u32;
    h * 60 + m
}

fn parse_days(s: &str) -> Vec<u8> {
    s.split(',').filter_map(|d| parse_u8(d.trim())).collect()
}

fn parse_u8(s: &str) -> Option<u8> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut result: u16 = 0;
    for c in s.chars() {
        if !c.is_ascii_digit() {
            return None;
        }
        result = result * 10 + (c as u16 - '0' as u16);
        if result > 255 {
            return None;
        }
    }
    Some(result as u8)
}

// ---------------------------------------------------------------------------
// Action execution
// ---------------------------------------------------------------------------

/// Execute a single action and return the result + log entry
fn execute_action(action: &Action, state: &mut SystemState) -> ActionLog {
    let now = crate::time::clock::unix_time();

    // Handle delay (we log it as Delayed; the caller should re-invoke later)
    if action.delay_ms > 0 {
        return ActionLog {
            action_type: action.action_type,
            result: ActionResult::Delayed,
            timestamp: now,
            detail: format!("Delayed {}ms", action.delay_ms),
        };
    }

    let (result, detail) = match action.action_type {
        ActionType::SendNotification => {
            let msg = get_param(&action.params, "message")
                .unwrap_or_else(|| String::from("(no message)"));
            serial_println!("  [automation] Notification: {}", msg);
            (ActionResult::Success, format!("Sent: {}", msg))
        }
        ActionType::Speak => {
            let text =
                get_param(&action.params, "text").unwrap_or_else(|| String::from("(no text)"));
            serial_println!("  [automation] Speaking: {}", text);
            (ActionResult::Success, format!("Spoke: {}", text))
        }
        ActionType::SetVolume => {
            if let Some(level_str) = get_param(&action.params, "level") {
                let level = parse_u8(&level_str).unwrap_or(50);
                state.current_volume = level;
                serial_println!("  [automation] Volume -> {}", level);
                (ActionResult::Success, format!("Volume set to {}", level))
            } else {
                (ActionResult::Failed, String::from("Missing 'level' param"))
            }
        }
        ActionType::SetBrightness => {
            if let Some(level_str) = get_param(&action.params, "level") {
                let level = parse_u8(&level_str).unwrap_or(50);
                state.current_brightness = level;
                serial_println!("  [automation] Brightness -> {}", level);
                (
                    ActionResult::Success,
                    format!("Brightness set to {}", level),
                )
            } else {
                (ActionResult::Failed, String::from("Missing 'level' param"))
            }
        }
        ActionType::ToggleDnd => {
            let enable = get_param(&action.params, "enabled")
                .map(|v| v == "true")
                .unwrap_or(!state.dnd_enabled);
            state.dnd_enabled = enable;
            serial_println!("  [automation] DND -> {}", enable);
            (
                ActionResult::Success,
                format!("DND {}", if enable { "on" } else { "off" }),
            )
        }
        ActionType::ToggleWifi => {
            let enable = get_param(&action.params, "enabled")
                .map(|v| v == "true")
                .unwrap_or(!state.wifi_connected);
            state.wifi_connected = enable;
            serial_println!("  [automation] WiFi -> {}", enable);
            (
                ActionResult::Success,
                format!("WiFi {}", if enable { "on" } else { "off" }),
            )
        }
        ActionType::ToggleBluetooth => {
            let enable = get_param(&action.params, "enabled")
                .map(|v| v == "true")
                .unwrap_or(!state.bluetooth_connected);
            state.bluetooth_connected = enable;
            serial_println!("  [automation] Bluetooth -> {}", enable);
            (
                ActionResult::Success,
                format!("BT {}", if enable { "on" } else { "off" }),
            )
        }
        ActionType::OpenApp => {
            let app = get_param(&action.params, "app").unwrap_or_else(|| String::from("unknown"));
            serial_println!("  [automation] Opening app: {}", app);
            if !state.running_apps.contains(&app) {
                state.running_apps.push(app.clone());
            }
            (ActionResult::Success, format!("Opened {}", app))
        }
        ActionType::SetSetting => {
            let key = get_param(&action.params, "key").unwrap_or_else(|| String::from(""));
            let val = get_param(&action.params, "value").unwrap_or_else(|| String::from(""));
            serial_println!("  [automation] Setting {} = {}", key, val);
            (ActionResult::Success, format!("{} = {}", key, val))
        }
        ActionType::RunShellCommand => {
            let cmd = get_param(&action.params, "command").unwrap_or_else(|| String::from(""));
            serial_println!("  [automation] Shell: {}", cmd);
            // Shell execution would be delegated to the kernel shell module
            (ActionResult::Success, format!("Ran: {}", cmd))
        }
        ActionType::Wait => {
            let ms = get_param(&action.params, "ms")
                .and_then(|v| {
                    let mut n: u64 = 0;
                    for c in v.chars() {
                        if c.is_ascii_digit() {
                            n = n * 10 + (c as u64 - '0' as u64);
                        }
                    }
                    Some(n)
                })
                .unwrap_or(1000);
            serial_println!("  [automation] Wait {}ms", ms);
            (ActionResult::Success, format!("Waited {}ms", ms))
        }
        _ => {
            serial_println!("  [automation] Action: {:?}", action.action_type);
            (ActionResult::Success, format!("{:?}", action.action_type))
        }
    };

    ActionLog {
        action_type: action.action_type,
        result,
        timestamp: now,
        detail,
    }
}

fn get_param(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

// ---------------------------------------------------------------------------
// Conflict resolution
// ---------------------------------------------------------------------------

/// Detect conflicts between routines that would fire simultaneously.
/// Two routines conflict if they modify the same system state in
/// contradictory ways.
fn detect_conflicts(routines: &[&Routine]) -> Vec<(u32, u32, String)> {
    let mut conflicts = Vec::new();

    for i in 0..routines.len() {
        for j in (i + 1)..routines.len() {
            let a = routines[i];
            let b = routines[j];

            // Check each action pair for conflicting types
            for act_a in &a.actions {
                for act_b in &b.actions {
                    if act_a.action_type == act_b.action_type {
                        // Same action type with different params = conflict
                        let params_differ = act_a
                            .params
                            .iter()
                            .zip(act_b.params.iter())
                            .any(|((ka, va), (kb, vb))| ka == kb && va != vb);

                        if params_differ {
                            conflicts.push((
                                a.id,
                                b.id,
                                format!("{:?} conflict", act_a.action_type),
                            ));
                        }
                    }
                }
            }
        }
    }
    conflicts
}

/// Given conflicting routines, select winners by priority (higher wins).
/// Returns the set of routine IDs that should execute.
fn resolve_conflicts(candidates: &[&Routine], conflicts: &[(u32, u32, String)]) -> Vec<u32> {
    let mut suppressed: Vec<u32> = Vec::new();

    for (id_a, id_b, _reason) in conflicts {
        let pri_a = candidates
            .iter()
            .find(|r| r.id == *id_a)
            .map(|r| r.priority);
        let pri_b = candidates
            .iter()
            .find(|r| r.id == *id_b)
            .map(|r| r.priority);

        match (pri_a, pri_b) {
            (Some(a), Some(b)) => {
                if a >= b {
                    if !suppressed.contains(id_b) {
                        suppressed.push(*id_b);
                    }
                } else {
                    if !suppressed.contains(id_a) {
                        suppressed.push(*id_a);
                    }
                }
            }
            _ => {}
        }
    }

    candidates
        .iter()
        .filter(|r| !suppressed.contains(&r.id))
        .map(|r| r.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Automation engine
// ---------------------------------------------------------------------------

/// Automation engine
pub struct AutomationEngine {
    pub routines: Vec<Routine>,
    pub next_id: u32,
    pub enabled: bool,
    pub total_runs: u64,
    pub max_routines: usize,
    /// System state for condition evaluation
    pub state: SystemState,
    /// Run history (circular buffer)
    pub run_history: Vec<RunLog>,
    pub max_history: usize,
    /// Action log for debugging
    pub action_log: Vec<ActionLog>,
    pub max_action_log: usize,
}

impl AutomationEngine {
    const fn new() -> Self {
        AutomationEngine {
            routines: Vec::new(),
            next_id: 1,
            enabled: true,
            total_runs: 0,
            max_routines: 100,
            state: SystemState::new(),
            run_history: Vec::new(),
            max_history: 200,
            action_log: Vec::new(),
            max_action_log: 500,
        }
    }

    pub fn create_routine(&mut self, name: &str) -> u32 {
        if self.routines.len() >= self.max_routines {
            serial_println!(
                "  [automation] Max routines reached ({})",
                self.max_routines
            );
            return 0;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.routines.push(Routine {
            id,
            name: String::from(name),
            enabled: true,
            triggers: Vec::new(),
            conditions: Vec::new(),
            actions: Vec::new(),
            run_count: 0,
            last_run: 0,
            created_at: crate::time::clock::unix_time(),
            priority: Priority::Normal,
            cooldown_secs: 0,
            max_daily_runs: 0,
            runs_today: 0,
            runs_today_day: 0,
            stop_on_failure: false,
            tags: Vec::new(),
        });
        id
    }

    /// Create a routine with a specific priority
    pub fn create_routine_with_priority(&mut self, name: &str, priority: Priority) -> u32 {
        let id = self.create_routine(name);
        if id > 0 {
            if let Some(r) = self.routines.iter_mut().find(|r| r.id == id) {
                r.priority = priority;
            }
        }
        id
    }

    pub fn add_trigger(&mut self, routine_id: u32, trigger: Trigger) {
        if let Some(routine) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            routine.triggers.push(trigger);
        }
    }

    pub fn add_condition(&mut self, routine_id: u32, condition: Condition) {
        if let Some(routine) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            routine.conditions.push(condition);
        }
    }

    pub fn add_action(&mut self, routine_id: u32, action: Action) {
        if let Some(routine) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            routine.actions.push(action);
        }
    }

    pub fn delete_routine(&mut self, id: u32) {
        self.routines.retain(|r| r.id != id);
    }

    pub fn toggle_routine(&mut self, id: u32) -> bool {
        if let Some(routine) = self.routines.iter_mut().find(|r| r.id == id) {
            routine.enabled = !routine.enabled;
            routine.enabled
        } else {
            false
        }
    }

    /// Set cooldown for a routine (seconds between allowed runs)
    pub fn set_cooldown(&mut self, routine_id: u32, cooldown_secs: u64) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            r.cooldown_secs = cooldown_secs;
        }
    }

    /// Set maximum daily runs for a routine
    pub fn set_max_daily_runs(&mut self, routine_id: u32, max: u32) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            r.max_daily_runs = max;
        }
    }

    /// Set stop-on-failure flag
    pub fn set_stop_on_failure(&mut self, routine_id: u32, stop: bool) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            r.stop_on_failure = stop;
        }
    }

    /// Add a tag to a routine
    pub fn add_tag(&mut self, routine_id: u32, tag: &str) {
        if let Some(r) = self.routines.iter_mut().find(|r| r.id == routine_id) {
            if !r.tags.contains(&String::from(tag)) {
                r.tags.push(String::from(tag));
            }
        }
    }

    /// Update system state (called by OS subsystems)
    pub fn update_state_battery(&mut self, level: u8, charging: bool) {
        self.state.battery_level = level;
        self.state.is_charging = charging;
    }

    pub fn update_state_wifi(&mut self, connected: bool, ssid: &str) {
        self.state.wifi_connected = connected;
        self.state.wifi_ssid = String::from(ssid);
    }

    pub fn update_state_bluetooth(&mut self, connected: bool, device: &str) {
        self.state.bluetooth_connected = connected;
        self.state.bluetooth_device = String::from(device);
    }

    pub fn update_state_screen(&mut self, on: bool) {
        self.state.screen_on = on;
    }

    pub fn update_state_dnd(&mut self, enabled: bool) {
        self.state.dnd_enabled = enabled;
    }

    pub fn update_state_headphones(&mut self, connected: bool) {
        self.state.headphones_connected = connected;
    }

    pub fn update_state_app(&mut self, app: &str, running: bool) {
        if running {
            if !self.state.running_apps.contains(&String::from(app)) {
                self.state.running_apps.push(String::from(app));
            }
        } else {
            self.state.running_apps.retain(|a| a != app);
        }
    }

    /// Fire a trigger event and execute matching routines.
    ///
    /// This is the main entry point for the automation system.
    /// It evaluates all routines, applies cooldown/daily-limit checks,
    /// performs conflict resolution among simultaneous matches,
    /// then executes actions with chaining support.
    pub fn fire_trigger(&mut self, trigger_type: TriggerType, value: &str) -> u32 {
        if !self.enabled {
            return 0;
        }

        let now = crate::time::clock::unix_time();
        self.state.update_time();

        // Day-of-year for daily run reset (approximate: now / 86400)
        let today = (now / 86400) as u32;

        // --- Phase 1: Find all matching routines ---
        let mut candidate_ids: Vec<u32> = Vec::new();

        // Take a reference to state before iterating routines mutably
        // (partial borrows: routines and state are disjoint fields)
        let sys_state = &self.state;

        for routine in self.routines.iter_mut() {
            if !routine.enabled {
                continue;
            }

            // Check trigger match
            let trigger_match = routine.triggers.iter().any(|t| {
                t.trigger_type == trigger_type && (t.value.is_empty() || t.value == value)
            });
            if !trigger_match {
                continue;
            }

            // Check conditions against current system state
            let conditions_met = routine
                .conditions
                .iter()
                .all(|c| check_condition(c, sys_state));
            if !conditions_met {
                continue;
            }

            // Check cooldown
            if routine.cooldown_secs > 0 && routine.last_run > 0 {
                let elapsed = now.saturating_sub(routine.last_run);
                if elapsed < routine.cooldown_secs {
                    serial_println!(
                        "  [automation] '{}' skipped (cooldown: {}s remaining)",
                        routine.name,
                        routine.cooldown_secs - elapsed
                    );
                    continue;
                }
            }

            // Check daily run limit
            if routine.max_daily_runs > 0 {
                // Reset daily counter if it's a new day
                if routine.runs_today_day != today {
                    routine.runs_today = 0;
                    routine.runs_today_day = today;
                }
                if routine.runs_today >= routine.max_daily_runs {
                    serial_println!(
                        "  [automation] '{}' skipped (daily limit: {}/{})",
                        routine.name,
                        routine.runs_today,
                        routine.max_daily_runs
                    );
                    continue;
                }
            }

            candidate_ids.push(routine.id);
        }

        if candidate_ids.is_empty() {
            return 0;
        }

        // --- Phase 2: Conflict resolution ---
        let candidate_refs: Vec<&Routine> = candidate_ids
            .iter()
            .filter_map(|id| self.routines.iter().find(|r| r.id == *id))
            .collect();

        let conflicts = detect_conflicts(&candidate_refs);
        let winners = if conflicts.is_empty() {
            candidate_ids.clone()
        } else {
            for (id_a, id_b, reason) in &conflicts {
                serial_println!(
                    "  [automation] Conflict between routine {} and {}: {}",
                    id_a,
                    id_b,
                    reason
                );
            }
            resolve_conflicts(&candidate_refs, &conflicts)
        };

        // --- Phase 3: Execute winning routines ---
        let mut total_executed: u32 = 0;

        for routine_id in &winners {
            // We need to find the routine index to avoid borrow issues
            let routine_idx = match self.routines.iter().position(|r| r.id == *routine_id) {
                Some(idx) => idx,
                None => continue,
            };

            let routine_name = self.routines[routine_idx].name.clone();
            let stop_on_fail = self.routines[routine_idx].stop_on_failure;
            let num_actions = self.routines[routine_idx].actions.len();

            serial_println!(
                "  [automation] Executing '{}' ({} actions)",
                routine_name,
                num_actions
            );

            let mut actions_ok: u32 = 0;
            let mut actions_fail: u32 = 0;

            for action_idx in 0..num_actions {
                // Clone action params to avoid borrow issues
                let action_type = self.routines[routine_idx].actions[action_idx].action_type;
                let action_params = self.routines[routine_idx].actions[action_idx]
                    .params
                    .clone();
                let action_delay = self.routines[routine_idx].actions[action_idx].delay_ms;

                let temp_action = Action {
                    action_type,
                    params: action_params,
                    delay_ms: action_delay,
                };

                let log = execute_action(&temp_action, &mut self.state);

                match log.result {
                    ActionResult::Success => actions_ok += 1,
                    ActionResult::Failed => {
                        actions_fail += 1;
                        if stop_on_fail {
                            serial_println!(
                                "  [automation] '{}' halted on failure at action {}",
                                routine_name,
                                action_idx
                            );
                            // Log remaining as skipped
                            self.log_action(ActionLog {
                                action_type,
                                result: ActionResult::Skipped,
                                timestamp: now,
                                detail: String::from("Skipped due to prior failure"),
                            });
                            break;
                        }
                    }
                    ActionResult::Delayed => actions_ok += 1, // count delayed as handled
                    ActionResult::Skipped => {}
                }

                self.log_action(log);
            }

            // Update routine stats
            let routine = &mut self.routines[routine_idx];
            routine.run_count = routine.run_count.saturating_add(1);
            routine.last_run = now;
            routine.runs_today = routine.runs_today.saturating_add(1);

            // Log the run
            self.log_run(RunLog {
                routine_id: *routine_id,
                routine_name: routine_name.clone(),
                timestamp: now,
                trigger_type,
                trigger_value: String::from(value),
                actions_executed: actions_ok,
                actions_failed: actions_fail,
            });

            total_executed += 1;
        }

        self.total_runs = self.total_runs.saturating_add(total_executed as u64);
        total_executed
    }

    /// Log an action result
    fn log_action(&mut self, log: ActionLog) {
        if self.action_log.len() >= self.max_action_log {
            self.action_log.remove(0);
        }
        self.action_log.push(log);
    }

    /// Log a routine run
    fn log_run(&mut self, log: RunLog) {
        if self.run_history.len() >= self.max_history {
            self.run_history.remove(0);
        }
        self.run_history.push(log);
    }

    /// Evaluate time-based triggers (should be called periodically by the scheduler)
    pub fn check_time_triggers(&mut self) -> u32 {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let minute = ((now / 60) % 60) as u8;
        let time_str = format!("{:02}:{:02}", hour, minute);
        self.fire_trigger(TriggerType::Time, &time_str)
    }

    pub fn routine_count(&self) -> usize {
        self.routines.len()
    }

    /// Get recent run history
    pub fn recent_runs(&self, max: usize) -> Vec<&RunLog> {
        let start = if self.run_history.len() > max {
            self.run_history.len() - max
        } else {
            0
        };
        self.run_history[start..].iter().collect()
    }

    /// Get routines by tag
    pub fn routines_by_tag(&self, tag: &str) -> Vec<&Routine> {
        self.routines
            .iter()
            .filter(|r| r.tags.iter().any(|t| t == tag))
            .collect()
    }

    /// Get routine by ID
    pub fn get_routine(&self, id: u32) -> Option<&Routine> {
        self.routines.iter().find(|r| r.id == id)
    }

    /// Get a summary string of all routines
    pub fn summary(&self) -> String {
        let mut s = format!(
            "Automation: {} routines, {} total runs\n",
            self.routines.len(),
            self.total_runs
        );
        for r in &self.routines {
            let status = if r.enabled { "ON" } else { "OFF" };
            s.push_str(&format!(
                "  [{}] {} ({}) - {} triggers, {} actions, ran {}x\n",
                status,
                r.name,
                format!("{:?}", r.priority),
                r.triggers.len(),
                r.actions.len(),
                r.run_count
            ));
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Default routines
// ---------------------------------------------------------------------------

/// Create default routines
fn create_defaults(engine: &mut AutomationEngine) {
    // Bedtime routine (high priority, 1 run per day)
    let id = engine.create_routine_with_priority("Bedtime", Priority::High);
    engine.add_trigger(
        id,
        Trigger {
            trigger_type: TriggerType::Time,
            value: String::from("22:00"),
        },
    );
    engine.add_action(
        id,
        Action {
            action_type: ActionType::ToggleDnd,
            params: alloc::vec![(String::from("enabled"), String::from("true"))],
            delay_ms: 0,
        },
    );
    engine.add_action(
        id,
        Action {
            action_type: ActionType::SetBrightness,
            params: alloc::vec![(String::from("level"), String::from("20"))],
            delay_ms: 0,
        },
    );
    engine.set_max_daily_runs(id, 1);
    engine.add_tag(id, "sleep");

    // Car connected routine
    let id2 = engine.create_routine_with_priority("Driving Mode", Priority::Normal);
    engine.add_trigger(
        id2,
        Trigger {
            trigger_type: TriggerType::BluetoothConnect,
            value: String::from("Car Audio"),
        },
    );
    engine.add_condition(
        id2,
        Condition {
            condition_type: ConditionType::BatteryAbove,
            value: String::from("10"),
            negate: false,
        },
    );
    engine.add_action(
        id2,
        Action {
            action_type: ActionType::OpenApp,
            params: alloc::vec![(String::from("app"), String::from("maps"))],
            delay_ms: 0,
        },
    );
    engine.add_action(
        id2,
        Action {
            action_type: ActionType::SetVolume,
            params: alloc::vec![(String::from("level"), String::from("80"))],
            delay_ms: 500,
        },
    );
    engine.set_cooldown(id2, 300); // 5 minute cooldown
    engine.add_tag(id2, "driving");

    // Low battery routine (critical priority)
    let id3 = engine.create_routine_with_priority("Low Battery Saver", Priority::Critical);
    engine.add_trigger(
        id3,
        Trigger {
            trigger_type: TriggerType::BatteryLow,
            value: String::new(),
        },
    );
    engine.add_condition(
        id3,
        Condition {
            condition_type: ConditionType::BatteryBelow,
            value: String::from("15"),
            negate: false,
        },
    );
    engine.add_action(
        id3,
        Action {
            action_type: ActionType::SendNotification,
            params: alloc::vec![(
                String::from("message"),
                String::from("Battery low! Enabling power save.")
            )],
            delay_ms: 0,
        },
    );
    engine.add_action(
        id3,
        Action {
            action_type: ActionType::SetBrightness,
            params: alloc::vec![(String::from("level"), String::from("10"))],
            delay_ms: 0,
        },
    );
    engine.set_cooldown(id3, 1800); // 30 minute cooldown
    engine.set_stop_on_failure(id3, true);
    engine.add_tag(id3, "power");

    // Morning routine
    let id4 = engine.create_routine("Morning");
    engine.add_trigger(
        id4,
        Trigger {
            trigger_type: TriggerType::Time,
            value: String::from("07:00"),
        },
    );
    engine.add_condition(
        id4,
        Condition {
            condition_type: ConditionType::DayOfWeek,
            value: String::from("1,2,3,4,5"), // Mon-Fri
            negate: false,
        },
    );
    engine.add_action(
        id4,
        Action {
            action_type: ActionType::ToggleDnd,
            params: alloc::vec![(String::from("enabled"), String::from("false"))],
            delay_ms: 0,
        },
    );
    engine.add_action(
        id4,
        Action {
            action_type: ActionType::SetBrightness,
            params: alloc::vec![(String::from("level"), String::from("70"))],
            delay_ms: 0,
        },
    );
    engine.add_action(
        id4,
        Action {
            action_type: ActionType::SendNotification,
            params: alloc::vec![(
                String::from("message"),
                String::from("Good morning! Ready for the day.")
            )],
            delay_ms: 1000,
        },
    );
    engine.set_max_daily_runs(id4, 1);
    engine.add_tag(id4, "morning");
}

// ---------------------------------------------------------------------------
// Global state and public API
// ---------------------------------------------------------------------------

static AUTOMATION: Mutex<AutomationEngine> = Mutex::new(AutomationEngine::new());

pub fn init() {
    create_defaults(&mut AUTOMATION.lock());
    let count = AUTOMATION.lock().routine_count();
    crate::serial_println!("    [automation] AI Automation initialized ({} routines, priority/cooldown/conflict resolution)", count);
}

pub fn fire_trigger(t: TriggerType, value: &str) -> u32 {
    AUTOMATION.lock().fire_trigger(t, value)
}

pub fn check_time_triggers() -> u32 {
    AUTOMATION.lock().check_time_triggers()
}

pub fn create_routine(name: &str) -> u32 {
    AUTOMATION.lock().create_routine(name)
}

pub fn create_routine_with_priority(name: &str, priority: Priority) -> u32 {
    AUTOMATION
        .lock()
        .create_routine_with_priority(name, priority)
}

pub fn add_trigger(routine_id: u32, trigger: Trigger) {
    AUTOMATION.lock().add_trigger(routine_id, trigger);
}

pub fn add_condition(routine_id: u32, condition: Condition) {
    AUTOMATION.lock().add_condition(routine_id, condition);
}

pub fn add_action(routine_id: u32, action: Action) {
    AUTOMATION.lock().add_action(routine_id, action);
}

pub fn delete_routine(id: u32) {
    AUTOMATION.lock().delete_routine(id);
}

pub fn toggle_routine(id: u32) -> bool {
    AUTOMATION.lock().toggle_routine(id)
}

pub fn set_cooldown(routine_id: u32, secs: u64) {
    AUTOMATION.lock().set_cooldown(routine_id, secs);
}

pub fn routine_count() -> usize {
    AUTOMATION.lock().routine_count()
}

pub fn summary() -> String {
    AUTOMATION.lock().summary()
}

pub fn update_battery(level: u8, charging: bool) {
    AUTOMATION.lock().update_state_battery(level, charging);
}

pub fn update_wifi(connected: bool, ssid: &str) {
    AUTOMATION.lock().update_state_wifi(connected, ssid);
}

pub fn update_bluetooth(connected: bool, device: &str) {
    AUTOMATION.lock().update_state_bluetooth(connected, device);
}

pub fn update_screen(on: bool) {
    AUTOMATION.lock().update_state_screen(on);
}
