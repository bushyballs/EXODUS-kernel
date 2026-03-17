/// AI-powered networking for Genesis
///
/// Smart traffic analysis, intrusion detection, adaptive QoS,
/// predictive DNS, bandwidth optimization, anomaly detection.
///
/// Inspired by: Cisco AI Network Analytics, Google Traffic Director. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Network traffic classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficClass {
    Web,
    Streaming,
    Gaming,
    VoIP,
    FileTransfer,
    Email,
    Dns,
    Vpn,
    Ssh,
    Unknown,
    Malicious,
}

/// QoS priority assigned by AI
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QosPriority {
    RealTime,
    Interactive,
    Streaming,
    BulkTransfer,
    Background,
    Throttled,
}

/// Network flow record
pub struct FlowRecord {
    pub src_ip: u32,
    pub dst_ip: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    pub packets: u64,
    pub start_time: u64,
    pub last_seen: u64,
    pub classification: TrafficClass,
    pub qos: QosPriority,
    pub anomaly_score: f32,
}

/// Intrusion detection alert
pub struct IntrusionAlert {
    pub alert_type: IntrusionType,
    pub source_ip: u32,
    pub target_port: u16,
    pub severity: f32,
    pub description: String,
    pub timestamp: u64,
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntrusionType {
    PortScan,
    DDoS,
    BruteForce,
    DataExfiltration,
    DnsAmplification,
    SynFlood,
    MalformedPacket,
    SuspiciousPayload,
}

/// DNS prediction entry
pub struct DnsPrediction {
    pub hostname: String,
    pub probability: f32,
    pub last_resolved: u64,
}

/// AI network engine
pub struct AiNetworkEngine {
    pub enabled: bool,
    pub flows: Vec<FlowRecord>,
    pub alerts: Vec<IntrusionAlert>,
    pub dns_predictions: Vec<DnsPrediction>,
    pub port_scan_tracker: BTreeMap<u32, Vec<u16>>,
    pub bandwidth_history: Vec<(u64, u64)>, // (timestamp, bytes/sec)
    pub total_classified: u64,
    pub total_blocked: u64,
    pub max_flows: usize,
    pub ids_enabled: bool,
    pub smart_qos_enabled: bool,
    pub dns_prefetch_enabled: bool,
}

impl AiNetworkEngine {
    const fn new() -> Self {
        AiNetworkEngine {
            enabled: true,
            flows: Vec::new(),
            alerts: Vec::new(),
            dns_predictions: Vec::new(),
            port_scan_tracker: BTreeMap::new(),
            bandwidth_history: Vec::new(),
            total_classified: 0,
            total_blocked: 0,
            max_flows: 5000,
            ids_enabled: true,
            smart_qos_enabled: true,
            dns_prefetch_enabled: true,
        }
    }

    /// Classify traffic and assign QoS
    pub fn classify_flow(
        &mut self,
        _src_port: u16,
        dst_port: u16,
        _protocol: u8,
    ) -> (TrafficClass, QosPriority) {
        self.total_classified = self.total_classified.saturating_add(1);
        let class = match dst_port {
            80 | 443 | 8080 | 8443 => TrafficClass::Web,
            53 => TrafficClass::Dns,
            22 | 2222 => TrafficClass::Ssh,
            25 | 465 | 587 | 993 | 995 => TrafficClass::Email,
            20 | 21 | 69 | 989 | 990 => TrafficClass::FileTransfer,
            5060 | 5061 | 3478 | 3479 => TrafficClass::VoIP,
            1194 | 51820 => TrafficClass::Vpn,
            _ if dst_port >= 27000 && dst_port <= 27100 => TrafficClass::Gaming,
            _ if dst_port >= 8000 && dst_port <= 9000 => TrafficClass::Streaming,
            _ => TrafficClass::Unknown,
        };
        let qos = match class {
            TrafficClass::VoIP => QosPriority::RealTime,
            TrafficClass::Gaming => QosPriority::Interactive,
            TrafficClass::Streaming => QosPriority::Streaming,
            TrafficClass::Web | TrafficClass::Dns => QosPriority::Interactive,
            TrafficClass::FileTransfer | TrafficClass::Email => QosPriority::BulkTransfer,
            TrafficClass::Ssh | TrafficClass::Vpn => QosPriority::Interactive,
            TrafficClass::Unknown => QosPriority::Background,
            TrafficClass::Malicious => QosPriority::Throttled,
        };
        (class, qos)
    }

    /// Check for intrusion patterns
    pub fn check_intrusion(
        &mut self,
        src_ip: u32,
        dst_port: u16,
        payload_size: u32,
    ) -> Option<IntrusionType> {
        if !self.ids_enabled {
            return None;
        }
        let now = crate::time::clock::unix_time();

        // Port scan detection
        let ports = self
            .port_scan_tracker
            .entry(src_ip)
            .or_insert_with(Vec::new);
        if !ports.contains(&dst_port) {
            ports.push(dst_port);
        }
        if ports.len() > 20 {
            self.total_blocked = self.total_blocked.saturating_add(1);
            self.alerts.push(IntrusionAlert {
                alert_type: IntrusionType::PortScan,
                source_ip: src_ip,
                target_port: dst_port,
                severity: 0.8,
                description: alloc::format!("Port scan: {} ports from {}", ports.len(), src_ip),
                timestamp: now,
                blocked: true,
            });
            return Some(IntrusionType::PortScan);
        }

        // Oversized packet (potential buffer overflow)
        if payload_size > 65000 {
            self.alerts.push(IntrusionAlert {
                alert_type: IntrusionType::MalformedPacket,
                source_ip: src_ip,
                target_port: dst_port,
                severity: 0.7,
                description: alloc::format!("Oversized packet: {} bytes", payload_size),
                timestamp: now,
                blocked: true,
            });
            return Some(IntrusionType::MalformedPacket);
        }

        None
    }

    /// Predict DNS queries user will make
    pub fn predict_dns(&mut self, recent_domains: &[&str]) -> Vec<String> {
        let mut predictions = Vec::new();
        // If user visits news sites, predict related domains
        for domain in recent_domains {
            let domain_lower = domain.to_lowercase();
            if domain_lower.contains("google") {
                predictions.push(String::from("fonts.googleapis.com"));
                predictions.push(String::from("apis.google.com"));
            }
            if domain_lower.contains("github") {
                predictions.push(String::from("raw.githubusercontent.com"));
                predictions.push(String::from("api.github.com"));
            }
        }
        predictions.truncate(5);
        predictions
    }

    /// Get bandwidth optimization suggestion
    pub fn optimize_bandwidth(&self) -> String {
        let total_bytes: u64 = self.flows.iter().map(|f| f.bytes_sent + f.bytes_recv).sum();
        let streaming_bytes: u64 = self
            .flows
            .iter()
            .filter(|f| f.classification == TrafficClass::Streaming)
            .map(|f| f.bytes_sent + f.bytes_recv)
            .sum();
        // Use integer percentage (0..=100) to avoid float arithmetic (no soft-float in kernel)
        let ratio_pct: u64 = if total_bytes > 0 {
            (streaming_bytes * 100) / total_bytes
        } else {
            0
        };

        if ratio_pct > 70 {
            String::from("reduce_streaming_quality")
        } else if total_bytes > 100_000_000 {
            String::from("enable_compression")
        } else {
            String::from("optimal")
        }
    }

    pub fn flow_count(&self) -> usize {
        self.flows.len()
    }
    pub fn alert_count(&self) -> usize {
        self.alerts.len()
    }
}

static AI_NET: Mutex<AiNetworkEngine> = Mutex::new(AiNetworkEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-net] AI network intelligence initialized (IDS, QoS, DNS prefetch)"
    );
}

pub fn classify_flow(src_port: u16, dst_port: u16, protocol: u8) -> (TrafficClass, QosPriority) {
    AI_NET.lock().classify_flow(src_port, dst_port, protocol)
}

pub fn check_intrusion(src_ip: u32, dst_port: u16, payload_size: u32) -> Option<IntrusionType> {
    AI_NET
        .lock()
        .check_intrusion(src_ip, dst_port, payload_size)
}

pub fn optimize_bandwidth() -> String {
    AI_NET.lock().optimize_bandwidth()
}
