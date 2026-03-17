/// Biometric prompt for Genesis
///
/// Unified biometric authentication dialog,
/// crypto-backed authentication, and fallback handling.
///
/// Inspired by: Android BiometricPrompt, iOS LAContext. All code is original.
use crate::sync::Mutex;
use alloc::string::String;

/// Biometric strength
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BiometricStrength {
    Strong,           // Fingerprint, Face with depth
    Weak,             // Face without depth
    DeviceCredential, // PIN/pattern/password
}

/// Authentication type used
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    Fingerprint,
    Face,
    Iris,
    Pin,
    Pattern,
    Password,
}

/// Prompt result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptResult {
    Success(AuthType),
    Failed,
    UserCancelled,
    HardwareUnavailable,
    NoneEnrolled,
    Lockout,
    LockoutPermanent,
}

/// Biometric prompt configuration
pub struct PromptConfig {
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub negative_button: String,
    pub allow_device_credential: bool,
    pub required_strength: BiometricStrength,
    pub crypto_object: bool, // require crypto-backed auth
    pub confirmation_required: bool,
}

/// Biometric prompt manager
pub struct BiometricPrompt {
    pub active: bool,
    pub config: Option<PromptConfig>,
    pub last_result: Option<PromptResult>,
    pub available_biometrics: u8, // bitmask: bit0=fingerprint, bit1=face, bit2=iris
}

impl BiometricPrompt {
    const fn new() -> Self {
        BiometricPrompt {
            active: false,
            config: None,
            last_result: None,
            available_biometrics: 0b011, // fingerprint + face
        }
    }

    pub fn show(&mut self, config: PromptConfig) {
        self.active = true;
        self.config = Some(config);
        self.last_result = None;
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.last_result = Some(PromptResult::UserCancelled);
        self.config = None;
    }

    pub fn on_authenticated(&mut self, auth_type: AuthType) {
        self.active = false;
        self.last_result = Some(PromptResult::Success(auth_type));
        self.config = None;
    }

    pub fn on_error(&mut self, permanent: bool) {
        self.active = false;
        self.last_result = Some(if permanent {
            PromptResult::LockoutPermanent
        } else {
            PromptResult::Lockout
        });
    }

    pub fn has_fingerprint(&self) -> bool {
        self.available_biometrics & 0b001 != 0
    }
    pub fn has_face(&self) -> bool {
        self.available_biometrics & 0b010 != 0
    }
    pub fn has_iris(&self) -> bool {
        self.available_biometrics & 0b100 != 0
    }

    pub fn can_authenticate(&self, strength: BiometricStrength) -> bool {
        match strength {
            BiometricStrength::Strong => {
                (self.has_fingerprint() && super::fingerprint::has_enrolled())
                    || (self.has_face() && super::face_auth::has_enrolled())
            }
            BiometricStrength::Weak => self.has_fingerprint() || self.has_face(),
            BiometricStrength::DeviceCredential => true,
        }
    }
}

static PROMPT: Mutex<BiometricPrompt> = Mutex::new(BiometricPrompt::new());

pub fn init() {
    let prompt = PROMPT.lock();
    let count = prompt.has_fingerprint() as u8 + prompt.has_face() as u8 + prompt.has_iris() as u8;
    crate::serial_println!(
        "  [biometrics] Biometric prompt initialized ({} methods)",
        count
    );
}
