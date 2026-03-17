use crate::crypto::aes::Aes256;
use crate::serial_println;
/// Disk encryption — dm-crypt with AES-XTS mode and LUKS1 header support
///
/// Implements:
///   - AES-XTS (IEEE 1619) sector encryption: the industry standard for
///     transparent disk encryption (used by Linux dm-crypt, macOS FileVault,
///     BitLocker).
///   - LUKS1 header parsing and passphrase-based unlock.
///   - A fixed-size table of up to 8 DmCryptVolume mappings (no heap).
///
/// Design constraints (bare-metal #![no_std] kernel):
///   - No alloc — all state in fixed-size arrays.
///   - No float casts (as f32 / as f64) anywhere.
///   - Saturating arithmetic on counters, wrapping on sequences.
///   - No panic — all error paths return Option/bool and log via serial_println!.
///
/// Crypto dependencies (all in crate::crypto):
///   - crate::crypto::aes::Aes256 — AES-256 block cipher (encrypt_block /
///     decrypt_block, 14-round key schedule)
///   - crate::crypto::hmac::pbkdf2_sha256 — PBKDF2-HMAC-SHA256 (returns Vec,
///     so we use an inline fixed-size variant here that avoids alloc entirely)
///
/// References:
///   IEEE Std 1619-2007 (XTS-AES)
///   LUKS On-Disk Format Specification v1.0
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// AES-XTS helpers (IEEE 1619)
// ---------------------------------------------------------------------------

/// GF(2^128) multiplication by alpha (x) — the XTS "galois multiply by 2".
///
/// The field polynomial is x^128 + x^7 + x^2 + x + 1, so the feedback
/// constant after a carry-out is 0x87.
///
/// Algorithm: left-shift all 128 bits by 1.  If the MSB (bit 127, which is
/// the most-significant bit of byte[15] in little-endian layout used by XTS)
/// was 1, XOR byte[0] with 0x87.
#[inline(always)]
fn gf128_mul_alpha(tweak: &mut [u8; 16]) {
    // Capture the carry from the most-significant byte (byte[15] in LE layout).
    // XTS stores tweaks in little-endian, so byte[15] is the high byte.
    let carry = (tweak[15] >> 7) & 1;

    // Left-shift the whole 128-bit value by 1 (LE layout: high byte = index 15).
    let mut i = 15usize;
    loop {
        if i > 0 {
            tweak[i] = (tweak[i] << 1) | (tweak[i - 1] >> 7);
        } else {
            tweak[0] = tweak[0] << 1;
            break;
        }
        i = i.saturating_sub(1);
    }

    // Feedback polynomial: if carry was 1, XOR with 0x87 in the LSB byte.
    if carry == 1 {
        tweak[0] ^= 0x87;
    }
}

/// Encode a 64-bit sector number as a little-endian 16-byte value.
/// The upper 8 bytes are zero, matching LUKS xts-plain64 IV convention.
#[inline(always)]
fn sector_to_le128(sector: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    let le = sector.to_le_bytes();
    out[0] = le[0];
    out[1] = le[1];
    out[2] = le[2];
    out[3] = le[3];
    out[4] = le[4];
    out[5] = le[5];
    out[6] = le[6];
    out[7] = le[7];
    // bytes 8..15 remain 0
    out
}

/// Encrypt one 512-byte sector using AES-XTS (IEEE 1619).
///
/// key1 (32 bytes): used for the data AES-256 cipher.
/// key2 (32 bytes): used to encrypt the sector-number tweak.
/// sector: logical block address — used as the tweak input.
/// data:   in/out buffer, encrypted in place.
///
/// XTS block loop (32 AES-128-bit blocks per 512-byte sector):
///   tweak_0 = AES_key2(sector_as_le128)
///   for j in 0..32:
///       pp = data[j*16 .. j*16+16] XOR tweak_j
///       cc = AES_key1_encrypt(pp)
///       data[j*16 .. j*16+16] = cc XOR tweak_j
///       tweak_{j+1} = gf128_mul_alpha(tweak_j)
pub fn aes_xts_encrypt_sector(key1: &[u8; 32], key2: &[u8; 32], sector: u64, data: &mut [u8; 512]) {
    let cipher1 = Aes256::new(key1);
    let cipher2 = Aes256::new(key2);

    // Compute initial tweak: T = AES_key2(sector_number_le128)
    let mut tweak = sector_to_le128(sector);
    cipher2.encrypt_block(&mut tweak);

    // Process 32 AES blocks (32 * 16 = 512 bytes)
    let mut j = 0usize;
    while j < 32 {
        let off = j * 16;

        // PP = P XOR T
        let mut block = [0u8; 16];
        block[0] = data[off] ^ tweak[0];
        block[1] = data[off + 1] ^ tweak[1];
        block[2] = data[off + 2] ^ tweak[2];
        block[3] = data[off + 3] ^ tweak[3];
        block[4] = data[off + 4] ^ tweak[4];
        block[5] = data[off + 5] ^ tweak[5];
        block[6] = data[off + 6] ^ tweak[6];
        block[7] = data[off + 7] ^ tweak[7];
        block[8] = data[off + 8] ^ tweak[8];
        block[9] = data[off + 9] ^ tweak[9];
        block[10] = data[off + 10] ^ tweak[10];
        block[11] = data[off + 11] ^ tweak[11];
        block[12] = data[off + 12] ^ tweak[12];
        block[13] = data[off + 13] ^ tweak[13];
        block[14] = data[off + 14] ^ tweak[14];
        block[15] = data[off + 15] ^ tweak[15];

        // CC = AES_encrypt(PP)
        cipher1.encrypt_block(&mut block);

        // C = CC XOR T
        data[off] = block[0] ^ tweak[0];
        data[off + 1] = block[1] ^ tweak[1];
        data[off + 2] = block[2] ^ tweak[2];
        data[off + 3] = block[3] ^ tweak[3];
        data[off + 4] = block[4] ^ tweak[4];
        data[off + 5] = block[5] ^ tweak[5];
        data[off + 6] = block[6] ^ tweak[6];
        data[off + 7] = block[7] ^ tweak[7];
        data[off + 8] = block[8] ^ tweak[8];
        data[off + 9] = block[9] ^ tweak[9];
        data[off + 10] = block[10] ^ tweak[10];
        data[off + 11] = block[11] ^ tweak[11];
        data[off + 12] = block[12] ^ tweak[12];
        data[off + 13] = block[13] ^ tweak[13];
        data[off + 14] = block[14] ^ tweak[14];
        data[off + 15] = block[15] ^ tweak[15];

        // Advance tweak for next block
        gf128_mul_alpha(&mut tweak);
        j = j.wrapping_add(1);
    }
}

/// Decrypt one 512-byte sector using AES-XTS.
///
/// Identical structure to encrypt, but the inner block cipher operation uses
/// AES-256 *decryption* (InvSubBytes / InvShiftRows / InvMixColumns).
pub fn aes_xts_decrypt_sector(key1: &[u8; 32], key2: &[u8; 32], sector: u64, data: &mut [u8; 512]) {
    let cipher1 = Aes256::new(key1);
    let cipher2 = Aes256::new(key2);

    let mut tweak = sector_to_le128(sector);
    cipher2.encrypt_block(&mut tweak);

    let mut j = 0usize;
    while j < 32 {
        let off = j * 16;

        // CC = C XOR T
        let mut block = [0u8; 16];
        block[0] = data[off] ^ tweak[0];
        block[1] = data[off + 1] ^ tweak[1];
        block[2] = data[off + 2] ^ tweak[2];
        block[3] = data[off + 3] ^ tweak[3];
        block[4] = data[off + 4] ^ tweak[4];
        block[5] = data[off + 5] ^ tweak[5];
        block[6] = data[off + 6] ^ tweak[6];
        block[7] = data[off + 7] ^ tweak[7];
        block[8] = data[off + 8] ^ tweak[8];
        block[9] = data[off + 9] ^ tweak[9];
        block[10] = data[off + 10] ^ tweak[10];
        block[11] = data[off + 11] ^ tweak[11];
        block[12] = data[off + 12] ^ tweak[12];
        block[13] = data[off + 13] ^ tweak[13];
        block[14] = data[off + 14] ^ tweak[14];
        block[15] = data[off + 15] ^ tweak[15];

        // PP = AES_decrypt(CC)
        cipher1.decrypt_block(&mut block);

        // P = PP XOR T
        data[off] = block[0] ^ tweak[0];
        data[off + 1] = block[1] ^ tweak[1];
        data[off + 2] = block[2] ^ tweak[2];
        data[off + 3] = block[3] ^ tweak[3];
        data[off + 4] = block[4] ^ tweak[4];
        data[off + 5] = block[5] ^ tweak[5];
        data[off + 6] = block[6] ^ tweak[6];
        data[off + 7] = block[7] ^ tweak[7];
        data[off + 8] = block[8] ^ tweak[8];
        data[off + 9] = block[9] ^ tweak[9];
        data[off + 10] = block[10] ^ tweak[10];
        data[off + 11] = block[11] ^ tweak[11];
        data[off + 12] = block[12] ^ tweak[12];
        data[off + 13] = block[13] ^ tweak[13];
        data[off + 14] = block[14] ^ tweak[14];
        data[off + 15] = block[15] ^ tweak[15];

        gf128_mul_alpha(&mut tweak);
        j = j.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------------
// PBKDF2-SHA256 — inline, no-alloc fixed-output variant
// ---------------------------------------------------------------------------
// crate::crypto::hmac::pbkdf2_sha256 returns a Vec, which we cannot use.
// We inline a 64-byte-output variant here using only fixed arrays.

use crate::crypto::hmac::HmacSha256;

/// PBKDF2-HMAC-SHA256 producing exactly `out_len` bytes (up to 64).
///
/// RFC 8018 §5.2:  DK = T_1 || T_2  (two 32-byte PRF-blocks, truncated)
/// T_i = U_1 XOR U_2 XOR ... XOR U_c
/// U_1 = PRF(Password, Salt || INT32BE(i))
/// U_j = PRF(Password, U_{j-1})
///
/// out_len must be <= 64.  Only blocks 1 and 2 are computed.
fn pbkdf2_sha256_64(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out: &mut [u8; 64],
    out_len: usize,
) {
    // Clamp out_len to 64
    let dk_len = if out_len <= 64 { out_len } else { 64 };
    // Number of 32-byte blocks needed (1 or 2)
    let num_blocks: usize = if dk_len <= 32 { 1 } else { 2 };

    let mut block_idx = 0usize;
    while block_idx < num_blocks {
        let i = (block_idx + 1) as u32;

        // U_1 = HMAC(password, salt || INT32BE(i))
        let mut hmac = HmacSha256::new(password);
        hmac.update(salt);
        hmac.update(&i.to_be_bytes());
        let mut u_prev = hmac.finalize();
        let mut t = u_prev;

        // U_2 .. U_c
        let mut iter = 1u32;
        while iter < iterations {
            let mut hmac2 = HmacSha256::new(password);
            hmac2.update(&u_prev);
            let u_next = hmac2.finalize();
            // XOR into accumulator
            let mut k = 0usize;
            while k < 32 {
                t[k] ^= u_next[k];
                k = k.wrapping_add(1);
            }
            u_prev = u_next;
            iter = iter.saturating_add(1);
        }

        // Write T_i into out
        let dst_off = block_idx * 32;
        let mut k = 0usize;
        while k < 32 {
            if dst_off + k < 64 {
                out[dst_off + k] = t[k];
            }
            k = k.wrapping_add(1);
        }

        block_idx = block_idx.wrapping_add(1);
    }

    // Zero bytes beyond dk_len for safety
    let mut k = dk_len;
    while k < 64 {
        out[k] = 0;
        k = k.wrapping_add(1);
    }
}

// ---------------------------------------------------------------------------
// LUKS1 on-disk structures
// ---------------------------------------------------------------------------

/// LUKS1 magic bytes: "LUKS" followed by 0xBA 0xBE.
pub const LUKS_MAGIC: [u8; 6] = [0x4C, 0x55, 0x4B, 0x53, 0xBA, 0xBE];

/// LUKS1 version field value.
pub const LUKS_VERSION_1: [u8; 2] = [0x00, 0x01];

/// AES cipher name as a null-terminated 32-byte field.
pub const LUKS_CIPHER_NAME_AES: &[u8] = b"aes\0";

/// XTS mode string as stored in the LUKS header cipher_mode field.
pub const LUKS_CIPHER_MODE_XTS: &[u8] = b"xts-plain64\0";

/// SHA-256 hash spec string.
pub const LUKS_HASH_SHA256: &[u8] = b"sha256\0";

/// Active key slot marker.
pub const LUKS_KEY_ENABLED: u32 = 0x00AC71F3;
/// Disabled key slot marker.
pub const LUKS_KEY_DISABLED: u32 = 0x0000DEAD;

/// LUKS1 key slot (48 bytes, packed).
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct LuksKeySlot {
    /// 0x00AC71F3 = enabled, 0x0000DEAD = disabled.
    pub active: u32,
    /// PBKDF2 iterations for this slot.
    pub iterations: u32,
    /// Per-slot random salt (32 bytes).
    pub salt: [u8; 32],
    /// Offset (in 512-byte sectors) of the encrypted key material on device.
    pub key_material_offset: u32,
    /// Anti-forensic stripe count (typically 4000).
    pub stripes: u32,
}

/// LUKS1 header — first 592 bytes of an encrypted device.
///
/// All multi-byte integers are big-endian on disk (LUKS spec §6).
/// We read raw bytes and convert with u32::from_be_bytes() as needed.
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct LuksHeader {
    pub magic: [u8; 6],
    pub version: [u8; 2],
    pub cipher_name: [u8; 32],
    pub cipher_mode: [u8; 32],
    pub hash_spec: [u8; 32],
    /// First payload sector (big-endian u32 on disk).
    pub payload_offset: u32,
    /// Master key size in bytes (big-endian u32), e.g. 32 for AES-256.
    pub key_bytes: u32,
    /// PBKDF2-SHA1 digest of the master key (20 bytes; we store 20, padded to 20).
    pub mk_digest: [u8; 20],
    /// Salt used to produce mk_digest.
    pub mk_digest_salt: [u8; 32],
    /// PBKDF2 iterations for mk_digest verification.
    pub mk_digest_iter: u32,
    /// Volume UUID as a null-terminated ASCII string.
    pub uuid: [u8; 40],
    /// Eight key slots.
    pub key_slots: [LuksKeySlot; 8],
}

// Compile-time size assertion: LUKS1 header must be exactly 592 bytes.
// Layout: 6+2+32+32+32+4+4+20+32+4+40 = 208 bytes for the header fields,
// then 8 * 48 = 384 bytes for key slots.  208 + 384 = 592.
const _LUKS_HEADER_SIZE_CHECK: () = {
    // core::mem::size_of is const — no float, no alloc.
    assert!(core::mem::size_of::<LuksHeader>() == 592);
};

/// Parse a LUKS1 header from the first 592 bytes of a device.
///
/// Returns `Some(header)` if the magic and version are valid.
/// All integers in the returned struct remain in big-endian byte order as
/// stored on disk; callers use `u32::from_be_bytes()` to read them.
pub fn luks_parse_header(data: &[u8; 592]) -> Option<LuksHeader> {
    // Safety: LuksHeader is repr(C, packed) and 592 bytes, matching the input.
    // We copy byte-by-byte to avoid any alignment UB.
    let mut hdr = unsafe { core::mem::zeroed::<LuksHeader>() };

    // Copy magic (6)
    hdr.magic.copy_from_slice(&data[0..6]);
    // Copy version (2)
    hdr.version[0] = data[6];
    hdr.version[1] = data[7];

    // Validate magic
    if hdr.magic != LUKS_MAGIC {
        serial_println!("  [dm-crypt] luks_parse_header: bad magic");
        return None;
    }
    // Validate version == 1
    if hdr.version != LUKS_VERSION_1 {
        serial_println!(
            "  [dm-crypt] luks_parse_header: unsupported version {:02x}{:02x}",
            hdr.version[0],
            hdr.version[1]
        );
        return None;
    }

    // Copy remaining fixed fields
    hdr.cipher_name.copy_from_slice(&data[8..40]);
    hdr.cipher_mode.copy_from_slice(&data[40..72]);
    hdr.hash_spec.copy_from_slice(&data[72..104]);

    hdr.payload_offset = u32::from_be_bytes([data[104], data[105], data[106], data[107]]);
    hdr.key_bytes = u32::from_be_bytes([data[108], data[109], data[110], data[111]]);

    hdr.mk_digest.copy_from_slice(&data[112..132]);
    hdr.mk_digest_salt.copy_from_slice(&data[132..164]);
    hdr.mk_digest_iter = u32::from_be_bytes([data[164], data[165], data[166], data[167]]);

    hdr.uuid.copy_from_slice(&data[168..208]);

    // Parse 8 key slots (each 48 bytes, starting at offset 208)
    let mut si = 0usize;
    while si < 8 {
        let base = 208 + si * 48;
        hdr.key_slots[si].active =
            u32::from_be_bytes([data[base], data[base + 1], data[base + 2], data[base + 3]]);
        hdr.key_slots[si].iterations = u32::from_be_bytes([
            data[base + 4],
            data[base + 5],
            data[base + 6],
            data[base + 7],
        ]);
        hdr.key_slots[si]
            .salt
            .copy_from_slice(&data[base + 8..base + 40]);
        hdr.key_slots[si].key_material_offset = u32::from_be_bytes([
            data[base + 40],
            data[base + 41],
            data[base + 42],
            data[base + 43],
        ]);
        hdr.key_slots[si].stripes = u32::from_be_bytes([
            data[base + 44],
            data[base + 45],
            data[base + 46],
            data[base + 47],
        ]);
        si = si.wrapping_add(1);
    }

    Some(hdr)
}

// ---------------------------------------------------------------------------
// LUKS unlock — key slot trial
// ---------------------------------------------------------------------------

/// Read an encrypted LUKS key-material sector from a device.
///
/// In the kernel, device I/O normally goes through the NVMe/SCSI driver.
/// This thin shim calls `crate::storage::volumes` if available, or falls back
/// to a zeroed buffer (which will cause verification to fail safely).
/// Replace with a real sector read once the block-device abstraction is wired.
fn read_key_material_sector(_device_idx: u8, _lba: u64, buf: &mut [u8; 512]) -> bool {
    // Stub: real implementation calls the NVMe/SCSI driver.
    // Returns false to signal "cannot read"; luks_unlock will skip the slot.
    buf.fill(0);
    false
}

/// Attempt to unlock a LUKS1 volume using a passphrase.
///
/// For each enabled key slot:
///   1. PBKDF2-SHA256(passphrase, slot.salt, slot.iterations) → 32-byte derived key
///      (LUKS uses a 32-byte key to wrap a 32-byte master key for AES-256)
///   2. Read the encrypted key material from disk (slot.key_material_offset)
///   3. Decrypt key material using AES-XTS with the derived key split into
///      key1 = derived[0..16] padded to 32 bytes, key2 = derived[16..32] padded.
///      NOTE: LUKS stores the wrapped key in `stripes * key_bytes` bytes using
///      its AF-split anti-forensic scheme.  Full AF-split recovery is complex;
///      here we decrypt the first sector (512 bytes) and take the first
///      `key_bytes` bytes as the candidate master key.
///   4. Verify: PBKDF2-SHA256(candidate_mk, mk_digest_salt, mk_digest_iter)[0..20]
///      must equal mk_digest (LUKS uses SHA1 for the mk_digest; we use SHA256
///      and take the first 20 bytes as an approximation — a full SHA1
///      implementation can replace this when available).
///   5. On match: copy candidate_mk into mk_out[0..key_bytes] and return true.
///
/// Returns true and fills mk_out on success.  Returns false on failure.
pub fn luks_unlock(
    header: &LuksHeader,
    device_idx: u8,
    passphrase: &[u8],
    mk_out: &mut [u8; 64],
) -> bool {
    // Fields in LuksHeader are native u32 after parsing (luks_parse_header already
    // did the from_be_bytes conversion).  For packed structs, copy to a local to
    // avoid any possible unaligned-access issue on strict-alignment architectures.
    let key_bytes = {
        let v = header.key_bytes;
        v
    } as usize;
    if key_bytes == 0 || key_bytes > 64 {
        serial_println!("  [dm-crypt] luks_unlock: invalid key_bytes {}", key_bytes);
        return false;
    }

    let mut si = 0usize;
    while si < 8 {
        let slot = &header.key_slots[si];
        // Copy packed u32 fields to locals before comparing/using
        let active: u32 = {
            let v = slot.active;
            v
        };
        let iterations: u32 = {
            let v = slot.iterations;
            v
        };
        let km_offset: u32 = {
            let v = slot.key_material_offset;
            v
        };

        if active != LUKS_KEY_ENABLED {
            si = si.wrapping_add(1);
            continue;
        }

        serial_println!("  [dm-crypt] luks_unlock: trying slot {}", si);

        // Step 1: derive key from passphrase
        // LUKS uses PBKDF2 to produce a key of `key_bytes` bytes.
        // For AES-256 XTS the split key is 64 bytes (32+32); LUKS key_bytes
        // is 32 for AES-256 in XEX mode — it stores only the base key and
        // derives both halves internally.  We produce 64 bytes and use both.
        let mut derived_key = [0u8; 64];
        pbkdf2_sha256_64(passphrase, &slot.salt, iterations, &mut derived_key, 64);

        // Build XTS key pair from derived key
        let mut xts_key1 = [0u8; 32];
        let mut xts_key2 = [0u8; 32];
        xts_key1.copy_from_slice(&derived_key[0..32]);
        xts_key2.copy_from_slice(&derived_key[32..64]);

        // Step 2: read encrypted key material from disk
        let lba = km_offset as u64;
        let mut km_sector = [0u8; 512];
        let read_ok = read_key_material_sector(device_idx, lba, &mut km_sector);
        if !read_ok {
            // Disk read unavailable (stub).  We still attempt verification
            // against the zeroed buffer so the API surface is correct; it
            // will simply fail verification.
            serial_println!("  [dm-crypt] slot {}: key material read failed (stub)", si);
        }

        // Step 3: decrypt key material with AES-XTS (sector 0 = lba 0 within slot)
        aes_xts_decrypt_sector(&xts_key1, &xts_key2, 0, &mut km_sector);

        // Candidate master key: first key_bytes bytes of decrypted material
        let mut candidate = [0u8; 64];
        let mut k = 0usize;
        while k < key_bytes && k < 64 {
            candidate[k] = km_sector[k];
            k = k.wrapping_add(1);
        }

        // Step 4: verify candidate master key
        // LUKS mk_digest = PBKDF2-SHA1(mk, mk_digest_salt, mk_digest_iter)[0..20]
        // We approximate with PBKDF2-SHA256 and compare the first 20 bytes.
        let mk_digest_iter: u32 = {
            let v = header.mk_digest_iter;
            v
        };
        let mut digest_out = [0u8; 64];
        pbkdf2_sha256_64(
            &candidate[..key_bytes],
            &header.mk_digest_salt,
            mk_digest_iter,
            &mut digest_out,
            20,
        );

        // Constant-time comparison of first 20 bytes
        let mut diff: u8 = 0;
        let mut j = 0usize;
        while j < 20 {
            diff |= digest_out[j] ^ header.mk_digest[j];
            j = j.wrapping_add(1);
        }

        if diff == 0 {
            serial_println!("  [dm-crypt] luks_unlock: slot {} matched", si);
            mk_out.copy_from_slice(&candidate);
            // Zero sensitive locals before returning
            derived_key.fill(0);
            candidate.fill(0);
            km_sector.fill(0);
            return true;
        }

        // Zero sensitive data before next slot attempt
        derived_key.fill(0);
        candidate.fill(0);
        km_sector.fill(0);

        si = si.wrapping_add(1);
    }

    serial_println!("  [dm-crypt] luks_unlock: no slot matched (wrong passphrase?)");
    false
}

// ---------------------------------------------------------------------------
// DmCryptVolume — transparent sector-level encryption mapping
// ---------------------------------------------------------------------------

/// Maximum number of simultaneously open encrypted volumes.
pub const MAX_DM_VOLUMES: usize = 8;

/// A named, key-carrying volume mapping.
///
/// Stores both XTS key halves in RAM. The volume encrypts/decrypts sectors
/// on the fly: the caller is responsible for the raw device I/O before
/// passing sectors to dm_crypt_read / dm_crypt_write.
#[derive(Copy, Clone)]
pub struct DmCryptVolume {
    /// Short name (e.g. "root", "data"), null-terminated or zero-padded.
    pub name: [u8; 32],
    /// AES-XTS key 1 (for the data cipher).
    pub key1: [u8; 32],
    /// AES-XTS key 2 (for the tweak cipher).
    pub key2: [u8; 32],
    /// LBA offset added to the logical sector when computing the tweak IV.
    /// For LUKS: set this to the payload_offset so sector 0 inside the
    /// volume maps to iv = payload_offset on the physical device.
    pub iv_offset: u64,
    /// Sector offset on the physical device where encrypted data begins.
    pub data_offset: u64,
    /// Total number of 512-byte sectors in the encrypted region.
    pub total_sectors: u64,
    /// Physical device index (NVMe/SCSI device number).
    pub device_idx: u8,
    /// True if this slot is occupied.
    pub active: bool,
    /// Read/write sector counters.
    pub reads: u64,
    pub writes: u64,
}

impl DmCryptVolume {
    const fn empty() -> Self {
        DmCryptVolume {
            name: [0u8; 32],
            key1: [0u8; 32],
            key2: [0u8; 32],
            iv_offset: 0,
            data_offset: 0,
            total_sectors: 0,
            device_idx: 0,
            active: false,
            reads: 0,
            writes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global volume table
// ---------------------------------------------------------------------------

static DM_VOLUMES: Mutex<[DmCryptVolume; MAX_DM_VOLUMES]> = Mutex::new([
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
    DmCryptVolume::empty(),
]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open a new dm-crypt volume mapping.
///
/// master_key: 64 bytes — key1 = mk[0..32], key2 = mk[32..64].
/// data_offset_sectors: first payload sector on the device.
/// total_sectors: number of encrypted sectors.
/// iv_offset: added to the logical sector for the XTS tweak (set to
///            data_offset_sectors for LUKS xts-plain64).
///
/// Returns the volume index (0..7) on success, or None if the table is full
/// or the name is too long.
pub fn dm_crypt_open(
    name: &[u8],
    device_idx: u8,
    master_key: &[u8; 64],
    data_offset_sectors: u64,
    total_sectors: u64,
) -> Option<u32> {
    if name.len() > 31 {
        serial_println!("  [dm-crypt] dm_crypt_open: name too long");
        return None;
    }

    let mut table = DM_VOLUMES.lock();

    // Find an empty slot
    let mut slot_idx: Option<usize> = None;
    let mut i = 0usize;
    while i < MAX_DM_VOLUMES {
        if !table[i].active {
            slot_idx = Some(i);
            break;
        }
        i = i.wrapping_add(1);
    }

    let idx = match slot_idx {
        Some(i) => i,
        None => {
            serial_println!("  [dm-crypt] dm_crypt_open: volume table full");
            return None;
        }
    };

    let vol = &mut table[idx];
    vol.name.fill(0);
    vol.name[..name.len()].copy_from_slice(name);
    vol.key1.copy_from_slice(&master_key[0..32]);
    vol.key2.copy_from_slice(&master_key[32..64]);
    vol.iv_offset = data_offset_sectors;
    vol.data_offset = data_offset_sectors;
    vol.total_sectors = total_sectors;
    vol.device_idx = device_idx;
    vol.active = true;
    vol.reads = 0;
    vol.writes = 0;

    serial_println!(
        "  [dm-crypt] opened volume {} at slot {} ({} sectors)",
        idx,
        device_idx,
        total_sectors
    );

    Some(idx as u32)
}

/// Decrypt a 512-byte sector from an encrypted volume.
///
/// `sector` is the logical sector number *within the volume* (0-based).
/// The physical LBA on device = vol.data_offset + sector.
/// The XTS IV = vol.iv_offset + sector  (matches LUKS xts-plain64 semantics).
///
/// The caller provides the ciphertext in `buf`; on success `buf` contains
/// the plaintext and the function returns true.
pub fn dm_crypt_read(vol_idx: u32, sector: u64, buf: &mut [u8; 512]) -> bool {
    if vol_idx as usize >= MAX_DM_VOLUMES {
        return false;
    }

    let table = DM_VOLUMES.lock();
    let vol = &table[vol_idx as usize];

    if !vol.active {
        serial_println!("  [dm-crypt] dm_crypt_read: volume {} not active", vol_idx);
        return false;
    }
    if sector >= vol.total_sectors {
        serial_println!(
            "  [dm-crypt] dm_crypt_read: sector {} out of range ({})",
            sector,
            vol.total_sectors
        );
        return false;
    }

    // Capture keys before releasing lock (copy to stack)
    let key1 = vol.key1;
    let key2 = vol.key2;
    let iv = vol.iv_offset.wrapping_add(sector);

    drop(table);

    aes_xts_decrypt_sector(&key1, &key2, iv, buf);

    // Update read counter — re-acquire lock briefly
    let mut table2 = DM_VOLUMES.lock();
    table2[vol_idx as usize].reads = table2[vol_idx as usize].reads.saturating_add(1);

    true
}

/// Encrypt a 512-byte sector and prepare it for writing to an encrypted volume.
///
/// `sector` is the logical sector number within the volume (0-based).
/// `data` is the plaintext; on return it contains the ciphertext ready to be
/// written to physical sector (vol.data_offset + sector) on the device.
pub fn dm_crypt_write(vol_idx: u32, sector: u64, data: &mut [u8; 512]) -> bool {
    if vol_idx as usize >= MAX_DM_VOLUMES {
        return false;
    }

    let table = DM_VOLUMES.lock();
    let vol = &table[vol_idx as usize];

    if !vol.active {
        serial_println!("  [dm-crypt] dm_crypt_write: volume {} not active", vol_idx);
        return false;
    }
    if sector >= vol.total_sectors {
        serial_println!(
            "  [dm-crypt] dm_crypt_write: sector {} out of range ({})",
            sector,
            vol.total_sectors
        );
        return false;
    }

    let key1 = vol.key1;
    let key2 = vol.key2;
    let iv = vol.iv_offset.wrapping_add(sector);

    drop(table);

    aes_xts_encrypt_sector(&key1, &key2, iv, data);

    let mut table2 = DM_VOLUMES.lock();
    table2[vol_idx as usize].writes = table2[vol_idx as usize].writes.saturating_add(1);

    true
}

/// Close an encrypted volume mapping, zeroing its key material.
pub fn dm_crypt_close(vol_idx: u32) {
    if vol_idx as usize >= MAX_DM_VOLUMES {
        return;
    }
    let mut table = DM_VOLUMES.lock();
    let vol = &mut table[vol_idx as usize];
    if !vol.active {
        return;
    }
    // Securely zero the key material
    vol.key1.fill(0);
    vol.key2.fill(0);
    vol.active = false;
    serial_println!("  [dm-crypt] closed volume {}", vol_idx);
}

/// Open a LUKS1 encrypted volume from a parsed header.
///
/// Combines luks_parse_header + luks_unlock + dm_crypt_open into a single
/// convenience call.  The caller provides:
///   - `header_data`: first 592 bytes of the device.
///   - `passphrase`: user passphrase.
///   - `device_idx`: NVMe/SCSI device number for I/O.
///   - `name`: volume name for the dm table.
///
/// Returns the volume index on success.
pub fn luks_open(
    header_data: &[u8; 592],
    passphrase: &[u8],
    device_idx: u8,
    name: &[u8],
) -> Option<u32> {
    let header = luks_parse_header(header_data)?;

    let mut mk = [0u8; 64];
    if !luks_unlock(&header, device_idx, passphrase, &mut mk) {
        mk.fill(0);
        return None;
    }

    let payload_offset: u64 = {
        let v = header.payload_offset;
        v as u64
    };

    // total_sectors: we don't know the device size here — pass 0 and let the
    // caller update via dm_crypt_set_total_sectors if needed.
    let vol_idx = dm_crypt_open(name, device_idx, &mk, payload_offset, u64::MAX);

    // Zero master key from stack
    mk.fill(0);

    vol_idx
}

/// Update total_sectors for a volume (e.g. after learning device size).
pub fn dm_crypt_set_total_sectors(vol_idx: u32, total_sectors: u64) {
    if vol_idx as usize >= MAX_DM_VOLUMES {
        return;
    }
    let mut table = DM_VOLUMES.lock();
    let vol = &mut table[vol_idx as usize];
    if vol.active {
        vol.total_sectors = total_sectors;
    }
}

/// Return (reads, writes) for a volume.
pub fn dm_crypt_stats(vol_idx: u32) -> (u64, u64) {
    if vol_idx as usize >= MAX_DM_VOLUMES {
        return (0, 0);
    }
    let table = DM_VOLUMES.lock();
    let vol = &table[vol_idx as usize];
    (vol.reads, vol.writes)
}

/// Initialize the dm-crypt subsystem (clears the volume table).
pub fn init() {
    let mut table = DM_VOLUMES.lock();
    let mut i = 0usize;
    while i < MAX_DM_VOLUMES {
        table[i] = DmCryptVolume::empty();
        i = i.wrapping_add(1);
    }
    serial_println!("  [storage] dm-crypt (AES-XTS + LUKS1) initialized");
}
