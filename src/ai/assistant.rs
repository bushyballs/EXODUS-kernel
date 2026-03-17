use crate::sync::Mutex;
/// Hoags Assistant — on-device AI assistant
///
/// A privacy-first assistant that:
///   - Understands natural language commands via keyword/pattern intent classification
///   - Tracks multi-turn conversation state with topic continuity
///   - Manages a context window of recent exchanges for coherent responses
///   - Helps manage files, settings, packages
///   - Answers questions from local knowledge
///   - Provides smart suggestions
///   - Never sends data off-device
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static ASSISTANT: Mutex<Option<Assistant>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Q16 fixed-point constants (for confidence scoring)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Assistant conversation state
pub struct Assistant {
    pub name: String,
    pub history: Vec<Message>,
    pub max_history: usize,
    pub system_prompt: String,
    pub personality: Personality,
    /// Current conversation topic for multi-turn tracking
    pub current_topic: Topic,
    /// Confidence of the current topic classification (Q16)
    pub topic_confidence: i32,
    /// Number of consecutive turns on the same topic
    pub topic_turns: u32,
    /// Entity slots extracted from conversation (key -> value)
    pub context_slots: BTreeMap<String, String>,
    /// Pending confirmation state for destructive actions
    pub pending_confirm: Option<PendingAction>,
    /// Topic history for back-tracking
    pub topic_history: Vec<Topic>,
    /// Intent match scores from last classification (for debugging / follow-ups)
    pub last_intent_scores: Vec<(Intent, i32)>,
    /// Turn counter
    pub turn_count: u64,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Personality {
    Professional,
    Friendly,
    Concise,
    Technical,
}

/// High-level conversation topic for multi-turn tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    None,
    FileManagement,
    PackageManagement,
    SystemInfo,
    Settings,
    Help,
    Search,
    General,
}

/// A pending destructive action awaiting user confirmation
#[derive(Debug, Clone)]
pub struct PendingAction {
    pub intent: Intent,
    pub description: String,
    pub created_turn: u64,
}

/// Intent classification for natural language commands
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    FileOperation(FileOp),
    SystemInfo,
    PackageManage(PkgOp),
    SettingsChange(String, String),
    Search(String),
    Help(String),
    Chat,
    Unknown,
    /// User is confirming a pending action
    Confirm,
    /// User is denying a pending action
    Deny,
    /// User is asking a follow-up about the previous topic
    FollowUp,
    /// User wants to undo last action
    Undo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    List(String),
    Create(String),
    Delete(String),
    Move(String, String),
    Search(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkgOp {
    Install(String),
    Remove(String),
    Update,
    Search(String),
}

// ---------------------------------------------------------------------------
// Keyword tables for intent classification
// ---------------------------------------------------------------------------

fn score_file_list(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = [
        "list ",
        "ls ",
        "show files",
        "dir ",
        "show directory",
        "what files",
        "what's in ",
    ];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("files") && lower.contains("in") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_file_create(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["create ", "make ", "touch ", "new file ", "create file "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("create") && lower.contains("file") {
        score += Q16_ONE / 2;
    }
    if lower.contains("make") && lower.contains("directory") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_file_delete(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["delete ", "remove file ", "rm ", "erase ", "del "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("delete") && lower.contains("file") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_file_move(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["move ", "mv ", "rename "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("move") && lower.contains("to") {
        score += Q16_ONE / 2;
    }
    if lower.contains("rename") && lower.contains("to") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_file_search(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["find ", "search for ", "locate ", "where is "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("find") && lower.contains("file") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_pkg_install(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["install ", "add package ", "get package "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("install") && lower.contains("package") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_pkg_remove(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let starters = ["uninstall ", "remove pkg ", "remove package "];
    for s in &starters {
        if lower.starts_with(s) {
            score += Q16_ONE;
        }
    }
    if lower.contains("uninstall") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_pkg_update(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower == "update" || lower == "update all" {
        score += Q16_ONE;
    }
    if lower.starts_with("update packages") || lower.starts_with("upgrade") {
        score += Q16_ONE;
    }
    if lower.contains("update") && lower.contains("package") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_pkg_search(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower.starts_with("search package") || lower.starts_with("find package") {
        score += Q16_ONE;
    }
    if lower.contains("available") && lower.contains("package") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_system_info(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower.contains("system info") || lower.contains("about this") || lower == "uname" {
        score += Q16_ONE;
    }
    if lower.contains("os version") || lower.contains("kernel version") {
        score += Q16_ONE;
    }
    if lower.contains("cpu") && (lower.contains("info") || lower.contains("what")) {
        score += Q16_ONE / 2;
    }
    if lower.contains("memory") && lower.contains("how much") {
        score += Q16_ONE / 2;
    }
    if lower.contains("uptime") {
        score += Q16_ONE / 2;
    }
    score
}

fn score_help(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower.starts_with("help") || lower.starts_with("how do i") {
        score += Q16_ONE;
    }
    if lower.starts_with("what is") || lower.starts_with("what are") {
        score += Q16_ONE / 2;
    }
    if lower.starts_with("explain") || lower.starts_with("how to") {
        score += Q16_ONE / 2;
    }
    if lower.contains("usage") || lower.contains("tutorial") {
        score += Q16_ONE / 4;
    }
    if lower == "?" {
        score += Q16_ONE;
    }
    score
}

fn score_settings(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower.starts_with("set ") {
        score += Q16_ONE;
    }
    if lower.starts_with("change ") && lower.contains("to") {
        score += Q16_ONE / 2;
    }
    if lower.starts_with("configure ") {
        score += Q16_ONE / 2;
    }
    if lower.contains("setting") {
        score += Q16_ONE / 4;
    }
    if lower.contains("brightness") || lower.contains("volume") || lower.contains("theme") {
        score += Q16_ONE / 4;
    }
    score
}

fn score_confirm(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let confirms = [
        "yes",
        "y",
        "confirm",
        "ok",
        "okay",
        "sure",
        "do it",
        "go ahead",
        "affirmative",
    ];
    for c in &confirms {
        if lower.trim() == *c {
            score += Q16_ONE;
        }
    }
    score
}

fn score_deny(lower: &str) -> i32 {
    let mut score: i32 = 0;
    let denials = [
        "no",
        "n",
        "cancel",
        "abort",
        "stop",
        "nevermind",
        "never mind",
        "nope",
        "don't",
    ];
    for d in &denials {
        if lower.trim() == *d {
            score += Q16_ONE;
        }
    }
    score
}

fn score_undo(lower: &str) -> i32 {
    let mut score: i32 = 0;
    if lower.starts_with("undo") || lower == "revert" {
        score += Q16_ONE;
    }
    if lower.contains("undo") && lower.contains("last") {
        score += Q16_ONE / 2;
    }
    score
}

// ---------------------------------------------------------------------------
// Context window helper
// ---------------------------------------------------------------------------

/// Extract a summary of recent conversation for context injection
fn build_context_summary(history: &[Message], max_turns: usize) -> String {
    let mut summary = String::new();
    let start = if history.len() > max_turns * 2 {
        history.len() - max_turns * 2
    } else {
        0
    };
    for msg in &history[start..] {
        let prefix = match msg.role {
            Role::User => "User",
            Role::Assistant => "Hoags",
            Role::System => continue,
        };
        // Truncate long messages in context
        let content = if msg.content.len() > 120 {
            let truncated: String = msg.content.chars().take(117).collect();
            format!("{}...", truncated)
        } else {
            msg.content.clone()
        };
        summary.push_str(prefix);
        summary.push_str(": ");
        summary.push_str(&content);
        summary.push('\n');
    }
    summary
}

/// Extract the argument portion of user input after a command prefix
fn extract_arg(input: &str, skip_words: usize) -> String {
    input
        .split_whitespace()
        .skip(skip_words)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse a "move X to Y" style command
fn parse_move_args(input: &str) -> (String, String) {
    let _lower = input.to_lowercase();
    // Try "move X to Y" pattern
    let parts: Vec<&str> = input.splitn(2, " to ").collect();
    if parts.len() == 2 {
        let src = extract_arg(parts[0], 1); // skip "move"
        let dst = String::from(parts[1].trim());
        return (src, dst);
    }
    // Try "mv X Y" pattern
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.len() >= 3 {
        return (String::from(words[1]), words[2..].join(" "));
    }
    (String::from(""), String::from(""))
}

/// Determine which topic an intent maps to
fn intent_to_topic(intent: &Intent) -> Topic {
    match intent {
        Intent::FileOperation(_) => Topic::FileManagement,
        Intent::PackageManage(_) => Topic::PackageManagement,
        Intent::SystemInfo => Topic::SystemInfo,
        Intent::SettingsChange(_, _) => Topic::Settings,
        Intent::Help(_) => Topic::Help,
        Intent::Search(_) => Topic::Search,
        Intent::Chat => Topic::General,
        Intent::FollowUp => Topic::None, // preserves current topic
        Intent::Confirm | Intent::Deny => Topic::None,
        Intent::Undo => Topic::None,
        Intent::Unknown => Topic::None,
    }
}

// ---------------------------------------------------------------------------
// Intent classification — scored multi-pattern matching
// ---------------------------------------------------------------------------

impl Assistant {
    pub fn new() -> Self {
        Assistant {
            name: String::from("Hoags"),
            history: Vec::new(),
            max_history: 50,
            system_prompt: String::from(
                "You are Hoags, the AI assistant built into Hoags OS. \
                 You help users manage their system, find files, install packages, \
                 and answer questions. You are privacy-focused — all processing \
                 happens locally on the device. Be helpful, concise, and friendly.",
            ),
            personality: Personality::Friendly,
            current_topic: Topic::None,
            topic_confidence: Q16_ZERO,
            topic_turns: 0,
            context_slots: BTreeMap::new(),
            pending_confirm: None,
            topic_history: Vec::new(),
            last_intent_scores: Vec::new(),
            turn_count: 0,
        }
    }

    /// Classify user intent from natural language using scored keyword matching.
    ///
    /// Each candidate intent is scored against the input. The highest-scoring
    /// intent wins, with topic continuity boosting applied when the user is
    /// already in an active conversation topic.
    pub fn classify_intent(&self, input: &str) -> Intent {
        let lower = input.to_lowercase();
        let lower = lower.trim();

        // --- Phase 1: score every candidate intent ---

        let mut scores: Vec<(Intent, i32)> = Vec::new();

        // Confirm / Deny (highest priority when pending)
        let confirm_score = score_confirm(lower);
        if confirm_score > 0 {
            scores.push((Intent::Confirm, confirm_score));
        }
        let deny_score = score_deny(lower);
        if deny_score > 0 {
            scores.push((Intent::Deny, deny_score));
        }

        // Undo
        let undo_score = score_undo(lower);
        if undo_score > 0 {
            scores.push((Intent::Undo, undo_score));
        }

        // File operations
        let s = score_file_list(lower);
        if s > 0 {
            let path = extract_arg(input, 1);
            scores.push((Intent::FileOperation(FileOp::List(path)), s));
        }
        let s = score_file_create(lower);
        if s > 0 {
            let path = extract_arg(input, 1);
            scores.push((Intent::FileOperation(FileOp::Create(path)), s));
        }
        let s = score_file_delete(lower);
        if s > 0 {
            let path = extract_arg(input, 1);
            scores.push((Intent::FileOperation(FileOp::Delete(path)), s));
        }
        let s = score_file_move(lower);
        if s > 0 {
            let (src, dst) = parse_move_args(input);
            scores.push((Intent::FileOperation(FileOp::Move(src, dst)), s));
        }
        let s = score_file_search(lower);
        if s > 0 {
            let query = extract_arg(input, 1);
            scores.push((Intent::FileOperation(FileOp::Search(query)), s));
        }

        // Package operations
        let s = score_pkg_install(lower);
        if s > 0 {
            let pkg = extract_arg(input, 1);
            scores.push((Intent::PackageManage(PkgOp::Install(pkg)), s));
        }
        let s = score_pkg_remove(lower);
        if s > 0 {
            let pkg = extract_arg(input, 1);
            scores.push((Intent::PackageManage(PkgOp::Remove(pkg)), s));
        }
        let s = score_pkg_update(lower);
        if s > 0 {
            scores.push((Intent::PackageManage(PkgOp::Update), s));
        }
        let s = score_pkg_search(lower);
        if s > 0 {
            let query = extract_arg(input, 2);
            scores.push((Intent::PackageManage(PkgOp::Search(query)), s));
        }

        // System info
        let s = score_system_info(lower);
        if s > 0 {
            scores.push((Intent::SystemInfo, s));
        }

        // Help
        let s = score_help(lower);
        if s > 0 {
            scores.push((Intent::Help(String::from(input)), s));
        }

        // Settings
        let s = score_settings(lower);
        if s > 0 {
            let parts: Vec<&str> = input.split_whitespace().collect();
            if parts.len() >= 3 {
                scores.push((
                    Intent::SettingsChange(String::from(parts[1]), parts[2..].join(" ")),
                    s,
                ));
            } else {
                scores.push((
                    Intent::SettingsChange(extract_arg(input, 1), String::new()),
                    s / 2,
                ));
            }
        }

        // --- Phase 2: apply topic continuity boost ---
        // If the user has been on a topic for multiple turns, boost intents
        // in that same topic so that ambiguous inputs stay on-topic.
        let topic_boost = if self.topic_turns >= 2 {
            Q16_ONE / 3 // boost by ~0.33
        } else if self.topic_turns == 1 {
            Q16_ONE / 5 // boost by ~0.2
        } else {
            Q16_ZERO
        };

        for (intent, score) in scores.iter_mut() {
            let intent_topic = intent_to_topic(intent);
            if intent_topic == self.current_topic && self.current_topic != Topic::None {
                *score += topic_boost;
            }
        }

        // --- Phase 3: check for follow-up indicators ---
        let follow_up_indicators = [
            "also",
            "and then",
            "what about",
            "how about",
            "another",
            "one more",
            "same but",
            "that too",
        ];
        let is_follow_up = follow_up_indicators.iter().any(|ind| lower.contains(ind));
        if is_follow_up && self.current_topic != Topic::None {
            // If follow-up detected and no strong other match, treat as follow-up
            let max_score = scores.iter().map(|(_, s)| *s).max().unwrap_or(0);
            if max_score < Q16_ONE / 2 {
                scores.push((Intent::FollowUp, Q16_ONE / 2));
            }
        }

        // --- Phase 4: pick winner ---
        // If a pending action exists, confirm/deny get priority boost
        if self.pending_confirm.is_some() {
            for (intent, score) in scores.iter_mut() {
                match intent {
                    Intent::Confirm | Intent::Deny => {
                        *score += Q16_ONE; // strong boost
                    }
                    _ => {}
                }
            }
        }

        scores.sort_by(|a, b| b.1.cmp(&a.1));

        if let Some((best, _best_score)) = scores.first() {
            best.clone()
        } else {
            // No scored match — default to Chat
            Intent::Chat
        }
    }

    /// Process a user message and generate a response.
    ///
    /// This is the main entry point. It:
    /// 1. Records the user message in history
    /// 2. Classifies intent with multi-pattern scoring
    /// 3. Updates topic tracking state
    /// 4. Handles pending confirmations
    /// 5. Generates a response considering conversation history
    /// 6. Trims history to max_history window
    pub fn process(&mut self, input: &str) -> String {
        self.turn_count = self.turn_count.saturating_add(1);
        self.history.push(Message {
            role: Role::User,
            content: String::from(input),
        });

        let intent = self.classify_intent(input);

        // --- Handle confirmation flow ---
        if let Some(pending) = &self.pending_confirm {
            match &intent {
                Intent::Confirm => {
                    let desc = pending.description.clone();
                    let confirmed_intent = pending.intent.clone();
                    self.pending_confirm = None;
                    let response = self.execute_confirmed(&confirmed_intent, &desc);
                    self.record_response(&response);
                    return response;
                }
                Intent::Deny => {
                    self.pending_confirm = None;
                    let response = String::from("Action cancelled.");
                    self.record_response(&response);
                    return response;
                }
                _ => {
                    // User changed topic; discard pending action
                    self.pending_confirm = None;
                }
            }
        }

        // --- Update topic tracking ---
        let new_topic = intent_to_topic(&intent);
        if new_topic != Topic::None && new_topic != self.current_topic {
            if self.current_topic != Topic::None {
                self.topic_history.push(self.current_topic);
                if self.topic_history.len() > 20 {
                    self.topic_history.remove(0);
                }
            }
            self.current_topic = new_topic;
            self.topic_turns = 1;
            self.topic_confidence = Q16_ONE;
        } else if new_topic == self.current_topic {
            self.topic_turns = self.topic_turns.saturating_add(1);
            // Increase confidence with consecutive same-topic turns (cap at Q16_ONE)
            self.topic_confidence = (self.topic_confidence + Q16_ONE / 8).min(Q16_ONE);
        }

        // --- Extract and store entities ---
        self.extract_entities(input);

        // --- Generate response ---
        let response = match intent {
            Intent::SystemInfo => self.respond_system_info(),
            Intent::FileOperation(ref op) => self.respond_file_op(op),
            Intent::PackageManage(ref op) => self.respond_pkg_op(op),
            Intent::Help(ref topic) => self.respond_help(topic),
            Intent::SettingsChange(ref key, ref value) => self.respond_settings(key, value),
            Intent::Search(ref query) => self.respond_search(query),
            Intent::FollowUp => self.respond_follow_up(input),
            Intent::Undo => self.respond_undo(),
            Intent::Chat => self.respond_chat(input),
            Intent::Confirm | Intent::Deny => {
                // No pending action — interpret as general chat
                String::from("Nothing pending to confirm. How can I help?")
            }
            Intent::Unknown => {
                String::from("I didn't understand that. Type 'help' for available commands.")
            }
        };

        self.record_response(&response);
        response
    }

    /// Record assistant response and trim history
    fn record_response(&mut self, response: &str) {
        self.history.push(Message {
            role: Role::Assistant,
            content: String::from(response),
        });
        while self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Extract entity mentions from user input and populate context slots
    fn extract_entities(&mut self, input: &str) {
        let words: Vec<&str> = input.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            // Path-like entities (contain / or \)
            if word.contains('/') || word.contains('\\') {
                self.context_slots
                    .insert(String::from("last_path"), String::from(*word));
            }
            // Numeric entities
            if word.parse::<i64>().is_ok() {
                self.context_slots
                    .insert(String::from("last_number"), String::from(*word));
            }
            // "to" keyword captures destination
            if *word == "to" && i + 1 < words.len() {
                self.context_slots
                    .insert(String::from("destination"), String::from(words[i + 1]));
            }
        }
    }

    // --- Response generators ---

    fn respond_system_info(&self) -> String {
        String::from(
            "Hoags OS Genesis v0.4.0\n\
             Kernel: Genesis (custom Rust kernel)\n\
             Architecture: x86_64\n\
             Built from scratch by Hoags Inc.\n\
             All AI processing runs locally on your device.",
        )
    }

    fn respond_file_op(&mut self, op: &FileOp) -> String {
        match op {
            FileOp::List(path) => {
                self.context_slots
                    .insert(String::from("last_path"), path.clone());
                let p = if path.is_empty() { "." } else { path.as_str() };
                format!("Listing files in: {}\n(VFS integration pending)", p)
            }
            FileOp::Create(path) => {
                self.context_slots
                    .insert(String::from("last_path"), path.clone());
                format!("Creating: {}\n(VFS integration pending)", path)
            }
            FileOp::Delete(path) => {
                // Destructive action — require confirmation
                self.context_slots
                    .insert(String::from("last_path"), path.clone());
                self.pending_confirm = Some(PendingAction {
                    intent: Intent::FileOperation(FileOp::Delete(path.clone())),
                    description: format!("Delete: {}", path),
                    created_turn: self.turn_count,
                });
                format!("Are you sure you want to delete '{}'? (yes/no)", path)
            }
            FileOp::Move(from, to) => {
                self.context_slots
                    .insert(String::from("last_path"), from.clone());
                self.context_slots
                    .insert(String::from("destination"), to.clone());
                format!("Moving {} -> {}\n(VFS integration pending)", from, to)
            }
            FileOp::Search(query) => {
                self.context_slots
                    .insert(String::from("last_search"), query.clone());
                format!("Searching for: {}\n(Semantic search pending)", query)
            }
        }
    }

    fn respond_pkg_op(&mut self, op: &PkgOp) -> String {
        match op {
            PkgOp::Install(pkg) => {
                self.context_slots
                    .insert(String::from("last_package"), pkg.clone());
                format!("Installing package: {}", pkg)
            }
            PkgOp::Remove(pkg) => {
                self.context_slots
                    .insert(String::from("last_package"), pkg.clone());
                // Destructive action — require confirmation
                self.pending_confirm = Some(PendingAction {
                    intent: Intent::PackageManage(PkgOp::Remove(pkg.clone())),
                    description: format!("Remove package: {}", pkg),
                    created_turn: self.turn_count,
                });
                format!(
                    "Are you sure you want to remove package '{}'? (yes/no)",
                    pkg
                )
            }
            PkgOp::Update => String::from("Checking for package updates..."),
            PkgOp::Search(query) => {
                self.context_slots
                    .insert(String::from("last_search"), query.clone());
                format!("Searching packages: {}", query)
            }
        }
    }

    fn respond_help(&self, _topic: &str) -> String {
        let base_help = String::from(
            "I can help with:\n\
             - File management (list, create, find, delete, move)\n\
             - Package management (install, remove, update, search)\n\
             - System info (uname, cpu, memory, uptime)\n\
             - System settings (set key value)\n\
             - General questions\n\n\
             What would you like to do?",
        );

        // If we have topic history, suggest returning to previous topic
        if let Some(prev_topic) = self.topic_history.last() {
            let topic_name = match prev_topic {
                Topic::FileManagement => "file management",
                Topic::PackageManagement => "package management",
                Topic::SystemInfo => "system info",
                Topic::Settings => "settings",
                Topic::Search => "searching",
                _ => "",
            };
            if !topic_name.is_empty() {
                return format!(
                    "{}\n\nYou were previously working on {}. Want to continue?",
                    base_help, topic_name
                );
            }
        }
        base_help
    }

    fn respond_settings(&mut self, key: &str, value: &str) -> String {
        self.context_slots
            .insert(String::from("last_setting_key"), String::from(key));
        if !value.is_empty() {
            self.context_slots
                .insert(String::from("last_setting_val"), String::from(value));
            format!("Setting {} = {}", key, value)
        } else {
            format!("What value would you like for '{}'?", key)
        }
    }

    fn respond_search(&self, query: &str) -> String {
        format!("Searching: {}", query)
    }

    fn respond_follow_up(&self, input: &str) -> String {
        match self.current_topic {
            Topic::FileManagement => {
                format!(
                    "Continuing with files. What would you like to do? ({})",
                    input
                )
            }
            Topic::PackageManagement => {
                format!(
                    "Continuing with packages. What would you like to do? ({})",
                    input
                )
            }
            Topic::Settings => {
                if let Some(key) = self.context_slots.get("last_setting_key") {
                    format!("Still configuring '{}'. What next?", key)
                } else {
                    String::from("What setting would you like to change?")
                }
            }
            _ => {
                format!("Sure, go on. ({})", input)
            }
        }
    }

    fn respond_undo(&self) -> String {
        String::from("Undo not yet implemented — action history tracking coming soon.")
    }

    fn respond_chat(&self, input: &str) -> String {
        // Build context window and try the inference engine
        let _context = build_context_summary(&self.history, 5);

        match super::inference::generate(input) {
            Ok(response) => {
                // Apply personality filter
                match self.personality {
                    Personality::Concise => {
                        // Truncate long responses
                        if response.len() > 200 {
                            let short: String = response.chars().take(197).collect();
                            format!("{}...", short)
                        } else {
                            response
                        }
                    }
                    Personality::Technical => {
                        format!("[tech] {}", response)
                    }
                    _ => response,
                }
            }
            Err(_) => {
                // Produce a helpful contextual fallback
                let turn = self.turn_count;
                if turn <= 1 {
                    String::from("Hello! I'm Hoags, your AI assistant. I can help with files, packages, settings, and more. Try 'help' for details.")
                } else {
                    String::from(
                        "I'm here to help! Try asking about files, packages, or system info.",
                    )
                }
            }
        }
    }

    /// Execute an action that was previously confirmed
    fn execute_confirmed(&mut self, intent: &Intent, description: &str) -> String {
        serial_println!("  [assistant] Confirmed action: {}", description);
        match intent {
            Intent::FileOperation(FileOp::Delete(path)) => {
                format!("Deleted: {}\n(VFS integration pending)", path)
            }
            Intent::PackageManage(PkgOp::Remove(pkg)) => {
                format!(
                    "Removed package: {}\n(Package manager integration pending)",
                    pkg
                )
            }
            _ => {
                format!("Executed: {}", description)
            }
        }
    }

    /// Get the number of turns on the current topic
    pub fn topic_turn_count(&self) -> u32 {
        self.topic_turns
    }

    /// Get current conversation topic
    pub fn current_topic(&self) -> Topic {
        self.current_topic
    }

    /// Reset conversation state
    pub fn reset(&mut self) {
        self.history.clear();
        self.current_topic = Topic::None;
        self.topic_confidence = Q16_ZERO;
        self.topic_turns = 0;
        self.context_slots.clear();
        self.pending_confirm = None;
        self.topic_history.clear();
        self.turn_count = 0;
    }

    /// Get a context slot value
    pub fn get_slot(&self, key: &str) -> Option<&String> {
        self.context_slots.get(key)
    }

    /// Set personality
    pub fn set_personality(&mut self, personality: Personality) {
        self.personality = personality;
    }

    /// Get conversation summary statistics
    pub fn stats(&self) -> (u64, usize, u32) {
        (self.turn_count, self.history.len(), self.topic_turns)
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    *ASSISTANT.lock() = Some(Assistant::new());
    serial_println!(
        "    [assistant] Hoags AI Assistant ready (multi-turn, scored intent classification)"
    );
}

/// Send a message to the assistant
pub fn chat(input: &str) -> String {
    ASSISTANT
        .lock()
        .as_mut()
        .map(|a| a.process(input))
        .unwrap_or_else(|| String::from("Assistant not initialized"))
}

/// Reset the assistant conversation state
pub fn reset() {
    if let Some(a) = ASSISTANT.lock().as_mut() {
        a.reset();
    }
}

/// Get current topic as a string
pub fn current_topic() -> String {
    ASSISTANT
        .lock()
        .as_ref()
        .map(|a| format!("{:?}", a.current_topic()))
        .unwrap_or_else(|| String::from("None"))
}

/// Set personality mode
pub fn set_personality(p: Personality) {
    if let Some(a) = ASSISTANT.lock().as_mut() {
        a.set_personality(p);
    }
}
