/// Conversation Manager — multi-turn context, memory, and pruning
///
/// Manages the full lifecycle of a conversation between the user
/// and the Hoags AI. Handles system prompts, multi-turn history,
/// context window budgeting, conversation memory, pruning of old
/// turns, and summarization of long conversations.
///
/// The AI remembers what was said, keeps track of context usage,
/// and intelligently prunes or summarizes when the window fills up.
/// All data stays local — no cloud, no telemetry, no leaks.
///
/// Features:
///   - Multi-turn conversation state machine
///   - System prompt injection and priority ordering
///   - Token budget tracking per turn and total
///   - Conversation memory (pinned facts from conversation)
///   - Automatic pruning of oldest turns when budget exceeded
///   - Summarization of pruned turns into a digest
///   - Conversation branching (fork a conversation)
///   - Turn-level metadata (timestamps, token counts, roles)

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

use super::transformer::{Q16, q16_mul, q16_from_int};

// ── Constants ────────────────────────────────────────────────────────

/// Maximum number of turns before forced pruning
const MAX_TURNS: usize = 512;

/// Maximum total token budget for conversation context (128K tokens)
const MAX_CONTEXT_TOKENS: u32 = 131072;

/// Number of turns to keep when pruning (most recent)
const PRUNE_KEEP_RECENT: usize = 32;

/// Maximum number of pinned memories per conversation
const MAX_PINNED_MEMORIES: usize = 64;

/// Maximum number of conversation summaries retained
const MAX_SUMMARIES: usize = 32;

/// Maximum number of active conversations tracked
const MAX_CONVERSATIONS: usize = 16;

/// Summary token budget — how many tokens a summary can consume
const SUMMARY_TOKEN_BUDGET: u32 = 2048;

/// System prompt token budget
const SYSTEM_PROMPT_BUDGET: u32 = 8192;

// ── Types ────────────────────────────────────────────────────────────

/// Role of a message in the conversation
#[derive(Clone, Copy, PartialEq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
    Thinking,
    Summary,
}

/// A single turn (message) in the conversation
#[derive(Clone, Copy)]
pub struct ConversationTurn {
    pub role: MessageRole,
    pub content_hash: u64,
    pub token_count: u32,
    pub timestamp: u64,
    pub turn_index: u32,
    pub pinned: bool,
    pub pruned: bool,
}

/// A pinned memory extracted from the conversation
#[derive(Clone, Copy)]
pub struct ConversationMemory {
    pub fact_hash: u64,
    pub source_turn: u32,
    pub importance: Q16,
    pub timestamp: u64,
    pub referenced_count: u32,
}

/// A summary of pruned conversation turns
#[derive(Clone, Copy)]
pub struct ConversationSummary {
    pub summary_hash: u64,
    pub turns_start: u32,
    pub turns_end: u32,
    pub token_count: u32,
    pub key_facts: u32,
    pub timestamp: u64,
}

/// State of a conversation
#[derive(Clone, Copy, PartialEq)]
pub enum ConversationState {
    Active,
    Paused,
    Archived,
    Branched,
}

/// Configuration for conversation behavior
#[derive(Clone, Copy)]
pub struct ConversationConfig {
    pub max_context_tokens: u32,
    pub auto_prune: bool,
    pub auto_summarize: bool,
    pub pin_important_facts: bool,
    pub keep_system_prompt: bool,
    pub summary_budget: u32,
    pub system_prompt_budget: u32,
}

impl ConversationConfig {
    fn default_config() -> Self {
        ConversationConfig {
            max_context_tokens: MAX_CONTEXT_TOKENS,
            auto_prune: true,
            auto_summarize: true,
            pin_important_facts: true,
            keep_system_prompt: true,
            summary_budget: SUMMARY_TOKEN_BUDGET,
            system_prompt_budget: SYSTEM_PROMPT_BUDGET,
        }
    }
}

/// A single conversation session
pub struct Conversation {
    pub id: u32,
    pub state: ConversationState,
    pub turns: Vec<ConversationTurn>,
    pub memories: Vec<ConversationMemory>,
    pub summaries: Vec<ConversationSummary>,
    pub config: ConversationConfig,
    pub system_prompt_hash: u64,
    pub total_tokens_used: u32,
    pub total_turns: u32,
    pub created_at: u64,
    pub last_active: u64,
    pub parent_conversation: u32,
    pub fork_point: u32,
}

impl Conversation {
    fn new(id: u32, system_prompt: u64, timestamp: u64) -> Self {
        let config = ConversationConfig::default_config();

        let mut conv = Conversation {
            id,
            state: ConversationState::Active,
            turns: Vec::new(),
            memories: Vec::new(),
            summaries: Vec::new(),
            config,
            system_prompt_hash: system_prompt,
            total_tokens_used: 0,
            total_turns: 0,
            created_at: timestamp,
            last_active: timestamp,
            parent_conversation: 0,
            fork_point: 0,
        };

        // Inject the system prompt as turn 0
        if system_prompt != 0 {
            conv.turns.push(ConversationTurn {
                role: MessageRole::System,
                content_hash: system_prompt,
                token_count: config.system_prompt_budget,
                timestamp,
                turn_index: 0,
                pinned: true,
                pruned: false,
            });
            conv.total_tokens_used = config.system_prompt_budget;
            conv.total_turns = 1;
        }

        conv
    }

    /// Add a new turn to the conversation
    fn add_turn(&mut self, role: MessageRole, content: u64, tokens: u32, timestamp: u64) -> u32 {
        let index = self.total_turns;

        self.turns.push(ConversationTurn {
            role,
            content_hash: content,
            token_count: tokens,
            timestamp,
            turn_index: index,
            pinned: false,
            pruned: false,
        });

        self.total_tokens_used += tokens;
        self.total_turns = self.total_turns.saturating_add(1);
        self.last_active = timestamp;

        // Check if we need to prune
        if self.config.auto_prune && self.should_prune() {
            self.prune();
        }

        index
    }

    /// Check if the conversation needs pruning
    fn should_prune(&self) -> bool {
        self.total_tokens_used > self.config.max_context_tokens
            || self.turns.len() > MAX_TURNS
    }

    /// Prune old turns, keeping recent ones and pinned turns
    fn prune(&mut self) {
        if self.turns.len() <= PRUNE_KEEP_RECENT {
            return;
        }

        let cutoff = self.turns.len() - PRUNE_KEEP_RECENT;
        let mut pruned_tokens: u32 = 0;
        let mut pruned_content_hashes: Vec<u64> = Vec::new();
        let mut first_pruned: u32 = u32::MAX;
        let mut last_pruned: u32 = 0;

        for i in 0..cutoff {
            let turn = &self.turns[i];
            // Never prune system prompts or pinned turns
            if turn.role == MessageRole::System || turn.pinned || turn.pruned {
                continue;
            }
            pruned_tokens += turn.token_count;
            pruned_content_hashes.push(turn.content_hash);
            if turn.turn_index < first_pruned {
                first_pruned = turn.turn_index;
            }
            if turn.turn_index > last_pruned {
                last_pruned = turn.turn_index;
            }
        }

        // Mark turns as pruned
        for i in 0..cutoff {
            let turn = &mut self.turns[i];
            if turn.role != MessageRole::System && !turn.pinned {
                turn.pruned = true;
            }
        }

        // Create a summary of pruned content if auto-summarize is on
        if self.config.auto_summarize && !pruned_content_hashes.is_empty() {
            let summary_hash = self.compute_summary_hash(&pruned_content_hashes);

            if self.summaries.len() < MAX_SUMMARIES {
                self.summaries.push(ConversationSummary {
                    summary_hash,
                    turns_start: first_pruned,
                    turns_end: last_pruned,
                    token_count: self.config.summary_budget,
                    key_facts: pruned_content_hashes.len() as u32,
                    timestamp: self.last_active,
                });
            }
        }

        // Remove pruned turns from the vec (keep system + pinned + recent)
        self.turns.retain(|t| !t.pruned || t.pinned || t.role == MessageRole::System);

        // Recalculate token usage
        self.recalculate_tokens();
    }

    /// Compute a summary hash from a set of content hashes
    fn compute_summary_hash(&self, hashes: &[u64]) -> u64 {
        let mut result: u64 = 0xCAFE_BABE_0000_0000;
        for (i, &h) in hashes.iter().enumerate() {
            result ^= h.wrapping_mul(0x0101_0101_0101_0101);
            result = result.wrapping_add(i as u64);
        }
        result
    }

    /// Recalculate total tokens from current turns and summaries
    fn recalculate_tokens(&mut self) {
        let mut total: u32 = 0;
        for turn in &self.turns {
            if !turn.pruned {
                total += turn.token_count;
            }
        }
        for summary in &self.summaries {
            total += summary.token_count;
        }
        self.total_tokens_used = total;
    }

    /// Pin a memory from a specific turn
    fn pin_memory(&mut self, turn_index: u32, fact_hash: u64, importance: Q16, timestamp: u64) {
        if self.memories.len() >= MAX_PINNED_MEMORIES {
            // Remove least important memory
            self.evict_weakest_memory();
        }

        self.memories.push(ConversationMemory {
            fact_hash,
            source_turn: turn_index,
            importance,
            timestamp,
            referenced_count: 1,
        });

        // Mark the source turn as pinned
        for turn in &mut self.turns {
            if turn.turn_index == turn_index {
                turn.pinned = true;
                break;
            }
        }
    }

    /// Remove the memory with the lowest importance
    fn evict_weakest_memory(&mut self) {
        if self.memories.is_empty() {
            return;
        }
        let mut min_importance = i32::MAX;
        let mut min_idx: usize = 0;

        for (i, mem) in self.memories.iter().enumerate() {
            if mem.importance < min_importance {
                min_importance = mem.importance;
                min_idx = i;
            }
        }

        self.memories.remove(min_idx);
    }

    /// Reference a memory (bump its reference count)
    fn reference_memory(&mut self, fact_hash: u64) {
        for mem in &mut self.memories {
            if mem.fact_hash == fact_hash {
                mem.referenced_count = mem.referenced_count.saturating_add(1);
                // Boost importance slightly for referenced memories
                let boost: Q16 = 655; // ~0.01 in Q16
                mem.importance = mem.importance.saturating_add(boost);
                return;
            }
        }
    }

    /// Build the context window: system prompt + summaries + memories + recent turns
    fn build_context(&self) -> Vec<u64> {
        let mut context: Vec<u64> = Vec::new();

        // 1. System prompt
        if self.config.keep_system_prompt && self.system_prompt_hash != 0 {
            context.push(self.system_prompt_hash);
        }

        // 2. Summaries of pruned history (chronological)
        for summary in &self.summaries {
            context.push(summary.summary_hash);
        }

        // 3. Pinned memories (by importance descending)
        let mut mem_indices: Vec<usize> = (0..self.memories.len()).collect();
        // Selection sort by importance descending
        for i in 0..mem_indices.len() {
            let mut best = i;
            for j in (i + 1)..mem_indices.len() {
                if self.memories[mem_indices[j]].importance
                    > self.memories[mem_indices[best]].importance
                {
                    best = j;
                }
            }
            if best != i {
                mem_indices.swap(i, best);
            }
        }
        for &idx in &mem_indices {
            context.push(self.memories[idx].fact_hash);
        }

        // 4. Non-pruned turns in order
        for turn in &self.turns {
            if !turn.pruned {
                context.push(turn.content_hash);
            }
        }

        context
    }

    /// Fork a conversation at a specific turn index
    fn fork(&self, new_id: u32, at_turn: u32, timestamp: u64) -> Conversation {
        let mut forked = Conversation::new(new_id, self.system_prompt_hash, timestamp);
        forked.state = ConversationState::Branched;
        forked.parent_conversation = self.id;
        forked.fork_point = at_turn;

        // Copy turns up to the fork point
        for turn in &self.turns {
            if turn.turn_index <= at_turn && !turn.pruned {
                forked.turns.push(*turn);
                forked.total_tokens_used += turn.token_count;
            }
        }

        // Copy memories
        for mem in &self.memories {
            if mem.source_turn <= at_turn {
                forked.memories.push(*mem);
            }
        }

        forked.total_turns = forked.turns.len() as u32;
        forked
    }

    /// Get conversation statistics
    fn get_stats(&self) -> ConversationStats {
        let mut user_turns: u32 = 0;
        let mut assistant_turns: u32 = 0;
        let mut tool_turns: u32 = 0;

        for turn in &self.turns {
            match turn.role {
                MessageRole::User => user_turns += 1,
                MessageRole::Assistant => assistant_turns += 1,
                MessageRole::Tool => tool_turns += 1,
                _ => {}
            }
        }

        ConversationStats {
            total_turns: self.total_turns,
            active_turns: self.turns.len() as u32,
            user_turns,
            assistant_turns,
            tool_turns,
            total_tokens: self.total_tokens_used,
            pinned_memories: self.memories.len() as u32,
            summaries: self.summaries.len() as u32,
            context_utilization: self.context_utilization(),
        }
    }

    /// How full is the context window? Returns Q16 ratio (0.0 to 1.0)
    fn context_utilization(&self) -> Q16 {
        if self.config.max_context_tokens == 0 {
            return 0;
        }
        let used = self.total_tokens_used as i64;
        let max = self.config.max_context_tokens as i64;
        (((used << 16) / max.max(1)) as i32).min(q16_from_int(1))
    }
}

/// Statistics about a conversation
#[derive(Clone, Copy)]
pub struct ConversationStats {
    pub total_turns: u32,
    pub active_turns: u32,
    pub user_turns: u32,
    pub assistant_turns: u32,
    pub tool_turns: u32,
    pub total_tokens: u32,
    pub pinned_memories: u32,
    pub summaries: u32,
    pub context_utilization: Q16,
}

// ── Conversation Manager ─────────────────────────────────────────────

/// The global conversation manager — tracks all active conversations
struct ConversationManager {
    conversations: Vec<Conversation>,
    active_conversation: u32,
    next_conversation_id: u32,
    total_conversations_created: u64,
    total_turns_processed: u64,
    total_prunes: u64,
    total_summaries_generated: u64,
}

impl ConversationManager {
    fn new() -> Self {
        ConversationManager {
            conversations: Vec::new(),
            active_conversation: 0,
            next_conversation_id: 1,
            total_conversations_created: 0,
            total_turns_processed: 0,
            total_prunes: 0,
            total_summaries_generated: 0,
        }
    }

    /// Start a new conversation with an optional system prompt
    fn new_conversation(&mut self, system_prompt: u64, timestamp: u64) -> u32 {
        let id = self.next_conversation_id;
        self.next_conversation_id = self.next_conversation_id.saturating_add(1);

        // Archive old conversations if at capacity
        if self.conversations.len() >= MAX_CONVERSATIONS {
            self.archive_oldest();
        }

        let conv = Conversation::new(id, system_prompt, timestamp);
        self.conversations.push(conv);
        self.active_conversation = id;
        self.total_conversations_created = self.total_conversations_created.saturating_add(1);
        id
    }

    /// Add a turn to the active conversation
    fn add_turn(&mut self, role: MessageRole, content: u64, tokens: u32, timestamp: u64) -> Option<u32> {
        let active_id = self.active_conversation;
        if let Some(conv) = self.conversations.iter_mut().find(|c| c.id == active_id) {
            let old_summary_count = conv.summaries.len();
            let idx = conv.add_turn(role, content, tokens, timestamp);
            self.total_turns_processed = self.total_turns_processed.saturating_add(1);

            // Track if pruning generated new summaries
            if conv.summaries.len() > old_summary_count {
                self.total_summaries_generated += (conv.summaries.len() - old_summary_count) as u64;
                self.total_prunes = self.total_prunes.saturating_add(1);
            }

            Some(idx)
        } else {
            None
        }
    }

    /// Pin a memory in the active conversation
    fn pin_memory(&mut self, turn_index: u32, fact_hash: u64, importance: Q16, timestamp: u64) {
        let active_id = self.active_conversation;
        if let Some(conv) = self.conversations.iter_mut().find(|c| c.id == active_id) {
            conv.pin_memory(turn_index, fact_hash, importance, timestamp);
        }
    }

    /// Build context for the active conversation
    fn build_active_context(&self) -> Vec<u64> {
        let active_id = self.active_conversation;
        if let Some(conv) = self.conversations.iter().find(|c| c.id == active_id) {
            conv.build_context()
        } else {
            Vec::new()
        }
    }

    /// Switch to a different conversation
    fn switch_conversation(&mut self, id: u32) -> bool {
        if self.conversations.iter().any(|c| c.id == id && c.state == ConversationState::Active) {
            self.active_conversation = id;
            true
        } else {
            false
        }
    }

    /// Fork the active conversation at a given turn
    fn fork_active(&mut self, at_turn: u32, timestamp: u64) -> u32 {
        let active_id = self.active_conversation;
        let new_id = self.next_conversation_id;
        self.next_conversation_id = self.next_conversation_id.saturating_add(1);

        let forked = if let Some(conv) = self.conversations.iter().find(|c| c.id == active_id) {
            Some(conv.fork(new_id, at_turn, timestamp))
        } else {
            None
        };

        if let Some(f) = forked {
            self.conversations.push(f);
            self.total_conversations_created = self.total_conversations_created.saturating_add(1);
            self.active_conversation = new_id;
        }

        new_id
    }

    /// Archive the oldest non-active conversation
    fn archive_oldest(&mut self) {
        let mut oldest_time: u64 = u64::MAX;
        let mut oldest_idx: Option<usize> = None;

        for (i, conv) in self.conversations.iter().enumerate() {
            if conv.id != self.active_conversation
                && conv.state == ConversationState::Active
                && conv.last_active < oldest_time
            {
                oldest_time = conv.last_active;
                oldest_idx = Some(i);
            }
        }

        if let Some(idx) = oldest_idx {
            self.conversations[idx].state = ConversationState::Archived;
        }
    }

    /// Get stats for the active conversation
    fn get_active_stats(&self) -> Option<ConversationStats> {
        let active_id = self.active_conversation;
        self.conversations.iter()
            .find(|c| c.id == active_id)
            .map(|c| c.get_stats())
    }

    /// Get global manager stats
    fn get_global_stats(&self) -> (u64, u64, u64, u64, u32) {
        (
            self.total_conversations_created,
            self.total_turns_processed,
            self.total_prunes,
            self.total_summaries_generated,
            self.conversations.len() as u32,
        )
    }
}

// ── Global State ─────────────────────────────────────────────────────

static MANAGER: Mutex<Option<ConversationManager>> = Mutex::new(None);

/// Access the global conversation manager
pub fn with_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut ConversationManager) -> R,
{
    let mut locked = MANAGER.lock();
    if let Some(ref mut mgr) = *locked {
        Some(f(mgr))
    } else {
        None
    }
}

// ── Module Initialization ────────────────────────────────────────────

pub fn init() {
    let mut m = MANAGER.lock();
    *m = Some(ConversationManager::new());
    serial_println!("    Conversation: multi-turn context, memory pinning, pruning, summarization, forking ready");
}
