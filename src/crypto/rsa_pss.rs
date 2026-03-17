/// rsa_pss — RSA-PSS signature encoding/verification (RFC 8017 §9.1)
///
/// Implements PSS-EMSA (Encoding Method for Signatures with Appendix):
///   - Hash: SHA-256 (32-byte output)
///   - MGF1: SHA-256 based mask generation (from rsa_oaep module)
///   - Salt length: 32 bytes (equal to hash output length — recommended)
///   - Key size: 2048 bits (256-byte modulus) assumed
///
/// This module provides the PSS encoding/padding layer only.
/// The raw RSA modular exponentiation must be provided externally.
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;

// Re-use MGF1 from rsa_oaep module.
use super::rsa_oaep::oaep_mgf1_sha256;

// Re-use SHA-256 from sha256 module.
use super::sha256::hash as sha256;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const PSS_HASH_LEN: usize = 32; // SHA-256 output
pub const PSS_SALT_LEN: usize = 32; // sLen = hLen (recommended)
pub const PSS_EMLEN: usize = 256; // EM length for 2048-bit key (ceil(2047/8)=256, but standard is 256)
                                  // For a 2048-bit key, emBits = 2047, emLen = ceil(2047/8) = 256, top bit masked.
pub const PSS_EM_BITS: usize = 2047;

// ---------------------------------------------------------------------------
// PSS-EMSA Encoding
// ---------------------------------------------------------------------------

/// PSS-EMSA encoding.
///
/// `m_hash`   — SHA-256 hash of the message (32 bytes)
/// `salt`     — random salt (32 bytes)
/// `em`       — output encoded message (PSS_EMLEN = 256 bytes)
///
/// Returns true on success.
pub fn pss_encode(
    m_hash: &[u8; PSS_HASH_LEN],
    salt: &[u8; PSS_SALT_LEN],
    em: &mut [u8; PSS_EMLEN],
) -> bool {
    // Step 1–2: check hash and em length are compatible — trivially true here.
    // Step 3: emLen = 256, hLen = 32, sLen = 32 → padding_len = 256 - 32 - 32 - 2 = 190
    let padding_len = PSS_EMLEN - PSS_HASH_LEN - PSS_SALT_LEN - 2;

    // Step 4: M' = 0x00×8 ‖ mHash ‖ salt
    let mut m_prime = [0u8; 8 + PSS_HASH_LEN + PSS_SALT_LEN];
    // first 8 bytes are zero (already)
    let mut i = 0usize;
    while i < PSS_HASH_LEN {
        m_prime[8 + i] = m_hash[i];
        i = i.saturating_add(1);
    }
    i = 0;
    while i < PSS_SALT_LEN {
        m_prime[8 + PSS_HASH_LEN + i] = salt[i];
        i = i.saturating_add(1);
    }

    // Step 5: H = SHA-256(M')
    let h = sha256(&m_prime);

    // Step 6: DB = PS ‖ 0x01 ‖ salt
    //   PS = padding_len zero bytes
    let mut db = [0u8; PSS_EMLEN]; // we'll use first (padding_len+1+sLen) bytes
                                   // PS is already zero
    db[padding_len] = 0x01;
    i = 0;
    while i < PSS_SALT_LEN {
        db[padding_len + 1 + i] = salt[i];
        i = i.saturating_add(1);
    }
    let db_len = padding_len + 1 + PSS_SALT_LEN; // = 256 - 32 - 2 = 222... wait let me recalculate
                                                 // padding_len = 256 - 32 - 32 - 2 = 190. db_len = 190 + 1 + 32 = 223. em = 256. h=32, last byte=0xBC.

    // Step 7: dbMask = MGF1(H, db_len)
    let mut db_mask = [0u8; PSS_EMLEN];
    oaep_mgf1_sha256(&h, &mut db_mask[..db_len]);

    // Step 8: maskedDB = DB XOR dbMask
    let mut masked_db = [0u8; PSS_EMLEN];
    i = 0;
    while i < db_len {
        masked_db[i] = db[i] ^ db_mask[i];
        i = i.saturating_add(1);
    }

    // Step 9: set the leftmost bits of maskedDB to zero
    // emBits = 2047, so top bit of first byte is masked (8*256 - 2047 = 1 bit)
    masked_db[0] &= 0x7F;

    // Step 10: EM = maskedDB ‖ H ‖ 0xBC
    i = 0;
    while i < db_len {
        em[i] = masked_db[i];
        i = i.saturating_add(1);
    }
    i = 0;
    while i < PSS_HASH_LEN {
        em[db_len + i] = h[i];
        i = i.saturating_add(1);
    }
    em[PSS_EMLEN - 1] = 0xBC;

    true
}

// ---------------------------------------------------------------------------
// PSS-EMSA Verification
// ---------------------------------------------------------------------------

/// PSS-EMSA verification.
///
/// `m_hash`   — SHA-256 hash of the message to verify
/// `em`       — encoded message recovered from RSA-public-key decryption (256 bytes)
///
/// Returns true if the signature is consistent (valid PSS encoding).
pub fn pss_verify(m_hash: &[u8; PSS_HASH_LEN], em: &[u8; PSS_EMLEN]) -> bool {
    // Step 3: check emLen >= hLen + sLen + 2
    // 256 >= 32 + 32 + 2 = 66 ✓

    // Step 4: check rightmost byte is 0xBC
    if em[PSS_EMLEN - 1] != 0xBC {
        return false;
    }

    let db_len = PSS_EMLEN - PSS_HASH_LEN - 1; // 256 - 32 - 1 = 223

    // Step 5: extract maskedDB and H
    let mut masked_db = [0u8; PSS_EMLEN];
    let mut h = [0u8; PSS_HASH_LEN];
    let mut i = 0usize;
    while i < db_len {
        masked_db[i] = em[i];
        i = i.saturating_add(1);
    }
    i = 0;
    while i < PSS_HASH_LEN {
        h[i] = em[db_len + i];
        i = i.saturating_add(1);
    }

    // Step 6: check top bit of maskedDB[0] is zero
    if masked_db[0] & 0x80 != 0 {
        return false;
    }

    // Step 7: dbMask = MGF1(H, db_len)
    let mut db_mask = [0u8; PSS_EMLEN];
    oaep_mgf1_sha256(&h, &mut db_mask[..db_len]);

    // Step 8: DB = maskedDB XOR dbMask
    let mut db = [0u8; PSS_EMLEN];
    i = 0;
    while i < db_len {
        db[i] = masked_db[i] ^ db_mask[i];
        i = i.saturating_add(1);
    }

    // Step 9: clear top bit of DB[0]
    db[0] &= 0x7F;

    // Step 10: check PS (leading zeros) then 0x01
    let padding_len = PSS_EMLEN - PSS_HASH_LEN - PSS_SALT_LEN - 2; // 190
    i = 0;
    while i < padding_len {
        if db[i] != 0x00 {
            return false;
        }
        i = i.saturating_add(1);
    }
    if db[padding_len] != 0x01 {
        return false;
    }

    // Step 11: extract salt (last sLen bytes of DB after the 0x01)
    let mut salt = [0u8; PSS_SALT_LEN];
    i = 0;
    while i < PSS_SALT_LEN {
        salt[i] = db[padding_len + 1 + i];
        i = i.saturating_add(1);
    }

    // Step 12: M' = 0x00×8 ‖ mHash ‖ salt
    let mut m_prime = [0u8; 8 + PSS_HASH_LEN + PSS_SALT_LEN];
    i = 0;
    while i < PSS_HASH_LEN {
        m_prime[8 + i] = m_hash[i];
        i = i.saturating_add(1);
    }
    i = 0;
    while i < PSS_SALT_LEN {
        m_prime[8 + PSS_HASH_LEN + i] = salt[i];
        i = i.saturating_add(1);
    }

    // Step 13: H' = SHA-256(M')
    let h_prime = sha256(&m_prime);

    // Step 14: verify H == H'
    i = 0;
    while i < PSS_HASH_LEN {
        if h[i] != h_prime[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Convenience: encode + verify round-trip (for unit testing)
// ---------------------------------------------------------------------------

/// Verify that PSS encode → verify round-trip succeeds for a given m_hash and salt.
/// Returns true if consistent.
#[cfg(test)]
pub fn pss_self_test(m_hash: &[u8; PSS_HASH_LEN], salt: &[u8; PSS_SALT_LEN]) -> bool {
    let mut em = [0u8; PSS_EMLEN];
    if !pss_encode(m_hash, salt, &mut em) {
        return false;
    }
    pss_verify(m_hash, &em)
}

pub fn init() {
    serial_println!("[rsa_pss] RSA-PSS (SHA-256, sLen=32, 2048-bit) initialized");
}
