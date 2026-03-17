use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::string::String;
/// Neural Bus — inter-subsystem event routing for the AI stack
///
/// The neural bus is a publish/subscribe message bus that connects all AI
/// subsystems (inference, embeddings, NLP, RAG, vision, voice, anomaly,
/// classifier, etc.).  Every AI module can publish events and subscribe to
/// topics.  The bus is intentionally lightweight:
///
///   - No heap allocation per message (events are copy-on-send u64 payloads)
///   - Static subscription table (up to MAX_SUBSCRIBERS per topic)
///   - Ring-buffer of recent events per topic (last N events retained)
///   - Single global Mutex guard (no per-topic locks to keep things simple)
///   - no_std compatible — no std::thread, no channels, no async
///
/// Usage:
///   // In a module's init():
///   neural_bus::subscribe("inference.output", my_handler);
///
///   // When an event occurs:
///   neural_bus::publish(NeuralEvent { topic: NeuralTopic::InferenceOutput,
///                                     source_id: SUBSYSTEM_INFERENCE,
///                                     payload: token_id as u64,
///                                     timestamp: 0 });
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of subscribers across all topics
const MAX_SUBSCRIBERS: usize = 64;

/// Maximum number of events retained in the per-topic ring buffer
const RING_BUFFER_SIZE: usize = 32;

/// Maximum number of distinct topic slots in the bus
const MAX_TOPICS: usize = 32;

// ---------------------------------------------------------------------------
// Hebbian learning constants
// ---------------------------------------------------------------------------

pub const HEBBIAN_WINDOW: u64 = 100; // ticks — co-fire window
pub const MAX_SYNAPSES: usize = 128; // max tracked connections
pub const WEIGHT_INCREMENT: u16 = 10; // weight gain per co-fire
pub const WEIGHT_DECAY: u16 = 1; // weight loss per tick of silence

// ---------------------------------------------------------------------------
// Subsystem source IDs
// ---------------------------------------------------------------------------

pub const SUBSYSTEM_INFERENCE: u16 = 0x0001;
pub const SUBSYSTEM_EMBEDDINGS: u16 = 0x0002;
pub const SUBSYSTEM_NLP: u16 = 0x0003;
pub const SUBSYSTEM_RAG: u16 = 0x0004;
pub const SUBSYSTEM_VISION: u16 = 0x0005;
pub const SUBSYSTEM_VOICE: u16 = 0x0006;
pub const SUBSYSTEM_ANOMALY: u16 = 0x0007;
pub const SUBSYSTEM_CLASSIFIER: u16 = 0x0008;
pub const SUBSYSTEM_ASSISTANT: u16 = 0x0009;
pub const SUBSYSTEM_TRAINING: u16 = 0x000A;
pub const SUBSYSTEM_SELF_IMPR: u16 = 0x000B;
pub const SUBSYSTEM_KNOWLEDGE: u16 = 0x000C;
pub const SUBSYSTEM_REASONING: u16 = 0x000D;
pub const SUBSYSTEM_PLANNING: u16 = 0x000E;
pub const SUBSYSTEM_SAFETY: u16 = 0x000F;
pub const SUBSYSTEM_ROUTER: u16 = 0x0010;
pub const SUBSYSTEM_FEEDBACK: u16 = 0x0011;
pub const SUBSYSTEM_SENTIMENT: u16 = 0x0012;
pub const SUBSYSTEM_SUMMARIZER: u16 = 0x0013;
pub const SUBSYSTEM_EXTERNAL: u16 = 0xFFFF;

// ---------------------------------------------------------------------------
// NeuralTopic — well-known event topics
// ---------------------------------------------------------------------------

/// Well-known topics on the neural bus.
/// Custom/extension topics can be represented as `NeuralTopic::Custom(id)`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NeuralTopic {
    /// Inference engine produced a new token (payload = token id)
    InferenceOutput,
    /// Inference engine finished a full generation run (payload = token count)
    InferenceDone,
    /// An anomaly was detected in a subsystem (payload = subsystem_id << 16 | severity_q8)
    AnomalyDetected,
    /// A classification result is available (payload = label hash)
    ClassificationResult,
    /// Embedding vector produced (payload = embedding buffer id)
    EmbeddingReady,
    /// Voice activity detected / wake word fired (payload = 0)
    VoiceActivity,
    /// RAG retrieval completed (payload = chunk count)
    RagRetrievalDone,
    /// Vision inference completed (payload = class id)
    VisionResult,
    /// Safety filter blocked content (payload = risk score Q8)
    SafetyBlock,
    /// Feedback signal received for RLHF (payload = reward Q16)
    FeedbackSignal,
    /// Knowledge graph updated (payload = node count delta)
    KnowledgeUpdate,
    /// Training step completed (payload = loss Q16)
    TrainingStep,
    /// Self-improvement cycle completed (payload = improvement score Q16)
    SelfImproveCycle,
    /// Sentiment analysis result (payload = sentiment score in Q8, 0=neg..255=pos)
    SentimentResult,
    /// Model router selected a new model (payload = model id hash low 32 bits)
    ModelSelected,
    /// Subsystem health check (payload = health_score Q8)
    HealthPing,
    /// Custom/extension topic — callers supply their own numeric id
    Custom(u32),
}

impl NeuralTopic {
    /// Unique numeric id for this topic (used as array index)
    pub fn id(&self) -> u32 {
        match self {
            NeuralTopic::InferenceOutput => 0,
            NeuralTopic::InferenceDone => 1,
            NeuralTopic::AnomalyDetected => 2,
            NeuralTopic::ClassificationResult => 3,
            NeuralTopic::EmbeddingReady => 4,
            NeuralTopic::VoiceActivity => 5,
            NeuralTopic::RagRetrievalDone => 6,
            NeuralTopic::VisionResult => 7,
            NeuralTopic::SafetyBlock => 8,
            NeuralTopic::FeedbackSignal => 9,
            NeuralTopic::KnowledgeUpdate => 10,
            NeuralTopic::TrainingStep => 11,
            NeuralTopic::SelfImproveCycle => 12,
            NeuralTopic::SentimentResult => 13,
            NeuralTopic::ModelSelected => 14,
            NeuralTopic::HealthPing => 15,
            NeuralTopic::Custom(id) => 16 + (id % (MAX_TOPICS as u32 - 16)),
        }
    }

    /// Human-readable topic name for serial logging
    pub fn name(&self) -> &'static str {
        match self {
            NeuralTopic::InferenceOutput => "inference.output",
            NeuralTopic::InferenceDone => "inference.done",
            NeuralTopic::AnomalyDetected => "anomaly.detected",
            NeuralTopic::ClassificationResult => "classifier.result",
            NeuralTopic::EmbeddingReady => "embeddings.ready",
            NeuralTopic::VoiceActivity => "voice.activity",
            NeuralTopic::RagRetrievalDone => "rag.done",
            NeuralTopic::VisionResult => "vision.result",
            NeuralTopic::SafetyBlock => "safety.block",
            NeuralTopic::FeedbackSignal => "feedback.signal",
            NeuralTopic::KnowledgeUpdate => "knowledge.update",
            NeuralTopic::TrainingStep => "training.step",
            NeuralTopic::SelfImproveCycle => "self_improve.cycle",
            NeuralTopic::SentimentResult => "sentiment.result",
            NeuralTopic::ModelSelected => "router.model_selected",
            NeuralTopic::HealthPing => "health.ping",
            NeuralTopic::Custom(_) => "custom",
        }
    }
}

// ---------------------------------------------------------------------------
// NeuralEvent
// ---------------------------------------------------------------------------

/// A single event published on the neural bus
#[derive(Clone, Copy)]
pub struct NeuralEvent {
    /// Which topic this event belongs to
    pub topic: NeuralTopic,
    /// Which subsystem produced this event (SUBSYSTEM_* constant)
    pub source_id: u16,
    /// Event payload — interpretation is topic-specific
    pub payload: u64,
    /// Monotonic timestamp (bus tick counter)
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Hebbian learning — SynapticWeight & HebbianLayer
// ---------------------------------------------------------------------------

/// Hebbian weight between two subsystems.
/// When both fire within HEBBIAN_WINDOW ticks, weight increases.
/// When one fires without the other, weight decays.
#[derive(Clone, Copy)]
pub struct SynapticWeight {
    pub source: u16,       // source subsystem id
    pub target: u16,       // target subsystem id
    pub weight: u16,       // 0-1000 connection strength
    pub last_co_fire: u64, // timestamp of last co-activation
    pub fire_count: u32,   // total co-fires
}

pub struct HebbianLayer {
    synapses: [SynapticWeight; MAX_SYNAPSES],
    count: usize,
    tick: u64,
}

impl HebbianLayer {
    pub const fn empty() -> Self {
        HebbianLayer {
            synapses: [SynapticWeight {
                source: 0,
                target: 0,
                weight: 0,
                last_co_fire: 0,
                fire_count: 0,
            }; MAX_SYNAPSES],
            count: 0,
            tick: 0,
        }
    }

    /// Record that `source` fired at `timestamp`.
    /// Strengthens synapses where the other endpoint also fired within
    /// HEBBIAN_WINDOW ticks, and creates new cross-synapses between recently
    /// co-active subsystems.
    pub fn record_fire(&mut self, source: u16, timestamp: u64) {
        self.tick = timestamp;

        // Strengthen existing synapses where source is one endpoint and the
        // other end also fired within the co-fire window.
        for i in 0..self.count {
            if self.synapses[i].source == source || self.synapses[i].target == source {
                let gap = timestamp.saturating_sub(self.synapses[i].last_co_fire);
                if gap > 0 && gap <= HEBBIAN_WINDOW {
                    let new_w =
                        (self.synapses[i].weight as u32 + WEIGHT_INCREMENT as u32).min(1000) as u16;
                    self.synapses[i].weight = new_w;
                    self.synapses[i].fire_count = self.synapses[i].fire_count.saturating_add(1);
                }
                self.synapses[i].last_co_fire = timestamp;
            }
        }

        // Collect other sources that fired within HEBBIAN_WINDOW (via their self-synapses).
        let mut recent_others: [u16; 8] = [0; 8];
        let mut recent_count = 0usize;
        for i in 0..self.count {
            let s = &self.synapses[i];
            if s.source == s.target && s.source != source && recent_count < 8 {
                let gap = timestamp.saturating_sub(s.last_co_fire);
                if gap > 0 && gap <= HEBBIAN_WINDOW {
                    recent_others[recent_count] = s.source;
                    recent_count += 1;
                }
            }
        }

        // Ensure a self-synapse exists for this source (tracks last-fire time).
        let has_self = (0..self.count)
            .any(|i| self.synapses[i].source == source && self.synapses[i].target == source);
        if !has_self && self.count < MAX_SYNAPSES {
            self.synapses[self.count] = SynapticWeight {
                source,
                target: source,
                weight: 0,
                last_co_fire: timestamp,
                fire_count: 0,
            };
            self.count += 1;
        }

        // Create directed cross-synapses with recently co-active sources.
        for k in 0..recent_count {
            let other = recent_others[k];
            let exists = (0..self.count).any(|i| {
                (self.synapses[i].source == other && self.synapses[i].target == source)
                    || (self.synapses[i].source == source && self.synapses[i].target == other)
            });
            if !exists && self.count < MAX_SYNAPSES {
                self.synapses[self.count] = SynapticWeight {
                    source: other,
                    target: source,
                    weight: WEIGHT_INCREMENT,
                    last_co_fire: timestamp,
                    fire_count: 1,
                };
                self.count += 1;
            }
        }
    }

    /// Decay all synapse weights by WEIGHT_DECAY (floor 0). Call periodically.
    pub fn decay_all(&mut self) {
        for i in 0..self.count {
            self.synapses[i].weight = self.synapses[i].weight.saturating_sub(WEIGHT_DECAY);
        }
    }

    /// Return the connection strength between source and target (0 if not found).
    pub fn weight_between(&self, source: u16, target: u16) -> u16 {
        for i in 0..self.count {
            if self.synapses[i].source == source && self.synapses[i].target == target {
                return self.synapses[i].weight;
            }
        }
        0
    }

    /// Return the highest-weight synapse, or None if the table is empty.
    pub fn strongest_connection(&self) -> Option<SynapticWeight> {
        if self.count == 0 {
            return None;
        }
        let mut best = 0usize;
        for i in 1..self.count {
            if self.synapses[i].weight > self.synapses[best].weight {
                best = i;
            }
        }
        Some(self.synapses[best])
    }

    /// Print the top 5 connections by weight to the serial console.
    pub fn report(&self) {
        serial_println!(
            "[hebbian] synapses={}/{} tick={}",
            self.count,
            MAX_SYNAPSES,
            self.tick
        );
        if self.count == 0 {
            serial_println!("[hebbian] (no synapses recorded yet)");
            return;
        }
        let mut top: [usize; 5] = [0; 5];
        let mut top_len = 0usize;
        for i in 0..self.count {
            if top_len < 5 {
                top[top_len] = i;
                top_len += 1;
            } else {
                let mut min_j = 0usize;
                for j in 1..5 {
                    if self.synapses[top[j]].weight < self.synapses[top[min_j]].weight {
                        min_j = j;
                    }
                }
                if self.synapses[i].weight > self.synapses[top[min_j]].weight {
                    top[min_j] = i;
                }
            }
        }
        serial_println!("[hebbian] top {} connections:", top_len);
        for k in 0..top_len {
            let s = &self.synapses[top[k]];
            serial_println!(
                "  {:04x}->{:04x}  weight={}  co_fires={}",
                s.source,
                s.target,
                s.weight,
                s.fire_count
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Internal bus state
// ---------------------------------------------------------------------------

/// Type alias for a subscriber callback
type SubscriberFn = fn(NeuralEvent);

/// One subscriber record
struct Subscriber {
    topic_id: u32,
    callback: SubscriberFn,
    /// Friendly name for debugging
    name_hash: u32,
}

/// Ring buffer for a single topic
struct TopicRing {
    topic_id: u32,
    events: [NeuralEvent; RING_BUFFER_SIZE],
    write_pos: usize,
    count: usize,
}

impl TopicRing {
    fn new(topic_id: u32) -> Self {
        TopicRing {
            topic_id,
            // SAFETY: NeuralEvent is Copy and contains only primitive types;
            // zero-initialising is valid.
            events: [NeuralEvent {
                topic: NeuralTopic::Custom(0),
                source_id: 0,
                payload: 0,
                timestamp: 0,
            }; RING_BUFFER_SIZE],
            write_pos: 0,
            count: 0,
        }
    }

    fn push(&mut self, event: NeuralEvent) {
        self.events[self.write_pos] = event;
        self.write_pos = (self.write_pos + 1) % RING_BUFFER_SIZE;
        if self.count < RING_BUFFER_SIZE {
            self.count += 1;
        }
    }

    /// Iterate events from oldest to newest
    fn iter_recent(&self, n: usize) -> impl Iterator<Item = &NeuralEvent> {
        let actual = n.min(self.count).min(RING_BUFFER_SIZE);
        // Start index (oldest of the last `actual` events)
        let start = if self.count >= RING_BUFFER_SIZE {
            self.write_pos // wrap-around: write_pos points to oldest
        } else {
            0
        };
        // We return events as a flat slice — the ring can wrap, so we collect
        // by index arithmetic.
        let events = &self.events;
        let total = RING_BUFFER_SIZE;
        let count = self.count;
        // Use a simple range over the last `actual` logical slots
        let skip = count.saturating_sub(actual);
        (0..count)
            .skip(skip)
            .map(move |i| &events[(start + i) % total])
    }
}

/// The global neural bus state
struct NeuralBusState {
    subscribers: Vec<Subscriber>,
    rings: Vec<TopicRing>,
    tick: u64,
    total_published: u64,
    total_delivered: u64,
}

impl NeuralBusState {
    fn new() -> Self {
        NeuralBusState {
            subscribers: Vec::new(),
            rings: Vec::new(),
            tick: 0,
            total_published: 0,
            total_delivered: 0,
        }
    }

    /// Get or create the ring buffer for a topic id
    fn ring_for(&mut self, topic_id: u32) -> &mut TopicRing {
        // Find existing
        for i in 0..self.rings.len() {
            if self.rings[i].topic_id == topic_id {
                return &mut self.rings[i];
            }
        }
        // Create new (capped at MAX_TOPICS)
        if self.rings.len() < MAX_TOPICS {
            self.rings.push(TopicRing::new(topic_id));
        }
        // Return the last element (just pushed, or oldest if at cap)
        let last = self.rings.len() - 1;
        &mut self.rings[last]
    }

    /// Publish an event: store in ring buffer, dispatch to all matching subscribers
    fn publish(&mut self, mut event: NeuralEvent) {
        self.tick = self.tick.saturating_add(1);
        event.timestamp = self.tick;
        self.total_published = self.total_published.saturating_add(1);

        let topic_id = event.topic.id();

        // Store in ring buffer
        self.ring_for(topic_id).push(event);

        // Dispatch to subscribers
        // We iterate by index to avoid borrow issues
        let n_subs = self.subscribers.len();
        for i in 0..n_subs {
            if self.subscribers[i].topic_id == topic_id {
                (self.subscribers[i].callback)(event);
                self.total_delivered = self.total_delivered.saturating_add(1);
            }
        }
    }

    /// Subscribe a callback to a topic. Returns a unique subscription handle (1-based index).
    /// Returns 0 if the subscriber table is full.
    fn subscribe(&mut self, topic_id: u32, callback: SubscriberFn, name_hash: u32) -> u32 {
        if self.subscribers.len() >= MAX_SUBSCRIBERS {
            return 0;
        }
        self.subscribers.push(Subscriber {
            topic_id,
            callback,
            name_hash,
        });
        self.subscribers.len() as u32
    }

    /// Remove a subscriber by the handle returned from subscribe()
    fn unsubscribe(&mut self, handle: u32) -> bool {
        let idx = handle as usize;
        if idx == 0 || idx > self.subscribers.len() {
            return false;
        }
        self.subscribers.remove(idx - 1);
        true
    }

    /// Count subscribers for a given topic
    fn subscriber_count(&self, topic_id: u32) -> usize {
        self.subscribers
            .iter()
            .filter(|s| s.topic_id == topic_id)
            .count()
    }
}

static BUS: Mutex<Option<NeuralBusState>> = Mutex::new(None);
pub static HEBBIAN: Mutex<HebbianLayer> = Mutex::new(HebbianLayer::empty());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the neural bus (must be called before any publish/subscribe).
pub fn init() {
    *BUS.lock() = Some(NeuralBusState::new());
    serial_println!(
        "    [neural_bus] Neural event bus ready ({} topic slots, {} max subscribers, {} event ring)",
        MAX_TOPICS, MAX_SUBSCRIBERS, RING_BUFFER_SIZE
    );
}

/// Publish an event to the bus.
///
/// The event is delivered synchronously to all matching subscribers in
/// registration order, then stored in the topic's ring buffer.
/// Safe to call from any AI subsystem.
pub fn publish(event: NeuralEvent) {
    let source_id = event.source_id;
    let ts = {
        let mut guard = BUS.lock();
        if let Some(bus) = guard.as_mut() {
            bus.publish(event);
            bus.tick
        } else {
            return;
        }
    };
    hebbian_fire(source_id, ts);
}

/// Convenience helper: publish with just a topic, source and payload.
pub fn emit(topic: NeuralTopic, source_id: u16, payload: u64) {
    publish(NeuralEvent {
        topic,
        source_id,
        payload,
        timestamp: 0,
    });
}

/// Subscribe a callback function to a topic.
///
/// `name_hash` is a caller-supplied identifier (e.g., FNV hash of the module
/// name) used only for diagnostic logging.
///
/// Returns a subscription handle (> 0) on success, or 0 if the table is full.
pub fn subscribe(topic: NeuralTopic, callback: SubscriberFn, name_hash: u32) -> u32 {
    BUS.lock()
        .as_mut()
        .map(|bus| bus.subscribe(topic.id(), callback, name_hash))
        .unwrap_or(0)
}

/// Unsubscribe using a handle previously returned by `subscribe()`.
pub fn unsubscribe(handle: u32) -> bool {
    BUS.lock()
        .as_mut()
        .map(|bus| bus.unsubscribe(handle))
        .unwrap_or(false)
}

/// Get the most recent event for a topic (if any).
pub fn last_event(topic: NeuralTopic) -> Option<NeuralEvent> {
    let guard = BUS.lock();
    guard.as_ref().and_then(|bus| {
        let topic_id = topic.id();
        bus.rings
            .iter()
            .find(|r| r.topic_id == topic_id)
            .and_then(|ring| ring.iter_recent(1).next().copied())
    })
}

/// Get the number of subscribers registered for a topic.
pub fn subscriber_count(topic: NeuralTopic) -> usize {
    BUS.lock()
        .as_ref()
        .map(|bus| bus.subscriber_count(topic.id()))
        .unwrap_or(0)
}

/// Get total events published since boot.
pub fn total_published() -> u64 {
    BUS.lock().as_ref().map(|b| b.total_published).unwrap_or(0)
}

/// Get total event deliveries (subscriber callback invocations) since boot.
pub fn total_delivered() -> u64 {
    BUS.lock().as_ref().map(|b| b.total_delivered).unwrap_or(0)
}

/// Current bus tick (monotonically increasing, incremented on each publish).
pub fn tick() -> u64 {
    BUS.lock().as_ref().map(|b| b.tick).unwrap_or(0)
}

/// Broadcast a health-ping from a subsystem so monitoring can track liveness.
pub fn health_ping(source_id: u16, health_score_q8: u8) {
    emit(NeuralTopic::HealthPing, source_id, health_score_q8 as u64);
}

/// Record a subsystem firing event in the Hebbian learning layer.
pub fn hebbian_fire(source: u16, timestamp: u64) {
    HEBBIAN.lock().record_fire(source, timestamp);
}

/// Print the top Hebbian connections to the serial console.
pub fn hebbian_report() {
    HEBBIAN.lock().report();
}
