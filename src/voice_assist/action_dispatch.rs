use super::command_parser::{Intent, IntentAction};
use crate::sync::Mutex;
/// Action dispatcher for Genesis OS voice assistant
///
/// Translates parsed intents into concrete OS actions:
///   - Opening/closing applications
///   - Searching files and content
///   - Controlling media playback
///   - Adjusting system settings
///   - Navigating the UI
///   - Creating/deleting resources
///   - Managing timers, alarms, and reminders
///
/// Includes confirmation flow for destructive actions
/// and a pending-action queue for deferred execution.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of actions in the pending queue
const MAX_PENDING_ACTIONS: usize = 32;

/// Hash sentinel for "success" response
const RESPONSE_SUCCESS: u64 = 0xABCD1234ABCD1234;
/// Hash sentinel for "failure" response
const RESPONSE_FAILURE: u64 = 0xDEADBEEFDEADBEEF;
/// Hash sentinel for "needs confirmation"
const RESPONSE_CONFIRM: u64 = 0xC0C0C0C0A1A1A1A1;
/// Hash sentinel for "not found"
const RESPONSE_NOT_FOUND: u64 = 0x404404404404AAAA;
/// Hash sentinel for "queued"
const RESPONSE_QUEUED: u64 = 0xAAAABBBBCCCCDDDD;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of dispatching an action
#[derive(Debug, Clone)]
pub struct ActionResult {
    /// Whether the action completed successfully
    pub success: bool,
    /// Hash identifying the response message to speak
    pub response_hash: u64,
    /// Hash of any data payload returned by the action
    pub data_hash: u64,
    /// Whether a follow-up interaction is expected
    pub follow_up: bool,
}

/// Priority level for queued actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// A pending action waiting in the queue
#[derive(Debug, Clone)]
pub struct PendingAction {
    /// The original intent
    pub intent: Intent,
    /// When this action was queued (kernel tick)
    pub queued_at: u64,
    /// Priority
    pub priority: ActionPriority,
    /// Whether confirmation has been received
    pub confirmed: bool,
    /// Number of retry attempts
    pub retries: u8,
}

/// Category of actions requiring confirmation before execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmCategory {
    /// No confirmation needed
    None,
    /// Verbal confirmation ("Are you sure?")
    Verbal,
    /// Strong confirmation for destructive operations
    Strong,
}

/// The dispatcher maps intents to system calls and manages the action queue.
pub struct Dispatcher {
    /// Queue of pending actions
    pending: Vec<PendingAction>,
    /// History of recent action results (circular, last N)
    history: Vec<ActionResult>,
    /// Write position in history
    history_pos: usize,
    /// Maximum history entries
    max_history: usize,
    /// Total actions dispatched
    total_dispatched: u64,
    /// Total actions succeeded
    total_succeeded: u64,
    /// Current kernel tick (updated externally)
    current_tick: u64,
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

static DISPATCHER: Mutex<Option<Dispatcher>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl ActionResult {
    fn success(response_hash: u64, data_hash: u64) -> Self {
        ActionResult {
            success: true,
            response_hash,
            data_hash,
            follow_up: false,
        }
    }

    fn failure(response_hash: u64) -> Self {
        ActionResult {
            success: false,
            response_hash,
            data_hash: 0,
            follow_up: false,
        }
    }

    fn needs_confirmation() -> Self {
        ActionResult {
            success: true,
            response_hash: RESPONSE_CONFIRM,
            data_hash: 0,
            follow_up: true,
        }
    }

    fn queued() -> Self {
        ActionResult {
            success: true,
            response_hash: RESPONSE_QUEUED,
            data_hash: 0,
            follow_up: false,
        }
    }
}

impl Dispatcher {
    /// Create a new dispatcher with empty queue and history.
    pub fn new() -> Self {
        let mut history = Vec::new();
        history.resize(
            64,
            ActionResult {
                success: false,
                response_hash: 0,
                data_hash: 0,
                follow_up: false,
            },
        );

        Dispatcher {
            pending: Vec::new(),
            history,
            history_pos: 0,
            max_history: 64,
            total_dispatched: 0,
            total_succeeded: 0,
            current_tick: 0,
        }
    }

    /// Primary dispatch entry point. Routes the intent to the appropriate handler.
    pub fn dispatch(&mut self, intent: &Intent) -> ActionResult {
        self.total_dispatched = self.total_dispatched.saturating_add(1);

        // Check if confirmation is required
        let confirm_cat = self.requires_confirmation(intent);
        match confirm_cat {
            ConfirmCategory::Strong => {
                // Queue and ask for confirmation
                self.queue_action(intent.clone(), ActionPriority::Normal);
                return ActionResult::needs_confirmation();
            }
            ConfirmCategory::Verbal => {
                // Lighter confirmation — queue but flag follow-up
                self.queue_action(intent.clone(), ActionPriority::Normal);
                let mut result = ActionResult::needs_confirmation();
                result.response_hash = RESPONSE_CONFIRM;
                return result;
            }
            ConfirmCategory::None => {}
        }

        let result = match intent.action {
            IntentAction::Open => self.execute_open(intent),
            IntentAction::Close => self.execute_close(intent),
            IntentAction::Search => self.execute_search(intent),
            IntentAction::Play => self.execute_play(intent),
            IntentAction::Pause => self.execute_pause(intent),
            IntentAction::Call => self.execute_call(intent),
            IntentAction::Message => self.execute_message(intent),
            IntentAction::Set => self.execute_set(intent),
            IntentAction::Navigate => self.execute_navigate(intent),
            IntentAction::Ask => self.execute_ask(intent),
            IntentAction::Create => self.execute_create(intent),
            IntentAction::Delete => self.execute_delete(intent),
            IntentAction::Toggle => self.execute_toggle(intent),
            IntentAction::Timer => self.execute_timer(intent),
            IntentAction::Alarm => self.execute_alarm(intent),
            IntentAction::Remind => self.execute_remind(intent),
        };

        if result.success {
            self.total_succeeded = self.total_succeeded.saturating_add(1);
        }

        // Record in history
        self.history[self.history_pos] = result.clone();
        self.history_pos = (self.history_pos + 1) % self.max_history;

        result
    }

    /// Open an application or resource identified by target_hash.
    pub fn execute_open(&self, intent: &Intent) -> ActionResult {
        let target = intent.target_hash;
        if target == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // In a full implementation this would call into the process/app subsystem:
        //   crate::app::launch_by_hash(target)
        serial_println!("    [action_dispatch] OPEN target={:#X}", target);
        ActionResult::success(RESPONSE_SUCCESS, target)
    }

    /// Close an application or resource.
    fn execute_close(&self, intent: &Intent) -> ActionResult {
        let target = intent.target_hash;
        if target == 0 {
            // Close the focused application
            serial_println!("    [action_dispatch] CLOSE focused app");
            return ActionResult::success(RESPONSE_SUCCESS, 0);
        }
        serial_println!("    [action_dispatch] CLOSE target={:#X}", target);
        ActionResult::success(RESPONSE_SUCCESS, target)
    }

    /// Search for files, content, or applications.
    pub fn execute_search(&self, intent: &Intent) -> ActionResult {
        let query_hash = intent.target_hash;
        if query_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // Would call crate::search::query_by_hash(query_hash)
        serial_println!("    [action_dispatch] SEARCH query={:#X}", query_hash);

        // Extract additional search parameters
        let mut scope_hash: u64 = 0;
        for &(key, val) in &intent.params {
            if key != 0 {
                scope_hash = val;
                break;
            }
        }

        ActionResult::success(RESPONSE_SUCCESS, query_hash)
    }

    /// Start media playback.
    pub fn execute_play(&self, intent: &Intent) -> ActionResult {
        let media_hash = intent.target_hash;
        // Would call crate::media::play(media_hash)
        serial_println!("    [action_dispatch] PLAY media={:#X}", media_hash);
        ActionResult::success(RESPONSE_SUCCESS, media_hash)
    }

    /// Pause media playback.
    fn execute_pause(&self, _intent: &Intent) -> ActionResult {
        // Would call crate::media::pause()
        serial_println!("    [action_dispatch] PAUSE");
        ActionResult::success(RESPONSE_SUCCESS, 0)
    }

    /// Initiate a call.
    fn execute_call(&self, intent: &Intent) -> ActionResult {
        let contact_hash = intent.target_hash;
        if contact_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // Would call crate::telephony::dial(contact_hash)
        serial_println!("    [action_dispatch] CALL contact={:#X}", contact_hash);
        ActionResult::success(RESPONSE_SUCCESS, contact_hash)
    }

    /// Send a message.
    fn execute_message(&self, intent: &Intent) -> ActionResult {
        let recipient_hash = intent.target_hash;
        if recipient_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // Extract message body hash from params
        let mut body_hash: u64 = 0;
        for &(_, val) in &intent.params {
            if val != 0 {
                body_hash = val;
                break;
            }
        }
        serial_println!(
            "    [action_dispatch] MESSAGE to={:#X} body={:#X}",
            recipient_hash,
            body_hash
        );
        ActionResult::success(RESPONSE_SUCCESS, recipient_hash)
    }

    /// Adjust a system setting.
    pub fn execute_set(&self, intent: &Intent) -> ActionResult {
        let setting_hash = intent.target_hash;
        if setting_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // Extract the value from params
        let mut value_hash: u64 = 0;
        for &(_, val) in &intent.params {
            if val != 0 {
                value_hash = val;
                break;
            }
        }
        serial_println!(
            "    [action_dispatch] SET setting={:#X} value={:#X}",
            setting_hash,
            value_hash
        );
        ActionResult::success(RESPONSE_SUCCESS, setting_hash)
    }

    /// Navigate to a location in the UI or filesystem.
    pub fn execute_navigate(&self, intent: &Intent) -> ActionResult {
        let destination_hash = intent.target_hash;
        if destination_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        serial_println!(
            "    [action_dispatch] NAVIGATE dest={:#X}",
            destination_hash
        );
        ActionResult::success(RESPONSE_SUCCESS, destination_hash)
    }

    /// Handle a question / knowledge query.
    fn execute_ask(&self, intent: &Intent) -> ActionResult {
        let question_hash = intent.target_hash;
        // Would call crate::ai::assistant::query(question_hash)
        serial_println!("    [action_dispatch] ASK question={:#X}", question_hash);
        let mut result = ActionResult::success(RESPONSE_SUCCESS, question_hash);
        result.follow_up = true; // AI response expected
        result
    }

    /// Create a new resource (file, note, event, etc.).
    pub fn execute_create(&self, intent: &Intent) -> ActionResult {
        let resource_hash = intent.target_hash;
        if resource_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        serial_println!("    [action_dispatch] CREATE resource={:#X}", resource_hash);
        ActionResult::success(RESPONSE_SUCCESS, resource_hash)
    }

    /// Delete a resource. This is a destructive operation.
    fn execute_delete(&self, intent: &Intent) -> ActionResult {
        let resource_hash = intent.target_hash;
        if resource_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        // At this point, confirmation has already been handled by dispatch()
        serial_println!("    [action_dispatch] DELETE resource={:#X}", resource_hash);
        ActionResult::success(RESPONSE_SUCCESS, resource_hash)
    }

    /// Toggle a boolean setting (wifi, bluetooth, dark mode, etc.).
    fn execute_toggle(&self, intent: &Intent) -> ActionResult {
        let feature_hash = intent.target_hash;
        if feature_hash == 0 {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        serial_println!("    [action_dispatch] TOGGLE feature={:#X}", feature_hash);
        ActionResult::success(RESPONSE_SUCCESS, feature_hash)
    }

    /// Set a countdown timer.
    fn execute_timer(&self, intent: &Intent) -> ActionResult {
        // Extract duration from params
        let mut duration_hash: u64 = 0;
        for &(_, val) in &intent.params {
            if val != 0 {
                duration_hash = val;
                break;
            }
        }
        serial_println!("    [action_dispatch] TIMER duration={:#X}", duration_hash);
        ActionResult::success(RESPONSE_SUCCESS, duration_hash)
    }

    /// Set an alarm.
    fn execute_alarm(&self, intent: &Intent) -> ActionResult {
        let mut time_hash: u64 = 0;
        for &(_, val) in &intent.params {
            if val != 0 {
                time_hash = val;
                break;
            }
        }
        serial_println!("    [action_dispatch] ALARM time={:#X}", time_hash);
        ActionResult::success(RESPONSE_SUCCESS, time_hash)
    }

    /// Set a reminder.
    fn execute_remind(&self, intent: &Intent) -> ActionResult {
        let reminder_hash = intent.target_hash;
        let mut when_hash: u64 = 0;
        for &(key, val) in &intent.params {
            if key != 0 {
                when_hash = val;
                break;
            }
        }
        serial_println!(
            "    [action_dispatch] REMIND what={:#X} when={:#X}",
            reminder_hash,
            when_hash
        );
        ActionResult::success(RESPONSE_SUCCESS, reminder_hash)
    }

    /// Confirm a pending action by index (from get_pending list).
    pub fn confirm_action(&mut self, index: usize) -> ActionResult {
        if index >= self.pending.len() {
            return ActionResult::failure(RESPONSE_NOT_FOUND);
        }
        self.pending[index].confirmed = true;
        let action = self.pending.remove(index);
        // Re-dispatch now that it is confirmed — use the inner handlers directly
        let result = match action.intent.action {
            IntentAction::Delete => self.execute_delete(&action.intent),
            IntentAction::Close => self.execute_close(&action.intent),
            _ => self.dispatch_confirmed(&action.intent),
        };

        if result.success {
            self.total_succeeded = self.total_succeeded.saturating_add(1);
        }
        result
    }

    /// Queue an action for deferred execution.
    pub fn queue_action(&mut self, intent: Intent, priority: ActionPriority) {
        if self.pending.len() >= MAX_PENDING_ACTIONS {
            // Drop the oldest low-priority action
            let mut drop_idx: Option<usize> = None;
            for (i, a) in self.pending.iter().enumerate() {
                if a.priority == ActionPriority::Low {
                    drop_idx = Some(i);
                    break;
                }
            }
            if let Some(idx) = drop_idx {
                self.pending.remove(idx);
            } else {
                self.pending.remove(0);
            }
        }

        self.pending.push(PendingAction {
            intent,
            queued_at: self.current_tick,
            priority,
            confirmed: false,
            retries: 0,
        });
    }

    /// Return a snapshot of all pending actions.
    pub fn get_pending(&self) -> Vec<PendingAction> {
        self.pending.clone()
    }

    /// Update the dispatcher's view of the current kernel tick.
    pub fn update_tick(&mut self, tick: u64) {
        self.current_tick = tick;
    }

    /// Get statistics: (total_dispatched, total_succeeded, pending_count).
    pub fn stats(&self) -> (u64, u64, usize) {
        (
            self.total_dispatched,
            self.total_succeeded,
            self.pending.len(),
        )
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Determine what level of confirmation an intent requires.
    fn requires_confirmation(&self, intent: &Intent) -> ConfirmCategory {
        match intent.action {
            IntentAction::Delete => ConfirmCategory::Strong,
            IntentAction::Call => ConfirmCategory::Verbal,
            IntentAction::Message => {
                // Only confirm if confidence is low
                if intent.confidence < 49152 {
                    // < 0.75 Q16
                    ConfirmCategory::Verbal
                } else {
                    ConfirmCategory::None
                }
            }
            _ => ConfirmCategory::None,
        }
    }

    /// Dispatch an already-confirmed intent (bypasses confirmation check).
    fn dispatch_confirmed(&mut self, intent: &Intent) -> ActionResult {
        match intent.action {
            IntentAction::Open => self.execute_open(intent),
            IntentAction::Close => self.execute_close(intent),
            IntentAction::Search => self.execute_search(intent),
            IntentAction::Play => self.execute_play(intent),
            IntentAction::Pause => self.execute_pause(intent),
            IntentAction::Call => self.execute_call(intent),
            IntentAction::Message => self.execute_message(intent),
            IntentAction::Set => self.execute_set(intent),
            IntentAction::Navigate => self.execute_navigate(intent),
            IntentAction::Ask => self.execute_ask(intent),
            IntentAction::Create => self.execute_create(intent),
            IntentAction::Delete => self.execute_delete(intent),
            IntentAction::Toggle => self.execute_toggle(intent),
            IntentAction::Timer => self.execute_timer(intent),
            IntentAction::Alarm => self.execute_alarm(intent),
            IntentAction::Remind => self.execute_remind(intent),
        }
    }
}

// ---------------------------------------------------------------------------
// Public free functions (operate on global DISPATCHER)
// ---------------------------------------------------------------------------

/// Dispatch an intent using the global dispatcher.
pub fn dispatch(intent: &Intent) -> ActionResult {
    let mut guard = DISPATCHER.lock();
    match guard.as_mut() {
        Some(d) => d.dispatch(intent),
        None => ActionResult::failure(RESPONSE_FAILURE),
    }
}

/// Confirm a pending action at the given index.
pub fn confirm_action(index: usize) -> ActionResult {
    let mut guard = DISPATCHER.lock();
    match guard.as_mut() {
        Some(d) => d.confirm_action(index),
        None => ActionResult::failure(RESPONSE_FAILURE),
    }
}

/// Queue an intent for deferred execution.
pub fn queue_action(intent: Intent, priority: ActionPriority) {
    let mut guard = DISPATCHER.lock();
    if let Some(ref mut d) = *guard {
        d.queue_action(intent, priority);
    }
}

/// Get the list of pending actions.
pub fn get_pending() -> Vec<PendingAction> {
    let guard = DISPATCHER.lock();
    guard
        .as_ref()
        .map(|d| d.get_pending())
        .unwrap_or_else(Vec::new)
}

/// Get dispatcher statistics.
pub fn stats() -> (u64, u64, usize) {
    let guard = DISPATCHER.lock();
    guard.as_ref().map(|d| d.stats()).unwrap_or((0, 0, 0))
}

/// Initialize the global action dispatcher.
pub fn init() {
    *DISPATCHER.lock() = Some(Dispatcher::new());
    serial_println!("    [action_dispatch] Action dispatcher initialized");
}
