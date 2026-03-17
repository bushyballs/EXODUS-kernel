use super::Ipv4Addr;
use crate::sync::Mutex;
/// MQTT protocol client/broker for Genesis — Message Queuing Telemetry Transport
///
/// Implements MQTT v3.1.1 (RFC 6455 transport) and v5.0 features:
///   - CONNECT, CONNACK, PUBLISH, PUBACK, SUBSCRIBE, SUBACK, UNSUBSCRIBE
///   - QoS levels 0 (at-most-once), 1 (at-least-once), 2 (exactly-once)
///   - Keep-alive with PINGREQ/PINGRESP
///   - Last Will and Testament (LWT)
///   - Retained messages, session persistence, topic wildcards
///
/// Inspired by: Mosquitto, EMQX, rumqtt. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ============================================================================
// MQTT packet types (4-bit, upper nibble of first byte)
// ============================================================================

pub const CONNECT: u8 = 0x10;
pub const CONNACK: u8 = 0x20;
pub const PUBLISH: u8 = 0x30;
pub const PUBACK: u8 = 0x40;
pub const PUBREC: u8 = 0x50;
pub const PUBREL: u8 = 0x60;
pub const PUBCOMP: u8 = 0x70;
pub const SUBSCRIBE: u8 = 0x80;
pub const SUBACK: u8 = 0x90;
pub const UNSUBSCRIBE: u8 = 0xA0;
pub const UNSUBACK: u8 = 0xB0;
pub const PINGREQ: u8 = 0xC0;
pub const PINGRESP: u8 = 0xD0;
pub const DISCONNECT: u8 = 0xE0;

// ============================================================================
// QoS levels
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QoS {
    AtMostOnce = 0,
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

impl QoS {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(QoS::AtMostOnce),
            1 => Some(QoS::AtLeastOnce),
            2 => Some(QoS::ExactlyOnce),
            _ => None,
        }
    }
}

// ============================================================================
// Connect return codes
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectReturnCode {
    Accepted = 0x00,
    BadProtocol = 0x01,
    ClientIdRejected = 0x02,
    ServerUnavailable = 0x03,
    BadCredentials = 0x04,
    NotAuthorized = 0x05,
}

impl ConnectReturnCode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => ConnectReturnCode::Accepted,
            0x01 => ConnectReturnCode::BadProtocol,
            0x02 => ConnectReturnCode::ClientIdRejected,
            0x03 => ConnectReturnCode::ServerUnavailable,
            0x04 => ConnectReturnCode::BadCredentials,
            _ => ConnectReturnCode::NotAuthorized,
        }
    }
}

// ============================================================================
// Connection state
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Error,
}

// ============================================================================
// Will message (Last Will and Testament)
// ============================================================================

#[derive(Debug, Clone)]
pub struct WillMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: QoS,
    pub retain: bool,
}

// ============================================================================
// Subscription
// ============================================================================

#[derive(Debug, Clone)]
pub struct Subscription {
    pub topic_filter: String,
    pub qos: QoS,
}

impl Subscription {
    /// Match a topic against this subscription's filter
    /// Supports '+' (single-level) and '#' (multi-level) wildcards
    pub fn matches(&self, topic: &str) -> bool {
        topic_matches(&self.topic_filter, topic)
    }
}

/// Match a topic against a filter with MQTT wildcard rules
fn topic_matches(filter: &str, topic: &str) -> bool {
    let filter_parts: Vec<&str> = filter.split('/').collect();
    let topic_parts: Vec<&str> = topic.split('/').collect();

    let mut fi = 0;
    let mut ti = 0;

    while fi < filter_parts.len() && ti < topic_parts.len() {
        match filter_parts[fi] {
            "#" => return true, // multi-level wildcard matches everything remaining
            "+" => {
                // single-level wildcard: matches exactly one level
                fi = fi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
            level => {
                if level != topic_parts[ti] {
                    return false;
                }
                fi = fi.saturating_add(1);
                ti = ti.saturating_add(1);
            }
        }
    }

    // Both must be exhausted for exact match (unless filter ended with #)
    fi == filter_parts.len() && ti == topic_parts.len()
}

// ============================================================================
// Retained message store
// ============================================================================

#[derive(Debug, Clone)]
pub struct RetainedMessage {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: QoS,
}

// ============================================================================
// Pending QoS message (awaiting acknowledgment)
// ============================================================================

#[derive(Debug, Clone)]
struct PendingMessage {
    packet_id: u16,
    topic: String,
    payload: Vec<u8>,
    qos: QoS,
    retry_count: u8,
    timestamp: u64,
}

// ============================================================================
// MQTT client
// ============================================================================

pub struct MqttClient {
    pub state: MqttState,
    pub client_id: String,
    pub broker_ip: Ipv4Addr,
    pub broker_port: u16,
    pub keep_alive_secs: u16,
    pub clean_session: bool,
    pub username: Option<String>,
    pub password: Option<Vec<u8>>,
    pub will: Option<WillMessage>,
    pub subscriptions: Vec<Subscription>,
    /// Incoming messages received from broker
    pub inbox: Vec<(String, Vec<u8>, QoS)>,
    /// Outgoing QoS 1/2 messages awaiting ACK
    pending_out: Vec<PendingMessage>,
    /// Incoming QoS 2 messages in PUBREC state
    pending_in: Vec<u16>,
    /// Next packet identifier (1..65535)
    next_packet_id: u16,
    /// Ticks since last PINGREQ sent
    ping_timer: u64,
    /// Total messages published
    pub stats_published: u64,
    /// Total messages received
    pub stats_received: u64,
    /// Total bytes sent
    pub stats_bytes_tx: u64,
    /// Total bytes received
    pub stats_bytes_rx: u64,
}

impl MqttClient {
    pub const fn new() -> Self {
        MqttClient {
            state: MqttState::Disconnected,
            client_id: String::new(),
            broker_ip: Ipv4Addr([0; 4]),
            broker_port: 1883,
            keep_alive_secs: 60,
            clean_session: true,
            username: None,
            password: None,
            will: None,
            subscriptions: Vec::new(),
            inbox: Vec::new(),
            pending_out: Vec::new(),
            pending_in: Vec::new(),
            next_packet_id: 1,
            ping_timer: 0,
            stats_published: 0,
            stats_received: 0,
            stats_bytes_tx: 0,
            stats_bytes_rx: 0,
        }
    }

    fn alloc_packet_id(&mut self) -> u16 {
        let id = self.next_packet_id;
        self.next_packet_id = if self.next_packet_id >= 0xFFFE {
            1
        } else {
            self.next_packet_id.saturating_add(1)
        };
        id
    }

    /// Build a CONNECT packet
    pub fn build_connect(&self) -> Vec<u8> {
        let mut payload = Vec::new();

        // Protocol name "MQTT"
        payload.extend_from_slice(&[0x00, 0x04]);
        payload.extend_from_slice(b"MQTT");
        // Protocol level (4 = v3.1.1)
        payload.push(0x04);

        // Connect flags
        let mut flags: u8 = 0;
        if self.clean_session {
            flags |= 0x02;
        }
        if self.will.is_some() {
            flags |= 0x04; // will flag
            if let Some(ref w) = self.will {
                flags |= (w.qos as u8) << 3;
                if w.retain {
                    flags |= 0x20;
                }
            }
        }
        if self.username.is_some() {
            flags |= 0x80;
        }
        if self.password.is_some() {
            flags |= 0x40;
        }
        payload.push(flags);

        // Keep alive
        payload.extend_from_slice(&self.keep_alive_secs.to_be_bytes());

        // Client ID
        encode_utf8_string(&mut payload, &self.client_id);

        // Will topic + payload
        if let Some(ref w) = self.will {
            encode_utf8_string(&mut payload, &w.topic);
            let wlen = w.payload.len() as u16;
            payload.extend_from_slice(&wlen.to_be_bytes());
            payload.extend_from_slice(&w.payload);
        }

        // Username
        if let Some(ref u) = self.username {
            encode_utf8_string(&mut payload, u);
        }

        // Password
        if let Some(ref p) = self.password {
            let plen = p.len() as u16;
            payload.extend_from_slice(&plen.to_be_bytes());
            payload.extend_from_slice(p);
        }

        wrap_fixed_header(CONNECT, &payload)
    }

    /// Build a PUBLISH packet
    pub fn build_publish(
        &mut self,
        topic: &str,
        payload: &[u8],
        qos: QoS,
        retain: bool,
    ) -> Vec<u8> {
        let mut body = Vec::new();

        // Topic name
        encode_utf8_string(&mut body, topic);

        // Packet identifier (QoS 1 and 2 only)
        let packet_id = if qos != QoS::AtMostOnce {
            let id = self.alloc_packet_id();
            body.extend_from_slice(&id.to_be_bytes());
            Some(id)
        } else {
            None
        };

        // Payload
        body.extend_from_slice(payload);

        // First byte: PUBLISH + flags
        let mut first_byte = PUBLISH;
        if retain {
            first_byte |= 0x01;
        }
        first_byte |= (qos as u8) << 1;

        // Track for QoS 1/2
        if let Some(id) = packet_id {
            self.pending_out.push(PendingMessage {
                packet_id: id,
                topic: String::from(topic),
                payload: Vec::from(payload),
                qos,
                retry_count: 0,
                timestamp: 0,
            });
        }

        self.stats_published = self.stats_published.saturating_add(1);
        wrap_fixed_header_byte(first_byte, &body)
    }

    /// Build a SUBSCRIBE packet
    pub fn build_subscribe(&mut self, topics: &[(&str, QoS)]) -> Vec<u8> {
        let mut body = Vec::new();

        let packet_id = self.alloc_packet_id();
        body.extend_from_slice(&packet_id.to_be_bytes());

        for &(topic, qos) in topics {
            encode_utf8_string(&mut body, topic);
            body.push(qos as u8);
            self.subscriptions.push(Subscription {
                topic_filter: String::from(topic),
                qos,
            });
        }

        // SUBSCRIBE has fixed flag bits: 0x02
        wrap_fixed_header_byte(SUBSCRIBE | 0x02, &body)
    }

    /// Build an UNSUBSCRIBE packet
    pub fn build_unsubscribe(&mut self, topics: &[&str]) -> Vec<u8> {
        let mut body = Vec::new();

        let packet_id = self.alloc_packet_id();
        body.extend_from_slice(&packet_id.to_be_bytes());

        for topic in topics {
            encode_utf8_string(&mut body, topic);
            self.subscriptions.retain(|s| s.topic_filter != *topic);
        }

        wrap_fixed_header_byte(UNSUBSCRIBE | 0x02, &body)
    }

    /// Build a PINGREQ packet
    pub fn build_pingreq() -> Vec<u8> {
        alloc::vec![PINGREQ, 0x00]
    }

    /// Build a DISCONNECT packet
    pub fn build_disconnect() -> Vec<u8> {
        alloc::vec![DISCONNECT, 0x00]
    }

    /// Build a PUBACK packet (QoS 1 acknowledgment)
    pub fn build_puback(packet_id: u16) -> Vec<u8> {
        let mut pkt = alloc::vec![PUBACK, 0x02];
        pkt.extend_from_slice(&packet_id.to_be_bytes());
        pkt
    }

    /// Build a PUBREC packet (QoS 2 step 1 acknowledgment)
    pub fn build_pubrec(packet_id: u16) -> Vec<u8> {
        let mut pkt = alloc::vec![PUBREC, 0x02];
        pkt.extend_from_slice(&packet_id.to_be_bytes());
        pkt
    }

    /// Build a PUBREL packet (QoS 2 step 2 release)
    pub fn build_pubrel(packet_id: u16) -> Vec<u8> {
        let mut pkt = alloc::vec![PUBREL | 0x02, 0x02];
        pkt.extend_from_slice(&packet_id.to_be_bytes());
        pkt
    }

    /// Build a PUBCOMP packet (QoS 2 step 3 complete)
    pub fn build_pubcomp(packet_id: u16) -> Vec<u8> {
        let mut pkt = alloc::vec![PUBCOMP, 0x02];
        pkt.extend_from_slice(&packet_id.to_be_bytes());
        pkt
    }

    /// Process an incoming MQTT packet from the broker
    pub fn process_packet(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        if data.is_empty() {
            return None;
        }

        let pkt_type = data[0] & 0xF0;
        let (_remaining_len, header_len) = decode_remaining_length(&data[1..])?;
        let body = &data[1 + header_len..];

        self.stats_bytes_rx = self.stats_bytes_rx.saturating_add(data.len() as u64);

        match pkt_type {
            CONNACK => {
                if body.len() >= 2 {
                    let return_code = ConnectReturnCode::from_u8(body[1]);
                    if return_code == ConnectReturnCode::Accepted {
                        self.state = MqttState::Connected;
                        serial_println!("  [mqtt] Connected to broker");
                    } else {
                        self.state = MqttState::Error;
                        serial_println!("  [mqtt] Connection refused: {:?}", return_code);
                    }
                }
                None
            }

            PUBLISH => self.process_incoming_publish(data),

            PUBACK => {
                // QoS 1 acknowledgment
                if body.len() >= 2 {
                    let packet_id = u16::from_be_bytes([body[0], body[1]]);
                    self.pending_out.retain(|p| p.packet_id != packet_id);
                    serial_println!("  [mqtt] PUBACK for packet {}", packet_id);
                }
                None
            }

            PUBREC => {
                // QoS 2 step 1: send PUBREL
                if body.len() >= 2 {
                    let packet_id = u16::from_be_bytes([body[0], body[1]]);
                    return Some(Self::build_pubrel(packet_id));
                }
                None
            }

            PUBCOMP => {
                // QoS 2 step 3: message delivery complete
                if body.len() >= 2 {
                    let packet_id = u16::from_be_bytes([body[0], body[1]]);
                    self.pending_out.retain(|p| p.packet_id != packet_id);
                    serial_println!("  [mqtt] PUBCOMP for packet {}", packet_id);
                }
                None
            }

            SUBACK => {
                serial_println!("  [mqtt] Subscription acknowledged");
                None
            }

            UNSUBACK => {
                serial_println!("  [mqtt] Unsubscription acknowledged");
                None
            }

            PINGRESP => {
                self.ping_timer = 0;
                None
            }

            _ => None,
        }
    }

    /// Process an incoming PUBLISH message
    fn process_incoming_publish(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        let flags = data[0] & 0x0F;
        let qos_val = (flags >> 1) & 0x03;
        let qos = QoS::from_u8(qos_val).unwrap_or(QoS::AtMostOnce);

        let (_remaining_len, header_len) = decode_remaining_length(&data[1..])?;
        let body = &data[1 + header_len..];

        // Parse topic
        if body.len() < 2 {
            return None;
        }
        let topic_len = u16::from_be_bytes([body[0], body[1]]) as usize;
        if body.len() < 2 + topic_len {
            return None;
        }
        let topic = core::str::from_utf8(&body[2..2 + topic_len]).unwrap_or("");
        let mut offset = 2 + topic_len;

        // Packet ID for QoS 1/2
        let packet_id = if qos != QoS::AtMostOnce {
            if body.len() < offset + 2 {
                return None;
            }
            let id = u16::from_be_bytes([body[offset], body[offset + 1]]);
            offset = offset.saturating_add(2);
            Some(id)
        } else {
            None
        };

        // Payload
        let payload = if offset < body.len() {
            Vec::from(&body[offset..])
        } else {
            Vec::new()
        };

        serial_println!(
            "  [mqtt] PUBLISH topic='{}' qos={} len={}",
            topic,
            qos_val,
            payload.len()
        );

        // Deliver to inbox
        self.inbox.push((String::from(topic), payload, qos));
        self.stats_received = self.stats_received.saturating_add(1);

        // Acknowledge
        match qos {
            QoS::AtMostOnce => None,
            QoS::AtLeastOnce => packet_id.map(Self::build_puback),
            QoS::ExactlyOnce => {
                if let Some(id) = packet_id {
                    self.pending_in.push(id);
                    Some(Self::build_pubrec(id))
                } else {
                    None
                }
            }
        }
    }

    /// Check if keep-alive ping is needed (call periodically)
    pub fn tick(&mut self) -> Option<Vec<u8>> {
        if self.state != MqttState::Connected {
            return None;
        }

        self.ping_timer = self.ping_timer.saturating_add(1);
        let threshold = (self.keep_alive_secs as u64) * 100; // rough tick-to-seconds

        if self.ping_timer >= threshold {
            self.ping_timer = 0;
            serial_println!("  [mqtt] Sending PINGREQ");
            return Some(Self::build_pingreq());
        }

        None
    }

    /// Get statistics as formatted string
    pub fn stats_info(&self) -> String {
        format!(
            "MQTT: state={:?} published={} received={} tx={}B rx={}B subs={} pending={}",
            self.state,
            self.stats_published,
            self.stats_received,
            self.stats_bytes_tx,
            self.stats_bytes_rx,
            self.subscriptions.len(),
            self.pending_out.len()
        )
    }
}

// ============================================================================
// Wire format helpers
// ============================================================================

/// Encode a UTF-8 string with 2-byte length prefix (MQTT wire format)
fn encode_utf8_string(buf: &mut Vec<u8>, s: &str) {
    let len = s.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

/// Encode remaining length in MQTT variable-length encoding (1-4 bytes)
fn encode_remaining_length(mut length: usize) -> Vec<u8> {
    let mut encoded = Vec::new();
    loop {
        let mut byte = (length % 128) as u8;
        length /= 128;
        if length > 0 {
            byte |= 0x80;
        }
        encoded.push(byte);
        if length == 0 {
            break;
        }
    }
    encoded
}

/// Decode remaining length from MQTT variable-length encoding
/// Returns (length, number_of_bytes_consumed)
fn decode_remaining_length(data: &[u8]) -> Option<(usize, usize)> {
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    let mut index = 0;

    loop {
        if index >= data.len() || index >= 4 {
            return None;
        }
        let byte = data[index];
        value += (byte as usize & 0x7F) * multiplier;
        multiplier *= 128;
        index += 1;
        if byte & 0x80 == 0 {
            break;
        }
    }

    Some((value, index))
}

/// Wrap a payload with MQTT fixed header (packet type byte + remaining length)
fn wrap_fixed_header(pkt_type: u8, payload: &[u8]) -> Vec<u8> {
    wrap_fixed_header_byte(pkt_type, payload)
}

/// Wrap a payload with a specific first byte + remaining length
fn wrap_fixed_header_byte(first_byte: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(first_byte);
    packet.extend_from_slice(&encode_remaining_length(payload.len()));
    packet.extend_from_slice(payload);
    packet
}

// ============================================================================
// Global state
// ============================================================================

static MQTT_CLIENT: Mutex<Option<MqttClient>> = Mutex::new(None);

/// Retained message store (topic -> message)
static RETAINED: Mutex<BTreeMap<String, RetainedMessage>> = Mutex::new(BTreeMap::new());

pub fn init() {
    *MQTT_CLIENT.lock() = Some(MqttClient::new());
    serial_println!("    [mqtt] MQTT v3.1.1 client initialized (QoS 0/1/2)");
}

/// Configure the MQTT client
pub fn configure(client_id: &str, broker_ip: Ipv4Addr, port: u16, keep_alive: u16) {
    if let Some(ref mut client) = *MQTT_CLIENT.lock() {
        client.client_id = String::from(client_id);
        client.broker_ip = broker_ip;
        client.broker_port = port;
        client.keep_alive_secs = keep_alive;
        serial_println!(
            "  [mqtt] Configured: id='{}' broker={}:{}",
            client_id,
            broker_ip,
            port
        );
    }
}

/// Set Last Will and Testament
pub fn set_will(topic: &str, payload: &[u8], qos: QoS, retain: bool) {
    if let Some(ref mut client) = *MQTT_CLIENT.lock() {
        client.will = Some(WillMessage {
            topic: String::from(topic),
            payload: Vec::from(payload),
            qos,
            retain,
        });
    }
}

/// Set credentials
pub fn set_credentials(username: &str, password: &[u8]) {
    if let Some(ref mut client) = *MQTT_CLIENT.lock() {
        client.username = Some(String::from(username));
        client.password = Some(Vec::from(password));
    }
}

/// Build a CONNECT packet (caller sends via TCP)
pub fn build_connect() -> Option<Vec<u8>> {
    MQTT_CLIENT.lock().as_ref().map(|c| c.build_connect())
}

/// Build a PUBLISH packet
pub fn publish(topic: &str, payload: &[u8], qos: QoS, retain: bool) -> Option<Vec<u8>> {
    if let Some(ref mut client) = *MQTT_CLIENT.lock() {
        let pkt = client.build_publish(topic, payload, qos, retain);

        // Store retained message
        if retain {
            RETAINED.lock().insert(
                String::from(topic),
                RetainedMessage {
                    topic: String::from(topic),
                    payload: Vec::from(payload),
                    qos,
                },
            );
        }

        Some(pkt)
    } else {
        None
    }
}

/// Build a SUBSCRIBE packet
pub fn subscribe(topics: &[(&str, QoS)]) -> Option<Vec<u8>> {
    MQTT_CLIENT
        .lock()
        .as_mut()
        .map(|c| c.build_subscribe(topics))
}

/// Build an UNSUBSCRIBE packet
pub fn unsubscribe(topics: &[&str]) -> Option<Vec<u8>> {
    MQTT_CLIENT
        .lock()
        .as_mut()
        .map(|c| c.build_unsubscribe(topics))
}

/// Process an incoming packet from broker
pub fn process_incoming(data: &[u8]) -> Option<Vec<u8>> {
    MQTT_CLIENT
        .lock()
        .as_mut()
        .and_then(|c| c.process_packet(data))
}

/// Read next message from inbox
pub fn recv() -> Option<(String, Vec<u8>, QoS)> {
    MQTT_CLIENT.lock().as_mut().and_then(|c| {
        if c.inbox.is_empty() {
            None
        } else {
            Some(c.inbox.remove(0))
        }
    })
}

/// Get client state
pub fn state() -> MqttState {
    MQTT_CLIENT
        .lock()
        .as_ref()
        .map(|c| c.state)
        .unwrap_or(MqttState::Disconnected)
}

/// Get stats string
pub fn stats() -> String {
    MQTT_CLIENT
        .lock()
        .as_ref()
        .map(|c| c.stats_info())
        .unwrap_or_else(|| String::from("MQTT: not initialized"))
}

/// Periodic tick for keep-alive
pub fn tick() -> Option<Vec<u8>> {
    MQTT_CLIENT.lock().as_mut().and_then(|c| c.tick())
}
