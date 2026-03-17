/// x509_crl — X.509 Certificate Revocation List parser and checker
///
/// Implements:
///   - Minimal DER/ASN.1 TLV parser for CRL tbsCertList structure
///   - In-memory flat table of revoked certificate serial numbers
///   - Revocation status query by serial number
///   - CRL validity window (thisUpdate / nextUpdate)
///
/// ASN.1 tags used:
///   0x30 SEQUENCE, 0x02 INTEGER, 0x17 UTCTime, 0x18 GeneralizedTime
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum CRL stores simultaneously loaded.
const MAX_CRLS: usize = 4;
/// Maximum revoked certificate serial numbers per CRL.
const MAX_REVOKED: usize = 512;
/// Max serial number length (bytes).
const MAX_SERIAL_LEN: usize = 20;

// ASN.1 tags
const TAG_SEQ: u8 = 0x30;
const TAG_INT: u8 = 0x02;
const TAG_UTCTIME: u8 = 0x17;
const TAG_GENTIME: u8 = 0x18;
const TAG_BITSTRING: u8 = 0x03;
const TAG_OID: u8 = 0x06;

// CRL revocation reasons (RFC 5280 §5.3.1)
pub const CRL_REASON_UNSPECIFIED: u8 = 0;
pub const CRL_REASON_KEY_COMPROMISE: u8 = 1;
pub const CRL_REASON_CA_COMPROMISE: u8 = 2;
pub const CRL_REASON_AFFILIATION_CHANGED: u8 = 3;
pub const CRL_REASON_SUPERSEDED: u8 = 4;
pub const CRL_REASON_CESSATION_OF_OPERATION: u8 = 5;
pub const CRL_REASON_CERTIFICATE_HOLD: u8 = 6;
pub const CRL_REASON_REMOVE_FROM_CRL: u8 = 8;
pub const CRL_REASON_PRIVILEGE_WITHDRAWN: u8 = 9;
pub const CRL_REASON_AA_COMPROMISE: u8 = 10;

// ---------------------------------------------------------------------------
// Revoked certificate entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct RevokedCert {
    pub serial: [u8; MAX_SERIAL_LEN],
    pub serial_len: u8,
    pub reason: u8, // CRL_REASON_*
    pub active: bool,
}

impl RevokedCert {
    pub const fn empty() -> Self {
        RevokedCert {
            serial: [0u8; MAX_SERIAL_LEN],
            serial_len: 0,
            reason: CRL_REASON_UNSPECIFIED,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// CRL store
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CrlStore {
    pub id: u32,
    pub revoked: [RevokedCert; MAX_REVOKED],
    pub revoked_count: u32,
    /// Validity: this_update and next_update as compact YYMMDDHHmmssZ bytes
    pub this_update: [u8; 13],
    pub next_update: [u8; 13],
    pub this_update_len: u8,
    pub next_update_len: u8,
    pub active: bool,
}

impl CrlStore {
    pub const fn empty() -> Self {
        const EMPTY_RC: RevokedCert = RevokedCert::empty();
        CrlStore {
            id: 0,
            revoked: [EMPTY_RC; MAX_REVOKED],
            revoked_count: 0,
            this_update: [0u8; 13],
            next_update: [0u8; 13],
            this_update_len: 0,
            next_update_len: 0,
            active: false,
        }
    }
}

const EMPTY_CRL: CrlStore = CrlStore::empty();
static CRL_TABLE: Mutex<[CrlStore; MAX_CRLS]> = Mutex::new([EMPTY_CRL; MAX_CRLS]);
static CRL_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// DER TLV helpers
// ---------------------------------------------------------------------------

/// Read a DER length at `buf[off]`. Returns (length, bytes_consumed).
fn der_read_len(buf: &[u8], off: usize) -> Option<(usize, usize)> {
    if off >= buf.len() {
        return None;
    }
    let first = buf[off];
    if first & 0x80 == 0 {
        // Short form
        return Some((first as usize, 1));
    }
    let n_bytes = (first & 0x7F) as usize;
    if n_bytes == 0 || n_bytes > 4 {
        return None;
    } // indefinite or too large
    if off + 1 + n_bytes > buf.len() {
        return None;
    }
    let mut len = 0usize;
    let mut i = 0usize;
    while i < n_bytes {
        len = (len << 8) | buf[off + 1 + i] as usize;
        i = i.saturating_add(1);
    }
    Some((len, 1 + n_bytes))
}

/// Read a DER TLV at `off`. Returns (tag, value_offset, value_len, total_bytes).
fn der_read_tlv(buf: &[u8], off: usize) -> Option<(u8, usize, usize, usize)> {
    if off >= buf.len() {
        return None;
    }
    let tag = buf[off];
    let (len, len_bytes) = der_read_len(buf, off + 1)?;
    let value_off = off + 1 + len_bytes;
    if value_off + len > buf.len() {
        return None;
    }
    Some((tag, value_off, len, 1 + len_bytes + len))
}

// ---------------------------------------------------------------------------
// CRL DER parser
// ---------------------------------------------------------------------------

/// Parse a DER-encoded CRL and populate a CrlStore with revoked entries.
/// Returns the number of revoked certificates parsed, or 0 on parse error.
pub fn crl_parse(der: &[u8], crl_id: u32) -> u32 {
    let mut table = CRL_TABLE.lock();
    // Find store
    let mut slot = MAX_CRLS;
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active && table[i].id == crl_id {
            slot = i;
            break;
        }
        i = i.saturating_add(1);
    }
    if slot == MAX_CRLS {
        return 0;
    }

    // CRL DER structure (simplified):
    //   SEQUENCE {                         ← CertificateList
    //     SEQUENCE {                       ← tbsCertList
    //       INTEGER version (optional)
    //       SEQUENCE signatureAlgorithm
    //       SEQUENCE issuer
    //       UTCTime/GeneralizedTime thisUpdate
    //       UTCTime/GeneralizedTime nextUpdate (optional)
    //       SEQUENCE revokedCertificates {
    //         SEQUENCE {
    //           INTEGER serialNumber
    //           UTCTime revocationDate
    //           SEQUENCE extensions (optional)
    //         } ...
    //       }
    //     }
    //     SEQUENCE signatureAlgorithm
    //     BIT STRING signature
    //   }

    // Outer SEQUENCE
    let (tag, voff, vlen, _) = match der_read_tlv(der, 0) {
        Some(t) => t,
        None => return 0,
    };
    if tag != TAG_SEQ {
        return 0;
    }
    let tbs_end = voff + vlen;

    // tbsCertList SEQUENCE
    let (tag2, tbs_voff, tbs_vlen, _) = match der_read_tlv(der, voff) {
        Some(t) => t,
        None => return 0,
    };
    if tag2 != TAG_SEQ {
        return 0;
    }

    let mut off = tbs_voff;
    let tbs_end2 = tbs_voff + tbs_vlen;

    // Skip optional version (INTEGER if present before algorithm)
    // Skip signatureAlgorithm (SEQUENCE)
    // Skip issuer (SEQUENCE)
    // Both are SEQUENCEs after version; skip up to 4 SEQUENCEs/INTEGERs before times
    let mut skipped = 0usize;
    while off < tbs_end2 && skipped < 4 {
        if off >= der.len() {
            break;
        }
        let t = der[off];
        if t == TAG_UTCTIME || t == TAG_GENTIME {
            break;
        }
        let (_, _, _, total) = match der_read_tlv(der, off) {
            Some(x) => x,
            None => break,
        };
        off = off.saturating_add(total);
        skipped = skipped.saturating_add(1);
    }

    // thisUpdate
    if off < tbs_end2 {
        let t = der[off];
        if t == TAG_UTCTIME || t == TAG_GENTIME {
            let (_, tv, tl, ttotal) = match der_read_tlv(der, off) {
                Some(x) => x,
                None => return 0,
            };
            let copy_len = tl.min(13);
            let mut k = 0usize;
            while k < copy_len {
                table[slot].this_update[k] = der[tv + k];
                k = k.saturating_add(1);
            }
            table[slot].this_update_len = copy_len as u8;
            off = off.saturating_add(ttotal);
        }
    }

    // nextUpdate (optional)
    if off < tbs_end2 {
        let t = der[off];
        if t == TAG_UTCTIME || t == TAG_GENTIME {
            let (_, tv, tl, ttotal) = match der_read_tlv(der, off) {
                Some(x) => x,
                None => return 0,
            };
            let copy_len = tl.min(13);
            let mut k = 0usize;
            while k < copy_len {
                table[slot].next_update[k] = der[tv + k];
                k = k.saturating_add(1);
            }
            table[slot].next_update_len = copy_len as u8;
            off = off.saturating_add(ttotal);
        }
    }

    // revokedCertificates SEQUENCE (optional — may be absent if CRL is empty)
    if off >= tbs_end2 {
        return 0;
    }
    let (tag3, rv_voff, rv_vlen, rv_total) = match der_read_tlv(der, off) {
        Some(x) => x,
        None => return 0,
    };
    if tag3 != TAG_SEQ {
        return 0;
    } // not revoked list
    let rv_end = rv_voff + rv_vlen;
    off = rv_voff;

    let mut count = 0u32;

    // Each revoked entry: SEQUENCE { INTEGER serialNumber, UTCTime date, ... }
    while off < rv_end && (table[slot].revoked_count as usize) < MAX_REVOKED {
        let (et, ev, el, etotal) = match der_read_tlv(der, off) {
            Some(x) => x,
            None => break,
        };
        if et != TAG_SEQ {
            off = off.saturating_add(etotal);
            continue;
        }
        let entry_end = ev + el;
        let mut eoff = ev;

        // Read serialNumber
        if eoff >= entry_end {
            off = off.saturating_add(etotal);
            continue;
        }
        let (st, sv, sl, stotal) = match der_read_tlv(der, eoff) {
            Some(x) => x,
            None => break,
        };
        if st != TAG_INT {
            off = off.saturating_add(etotal);
            continue;
        }

        let copy_len = sl.min(MAX_SERIAL_LEN);
        let idx = table[slot].revoked_count as usize;
        if idx < MAX_REVOKED {
            let mut k = 0usize;
            while k < copy_len {
                table[slot].revoked[idx].serial[k] = der[sv + k];
                k = k.saturating_add(1);
            }
            table[slot].revoked[idx].serial_len = copy_len as u8;
            table[slot].revoked[idx].reason = CRL_REASON_UNSPECIFIED;
            table[slot].revoked[idx].active = true;
            table[slot].revoked_count = table[slot].revoked_count.saturating_add(1);
            count = count.saturating_add(1);
        }

        off = off.saturating_add(etotal);
    }

    count
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new empty CRL store. Returns store id.
pub fn crl_create() -> Option<u32> {
    let id = CRL_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut table = CRL_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if !table[i].active {
            table[i] = CrlStore::empty();
            table[i].id = id;
            table[i].active = true;
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Manually add a revoked serial number to a CRL store.
pub fn crl_add_revoked(crl_id: u32, serial: &[u8], reason: u8) -> bool {
    let mut table = CRL_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active && table[i].id == crl_id {
            let idx = table[i].revoked_count as usize;
            if idx >= MAX_REVOKED {
                return false;
            }
            let copy_len = serial.len().min(MAX_SERIAL_LEN);
            let mut k = 0usize;
            while k < copy_len {
                table[i].revoked[idx].serial[k] = serial[k];
                k = k.saturating_add(1);
            }
            table[i].revoked[idx].serial_len = copy_len as u8;
            table[i].revoked[idx].reason = reason;
            table[i].revoked[idx].active = true;
            table[i].revoked_count = table[i].revoked_count.saturating_add(1);
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Check if a certificate serial number is revoked in a given CRL.
/// Returns Some(reason) if revoked, None if not found.
pub fn crl_is_revoked(crl_id: u32, serial: &[u8]) -> Option<u8> {
    let table = CRL_TABLE.lock();
    let slen = serial.len();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active && table[i].id == crl_id {
            let count = table[i].revoked_count as usize;
            let mut j = 0usize;
            while j < count {
                if table[i].revoked[j].active && table[i].revoked[j].serial_len as usize == slen {
                    let mut eq = true;
                    let mut k = 0usize;
                    while k < slen {
                        if table[i].revoked[j].serial[k] != serial[k] {
                            eq = false;
                            break;
                        }
                        k = k.saturating_add(1);
                    }
                    if eq {
                        return Some(table[i].revoked[j].reason);
                    }
                }
                j = j.saturating_add(1);
            }
            return None;
        }
        i = i.saturating_add(1);
    }
    None
}

/// Check serial against ALL loaded CRLs. Returns Some(reason) if revoked in any.
pub fn crl_check_all(serial: &[u8]) -> Option<u8> {
    let table = CRL_TABLE.lock();
    let slen = serial.len();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active {
            let count = table[i].revoked_count as usize;
            let mut j = 0usize;
            while j < count {
                if table[i].revoked[j].active && table[i].revoked[j].serial_len as usize == slen {
                    let mut eq = true;
                    let mut k = 0usize;
                    while k < slen {
                        if table[i].revoked[j].serial[k] != serial[k] {
                            eq = false;
                            break;
                        }
                        k = k.saturating_add(1);
                    }
                    if eq {
                        return Some(table[i].revoked[j].reason);
                    }
                }
                j = j.saturating_add(1);
            }
        }
        i = i.saturating_add(1);
    }
    None
}

/// Return the number of revoked entries in a CRL.
pub fn crl_count(crl_id: u32) -> u32 {
    let table = CRL_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active && table[i].id == crl_id {
            return table[i].revoked_count;
        }
        i = i.saturating_add(1);
    }
    0
}

/// Free a CRL store.
pub fn crl_free(crl_id: u32) -> bool {
    let mut table = CRL_TABLE.lock();
    let mut i = 0usize;
    while i < MAX_CRLS {
        if table[i].active && table[i].id == crl_id {
            table[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    serial_println!(
        "[x509_crl] X.509 CRL subsystem initialized (max {} CRLs, {} entries each)",
        MAX_CRLS,
        MAX_REVOKED
    );
}
