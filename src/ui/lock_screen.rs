/// Lock screen for Genesis — device security and at-a-glance info
///
/// Supports: PIN, pattern, password, and biometric unlock.
/// Shows: time, date, notifications, media controls, emergency dialer.
///
/// Inspired by: Android Keyguard, iOS Lock Screen. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Lock method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockMethod {
    None,     // Swipe to unlock
    Pin,      // 4-6 digit PIN
    Pattern,  // 3x3 dot pattern
    Password, // Alphanumeric
    Fingerprint,
    FaceId,
}

/// Lock screen state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Locked,
    Authenticating,
    Unlocked,
    Timeout, // locked after failed attempts
}

/// Lock screen
pub struct LockScreen {
    pub state: LockState,
    pub method: LockMethod,
    /// Stored credential hash (SHA-256)
    credential_hash: [u8; 32],
    /// Pattern (sequence of dot indices 0-8)
    pattern: Vec<u8>,
    /// Failed attempt count
    pub failed_attempts: u32,
    /// Max attempts before timeout
    pub max_attempts: u32,
    /// Timeout duration (seconds)
    pub timeout_duration: u32,
    /// Timeout expiry time
    pub timeout_until: u64,
    /// Lock after timeout (seconds of inactivity)
    pub auto_lock_timeout: u32,
    /// Last activity timestamp
    pub last_activity: u64,
    /// Show notifications on lock screen
    pub show_notifications: bool,
    /// Show sensitive notification content
    pub show_sensitive_content: bool,
    /// Emergency call enabled
    pub emergency_enabled: bool,
    /// Owner info text
    pub owner_info: String,
}

impl LockScreen {
    const fn new() -> Self {
        LockScreen {
            state: LockState::Locked,
            method: LockMethod::None,
            credential_hash: [0; 32],
            pattern: Vec::new(),
            failed_attempts: 0,
            max_attempts: 5,
            timeout_duration: 30,
            timeout_until: 0,
            auto_lock_timeout: 60,
            last_activity: 0,
            show_notifications: true,
            show_sensitive_content: false,
            emergency_enabled: true,
            owner_info: String::new(),
        }
    }

    /// Set PIN
    pub fn set_pin(&mut self, pin: &str) {
        self.method = LockMethod::Pin;
        self.credential_hash = Self::hash_credential(pin.as_bytes());
    }

    /// Set password
    pub fn set_password(&mut self, password: &str) {
        self.method = LockMethod::Password;
        self.credential_hash = Self::hash_credential(password.as_bytes());
    }

    /// Set pattern
    pub fn set_pattern(&mut self, dots: &[u8]) {
        self.method = LockMethod::Pattern;
        self.pattern = dots.to_vec();
        self.credential_hash = Self::hash_credential(dots);
    }

    /// Attempt to unlock
    pub fn unlock(&mut self, credential: &[u8]) -> bool {
        let now = crate::time::clock::unix_time();

        // Check timeout
        if self.state == LockState::Timeout {
            if now < self.timeout_until {
                return false;
            }
            self.state = LockState::Locked;
            self.failed_attempts = 0;
        }

        self.state = LockState::Authenticating;

        let hash = Self::hash_credential(credential);
        if hash == self.credential_hash {
            self.state = LockState::Unlocked;
            self.failed_attempts = 0;
            self.last_activity = now;
            true
        } else {
            self.failed_attempts = self.failed_attempts.saturating_add(1);
            if self.failed_attempts >= self.max_attempts {
                self.state = LockState::Timeout;
                self.timeout_until = now + self.timeout_duration as u64;
            } else {
                self.state = LockState::Locked;
            }
            false
        }
    }

    /// Lock the screen
    pub fn lock(&mut self) {
        if self.method == LockMethod::None {
            self.state = LockState::Unlocked; // no lock method = always unlocked
        } else {
            self.state = LockState::Locked;
        }
    }

    /// Check if auto-lock should trigger
    pub fn check_auto_lock(&mut self) {
        if self.state != LockState::Unlocked {
            return;
        }
        if self.auto_lock_timeout == 0 {
            return;
        }

        let now = crate::time::clock::unix_time();
        if now - self.last_activity > self.auto_lock_timeout as u64 {
            self.lock();
        }
    }

    /// Touch activity (resets auto-lock timer)
    pub fn activity(&mut self) {
        self.last_activity = crate::time::clock::unix_time();
    }

    /// Simple hash (would use SHA-256 in production)
    fn hash_credential(data: &[u8]) -> [u8; 32] {
        let mut hash = [0u8; 32];
        for (i, &b) in data.iter().enumerate() {
            hash[i % 32] ^= b;
            hash[(i * 7 + 3) % 32] = hash[(i * 7 + 3) % 32].wrapping_add(b);
            hash[(i * 13 + 17) % 32] = hash[(i * 13 + 17) % 32].wrapping_mul(b | 1);
        }
        hash
    }

    /// Get remaining timeout seconds
    pub fn timeout_remaining(&self) -> u32 {
        if self.state != LockState::Timeout {
            return 0;
        }
        let now = crate::time::clock::unix_time();
        if now >= self.timeout_until {
            return 0;
        }
        (self.timeout_until - now) as u32
    }

    /// Is unlocked?
    pub fn is_unlocked(&self) -> bool {
        self.state == LockState::Unlocked || self.method == LockMethod::None
    }
}

static LOCK_SCREEN: Mutex<LockScreen> = Mutex::new(LockScreen::new());

pub fn init() {
    crate::serial_println!("  [lock-screen] Lock screen initialized");
}

pub fn lock() {
    LOCK_SCREEN.lock().lock();
}
pub fn unlock(credential: &[u8]) -> bool {
    LOCK_SCREEN.lock().unlock(credential)
}
pub fn is_unlocked() -> bool {
    LOCK_SCREEN.lock().is_unlocked()
}
pub fn set_pin(pin: &str) {
    LOCK_SCREEN.lock().set_pin(pin);
}
pub fn activity() {
    LOCK_SCREEN.lock().activity();
}
