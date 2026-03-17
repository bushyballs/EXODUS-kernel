use crate::sync::Mutex;
/// Real-time streaming for Genesis agent
///
/// Token-by-token output streaming, progress reporting,
/// live tool call visualization, event subscriptions.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum StreamEvent {
    TokenGenerated, // New token from LLM
    ToolCallStart,  // Agent started a tool call
    ToolCallEnd,    // Tool call completed
    ThinkingStart,  // Agent is planning
    ThinkingEnd,
    FileChanged, // A file was modified
    ErrorOccurred,
    ProgressUpdate, // Percentage progress on long task
    AgentSpawned,   // New sub-agent created
    AgentCompleted, // Sub-agent finished
    SessionEvent,   // Session state change
}

#[derive(Clone, Copy)]
struct StreamMessage {
    id: u64,
    event: StreamEvent,
    payload_hash: u64,
    timestamp: u64,
    sequence: u32,
    session_id: u32,
    agent_id: u32,
}

#[derive(Clone, Copy)]
struct StreamSubscription {
    subscriber_id: u32,
    events_mask: u16, // Bitmask of StreamEvent types
    active: bool,
    messages_delivered: u64,
}

#[derive(Clone, Copy)]
struct ProgressInfo {
    task_hash: u64,
    current: u32,
    total: u32,
    phase_hash: u64,
    eta_seconds: u32,
}

struct StreamManager {
    buffer: Vec<StreamMessage>,
    subscriptions: Vec<StreamSubscription>,
    progress: Option<ProgressInfo>,
    next_msg_id: u64,
    next_sub_id: u32,
    sequence: u32,
    buffer_max: usize,
    // Token streaming state
    tokens_streamed: u64,
    tokens_per_second: u32,
    streaming_active: bool,
    // Rate limiting
    max_events_per_sec: u32,
    events_this_second: u32,
    current_second: u64,
}

static STREAM_MGR: Mutex<Option<StreamManager>> = Mutex::new(None);

impl StreamManager {
    fn new() -> Self {
        StreamManager {
            buffer: Vec::new(),
            subscriptions: Vec::new(),
            progress: None,
            next_msg_id: 1,
            next_sub_id: 1,
            sequence: 0,
            buffer_max: 1000,
            tokens_streamed: 0,
            tokens_per_second: 0,
            streaming_active: false,
            max_events_per_sec: 100,
            events_this_second: 0,
            current_second: 0,
        }
    }

    fn emit(
        &mut self,
        event: StreamEvent,
        payload_hash: u64,
        session_id: u32,
        agent_id: u32,
        timestamp: u64,
    ) -> u64 {
        // Rate limiting
        let second = timestamp / 1000;
        if second != self.current_second {
            self.current_second = second;
            self.events_this_second = 0;
        }
        if self.events_this_second >= self.max_events_per_sec {
            return 0; // Dropped
        }
        self.events_this_second = self.events_this_second.saturating_add(1);

        let msg_id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);
        self.sequence = self.sequence.saturating_add(1);

        let msg = StreamMessage {
            id: msg_id,
            event,
            payload_hash,
            timestamp,
            sequence: self.sequence,
            session_id,
            agent_id,
        };

        // Circular buffer
        if self.buffer.len() >= self.buffer_max {
            self.buffer.remove(0);
        }
        self.buffer.push(msg);

        // Track token metrics
        if event == StreamEvent::TokenGenerated {
            self.tokens_streamed = self.tokens_streamed.saturating_add(1);
        }

        msg_id
    }

    fn subscribe(&mut self, events_mask: u16) -> u32 {
        let id = self.next_sub_id;
        self.next_sub_id = self.next_sub_id.saturating_add(1);
        self.subscriptions.push(StreamSubscription {
            subscriber_id: id,
            events_mask,
            active: true,
            messages_delivered: 0,
        });
        id
    }

    fn unsubscribe(&mut self, sub_id: u32) {
        if let Some(s) = self
            .subscriptions
            .iter_mut()
            .find(|s| s.subscriber_id == sub_id)
        {
            s.active = false;
        }
    }

    fn poll(&self, sub_id: u32, since_sequence: u32) -> Vec<StreamMessage> {
        let sub = self
            .subscriptions
            .iter()
            .find(|s| s.subscriber_id == sub_id);
        if let Some(s) = sub {
            if !s.active {
                return Vec::new();
            }
            self.buffer
                .iter()
                .filter(|m| m.sequence > since_sequence)
                .filter(|m| {
                    let event_bit = 1u16 << (m.event as u16);
                    s.events_mask & event_bit != 0 || s.events_mask == 0xFFFF
                })
                .copied()
                .collect()
        } else {
            Vec::new()
        }
    }

    fn update_progress(&mut self, task_hash: u64, current: u32, total: u32, phase_hash: u64) {
        let eta = if current > 0 {
            ((total - current) as u64 * 2) as u32 // Rough estimate: 2 sec per unit
        } else {
            0
        };
        self.progress = Some(ProgressInfo {
            task_hash,
            current,
            total,
            phase_hash,
            eta_seconds: eta,
        });
    }

    fn get_progress(&self) -> Option<ProgressInfo> {
        self.progress
    }

    fn start_streaming(&mut self) {
        self.streaming_active = true;
    }
    fn stop_streaming(&mut self) {
        self.streaming_active = false;
    }

    fn get_tokens_per_second(&self) -> u32 {
        self.tokens_per_second
    }

    fn update_tps(&mut self, tokens_last_second: u32) {
        self.tokens_per_second = tokens_last_second;
    }
}

pub fn init() {
    let mut sm = STREAM_MGR.lock();
    *sm = Some(StreamManager::new());
    serial_println!("    Streaming: real-time events, progress, token metrics ready");
}
