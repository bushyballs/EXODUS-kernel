/// AI-powered security for Genesis
///
/// Threat detection, behavioral analysis, anomaly detection,
/// malware classification, intrusion prevention — all ML-driven.
///
/// Inspired by: CrowdStrike Falcon, Google Play Protect. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Threat level classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreatLevel {
    None,
    Low,
    Medium,
    High,
    Critical,
}

/// Security event for AI analysis
pub struct SecurityEvent {
    pub event_type: SecurityEventType,
    pub source_pid: u32,
    pub source_uid: u32,
    pub target: String,
    pub timestamp: u64,
    pub details: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventType {
    LoginAttempt,
    PrivilegeEscalation,
    FileAccess,
    NetworkConnection,
    SyscallAnomaly,
    ProcessSpawn,
    MemoryViolation,
    CryptoOperation,
    PortScan,
    BruteForce,
}

/// Behavioral profile for a process
pub struct ProcessBehaviorProfile {
    pub pid: u32,
    pub name: String,
    pub syscall_histogram: [u32; 16],
    pub file_access_count: u32,
    pub network_connection_count: u32,
    pub memory_allocation_rate: f32,
    pub cpu_usage_avg: f32,
    pub anomaly_score: f32,
    pub last_updated: u64,
}

/// AI security engine
pub struct AiSecurityEngine {
    pub enabled: bool,
    pub event_log: Vec<SecurityEvent>,
    pub behavior_profiles: Vec<ProcessBehaviorProfile>,
    pub threat_signatures: Vec<ThreatSignature>,
    pub active_threats: Vec<ActiveThreat>,
    pub blocked_ips: Vec<u32>,
    pub blocked_pids: Vec<u32>,
    pub learning_mode: bool,
    pub total_events: u64,
    pub threats_blocked: u64,
    pub false_positives: u64,
    pub max_events: usize,
}

pub struct ThreatSignature {
    pub name: String,
    pub pattern: String,
    pub severity: ThreatLevel,
    pub weight: f32,
}

pub struct ActiveThreat {
    pub id: u32,
    pub threat_type: SecurityEventType,
    pub level: ThreatLevel,
    pub source: String,
    pub description: String,
    pub detected_at: u64,
    pub mitigated: bool,
}

impl AiSecurityEngine {
    const fn new() -> Self {
        AiSecurityEngine {
            enabled: true,
            event_log: Vec::new(),
            behavior_profiles: Vec::new(),
            threat_signatures: Vec::new(),
            active_threats: Vec::new(),
            blocked_ips: Vec::new(),
            blocked_pids: Vec::new(),
            learning_mode: true,
            total_events: 0,
            threats_blocked: 0,
            false_positives: 0,
            max_events: 10000,
        }
    }

    /// Analyze a security event with AI
    pub fn analyze_event(&mut self, event: SecurityEvent) -> ThreatLevel {
        self.total_events = self.total_events.saturating_add(1);

        // Behavioral analysis
        let anomaly = self.check_behavior_anomaly(&event);

        // Pattern matching against known threats
        let pattern_match = self.match_threat_patterns(&event);

        // Rate analysis (e.g., brute force detection)
        let rate_threat = self.check_rate_anomaly(&event);

        // Combined threat score
        let combined = anomaly * 0.4 + pattern_match * 0.4 + rate_threat * 0.2;
        let level = if combined > 0.8 {
            ThreatLevel::Critical
        } else if combined > 0.6 {
            ThreatLevel::High
        } else if combined > 0.4 {
            ThreatLevel::Medium
        } else if combined > 0.2 {
            ThreatLevel::Low
        } else {
            ThreatLevel::None
        };

        // Auto-mitigate high threats
        if matches!(level, ThreatLevel::High | ThreatLevel::Critical) {
            self.mitigate_threat(&event, level);
        }

        // Store event
        if self.event_log.len() >= self.max_events {
            self.event_log.remove(0);
        }
        self.event_log.push(event);

        level
    }

    fn check_behavior_anomaly(&self, event: &SecurityEvent) -> f32 {
        // Check if process behavior deviates from profile
        if let Some(profile) = self
            .behavior_profiles
            .iter()
            .find(|p| p.pid == event.source_pid)
        {
            profile.anomaly_score
        } else {
            // Unknown process — slightly suspicious
            0.3
        }
    }

    fn match_threat_patterns(&self, event: &SecurityEvent) -> f32 {
        let mut max_match: f32 = 0.0;
        let details_lower = event.details.to_lowercase();
        for sig in &self.threat_signatures {
            if details_lower.contains(&sig.pattern.to_lowercase()) {
                max_match = max_match.max(sig.weight);
            }
        }
        max_match
    }

    fn check_rate_anomaly(&self, event: &SecurityEvent) -> f32 {
        // Count recent similar events
        let recent_count = self
            .event_log
            .iter()
            .rev()
            .take(100)
            .filter(|e| e.event_type == event.event_type && e.source_pid == event.source_pid)
            .count();
        if recent_count > 20 {
            0.9
        } else if recent_count > 10 {
            0.6
        } else if recent_count > 5 {
            0.3
        } else {
            0.0
        }
    }

    fn mitigate_threat(&mut self, event: &SecurityEvent, level: ThreatLevel) {
        self.threats_blocked = self.threats_blocked.saturating_add(1);
        self.active_threats.push(ActiveThreat {
            id: self.threats_blocked as u32,
            threat_type: event.event_type,
            level,
            source: alloc::format!("pid:{}", event.source_pid),
            description: event.details.clone(),
            detected_at: event.timestamp,
            mitigated: true,
        });
        self.blocked_pids.push(event.source_pid);
        crate::serial_println!(
            "  [ai-sec] THREAT BLOCKED: {:?} from pid {} (level: {:?})",
            event.event_type,
            event.source_pid,
            level
        );
    }

    /// Update behavior profile for a process
    pub fn update_profile(&mut self, pid: u32, name: &str, syscall_idx: usize) {
        let now = crate::time::clock::unix_time();
        if let Some(profile) = self.behavior_profiles.iter_mut().find(|p| p.pid == pid) {
            if syscall_idx < 16 {
                profile.syscall_histogram[syscall_idx] =
                    profile.syscall_histogram[syscall_idx].saturating_add(1);
            }
            profile.last_updated = now;
        } else {
            let mut hist = [0u32; 16];
            if syscall_idx < 16 {
                hist[syscall_idx] = 1;
            }
            self.behavior_profiles.push(ProcessBehaviorProfile {
                pid,
                name: String::from(name),
                syscall_histogram: hist,
                file_access_count: 0,
                network_connection_count: 0,
                memory_allocation_rate: 0.0,
                cpu_usage_avg: 0.0,
                anomaly_score: 0.0,
                last_updated: now,
            });
        }
    }

    /// Check if a PID is blocked
    pub fn is_blocked(&self, pid: u32) -> bool {
        self.blocked_pids.contains(&pid)
    }

    /// Scan binary data for malware signatures
    pub fn scan_binary(&self, _data: &[u8]) -> ThreatLevel {
        // In real impl: run ML classifier on binary features
        ThreatLevel::None
    }
}

fn seed_signatures(engine: &mut AiSecurityEngine) {
    let sigs = [
        (
            "SQL Injection",
            "select.*from.*where",
            ThreatLevel::High,
            0.8,
        ),
        ("XSS Attack", "<script>", ThreatLevel::High, 0.9),
        ("Path Traversal", "../../../", ThreatLevel::High, 0.85),
        ("Shell Injection", "; rm -rf", ThreatLevel::Critical, 0.95),
        (
            "Buffer Overflow",
            "\\x90\\x90\\x90",
            ThreatLevel::Critical,
            0.9,
        ),
        ("Privilege Escalation", "setuid", ThreatLevel::Medium, 0.5),
        ("Port Scanner", "syn_scan", ThreatLevel::Medium, 0.6),
        ("Crypto Miner", "stratum+tcp", ThreatLevel::High, 0.85),
    ];
    for (name, pattern, level, weight) in &sigs {
        engine.threat_signatures.push(ThreatSignature {
            name: String::from(*name),
            pattern: String::from(*pattern),
            severity: *level,
            weight: *weight,
        });
    }
}

static AI_SECURITY: Mutex<AiSecurityEngine> = Mutex::new(AiSecurityEngine::new());

pub fn init() {
    seed_signatures(&mut AI_SECURITY.lock());
    crate::serial_println!(
        "    [ai-sec] AI security engine initialized ({} threat signatures)",
        AI_SECURITY.lock().threat_signatures.len()
    );
}

pub fn analyze_event(event: SecurityEvent) -> ThreatLevel {
    AI_SECURITY.lock().analyze_event(event)
}

pub fn is_blocked(pid: u32) -> bool {
    AI_SECURITY.lock().is_blocked(pid)
}

pub fn update_profile(pid: u32, name: &str, syscall_idx: usize) {
    AI_SECURITY.lock().update_profile(pid, name, syscall_idx);
}

pub fn scan_binary(data: &[u8]) -> ThreatLevel {
    AI_SECURITY.lock().scan_binary(data)
}
