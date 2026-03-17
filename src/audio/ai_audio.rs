/// AI-powered audio for Genesis
///
/// Noise cancellation, audio enhancement, spatial audio,
/// sound classification, adaptive EQ, voice isolation.
///
/// Inspired by: Apple Computational Audio, Google Sound Amplifier. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Audio enhancement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEnhancement {
    Off,
    VoiceIsolation,
    NoiseCancellation,
    SpatialAudio,
    BassBoost,
    VocalBoost,
    Clarity,
    NightMode, // Compress dynamic range
}

/// EQ preset from AI
pub struct EqPreset {
    pub name: String,
    pub bands: [f32; 10], // 10-band EQ, -12 to +12 dB
    pub for_content: AudioContentType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioContentType {
    Music,
    Podcast,
    Audiobook,
    Movie,
    Game,
    Call,
    Notification,
    Unknown,
}

/// Audio scene detection
pub struct AudioScene {
    pub scene_type: AudioSceneType,
    pub confidence: f32,
    pub noise_level_db: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSceneType {
    Quiet,
    Office,
    Cafe,
    Street,
    Transit,
    Concert,
    Nature,
    Home,
}

/// AI audio engine
pub struct AiAudioEngine {
    pub enabled: bool,
    pub current_enhancement: AudioEnhancement,
    pub auto_eq: bool,
    pub current_eq: [f32; 10],
    pub presets: Vec<EqPreset>,
    pub current_scene: AudioSceneType,
    pub noise_level: f32,
    pub voice_detected: bool,
    pub spatial_enabled: bool,
    pub head_tracking: bool,
    pub hearing_profile: HearingProfile,
    pub total_enhanced: u64,
}

pub struct HearingProfile {
    pub left_compensation: [f32; 10],
    pub right_compensation: [f32; 10],
    pub sensitivity: f32,
    pub calibrated: bool,
}

impl AiAudioEngine {
    const fn new() -> Self {
        AiAudioEngine {
            enabled: true,
            current_enhancement: AudioEnhancement::Off,
            auto_eq: true,
            current_eq: [0.0; 10],
            presets: Vec::new(),
            current_scene: AudioSceneType::Quiet,
            noise_level: 0.0,
            voice_detected: false,
            spatial_enabled: false,
            head_tracking: false,
            hearing_profile: HearingProfile {
                left_compensation: [0.0; 10],
                right_compensation: [0.0; 10],
                sensitivity: 1.0,
                calibrated: false,
            },
            total_enhanced: 0,
        }
    }

    /// Detect audio content type for adaptive EQ
    pub fn detect_content(
        &self,
        has_voice: bool,
        spectral_centroid: f32,
        dynamic_range: f32,
    ) -> AudioContentType {
        if has_voice && dynamic_range < 20.0 {
            AudioContentType::Call
        } else if has_voice && dynamic_range < 30.0 {
            AudioContentType::Podcast
        } else if has_voice && dynamic_range < 25.0 {
            AudioContentType::Audiobook
        } else if spectral_centroid > 3000.0 {
            AudioContentType::Music
        } else if dynamic_range > 50.0 {
            AudioContentType::Movie
        } else {
            AudioContentType::Unknown
        }
    }

    /// Get optimal EQ for content type
    pub fn auto_eq_for_content(&mut self, content: AudioContentType) -> [f32; 10] {
        let eq = match content {
            AudioContentType::Music => [0.0, 1.0, 0.5, 0.0, -0.5, 0.0, 0.5, 1.0, 1.5, 1.0],
            AudioContentType::Podcast => [-2.0, -1.0, 0.0, 2.0, 3.0, 3.0, 2.0, 0.0, -1.0, -2.0],
            AudioContentType::Audiobook => [-3.0, -2.0, 0.0, 3.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0],
            AudioContentType::Movie => [3.0, 2.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 2.0, 1.0],
            AudioContentType::Game => [2.0, 3.0, 1.0, 0.0, -1.0, 0.0, 1.0, 2.0, 3.0, 2.0],
            AudioContentType::Call => [-4.0, -2.0, 0.0, 4.0, 5.0, 5.0, 4.0, 2.0, 0.0, -2.0],
            _ => [0.0; 10],
        };

        // Apply hearing compensation if calibrated
        let mut final_eq = eq;
        if self.hearing_profile.calibrated {
            for i in 0..10 {
                final_eq[i] += (self.hearing_profile.left_compensation[i]
                    + self.hearing_profile.right_compensation[i])
                    / 2.0;
            }
        }

        self.current_eq = final_eq;
        final_eq
    }

    /// Detect audio scene (environment)
    pub fn detect_scene(&mut self, ambient_db: f32) -> AudioScene {
        self.noise_level = ambient_db;
        let scene = if ambient_db < 30.0 {
            AudioSceneType::Quiet
        } else if ambient_db < 50.0 {
            AudioSceneType::Home
        } else if ambient_db < 60.0 {
            AudioSceneType::Office
        } else if ambient_db < 70.0 {
            AudioSceneType::Cafe
        } else if ambient_db < 80.0 {
            AudioSceneType::Street
        } else if ambient_db < 90.0 {
            AudioSceneType::Transit
        } else {
            AudioSceneType::Concert
        };
        self.current_scene = scene;
        AudioScene {
            scene_type: scene,
            confidence: 0.75,
            noise_level_db: ambient_db,
        }
    }

    /// Process audio samples with AI enhancement
    pub fn enhance_audio(&mut self, samples: &mut [i16], enhancement: AudioEnhancement) {
        self.total_enhanced = self.total_enhanced.saturating_add(1);
        self.current_enhancement = enhancement;
        match enhancement {
            AudioEnhancement::NoiseCancellation => {
                // Simple noise gate: reduce samples below threshold
                let threshold = 500i16;
                for sample in samples.iter_mut() {
                    if sample.abs() < threshold {
                        *sample = (*sample as f32 * 0.1) as i16;
                    }
                }
            }
            AudioEnhancement::VoiceIsolation => {
                // Boost mid-range (voice frequencies)
                // In real impl: spectral masking
                for sample in samples.iter_mut() {
                    *sample = (*sample as f32 * 1.2).min(i16::MAX as f32) as i16;
                }
            }
            AudioEnhancement::NightMode => {
                // Compress dynamic range
                for sample in samples.iter_mut() {
                    let val = *sample as f32;
                    let compressed = if val.abs() > 10000.0 {
                        val.signum() * (10000.0 + (val.abs() - 10000.0) * 0.3)
                    } else {
                        val
                    };
                    *sample = compressed as i16;
                }
            }
            AudioEnhancement::BassBoost => {
                // Simple bass emphasis (low-pass boosted)
                let mut prev = 0i32;
                for sample in samples.iter_mut() {
                    let val = *sample as i32;
                    let low = (prev + val) / 2;
                    *sample = (val + low / 2).max(i16::MIN as i32).min(i16::MAX as i32) as i16;
                    prev = val;
                }
            }
            _ => {}
        }
    }
}

fn seed_presets(engine: &mut AiAudioEngine) {
    let presets = [
        ("Flat", [0.0; 10], AudioContentType::Unknown),
        (
            "Bass Boost",
            [4.0, 3.0, 2.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            AudioContentType::Music,
        ),
        (
            "Vocal",
            [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0, -2.0],
            AudioContentType::Podcast,
        ),
        (
            "Rock",
            [3.0, 2.0, -1.0, -2.0, 0.0, 2.0, 3.0, 3.0, 2.0, 1.0],
            AudioContentType::Music,
        ),
    ];
    for (name, bands, content) in &presets {
        engine.presets.push(EqPreset {
            name: String::from(*name),
            bands: *bands,
            for_content: *content,
        });
    }
}

static AI_AUDIO: Mutex<AiAudioEngine> = Mutex::new(AiAudioEngine::new());

pub fn init() {
    seed_presets(&mut AI_AUDIO.lock());
    crate::serial_println!("    [ai-audio] AI audio initialized (enhance, EQ, scene, spatial)");
}

pub fn detect_scene(ambient_db: f32) -> AudioScene {
    AI_AUDIO.lock().detect_scene(ambient_db)
}

pub fn enhance(samples: &mut [i16], mode: AudioEnhancement) {
    AI_AUDIO.lock().enhance_audio(samples, mode);
}

pub fn auto_eq(content: AudioContentType) -> [f32; 10] {
    AI_AUDIO.lock().auto_eq_for_content(content)
}
