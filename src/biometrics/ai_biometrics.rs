use crate::sync::Mutex;
/// AI-enhanced biometric authentication for Genesis
///
/// Adaptive authentication, liveness detection,
/// behavioral biometrics (gait, typing patterns),
/// continuous authentication, and spoof detection.
///
/// Original implementation for Hoags OS.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Liveness Detection ──

#[derive(Clone, Copy, PartialEq)]
pub enum LivenessResult {
    Live,
    Spoof,
    Uncertain,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SpoofType {
    Photo,
    Video,
    Mask,
    SiliconFingerprint,
    Replay,
    Unknown,
}

// ── Behavioral Biometrics ──

#[derive(Clone, Copy)]
struct TypingPattern {
    avg_dwell_ms: u32,         // key held down
    avg_flight_ms: u32,        // between key presses
    avg_digraph_ms: [u32; 10], // common digraph timings
    wpm_estimate: u32,
    error_rate: u32, // per 1000 keystrokes
    sample_count: u32,
}

#[derive(Clone, Copy)]
struct GaitPattern {
    stride_length_mm: u32,
    cadence_steps_per_min: u32,
    impact_force: u32,   // relative units
    symmetry_score: u32, // 0-100
    sample_count: u32,
}

#[derive(Clone, Copy)]
struct TouchPattern {
    avg_pressure: u32,    // 0-1000
    avg_touch_size: u32,  // pixels
    avg_swipe_speed: u32, // pixels/sec
    preferred_thumb: u8,  // 0=left, 1=right
    sample_count: u32,
}

// ── Continuous Auth ──

#[derive(Clone, Copy, PartialEq)]
pub enum AuthConfidence {
    High,   // >90% - full access
    Medium, // 60-90% - some restrictions
    Low,    // 30-60% - limited access
    None,   // <30% - lock
}

#[derive(Clone, Copy)]
struct AuthSession {
    user_id: u32,
    confidence: u32,         // 0-100
    last_explicit_auth: u64, // timestamp
    last_behavior_check: u64,
    typing_score: u32, // 0-100
    gait_score: u32,   // 0-100
    touch_score: u32,  // 0-100
    face_score: u32,   // 0-100
}

// ── AI Engine ──

struct AiBiometricEngine {
    typing_profiles: Vec<(u32, TypingPattern)>, // (user_id, pattern)
    gait_profiles: Vec<(u32, GaitPattern)>,
    touch_profiles: Vec<(u32, TouchPattern)>,
    sessions: Vec<AuthSession>,
    spoof_attempts: u32,
    auth_count: u32,
}

static AI_BIOMETRICS: Mutex<Option<AiBiometricEngine>> = Mutex::new(None);

impl AiBiometricEngine {
    fn new() -> Self {
        AiBiometricEngine {
            typing_profiles: Vec::new(),
            gait_profiles: Vec::new(),
            touch_profiles: Vec::new(),
            sessions: Vec::new(),
            spoof_attempts: 0,
            auth_count: 0,
        }
    }

    /// Check liveness from face capture features
    fn check_liveness(
        &self,
        texture_variance: u32,
        depth_range: u32,
        eye_blink_detected: bool,
        micro_movement: u32,
    ) -> LivenessResult {
        let mut score = 0u32;

        // Real faces have texture variance (vs flat photos)
        if texture_variance > 50 {
            score += 25;
        }
        if texture_variance > 100 {
            score += 10;
        }

        // Depth range indicates 3D structure
        if depth_range > 20 {
            score += 25;
        }
        if depth_range > 50 {
            score += 10;
        }

        // Eye blink is strong liveness signal
        if eye_blink_detected {
            score += 20;
        }

        // Micro-movements (tremor, breathing)
        if micro_movement > 5 && micro_movement < 50 {
            score += 10;
        }

        if score >= 60 {
            LivenessResult::Live
        } else if score >= 30 {
            LivenessResult::Uncertain
        } else {
            LivenessResult::Spoof
        }
    }

    /// Match typing pattern against enrolled profile
    fn match_typing(&self, user_id: u32, dwell_ms: u32, flight_ms: u32, wpm: u32) -> u32 {
        if let Some((_, profile)) = self.typing_profiles.iter().find(|(id, _)| *id == user_id) {
            if profile.sample_count < 5 {
                return 50;
            } // not enough data

            let dwell_diff = if dwell_ms > profile.avg_dwell_ms {
                dwell_ms - profile.avg_dwell_ms
            } else {
                profile.avg_dwell_ms - dwell_ms
            };
            let flight_diff = if flight_ms > profile.avg_flight_ms {
                flight_ms - profile.avg_flight_ms
            } else {
                profile.avg_flight_ms - flight_ms
            };
            let wpm_diff = if wpm > profile.wpm_estimate {
                wpm - profile.wpm_estimate
            } else {
                profile.wpm_estimate - wpm
            };

            let mut score = 100u32;
            // Penalize deviations
            if dwell_diff > 20 {
                score = score.saturating_sub(dwell_diff / 2);
            }
            if flight_diff > 30 {
                score = score.saturating_sub(flight_diff / 3);
            }
            if wpm_diff > 15 {
                score = score.saturating_sub(wpm_diff);
            }
            score
        } else {
            50 // no profile
        }
    }

    /// Match gait pattern against enrolled profile
    fn match_gait(&self, user_id: u32, stride_mm: u32, cadence: u32, _impact: u32) -> u32 {
        if let Some((_, profile)) = self.gait_profiles.iter().find(|(id, _)| *id == user_id) {
            if profile.sample_count < 10 {
                return 50;
            }

            let stride_diff = if stride_mm > profile.stride_length_mm {
                stride_mm - profile.stride_length_mm
            } else {
                profile.stride_length_mm - stride_mm
            };
            let cadence_diff = if cadence > profile.cadence_steps_per_min {
                cadence - profile.cadence_steps_per_min
            } else {
                profile.cadence_steps_per_min - cadence
            };

            let mut score = 100u32;
            if stride_diff > 50 {
                score = score.saturating_sub(stride_diff / 10);
            }
            if cadence_diff > 10 {
                score = score.saturating_sub(cadence_diff * 2);
            }
            score
        } else {
            50
        }
    }

    /// Update continuous auth confidence
    fn update_session_confidence(&mut self, user_id: u32) -> AuthConfidence {
        if let Some(session) = self.sessions.iter_mut().find(|s| s.user_id == user_id) {
            // Weighted combination of all biometric scores
            let combined = (session.typing_score * 25
                + session.gait_score * 20
                + session.touch_score * 25
                + session.face_score * 30)
                / 100;
            session.confidence = combined;

            if combined >= 90 {
                AuthConfidence::High
            } else if combined >= 60 {
                AuthConfidence::Medium
            } else if combined >= 30 {
                AuthConfidence::Low
            } else {
                AuthConfidence::None
            }
        } else {
            AuthConfidence::None
        }
    }

    /// Enroll typing pattern
    fn enroll_typing(&mut self, user_id: u32, dwell_ms: u32, flight_ms: u32, wpm: u32) {
        if let Some((_, profile)) = self
            .typing_profiles
            .iter_mut()
            .find(|(id, _)| *id == user_id)
        {
            // Update running average
            let n = profile.sample_count;
            profile.avg_dwell_ms = (profile.avg_dwell_ms * n + dwell_ms) / (n + 1);
            profile.avg_flight_ms = (profile.avg_flight_ms * n + flight_ms) / (n + 1);
            profile.wpm_estimate = (profile.wpm_estimate * n + wpm) / (n + 1);
            profile.sample_count = profile.sample_count.saturating_add(1);
        } else if self.typing_profiles.len() < 64 {
            self.typing_profiles.push((
                user_id,
                TypingPattern {
                    avg_dwell_ms: dwell_ms,
                    avg_flight_ms: flight_ms,
                    avg_digraph_ms: [0; 10],
                    wpm_estimate: wpm,
                    error_rate: 0,
                    sample_count: 1,
                },
            ));
        }
    }

    /// Enroll gait pattern
    fn enroll_gait(
        &mut self,
        user_id: u32,
        stride_mm: u32,
        cadence: u32,
        impact: u32,
        symmetry: u32,
    ) {
        if let Some((_, profile)) = self.gait_profiles.iter_mut().find(|(id, _)| *id == user_id) {
            let n = profile.sample_count;
            profile.stride_length_mm = (profile.stride_length_mm * n + stride_mm) / (n + 1);
            profile.cadence_steps_per_min = (profile.cadence_steps_per_min * n + cadence) / (n + 1);
            profile.impact_force = (profile.impact_force * n + impact) / (n + 1);
            profile.symmetry_score = (profile.symmetry_score * n + symmetry) / (n + 1);
            profile.sample_count = profile.sample_count.saturating_add(1);
        } else if self.gait_profiles.len() < 64 {
            self.gait_profiles.push((
                user_id,
                GaitPattern {
                    stride_length_mm: stride_mm,
                    cadence_steps_per_min: cadence,
                    impact_force: impact,
                    symmetry_score: symmetry,
                    sample_count: 1,
                },
            ));
        }
    }

    /// Detect spoof type from capture anomalies
    fn classify_spoof(
        &self,
        flat_texture: bool,
        no_depth: bool,
        periodic_pattern: bool,
        edge_artifacts: bool,
    ) -> SpoofType {
        if flat_texture && no_depth && !periodic_pattern {
            SpoofType::Photo
        } else if periodic_pattern && !no_depth {
            SpoofType::Video
        } else if !flat_texture && no_depth {
            SpoofType::Mask
        } else if edge_artifacts {
            SpoofType::Replay
        } else {
            SpoofType::Unknown
        }
    }
}

pub fn init() {
    let mut engine = AI_BIOMETRICS.lock();
    *engine = Some(AiBiometricEngine::new());
    serial_println!("    AI biometrics: liveness, behavioral auth, continuous auth ready");
}

/// Check face liveness
pub fn check_liveness(texture_var: u32, depth: u32, blink: bool, movement: u32) -> LivenessResult {
    let engine = AI_BIOMETRICS.lock();
    engine
        .as_ref()
        .map(|e| e.check_liveness(texture_var, depth, blink, movement))
        .unwrap_or(LivenessResult::Uncertain)
}

/// Get continuous auth confidence for a user
pub fn auth_confidence(user_id: u32) -> AuthConfidence {
    let mut engine = AI_BIOMETRICS.lock();
    engine
        .as_mut()
        .map(|e| e.update_session_confidence(user_id))
        .unwrap_or(AuthConfidence::None)
}

/// Match typing behavior
pub fn match_typing(user_id: u32, dwell_ms: u32, flight_ms: u32, wpm: u32) -> u32 {
    let engine = AI_BIOMETRICS.lock();
    engine
        .as_ref()
        .map(|e| e.match_typing(user_id, dwell_ms, flight_ms, wpm))
        .unwrap_or(50)
}
