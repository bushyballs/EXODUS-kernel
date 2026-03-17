/// Login and password authentication
///
/// Password storage: PBKDF2-SHA256 with per-user random salt.
/// The static user table (`USER_TABLE`) holds up to 8 entries.
/// A default admin user is pre-populated with a well-known password hash.
/// Never stores plaintext passwords.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Static user table
// ---------------------------------------------------------------------------

/// Maximum users in the static table.
const MAX_USERS: usize = 8;

/// One entry in the static user table.
#[derive(Clone, Copy)]
struct UserEntry {
    /// Null-terminated username (up to 31 bytes + NUL).
    username: [u8; 32],
    /// PBKDF2-SHA256 hash of the password (32 bytes).
    password_hash: [u8; 32],
    /// PBKDF2 salt (16 bytes).
    salt: [u8; 16],
    /// Unix timestamp (seconds since epoch) of the last successful login.
    last_login: u64,
    /// Whether this slot is occupied.
    active: bool,
}

impl UserEntry {
    const EMPTY: Self = UserEntry {
        username: [0u8; 32],
        password_hash: [0u8; 32],
        salt: [0u8; 16],
        last_login: 0,
        active: false,
    };
}

/// Copy `src` into a 32-byte NUL-terminated array, truncating if necessary.
fn str_to_fixed32(s: &str) -> [u8; 32] {
    let mut buf = [0u8; 32];
    let bytes = s.as_bytes();
    let len = bytes.len().min(31);
    buf[..len].copy_from_slice(&bytes[..len]);
    buf
}

/// Compare a str against a NUL-terminated fixed-32 array (constant-time length only).
fn fixed32_eq(fixed: &[u8; 32], s: &str) -> bool {
    let sbytes = s.as_bytes();
    // Find the length of the stored name (up to first NUL or 32 bytes).
    let mut stored_len = 0usize;
    while stored_len < 32 && fixed[stored_len] != 0 {
        stored_len += 1;
    }
    if sbytes.len() != stored_len {
        return false;
    }
    // Constant-time comparison over the stored length
    let mut diff: u8 = 0;
    for i in 0..stored_len {
        diff |= fixed[i] ^ sbytes[i];
    }
    diff == 0
}

/// A fixed salt for the bootstrap admin user (not random because it must be
/// a compile-time constant; the first thing `init()` does is re-derive the
/// admin hash with this well-known salt so the values are self-consistent).
const ADMIN_SALT: [u8; 16] = [
    0x48, 0x4f, 0x41, 0x47, 0x53, 0x5f, 0x41, 0x44, 0x4d, 0x49, 0x4e, 0x5f, 0x53, 0x41, 0x4c, 0x54,
];

/// The static user table, protected by a spin-lock.
static USER_TABLE: Mutex<[UserEntry; MAX_USERS]> = Mutex::new([UserEntry::EMPTY; MAX_USERS]);

/// Initialise the user table and populate the default admin entry.
///
/// Called once during kernel boot from `auth::init()`.
pub fn init_user_table() {
    let mut table = USER_TABLE.lock();

    // Slot 0: root / admin (UID 0 maps via security::users)
    let admin_name = str_to_fixed32("admin");
    // Derive the actual PBKDF2 hash with the bootstrap salt
    let admin_hash = pbkdf2_sha256(b"admin", &ADMIN_SALT, 10_000);

    table[0] = UserEntry {
        username: admin_name,
        password_hash: admin_hash,
        salt: ADMIN_SALT,
        last_login: 0,
        active: true,
    };

    // Slot 1: root / hoags — for serial console access
    let root_name = str_to_fixed32("root");
    let root_salt = ADMIN_SALT; // reuse bootstrap salt for simplicity
    let root_hash = pbkdf2_sha256(b"hoags", &root_salt, 10_000);
    table[1] = UserEntry {
        username: root_name,
        password_hash: root_hash,
        salt: root_salt,
        last_login: 0,
        active: true,
    };

    serial_println!("    [auth] User table initialised (2 users: admin, root)");
}

/// Add or update a user in the static table.
///
/// Returns `Ok(())` on success, `Err` when the table is full.
pub fn upsert_user(username: &str, password: &str) -> Result<(), &'static str> {
    let salt = generate_salt();
    let hash = hash_password(password, &salt);
    let name_fixed = str_to_fixed32(username);

    let mut table = USER_TABLE.lock();

    // Update existing entry if username matches
    for entry in table.iter_mut() {
        if entry.active && fixed32_eq(&entry.username, username) {
            entry.password_hash = hash;
            entry.salt = salt;
            return Ok(());
        }
    }

    // Add to first empty slot
    for entry in table.iter_mut() {
        if !entry.active {
            *entry = UserEntry {
                username: name_fixed,
                password_hash: hash,
                salt,
                last_login: 0,
                active: true,
            };
            return Ok(());
        }
    }

    Err("user table full")
}

// ---------------------------------------------------------------------------
// Password primitives
// ---------------------------------------------------------------------------

/// Hash a password with PBKDF2-SHA256 and the given 16-byte salt.
pub fn hash_password(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    pbkdf2_sha256(password.as_bytes(), salt, 10_000)
}

/// PBKDF2-SHA256 key derivation (RFC 2898 §5.2, one output block).
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    // PRF = HMAC-SHA256
    // U1 = HMAC(Password, Salt || INT(1))
    let mut input = Vec::new();
    input.extend_from_slice(salt);
    input.extend_from_slice(&1u32.to_be_bytes());

    let mut u = crate::crypto::hmac::hmac_sha256(password, &input);
    let mut result = u;

    for _ in 1..iterations {
        u = crate::crypto::hmac::hmac_sha256(password, &u);
        for j in 0..32 {
            result[j] ^= u[j];
        }
    }
    result
}

/// Generate a 16-byte random salt using the kernel CSPRNG.
pub fn generate_salt() -> [u8; 16] {
    let mut salt = [0u8; 16];
    crate::crypto::random::fill_bytes(&mut salt);
    salt
}

/// Verify a password against a stored (salt, hash) pair.
/// Uses constant-time comparison to prevent timing attacks.
pub fn verify_password(password: &str, salt: &[u8; 16], expected_hash: &[u8; 32]) -> bool {
    let computed = hash_password(password, salt);
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= computed[i] ^ expected_hash[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// PasswordEntry — per-user password record stored in the security subsystem
// ---------------------------------------------------------------------------

/// Stored password entry (used by the security subsystem and PAM layer).
#[derive(Debug, Clone)]
pub struct PasswordEntry {
    pub uid: u32,
    pub salt: [u8; 16],
    pub hash: [u8; 32],
    pub must_change: bool,
    pub last_changed: u64,
    pub max_age_days: u32,
}

impl PasswordEntry {
    pub fn new(uid: u32, password: &str) -> Self {
        let salt = generate_salt();
        let hash = hash_password(password, &salt);
        PasswordEntry {
            uid,
            salt,
            hash,
            must_change: false,
            last_changed: crate::time::clock::unix_time(),
            max_age_days: 90,
        }
    }

    pub fn verify(&self, password: &str) -> bool {
        verify_password(password, &self.salt, &self.hash)
    }

    /// Change the stored password and update the `last_changed` timestamp.
    pub fn change_password(&mut self, new_password: &str) {
        self.salt = generate_salt();
        self.hash = hash_password(new_password, &self.salt);
        // Update last_changed to current Unix time
        self.last_changed = crate::time::clock::unix_time();
        serial_println!(
            "    [auth] Password changed (UID {}), last_changed={}",
            self.uid,
            self.last_changed
        );
    }
}

// ---------------------------------------------------------------------------
// authenticate — the main entry point
// ---------------------------------------------------------------------------

/// Authenticate a user by username and password.
///
/// 1. Looks the username up in the static `USER_TABLE`.
/// 2. Derives PBKDF2-SHA256 of the supplied password with the stored salt.
/// 3. Performs a constant-time comparison against the stored hash.
/// 4. On success, updates `last_login` in the table.
/// 5. Also checks the legacy `security::users::USER_DB` to resolve the UID.
///
/// Returns the user's UID on success, or an `AuthError` on failure.
pub fn authenticate(username: &str, password: &str) -> Result<u32, AuthError> {
    // ------------------------------------------------------------------
    // Step 1: Look up in the static user table
    // ------------------------------------------------------------------
    let mut table = USER_TABLE.lock();

    let entry_idx = table
        .iter()
        .position(|e| e.active && fixed32_eq(&e.username, username));

    let idx = match entry_idx {
        Some(i) => i,
        None => {
            serial_println!("    [auth] Login failed — unknown user: {}", username);
            return Err(AuthError::UserNotFound);
        }
    };

    // ------------------------------------------------------------------
    // Step 2: Hash the supplied password with the stored salt
    // ------------------------------------------------------------------
    let salt = table[idx].salt;
    let computed_hash = pbkdf2_sha256(password.as_bytes(), &salt, 10_000);

    // ------------------------------------------------------------------
    // Step 3: Constant-time comparison
    // ------------------------------------------------------------------
    let mut diff: u8 = 0;
    for i in 0..32 {
        diff |= computed_hash[i] ^ table[idx].password_hash[i];
    }

    if diff != 0 {
        serial_println!(
            "    [auth] Login failed — bad password for user: {}",
            username
        );
        return Err(AuthError::BadPassword);
    }

    // ------------------------------------------------------------------
    // Step 4: Update last_login timestamp
    // ------------------------------------------------------------------
    let now = crate::time::clock::unix_time();
    table[idx].last_login = now;

    serial_println!(
        "    [auth] Login successful: {} (last_login={})",
        username,
        now
    );

    // ------------------------------------------------------------------
    // Step 5: Resolve UID from the security user database
    // ------------------------------------------------------------------
    // Drop the table lock before acquiring the user DB lock to avoid
    // potential deadlock ordering issues.
    drop(table);

    let uid = {
        let user_db = crate::security::users::USER_DB.lock();
        match user_db.as_ref() {
            Some(db) => {
                match db.find_user_by_name(username) {
                    Some(u) => u.uid,
                    None => {
                        // User exists in our table but not yet in the security DB.
                        // Default to UID 1000 for regular users, 0 for "admin".
                        if username == "admin" || username == "root" {
                            0
                        } else {
                            1000
                        }
                    }
                }
            }
            None => {
                serial_println!("    [auth] WARNING: security user DB not initialised");
                0
            }
        }
    };

    Ok(uid)
}

// ---------------------------------------------------------------------------
// AuthError
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum AuthError {
    UserNotFound,
    BadPassword,
    AccountLocked,
    PasswordExpired,
    SystemError,
}
