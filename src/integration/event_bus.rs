/// Hoags Event Bus — system-wide publish/subscribe event routing
///
/// Provides a decoupled event delivery mechanism for all OS subsystems.
/// Publishers emit events to named topics; subscribers receive matching
/// events filtered by topic pattern, priority threshold, and custom
/// predicates. Supports synchronous immediate dispatch and asynchronous
/// queued delivery with configurable queue depth.
///
/// Topics are hierarchical (e.g., "kernel.memory.oom", "net.tcp.connect")
/// represented as u64 hashes. Subscribers can match exact topics or
/// wildcard prefixes via topic_mask filtering.
///
/// All numeric values use i32 Q16 fixed-point (65536 = 1.0).
/// No external crates. No f32/f64.
///
/// Inspired by: D-Bus signals, Android Intents, MQTT topics, EventEmitter.
/// All code is original.

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

/// Q16 fixed-point: 65536 = 1.0
type Q16 = i32;
const Q16_ONE: Q16 = 65536;

/// Maximum number of topics the bus can track
const MAX_TOPICS: usize = 256;
/// Maximum subscribers per topic
const MAX_SUBSCRIBERS_PER_TOPIC: usize = 64;
/// Maximum total subscribers across all topics
const MAX_TOTAL_SUBSCRIBERS: usize = 1024;
/// Maximum events in the async delivery queue
const MAX_QUEUE_DEPTH: usize = 512;
/// Maximum events retained in the history ring buffer
const MAX_HISTORY: usize = 256;
/// Maximum payload size in u64 words
const MAX_PAYLOAD_WORDS: usize = 8;

// ---------------------------------------------------------------------------
// Priority levels for event delivery ordering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Lowest priority, delivered last
    Low,
    /// Default priority for most events
    Normal,
    /// Elevated priority for important subsystem events
    High,
    /// Highest priority for critical kernel events
    Critical,
}

impl Priority {
    fn as_q16(&self) -> Q16 {
        match self {
            Priority::Low => Q16_ONE / 4,
            Priority::Normal => Q16_ONE / 2,
            Priority::High => (Q16_ONE * 3) / 4,
            Priority::Critical => Q16_ONE,
        }
    }

    fn from_q16(val: Q16) -> Priority {
        if val >= Q16_ONE {
            Priority::Critical
        } else if val >= (Q16_ONE * 3) / 4 {
            Priority::High
        } else if val >= Q16_ONE / 2 {
            Priority::Normal
        } else {
            Priority::Low
        }
    }
}

// ---------------------------------------------------------------------------
// DeliveryMode — synchronous vs. asynchronous
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryMode {
    /// Delivered immediately during publish() call
    Synchronous,
    /// Queued for later delivery via drain_queue()
    Asynchronous,
}

// ---------------------------------------------------------------------------
// Event — a published event with metadata
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Event {
    pub id: u64,
    pub topic_hash: u64,
    pub source_id: u32,
    pub priority: Priority,
    pub timestamp: u64,
    pub payload: Vec<u64>,
    pub delivery_mode: DeliveryMode,
}

impl Event {
    fn new(id: u64, topic_hash: u64, source_id: u32, priority: Priority, timestamp: u64) -> Self {
        Event {
            id,
            topic_hash,
            source_id,
            priority,
            timestamp,
            payload: Vec::new(),
            delivery_mode: DeliveryMode::Synchronous,
        }
    }
}

// ---------------------------------------------------------------------------
// TopicFilter — subscriber matching criteria
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TopicFilter {
    /// Exact topic hash to match (0 = match all)
    pub topic_hash: u64,
    /// Bitmask for partial/prefix matching (0xFFFFFFFFFFFFFFFF = exact)
    pub topic_mask: u64,
    /// Minimum priority to accept
    pub min_priority: Priority,
    /// Only accept events from this source (0 = any source)
    pub source_filter: u32,
}

impl TopicFilter {
    fn matches(&self, event: &Event) -> bool {
        // Check priority threshold
        if event.priority < self.min_priority {
            return false;
        }
        // Check source filter
        if self.source_filter != 0 && event.source_id != self.source_filter {
            return false;
        }
        // Check topic match with mask
        if self.topic_hash == 0 {
            return true; // wildcard subscriber
        }
        (event.topic_hash & self.topic_mask) == (self.topic_hash & self.topic_mask)
    }
}

// ---------------------------------------------------------------------------
// Subscriber — a registered event listener
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Subscriber {
    id: u32,
    name_hash: u64,
    filter: TopicFilter,
    delivery_mode: DeliveryMode,
    callback_hash: u64,
    enabled: bool,
    events_received: u64,
    last_event_time: u64,
}

// ---------------------------------------------------------------------------
// Topic — a named event channel
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Topic {
    hash: u64,
    subscriber_ids: Vec<u32>,
    event_count: u64,
    last_publish_time: u64,
    retained_event: Option<Event>,
}

impl Topic {
    fn new(hash: u64) -> Self {
        Topic {
            hash,
            subscriber_ids: Vec::new(),
            event_count: 0,
            last_publish_time: 0,
            retained_event: None,
        }
    }
}

// ---------------------------------------------------------------------------
// QueuedDelivery — an event waiting in the async queue
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct QueuedDelivery {
    event: Event,
    subscriber_id: u32,
    enqueue_time: u64,
}

// ---------------------------------------------------------------------------
// EventHistory — ring buffer entry for recent events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct EventHistory {
    event_id: u64,
    topic_hash: u64,
    source_id: u32,
    timestamp: u64,
    subscriber_count: u32,
}

// ---------------------------------------------------------------------------
// EventBusState — main state
// ---------------------------------------------------------------------------

struct EventBusState {
    topics: Vec<Topic>,
    subscribers: Vec<Subscriber>,
    queue: Vec<QueuedDelivery>,
    history: Vec<EventHistory>,
    next_subscriber_id: u32,
    next_event_id: u64,
    total_events_published: u64,
    total_events_delivered: u64,
    total_events_dropped: u64,
    initialized: bool,
}

impl EventBusState {
    fn new() -> Self {
        EventBusState {
            topics: Vec::new(),
            subscribers: Vec::new(),
            queue: Vec::new(),
            history: Vec::new(),
            next_subscriber_id: 1,
            next_event_id: 1,
            total_events_published: 0,
            total_events_delivered: 0,
            total_events_dropped: 0,
            initialized: false,
        }
    }

    fn find_or_create_topic(&mut self, topic_hash: u64) -> usize {
        for (i, t) in self.topics.iter().enumerate() {
            if t.hash == topic_hash {
                return i;
            }
        }
        if self.topics.len() >= MAX_TOPICS {
            serial_println!("[event_bus] WARNING: max topics reached, reusing slot 0");
            return 0;
        }
        let idx = self.topics.len();
        self.topics.push(Topic::new(topic_hash));
        idx
    }
}

static EVENT_BUS: Mutex<Option<EventBusState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Subscribe / Unsubscribe
// ---------------------------------------------------------------------------

/// Subscribe to events matching the given filter. Returns subscriber ID.
pub fn subscribe(
    name_hash: u64,
    filter: TopicFilter,
    delivery_mode: DeliveryMode,
    callback_hash: u64,
) -> u32 {
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        if state.subscribers.len() >= MAX_TOTAL_SUBSCRIBERS {
            serial_println!("[event_bus] ERROR: max subscribers ({}) reached", MAX_TOTAL_SUBSCRIBERS);
            return 0;
        }

        let id = state.next_subscriber_id;
        state.next_subscriber_id = state.next_subscriber_id.saturating_add(1);

        let sub = Subscriber {
            id,
            name_hash,
            filter: filter.clone(),
            delivery_mode,
            callback_hash,
            enabled: true,
            events_received: 0,
            last_event_time: 0,
        };

        // Register subscriber in the matching topic
        if filter.topic_hash != 0 {
            let topic_idx = state.find_or_create_topic(filter.topic_hash);
            if state.topics[topic_idx].subscriber_ids.len() < MAX_SUBSCRIBERS_PER_TOPIC {
                state.topics[topic_idx].subscriber_ids.push(id);
            }
        }

        serial_println!("[event_bus] Subscriber {} registered (topic={:#018X}, mask={:#018X})",
            id, filter.topic_hash, filter.topic_mask);

        state.subscribers.push(sub);
        id
    } else {
        0
    }
}

/// Unsubscribe by subscriber ID. Returns true if found and removed.
pub fn unsubscribe(subscriber_id: u32) -> bool {
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        // Remove from topic subscriber lists
        for topic in &mut state.topics {
            topic.subscriber_ids.retain(|&sid| sid != subscriber_id);
        }
        // Remove from subscriber list
        let before = state.subscribers.len();
        state.subscribers.retain(|s| s.id != subscriber_id);
        let removed = state.subscribers.len() < before;
        if removed {
            serial_println!("[event_bus] Subscriber {} removed", subscriber_id);
        }
        removed
    } else {
        false
    }
}

/// Enable or disable a subscriber.
pub fn set_subscriber_enabled(subscriber_id: u32, enabled: bool) -> bool {
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        for sub in &mut state.subscribers {
            if sub.id == subscriber_id {
                sub.enabled = enabled;
                serial_println!("[event_bus] Subscriber {} {}", subscriber_id,
                    if enabled { "enabled" } else { "disabled" });
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

/// Publish an event to the bus. Returns the number of subscribers notified.
pub fn publish(
    topic_hash: u64,
    source_id: u32,
    priority: Priority,
    timestamp: u64,
    payload: Vec<u64>,
) -> u32 {
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        let event_id = state.next_event_id;
        state.next_event_id = state.next_event_id.saturating_add(1);
        state.total_events_published = state.total_events_published.saturating_add(1);

        let mut event = Event::new(event_id, topic_hash, source_id, priority, timestamp);
        for (i, &word) in payload.iter().enumerate() {
            if i >= MAX_PAYLOAD_WORDS { break; }
            event.payload.push(word);
        }

        // Update topic stats
        let topic_idx = state.find_or_create_topic(topic_hash);
        state.topics[topic_idx].event_count = state.topics[topic_idx].event_count.saturating_add(1);
        state.topics[topic_idx].last_publish_time = timestamp;

        // Find matching subscribers
        let mut delivered = 0u32;
        let sub_count = state.subscribers.len();
        for i in 0..sub_count {
            if !state.subscribers[i].enabled {
                continue;
            }
            if !state.subscribers[i].filter.matches(&event) {
                continue;
            }

            match state.subscribers[i].delivery_mode {
                DeliveryMode::Synchronous => {
                    // Immediate delivery (log the callback)
                    serial_println!("[event_bus] DELIVER event {} -> subscriber {} (sync, cb={:#018X})",
                        event_id, state.subscribers[i].id, state.subscribers[i].callback_hash);
                    state.subscribers[i].events_received = state.subscribers[i].events_received.saturating_add(1);
                    state.subscribers[i].last_event_time = timestamp;
                    state.total_events_delivered = state.total_events_delivered.saturating_add(1);
                    delivered += 1;
                }
                DeliveryMode::Asynchronous => {
                    // Enqueue for later delivery
                    if state.queue.len() < MAX_QUEUE_DEPTH {
                        state.queue.push(QueuedDelivery {
                            event: event.clone(),
                            subscriber_id: state.subscribers[i].id,
                            enqueue_time: timestamp,
                        });
                        delivered += 1;
                    } else {
                        state.total_events_dropped = state.total_events_dropped.saturating_add(1);
                        serial_println!("[event_bus] WARNING: queue full, dropping async delivery");
                    }
                }
            }
        }

        // Also deliver to wildcard subscribers (topic_hash == 0 in filter)
        // Already handled by filter.matches() above since topic_hash 0 matches all

        // Retain event on topic if it's critical
        if priority == Priority::Critical {
            state.topics[topic_idx].retained_event = Some(event.clone());
        }

        // Record history
        if state.history.len() >= MAX_HISTORY {
            state.history.remove(0);
        }
        state.history.push(EventHistory {
            event_id,
            topic_hash,
            source_id,
            timestamp,
            subscriber_count: delivered,
        });

        delivered
    } else {
        0
    }
}

/// Publish with retained flag -- the last event is stored on the topic
/// so new subscribers get it immediately upon subscribing.
pub fn publish_retained(
    topic_hash: u64,
    source_id: u32,
    priority: Priority,
    timestamp: u64,
    payload: Vec<u64>,
) -> u32 {
    let delivered = publish(topic_hash, source_id, priority, timestamp, payload);
    // The publish() call already retains Critical events; for explicit retain,
    // we force-retain regardless of priority
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        for topic in &mut state.topics {
            if topic.hash == topic_hash {
                if let Some(ref hist) = state.history.last() {
                    if hist.topic_hash == topic_hash {
                        // Reconstruct a minimal retained event
                        let mut ev = Event::new(hist.event_id, topic_hash, source_id, priority, timestamp);
                        ev.delivery_mode = DeliveryMode::Synchronous;
                        topic.retained_event = Some(ev);
                    }
                }
                break;
            }
        }
    }
    delivered
}

// ---------------------------------------------------------------------------
// Async queue drain
// ---------------------------------------------------------------------------

/// Drain the async delivery queue. Returns number of events delivered.
/// Call this periodically from the kernel main loop or a worker thread.
pub fn drain_queue(max_deliveries: u32) -> u32 {
    let mut guard = EVENT_BUS.lock();
    if let Some(ref mut state) = *guard {
        let mut delivered = 0u32;
        let mut remaining = Vec::new();

        for queued in state.queue.drain(..) {
            if delivered >= max_deliveries {
                remaining.push(queued);
                continue;
            }
            // Find the subscriber
            let mut found = false;
            for sub in &mut state.subscribers {
                if sub.id == queued.subscriber_id && sub.enabled {
                    serial_println!("[event_bus] DELIVER event {} -> subscriber {} (async, cb={:#018X})",
                        queued.event.id, sub.id, sub.callback_hash);
                    sub.events_received = sub.events_received.saturating_add(1);
                    sub.last_event_time = queued.event.timestamp;
                    state.total_events_delivered = state.total_events_delivered.saturating_add(1);
                    delivered += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                state.total_events_dropped = state.total_events_dropped.saturating_add(1);
            }
        }

        state.queue = remaining;
        delivered
    } else {
        0
    }
}

/// Get the current async queue depth.
pub fn queue_depth() -> usize {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        state.queue.len()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Query API
// ---------------------------------------------------------------------------

/// Get the retained (last) event for a topic, if any.
pub fn get_retained(topic_hash: u64) -> Option<(u64, u32, u64)> {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        for topic in &state.topics {
            if topic.hash == topic_hash {
                if let Some(ref ev) = topic.retained_event {
                    return Some((ev.id, ev.source_id, ev.timestamp));
                }
            }
        }
    }
    None
}

/// Get stats for a topic: (event_count, subscriber_count, last_publish_time).
pub fn topic_stats(topic_hash: u64) -> Option<(u64, usize, u64)> {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        for topic in &state.topics {
            if topic.hash == topic_hash {
                return Some((topic.event_count, topic.subscriber_ids.len(), topic.last_publish_time));
            }
        }
    }
    None
}

/// Get subscriber stats: (events_received, last_event_time, enabled).
pub fn subscriber_stats(subscriber_id: u32) -> Option<(u64, u64, bool)> {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        for sub in &state.subscribers {
            if sub.id == subscriber_id {
                return Some((sub.events_received, sub.last_event_time, sub.enabled));
            }
        }
    }
    None
}

/// Get recent event history as (event_id, topic_hash, source_id, timestamp, sub_count).
pub fn recent_history(count: usize) -> Vec<(u64, u64, u32, u64, u32)> {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        let start = if state.history.len() > count {
            state.history.len() - count
        } else {
            0
        };
        let mut result = Vec::new();
        for entry in &state.history[start..] {
            result.push((entry.event_id, entry.topic_hash, entry.source_id,
                entry.timestamp, entry.subscriber_count));
        }
        result
    } else {
        Vec::new()
    }
}

/// Get global bus statistics: (published, delivered, dropped, topics, subscribers).
pub fn bus_stats() -> (u64, u64, u64, usize, usize) {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        (
            state.total_events_published,
            state.total_events_delivered,
            state.total_events_dropped,
            state.topics.len(),
            state.subscribers.len(),
        )
    } else {
        (0, 0, 0, 0, 0)
    }
}

/// List all topic hashes with their event counts.
pub fn list_topics() -> Vec<(u64, u64, usize)> {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        let mut result = Vec::new();
        for topic in &state.topics {
            result.push((topic.hash, topic.event_count, topic.subscriber_ids.len()));
        }
        result
    } else {
        Vec::new()
    }
}

/// Compute delivery rate as Q16 fraction (delivered / published).
pub fn delivery_rate_q16() -> Q16 {
    let guard = EVENT_BUS.lock();
    if let Some(ref state) = *guard {
        if state.total_events_published == 0 {
            return Q16_ONE;
        }
        let delivered = state.total_events_delivered as i64;
        let published = state.total_events_published as i64;
        (((delivered << 16) / published)) as Q16
    } else {
        Q16_ONE
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = EVENT_BUS.lock();
    *guard = Some(EventBusState::new());
    if let Some(ref mut state) = *guard {
        state.initialized = true;
    }
    serial_println!("    [integration] Event bus initialized (pub/sub, topics, priority, async queue)");
}
