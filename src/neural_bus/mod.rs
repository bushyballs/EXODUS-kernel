pub mod adaptive_ui;
/// Hoags Neural Bus — the nervous system of Genesis
///
/// Every subsystem in the OS is a neural node. Signals flow through
/// the bus like neurons firing — observations in, insights out.
/// The AI sees everything, learns everything, shortcuts everything.
///
/// Architecture:
///   - NeuralNode: any subsystem that emits/receives signals
///   - NeuralBus: the backbone connecting all nodes
///   - Cortex: the central brain that processes all signals
///   - StreamPipeline: zero-latency ring buffers between nodes
///   - AdaptiveUI: malleable graphics that morph per-user
///   - ShortcutEngine: AI pre-executes predicted operations
///
/// Signal flow:
///   Subsystem → NeuralNode.emit(signal) → Bus → Cortex → learn + predict
///   Cortex → ShortcutEngine → pre-execute / pre-render / pre-fetch
///   Cortex → AdaptiveUI → morph layout / colors / behavior
///   Cortex → NeuralNode.receive(insight) → Subsystem adapts
///
/// All Q16 fixed-point math (i32, 16 fractional bits, 65536 = 1.0).
/// All on-device. No data leaves. Zero external dependencies.
pub mod cortex;
pub mod federation;
pub mod history;
pub mod metrics;
pub mod priority;
pub mod replay;
pub mod router;
pub mod shortcuts;
pub mod signal_types;
pub mod streaming;
pub mod subsystem_registry;

use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Q16 Fixed-Point ─────────────────────────────────────────────────

pub type Q16 = i32;
pub const Q16_ONE: Q16 = 65536; // 1.0
pub const Q16_HALF: Q16 = 32768; // 0.5
pub const Q16_ZERO: Q16 = 0;
pub const Q16_TENTH: Q16 = 6554; // 0.1

#[inline]
pub fn q16_mul(a: Q16, b: Q16) -> Q16 {
    ((a as i64 * b as i64) >> 16) as Q16
}

#[inline]
pub fn q16_div(a: Q16, b: Q16) -> Q16 {
    if b == 0 {
        return 0;
    }
    ((a as i64) << 16 / b as i64) as Q16
}

#[inline]
pub fn q16_from_int(x: i32) -> Q16 {
    x << 16
}

#[inline]
pub fn q16_to_int(x: Q16) -> i32 {
    x >> 16
}

// ── Signal Types ────────────────────────────────────────────────────

/// What kind of neural signal is flowing through the bus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    // User behavior signals
    AppLaunch,       // User launched an app
    AppSwitch,       // User switched apps
    AppClose,        // User closed an app
    TouchEvent,      // Touch/click at coordinates
    GestureDetected, // Swipe, pinch, long-press
    TextInput,       // User typing
    VoiceCommand,    // Voice input detected
    SearchQuery,     // User searched for something

    // System observation signals
    CpuLoad,        // CPU utilization changed
    MemoryPressure, // Memory getting tight
    DiskIo,         // Disk read/write spike
    NetworkTraffic, // Network activity
    BatteryDrain,   // Power consumption change
    ThermalEvent,   // Temperature change
    ProcessSpawn,   // New process created
    ProcessExit,    // Process terminated

    // AI insight signals (cortex → nodes)
    PredictedAction, // AI thinks user will do X next
    PreloadHint,     // AI says pre-load this resource
    LayoutMorph,     // AI says rearrange the UI
    ColorAdapt,      // AI says adjust colors/theme
    ShortcutReady,   // AI has pre-computed a result
    AnomalyAlert,    // AI detected something wrong
    LearningUpdate,  // AI model weights updated
    ContextShift,    // User context changed (work→home, etc.)

    // Inter-node coordination
    NodeSync,     // Synchronize state between nodes
    NodeQuery,    // One node asking another for data
    NodeResponse, // Response to a query
    Heartbeat,    // Keep-alive ping
}

/// A neural signal flowing through the bus
#[derive(Clone)]
pub struct NeuralSignal {
    pub kind: SignalKind,
    pub source_node: u16,
    pub target_node: u16, // 0 = broadcast to all
    pub timestamp: u64,
    pub strength: Q16, // Signal strength (confidence) 0..Q16_ONE
    pub payload: SignalPayload,
}

/// Signal data payload — keeps it efficient with fixed-size variants
#[derive(Clone)]
pub enum SignalPayload {
    Empty,
    Integer(i64),
    Pair(i32, i32),        // x,y or key,value
    Triple(i32, i32, i32), // x,y,z or r,g,b
    Text(String),
    Vector(Vec<Q16>), // Embedding or feature vector
    Bytes(Vec<u8>),   // Raw data
}

impl NeuralSignal {
    pub fn new(kind: SignalKind, source: u16, strength: Q16) -> Self {
        NeuralSignal {
            kind,
            source_node: source,
            target_node: 0,
            timestamp: crate::time::clock::unix_time(),
            strength,
            payload: SignalPayload::Empty,
        }
    }

    pub fn with_target(mut self, target: u16) -> Self {
        self.target_node = target;
        self
    }

    pub fn with_int(mut self, val: i64) -> Self {
        self.payload = SignalPayload::Integer(val);
        self
    }

    pub fn with_pair(mut self, a: i32, b: i32) -> Self {
        self.payload = SignalPayload::Pair(a, b);
        self
    }

    pub fn with_triple(mut self, a: i32, b: i32, c: i32) -> Self {
        self.payload = SignalPayload::Triple(a, b, c);
        self
    }

    pub fn with_text(mut self, t: String) -> Self {
        self.payload = SignalPayload::Text(t);
        self
    }

    pub fn with_vector(mut self, v: Vec<Q16>) -> Self {
        self.payload = SignalPayload::Vector(v);
        self
    }
}

// ── Neural Node ─────────────────────────────────────────────────────

/// Node capability flags (what this node can do)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCapability {
    Emit,    // Can send signals
    Receive, // Can receive signals
    Learn,   // Has local learning (weights)
    Predict, // Can make predictions
    Render,  // Can render UI elements
    Execute, // Can execute operations
    Store,   // Can store persistent data
}

/// Subsystem category for routing
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubsystemClass {
    Kernel,        // memory, process, scheduler, ipc
    Hardware,      // drivers, usb, gpu, audio
    Storage,       // fs, storage, database
    Network,       // net, connectivity, p2p
    Security,      // security, crypto, privacy
    Display,       // display, theming, multiwindow
    Input,         // input, touch, voice, camera
    AI,            // ai, llm, ml, learning, agent
    Application,   // app, sysapps, browser
    User,          // preferences, accessibility, wellbeing
    Communication, // messaging, email, telephony, contacts
    System,        // services, updates, recovery, config
}

/// A node on the neural bus — every subsystem gets one
pub struct NeuralNode {
    pub id: u16,
    pub name: String,
    pub class: SubsystemClass,
    pub capabilities: u16,       // Bitmask of NodeCapability
    pub priority: Q16,           // How important this node's signals are
    pub local_weights: Vec<Q16>, // Per-node learned weights (optional)
    pub signal_count: u64,       // Total signals emitted
    pub last_active: u64,        // Last signal timestamp
    pub connected: bool,         // Is this node live on the bus
    pub subscriptions: u32,      // Bitmask of SignalKind subscriptions
}

impl NeuralNode {
    pub fn new(id: u16, name: &str, class: SubsystemClass) -> Self {
        NeuralNode {
            id,
            name: String::from(name),
            class,
            capabilities: 0x03, // Emit + Receive by default
            priority: Q16_ONE,
            local_weights: Vec::new(),
            signal_count: 0,
            last_active: 0,
            connected: false,
            subscriptions: 0xFFFFFFFF, // Subscribe to all by default
        }
    }

    pub fn with_capability(mut self, cap: NodeCapability) -> Self {
        self.capabilities |= 1 << (cap as u16);
        self
    }

    pub fn with_priority(mut self, p: Q16) -> Self {
        self.priority = p;
        self
    }

    pub fn with_local_weights(mut self, size: usize) -> Self {
        self.local_weights = alloc::vec![Q16_ZERO; size];
        self
    }

    pub fn is_capable(&self, cap: NodeCapability) -> bool {
        self.capabilities & (1 << (cap as u16)) != 0
    }
}

// ── Neural Bus (the backbone) ───────────────────────────────────────

/// Ring buffer for signal transport — lock-free single-producer style
const SIGNAL_RING_SIZE: usize = 1024;

pub struct SignalRing {
    pub signals: Vec<NeuralSignal>,
    pub write_pos: usize,
    pub read_pos: usize,
    pub capacity: usize,
    pub overflow_count: u64,
}

impl SignalRing {
    pub fn new() -> Self {
        SignalRing {
            signals: Vec::new(),
            write_pos: 0,
            read_pos: 0,
            capacity: SIGNAL_RING_SIZE,
            overflow_count: 0,
        }
    }

    pub fn push(&mut self, signal: NeuralSignal) {
        if self.signals.len() < self.capacity {
            self.signals.push(signal);
        } else {
            self.signals[self.write_pos % self.capacity] = signal;
        }
        self.write_pos += 1;
        if self.write_pos - self.read_pos > self.capacity {
            self.read_pos = self.write_pos - self.capacity;
            self.overflow_count = self.overflow_count.saturating_add(1);
        }
    }

    pub fn pop(&mut self) -> Option<NeuralSignal> {
        if self.read_pos >= self.write_pos {
            return None;
        }
        let idx = self.read_pos % self.capacity;
        self.read_pos += 1;
        if idx < self.signals.len() {
            Some(self.signals[idx].clone())
        } else {
            None
        }
    }

    pub fn pending(&self) -> usize {
        self.write_pos.saturating_sub(self.read_pos)
    }

    pub fn is_empty(&self) -> bool {
        self.pending() == 0
    }
}

/// The Neural Bus itself — connects all nodes
pub struct NeuralBus {
    pub nodes: BTreeMap<u16, NeuralNode>,
    pub signal_ring: SignalRing,
    pub next_node_id: u16,
    pub total_signals: u64,
    pub total_shortcuts: u64,
    pub bus_active: bool,
    pub cortex_enabled: bool,

    // Per-class signal aggregation (for the cortex to read)
    pub class_activity: [u64; 12],    // One counter per SubsystemClass
    pub class_last_signal: [u64; 12], // Timestamp of last signal per class

    // Cross-node connection weights (learned by cortex)
    // node_weights[i][j] = how strongly node i influences node j
    pub node_weights: Vec<Vec<Q16>>,
}

const MAX_NODES: usize = 128;

impl NeuralBus {
    pub const fn new() -> Self {
        NeuralBus {
            nodes: BTreeMap::new(),
            signal_ring: SignalRing {
                signals: Vec::new(),
                write_pos: 0,
                read_pos: 0,
                capacity: SIGNAL_RING_SIZE,
                overflow_count: 0,
            },
            next_node_id: 1,
            total_signals: 0,
            total_shortcuts: 0,
            bus_active: false,
            cortex_enabled: false,
            class_activity: [0; 12],
            class_last_signal: [0; 12],
            node_weights: Vec::new(),
        }
    }

    /// Register a new node on the bus. Returns the node ID.
    pub fn register_node(&mut self, mut node: NeuralNode) -> u16 {
        let id = self.next_node_id;
        node.id = id;
        node.connected = true;
        serial_println!(
            "    [neural-bus] Node #{}: '{}' ({:?}) connected",
            id,
            node.name,
            node.class
        );
        self.nodes.insert(id, node);
        self.next_node_id = self.next_node_id.saturating_add(1);

        // Grow weight matrix
        let n = self.nodes.len();
        self.node_weights.resize(n, Vec::new());
        for row in self.node_weights.iter_mut() {
            row.resize(n, Q16_ZERO);
        }

        id
    }

    /// Emit a signal from a node into the bus
    pub fn emit(&mut self, signal: NeuralSignal) {
        let class_idx = self.node_class_index(signal.source_node);
        if class_idx < 12 {
            self.class_activity[class_idx] = self.class_activity[class_idx].saturating_add(1);
            self.class_last_signal[class_idx] = signal.timestamp;
        }

        // Update source node stats
        if let Some(node) = self.nodes.get_mut(&signal.source_node) {
            node.signal_count = node.signal_count.saturating_add(1);
            node.last_active = signal.timestamp;
        }

        self.signal_ring.push(signal);
        self.total_signals = self.total_signals.saturating_add(1);
    }

    /// Drain pending signals for cortex processing
    pub fn drain_signals(&mut self, max: usize) -> Vec<NeuralSignal> {
        let mut batch = Vec::new();
        for _ in 0..max {
            match self.signal_ring.pop() {
                Some(s) => batch.push(s),
                None => break,
            }
        }
        batch
    }

    /// Get a node's class index for activity tracking
    fn node_class_index(&self, node_id: u16) -> usize {
        self.nodes
            .get(&node_id)
            .map(|n| n.class as usize)
            .unwrap_or(11) // System as fallback
    }

    /// How many nodes are connected
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get the most active subsystem class right now
    pub fn most_active_class(&self) -> SubsystemClass {
        let max_idx = self
            .class_activity
            .iter()
            .enumerate()
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i)
            .unwrap_or(0);
        match max_idx {
            0 => SubsystemClass::Kernel,
            1 => SubsystemClass::Hardware,
            2 => SubsystemClass::Storage,
            3 => SubsystemClass::Network,
            4 => SubsystemClass::Security,
            5 => SubsystemClass::Display,
            6 => SubsystemClass::Input,
            7 => SubsystemClass::AI,
            8 => SubsystemClass::Application,
            9 => SubsystemClass::User,
            10 => SubsystemClass::Communication,
            _ => SubsystemClass::System,
        }
    }

    /// Update cross-node weights based on co-activation
    /// (Hebbian learning: nodes that fire together wire together)
    pub fn hebbian_update(&mut self, node_a: u16, node_b: u16, strength: Q16) {
        let n = self.node_weights.len();
        let a = (node_a as usize).saturating_sub(1); // IDs are 1-based
        let b = (node_b as usize).saturating_sub(1);
        if a < n && b < n {
            // w += learning_rate * strength
            let lr = Q16_TENTH / 10; // 0.01 learning rate
            let delta = q16_mul(lr, strength);
            self.node_weights[a][b] = (self.node_weights[a][b] + delta).min(Q16_ONE);
            self.node_weights[b][a] = (self.node_weights[b][a] + delta).min(Q16_ONE);
        }
    }

    /// Get the strongest connections for a given node
    pub fn strongest_connections(&self, node_id: u16, top_n: usize) -> Vec<(u16, Q16)> {
        let idx = (node_id as usize).saturating_sub(1);
        if idx >= self.node_weights.len() {
            return Vec::new();
        }

        let mut connections: Vec<(u16, Q16)> = self.node_weights[idx]
            .iter()
            .enumerate()
            .filter(|(_, &w)| w > Q16_ZERO)
            .map(|(j, &w)| ((j + 1) as u16, w))
            .collect();

        connections.sort_by(|a, b| b.1.cmp(&a.1));
        connections.truncate(top_n);
        connections
    }
}

// ── Global Bus Instance ─────────────────────────────────────────────

pub static BUS: Mutex<NeuralBus> = Mutex::new(NeuralBus::new());

// ── Public API (any subsystem can call these) ───────────────────────

/// Register a neural node for a subsystem
pub fn register_node(node: NeuralNode) -> u16 {
    BUS.lock().register_node(node)
}

/// Emit a signal into the neural bus
pub fn emit(signal: NeuralSignal) {
    BUS.lock().emit(signal);
}

/// Quick emit: fire a signal with integer payload
pub fn fire(kind: SignalKind, source: u16, value: i64) {
    let sig = NeuralSignal::new(kind, source, Q16_ONE).with_int(value);
    BUS.lock().emit(sig);
}

/// Quick emit: fire a signal with coordinates
pub fn fire_xy(kind: SignalKind, source: u16, x: i32, y: i32) {
    let sig = NeuralSignal::new(kind, source, Q16_ONE).with_pair(x, y);
    BUS.lock().emit(sig);
}

/// Quick emit: fire a signal with text
pub fn fire_text(kind: SignalKind, source: u16, text: &str) {
    let sig = NeuralSignal::new(kind, source, Q16_ONE).with_text(String::from(text));
    BUS.lock().emit(sig);
}

/// Get the node count
pub fn node_count() -> usize {
    BUS.lock().node_count()
}

/// Get total signals processed
pub fn total_signals() -> u64 {
    BUS.lock().total_signals
}

// ── Init ────────────────────────────────────────────────────────────

pub fn init() {
    {
        let mut bus = BUS.lock();
        bus.bus_active = true;
        bus.cortex_enabled = true;
    }

    cortex::init();
    streaming::init();
    adaptive_ui::init();
    shortcuts::init();
    signal_types::init();
    router::init();
    priority::init();
    metrics::init();
    replay::init();
    federation::init();
    history::init();
    subsystem_registry::init();

    serial_println!("  Neural Bus: backbone active, cortex online, streaming ready, signal_types, router, priority, metrics, replay, federation, history, subsystem_registry");
    serial_println!("    [neural-bus] Signal ring: {} slots", SIGNAL_RING_SIZE);
    serial_println!("    [neural-bus] Max nodes: {}", MAX_NODES);
    serial_println!("    [neural-bus] Hebbian learning: enabled");
    serial_println!("    [neural-bus] Adaptive UI: enabled");
    serial_println!("    [neural-bus] Shortcut engine: enabled");
}
