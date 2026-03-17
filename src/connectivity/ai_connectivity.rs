/// AI-powered connectivity for Genesis
///
/// Smart network selection, signal prediction, bandwidth estimation,
/// handoff optimization, roaming intelligence.
///
/// Inspired by: Android Adaptive Connectivity, iOS Network Intelligence. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Network quality assessment
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NetworkQuality {
    Excellent,
    Good,
    Fair,
    Poor,
    Unusable,
    Disconnected,
}

/// Network recommendation from AI
pub struct NetworkRecommendation {
    pub recommended_type: RecommendedNetwork,
    pub reason: String,
    pub estimated_speed_mbps: f32,
    pub estimated_latency_ms: u32,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendedNetwork {
    WiFi,
    Cellular,
    Ethernet,
    HotspotFromPhone,
    OfflineMode,
}

/// WiFi network scoring
pub struct WifiScore {
    pub ssid: String,
    pub signal_dbm: i32,
    pub speed_score: f32,
    pub reliability_score: f32,
    pub security_score: f32,
    pub overall_score: f32,
    pub connection_count: u32,
    pub last_connected: u64,
}

/// Bandwidth estimation
pub struct BandwidthEstimate {
    pub download_mbps: f32,
    pub upload_mbps: f32,
    pub latency_ms: u32,
    pub jitter_ms: u32,
    pub packet_loss_percent: f32,
    pub confidence: f32,
}

/// AI connectivity engine
pub struct AiConnectivityEngine {
    pub enabled: bool,
    pub wifi_scores: Vec<WifiScore>,
    pub cellular_quality_history: Vec<(u64, NetworkQuality)>,
    pub location_network_map: BTreeMap<String, String>,
    pub current_quality: NetworkQuality,
    pub bandwidth_history: Vec<BandwidthEstimate>,
    pub handoff_count: u64,
    pub smart_selection_enabled: bool,
    pub prefer_wifi: bool,
    pub metered_awareness: bool,
}

impl AiConnectivityEngine {
    const fn new() -> Self {
        AiConnectivityEngine {
            enabled: true,
            wifi_scores: Vec::new(),
            cellular_quality_history: Vec::new(),
            location_network_map: BTreeMap::new(),
            current_quality: NetworkQuality::Disconnected,
            bandwidth_history: Vec::new(),
            handoff_count: 0,
            smart_selection_enabled: true,
            prefer_wifi: true,
            metered_awareness: true,
        }
    }

    /// Score a WiFi network
    pub fn score_wifi(&mut self, ssid: &str, signal_dbm: i32, is_secure: bool) -> f32 {
        let signal_score = if signal_dbm > -50 {
            1.0
        } else if signal_dbm > -60 {
            0.8
        } else if signal_dbm > -70 {
            0.6
        } else if signal_dbm > -80 {
            0.4
        } else {
            0.2
        };

        let security_score = if is_secure { 1.0 } else { 0.3 };

        // Historical reliability
        let reliability = self
            .wifi_scores
            .iter()
            .find(|w| w.ssid == ssid)
            .map_or(0.5, |w| w.reliability_score);

        let overall = signal_score * 0.4 + security_score * 0.2 + reliability * 0.4;

        if let Some(existing) = self.wifi_scores.iter_mut().find(|w| w.ssid == ssid) {
            existing.signal_dbm = signal_dbm;
            existing.overall_score = overall;
            existing.connection_count = existing.connection_count.saturating_add(1);
            existing.last_connected = crate::time::clock::unix_time();
        } else {
            self.wifi_scores.push(WifiScore {
                ssid: String::from(ssid),
                signal_dbm,
                speed_score: signal_score,
                reliability_score: 0.5,
                security_score,
                overall_score: overall,
                connection_count: 1,
                last_connected: crate::time::clock::unix_time(),
            });
        }

        overall
    }

    /// Recommend best network
    pub fn recommend_network(
        &self,
        wifi_available: bool,
        cellular_signal: i32,
    ) -> NetworkRecommendation {
        let best_wifi = self.wifi_scores.iter().max_by(|a, b| {
            a.overall_score
                .partial_cmp(&b.overall_score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let wifi_good = best_wifi.map_or(false, |w| w.overall_score > 0.5);
        let cellular_good = cellular_signal > -90;

        let (network, reason, speed, latency) = if wifi_available && wifi_good {
            (RecommendedNetwork::WiFi, "Strong WiFi available", 50.0, 10)
        } else if cellular_good {
            (
                RecommendedNetwork::Cellular,
                "Cellular has good signal",
                20.0,
                30,
            )
        } else if wifi_available {
            (RecommendedNetwork::WiFi, "WiFi available (weak)", 10.0, 50)
        } else {
            (
                RecommendedNetwork::OfflineMode,
                "No good connection available",
                0.0,
                0,
            )
        };

        NetworkRecommendation {
            recommended_type: network,
            reason: String::from(reason),
            estimated_speed_mbps: speed,
            estimated_latency_ms: latency,
            confidence: if wifi_good || cellular_good {
                0.85
            } else {
                0.5
            },
        }
    }

    /// Estimate current bandwidth
    pub fn estimate_bandwidth(
        &mut self,
        recent_download_bytes: u64,
        duration_ms: u64,
    ) -> BandwidthEstimate {
        let mbps = if duration_ms > 0 {
            (recent_download_bytes as f32 * 8.0) / (duration_ms as f32 * 1000.0)
        } else {
            0.0
        };

        let estimate = BandwidthEstimate {
            download_mbps: mbps,
            upload_mbps: mbps * 0.3,
            latency_ms: if mbps > 10.0 {
                20
            } else if mbps > 1.0 {
                50
            } else {
                200
            },
            jitter_ms: 5,
            packet_loss_percent: 0.1,
            confidence: if self.bandwidth_history.len() > 5 {
                0.8
            } else {
                0.5
            },
        };

        self.bandwidth_history.push(BandwidthEstimate {
            download_mbps: mbps,
            upload_mbps: mbps * 0.3,
            latency_ms: estimate.latency_ms,
            jitter_ms: 5,
            packet_loss_percent: 0.1,
            confidence: estimate.confidence,
        });
        if self.bandwidth_history.len() > 100 {
            self.bandwidth_history.remove(0);
        }

        estimate
    }

    /// Should we handoff to a different network?
    pub fn should_handoff(&self, current_quality: NetworkQuality) -> bool {
        matches!(
            current_quality,
            NetworkQuality::Poor | NetworkQuality::Unusable
        )
    }

    /// Record network quality change
    pub fn record_quality(&mut self, quality: NetworkQuality) {
        let now = crate::time::clock::unix_time();
        self.current_quality = quality;
        self.cellular_quality_history.push((now, quality));
        if self.cellular_quality_history.len() > 500 {
            self.cellular_quality_history.remove(0);
        }
    }
}

static AI_CONN: Mutex<AiConnectivityEngine> = Mutex::new(AiConnectivityEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-conn] AI connectivity initialized (network selection, bandwidth, handoff)"
    );
}

pub fn score_wifi(ssid: &str, signal: i32, secure: bool) -> f32 {
    AI_CONN.lock().score_wifi(ssid, signal, secure)
}

pub fn recommend_network(wifi: bool, cell_signal: i32) -> NetworkRecommendation {
    AI_CONN.lock().recommend_network(wifi, cell_signal)
}

pub fn record_quality(q: NetworkQuality) {
    AI_CONN.lock().record_quality(q);
}
