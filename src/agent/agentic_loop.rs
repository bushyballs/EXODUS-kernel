use crate::sync::Mutex;
/// Agentic loop for Genesis
///
/// The core observe -> think -> act -> observe cycle.
/// Manages conversation turns, tool call chains,
/// context window, and stopping conditions.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum TurnType {
    UserMessage,
    AssistantMessage,
    ToolCall,
    ToolResult,
    SystemPrompt,
    ContextSummary,
}

#[derive(Clone, Copy, PartialEq)]
pub enum StopReason {
    Complete,         // Agent decided it's done
    MaxTurns,         // Hit turn limit
    UserInterrupt,    // User pressed Ctrl+C
    Error,            // Unrecoverable error
    PermissionDenied, // User denied a critical tool
    TokenLimit,       // Context window full
}

#[derive(Clone, Copy, PartialEq)]
pub enum LoopState {
    Idle,
    Thinking,        // LLM generating response
    Acting,          // Executing tool call
    WaitingApproval, // Waiting for user to approve tool
    Streaming,       // Streaming output to user
    Paused,          // User paused execution
    Done,
}

#[derive(Clone, Copy)]
struct ConversationTurn {
    turn_id: u32,
    turn_type: TurnType,
    content_hash: u64,
    token_count: u32,
    timestamp: u64,
    tool_call_id: u32, // 0 if not a tool turn
}

#[derive(Clone, Copy)]
struct LoopConfig {
    max_turns: u32,
    max_tokens: u32,         // Context window limit
    compress_threshold: u32, // Auto-compress when tokens exceed this
    auto_approve_reads: bool,
    stop_on_error: bool,
    streaming_enabled: bool,
    thinking_budget: u32, // Max tokens for thinking/planning
}

struct AgenticLoop {
    turns: Vec<ConversationTurn>,
    state: LoopState,
    config: LoopConfig,
    current_turn: u32,
    total_tokens_used: u64,
    total_tool_calls: u32,
    total_sessions: u32,
    active_session_id: u32,
    // Context management
    compressed_count: u32, // How many times we've compressed
    system_prompt_hash: u64,
}

static AGENT_LOOP: Mutex<Option<AgenticLoop>> = Mutex::new(None);

impl AgenticLoop {
    fn new() -> Self {
        AgenticLoop {
            turns: Vec::new(),
            state: LoopState::Idle,
            config: LoopConfig {
                max_turns: 50,
                max_tokens: 200_000,
                compress_threshold: 150_000,
                auto_approve_reads: true,
                stop_on_error: false,
                streaming_enabled: true,
                thinking_budget: 10_000,
            },
            current_turn: 0,
            total_tokens_used: 0,
            total_tool_calls: 0,
            total_sessions: 0,
            active_session_id: 0,
            compressed_count: 0,
            system_prompt_hash: 0,
        }
    }

    fn start_session(&mut self, system_prompt_hash: u64) -> u32 {
        self.total_sessions = self.total_sessions.saturating_add(1);
        self.active_session_id = self.total_sessions;
        self.turns.clear();
        self.current_turn = 0;
        self.system_prompt_hash = system_prompt_hash;
        self.state = LoopState::Idle;
        // Add system prompt as first turn
        self.add_turn(TurnType::SystemPrompt, system_prompt_hash, 0, 0);
        self.active_session_id
    }

    fn add_turn(
        &mut self,
        turn_type: TurnType,
        content_hash: u64,
        tokens: u32,
        timestamp: u64,
    ) -> u32 {
        self.current_turn = self.current_turn.saturating_add(1);
        let tool_call_id = if turn_type == TurnType::ToolCall || turn_type == TurnType::ToolResult {
            self.total_tool_calls = self.total_tool_calls.saturating_add(1);
            self.total_tool_calls
        } else {
            0
        };

        self.turns.push(ConversationTurn {
            turn_id: self.current_turn,
            turn_type,
            content_hash,
            token_count: tokens,
            timestamp,
            tool_call_id,
        });
        self.total_tokens_used = self.total_tokens_used.saturating_add(tokens as u64);
        self.current_turn
    }

    fn should_compress(&self) -> bool {
        let session_tokens: u32 = self.turns.iter().map(|t| t.token_count).sum();
        session_tokens > self.config.compress_threshold
    }

    fn compress_context(&mut self) {
        // Keep system prompt (first turn) and recent turns
        // Summarize everything in between
        if self.turns.len() > 10 {
            let keep_recent = 6;
            let first = self.turns[0]; // system prompt
            let recent: Vec<_> = self.turns[self.turns.len() - keep_recent..].to_vec();

            // Calculate summarized token count (rough estimate: 10% of original)
            let summarized_tokens: u32 = self.turns[1..self.turns.len() - keep_recent]
                .iter()
                .map(|t| t.token_count)
                .sum::<u32>()
                / 10;

            self.turns.clear();
            self.turns.push(first);
            // Add summary turn
            self.turns.push(ConversationTurn {
                turn_id: 0,
                turn_type: TurnType::ContextSummary,
                content_hash: 0xC0DE_55ED,
                token_count: summarized_tokens,
                timestamp: 0,
                tool_call_id: 0,
            });
            self.turns.extend_from_slice(&recent);
            self.compressed_count = self.compressed_count.saturating_add(1);
        }
    }

    fn should_stop(&self) -> Option<StopReason> {
        if self.current_turn >= self.config.max_turns {
            return Some(StopReason::MaxTurns);
        }
        let session_tokens: u32 = self.turns.iter().map(|t| t.token_count).sum();
        if session_tokens > self.config.max_tokens {
            return Some(StopReason::TokenLimit);
        }
        if self.state == LoopState::Done {
            return Some(StopReason::Complete);
        }
        None
    }

    fn step(&mut self, _timestamp: u64) -> LoopState {
        // Check stopping conditions
        if let Some(_reason) = self.should_stop() {
            self.state = LoopState::Done;
            return self.state;
        }
        // Auto-compress if needed
        if self.should_compress() {
            self.compress_context();
        }
        // Transition state machine
        self.state = match self.state {
            LoopState::Idle => LoopState::Thinking,
            LoopState::Thinking => LoopState::Acting, // After LLM response
            LoopState::Acting => LoopState::Streaming, // Tool executed
            LoopState::Streaming => LoopState::Thinking, // Ready for next turn
            LoopState::WaitingApproval => LoopState::WaitingApproval, // Stay until approved
            LoopState::Paused => LoopState::Paused,
            LoopState::Done => LoopState::Done,
        };
        self.state
    }

    fn pause(&mut self) {
        self.state = LoopState::Paused;
    }
    fn resume(&mut self) {
        self.state = LoopState::Thinking;
    }
    fn interrupt(&mut self) {
        self.state = LoopState::Done;
    }

    fn get_turn_count(&self) -> u32 {
        self.current_turn
    }
    fn get_state(&self) -> LoopState {
        self.state
    }
}

pub fn init() {
    let mut al = AGENT_LOOP.lock();
    *al = Some(AgenticLoop::new());
    serial_println!("    Agentic loop: observe-think-act cycle, context compression ready");
}
