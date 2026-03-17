/// AI-powered accessibility for Genesis
///
/// Intelligent screen reading, image descriptions, sound recognition,
/// smart navigation, context-aware verbosity, gesture coaching.
///
/// Inspired by: Apple VoiceOver Intelligence, Google Lookout. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// AI accessibility feature
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiAccessFeature {
    ImageDescription,
    SoundRecognition,
    SmartNavigation,
    TextSimplification,
    GestureCoaching,
    ContextVerbosity,
    LiveTranslation,
    SignLanguageDetect,
}

/// Image description for screen reader
pub struct ImageDescription {
    pub short_desc: String,
    pub detailed_desc: String,
    pub detected_text: String,
    pub objects: Vec<String>,
    pub scene: String,
    /// Confidence as a fixed-point percentage: 0–100 (100 = 100% confident).
    pub confidence: u8,
}

/// Sound recognition event
pub struct SoundEvent {
    pub sound_type: RecognizedSound,
    /// Confidence as a fixed-point percentage: 0–100.
    pub confidence: u8,
    pub timestamp: u64,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecognizedSound {
    Doorbell,
    Knock,
    Alarm,
    Siren,
    Baby,
    Dog,
    Cat,
    Appliance,
    WaterRunning,
    PhoneRing,
    PersonSpeaking,
    Music,
    CarHorn,
    Glass,
    Smoke,
}

/// Smart navigation hint
pub struct NavHint {
    pub action: String,
    pub target: String,
    /// Confidence as a fixed-point percentage: 0–100.
    pub confidence: u8,
    pub shortcut: String,
}

/// AI accessibility engine
pub struct AiAccessEngine {
    pub enabled: bool,
    pub active_features: Vec<AiAccessFeature>,
    pub sound_history: Vec<SoundEvent>,
    pub description_cache: Vec<(u64, ImageDescription)>,
    pub verbosity_level: VerbosityLevel,
    pub auto_verbosity: bool,
    pub sound_recognition_on: bool,
    pub image_description_on: bool,
    pub total_descriptions: u64,
    pub total_sounds: u64,
    /// Reading speed as words-per-minute (integer). Default 200 wpm.
    pub user_reading_speed_wpm: u32,
    pub preferred_detail: DetailLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbosityLevel {
    Minimal,
    Low,
    Medium,
    High,
    Maximum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailLevel {
    Brief,
    Standard,
    Detailed,
}

impl AiAccessEngine {
    const fn new() -> Self {
        AiAccessEngine {
            enabled: true,
            active_features: Vec::new(),
            sound_history: Vec::new(),
            description_cache: Vec::new(),
            verbosity_level: VerbosityLevel::Medium,
            auto_verbosity: true,
            sound_recognition_on: true,
            image_description_on: true,
            total_descriptions: 0,
            total_sounds: 0,
            user_reading_speed_wpm: 200,
            preferred_detail: DetailLevel::Standard,
        }
    }

    /// Describe an image for screen reader.
    ///
    /// Uses integer-only arithmetic (no f32/f64) for `#![no_std]`
    /// bare-metal compatibility.  Aspect ratio is expressed as a fixed-point
    /// ratio: `aspect_x100 = width * 100 / height` (i.e., 100 = 1:1).
    pub fn describe_image(
        &mut self,
        _pixels: &[u8],
        width: u32,
        height: u32,
        alt_text: &str,
    ) -> ImageDescription {
        self.total_descriptions = self.total_descriptions.saturating_add(1);
        // Compute aspect ratio as integer × 100 (e.g. 1:1 → 100, 16:9 → 177).
        let h = height.max(1);
        let aspect_x100 = width.saturating_mul(100) / h;

        let scene = if aspect_x100 > 200 {
            "panoramic view"
        } else if aspect_x100 > 130 {
            "landscape photo"
        } else if aspect_x100 < 80 {
            "portrait photo"
        } else {
            "image"
        };

        let short_desc = if !alt_text.is_empty() {
            String::from(alt_text)
        } else {
            alloc::format!("{} ({}x{})", scene, width, height)
        };

        // Express aspect ratio as "W:H" integers for the description string.
        let ar_w = width;
        let ar_h = h;
        let gcd = {
            let mut a = ar_w;
            let mut b = ar_h;
            while b != 0 {
                let t = b;
                b = a % b;
                a = t;
            }
            a.max(1)
        };
        let detailed_desc = alloc::format!(
            "{}, resolution {}x{} pixels, aspect ratio {}:{}",
            short_desc,
            width,
            height,
            ar_w / gcd,
            ar_h / gcd
        );

        // Confidence: 90 if alt text was provided, 50 if we are guessing.
        let confidence: u8 = if alt_text.is_empty() { 50 } else { 90 };

        ImageDescription {
            short_desc,
            detailed_desc,
            detected_text: String::new(),
            objects: Vec::new(),
            scene: String::from(scene),
            confidence,
        }
    }

    /// Recognize a sound from audio samples.
    ///
    /// Confidence is returned as an integer percentage (0–100).
    pub fn recognize_sound(&mut self, _samples: &[i16], duration_ms: u32) -> Option<SoundEvent> {
        self.total_sounds = self.total_sounds.saturating_add(1);
        // Placeholder classifier: categorise by clip duration.
        let sound = if duration_ms < 500 {
            Some(RecognizedSound::Knock)
        } else if duration_ms < 2000 {
            Some(RecognizedSound::Doorbell)
        } else {
            None
        };

        sound.map(|s| {
            let ts = crate::time::clock::unix_time();
            // Confidence: 60% for this heuristic classifier.
            let confidence: u8 = 60;
            self.sound_history.push(SoundEvent {
                sound_type: s,
                confidence,
                timestamp: ts,
                duration_ms,
            });
            SoundEvent {
                sound_type: s,
                confidence,
                timestamp: ts,
                duration_ms,
            }
        })
    }

    /// Get smart navigation hints based on current UI context.
    ///
    /// Confidence values are integer percentages (0–100).
    pub fn get_nav_hints(&self, current_element: &str, element_type: &str) -> Vec<NavHint> {
        let mut hints = Vec::new();

        match element_type {
            "button" => {
                hints.push(NavHint {
                    action: String::from("Activate"),
                    target: String::from(current_element),
                    confidence: 95,
                    shortcut: String::from("Enter or Space"),
                });
            }
            "text_field" => {
                hints.push(NavHint {
                    action: String::from("Edit"),
                    target: String::from(current_element),
                    confidence: 95,
                    shortcut: String::from("Enter to edit, Escape to exit"),
                });
            }
            "list" => {
                hints.push(NavHint {
                    action: String::from("Navigate items"),
                    target: String::from(current_element),
                    confidence: 90,
                    shortcut: String::from("Up/Down arrows"),
                });
            }
            _ => {}
        }

        // Context-aware suggestions
        hints.push(NavHint {
            action: String::from("Next element"),
            target: String::new(),
            confidence: 80,
            shortcut: String::from("Tab"),
        });

        hints
    }

    /// Simplify text for easier comprehension
    pub fn simplify_text(&self, text: &str) -> String {
        // Simple readability improvement: shorten sentences, replace complex words
        let mut simplified = String::from(text);
        let replacements = [
            ("utilize", "use"),
            ("implement", "make"),
            ("approximately", "about"),
            ("subsequently", "then"),
            ("nevertheless", "but"),
            ("furthermore", "also"),
            ("demonstrate", "show"),
            ("facilitate", "help"),
            ("commence", "start"),
            ("terminate", "end"),
        ];
        for (complex, simple) in &replacements {
            if simplified.contains(complex) {
                // Simple word replacement
                let parts: Vec<&str> = simplified.split(complex).collect();
                simplified = parts.join(simple);
            }
        }
        simplified
    }

    /// Adjust verbosity based on user behavior
    pub fn auto_adjust_verbosity(&mut self, user_skipping: bool, user_repeating: bool) {
        if !self.auto_verbosity {
            return;
        }
        if user_skipping {
            self.verbosity_level = match self.verbosity_level {
                VerbosityLevel::Maximum => VerbosityLevel::High,
                VerbosityLevel::High => VerbosityLevel::Medium,
                VerbosityLevel::Medium => VerbosityLevel::Low,
                _ => VerbosityLevel::Minimal,
            };
        }
        if user_repeating {
            self.verbosity_level = match self.verbosity_level {
                VerbosityLevel::Minimal => VerbosityLevel::Low,
                VerbosityLevel::Low => VerbosityLevel::Medium,
                VerbosityLevel::Medium => VerbosityLevel::High,
                _ => VerbosityLevel::Maximum,
            };
        }
    }
}

static AI_ACCESS: Mutex<AiAccessEngine> = Mutex::new(AiAccessEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-a11y] AI accessibility initialized (descriptions, sounds, navigation)"
    );
}

pub fn describe_image(pixels: &[u8], w: u32, h: u32, alt: &str) -> ImageDescription {
    AI_ACCESS.lock().describe_image(pixels, w, h, alt)
}

pub fn recognize_sound(samples: &[i16], dur: u32) -> Option<SoundEvent> {
    AI_ACCESS.lock().recognize_sound(samples, dur)
}

pub fn simplify_text(text: &str) -> String {
    AI_ACCESS.lock().simplify_text(text)
}
