use super::sha256;
/// RSA-OAEP (Optimal Asymmetric Encryption Padding)
///
/// Pure Rust, no-heap implementation of RSA-OAEP padding per PKCS#1 v2.2.
/// Used with 2048-bit RSA keys (256-byte moduli).
///
/// Provides OAEP padding/unpadding layer; RSA encryption/decryption
/// (modular exponentiation) is handled by caller.
///
/// Rules: no heap, no Vec/Box/String, no float casts, no panic.
use crate::serial_println;

pub const RSA_OAEP_MAX_KEY_BYTES: usize = 256; // 2048-bit key
pub const RSA_OAEP_LABEL_MAX: usize = 64;

/// Result type for RSA-OAEP operations
#[derive(Copy, Clone, PartialEq)]
pub enum OaepResult {
    Ok,
    MessageTooLong,
    DecryptionError,
    InvalidPadding,
}

/// MGF1 (Mask Generation Function) with SHA-256
///
/// Per RFC 3447 (PKCS #1 v2.1):
/// MGF1(seed, maskLen) generates a mask of maskLen bytes by
/// hashing seed || counter for increasing counter values.
///
/// # Arguments
/// - `seed`: seed bytes
/// - `seed_len`: length of seed
/// - `mask`: output mask buffer (256 bytes max, 16 iterations of SHA-256)
/// - `mask_len`: desired mask length
pub fn oaep_mgf1_sha256(seed: &[u8], mask: &mut [u8]) {
    let seed_len = seed.len();
    let mask_len = mask.len();
    let mut buf = [0u8; 256];
    oaep_mgf1_sha256_inner(seed, seed_len, &mut buf, mask_len);
    let copy_len = mask_len.min(256);
    mask[..copy_len].copy_from_slice(&buf[..copy_len]);
}

fn oaep_mgf1_sha256_inner(seed: &[u8], seed_len: usize, mask: &mut [u8; 256], mask_len: usize) {
    let mut offset = 0;
    let mut counter = 0u32;

    // Cap at 16 iterations (256 * 16 = 4096 bytes max)
    while offset < mask_len && counter < 16 {
        // Hash seed || counter_be32
        let mut hasher = sha256::Sha256::new();
        hasher.update(&seed[..seed_len]);
        let counter_be = counter.to_be_bytes();
        hasher.update(&counter_be);
        let digest = hasher.finalize();

        // Copy min(32, remaining) bytes to mask
        let remaining = mask_len - offset;
        let to_copy = if remaining < 32 { remaining } else { 32 };
        mask[offset..offset + to_copy].copy_from_slice(&digest[..to_copy]);

        offset += to_copy;
        counter = counter.wrapping_add(1);
    }
}

/// OAEP encoding per RFC 3447 Section 7.1.1
///
/// # Arguments
/// - `message`: message to encode
/// - `msg_len`: message length
/// - `label`: optional label for domain separation (may be empty)
/// - `label_len`: label length
/// - `seed`: 32-byte random seed
/// - `em`: output EM buffer (256 bytes for 2048-bit key)
///
/// # Returns
/// OaepResult::Ok on success, or error if message too long
pub fn oaep_encode(
    message: &[u8],
    msg_len: usize,
    label: &[u8],
    label_len: usize,
    seed: &[u8; 32],
    em: &mut [u8; 256],
) -> OaepResult {
    // k = 256 bytes (2048-bit key)
    // hlen = 32 bytes (SHA-256)
    // mlen <= k - 2*hlen - 2 = 256 - 64 - 2 = 190 bytes
    if msg_len > 190 {
        return OaepResult::MessageTooLong;
    }

    // Step 2a: lHash = SHA-256(label)
    let mut l_hash = [0u8; 32];
    let mut hasher = sha256::Sha256::new();
    hasher.update(&label[..label_len]);
    let hash_result = hasher.finalize();
    l_hash.copy_from_slice(&hash_result);

    // Step 2b: DB = lHash || PS || 0x01 || M
    // DB is 256 - 32 - 1 = 223 bytes
    let mut db = [0u8; 223];
    db[..32].copy_from_slice(&l_hash);
    // PS is all zeros (skip, already zero-initialized)
    // 0x01 separator at position 32 + (190 - msg_len) = 222 - msg_len
    let sep_pos = 32 + (190 - msg_len);
    db[sep_pos] = 0x01;
    // Copy message
    db[sep_pos + 1..sep_pos + 1 + msg_len].copy_from_slice(&message[..msg_len]);

    // Step 2c: Generate masking seed and DB mask
    let mut db_mask = [0u8; 256];
    oaep_mgf1_sha256_inner(&seed[..], 32, &mut db_mask, 223);

    // Step 2d: Mask DB
    let mut masked_db = [0u8; 223];
    let mut i = 0;
    while i < 223 {
        masked_db[i] = db[i] ^ db_mask[i];
        i += 1;
    }

    // Step 2e: Generate seed mask
    let mut seed_mask = [0u8; 256];
    oaep_mgf1_sha256_inner(&masked_db[..], 223, &mut seed_mask, 32);

    // Step 2f: Mask seed
    let mut masked_seed = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        masked_seed[i] = seed[i] ^ seed_mask[i];
        i += 1;
    }

    // Step 3: Construct EM = 0x00 || maskedSeed || maskedDB
    em[0] = 0x00;
    em[1..33].copy_from_slice(&masked_seed);
    em[33..256].copy_from_slice(&masked_db);

    OaepResult::Ok
}

/// OAEP decoding per RFC 3447 Section 7.1.2
///
/// # Arguments
/// - `em`: encoded message (256 bytes for 2048-bit key)
/// - `label`: optional label (must match encoding label)
/// - `label_len`: label length
/// - `message`: output message buffer (190 bytes max)
/// - `msg_len`: output for message length
///
/// # Returns
/// OaepResult::Ok with msg_len set, or error
pub fn oaep_decode(
    em: &[u8; 256],
    label: &[u8],
    label_len: usize,
    message: &mut [u8; 190],
    msg_len: &mut usize,
) -> OaepResult {
    // Step 1: Verify leading zero octet
    if em[0] != 0x00 {
        return OaepResult::InvalidPadding;
    }

    // Step 2a: Extract maskedSeed and maskedDB
    let masked_seed = &em[1..33];
    let masked_db = &em[33..256];

    // Step 2b: Compute lHash
    let mut l_hash = [0u8; 32];
    let mut hasher = sha256::Sha256::new();
    hasher.update(&label[..label_len]);
    let hash_result = hasher.finalize();
    l_hash.copy_from_slice(&hash_result);

    // Step 2c: Generate seed mask and unmask seed
    let mut seed_mask = [0u8; 256];
    oaep_mgf1_sha256_inner(masked_db, 223, &mut seed_mask, 32);

    let mut seed = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        seed[i] = masked_seed[i] ^ seed_mask[i];
        i += 1;
    }

    // Step 2d: Generate DB mask and unmask DB
    let mut db_mask = [0u8; 256];
    oaep_mgf1_sha256_inner(&seed, 32, &mut db_mask, 223);

    let mut db = [0u8; 223];
    let mut i = 0;
    while i < 223 {
        db[i] = masked_db[i] ^ db_mask[i];
        i += 1;
    }

    // Step 3: Verify lHash
    let mut lhash_match: u8 = 1;
    let mut i = 0;
    while i < 32 {
        if db[i] != l_hash[i] {
            lhash_match = 0;
        }
        i += 1;
    }

    if lhash_match == 0 {
        return OaepResult::InvalidPadding;
    }

    // Step 4: Find 0x01 separator in DB
    // DB[32..222] is PS (all zeros), DB[222] should be 0x01
    let mut sep_pos = 0usize;
    let mut found = false;
    let mut i = 32;
    while i < 223 {
        if db[i] == 0x01 && !found {
            sep_pos = i;
            found = true;
        }
        i += 1;
    }

    if !found {
        return OaepResult::InvalidPadding;
    }

    // Extract message (after 0x01 separator)
    *msg_len = 223 - sep_pos - 1;
    if *msg_len > 190 {
        return OaepResult::InvalidPadding;
    }

    message[..*msg_len].copy_from_slice(&db[sep_pos + 1..sep_pos + 1 + *msg_len]);

    OaepResult::Ok
}

/// Self-test: encode then decode a known message
pub fn init() {
    // Test vector: "genesis-kernel!!" (16 bytes)
    let message = b"genesis-kernel!!";
    let label = b"";
    let seed = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];

    let mut em = [0u8; 256];
    let result = oaep_encode(message, message.len(), label, 0, &seed, &mut em);

    if result != OaepResult::Ok {
        serial_println!("    [rsa_oaep] Encode failed");
        return;
    }

    let mut decoded = [0u8; 190];
    let mut decoded_len = 0usize;
    let result = oaep_decode(&em, label, 0, &mut decoded, &mut decoded_len);

    // Verify round-trip
    let mut success = false;
    if result == OaepResult::Ok && decoded_len == message.len() {
        let mut match_all = true;
        let mut i = 0;
        while i < message.len() {
            if decoded[i] != message[i] {
                match_all = false;
            }
            i += 1;
        }
        if match_all {
            success = true;
        }
    }

    if success {
        serial_println!("    [rsa_oaep] RSA-OAEP padding initialized");
    } else {
        serial_println!("    [rsa_oaep] Self-test FAILED");
    }
}
