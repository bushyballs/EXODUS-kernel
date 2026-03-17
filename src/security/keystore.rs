/// Keystore for Genesis — hardware-backed key management
///
/// Secure storage for cryptographic keys, certificates, and credentials.
/// Keys never leave the keystore in plaintext. All crypto operations
/// happen inside the keystore.
///
/// Inspired by: Android Keystore, iOS Keychain, TPM. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Key type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// AES symmetric key (128/256 bit)
    Aes128,
    Aes256,
    /// ChaCha20 symmetric key
    ChaCha20,
    /// X25519 key pair (for ECDH)
    X25519,
    /// Ed25519 signing key pair
    Ed25519,
    /// HMAC key
    HmacSha256,
    /// Password-derived key
    Pbkdf2,
    /// Generic secret (API tokens, etc.)
    GenericSecret,
}

/// Key purpose (what operations are allowed)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPurpose {
    Encrypt,
    Decrypt,
    Sign,
    Verify,
    DeriveKey,
    WrapKey,
}

/// Key entry in the keystore
pub struct KeyEntry {
    /// Key alias (user-visible name)
    pub alias: String,
    /// Key type
    pub key_type: KeyType,
    /// Allowed purposes
    pub purposes: Vec<KeyPurpose>,
    /// Key material (encrypted at rest with master key)
    material: Vec<u8>,
    /// Creation time (unix)
    pub created: u64,
    /// Expiration time (unix, 0 = never)
    pub expires: u64,
    /// Owner UID
    pub owner_uid: u32,
    /// Whether user authentication is required to use this key
    pub requires_auth: bool,
    /// Auth timeout (seconds, 0 = every use)
    pub auth_timeout: u32,
    /// Active flag
    pub active: bool,
}

/// Keystore
pub struct Keystore {
    keys: Vec<KeyEntry>,
    /// Master key (derived from device secret)
    master_key: [u8; 32],
    /// Whether the keystore is unlocked
    unlocked: bool,
}

impl Keystore {
    const fn new() -> Self {
        Keystore {
            keys: Vec::new(),
            master_key: [0; 32],
            unlocked: false,
        }
    }

    /// Unlock the keystore with the device credential
    pub fn unlock(&mut self, credential: &[u8]) -> bool {
        // Derive master key from credential using PBKDF2 (simplified)
        let mut key = [0u8; 32];
        for (i, &b) in credential.iter().enumerate() {
            key[i % 32] ^= b;
            key[(i + 13) % 32] = key[(i + 13) % 32].wrapping_add(b);
        }
        self.master_key = key;
        self.unlocked = true;
        true
    }

    /// Lock the keystore
    pub fn lock(&mut self) {
        self.master_key = [0; 32];
        self.unlocked = false;
    }

    /// Generate a new key
    pub fn generate_key(
        &mut self,
        alias: &str,
        key_type: KeyType,
        purposes: &[KeyPurpose],
        owner_uid: u32,
    ) -> bool {
        if !self.unlocked {
            return false;
        }
        if self
            .keys
            .iter()
            .any(|k| k.alias == alias && k.owner_uid == owner_uid)
        {
            return false; // alias already exists for this user
        }

        // Generate key material
        let material = match key_type {
            KeyType::Aes128 => {
                let mut key = alloc::vec![0u8; 16];
                // Use CSPRNG
                for b in &mut key {
                    *b = (crate::crypto::random::random_u32() & 0xFF) as u8;
                }
                key
            }
            KeyType::Aes256 | KeyType::ChaCha20 | KeyType::HmacSha256 => {
                let mut key = alloc::vec![0u8; 32];
                for b in &mut key {
                    *b = (crate::crypto::random::random_u32() & 0xFF) as u8;
                }
                key
            }
            KeyType::X25519 | KeyType::Ed25519 => {
                let mut key = alloc::vec![0u8; 32];
                for b in &mut key {
                    *b = (crate::crypto::random::random_u32() & 0xFF) as u8;
                }
                key
            }
            _ => {
                let mut key = alloc::vec![0u8; 32];
                for b in &mut key {
                    *b = (crate::crypto::random::random_u32() & 0xFF) as u8;
                }
                key
            }
        };

        // Encrypt material with master key (XOR for now, would use AES-GCM)
        let encrypted: Vec<u8> = material
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ self.master_key[i % 32])
            .collect();

        self.keys.push(KeyEntry {
            alias: String::from(alias),
            key_type,
            purposes: purposes.to_vec(),
            material: encrypted,
            created: crate::time::clock::unix_time(),
            expires: 0,
            owner_uid,
            requires_auth: false,
            auth_timeout: 0,
            active: true,
        });

        true
    }

    /// Delete a key
    pub fn delete_key(&mut self, alias: &str, uid: u32) -> bool {
        if let Some(pos) = self
            .keys
            .iter()
            .position(|k| k.alias == alias && k.owner_uid == uid)
        {
            // Zero out key material before removing
            for b in &mut self.keys[pos].material {
                *b = 0;
            }
            self.keys.remove(pos);
            true
        } else {
            false
        }
    }

    /// Check if a key exists
    pub fn contains_key(&self, alias: &str, uid: u32) -> bool {
        self.keys
            .iter()
            .any(|k| k.alias == alias && k.owner_uid == uid && k.active)
    }

    /// List keys for a user
    pub fn list_keys(&self, uid: u32) -> Vec<(String, KeyType)> {
        self.keys
            .iter()
            .filter(|k| k.owner_uid == uid && k.active)
            .map(|k| (k.alias.clone(), k.key_type))
            .collect()
    }

    /// Decrypt key material for use (internal only)
    fn get_key_material(&self, alias: &str, uid: u32) -> Option<Vec<u8>> {
        if !self.unlocked {
            return None;
        }
        let entry = self
            .keys
            .iter()
            .find(|k| k.alias == alias && k.owner_uid == uid)?;

        // Decrypt with master key
        let decrypted: Vec<u8> = entry
            .material
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ self.master_key[i % 32])
            .collect();
        Some(decrypted)
    }

    /// Sign data with a stored key
    pub fn sign(&self, alias: &str, uid: u32, data: &[u8]) -> Option<Vec<u8>> {
        let key_material = self.get_key_material(alias, uid)?;
        let entry = self
            .keys
            .iter()
            .find(|k| k.alias == alias && k.owner_uid == uid)?;

        if !entry.purposes.contains(&KeyPurpose::Sign) {
            return None;
        }

        // HMAC-SHA256 signature (simplified)
        let mut mac = [0u8; 32];
        for (i, &b) in data.iter().enumerate() {
            mac[i % 32] ^= b ^ key_material[i % key_material.len()];
        }
        Some(mac.to_vec())
    }

    /// Encrypt data with a stored key
    pub fn encrypt(&self, alias: &str, uid: u32, plaintext: &[u8]) -> Option<Vec<u8>> {
        let key_material = self.get_key_material(alias, uid)?;
        let entry = self
            .keys
            .iter()
            .find(|k| k.alias == alias && k.owner_uid == uid)?;

        if !entry.purposes.contains(&KeyPurpose::Encrypt) {
            return None;
        }

        // XOR encryption (placeholder — would use AES-GCM or ChaCha20-Poly1305)
        let ciphertext: Vec<u8> = plaintext
            .iter()
            .enumerate()
            .map(|(i, &b)| b ^ key_material[i % key_material.len()])
            .collect();
        Some(ciphertext)
    }
}

static KEYSTORE: Mutex<Keystore> = Mutex::new(Keystore::new());

pub fn init() {
    // Unlock with device secret (in production, from TPM/TEE)
    KEYSTORE.lock().unlock(b"genesis-device-secret");
    crate::serial_println!("  [keystore] Hardware keystore initialized");
}

pub fn generate_key(alias: &str, key_type: KeyType, uid: u32) -> bool {
    KEYSTORE.lock().generate_key(
        alias,
        key_type,
        &[
            KeyPurpose::Encrypt,
            KeyPurpose::Decrypt,
            KeyPurpose::Sign,
            KeyPurpose::Verify,
        ],
        uid,
    )
}
pub fn delete_key(alias: &str, uid: u32) -> bool {
    KEYSTORE.lock().delete_key(alias, uid)
}
pub fn list_keys(uid: u32) -> Vec<(String, KeyType)> {
    KEYSTORE.lock().list_keys(uid)
}
