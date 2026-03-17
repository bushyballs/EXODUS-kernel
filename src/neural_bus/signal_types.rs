use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// All signal type definitions for the neural bus
///
/// Part of the AIOS neural bus layer. Defines extended signal categories
/// beyond the core `SignalKind` enum in `mod.rs`. These cover peripheral
/// and contextual events that the cortex can use for richer predictions.
///
/// Each `TypedSignal` wraps an `ExtSignalType` with metadata (source,
/// timestamp, schema version) and can be converted to/from the core
/// `NeuralSignal` format for bus transport.
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Extended signal categories beyond core SignalKind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtSignalType {
    /// File was opened, read, written, or deleted
    FileAccess,
    /// Clipboard contents changed (copy/paste)
    ClipboardChange,
    /// A user-facing notification was posted
    NotificationPosted,
    /// Bluetooth device connected/disconnected/data
    BluetoothEvent,
    /// USB device plugged/unplugged/data
    UsbEvent,
    /// Power state changed (charging, discharging, sleep, wake)
    PowerStateChange,
    /// Media playback started, paused, stopped, or track changed
    MediaPlayback,
    /// Location update (GPS, Wi-Fi triangulation, etc.)
    LocationUpdate,
    /// Sensor reading (accelerometer, gyroscope, ambient light, etc.)
    SensorReading,
    /// Camera activated or frame captured
    CameraEvent,
    /// Microphone activated or audio level change
    MicrophoneEvent,
    /// Network interface state change (Wi-Fi, Ethernet, cellular)
    NetworkInterface,
    /// Application crash or ANR
    AppCrash,
    /// Biometric event (fingerprint, face unlock)
    BiometricEvent,
    /// Display state change (on, off, brightness)
    DisplayChange,
    /// Accessibility event (screen reader, magnifier)
    AccessibilityEvent,
}

/// Map ExtSignalType to a numeric ID for serialisation
impl ExtSignalType {
    pub fn to_id(self) -> u16 {
        match self {
            ExtSignalType::FileAccess => 100,
            ExtSignalType::ClipboardChange => 101,
            ExtSignalType::NotificationPosted => 102,
            ExtSignalType::BluetoothEvent => 103,
            ExtSignalType::UsbEvent => 104,
            ExtSignalType::PowerStateChange => 105,
            ExtSignalType::MediaPlayback => 106,
            ExtSignalType::LocationUpdate => 107,
            ExtSignalType::SensorReading => 108,
            ExtSignalType::CameraEvent => 109,
            ExtSignalType::MicrophoneEvent => 110,
            ExtSignalType::NetworkInterface => 111,
            ExtSignalType::AppCrash => 112,
            ExtSignalType::BiometricEvent => 113,
            ExtSignalType::DisplayChange => 114,
            ExtSignalType::AccessibilityEvent => 115,
        }
    }

    pub fn from_id(id: u16) -> Option<Self> {
        match id {
            100 => Some(ExtSignalType::FileAccess),
            101 => Some(ExtSignalType::ClipboardChange),
            102 => Some(ExtSignalType::NotificationPosted),
            103 => Some(ExtSignalType::BluetoothEvent),
            104 => Some(ExtSignalType::UsbEvent),
            105 => Some(ExtSignalType::PowerStateChange),
            106 => Some(ExtSignalType::MediaPlayback),
            107 => Some(ExtSignalType::LocationUpdate),
            108 => Some(ExtSignalType::SensorReading),
            109 => Some(ExtSignalType::CameraEvent),
            110 => Some(ExtSignalType::MicrophoneEvent),
            111 => Some(ExtSignalType::NetworkInterface),
            112 => Some(ExtSignalType::AppCrash),
            113 => Some(ExtSignalType::BiometricEvent),
            114 => Some(ExtSignalType::DisplayChange),
            115 => Some(ExtSignalType::AccessibilityEvent),
            _ => None,
        }
    }

    /// Human-readable name for logging.
    pub fn name(self) -> &'static str {
        match self {
            ExtSignalType::FileAccess => "file-access",
            ExtSignalType::ClipboardChange => "clipboard-change",
            ExtSignalType::NotificationPosted => "notification-posted",
            ExtSignalType::BluetoothEvent => "bluetooth-event",
            ExtSignalType::UsbEvent => "usb-event",
            ExtSignalType::PowerStateChange => "power-state-change",
            ExtSignalType::MediaPlayback => "media-playback",
            ExtSignalType::LocationUpdate => "location-update",
            ExtSignalType::SensorReading => "sensor-reading",
            ExtSignalType::CameraEvent => "camera-event",
            ExtSignalType::MicrophoneEvent => "microphone-event",
            ExtSignalType::NetworkInterface => "network-interface",
            ExtSignalType::AppCrash => "app-crash",
            ExtSignalType::BiometricEvent => "biometric-event",
            ExtSignalType::DisplayChange => "display-change",
            ExtSignalType::AccessibilityEvent => "accessibility-event",
        }
    }

    /// Priority weight: how important this signal type is for cortex learning.
    /// Returns a value 1-10.
    pub fn priority_weight(self) -> u8 {
        match self {
            ExtSignalType::AppCrash => 10,          // Critical event
            ExtSignalType::PowerStateChange => 8,   // Affects everything
            ExtSignalType::FileAccess => 5,         // Common, moderate value
            ExtSignalType::ClipboardChange => 6,    // User intent signal
            ExtSignalType::NotificationPosted => 4, // Informational
            ExtSignalType::BluetoothEvent => 3,     // Peripheral
            ExtSignalType::UsbEvent => 4,           // Device state
            ExtSignalType::MediaPlayback => 5,      // Context signal
            ExtSignalType::LocationUpdate => 7,     // Strong context
            ExtSignalType::SensorReading => 2,      // Noisy, low value per sample
            ExtSignalType::CameraEvent => 6,        // User engagement
            ExtSignalType::MicrophoneEvent => 6,    // User engagement
            ExtSignalType::NetworkInterface => 5,   // Connectivity context
            ExtSignalType::BiometricEvent => 9,     // Security-sensitive
            ExtSignalType::DisplayChange => 3,      // Low-level state
            ExtSignalType::AccessibilityEvent => 7, // User needs
        }
    }
}

/// Typed signal wrapper with metadata
pub struct TypedSignal {
    /// The extended signal type
    pub signal_type: ExtSignalType,
    /// Source subsystem identifier
    pub source: String,
    /// Timestamp in microseconds (monotonic)
    pub timestamp: u64,
    /// Schema version for forward/backward compatibility
    pub schema_version: u16,
    /// Optional payload: key-value string data
    pub metadata: Vec<(String, String)>,
    /// Numeric payload for sensor data etc.
    pub numeric_payload: Vec<i64>,
    /// Sequence number for ordering within a source
    pub sequence: u64,
}

impl TypedSignal {
    /// Create a new typed signal with the current timestamp.
    pub fn new(signal_type: ExtSignalType, source: &str) -> Self {
        TypedSignal {
            signal_type,
            source: String::from(source),
            timestamp: crate::time::clock::unix_time(),
            schema_version: 1,
            metadata: Vec::new(),
            numeric_payload: Vec::new(),
            sequence: 0,
        }
    }

    /// Create with explicit timestamp (for testing/replay).
    pub fn with_timestamp(signal_type: ExtSignalType, source: &str, timestamp: u64) -> Self {
        TypedSignal {
            signal_type,
            source: String::from(source),
            timestamp,
            schema_version: 1,
            metadata: Vec::new(),
            numeric_payload: Vec::new(),
            sequence: 0,
        }
    }

    /// Add a key-value metadata pair.
    pub fn with_metadata(mut self, key: &str, value: &str) -> Self {
        self.metadata.push((String::from(key), String::from(value)));
        self
    }

    /// Add numeric payload values.
    pub fn with_numeric(mut self, values: Vec<i64>) -> Self {
        self.numeric_payload = values;
        self
    }

    /// Set the sequence number.
    pub fn with_sequence(mut self, seq: u64) -> Self {
        self.sequence = seq;
        self
    }

    /// Get a metadata value by key.
    pub fn get_metadata(&self, key: &str) -> Option<&str> {
        for (k, v) in &self.metadata {
            if k == key {
                return Some(v.as_str());
            }
        }
        None
    }

    /// Serialise the signal to bytes for bus transport.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Header: type_id (2) + schema_version (2) + timestamp (8) + sequence (8) = 20 bytes
        buf.extend_from_slice(&self.signal_type.to_id().to_le_bytes());
        buf.extend_from_slice(&self.schema_version.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.sequence.to_le_bytes());

        // Source string length (2) + source bytes
        let src_bytes = self.source.as_bytes();
        let src_len = (src_bytes.len() as u16).min(255);
        buf.extend_from_slice(&src_len.to_le_bytes());
        buf.extend_from_slice(&src_bytes[..src_len as usize]);

        // Metadata count (2) + key-value pairs
        let meta_count = (self.metadata.len() as u16).min(64);
        buf.extend_from_slice(&meta_count.to_le_bytes());
        for (k, v) in self.metadata.iter().take(meta_count as usize) {
            let klen = (k.len() as u16).min(255);
            let vlen = (v.len() as u16).min(255);
            buf.extend_from_slice(&klen.to_le_bytes());
            buf.extend_from_slice(&k.as_bytes()[..klen as usize]);
            buf.extend_from_slice(&vlen.to_le_bytes());
            buf.extend_from_slice(&v.as_bytes()[..vlen as usize]);
        }

        // Numeric payload count (2) + values
        let num_count = (self.numeric_payload.len() as u16).min(256);
        buf.extend_from_slice(&num_count.to_le_bytes());
        for &val in self.numeric_payload.iter().take(num_count as usize) {
            buf.extend_from_slice(&val.to_le_bytes());
        }

        buf
    }

    /// Get the priority weight for this signal.
    pub fn priority(&self) -> u8 {
        self.signal_type.priority_weight()
    }
}

/// Registry of known extended signal types with their handlers
struct TypeRegistry {
    /// Which signal types are enabled for collection
    enabled: Vec<ExtSignalType>,
    /// Total signals processed per type
    counts: Vec<(ExtSignalType, u64)>,
}

impl TypeRegistry {
    fn new() -> Self {
        let all_types = vec![
            ExtSignalType::FileAccess,
            ExtSignalType::ClipboardChange,
            ExtSignalType::NotificationPosted,
            ExtSignalType::BluetoothEvent,
            ExtSignalType::UsbEvent,
            ExtSignalType::PowerStateChange,
            ExtSignalType::MediaPlayback,
            ExtSignalType::LocationUpdate,
            ExtSignalType::SensorReading,
            ExtSignalType::CameraEvent,
            ExtSignalType::MicrophoneEvent,
            ExtSignalType::NetworkInterface,
            ExtSignalType::AppCrash,
            ExtSignalType::BiometricEvent,
            ExtSignalType::DisplayChange,
            ExtSignalType::AccessibilityEvent,
        ];
        let counts = all_types.iter().map(|&t| (t, 0u64)).collect();
        TypeRegistry {
            enabled: all_types,
            counts,
        }
    }

    fn is_enabled(&self, signal_type: ExtSignalType) -> bool {
        self.enabled.contains(&signal_type)
    }

    fn record(&mut self, signal_type: ExtSignalType) {
        for (t, count) in self.counts.iter_mut() {
            if *t == signal_type {
                *count = count.saturating_add(1);
                return;
            }
        }
    }

    fn disable(&mut self, signal_type: ExtSignalType) {
        self.enabled.retain(|&t| t != signal_type);
    }

    fn enable(&mut self, signal_type: ExtSignalType) {
        if !self.enabled.contains(&signal_type) {
            self.enabled.push(signal_type);
        }
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct SignalTypesState {
    registry: TypeRegistry,
}

static SIGNAL_TYPES: Mutex<Option<SignalTypesState>> = Mutex::new(None);

pub fn init() {
    let registry = TypeRegistry::new();
    let mut guard = SIGNAL_TYPES.lock();
    *guard = Some(SignalTypesState { registry });
    serial_println!("    [signal-types] Extended signal type registry initialised (16 types)");
}

/// Check if an extended signal type is enabled.
pub fn is_enabled(signal_type: ExtSignalType) -> bool {
    let guard = SIGNAL_TYPES.lock();
    if let Some(state) = guard.as_ref() {
        state.registry.is_enabled(signal_type)
    } else {
        false
    }
}

/// Record an occurrence of an extended signal type.
pub fn record_type(signal_type: ExtSignalType) {
    let mut guard = SIGNAL_TYPES.lock();
    if let Some(state) = guard.as_mut() {
        state.registry.record(signal_type);
    }
}
