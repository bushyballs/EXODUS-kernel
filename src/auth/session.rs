use crate::sync::Mutex;
/// Session management — user sessions and screen lock
///
/// After login, a session token is created. The token:
///   - Is cryptographically random (256-bit)
///   - Is bound to a UID and TTY/display
///   - Expires after configurable timeout
///   - Can be invalidated (logout/lock)
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;

static SESSIONS: Mutex<Option<SessionManager>> = Mutex::new(None);

/// Session token (256-bit random)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionToken(pub [u8; 32]);

impl SessionToken {
    pub fn generate() -> Self {
        let mut token = [0u8; 32];
        crate::crypto::random::fill_bytes(&mut token);
        SessionToken(token)
    }

    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for byte in &self.0 {
            s.push_str(&alloc::format!("{:02x}", byte));
        }
        s
    }
}

/// A user session
#[derive(Debug, Clone)]
pub struct Session {
    pub token: SessionToken,
    pub uid: u32,
    pub username: String,
    pub created_at: u64,
    pub last_active: u64,
    pub timeout_secs: u64,
    pub locked: bool,
    pub tty: String,
}

impl Session {
    pub fn is_expired(&self, now: u64) -> bool {
        now > self.last_active + self.timeout_secs
    }

    pub fn touch(&mut self, now: u64) {
        self.last_active = now;
    }
}

/// Session manager
pub struct SessionManager {
    sessions: BTreeMap<[u8; 32], Session>,
    active_session: Option<[u8; 32]>,
    default_timeout: u64,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            sessions: BTreeMap::new(),
            active_session: None,
            default_timeout: 3600, // 1 hour
        }
    }

    /// Create a new session after successful login
    pub fn create(&mut self, uid: u32, username: &str, tty: &str) -> SessionToken {
        let token = SessionToken::generate();
        let now = crate::time::clock::unix_time();

        let session = Session {
            token: token.clone(),
            uid,
            username: String::from(username),
            created_at: now,
            last_active: now,
            timeout_secs: self.default_timeout,
            locked: false,
            tty: String::from(tty),
        };

        self.sessions.insert(token.0, session);
        self.active_session = Some(token.0);

        serial_println!(
            "    [session] Created session for {} (UID {})",
            username,
            uid
        );
        token
    }

    /// Validate a session token
    pub fn validate(&mut self, token: &SessionToken) -> Option<&Session> {
        let now = crate::time::clock::unix_time();
        let session = self.sessions.get_mut(&token.0)?;

        if session.is_expired(now) {
            return None;
        }

        session.touch(now);
        Some(session)
    }

    /// Lock the current session (screen lock)
    pub fn lock(&mut self) {
        if let Some(key) = self.active_session {
            if let Some(session) = self.sessions.get_mut(&key) {
                session.locked = true;
                serial_println!("    [session] Screen locked for {}", session.username);
            }
        }
    }

    /// Unlock the current session (requires password)
    pub fn unlock(&mut self, password: &str) -> Result<(), &'static str> {
        let key = self.active_session.ok_or("no active session")?;
        let session = self.sessions.get(&key).ok_or("session not found")?;

        // Verify password
        match super::login::authenticate(&session.username, password) {
            Ok(_) => {
                if let Some(s) = self.sessions.get_mut(&key) {
                    s.locked = false;
                    let now = crate::time::clock::unix_time();
                    s.touch(now);
                }
                serial_println!("    [session] Screen unlocked");
                Ok(())
            }
            Err(_) => Err("bad password"),
        }
    }

    /// Destroy a session (logout)
    pub fn destroy(&mut self, token: &SessionToken) {
        if let Some(session) = self.sessions.remove(&token.0) {
            serial_println!("    [session] Destroyed session for {}", session.username);
        }
        if self.active_session == Some(token.0) {
            self.active_session = None;
        }
    }

    /// Get the active session
    pub fn active(&self) -> Option<&Session> {
        self.active_session.and_then(|key| self.sessions.get(&key))
    }

    /// Get active user's UID
    pub fn active_uid(&self) -> Option<u32> {
        self.active().map(|s| s.uid)
    }

    /// Clean up expired sessions
    pub fn cleanup(&mut self) {
        let now = crate::time::clock::unix_time();
        self.sessions.retain(|_, s| !s.is_expired(now));
    }
}

pub fn init() {
    *SESSIONS.lock() = Some(SessionManager::new());
    serial_println!("    [session] Session manager initialized");
}

/// Create a session after login
pub fn create(uid: u32, username: &str, tty: &str) -> SessionToken {
    SESSIONS
        .lock()
        .as_mut()
        .map(|m| m.create(uid, username, tty))
        .unwrap_or(SessionToken([0; 32]))
}

/// Lock screen
pub fn lock() {
    if let Some(m) = SESSIONS.lock().as_mut() {
        m.lock();
    }
}

/// Check if screen is locked
pub fn is_locked() -> bool {
    SESSIONS
        .lock()
        .as_ref()
        .and_then(|m| m.active())
        .map(|s| s.locked)
        .unwrap_or(false)
}
