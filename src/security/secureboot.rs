use crate::sync::Mutex;
/// Secure boot chain for Genesis
///
/// Verifies the integrity of the boot process:
///   1. Bootloader verifies kernel signature (UEFI Secure Boot)
///   2. Kernel verifies module/driver signatures
///   3. Kernel verifies userspace binary signatures
///   4. Runtime integrity monitoring
///
/// Uses our own crypto: SHA-256 for hashing, HMAC for signatures.
/// Future: Ed25519 signatures for proper public-key verification.
///
/// Trust model:
///   - Platform key (PK) — owned by Hoags Inc
///   - Key Exchange Key (KEK) — signs database updates
///   - Signature Database (db) — contains trusted code hashes/keys
///   - Forbidden Database (dbx) — contains revoked hashes/keys
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static SECUREBOOT: Mutex<Option<SecureBootState>> = Mutex::new(None);

/// Secure boot enforcement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementMode {
    /// Signatures checked but violations only logged (permissive)
    Audit,
    /// Signatures required, unsigned code blocked
    Enforcing,
    /// Disabled — no signature checks
    Disabled,
}

/// Signature verification result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyResult {
    /// Signature is valid
    Valid,
    /// Signature is invalid / doesn't match
    Invalid,
    /// No signature present
    Unsigned,
    /// Signing key has been revoked
    Revoked,
    /// Hash is in the forbidden database
    Forbidden,
}

/// A code signature
#[derive(Debug, Clone)]
pub struct CodeSignature {
    /// SHA-256 hash of the signed content
    pub hash: [u8; 32],
    /// HMAC signature (will be Ed25519 in the future)
    pub signature: [u8; 32],
    /// Signer identity
    pub signer: String,
    /// Timestamp of signing
    pub timestamp: u64,
}

/// A trusted key entry
#[derive(Debug, Clone)]
pub struct TrustedKey {
    /// Key identifier
    pub id: String,
    /// Key material (HMAC key for now, public key later)
    pub key: [u8; 32],
    /// Who this key belongs to
    pub owner: String,
    /// Whether this key can sign other keys
    pub can_sign_keys: bool,
    /// Expiration timestamp (0 = no expiry)
    pub expires: u64,
    /// Revoked
    pub revoked: bool,
}

/// Secure boot state
pub struct SecureBootState {
    /// Current enforcement mode
    pub mode: EnforcementMode,
    /// Trusted signature database (hash -> signature)
    pub trusted_hashes: BTreeMap<[u8; 32], CodeSignature>,
    /// Forbidden hashes (known-bad code)
    pub forbidden_hashes: Vec<[u8; 32]>,
    /// Trusted signing keys
    pub trusted_keys: Vec<TrustedKey>,
    /// Platform key
    pub platform_key: Option<TrustedKey>,
    /// Verification statistics
    pub stats: VerifyStats,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VerifyStats {
    pub total_checks: u64,
    pub valid: u64,
    pub invalid: u64,
    pub unsigned: u64,
    pub revoked: u64,
    pub forbidden: u64,
}

impl SecureBootState {
    pub fn new(mode: EnforcementMode) -> Self {
        SecureBootState {
            mode,
            trusted_hashes: BTreeMap::new(),
            forbidden_hashes: Vec::new(),
            trusted_keys: Vec::new(),
            platform_key: None,
            stats: VerifyStats::default(),
        }
    }

    /// Verify a code blob against the secure boot database
    pub fn verify(&mut self, code: &[u8], signature: Option<&CodeSignature>) -> VerifyResult {
        self.stats.total_checks = self.stats.total_checks.saturating_add(1);

        if self.mode == EnforcementMode::Disabled {
            return VerifyResult::Valid;
        }

        // Hash the code
        let hash = crate::crypto::sha256::hash(code);

        // Check forbidden database first
        if self.forbidden_hashes.iter().any(|h| *h == hash) {
            self.stats.forbidden = self.stats.forbidden.saturating_add(1);
            serial_println!("  [secureboot] FORBIDDEN code hash detected");
            crate::security::audit::log(
                crate::security::audit::AuditEvent::PolicyChange,
                crate::security::audit::AuditResult::Deny,
                0,
                0,
                "forbidden code hash blocked",
            );
            return VerifyResult::Forbidden;
        }

        // Check if hash is in trusted database
        if self.trusted_hashes.contains_key(&hash) {
            self.stats.valid = self.stats.valid.saturating_add(1);
            return VerifyResult::Valid;
        }

        // Check signature if provided
        if let Some(sig) = signature {
            // Verify the signature hash matches
            if sig.hash != hash {
                self.stats.invalid = self.stats.invalid.saturating_add(1);
                return VerifyResult::Invalid;
            }

            // Find the signing key
            let key = self.trusted_keys.iter().find(|k| k.id == sig.signer);
            match key {
                Some(k) if k.revoked => {
                    self.stats.revoked = self.stats.revoked.saturating_add(1);
                    return VerifyResult::Revoked;
                }
                Some(k) => {
                    // Verify HMAC signature
                    let expected = crate::crypto::hmac::hmac_sha256(&k.key, &hash);
                    if constant_time_eq(&expected, &sig.signature) {
                        self.stats.valid = self.stats.valid.saturating_add(1);
                        return VerifyResult::Valid;
                    } else {
                        self.stats.invalid = self.stats.invalid.saturating_add(1);
                        return VerifyResult::Invalid;
                    }
                }
                None => {
                    self.stats.invalid = self.stats.invalid.saturating_add(1);
                    return VerifyResult::Invalid;
                }
            }
        }

        // No signature
        self.stats.unsigned = self.stats.unsigned.saturating_add(1);
        VerifyResult::Unsigned
    }

    /// Add a hash to the trusted database
    pub fn trust_hash(&mut self, hash: [u8; 32], sig: CodeSignature) {
        self.trusted_hashes.insert(hash, sig);
    }

    /// Add a hash to the forbidden database
    pub fn forbid_hash(&mut self, hash: [u8; 32]) {
        if !self.forbidden_hashes.contains(&hash) {
            self.forbidden_hashes.push(hash);
        }
    }

    /// Add a trusted signing key
    pub fn add_key(&mut self, key: TrustedKey) {
        self.trusted_keys.push(key);
    }

    /// Revoke a signing key
    pub fn revoke_key(&mut self, id: &str) {
        for key in &mut self.trusted_keys {
            if key.id == id {
                key.revoked = true;
                serial_println!("  [secureboot] Key revoked: {}", id);
            }
        }
    }
}

/// Constant-time comparison
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Initialize secure boot
pub fn init(mode: EnforcementMode) {
    let mut state = SecureBootState::new(mode);

    // Generate platform key from CSPRNG
    let mut pk_bytes = [0u8; 32];
    crate::crypto::random::fill_bytes(&mut pk_bytes);
    let mut pk_key = [0u8; 32];
    pk_key.copy_from_slice(&pk_bytes[..32]);

    state.platform_key = Some(TrustedKey {
        id: String::from("hoags-platform-key"),
        key: pk_key,
        owner: String::from("Hoags Inc."),
        can_sign_keys: true,
        expires: 0,
        revoked: false,
    });

    // Add platform key to trusted keys
    state.trusted_keys.push(state.platform_key.clone().unwrap());

    *SECUREBOOT.lock() = Some(state);
    serial_println!("  [secureboot] Initialized (mode: {:?})", mode);
}

/// Verify code before execution
pub fn verify(code: &[u8], signature: Option<&CodeSignature>) -> VerifyResult {
    SECUREBOOT
        .lock()
        .as_mut()
        .map(|sb| sb.verify(code, signature))
        .unwrap_or(VerifyResult::Valid)
}

/// Check if secure boot is enforcing
pub fn is_enforcing() -> bool {
    SECUREBOOT
        .lock()
        .as_ref()
        .map(|sb| sb.mode == EnforcementMode::Enforcing)
        .unwrap_or(false)
}
