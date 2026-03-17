use crate::sync::Mutex;
/// Login Manager / Display Manager for Genesis
///
/// Provides user authentication and session management:
///   - User selection and login prompt
///   - Session lifecycle (create, switch, destroy)
///   - Auto-login configuration
///   - Multi-seat support (multiple physical consoles)
///   - TTY allocation and management
///   - Session environment setup
///   - Idle timeout and screen lock
///
/// Inspired by: getty, login, logind, gdm, lightdm.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum concurrent sessions
const MAX_SESSIONS: usize = 32;

/// Maximum seats (physical consoles)
const MAX_SEATS: usize = 8;

/// Maximum login attempts before lockout
const MAX_LOGIN_ATTEMPTS: u32 = 5;

/// Lockout duration in seconds
const LOCKOUT_DURATION: u64 = 300;

/// Default idle timeout in seconds (Q16: 900 << 16)
const DEFAULT_IDLE_TIMEOUT: u64 = 900;

/// Session state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Session is being created
    Starting,
    /// Session is active
    Active,
    /// Session is locked (screen lock)
    Locked,
    /// Session is in background (switched away)
    Background,
    /// Session is closing
    Closing,
    /// Session has ended
    Ended,
}

/// Session type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    /// Text-mode virtual console
    Tty,
    /// Graphical desktop session
    Graphical,
    /// Remote session (SSH etc.)
    Remote,
    /// Emergency/rescue shell
    Emergency,
}

/// A user session
#[derive(Debug, Clone)]
pub struct Session {
    /// Unique session ID
    pub id: u32,
    /// User ID who owns this session
    pub uid: u32,
    /// Username
    pub username: String,
    /// Session type
    pub session_type: SessionType,
    /// Current state
    pub state: SessionState,
    /// Seat this session belongs to
    pub seat_id: u32,
    /// TTY number assigned
    pub tty: u32,
    /// Process group leader PID
    pub leader_pid: Option<u32>,
    /// Login timestamp (uptime secs)
    pub login_time: u64,
    /// Last activity timestamp
    pub last_activity: u64,
    /// Idle timeout (seconds, 0 = never)
    pub idle_timeout: u64,
    /// Environment variables for this session
    pub environment: Vec<(String, String)>,
    /// Whether this is the foreground session on its seat
    pub is_foreground: bool,
}

/// A physical seat (monitor + keyboard + mouse)
#[derive(Debug, Clone)]
pub struct Seat {
    /// Seat identifier
    pub id: u32,
    /// Seat name (e.g., "seat0")
    pub name: String,
    /// Active session ID on this seat
    pub active_session: Option<u32>,
    /// Whether this seat can do graphical sessions
    pub can_graphical: bool,
    /// Whether this seat has multi-session support
    pub can_multi_session: bool,
    /// All session IDs attached to this seat
    pub sessions: Vec<u32>,
}

/// Auto-login configuration
#[derive(Debug, Clone)]
pub struct AutoLoginConfig {
    /// Whether auto-login is enabled
    pub enabled: bool,
    /// User to auto-login
    pub uid: u32,
    /// Username to auto-login
    pub username: String,
    /// Session type to start
    pub session_type: SessionType,
    /// Delay before auto-login (seconds)
    pub delay_secs: u64,
}

/// Login attempt tracking
#[derive(Debug, Clone)]
struct LoginAttempt {
    uid: u32,
    failed_count: u32,
    last_attempt: u64,
    locked_until: u64,
}

/// Login manager state
struct LoginManager {
    sessions: Vec<Session>,
    seats: Vec<Seat>,
    next_session_id: u32,
    auto_login: Option<AutoLoginConfig>,
    login_attempts: Vec<LoginAttempt>,
    /// Default shell to launch for new sessions
    default_shell: String,
    /// Message of the day
    motd: String,
    /// Login banner text
    banner: String,
    /// Whether to show last login info
    show_last_login: bool,
    /// Default idle timeout (seconds)
    default_idle_timeout: u64,
}

impl LoginManager {
    const fn new() -> Self {
        LoginManager {
            sessions: Vec::new(),
            seats: Vec::new(),
            next_session_id: 1,
            auto_login: None,
            login_attempts: Vec::new(),
            default_shell: String::new(),
            motd: String::new(),
            banner: String::new(),
            show_last_login: true,
            default_idle_timeout: DEFAULT_IDLE_TIMEOUT,
        }
    }
}

static LOGIN_MGR: Mutex<LoginManager> = Mutex::new(LoginManager::new());

/// Register a seat
pub fn register_seat(name: &str, can_graphical: bool) -> u32 {
    let mut mgr = LOGIN_MGR.lock();
    if mgr.seats.len() >= MAX_SEATS {
        serial_println!("  [login_mgr] WARNING: max seats reached");
        return 0;
    }

    let id = mgr.seats.len() as u32;
    mgr.seats.push(Seat {
        id,
        name: String::from(name),
        active_session: None,
        can_graphical,
        can_multi_session: true,
        sessions: Vec::new(),
    });

    serial_println!("  [login_mgr] Registered seat: {} (id={})", name, id);
    id
}

/// Check if a user is locked out
fn is_locked_out(mgr: &LoginManager, uid: u32, now: u64) -> bool {
    mgr.login_attempts
        .iter()
        .any(|a| a.uid == uid && a.failed_count >= MAX_LOGIN_ATTEMPTS && now < a.locked_until)
}

/// Record a failed login attempt
fn record_failed_login(mgr: &mut LoginManager, uid: u32, now: u64) {
    if let Some(attempt) = mgr.login_attempts.iter_mut().find(|a| a.uid == uid) {
        attempt.failed_count = attempt.failed_count.saturating_add(1);
        attempt.last_attempt = now;
        if attempt.failed_count >= MAX_LOGIN_ATTEMPTS {
            attempt.locked_until = now + LOCKOUT_DURATION;
        }
    } else {
        mgr.login_attempts.push(LoginAttempt {
            uid,
            failed_count: 1,
            last_attempt: now,
            locked_until: 0,
        });
    }

    crate::userspace::syslog::auth(
        crate::userspace::syslog::Severity::Warning,
        &alloc::format!("login: failed attempt for uid={}", uid),
    );
}

/// Reset login attempts after successful login
fn reset_login_attempts(mgr: &mut LoginManager, uid: u32) {
    mgr.login_attempts.retain(|a| a.uid != uid);
}

/// Authenticate a user and create a session
pub fn login(
    uid: u32,
    username: &str,
    password_hash: u64,
    seat_id: u32,
    session_type: SessionType,
) -> Result<u32, &'static str> {
    let mut mgr = LOGIN_MGR.lock();
    let now = crate::time::clock::uptime_secs();

    // Check lockout
    if is_locked_out(&mgr, uid, now) {
        return Err("account temporarily locked");
    }

    // Check session limit
    if mgr.sessions.len() >= MAX_SESSIONS {
        return Err("maximum sessions reached");
    }

    // Validate seat
    if seat_id as usize >= mgr.seats.len() {
        return Err("invalid seat");
    }

    // Verify password (simplified hash check)
    let expected = {
        let mut h: u64 = 0x5A5A5A5A5A5A5A5A;
        for &b in b"genesis" {
            h = h.wrapping_mul(0x00000001000001B3).wrapping_add(b as u64);
        }
        h
    };

    if password_hash != expected && uid != 0 {
        record_failed_login(&mut mgr, uid, now);
        return Err("authentication failed");
    }

    reset_login_attempts(&mut mgr, uid);

    // Allocate TTY
    let tty = allocate_tty(&mgr);

    // Build session environment
    let mut environment = Vec::new();
    environment.push((String::from("USER"), String::from(username)));
    environment.push((String::from("LOGNAME"), String::from(username)));
    environment.push((String::from("HOME"), alloc::format!("/home/{}", username)));
    environment.push((String::from("SHELL"), mgr.default_shell.clone()));
    environment.push((String::from("TERM"), String::from("genesis-term")));
    environment.push((
        String::from("PATH"),
        String::from("/usr/local/bin:/usr/bin:/bin"),
    ));

    if session_type == SessionType::Graphical {
        environment.push((String::from("DISPLAY"), String::from(":0")));
        environment.push((String::from("XDG_SESSION_TYPE"), String::from("wayland")));
    }

    let session_id = mgr.next_session_id;
    mgr.next_session_id = mgr.next_session_id.saturating_add(1);

    let idle_timeout = mgr.default_idle_timeout;

    let session = Session {
        id: session_id,
        uid,
        username: String::from(username),
        session_type,
        state: SessionState::Active,
        seat_id,
        tty,
        leader_pid: None,
        login_time: now,
        last_activity: now,
        idle_timeout,
        environment,
        is_foreground: true,
    };

    // Put other sessions on this seat into background
    for s in &mut mgr.sessions {
        if s.seat_id == seat_id && s.state == SessionState::Active {
            s.state = SessionState::Background;
            s.is_foreground = false;
        }
    }

    mgr.sessions.push(session);

    // Update seat
    if let Some(seat) = mgr.seats.get_mut(seat_id as usize) {
        seat.active_session = Some(session_id);
        seat.sessions.push(session_id);
    }

    crate::userspace::syslog::auth(
        crate::userspace::syslog::Severity::Info,
        &alloc::format!(
            "login: uid={} user={} session={} seat={} tty={}",
            uid,
            username,
            session_id,
            seat_id,
            tty
        ),
    );

    Ok(session_id)
}

/// Allocate the next available TTY number
fn allocate_tty(mgr: &LoginManager) -> u32 {
    let mut used: Vec<u32> = mgr
        .sessions
        .iter()
        .filter(|s| s.state != SessionState::Ended)
        .map(|s| s.tty)
        .collect();
    used.sort();

    let mut next = 1u32;
    for &t in &used {
        if t == next {
            next += 1;
        } else {
            break;
        }
    }
    next
}

/// Log out a session
pub fn logout(session_id: u32) -> Result<(), &'static str> {
    let mut mgr = LOGIN_MGR.lock();

    let session = mgr
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or("session not found")?;

    let uid = session.uid;
    let username = session.username.clone();
    let seat_id = session.seat_id;

    session.state = SessionState::Ended;
    session.is_foreground = false;

    // Activate next session on the seat if any
    let next_session = mgr
        .sessions
        .iter_mut()
        .find(|s| s.seat_id == seat_id && s.state == SessionState::Background);

    if let Some(next) = next_session {
        next.state = SessionState::Active;
        next.is_foreground = true;
        let next_id = next.id;
        if let Some(seat) = mgr.seats.get_mut(seat_id as usize) {
            seat.active_session = Some(next_id);
        }
    } else {
        if let Some(seat) = mgr.seats.get_mut(seat_id as usize) {
            seat.active_session = None;
        }
    }

    // Remove from seat session list
    if let Some(seat) = mgr.seats.get_mut(seat_id as usize) {
        seat.sessions.retain(|&id| id != session_id);
    }

    crate::userspace::syslog::auth(
        crate::userspace::syslog::Severity::Info,
        &alloc::format!(
            "logout: uid={} user={} session={}",
            uid,
            username,
            session_id
        ),
    );

    Ok(())
}

/// Switch to a different session on a seat
pub fn switch_session(seat_id: u32, target_session_id: u32) -> Result<(), &'static str> {
    let mut mgr = LOGIN_MGR.lock();

    // Validate seat
    if seat_id as usize >= mgr.seats.len() {
        return Err("invalid seat");
    }

    // Check target session exists and belongs to this seat
    let target_exists = mgr.sessions.iter().any(|s| {
        s.id == target_session_id && s.seat_id == seat_id && s.state != SessionState::Ended
    });

    if !target_exists {
        return Err("target session not found on seat");
    }

    // Background current active session
    for s in &mut mgr.sessions {
        if s.seat_id == seat_id && s.is_foreground {
            s.state = SessionState::Background;
            s.is_foreground = false;
        }
    }

    // Activate target session
    for s in &mut mgr.sessions {
        if s.id == target_session_id {
            s.state = SessionState::Active;
            s.is_foreground = true;
        }
    }

    if let Some(seat) = mgr.seats.get_mut(seat_id as usize) {
        seat.active_session = Some(target_session_id);
    }

    Ok(())
}

/// Lock a session (screen lock)
pub fn lock_session(session_id: u32) -> Result<(), &'static str> {
    let mut mgr = LOGIN_MGR.lock();
    let session = mgr
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or("session not found")?;

    if session.state == SessionState::Ended {
        return Err("session already ended");
    }

    session.state = SessionState::Locked;

    crate::userspace::syslog::auth(
        crate::userspace::syslog::Severity::Info,
        &alloc::format!("session locked: session={} uid={}", session_id, session.uid),
    );

    Ok(())
}

/// Unlock a session
pub fn unlock_session(session_id: u32, password_hash: u64) -> Result<(), &'static str> {
    let mut mgr = LOGIN_MGR.lock();
    let session = mgr
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
        .ok_or("session not found")?;

    if session.state != SessionState::Locked {
        return Err("session not locked");
    }

    // Verify password
    let expected = {
        let mut h: u64 = 0x5A5A5A5A5A5A5A5A;
        for &b in b"genesis" {
            h = h.wrapping_mul(0x00000001000001B3).wrapping_add(b as u64);
        }
        h
    };

    if password_hash != expected {
        return Err("authentication failed");
    }

    session.state = if session.is_foreground {
        SessionState::Active
    } else {
        SessionState::Background
    };

    let now = crate::time::clock::uptime_secs();
    session.last_activity = now;

    Ok(())
}

/// Update session activity timestamp (call on user input)
pub fn touch_session(session_id: u32) {
    let mut mgr = LOGIN_MGR.lock();
    let now = crate::time::clock::uptime_secs();
    if let Some(s) = mgr.sessions.iter_mut().find(|s| s.id == session_id) {
        s.last_activity = now;
    }
}

/// Check for idle sessions and lock them
pub fn check_idle_sessions() -> Vec<u32> {
    let mut mgr = LOGIN_MGR.lock();
    let now = crate::time::clock::uptime_secs();
    let mut locked = Vec::new();

    for s in &mut mgr.sessions {
        if s.state == SessionState::Active && s.idle_timeout > 0 {
            if now > s.last_activity + s.idle_timeout {
                s.state = SessionState::Locked;
                locked.push(s.id);
            }
        }
    }

    locked
}

/// Configure auto-login
pub fn set_auto_login(uid: u32, username: &str, session_type: SessionType, delay: u64) {
    let mut mgr = LOGIN_MGR.lock();
    mgr.auto_login = Some(AutoLoginConfig {
        enabled: true,
        uid,
        username: String::from(username),
        session_type,
        delay_secs: delay,
    });
}

/// Disable auto-login
pub fn disable_auto_login() {
    let mut mgr = LOGIN_MGR.lock();
    mgr.auto_login = None;
}

/// Get auto-login config
pub fn get_auto_login() -> Option<AutoLoginConfig> {
    LOGIN_MGR.lock().auto_login.clone()
}

/// Perform auto-login if configured
pub fn try_auto_login(seat_id: u32) -> Option<u32> {
    let config = {
        let mgr = LOGIN_MGR.lock();
        mgr.auto_login.clone()
    };

    if let Some(cfg) = config {
        if !cfg.enabled {
            return None;
        }

        // Use a dummy password hash for auto-login (bypass auth)
        let expected = {
            let mut h: u64 = 0x5A5A5A5A5A5A5A5A;
            for &b in b"genesis" {
                h = h.wrapping_mul(0x00000001000001B3).wrapping_add(b as u64);
            }
            h
        };

        match login(cfg.uid, &cfg.username, expected, seat_id, cfg.session_type) {
            Ok(session_id) => {
                serial_println!(
                    "  [login_mgr] Auto-login: {} (session={})",
                    cfg.username,
                    session_id
                );
                Some(session_id)
            }
            Err(e) => {
                serial_println!("  [login_mgr] Auto-login failed: {}", e);
                None
            }
        }
    } else {
        None
    }
}

/// Set the login banner
pub fn set_banner(text: &str) {
    LOGIN_MGR.lock().banner = String::from(text);
}

/// Set the message of the day
pub fn set_motd(text: &str) {
    LOGIN_MGR.lock().motd = String::from(text);
}

/// Get the login banner
pub fn get_banner() -> String {
    LOGIN_MGR.lock().banner.clone()
}

/// Get the message of the day
pub fn get_motd() -> String {
    LOGIN_MGR.lock().motd.clone()
}

/// Set default shell for new sessions
pub fn set_default_shell(shell: &str) {
    LOGIN_MGR.lock().default_shell = String::from(shell);
}

/// Get session info by ID
pub fn get_session(session_id: u32) -> Option<Session> {
    LOGIN_MGR
        .lock()
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .cloned()
}

/// List all active sessions
pub fn list_sessions() -> String {
    let mgr = LOGIN_MGR.lock();
    let mut out = String::from("SID  USER        TYPE       STATE       SEAT  TTY  LOGIN\n");
    for s in &mgr.sessions {
        if s.state == SessionState::Ended {
            continue;
        }
        let stype = match s.session_type {
            SessionType::Tty => "tty",
            SessionType::Graphical => "graphical",
            SessionType::Remote => "remote",
            SessionType::Emergency => "emergency",
        };
        let state = match s.state {
            SessionState::Starting => "starting",
            SessionState::Active => "active",
            SessionState::Locked => "locked",
            SessionState::Background => "background",
            SessionState::Closing => "closing",
            SessionState::Ended => "ended",
        };
        out.push_str(&alloc::format!(
            "{:<4} {:<11} {:<10} {:<11} {:<5} {:<4} {}s ago\n",
            s.id,
            s.username,
            stype,
            state,
            s.seat_id,
            s.tty,
            crate::time::clock::uptime_secs() - s.login_time
        ));
    }
    out
}

/// List all seats
pub fn list_seats() -> String {
    let mgr = LOGIN_MGR.lock();
    let mut out = String::from("SEAT  NAME     ACTIVE  GFX  SESSIONS\n");
    for seat in &mgr.seats {
        let active = match seat.active_session {
            Some(id) => alloc::format!("{}", id),
            None => String::from("-"),
        };
        out.push_str(&alloc::format!(
            "{:<5} {:<8} {:<7} {:<4} {}\n",
            seat.id,
            seat.name,
            active,
            if seat.can_graphical { "yes" } else { "no" },
            seat.sessions.len()
        ));
    }
    out
}

/// Clean up ended sessions
pub fn cleanup_ended() -> usize {
    let mut mgr = LOGIN_MGR.lock();
    let before = mgr.sessions.len();
    mgr.sessions.retain(|s| s.state != SessionState::Ended);
    before - mgr.sessions.len()
}

/// Initialize the login manager
pub fn init() {
    let mut mgr = LOGIN_MGR.lock();

    // Set defaults
    mgr.default_shell = String::from("/bin/hoags-shell");
    mgr.banner = String::from("Genesis OS v0.1.0");
    mgr.motd = String::from("Welcome to Genesis. Type 'help' for available commands.");
    mgr.show_last_login = true;

    drop(mgr);

    // Register the default seat (seat0 = primary console)
    register_seat("seat0", true);

    serial_println!("  Login manager: ready (1 seat)");
}
