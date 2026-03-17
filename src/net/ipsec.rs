/// IPsec (ESP and AH) for Genesis — no-heap, no-float, no-panic
///
/// Implements IP Security Architecture (RFC 4301) with:
///   - ESP (Encapsulating Security Payload, RFC 4303): encryption + optional auth
///   - AH  (Authentication Header, RFC 4302): integrity + anti-replay, no encryption
///
/// Security Associations (SAs) are stored in a fixed-size static table.
/// A SA identifies a one-directional security relationship: each tunnel
/// direction has its own SA identified by SPI + destination IP.
///
/// Encryption: AES-CBC-128/256 (XOR stub — real AES would call crate::crypto::aes).
/// Authentication: HMAC-SHA-256 truncated to 12 bytes (ICV field).
///
/// Inspired by: Linux xfrm, BSD FAST_IPSEC, strongSwan. All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Protocol numbers (IANA assigned)
// ---------------------------------------------------------------------------

/// IP protocol number for ESP (Encapsulating Security Payload)
pub const IPPROTO_ESP: u8 = 50;

/// IP protocol number for AH (Authentication Header)
pub const IPPROTO_AH: u8 = 51;

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

/// Maximum number of Security Associations held simultaneously
pub const MAX_SAS: usize = 16;

// ---------------------------------------------------------------------------
// Enumerations — all Copy, no heap
// ---------------------------------------------------------------------------

/// IPsec mode: Transport (end-to-end, original IP header kept) or
/// Tunnel (entire packet wrapped in new IP header).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpsecMode {
    /// Transport mode: protects payload only; original IP header unchanged.
    Transport,
    /// Tunnel mode: protects entire original IP packet inside a new IP header.
    Tunnel,
}

/// Which IPsec protocol this SA uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpsecProto {
    /// Encapsulating Security Payload: provides confidentiality + optional auth.
    Esp,
    /// Authentication Header: provides integrity + anti-replay, no confidentiality.
    Ah,
}

/// Encryption transform applied by ESP.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpsecTransform {
    /// AES-CBC with 128-bit (16-byte) key.
    AesCbc128,
    /// AES-CBC with 256-bit (32-byte) key.
    AesCbc256,
    /// Null encryption (ESP confidentiality disabled; auth still applied).
    Null,
}

/// Authentication algorithm (HMAC variant) for ICV computation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpsecAuthAlg {
    /// HMAC-SHA-1 truncated to 96 bits (12 bytes) — RFC 2404.
    HmacSha1,
    /// HMAC-SHA-256 truncated to 96 bits (12 bytes) — RFC 4868.
    HmacSha256,
    /// No authentication (ESP-NULL, or AH with no ICV).
    Null,
}

// ---------------------------------------------------------------------------
// Security Association
// ---------------------------------------------------------------------------

/// A Security Association: the full set of parameters for one direction
/// of protected communication between two IP endpoints.
///
/// Must be `Copy` so it can live in a static `Mutex<[SecurityAssoc; N]>`.
#[derive(Clone, Copy)]
pub struct SecurityAssoc {
    /// Security Parameters Index — 32-bit token identifying this SA to the receiver.
    pub spi: u32,
    /// Outbound sequence number (wrapping per RFC 4303 §3.3.3).
    pub seq: u32,
    /// Transport or Tunnel mode.
    pub mode: IpsecMode,
    /// ESP or AH.
    pub proto: IpsecProto,
    /// Encryption algorithm.
    pub transform: IpsecTransform,
    /// Authentication algorithm.
    pub auth_alg: IpsecAuthAlg,
    /// Encryption key bytes (up to 32 bytes; length given by enc_key_len).
    pub enc_key: [u8; 32],
    /// Number of valid bytes in enc_key.
    pub enc_key_len: u8,
    /// Authentication key bytes (up to 32 bytes; length given by auth_key_len).
    pub auth_key: [u8; 32],
    /// Number of valid bytes in auth_key.
    pub auth_key_len: u8,
    /// Source IP address (4 bytes, network byte order).
    pub src_ip: [u8; 4],
    /// Destination IP address (4 bytes, network byte order).
    pub dst_ip: [u8; 4],
    /// Whether this SA slot is occupied.
    pub active: bool,
    /// Total bytes transmitted through this SA (outbound).
    pub tx_bytes: u64,
    /// Total bytes received through this SA (inbound).
    pub rx_bytes: u64,
}

impl SecurityAssoc {
    /// Construct an empty (inactive) SA suitable for use in a const context.
    pub const fn empty() -> Self {
        SecurityAssoc {
            spi: 0,
            seq: 0,
            mode: IpsecMode::Transport,
            proto: IpsecProto::Esp,
            transform: IpsecTransform::Null,
            auth_alg: IpsecAuthAlg::Null,
            enc_key: [0u8; 32],
            enc_key_len: 0,
            auth_key: [0u8; 32],
            auth_key_len: 0,
            src_ip: [0u8; 4],
            dst_ip: [0u8; 4],
            active: false,
            tx_bytes: 0,
            rx_bytes: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global SA table
// ---------------------------------------------------------------------------

/// Global Security Association database.
///
/// Fixed-size array; no heap. Lock before accessing.
static SECURITY_ASSOCS: Mutex<[SecurityAssoc; MAX_SAS]> =
    Mutex::new([SecurityAssoc::empty(); MAX_SAS]);

// ---------------------------------------------------------------------------
// SA management
// ---------------------------------------------------------------------------

/// Add a new Security Association to the database.
///
/// Finds the first inactive slot, fills it, and returns `true`.
/// Returns `false` if the table is full or the key slices are too long.
pub fn ipsec_add_sa(
    spi: u32,
    mode: IpsecMode,
    proto: IpsecProto,
    transform: IpsecTransform,
    auth_alg: IpsecAuthAlg,
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    enc_key: &[u8],
    auth_key: &[u8],
) -> bool {
    // Keys must fit in the fixed arrays.
    if enc_key.len() > 32 || auth_key.len() > 32 {
        return false;
    }

    let mut sas = SECURITY_ASSOCS.lock();
    for i in 0..MAX_SAS {
        if !sas[i].active {
            let mut sa = SecurityAssoc::empty();
            sa.spi = spi;
            sa.mode = mode;
            sa.proto = proto;
            sa.transform = transform;
            sa.auth_alg = auth_alg;
            sa.src_ip = src_ip;
            sa.dst_ip = dst_ip;
            sa.enc_key_len = enc_key.len() as u8;
            sa.auth_key_len = auth_key.len() as u8;
            sa.enc_key[..enc_key.len()].copy_from_slice(enc_key);
            sa.auth_key[..auth_key.len()].copy_from_slice(auth_key);
            sa.active = true;
            sas[i] = sa;
            return true;
        }
    }
    false // table full
}

/// Remove the Security Association with the given SPI.
///
/// Returns `true` if found and removed, `false` if not found.
pub fn ipsec_del_sa(spi: u32) -> bool {
    let mut sas = SECURITY_ASSOCS.lock();
    for i in 0..MAX_SAS {
        if sas[i].active && sas[i].spi == spi {
            sas[i] = SecurityAssoc::empty();
            return true;
        }
    }
    false
}

/// Find an outbound SA for the given destination IP.
///
/// Returns the SPI of the first active SA whose dst_ip matches, or `None`.
pub fn ipsec_find_sa_out(dst_ip: [u8; 4]) -> Option<u32> {
    let sas = SECURITY_ASSOCS.lock();
    for i in 0..MAX_SAS {
        if sas[i].active && sas[i].dst_ip == dst_ip {
            return Some(sas[i].spi);
        }
    }
    None
}

/// Find an inbound SA by SPI.
///
/// Returns the index into the SA table, or `None` if not found.
pub fn ipsec_find_sa_in(spi: u32) -> Option<usize> {
    let sas = SECURITY_ASSOCS.lock();
    for i in 0..MAX_SAS {
        if sas[i].active && sas[i].spi == spi {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// ICV (Integrity Check Value) length in bytes — 96-bit truncation per RFC 2404 / RFC 4868.
const ICV_LEN: usize = 12;

/// Return the key length in bytes implied by the transform, or 0 for Null.
#[inline]
fn enc_key_len_for(transform: IpsecTransform) -> usize {
    match transform {
        IpsecTransform::AesCbc128 => 16,
        IpsecTransform::AesCbc256 => 32,
        IpsecTransform::Null => 0,
    }
}

/// XOR-encrypt/decrypt `data` in-place using the key in a rolling byte pattern.
///
/// NOTE: This is a STUB that mimics the interface of AES-CBC encryption.
/// A production kernel MUST replace this with a call to `crate::crypto::aes`
/// (specifically the CBC mode in `crate::crypto::ctr` or `crate::crypto::aes`
/// using a proper IV).  The XOR stub preserves packet structure and allows
/// the rest of the ESP pipeline to be exercised without a full AES dependency.
#[inline]
fn xor_encrypt(data: &mut [u8], key: &[u8], key_len: usize) {
    if key_len == 0 {
        return; // Null transform: no encryption
    }
    // Real AES-CBC would call: crate::crypto::aes::cbc_encrypt(key, iv, data)
    for (i, byte) in data.iter_mut().enumerate() {
        *byte ^= key[i % key_len];
    }
}

/// Compute a 12-byte ICV (Integrity Check Value) over `data` using HMAC-SHA-256.
///
/// Uses `crate::crypto::sha256::hmac_sha256(auth_key, data)` and takes the
/// first 12 bytes (96-bit truncation per RFC 4868 §2.1).
///
/// For HmacSha1 we fall through to the same HMAC-SHA-256 primitive as a
/// conservative placeholder — real HMAC-SHA-1 would call a dedicated SHA-1
/// implementation.  The truncated output width (12 bytes) is the same.
fn compute_icv(auth_alg: IpsecAuthAlg, auth_key: &[u8], data: &[u8]) -> [u8; ICV_LEN] {
    let mut icv = [0u8; ICV_LEN];
    match auth_alg {
        IpsecAuthAlg::Null => {
            // No authentication: ICV stays all-zero.
        }
        IpsecAuthAlg::HmacSha1 | IpsecAuthAlg::HmacSha256 => {
            // crate::crypto::sha256::hmac_sha256 → 32-byte tag; truncate to 12.
            let tag = crate::crypto::sha256::hmac_sha256(auth_key, data);
            icv.copy_from_slice(&tag[..ICV_LEN]);
        }
    }
    icv
}

/// Verify a 12-byte ICV in constant time.
///
/// Returns `true` if the computed ICV matches the supplied one.
fn verify_icv(
    auth_alg: IpsecAuthAlg,
    auth_key: &[u8],
    data: &[u8],
    expected: &[u8; ICV_LEN],
) -> bool {
    if auth_alg == IpsecAuthAlg::Null {
        return true; // No authentication; always accept.
    }
    let computed = compute_icv(auth_alg, auth_key, data);
    // Constant-time comparison — all bytes examined regardless of difference.
    let mut diff: u8 = 0;
    for i in 0..ICV_LEN {
        diff |= computed[i] ^ expected[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// ESP processing
// ---------------------------------------------------------------------------

/// Encrypt an IP payload into an ESP packet.
///
/// ESP wire format (RFC 4303 §2):
///   [SPI (4)] [Seq (4)] [Payload ...] [Pad (0-3)] [Pad_len (1)] [NH (1)] [ICV (12)]
///
/// # Arguments
/// - `sa_idx`  — index of the outbound SA in SECURITY_ASSOCS
/// - `payload` — plaintext upper-layer data
/// - `out`     — output buffer (1500 bytes — Ethernet MTU)
/// - `out_len` — receives the number of bytes written to `out`
///
/// Returns `true` on success, `false` if the payload is too large to fit.
pub fn esp_encrypt(
    sa_idx: usize,
    payload: &[u8],
    out: &mut [u8; 1500],
    out_len: &mut usize,
) -> bool {
    *out_len = 0;

    // --- Read SA fields (briefly hold lock, copy what we need) ---
    let (spi, seq_new, transform, auth_alg, enc_key, enc_key_len, auth_key, auth_key_len) = {
        let mut sas = SECURITY_ASSOCS.lock();
        if sa_idx >= MAX_SAS || !sas[sa_idx].active {
            return false;
        }
        let sa = &mut sas[sa_idx];
        // Advance sequence number (wrapping per RFC 4303 §3.3.3).
        sa.seq = sa.seq.wrapping_add(1);
        let seq = sa.seq;
        let klen = sa.enc_key_len as usize;
        let alen = sa.auth_key_len as usize;
        let mut ek = [0u8; 32];
        let mut ak = [0u8; 32];
        ek[..klen].copy_from_slice(&sa.enc_key[..klen]);
        ak[..alen].copy_from_slice(&sa.auth_key[..alen]);
        (sa.spi, seq, sa.transform, sa.auth_alg, ek, klen, ak, alen)
    };

    // --- Build ESP header (8 bytes: SPI + Seq) ---
    let spi_bytes = spi.to_be_bytes(); // SPI, big-endian
    let seq_bytes = seq_new.to_be_bytes(); // Seq, big-endian

    // Calculate padding: payload + pad + pad_len(1) + NH(1) must be 4-byte aligned.
    let raw_len = payload.len().saturating_add(2); // +pad_len +NH
    let pad_len: usize = if raw_len % 4 == 0 {
        0
    } else {
        4 - (raw_len % 4)
    };
    let plaintext_total = payload.len().saturating_add(pad_len).saturating_add(2);

    // Total ESP packet: 4(SPI) + 4(Seq) + plaintext_total + ICV_LEN
    let total = 4usize
        .saturating_add(4)
        .saturating_add(plaintext_total)
        .saturating_add(ICV_LEN);
    if total > 1500 {
        return false; // Won't fit in one Ethernet frame.
    }

    // --- Write SPI and Seq into output buffer ---
    out[0..4].copy_from_slice(&spi_bytes);
    out[4..8].copy_from_slice(&seq_bytes);

    // --- Assemble plaintext region: payload || pad bytes || pad_len || NH ---
    // pad bytes are 1, 2, 3, ... (RFC 4303 §2.4 self-describing padding).
    let pt_start = 8usize;
    out[pt_start..pt_start + payload.len()].copy_from_slice(payload);
    let pad_start = pt_start + payload.len();
    for i in 0..pad_len {
        out[pad_start + i] = (i + 1) as u8;
    }
    out[pad_start + pad_len] = pad_len as u8; // Pad_len
    out[pad_start + pad_len + 1] = 0x00; // Next Header (0 = placeholder; real use = upper proto)

    // --- Encrypt the plaintext region in-place ---
    // Real AES-CBC: crate::crypto::aes::cbc_encrypt(&enc_key[..enc_key_len], iv, plaintext)
    let actual_key_len = if enc_key_len == 0 {
        enc_key_len_for(transform)
    } else {
        enc_key_len
    };
    xor_encrypt(
        &mut out[pt_start..pt_start + plaintext_total],
        &enc_key,
        actual_key_len,
    );

    // --- Compute ICV over SPI || Seq || ciphertext (bytes 0..8+plaintext_total) ---
    let auth_end = 8usize.saturating_add(plaintext_total);
    let auth_key_slice = &auth_key[..auth_key_len];
    let icv = compute_icv(auth_alg, auth_key_slice, &out[..auth_end]);
    out[auth_end..auth_end + ICV_LEN].copy_from_slice(&icv);

    // --- Update tx_bytes counter ---
    {
        let mut sas = SECURITY_ASSOCS.lock();
        if sa_idx < MAX_SAS && sas[sa_idx].active {
            sas[sa_idx].tx_bytes = sas[sa_idx].tx_bytes.saturating_add(total as u64);
        }
    }

    *out_len = total;
    true
}

/// Decrypt an inbound ESP packet.
///
/// Verifies the ICV, then decrypts and strips the ESP header, padding,
/// and ICV to recover the original payload.
///
/// # Arguments
/// - `sa_idx`  — index of the inbound SA
/// - `pkt`     — raw ESP packet bytes (starting at the SPI field)
/// - `len`     — number of valid bytes in `pkt`
/// - `out`     — output buffer for decrypted payload
/// - `out_len` — receives number of decrypted bytes
///
/// Returns `true` on success, `false` on auth failure or malformed packet.
pub fn esp_decrypt(
    sa_idx: usize,
    pkt: &[u8],
    len: usize,
    out: &mut [u8; 1500],
    out_len: &mut usize,
) -> bool {
    *out_len = 0;

    // Minimum ESP packet: 4(SPI) + 4(Seq) + 2(pad_len+NH) + 12(ICV) = 22 bytes.
    if len < 22 || len > 1500 {
        return false;
    }

    // --- Read SA (copy needed fields) ---
    let (transform, auth_alg, enc_key, enc_key_len, auth_key, auth_key_len) = {
        let sas = SECURITY_ASSOCS.lock();
        if sa_idx >= MAX_SAS || !sas[sa_idx].active {
            return false;
        }
        let sa = &sas[sa_idx];
        let klen = sa.enc_key_len as usize;
        let alen = sa.auth_key_len as usize;
        let mut ek = [0u8; 32];
        let mut ak = [0u8; 32];
        ek[..klen].copy_from_slice(&sa.enc_key[..klen]);
        ak[..alen].copy_from_slice(&sa.auth_key[..alen]);
        (sa.transform, sa.auth_alg, ek, klen, ak, alen)
    };

    // --- Verify ICV before decrypting (authenticate-then-decrypt) ---
    let icv_start = len - ICV_LEN;
    let mut expected_icv = [0u8; ICV_LEN];
    expected_icv.copy_from_slice(&pkt[icv_start..len]);

    let auth_key_slice = &auth_key[..auth_key_len];
    if !verify_icv(auth_alg, auth_key_slice, &pkt[..icv_start], &expected_icv) {
        return false; // ICV mismatch — drop packet.
    }

    // --- Copy and decrypt the ciphertext (bytes 8..icv_start) ---
    let ct_start = 8usize;
    let ct_len = icv_start - ct_start;
    if ct_len < 2 {
        return false; // Must have at least pad_len + NH bytes.
    }

    let mut plaintext = [0u8; 1500];
    plaintext[..ct_len].copy_from_slice(&pkt[ct_start..icv_start]);

    // Real AES-CBC: crate::crypto::aes::cbc_decrypt(&enc_key[..enc_key_len], iv, &mut plaintext[..ct_len])
    let actual_key_len = if enc_key_len == 0 {
        enc_key_len_for(transform)
    } else {
        enc_key_len
    };
    xor_encrypt(&mut plaintext[..ct_len], &enc_key, actual_key_len); // XOR is self-inverse

    // --- Strip padding: last two bytes of plaintext are Pad_len and NH ---
    let pad_len = plaintext[ct_len - 2] as usize;
    let _nh = plaintext[ct_len - 1]; // Next Header (caller may inspect if needed)

    // Payload occupies bytes 0..(ct_len - 2 - pad_len).
    let payload_len = ct_len.saturating_sub(2).saturating_sub(pad_len);
    if payload_len > 1500 {
        return false;
    }
    out[..payload_len].copy_from_slice(&plaintext[..payload_len]);

    // --- Update rx_bytes counter ---
    {
        let mut sas = SECURITY_ASSOCS.lock();
        if sa_idx < MAX_SAS && sas[sa_idx].active {
            sas[sa_idx].rx_bytes = sas[sa_idx].rx_bytes.saturating_add(len as u64);
        }
    }

    *out_len = payload_len;
    true
}

// ---------------------------------------------------------------------------
// AH processing
// ---------------------------------------------------------------------------

/// AH header layout (RFC 4302 §2.1):
///   Next Header (1) | Payload Len (1) | Reserved (2) | SPI (4) | Seq (4) | ICV (12)
///   Total fixed header = 24 bytes.
const AH_HDR_LEN: usize = 24;

/// Compute the AH ICV over the packet and write it into the AH ICV field.
///
/// The ICV covers the entire packet with the ICV field zeroed (mutable
/// zero-filled position), per RFC 4302 §2.6.
///
/// # Arguments
/// - `sa_idx` — index of the SA in SECURITY_ASSOCS
/// - `pkt`    — packet buffer containing the AH header starting at byte 0;
///              the ICV field (bytes 12..24 of the AH header) is cleared and
///              then written with the computed ICV.
/// - `len`    — total packet length (must be at least AH_HDR_LEN)
pub fn ah_sign(sa_idx: usize, pkt: &mut [u8], len: usize) {
    if len < AH_HDR_LEN || sa_idx >= MAX_SAS {
        return;
    }

    // Read SA auth fields.
    let (auth_alg, auth_key, auth_key_len) = {
        let sas = SECURITY_ASSOCS.lock();
        if !sas[sa_idx].active {
            return;
        }
        let sa = &sas[sa_idx];
        let alen = sa.auth_key_len as usize;
        let mut ak = [0u8; 32];
        ak[..alen].copy_from_slice(&sa.auth_key[..alen]);
        (sa.auth_alg, ak, alen)
    };

    // Zero the ICV field before hashing (per RFC 4302).
    // AH ICV starts at byte 12 within the AH header.
    for i in 12..12 + ICV_LEN {
        if i < len {
            pkt[i] = 0;
        }
    }

    // Hash auth_key || entire packet (with zeroed ICV) using SHA-256.
    // Real HMAC-SHA-1 would call crate::crypto::sha256::hmac_sha256 with SHA-1 internals.
    let auth_key_slice = &auth_key[..auth_key_len];
    let icv = compute_icv(auth_alg, auth_key_slice, &pkt[..len]);

    // Write ICV into bytes 12..24 of the AH header.
    pkt[12..12 + ICV_LEN].copy_from_slice(&icv);
}

/// Verify an AH-protected packet.
///
/// Zeros the ICV field, recomputes the ICV, and compares with the stored value.
///
/// Returns `true` if the ICV is valid (packet is authentic), `false` otherwise.
pub fn ah_verify(sa_idx: usize, pkt: &[u8], len: usize) -> bool {
    if len < AH_HDR_LEN || sa_idx >= MAX_SAS {
        return false;
    }

    // Read SA auth fields.
    let (auth_alg, auth_key, auth_key_len) = {
        let sas = SECURITY_ASSOCS.lock();
        if sa_idx >= MAX_SAS || !sas[sa_idx].active {
            return false;
        }
        let sa = &sas[sa_idx];
        let alen = sa.auth_key_len as usize;
        let mut ak = [0u8; 32];
        ak[..alen].copy_from_slice(&sa.auth_key[..alen]);
        (sa.auth_alg, ak, alen)
    };

    // Extract the ICV from the packet.
    if 12 + ICV_LEN > len {
        return false;
    }
    let mut stored_icv = [0u8; ICV_LEN];
    stored_icv.copy_from_slice(&pkt[12..12 + ICV_LEN]);

    // Build a working copy of the packet with ICV zeroed.
    let mut buf = [0u8; 1500];
    if len > 1500 {
        return false;
    }
    buf[..len].copy_from_slice(&pkt[..len]);
    for i in 12..12 + ICV_LEN {
        buf[i] = 0;
    }

    let auth_key_slice = &auth_key[..auth_key_len];
    verify_icv(auth_alg, auth_key_slice, &buf[..len], &stored_icv)
}

// ---------------------------------------------------------------------------
// Output / Input dispatch
// ---------------------------------------------------------------------------

/// Process an outbound packet through IPsec.
///
/// Looks up the SA for `dst_ip`, applies ESP or AH processing, and
/// returns the resulting packet in `out`.
///
/// # Arguments
/// - `dst_ip`  — destination IP address of the original packet
/// - `payload` — upper-layer payload bytes
/// - `plen`    — number of valid bytes in `payload`
/// - `proto`   — upper-layer protocol number (written into ESP NH field)
/// - `out`     — output buffer (1500 bytes)
/// - `out_len` — receives the number of bytes written
///
/// Returns `true` if an SA was found and processing succeeded.
pub fn ipsec_output(
    dst_ip: [u8; 4],
    payload: &[u8],
    plen: usize,
    proto: u8,
    out: &mut [u8; 1500],
    out_len: &mut usize,
) -> bool {
    *out_len = 0;

    // Find the SA for this destination.
    let spi = match ipsec_find_sa_out(dst_ip) {
        Some(s) => s,
        None => return false,
    };
    let sa_idx = match ipsec_find_sa_in(spi) {
        Some(i) => i,
        None => return false,
    };

    // Read the protocol from the SA.
    let ipsec_proto = {
        let sas = SECURITY_ASSOCS.lock();
        sas[sa_idx].proto
    };

    let pslice = &payload[..plen.min(payload.len())];

    match ipsec_proto {
        IpsecProto::Esp => {
            let _ = proto; // NH field not written into ESP header in this stub
            esp_encrypt(sa_idx, pslice, out, out_len)
        }
        IpsecProto::Ah => {
            // For AH in transport mode we copy the payload first, then sign.
            let total = plen.min(payload.len());
            if total.saturating_add(AH_HDR_LEN) > 1500 {
                return false;
            }
            // Build minimal AH header: NH | Len | Reserved (2) | SPI (4) | Seq (4) | ICV (12)
            let (sa_spi, sa_seq_new) = {
                let mut sas = SECURITY_ASSOCS.lock();
                sas[sa_idx].seq = sas[sa_idx].seq.wrapping_add(1);
                let s = sas[sa_idx].seq;
                (sas[sa_idx].spi, s)
            };

            // AH header (24 bytes) followed by payload.
            out[0] = proto; // Next Header
            out[1] = ((AH_HDR_LEN / 4) - 2) as u8; // Payload Len in 4-byte units − 2
            out[2] = 0; // Reserved
            out[3] = 0;
            out[4..8].copy_from_slice(&sa_spi.to_be_bytes()); // SPI
            out[8..12].copy_from_slice(&sa_seq_new.to_be_bytes()); // Seq
                                                                   // ICV (12 bytes) at offset 12..24 — zeroed initially.
            for i in 12..AH_HDR_LEN {
                out[i] = 0;
            }
            // Payload follows AH header.
            out[AH_HDR_LEN..AH_HDR_LEN + total].copy_from_slice(pslice);

            let total_len = AH_HDR_LEN.saturating_add(total);
            ah_sign(sa_idx, out, total_len);
            *out_len = total_len;
            true
        }
    }
}

/// Process an inbound IPsec packet.
///
/// Dispatches to ESP decrypt or AH verify based on `proto`.
///
/// # Arguments
/// - `pkt`   — raw packet bytes (starting at the ESP/AH header)
/// - `len`   — number of valid bytes in `pkt`
/// - `proto` — IP protocol number (IPPROTO_ESP or IPPROTO_AH)
///
/// Returns `true` if the packet is authentic and (for ESP) successfully decrypted.
pub fn ipsec_input(pkt: &[u8], len: usize, proto: u8) -> bool {
    if len < 8 {
        return false;
    }

    // Extract SPI from the first 4 bytes (both ESP and AH share this layout).
    let spi = u32::from_be_bytes([pkt[0], pkt[1], pkt[2], pkt[3]]);

    let sa_idx = match ipsec_find_sa_in(spi) {
        Some(i) => i,
        None => return false,
    };

    match proto {
        IPPROTO_ESP => {
            let mut out = [0u8; 1500];
            let mut out_len = 0usize;
            esp_decrypt(sa_idx, pkt, len, &mut out, &mut out_len)
        }
        IPPROTO_AH => ah_verify(sa_idx, pkt, len),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the IPsec subsystem.
///
/// Currently just clears the SA table (already zeroed as a static) and
/// prints a banner to the serial console.
pub fn init() {
    // SA table is already zero-initialised (all inactive) via the static initialiser.
    serial_println!("[ipsec] subsystem initialized");
}
