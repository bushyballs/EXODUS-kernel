/// Hoags Intelligence — OS-wide AI integration bus
///
/// The central intelligence layer that connects AI capabilities
/// to every subsystem in Genesis. Every module in the OS can
/// request AI services through this unified interface.
///
/// All processing happens on-device. No data leaves.
///
/// Inspired by: Apple Intelligence (system-wide AI), Google AICore. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// AI service request type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRequest {
    // NLP
    Classify,
    Sentiment,
    Summarize,
    ExtractEntities,
    GenerateText,
    Translate,
    Autocomplete,

    // Vision
    DetectObjects,
    ClassifyScene,
    RecognizeText,
    DetectFaces,
    DescribeImage,

    // Voice
    SpeechToText,
    TextToSpeech,
    IdentifySpeaker,

    // Knowledge
    QueryKnowledge,
    InferRelation,

    // Prediction
    PredictNextApp,
    PredictNextAction,
    PredictResource,
    AnomalyDetect,

    // Security
    ThreatDetect,
    BehaviorAnalyze,
    MalwareClassify,

    // System
    OptimizePower,
    OptimizeMemory,
    OptimizeNetwork,
    SmartSchedule,
}

/// AI priority for request queue
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AiPriority {
    Critical,   // Security threats, real-time voice
    High,       // User-facing suggestions, active queries
    Normal,     // Background analysis, learning
    Low,        // Prefetching, optimization
    Background, // Training, knowledge building
}

/// Result from AI processing
pub struct AiResult {
    pub request_type: AiRequest,
    pub confidence: f32,
    pub label: String,
    pub data: Vec<(String, String)>,
    pub numeric: f32,
}

/// AI context — information about the current system state
/// that helps AI make better decisions
pub struct AiContext {
    pub current_app: String,
    pub time_of_day: u8,
    pub day_of_week: u8,
    pub battery_level: u8,
    pub is_charging: bool,
    pub network_type: NetworkContext,
    pub user_activity: ActivityLevel,
    pub location_context: String,
    pub recent_apps: Vec<String>,
    pub active_notifications: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkContext {
    None,
    WiFi,
    Cellular,
    Metered,
    Roaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityLevel {
    Idle,
    LightUse,
    ActiveUse,
    HeavyUse,
    Gaming,
    Media,
    Sleeping,
}

/// Anomaly detection result
pub struct AnomalyReport {
    pub subsystem: String,
    pub severity: f32,
    pub description: String,
    pub metric_name: String,
    pub expected_value: f32,
    pub actual_value: f32,
    pub timestamp: u64,
}

/// Pattern learned from user behavior
pub struct BehaviorPattern {
    pub pattern_type: String,
    pub hour: u8,
    pub day: u8,
    pub action: String,
    pub frequency: u32,
    pub confidence: f32,
}

/// The OS-wide intelligence engine
pub struct IntelligenceEngine {
    pub enabled: bool,
    pub context: AiContext,
    pub behavior_patterns: Vec<BehaviorPattern>,
    pub anomaly_history: Vec<AnomalyReport>,
    pub request_count: u64,
    pub model_cache: BTreeMap<String, bool>,
    pub subsystem_hooks: Vec<SubsystemHook>,
    pub learning_rate: f32,
    pub privacy_mode: PrivacyMode,
}

/// Privacy mode controls what AI can learn
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivacyMode {
    Full,     // AI can learn from all interactions
    Limited,  // AI learns patterns but not content
    Minimal,  // AI only uses pre-trained models
    Disabled, // No AI processing
}

/// A hook into a subsystem
pub struct SubsystemHook {
    pub name: String,
    pub enabled: bool,
    pub priority: AiPriority,
    pub request_count: u64,
}

impl IntelligenceEngine {
    const fn new() -> Self {
        IntelligenceEngine {
            enabled: true,
            context: AiContext {
                current_app: String::new(),
                time_of_day: 0,
                day_of_week: 0,
                battery_level: 100,
                is_charging: false,
                network_type: NetworkContext::WiFi,
                user_activity: ActivityLevel::Idle,
                location_context: String::new(),
                recent_apps: Vec::new(),
                active_notifications: 0,
            },
            behavior_patterns: Vec::new(),
            anomaly_history: Vec::new(),
            request_count: 0,
            model_cache: BTreeMap::new(),
            subsystem_hooks: Vec::new(),
            learning_rate: 0.01,
            privacy_mode: PrivacyMode::Full,
        }
    }

    /// Update the system context (called periodically by the kernel)
    pub fn update_context(&mut self) {
        let now = crate::time::clock::unix_time();
        self.context.time_of_day = ((now / 3600) % 24) as u8;
        self.context.day_of_week = ((now / 86400) % 7) as u8;
    }

    /// Register a subsystem hook
    pub fn register_hook(&mut self, name: &str, priority: AiPriority) {
        self.subsystem_hooks.push(SubsystemHook {
            name: String::from(name),
            enabled: true,
            priority,
            request_count: 0,
        });
    }

    /// Process an AI request from any subsystem
    pub fn process_request(&mut self, request: AiRequest, input: &str) -> AiResult {
        self.request_count = self.request_count.saturating_add(1);

        match request {
            AiRequest::Classify => self.classify_text(input),
            AiRequest::Sentiment => self.analyze_sentiment(input),
            AiRequest::Summarize => self.summarize_text(input),
            AiRequest::ExtractEntities => self.extract_entities(input),
            AiRequest::Autocomplete => self.autocomplete(input),
            AiRequest::AnomalyDetect => self.detect_anomaly(input),
            AiRequest::ThreatDetect => self.detect_threat(input),
            AiRequest::PredictNextApp => self.predict_next_app(),
            AiRequest::PredictNextAction => self.predict_next_action(),
            AiRequest::PredictResource => self.predict_resource(input),
            AiRequest::SmartSchedule => self.smart_schedule(input),
            AiRequest::OptimizePower => self.optimize_power(),
            AiRequest::OptimizeMemory => self.optimize_memory(),
            AiRequest::OptimizeNetwork => self.optimize_network(),
            _ => AiResult {
                request_type: request,
                confidence: 0.0,
                label: String::from("unsupported"),
                data: Vec::new(),
                numeric: 0.0,
            },
        }
    }

    fn classify_text(&self, text: &str) -> AiResult {
        let lower = text.to_lowercase();
        let label = if lower.contains("error") || lower.contains("fail") || lower.contains("crash")
        {
            "error"
        } else if lower.contains("warning") || lower.contains("warn") {
            "warning"
        } else if lower.contains("success") || lower.contains("ok") || lower.contains("done") {
            "success"
        } else if lower.contains("help") || lower.contains("how") || lower.contains("?") {
            "question"
        } else if lower.contains("open") || lower.contains("run") || lower.contains("start") {
            "command"
        } else {
            "general"
        };
        AiResult {
            request_type: AiRequest::Classify,
            confidence: 0.85,
            label: String::from(label),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    fn analyze_sentiment(&self, text: &str) -> AiResult {
        let lower = text.to_lowercase();
        let positive = [
            "good",
            "great",
            "excellent",
            "love",
            "nice",
            "happy",
            "thanks",
            "awesome",
        ];
        let negative = [
            "bad", "terrible", "hate", "awful", "horrible", "angry", "slow", "broken",
        ];
        let mut score: f32 = 0.0;
        let words: Vec<&str> = lower.split_whitespace().collect();
        let word_count = words.len().max(1) as f32;
        for word in &words {
            if positive.iter().any(|p| word.contains(p)) {
                score += 1.0;
            }
            if negative.iter().any(|n| word.contains(n)) {
                score -= 1.0;
            }
        }
        score /= word_count;
        let label = if score > 0.1 {
            "positive"
        } else if score < -0.1 {
            "negative"
        } else {
            "neutral"
        };
        AiResult {
            request_type: AiRequest::Sentiment,
            confidence: 0.75,
            label: String::from(label),
            data: Vec::new(),
            numeric: score,
        }
    }

    fn summarize_text(&self, text: &str) -> AiResult {
        // Extract first sentence or first 100 chars as summary
        let summary = if let Some(end) = text.find('.') {
            if end < 200 {
                &text[..end + 1]
            } else {
                &text[..100.min(text.len())]
            }
        } else {
            &text[..100.min(text.len())]
        };
        AiResult {
            request_type: AiRequest::Summarize,
            confidence: 0.7,
            label: String::from(summary),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    fn extract_entities(&self, text: &str) -> AiResult {
        let mut entities = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();
        for word in &words {
            if word.len() > 1 && word.chars().next().map_or(false, |c| c.is_uppercase()) {
                entities.push((String::from("entity"), String::from(*word)));
            }
            if word.contains('@') {
                entities.push((String::from("email"), String::from(*word)));
            }
            if word.chars().all(|c| c.is_numeric() || c == '.' || c == '-') && word.len() > 2 {
                entities.push((String::from("number"), String::from(*word)));
            }
        }
        AiResult {
            request_type: AiRequest::ExtractEntities,
            confidence: 0.7,
            label: alloc::format!("{} entities", entities.len()),
            data: entities,
            numeric: 0.0,
        }
    }

    fn autocomplete(&self, prefix: &str) -> AiResult {
        // Common shell commands and OS interactions
        let completions = [
            "help", "ls", "cd", "cat", "grep", "find", "ps", "kill", "mount", "unmount",
            "ifconfig", "ping", "ssh", "wget", "settings", "open", "close", "shutdown", "reboot",
        ];
        let matches: Vec<(String, String)> = completions
            .iter()
            .filter(|c| c.starts_with(prefix))
            .map(|c| (String::from("completion"), String::from(*c)))
            .collect();
        AiResult {
            request_type: AiRequest::Autocomplete,
            confidence: 0.9,
            label: if matches.is_empty() {
                String::new()
            } else {
                matches[0].1.clone()
            },
            data: matches,
            numeric: 0.0,
        }
    }

    fn detect_anomaly(&self, metric: &str) -> AiResult {
        // Simple threshold-based anomaly detection
        let severity = if metric.contains("critical") {
            0.9
        } else if metric.contains("high") {
            0.7
        } else if metric.contains("medium") {
            0.5
        } else {
            0.2
        };
        AiResult {
            request_type: AiRequest::AnomalyDetect,
            confidence: 0.8,
            label: if severity > 0.6 {
                String::from("anomaly_detected")
            } else {
                String::from("normal")
            },
            data: Vec::new(),
            numeric: severity,
        }
    }

    fn detect_threat(&self, event: &str) -> AiResult {
        let lower = event.to_lowercase();
        let threat_indicators = [
            "brute_force",
            "injection",
            "overflow",
            "escalation",
            "rootkit",
            "backdoor",
            "exfiltrate",
            "suspicious",
            "unauthorized",
            "exploit",
            "malware",
            "ransomware",
        ];
        let mut threat_level: f32 = 0.0;
        for indicator in &threat_indicators {
            if lower.contains(indicator) {
                threat_level += 0.3;
            }
        }
        threat_level = threat_level.min(1.0);
        let label = if threat_level > 0.6 {
            "threat_high"
        } else if threat_level > 0.3 {
            "threat_medium"
        } else if threat_level > 0.0 {
            "threat_low"
        } else {
            "safe"
        };
        AiResult {
            request_type: AiRequest::ThreatDetect,
            confidence: 0.85,
            label: String::from(label),
            data: Vec::new(),
            numeric: threat_level,
        }
    }

    fn predict_next_app(&self) -> AiResult {
        // Use behavior patterns to predict
        let hour = self.context.time_of_day;
        let mut best_app = String::from("shell");
        let mut best_score: f32 = 0.0;
        for pattern in &self.behavior_patterns {
            if pattern.hour == hour && pattern.pattern_type == "app_launch" {
                let score = pattern.frequency as f32 * pattern.confidence;
                if score > best_score {
                    best_score = score;
                    best_app = pattern.action.clone();
                }
            }
        }
        AiResult {
            request_type: AiRequest::PredictNextApp,
            confidence: best_score.min(1.0),
            label: best_app,
            data: Vec::new(),
            numeric: best_score,
        }
    }

    fn predict_next_action(&self) -> AiResult {
        let hour = self.context.time_of_day;
        let action = match hour {
            6..=8 => "check_notifications",
            9..=11 => "open_work_apps",
            12..=13 => "browse_media",
            14..=17 => "productivity",
            18..=20 => "entertainment",
            21..=23 => "wind_down",
            _ => "sleep",
        };
        AiResult {
            request_type: AiRequest::PredictNextAction,
            confidence: 0.65,
            label: String::from(action),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    fn predict_resource(&self, resource: &str) -> AiResult {
        // Predict resource needs based on patterns
        let prediction = match resource {
            "cpu" => match self.context.user_activity {
                ActivityLevel::Gaming | ActivityLevel::HeavyUse => 0.9,
                ActivityLevel::ActiveUse => 0.6,
                ActivityLevel::LightUse => 0.3,
                _ => 0.1,
            },
            "memory" => match self.context.user_activity {
                ActivityLevel::Gaming | ActivityLevel::HeavyUse => 0.8,
                ActivityLevel::Media => 0.7,
                ActivityLevel::ActiveUse => 0.5,
                _ => 0.2,
            },
            "network" => match self.context.network_type {
                NetworkContext::WiFi => 0.3,
                NetworkContext::Cellular | NetworkContext::Metered => 0.6,
                _ => 0.1,
            },
            _ => 0.5,
        };
        AiResult {
            request_type: AiRequest::PredictResource,
            confidence: 0.7,
            label: alloc::format!("{}_{:.0}pct", resource, prediction * 100.0),
            data: Vec::new(),
            numeric: prediction,
        }
    }

    fn smart_schedule(&self, task: &str) -> AiResult {
        // Determine optimal time to run background tasks
        let battery = self.context.battery_level;
        let charging = self.context.is_charging;
        let activity = self.context.user_activity;

        let should_run = (charging && battery > 20)
            || (battery > 50 && matches!(activity, ActivityLevel::Idle | ActivityLevel::Sleeping));

        AiResult {
            request_type: AiRequest::SmartSchedule,
            confidence: 0.8,
            label: if should_run {
                String::from("run_now")
            } else {
                String::from("defer")
            },
            data: alloc::vec![(String::from("task"), String::from(task))],
            numeric: if should_run { 1.0 } else { 0.0 },
        }
    }

    fn optimize_power(&self) -> AiResult {
        let suggestion = match self.context.user_activity {
            ActivityLevel::Sleeping => "deep_sleep",
            ActivityLevel::Idle => "aggressive_doze",
            ActivityLevel::LightUse => "moderate_doze",
            ActivityLevel::ActiveUse => "balanced",
            ActivityLevel::HeavyUse | ActivityLevel::Gaming => "performance",
            ActivityLevel::Media => "media_optimized",
        };
        AiResult {
            request_type: AiRequest::OptimizePower,
            confidence: 0.85,
            label: String::from(suggestion),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    fn optimize_memory(&self) -> AiResult {
        let action = match self.context.user_activity {
            ActivityLevel::Gaming | ActivityLevel::HeavyUse => "expand_cache",
            ActivityLevel::Idle | ActivityLevel::Sleeping => "compact_memory",
            _ => "balanced",
        };
        AiResult {
            request_type: AiRequest::OptimizeMemory,
            confidence: 0.8,
            label: String::from(action),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    fn optimize_network(&self) -> AiResult {
        let strategy = match (self.context.network_type, self.context.battery_level > 30) {
            (NetworkContext::WiFi, _) => "prefetch_aggressive",
            (NetworkContext::Cellular, true) => "prefetch_conservative",
            (NetworkContext::Metered, _) => "minimize_data",
            (NetworkContext::Roaming, _) => "essential_only",
            _ => "offline_cache",
        };
        AiResult {
            request_type: AiRequest::OptimizeNetwork,
            confidence: 0.85,
            label: String::from(strategy),
            data: Vec::new(),
            numeric: 0.0,
        }
    }

    /// Learn from user behavior
    pub fn learn_behavior(&mut self, pattern_type: &str, action: &str) {
        if self.privacy_mode == PrivacyMode::Disabled {
            return;
        }

        let hour = self.context.time_of_day;
        let day = self.context.day_of_week;

        if let Some(existing) = self.behavior_patterns.iter_mut().find(|p| {
            p.pattern_type == pattern_type && p.action == action && p.hour == hour && p.day == day
        }) {
            existing.frequency = existing.frequency.saturating_add(1);
            existing.confidence = (existing.confidence + self.learning_rate).min(1.0);
        } else {
            self.behavior_patterns.push(BehaviorPattern {
                pattern_type: String::from(pattern_type),
                hour,
                day,
                action: String::from(action),
                frequency: 1,
                confidence: 0.1,
            });
        }
    }

    /// Report an anomaly
    pub fn report_anomaly(&mut self, subsystem: &str, metric: &str, expected: f32, actual: f32) {
        let diff = (actual - expected).abs();
        let severity = (diff / expected.abs().max(1.0)).min(1.0);
        if severity > 0.3 {
            self.anomaly_history.push(AnomalyReport {
                subsystem: String::from(subsystem),
                severity,
                description: alloc::format!(
                    "{}: expected {:.1}, got {:.1}",
                    metric,
                    expected,
                    actual
                ),
                metric_name: String::from(metric),
                expected_value: expected,
                actual_value: actual,
                timestamp: crate::time::clock::unix_time(),
            });
        }
    }

    pub fn pattern_count(&self) -> usize {
        self.behavior_patterns.len()
    }
    pub fn anomaly_count(&self) -> usize {
        self.anomaly_history.len()
    }
    pub fn hook_count(&self) -> usize {
        self.subsystem_hooks.len()
    }
}

static ENGINE: Mutex<IntelligenceEngine> = Mutex::new(IntelligenceEngine::new());

pub fn init() {
    let mut engine = ENGINE.lock();
    // Register all subsystem hooks
    engine.register_hook("security", AiPriority::Critical);
    engine.register_hook("network", AiPriority::High);
    engine.register_hook("filesystem", AiPriority::Normal);
    engine.register_hook("process", AiPriority::High);
    engine.register_hook("display", AiPriority::Normal);
    engine.register_hook("accessibility", AiPriority::High);
    engine.register_hook("media", AiPriority::Normal);
    engine.register_hook("storage", AiPriority::Low);
    engine.register_hook("power", AiPriority::High);
    engine.register_hook("services", AiPriority::Normal);
    engine.register_hook("connectivity", AiPriority::Normal);
    engine.register_hook("biometrics", AiPriority::Critical);
    engine.register_hook("enterprise", AiPriority::Normal);
    engine.register_hook("i18n", AiPriority::Low);
    engine.register_hook("crossdevice", AiPriority::Normal);
    engine.register_hook("input", AiPriority::High);
    engine.register_hook("shell", AiPriority::Normal);
    engine.update_context();
    crate::serial_println!(
        "    [intelligence] OS-wide AI intelligence bus initialized ({} hooks)",
        engine.hook_count()
    );
}

/// Process an AI request from any subsystem
pub fn request(req: AiRequest, input: &str) -> AiResult {
    ENGINE.lock().process_request(req, input)
}

/// Learn from user behavior
pub fn learn(pattern_type: &str, action: &str) {
    ENGINE.lock().learn_behavior(pattern_type, action);
}

/// Report anomaly from subsystem
pub fn report_anomaly(subsystem: &str, metric: &str, expected: f32, actual: f32) {
    ENGINE
        .lock()
        .report_anomaly(subsystem, metric, expected, actual);
}

/// Update context periodically
pub fn update_context() {
    ENGINE.lock().update_context();
}

/// Set battery level for AI context
pub fn set_battery(level: u8, charging: bool) {
    let mut engine = ENGINE.lock();
    engine.context.battery_level = level;
    engine.context.is_charging = charging;
}

/// Set user activity level
pub fn set_activity(activity: ActivityLevel) {
    ENGINE.lock().context.user_activity = activity;
}

/// Set current app for context
pub fn set_current_app(app: &str) {
    ENGINE.lock().context.current_app = String::from(app);
}
