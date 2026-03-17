use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
/// AI context management -- conversation history and working memory
///
/// Part of the AIOS AI layer. Maintains conversation-like context windows
/// with sliding context management, token budget enforcement, and context
/// priority ranking. Supports multi-turn dialogue with role-based history.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Priority level for context entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContextPriority {
    /// System instructions that should never be evicted
    System = 3,
    /// Pinned context that persists across turns
    Pinned = 2,
    /// Normal conversational context
    Normal = 1,
    /// Low priority, evicted first when budget is tight
    Low = 0,
}

/// A single context entry (turn, memory, or instruction)
#[derive(Clone)]
pub struct ContextEntry {
    pub role: String,
    pub content: String,
    pub priority: ContextPriority,
    pub token_count: usize,
    pub turn_number: u64,
    pub metadata: BTreeMap<String, String>,
}

impl ContextEntry {
    fn new(role: &str, content: &str, priority: ContextPriority, turn: u64) -> Self {
        let token_count = estimate_tokens(content);
        ContextEntry {
            role: String::from(role),
            content: String::from(content),
            priority,
            token_count,
            turn_number: turn,
            metadata: BTreeMap::new(),
        }
    }

    fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.insert(String::from(key), String::from(value));
        self
    }
}

/// Tracks conversation turns and active working memory
pub struct AiContext {
    pub history: Vec<String>,
    pub working_memory: Vec<String>,
    pub max_turns: usize,
    /// Full context entries with priority and metadata
    entries: Vec<ContextEntry>,
    /// Maximum token budget for the entire context window
    token_budget: usize,
    /// Current total token count
    current_tokens: usize,
    /// Turn counter
    turn_counter: u64,
    /// System prompt (always included at the start)
    system_prompt: Option<String>,
    /// Sliding window: number of recent turns to always keep
    recent_window: usize,
}

impl AiContext {
    pub fn new(max_turns: usize) -> Self {
        AiContext {
            history: Vec::new(),
            working_memory: Vec::new(),
            max_turns,
            entries: Vec::new(),
            token_budget: 4096,
            current_tokens: 0,
            turn_counter: 0,
            system_prompt: None,
            recent_window: max_turns.min(10),
        }
    }

    /// Create with a specific token budget
    pub fn with_budget(max_turns: usize, token_budget: usize) -> Self {
        let mut ctx = Self::new(max_turns);
        ctx.token_budget = token_budget;
        ctx
    }

    /// Set the system prompt (highest priority, always included)
    pub fn set_system_prompt(&mut self, prompt: &str) {
        let entry = ContextEntry::new("system", prompt, ContextPriority::System, 0);
        // Remove any existing system prompt
        self.entries
            .retain(|e| e.priority != ContextPriority::System || e.role != "system");
        let tokens = entry.token_count;
        self.entries.insert(0, entry);
        self.system_prompt = Some(String::from(prompt));
        self.recalculate_tokens();
        // If system prompt alone exceeds budget, we still keep it
        if self.current_tokens > self.token_budget && tokens < self.token_budget {
            self.evict_to_budget();
        }
    }

    /// Push a new conversational turn
    pub fn push_turn(&mut self, role: &str, content: &str) {
        self.turn_counter = self.turn_counter.saturating_add(1);
        let entry = ContextEntry::new(role, content, ContextPriority::Normal, self.turn_counter);

        // Add to flat history for backward compatibility
        let formatted = format!("[{}]: {}", role, content);
        self.history.push(formatted);

        // Add entry
        self.entries.push(entry);
        self.recalculate_tokens();

        // Enforce sliding window: keep at most max_turns normal entries
        self.enforce_turn_limit();

        // Enforce token budget
        if self.current_tokens > self.token_budget {
            self.evict_to_budget();
        }
    }

    /// Add pinned context that persists across eviction cycles
    pub fn pin_context(&mut self, role: &str, content: &str) {
        let entry = ContextEntry::new(role, content, ContextPriority::Pinned, self.turn_counter);
        self.entries.push(entry);
        self.recalculate_tokens();
        if self.current_tokens > self.token_budget {
            self.evict_to_budget();
        }
    }

    /// Add to working memory (low priority, evicted first)
    pub fn add_working_memory(&mut self, content: &str) {
        self.working_memory.push(String::from(content));
        let entry = ContextEntry::new("memory", content, ContextPriority::Low, self.turn_counter);
        self.entries.push(entry);
        self.recalculate_tokens();
        if self.current_tokens > self.token_budget {
            self.evict_to_budget();
        }
    }

    /// Build the full prompt from the current context window
    pub fn build_prompt(&self) -> String {
        let mut prompt = String::new();
        let mut total_chars = 0;

        for entry in &self.entries {
            if !prompt.is_empty() {
                prompt.push('\n');
                total_chars += 1;
            }

            let line = if entry.role == "system" {
                format!("[SYSTEM] {}", entry.content)
            } else if entry.role == "memory" {
                format!("[MEMORY] {}", entry.content)
            } else if entry.role == "user" {
                format!("[USER] {}", entry.content)
            } else if entry.role == "assistant" {
                format!("[ASSISTANT] {}", entry.content)
            } else {
                format!("[{}] {}", entry.role.to_uppercase(), entry.content)
            };

            total_chars += line.len();
            prompt.push_str(&line);
        }

        prompt
    }

    /// Build a compact prompt that only includes the most recent N turns
    pub fn build_recent_prompt(&self, n: usize) -> String {
        let mut prompt = String::new();

        // Always include system entries
        for entry in &self.entries {
            if entry.priority == ContextPriority::System {
                if !prompt.is_empty() {
                    prompt.push('\n');
                }
                prompt.push_str(&format!("[SYSTEM] {}", entry.content));
            }
        }

        // Include pinned entries
        for entry in &self.entries {
            if entry.priority == ContextPriority::Pinned {
                if !prompt.is_empty() {
                    prompt.push('\n');
                }
                prompt.push_str(&format!(
                    "[{}] {}",
                    entry.role.to_uppercase(),
                    entry.content
                ));
            }
        }

        // Include last N normal/low entries
        let normal_entries: Vec<&ContextEntry> = self
            .entries
            .iter()
            .filter(|e| e.priority == ContextPriority::Normal || e.priority == ContextPriority::Low)
            .collect();
        let start = if normal_entries.len() > n {
            normal_entries.len() - n
        } else {
            0
        };
        for entry in &normal_entries[start..] {
            if !prompt.is_empty() {
                prompt.push('\n');
            }
            prompt.push_str(&format!(
                "[{}] {}",
                entry.role.to_uppercase(),
                entry.content
            ));
        }

        prompt
    }

    /// Get current token usage
    pub fn token_usage(&self) -> (usize, usize) {
        (self.current_tokens, self.token_budget)
    }

    /// Get the number of turns in history
    pub fn turn_count(&self) -> u64 {
        self.turn_counter
    }

    /// Get the number of entries in the context window
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get remaining token budget
    pub fn remaining_budget(&self) -> usize {
        if self.current_tokens >= self.token_budget {
            0
        } else {
            self.token_budget - self.current_tokens
        }
    }

    /// Set the token budget
    pub fn set_token_budget(&mut self, budget: usize) {
        self.token_budget = budget;
        if self.current_tokens > self.token_budget {
            self.evict_to_budget();
        }
    }

    /// Clear all context except system prompt
    pub fn clear(&mut self) {
        self.entries
            .retain(|e| e.priority == ContextPriority::System);
        self.history.clear();
        self.working_memory.clear();
        self.turn_counter = 0;
        self.recalculate_tokens();
    }

    /// Summarize old turns: replace the oldest N normal turns with a summary
    pub fn summarize_old_turns(&mut self, keep_recent: usize) {
        let normal_entries: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.priority == ContextPriority::Normal)
            .map(|(i, _)| i)
            .collect();

        if normal_entries.len() <= keep_recent {
            return;
        }

        let to_summarize = normal_entries.len() - keep_recent;
        let indices_to_remove: Vec<usize> = normal_entries[..to_summarize].to_vec();

        // Build a summary of the removed entries
        let mut summary_parts = Vec::new();
        for &idx in &indices_to_remove {
            let entry = &self.entries[idx];
            // Take first 50 chars of each entry for the summary
            let snippet: String = entry.content.chars().take(50).collect();
            summary_parts.push(format!("{}: {}", entry.role, snippet));
        }

        let summary_text = format!(
            "[Summary of {} earlier turns] {}",
            to_summarize,
            summary_parts.join(" | ")
        );

        // Remove old entries (in reverse order to preserve indices)
        let mut indices_rev = indices_to_remove.clone();
        indices_rev.reverse();
        for idx in indices_rev {
            if idx < self.entries.len() {
                self.entries.remove(idx);
            }
        }

        // Insert the summary as a pinned entry after system entries
        let insert_pos = self
            .entries
            .iter()
            .position(|e| e.priority != ContextPriority::System)
            .unwrap_or(self.entries.len());
        let summary_entry = ContextEntry::new("summary", &summary_text, ContextPriority::Pinned, 0);
        self.entries.insert(insert_pos, summary_entry);
        self.recalculate_tokens();
    }

    /// Search context entries for a keyword, return matching entries
    pub fn search(&self, keyword: &str) -> Vec<&ContextEntry> {
        let lower_kw = keyword.to_lowercase();
        self.entries
            .iter()
            .filter(|e| e.content.to_lowercase().contains(&lower_kw))
            .collect()
    }

    /// Get the last N entries
    pub fn recent_entries(&self, n: usize) -> Vec<&ContextEntry> {
        let start = if self.entries.len() > n {
            self.entries.len() - n
        } else {
            0
        };
        self.entries[start..].iter().collect()
    }

    // -----------------------------------------------------------------------
    // Internal methods
    // -----------------------------------------------------------------------

    fn recalculate_tokens(&mut self) {
        self.current_tokens = self.entries.iter().map(|e| e.token_count).sum();
    }

    /// Enforce the max_turns limit for Normal priority entries
    fn enforce_turn_limit(&mut self) {
        let normal_count = self
            .entries
            .iter()
            .filter(|e| e.priority == ContextPriority::Normal)
            .count();

        if normal_count <= self.max_turns {
            return;
        }

        let excess = normal_count - self.max_turns;
        let mut removed = 0;
        self.entries.retain(|e| {
            if removed >= excess {
                return true;
            }
            if e.priority == ContextPriority::Normal {
                removed += 1;
                return false;
            }
            true
        });
        self.recalculate_tokens();
    }

    /// Evict entries until we are within token budget.
    /// Eviction order: Low priority first (oldest first), then Normal (oldest first).
    /// System and Pinned entries are never evicted.
    fn evict_to_budget(&mut self) {
        // Phase 1: evict Low priority entries (oldest first)
        while self.current_tokens > self.token_budget {
            let pos = self
                .entries
                .iter()
                .position(|e| e.priority == ContextPriority::Low);
            match pos {
                Some(i) => {
                    self.entries.remove(i);
                    self.recalculate_tokens();
                }
                None => break,
            }
        }

        // Phase 2: evict Normal priority entries (oldest first)
        while self.current_tokens > self.token_budget {
            let pos = self
                .entries
                .iter()
                .position(|e| e.priority == ContextPriority::Normal);
            match pos {
                Some(i) => {
                    self.entries.remove(i);
                    self.recalculate_tokens();
                }
                None => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Estimate token count for a string.
/// Approximation: ~4 characters per token (GPT-like tokenizer average).
fn estimate_tokens(text: &str) -> usize {
    let char_count = text.len();
    // Rough heuristic: 1 token per 4 bytes, minimum 1
    let estimate = (char_count + 3) / 4;
    if estimate == 0 {
        1
    } else {
        estimate
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CONTEXT: Mutex<Option<AiContext>> = Mutex::new(None);

pub fn init() {
    let mut ctx = AiContext::with_budget(32, 8192);
    ctx.set_system_prompt(
        "You are the Genesis AIOS assistant. Help the user with system tasks, \
         answer questions about the OS, and provide intelligent suggestions. \
         You run entirely on-device with no external API calls.",
    );
    *CONTEXT.lock() = Some(ctx);
    crate::serial_println!("    [context] AI context manager ready (32 turns, 8192 token budget)");
}

/// Push a turn to the global context
pub fn push_turn(role: &str, content: &str) {
    if let Some(ctx) = CONTEXT.lock().as_mut() {
        ctx.push_turn(role, content);
    }
}

/// Build a prompt from the global context
pub fn build_prompt() -> String {
    CONTEXT
        .lock()
        .as_ref()
        .map(|ctx| ctx.build_prompt())
        .unwrap_or_else(String::new)
}

/// Get remaining token budget
pub fn remaining_budget() -> usize {
    CONTEXT
        .lock()
        .as_ref()
        .map(|ctx| ctx.remaining_budget())
        .unwrap_or(0)
}
