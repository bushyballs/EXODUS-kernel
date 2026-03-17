use super::SignalKind;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Signal routing and filtering for the neural bus
///
/// Part of the AIOS neural bus layer. Implements a rule-based signal
/// router that matches incoming signals against subscription rules and
/// returns the set of destination nodes. Rules can filter by signal
/// kind, source node, priority, and strength threshold.
///
/// The router supports:
///   - Exact kind matching or wildcard (None = match all kinds)
///   - Source node filtering or wildcard
///   - Priority-weighted routing (higher priority rules are checked first)
///   - Bloom-filter-based fast rejection for signals that match no rules
///   - Wildcard routes that broadcast to all connected nodes
use alloc::vec::Vec;

/// A routing rule that matches signals to destinations
pub struct RouteRule {
    /// If Some, only match signals of this kind; None = match all kinds
    pub kind_filter: Option<SignalKind>,
    /// If Some, only match signals from this source node; None = any source
    pub source_filter: Option<u16>,
    /// Destination node ID that receives matched signals
    pub dest_node: u16,
    /// Rule priority (higher = checked first)
    pub priority: i32,
    /// Minimum signal strength (Q16) required for this rule to fire
    pub min_strength: i32,
    /// Whether this rule is currently active
    pub active: bool,
    /// Hit counter: how many signals matched this rule
    pub hit_count: u64,
}

impl RouteRule {
    /// Create a simple rule: route signals of `kind` from any source to `dest`.
    pub fn new(kind: SignalKind, dest_node: u16) -> Self {
        RouteRule {
            kind_filter: Some(kind),
            source_filter: None,
            dest_node,
            priority: 0,
            min_strength: 0,
            active: true,
            hit_count: 0,
        }
    }

    /// Create a wildcard rule: route all signals to `dest`.
    pub fn wildcard(dest_node: u16) -> Self {
        RouteRule {
            kind_filter: None,
            source_filter: None,
            dest_node,
            priority: -1, // Lower priority than specific rules
            min_strength: 0,
            active: true,
            hit_count: 0,
        }
    }

    /// Check if this rule matches the given signal parameters.
    pub fn matches(&self, kind: SignalKind, source: u16, strength: i32) -> bool {
        if !self.active {
            return false;
        }
        if strength < self.min_strength {
            return false;
        }
        if let Some(k) = self.kind_filter {
            if k != kind {
                return false;
            }
        }
        if let Some(s) = self.source_filter {
            if s != source {
                return false;
            }
        }
        true
    }
}

/// Simple bloom filter for fast rule rejection.
/// Uses 2 hash functions and a 256-bit bitmap.
struct BloomFilter {
    bits: [u64; 4], // 256 bits
}

impl BloomFilter {
    fn new() -> Self {
        BloomFilter { bits: [0; 4] }
    }

    fn hash1(kind_byte: u8) -> (usize, u64) {
        let idx = (kind_byte as usize) / 64;
        let bit = 1u64 << ((kind_byte as usize) % 64);
        (idx.min(3), bit)
    }

    fn hash2(kind_byte: u8) -> (usize, u64) {
        let rotated = kind_byte.wrapping_mul(137).wrapping_add(43);
        let idx = (rotated as usize) / 64;
        let bit = 1u64 << ((rotated as usize) % 64);
        (idx.min(3), bit)
    }

    fn insert(&mut self, kind_byte: u8) {
        let (i1, b1) = Self::hash1(kind_byte);
        let (i2, b2) = Self::hash2(kind_byte);
        self.bits[i1] |= b1;
        self.bits[i2] |= b2;
    }

    fn maybe_contains(&self, kind_byte: u8) -> bool {
        let (i1, b1) = Self::hash1(kind_byte);
        let (i2, b2) = Self::hash2(kind_byte);
        (self.bits[i1] & b1 != 0) && (self.bits[i2] & b2 != 0)
    }

    fn clear(&mut self) {
        self.bits = [0; 4];
    }
}

/// Convert a SignalKind to a byte for bloom filter hashing
fn kind_to_byte(kind: SignalKind) -> u8 {
    // Discriminant-based mapping
    match kind {
        SignalKind::AppLaunch => 0,
        SignalKind::AppSwitch => 1,
        SignalKind::AppClose => 2,
        SignalKind::TouchEvent => 3,
        SignalKind::GestureDetected => 4,
        SignalKind::TextInput => 5,
        SignalKind::VoiceCommand => 6,
        SignalKind::SearchQuery => 7,
        SignalKind::CpuLoad => 8,
        SignalKind::MemoryPressure => 9,
        SignalKind::DiskIo => 10,
        SignalKind::NetworkTraffic => 11,
        SignalKind::BatteryDrain => 12,
        SignalKind::ThermalEvent => 13,
        SignalKind::ProcessSpawn => 14,
        SignalKind::ProcessExit => 15,
        SignalKind::PredictedAction => 16,
        SignalKind::PreloadHint => 17,
        SignalKind::LayoutMorph => 18,
        SignalKind::ColorAdapt => 19,
        SignalKind::ShortcutReady => 20,
        SignalKind::AnomalyAlert => 21,
        SignalKind::LearningUpdate => 22,
        SignalKind::ContextShift => 23,
        SignalKind::NodeSync => 24,
        SignalKind::NodeQuery => 25,
        SignalKind::NodeResponse => 26,
        SignalKind::Heartbeat => 27,
    }
}

/// Routes signals based on subscription rules
pub struct SignalRouter {
    /// All routing rules
    pub rules: Vec<RouteRule>,
    /// Bloom filter for fast rejection of signal kinds with no rules
    bloom: BloomFilter,
    /// Total signals routed
    pub total_routed: u64,
    /// Total signals that matched zero rules
    pub total_unmatched: u64,
    /// Maximum number of rules
    pub max_rules: usize,
}

impl SignalRouter {
    /// Create a new empty signal router.
    pub fn new() -> Self {
        SignalRouter {
            rules: Vec::new(),
            bloom: BloomFilter::new(),
            total_routed: 0,
            total_unmatched: 0,
            max_rules: 1024,
        }
    }

    /// Add a routing rule.
    pub fn add_rule(&mut self, rule: RouteRule) {
        if self.rules.len() >= self.max_rules {
            serial_println!(
                "    [router] Rule limit reached ({}), dropping oldest low-priority rule",
                self.max_rules
            );
            // Remove lowest-priority rule
            if let Some(min_idx) = self
                .rules
                .iter()
                .enumerate()
                .min_by_key(|(_, r)| r.priority)
                .map(|(i, _)| i)
            {
                self.rules.swap_remove(min_idx);
            }
        }

        // Update bloom filter
        if let Some(kind) = rule.kind_filter {
            self.bloom.insert(kind_to_byte(kind));
        } else {
            // Wildcard: insert all possible kinds
            for k in 0..28u8 {
                self.bloom.insert(k);
            }
        }

        self.rules.push(rule);

        // Keep rules sorted by priority (descending)
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Remove all rules targeting a specific destination node.
    pub fn remove_rules_for_dest(&mut self, dest_node: u16) {
        let before = self.rules.len();
        self.rules.retain(|r| r.dest_node != dest_node);
        let removed = before - self.rules.len();
        if removed > 0 {
            self.rebuild_bloom();
            serial_println!(
                "    [router] Removed {} rules for dest node {}",
                removed,
                dest_node
            );
        }
    }

    /// Route a signal: return the list of destination node IDs.
    ///
    /// `kind` is the signal type, `source` is the source node ID.
    /// Returns a deduplicated, sorted list of destination nodes.
    pub fn route(&self, kind: SignalKind, source: u16) -> Vec<u16> {
        self.route_with_strength(kind, source, super::Q16_ONE)
    }

    /// Route with explicit signal strength for threshold filtering.
    pub fn route_with_strength(&self, kind: SignalKind, source: u16, strength: i32) -> Vec<u16> {
        // Fast rejection via bloom filter
        let kb = kind_to_byte(kind);
        if !self.bloom.maybe_contains(kb) {
            return Vec::new();
        }

        let mut destinations = Vec::new();

        for rule in &self.rules {
            if rule.matches(kind, source, strength) {
                // Avoid duplicates
                if !destinations.contains(&rule.dest_node) {
                    destinations.push(rule.dest_node);
                }
            }
        }

        destinations
    }

    /// Route and return mutable references to update hit counters.
    pub fn route_and_count(&mut self, kind: SignalKind, source: u16, strength: i32) -> Vec<u16> {
        self.total_routed = self.total_routed.saturating_add(1);

        let kb = kind_to_byte(kind);
        if !self.bloom.maybe_contains(kb) {
            self.total_unmatched = self.total_unmatched.saturating_add(1);
            return Vec::new();
        }

        let mut destinations = Vec::new();

        for rule in self.rules.iter_mut() {
            if rule.matches(kind, source, strength) {
                rule.hit_count = rule.hit_count.saturating_add(1);
                if !destinations.contains(&rule.dest_node) {
                    destinations.push(rule.dest_node);
                }
            }
        }

        if destinations.is_empty() {
            self.total_unmatched = self.total_unmatched.saturating_add(1);
        }

        destinations
    }

    /// Rebuild the bloom filter from scratch (after rule removal).
    fn rebuild_bloom(&mut self) {
        self.bloom.clear();
        for rule in &self.rules {
            if let Some(kind) = rule.kind_filter {
                self.bloom.insert(kind_to_byte(kind));
            } else {
                for k in 0..28u8 {
                    self.bloom.insert(k);
                }
            }
        }
    }

    /// Get the number of active rules.
    pub fn active_rule_count(&self) -> usize {
        self.rules.iter().filter(|r| r.active).count()
    }

    /// Disable all rules targeting a specific destination.
    pub fn disable_dest(&mut self, dest_node: u16) {
        for rule in self.rules.iter_mut() {
            if rule.dest_node == dest_node {
                rule.active = false;
            }
        }
    }

    /// Enable all rules targeting a specific destination.
    pub fn enable_dest(&mut self, dest_node: u16) {
        for rule in self.rules.iter_mut() {
            if rule.dest_node == dest_node {
                rule.active = true;
            }
        }
    }

    /// Get the top N rules by hit count.
    pub fn hottest_rules(&self, n: usize) -> Vec<(usize, u64)> {
        let mut indexed: Vec<(usize, u64)> = self
            .rules
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.hit_count))
            .collect();
        indexed.sort_by(|a, b| b.1.cmp(&a.1));
        indexed.truncate(n);
        indexed
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct RouterState {
    router: SignalRouter,
}

static ROUTER: Mutex<Option<RouterState>> = Mutex::new(None);

pub fn init() {
    let router = SignalRouter::new();
    let mut guard = ROUTER.lock();
    *guard = Some(RouterState { router });
    serial_println!("    [router] Signal router subsystem initialised");
}

/// Add a rule to the global router.
pub fn add_rule_global(rule: RouteRule) {
    let mut guard = ROUTER.lock();
    if let Some(state) = guard.as_mut() {
        state.router.add_rule(rule);
    }
}

/// Route a signal through the global router.
pub fn route_global(kind: SignalKind, source: u16) -> Vec<u16> {
    let mut guard = ROUTER.lock();
    if let Some(state) = guard.as_mut() {
        state.router.route_and_count(kind, source, super::Q16_ONE)
    } else {
        Vec::new()
    }
}
