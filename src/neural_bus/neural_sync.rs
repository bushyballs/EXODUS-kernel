/// Cross-device neural sync — synchronize learned patterns across HoagsOS devices
/// Manages device registry, sync protocols, conflict resolution, and privacy controls.

use super::*;
use crate::{serial_print, serial_println};

use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Synchronization protocol strategy
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncProtocol {
    Manual,    // Sync on explicit user request
    Periodic,  // Sync at regular intervals
    Realtime,  // Sync immediately on pattern change
    OnDemand,  // Sync when high-confidence data available
}

/// Device types in HoagsOS network
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceType {
    Phone,
    Tablet,
    Laptop,
    Desktop,
    Watch,
    TV,
    Car,
    IoT,
}

/// Device information and trust metadata
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub device_id: u64,
    pub name: String,
    pub device_type: DeviceType,
    pub last_sync: u64,        // milliseconds since epoch
    pub trust_level: Q16,      // 0.0 to 1.0 (Q16)
    pub pattern_count: u32,    // patterns stored on device
    pub sync_count: u32,       // successful syncs
}

impl DeviceInfo {
    pub fn new(device_id: u64, name: String, device_type: DeviceType) -> Self {
        DeviceInfo {
            device_id,
            name,
            device_type,
            last_sync: 0,
            trust_level: Q16::from_f64(1.0), // Start at full trust
            pattern_count: 0,
            sync_count: 0,
        }
    }
}

/// Sync payload variants for different data types
#[derive(Clone, Debug)]
pub enum SyncPayload {
    PatternSync(Vec<PatternData>),
    PreferenceSync(BTreeMap<String, String>),
    ContextSync(ContextData),
    ShortcutSync(Vec<ShortcutDef>),
    WeightSync(Vec<WeightUpdate>),
}

/// Pattern data with metadata
#[derive(Clone, Debug)]
pub struct PatternData {
    pub pattern_id: u32,
    pub signal_kind: SignalKind,
    pub confidence: Q16,
    pub last_seen: u64,
    pub frequency: u32,
}

/// User context snapshot
#[derive(Clone, Debug)]
pub struct ContextData {
    pub user_intent: u8,       // encoded intent
    pub emotion: Q16,          // emotional state -1.0 to 1.0
    pub active_app: u16,       // app identifier
    pub timestamp: u64,
}

/// Shortcut definition
#[derive(Clone, Debug)]
pub struct ShortcutDef {
    pub shortcut_id: u16,
    pub input_signal: u8,
    pub output_action: u8,
    pub enabled: bool,
}

/// Weight update for neural model
#[derive(Clone, Debug)]
pub struct WeightUpdate {
    pub layer_id: u8,
    pub neuron_id: u16,
    pub delta: Q16,
    pub confidence: Q16,
}

/// Packet to synchronize across network
#[derive(Clone, Debug)]
pub struct SyncPacket {
    pub source_device: u64,
    pub target_device: u64,
    pub timestamp: u64,
    pub sequence_num: u32,
    pub payload: SyncPayload,
    pub checksum: u32,
}

impl SyncPacket {
    pub fn compute_checksum(&self) -> u32 {
        let mut hash = 5381u32;
        hash = hash.wrapping_mul(33).wrapping_add(self.source_device as u32);
        hash = hash.wrapping_mul(33).wrapping_add(self.target_device as u32);
        hash = hash.wrapping_mul(33).wrapping_add((self.timestamp >> 32) as u32);
        hash = hash.wrapping_mul(33).wrapping_add((self.timestamp & 0xFFFFFFFF) as u32);
        hash
    }

    pub fn verify_checksum(&self) -> bool {
        self.checksum == self.compute_checksum()
    }
}

/// Conflict resolution strategy
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictStrategy {
    NewestWins,      // Most recent timestamp wins
    MostConfident,   // Highest confidence wins
    Merge,           // Blend both patterns (weighted average)
    AskUser,         // Request user decision
}

/// Neural sync engine for cross-device coordination
pub struct NeuralSyncEngine {
    known_devices: BTreeMap<u64, DeviceInfo>,
    sync_protocol: SyncProtocol,
    outbox: Vec<SyncPacket>,
    inbox: Vec<SyncPacket>,
    sync_interval_ms: u64,
    last_sync: u64,
    total_synced: u64,
    sequence_counter: u32,
    conflict_resolution: ConflictStrategy,
    bandwidth_limit: u32,  // bytes per sync
    bytes_synced: u32,
}

impl NeuralSyncEngine {
    pub const fn new() -> Self {
        NeuralSyncEngine {
            known_devices: BTreeMap::new(),
            sync_protocol: SyncProtocol::Periodic,
            outbox: Vec::new(),
            inbox: Vec::new(),
            sync_interval_ms: 30000,  // 30 second default
            last_sync: 0,
            total_synced: 0,
            sequence_counter: 0,
            conflict_resolution: ConflictStrategy::MostConfident,
            bandwidth_limit: 8192,
            bytes_synced: 0,
        }
    }

    /// Initialize the sync engine with local device info
    pub fn init(&mut self, device_id: u64, device_name: &str, device_type: DeviceType) {
        let local_device = DeviceInfo::new(
            device_id,
            String::from(device_name),
            device_type,
        );
        self.known_devices.insert(device_id, local_device);
    }

    /// Register a new device in the network
    pub fn register_device(&mut self, device_id: u64, name: &str, device_type: DeviceType) -> bool {
        if self.known_devices.len() >= 16 {
            return false;  // Limit to 16 devices
        }
        let device = DeviceInfo::new(device_id, String::from(name), device_type);
        self.known_devices.insert(device_id, device);
        true
    }

    /// Queue a sync packet for transmission
    pub fn queue_packet(&mut self, packet: SyncPacket) -> bool {
        if self.outbox.len() >= 32 {
            return false;  // Queue full
        }
        self.outbox.push(packet);
        true
    }

    /// Receive and queue an incoming sync packet
    pub fn receive_packet(&mut self, packet: SyncPacket) -> bool {
        if !packet.verify_checksum() {
            return false;  // Checksum failed
        }
        if self.inbox.len() >= 32 {
            return false;  // Queue full
        }
        self.inbox.push(packet);
        true
    }

    /// Process all pending inbox packets with conflict resolution
    pub fn process_inbox(&mut self) -> u32 {
        let mut processed = 0u32;
        while let Some(packet) = self.inbox.pop() {
            if self.process_single_packet(&packet) {
                processed += 1;
                self.total_synced = self.total_synced.saturating_add(1);

                // Update source device metadata
                if let Some(device) = self.known_devices.get_mut(&packet.source_device) {
                    device.sync_count = device.sync_count.saturating_add(1);
                    device.last_sync = packet.timestamp;
                }
            }
        }
        processed
    }

    /// Process a single sync packet
    fn process_single_packet(&mut self, packet: &SyncPacket) -> bool {
        // Verify source device is known
        if !self.known_devices.contains_key(&packet.source_device) {
            return false;
        }

        // Check bandwidth constraints
        let estimated_size = 64 + (packet.checksum as u32);
        if self.bytes_synced + estimated_size > self.bandwidth_limit {
            return false;
        }

        // Privacy: strip sensitive identifiers (in real implementation)
        // Apply conflict resolution based on strategy
        match self.conflict_resolution {
            ConflictStrategy::NewestWins => {
                // Implementation: compare timestamps with existing patterns
            }
            ConflictStrategy::MostConfident => {
                // Implementation: compare confidence levels in payload
            }
            ConflictStrategy::Merge => {
                // Implementation: weighted blend of patterns
            }
            ConflictStrategy::AskUser => {
                // Implementation: flag for user review
            }
        }

        self.bytes_synced += estimated_size;
        true
    }

    /// Trigger immediate sync cycle
    pub fn sync_now(&mut self, current_time_ms: u64) -> u32 {
        let mut sent_count = 0u32;
        while let Some(packet) = self.outbox.pop() {
            // In real implementation, transmit packet over network
            sent_count += 1;
        }
        self.last_sync = current_time_ms;
        self.bytes_synced = 0;
        sent_count
    }

    /// Check if sync should trigger based on protocol
    pub fn should_sync(&self, current_time_ms: u64) -> bool {
        match self.sync_protocol {
            SyncProtocol::Manual => false,
            SyncProtocol::Periodic => {
                current_time_ms.wrapping_sub(self.last_sync) >= self.sync_interval_ms
            }
            SyncProtocol::Realtime => true,
            SyncProtocol::OnDemand => false,  // Checked elsewhere
        }
    }

    /// Set the sync protocol
    pub fn set_protocol(&mut self, protocol: SyncProtocol) {
        self.sync_protocol = protocol;
    }

    /// Set conflict resolution strategy
    pub fn set_conflict_strategy(&mut self, strategy: ConflictStrategy) {
        self.conflict_resolution = strategy;
    }

    /// Update sync interval (milliseconds)
    pub fn set_sync_interval(&mut self, interval_ms: u64) {
        self.sync_interval_ms = interval_ms;
    }

    /// Update device trust level (0.0 to 1.0)
    pub fn update_trust_level(&mut self, device_id: u64, trust: Q16) -> bool {
        if let Some(device) = self.known_devices.get_mut(&device_id) {
            device.trust_level = trust;
            true
        } else {
            false
        }
    }

    /// Get engine statistics
    pub fn stats(&self) -> EngineStats {
        EngineStats {
            known_devices: self.known_devices.len() as u32,
            outbox_count: self.outbox.len() as u32,
            inbox_count: self.inbox.len() as u32,
            total_synced: self.total_synced,
            last_sync_ms: self.last_sync,
            bytes_synced_this_cycle: self.bytes_synced,
            protocol: self.sync_protocol,
        }
    }

    /// Remove a device from the network
    pub fn unregister_device(&mut self, device_id: u64) -> bool {
        self.known_devices.remove(&device_id).is_some()
    }

    /// Get device info by ID
    pub fn get_device(&self, device_id: u64) -> Option<DeviceInfo> {
        self.known_devices.get(&device_id).cloned()
    }

    /// List all known devices
    pub fn list_devices(&self) -> Vec<u64> {
        self.known_devices.keys().copied().collect()
    }
}

/// Engine statistics snapshot
#[derive(Clone, Debug)]
pub struct EngineStats {
    pub known_devices: u32,
    pub outbox_count: u32,
    pub inbox_count: u32,
    pub total_synced: u64,
    pub last_sync_ms: u64,
    pub bytes_synced_this_cycle: u32,
    pub protocol: SyncProtocol,
}

// Global sync engine instance
pub static NEURAL_SYNC: Mutex<NeuralSyncEngine> = Mutex::new(NeuralSyncEngine::new());

/// Initialize neural sync engine
pub fn init_neural_sync(device_id: u64, device_name: &str, device_type: DeviceType) {
    let mut engine = NEURAL_SYNC.lock();
    engine.init(device_id, device_name, device_type);
}

/// Register a device for cross-device sync
pub fn register_sync_device(device_id: u64, name: &str, device_type: DeviceType) -> bool {
    let mut engine = NEURAL_SYNC.lock();
    engine.register_device(device_id, name, device_type)
}

/// Queue a sync packet for outbound transmission
pub fn queue_sync_packet(packet: SyncPacket) -> bool {
    let mut engine = NEURAL_SYNC.lock();
    engine.queue_packet(packet)
}

/// Receive an incoming sync packet
pub fn receive_sync_packet(packet: SyncPacket) -> bool {
    let mut engine = NEURAL_SYNC.lock();
    engine.receive_packet(packet)
}

/// Process all pending inbox packets
pub fn process_sync_inbox() -> u32 {
    let mut engine = NEURAL_SYNC.lock();
    engine.process_inbox()
}

/// Trigger immediate sync cycle (returns packets sent)
pub fn sync_now(current_time_ms: u64) -> u32 {
    let mut engine = NEURAL_SYNC.lock();
    engine.sync_now(current_time_ms)
}

/// Check if sync should run based on current protocol
pub fn should_sync_now(current_time_ms: u64) -> bool {
    let engine = NEURAL_SYNC.lock();
    engine.should_sync(current_time_ms)
}

/// Get current sync engine statistics
pub fn get_sync_stats() -> EngineStats {
    let engine = NEURAL_SYNC.lock();
    engine.stats()
}

/// Update a device's trust level
pub fn update_device_trust(device_id: u64, trust: Q16) -> bool {
    let mut engine = NEURAL_SYNC.lock();
    engine.update_trust_level(device_id, trust)
}

/// Remove a device from sync network
pub fn unregister_sync_device(device_id: u64) -> bool {
    let mut engine = NEURAL_SYNC.lock();
    engine.unregister_device(device_id)
}

/// List all registered devices
pub fn list_sync_devices() -> Vec<u64> {
    let engine = NEURAL_SYNC.lock();
    engine.list_devices()
}

/// Get info for a specific device
pub fn get_sync_device_info(device_id: u64) -> Option<DeviceInfo> {
    let engine = NEURAL_SYNC.lock();
    engine.get_device(device_id)
}
