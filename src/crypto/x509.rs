use super::asn1::{
    Asn1Der, OID_COMMON_NAME, OID_ORGANIZATION, OID_SHA256_WITH_RSA, TAG_BIT_STRING, TAG_BOOLEAN,
    TAG_CONTEXT_0, TAG_CONTEXT_3, TAG_GENERALIZED_TIME, TAG_IA5_STRING, TAG_INTEGER,
    TAG_OCTET_STRING, TAG_OID, TAG_PRINTABLE_STRING, TAG_SEQUENCE, TAG_SET, TAG_UTC_TIME,
    TAG_UTF8_STRING,
};
use super::rsa::{rsa_parse_public_key_fixed, rsa_pkcs1_verify_sha256_fixed, RsaPublicKeyFixed};
use crate::sync::Mutex;
/// X.509 v3 certificate parser — no heap, no alloc
///
/// Parses DER-encoded X.509 certificates into fixed-size structs.
/// Signature verification delegates to `crypto::rsa` (RSA-2048 / PKCS#1
/// v1.5 / SHA-256).  A static trust-store holds up to 4 root CA certs.
///
/// All arithmetic is integer-only (no float casts, no f32/f64).
/// Timestamps are Unix seconds (u64) computed with integer day-counting.
use core::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Distinguished Name (subject / issuer)
// ---------------------------------------------------------------------------

/// Compact X.509 Name with just the fields we need.
#[derive(Clone, Copy)]
pub struct X509Name {
    pub common_name: [u8; 64],
    pub cn_len: usize,
    pub organization: [u8; 64],
    pub org_len: usize,
}

impl X509Name {
    pub const fn empty() -> Self {
        X509Name {
            common_name: [0u8; 64],
            cn_len: 0,
            organization: [0u8; 64],
            org_len: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// X.509 Certificate
// ---------------------------------------------------------------------------

/// Parsed X.509 v3 certificate — fixed-size, no heap.
#[derive(Clone, Copy)]
pub struct X509Cert {
    pub version: u8,
    pub serial: [u8; 20],
    pub serial_len: usize,
    pub issuer: X509Name,
    pub subject: X509Name,
    /// Unix timestamp for "not before"
    pub not_before: u64,
    /// Unix timestamp for "not after"
    pub not_after: u64,
    pub public_key: RsaPublicKeyFixed,
    pub is_ca: bool,
    /// Key Usage extension bits (RFC 5280 §4.2.1.3)
    pub key_usage: u16,
    /// True iff the outer signature algorithm OID is sha256WithRSAEncryption
    pub sig_algorithm_is_sha256rsa: bool,
    /// Raw 256-byte RSA signature block (big-endian)
    pub signature: [u8; 256],
    /// SHA-256 hash of the DER-encoded TBSCertificate
    pub tbs_hash: [u8; 32],
    /// Set by `x509_parse` after successful structural parse
    pub valid: bool,
}

impl X509Cert {
    pub const fn empty() -> Self {
        X509Cert {
            version: 0,
            serial: [0u8; 20],
            serial_len: 0,
            issuer: X509Name::empty(),
            subject: X509Name::empty(),
            not_before: 0,
            not_after: 0,
            public_key: RsaPublicKeyFixed::empty(),
            is_ca: false,
            key_usage: 0,
            sig_algorithm_is_sha256rsa: false,
            signature: [0u8; 256],
            tbs_hash: [0u8; 32],
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Trust store — static, up to 4 root CA certs
// ---------------------------------------------------------------------------

static TRUST_STORE: Mutex<[X509Cert; 4]> = Mutex::new([X509Cert::empty(); 4]);
static TRUST_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Add a certificate to the static trust store.
/// Returns `true` on success, `false` if the store is full.
pub fn x509_add_trust(cert: X509Cert) -> bool {
    let idx = TRUST_COUNT.load(Ordering::Relaxed);
    if idx >= 4 {
        return false;
    }
    let mut store = TRUST_STORE.lock();
    store[idx] = cert;
    TRUST_COUNT.store(idx.saturating_add(1), Ordering::Release);
    true
}

/// Verify `cert` against each trusted root: return `true` if at least one
/// issuer in the trust store successfully validates the certificate signature.
pub fn x509_verify_chain(cert: &X509Cert) -> bool {
    let count = TRUST_COUNT.load(Ordering::Acquire);
    let store = TRUST_STORE.lock();
    for i in 0..count {
        if x509_names_match(&store[i].subject, &cert.issuer)
            && x509_verify(cert, &store[i].public_key)
        {
            return true;
        }
    }
    false
}

/// Check whether `cert` is currently valid at Unix time `unix_time`.
#[inline]
pub fn x509_is_valid_at(cert: &X509Cert, unix_time: u64) -> bool {
    cert.not_before <= unix_time && unix_time <= cert.not_after
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

/// Verify `cert`'s signature against `issuer_key`.
///
/// Only sha256WithRSAEncryption is supported.  Returns `false` for any other
/// algorithm or if the key is not a valid 2048-bit RSA key.
pub fn x509_verify(cert: &X509Cert, issuer_key: &RsaPublicKeyFixed) -> bool {
    if !cert.sig_algorithm_is_sha256rsa {
        return false;
    }
    rsa_pkcs1_verify_sha256_fixed(issuer_key, &cert.signature, &cert.tbs_hash)
}

// ---------------------------------------------------------------------------
// Top-level parser
// ---------------------------------------------------------------------------

/// Parse a DER-encoded X.509 certificate.
/// Returns `Some(cert)` on success with `cert.valid == true`, or `None` on
/// structural parse failure.
pub fn x509_parse(der: &[u8]) -> Option<X509Cert> {
    let mut cert = X509Cert::empty();

    // Outer SEQUENCE
    let mut p = Asn1Der::new(der);
    let (tag, cert_val) = p.read_tlv()?;
    if tag != TAG_SEQUENCE {
        return None;
    }

    let mut cp = Asn1Der::new(cert_val);

    // -- TBSCertificate --
    // We need its DER bytes (for hashing) so we record the offset/length.
    let tbs_start = cp.position();
    let (tbs_tag, tbs_val) = cp.read_tlv()?;
    let tbs_end = cp.position();
    if tbs_tag != TAG_SEQUENCE {
        return None;
    }
    // Hash the TBSCertificate DER bytes (tag+length+value) for signature check
    let tbs_der = &cert_val[tbs_start..tbs_end];
    cert.tbs_hash = super::sha256::hash(tbs_der);

    if !parse_tbs_certificate(tbs_val, &mut cert) {
        return None;
    }

    // -- Outer signatureAlgorithm --
    let (sa_tag, sa_val) = cp.read_tlv()?;
    if sa_tag == TAG_SEQUENCE {
        let mut sp = Asn1Der::new(sa_val);
        if let Some((oid_tag, oid_val)) = sp.read_tlv() {
            if oid_tag == TAG_OID {
                cert.sig_algorithm_is_sha256rsa = oid_val == OID_SHA256_WITH_RSA.as_slice();
            }
        }
    }

    // -- signatureValue BIT STRING --
    let (sig_tag, sig_val) = cp.read_tlv()?;
    if sig_tag == TAG_BIT_STRING && !sig_val.is_empty() {
        // First byte is the "unused bits" count — skip it
        let raw_sig = &sig_val[1..];
        if raw_sig.len() <= 256 {
            let offset = 256usize.saturating_sub(raw_sig.len());
            cert.signature[offset..].copy_from_slice(raw_sig);
        }
    }

    cert.valid = true;
    Some(cert)
}

// ---------------------------------------------------------------------------
// TBSCertificate field parser
// ---------------------------------------------------------------------------

/// Parse the TBSCertificate SEQUENCE value bytes into `cert`.
fn parse_tbs_certificate(tbs: &[u8], cert: &mut X509Cert) -> bool {
    let mut p = Asn1Der::new(tbs);

    // Version [0] EXPLICIT INTEGER (optional; default v1)
    cert.version = 1;
    match p.peek_tag() {
        Some(TAG_CONTEXT_0) => {
            if let Some((_, ver_val)) = p.read_tlv() {
                let mut vp = Asn1Der::new(ver_val);
                if let Some((TAG_INTEGER, int_val)) = vp.read_tlv() {
                    if !int_val.is_empty() {
                        cert.version = int_val[0].saturating_add(1); // 0=v1, 1=v2, 2=v3
                    }
                }
            }
        }
        _ => {}
    }

    // serialNumber INTEGER
    let serial_pair = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if serial_pair.0 != TAG_INTEGER {
        return false;
    }
    let sval = serial_pair.1;
    let copy_len = sval.len().min(20);
    cert.serial[..copy_len].copy_from_slice(&sval[..copy_len]);
    cert.serial_len = copy_len;

    // signature AlgorithmIdentifier (inside TBS, should match outer)
    let (sig_alg_tag, sig_alg_val) = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if sig_alg_tag == TAG_SEQUENCE {
        let mut ap = Asn1Der::new(sig_alg_val);
        if let Some((oid_tag, oid_val)) = ap.read_tlv() {
            if oid_tag == TAG_OID {
                cert.sig_algorithm_is_sha256rsa = oid_val == OID_SHA256_WITH_RSA.as_slice();
            }
        }
    }

    // issuer Name
    let (iss_tag, iss_val) = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if iss_tag != TAG_SEQUENCE {
        return false;
    }
    cert.issuer = parse_name(iss_val);

    // validity Validity
    let (val_tag, val_val) = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if val_tag != TAG_SEQUENCE {
        return false;
    }
    {
        let mut vp = Asn1Der::new(val_val);
        if let Some((t, v)) = vp.read_tlv() {
            cert.not_before = if t == TAG_UTC_TIME {
                parse_utc_time(v)
            } else if t == TAG_GENERALIZED_TIME {
                parse_generalized_time(v)
            } else {
                0
            };
        }
        if let Some((t, v)) = vp.read_tlv() {
            cert.not_after = if t == TAG_UTC_TIME {
                parse_utc_time(v)
            } else if t == TAG_GENERALIZED_TIME {
                parse_generalized_time(v)
            } else {
                0
            };
        }
    }

    // subject Name
    let (sub_tag, sub_val) = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if sub_tag != TAG_SEQUENCE {
        return false;
    }
    cert.subject = parse_name(sub_val);

    // subjectPublicKeyInfo
    let (spki_tag, spki_val) = match p.read_tlv() {
        Some(v) => v,
        None => return false,
    };
    if spki_tag != TAG_SEQUENCE {
        return false;
    }
    {
        let mut sp = Asn1Der::new(spki_val);
        // algorithm AlgorithmIdentifier
        let _ = sp.read_tlv(); // skip it
                               // subjectPublicKey BIT STRING
        if let Some((bs_tag, bs_val)) = sp.read_tlv() {
            if bs_tag == TAG_BIT_STRING && !bs_val.is_empty() {
                // bs_val[0] = unused bits byte; rest is DER SEQUENCE { n, e }
                if let Some(pk) = rsa_parse_public_key_fixed(&bs_val[1..]) {
                    cert.public_key = pk;
                }
            }
        }
    }

    // Optional extensions — skip issuerUniqueID [1], subjectUniqueID [2]
    // then look for extensions [3]
    while p.remaining() > 0 {
        match p.peek_tag() {
            Some(TAG_CONTEXT_3) => {
                if let Some((_, ext_wrapper)) = p.read_tlv() {
                    // Extensions wrapper contains a SEQUENCE OF Extension
                    if let Some((seq_tag, seq_val)) = Asn1Der::new(ext_wrapper).read_tlv() {
                        if seq_tag == TAG_SEQUENCE {
                            parse_extensions(seq_val, cert);
                        }
                    }
                }
            }
            _ => {
                // Skip issuerUniqueID / subjectUniqueID / other unknowns
                p.skip_tlv();
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------

/// OID for Basic Constraints: 2.5.29.19 → raw bytes 0x55,0x1D,0x13
const OID_BASIC_CONSTRAINTS: [u8; 3] = [0x55, 0x1D, 0x13];
/// OID for Key Usage: 2.5.29.15 → raw bytes 0x55,0x1D,0x0F
const OID_KEY_USAGE: [u8; 3] = [0x55, 0x1D, 0x0F];

/// Parse a SEQUENCE OF Extension into the cert struct.
fn parse_extensions(exts: &[u8], cert: &mut X509Cert) {
    let mut ep = Asn1Der::new(exts);
    while ep.remaining() > 0 {
        let ext_pair = match ep.read_tlv() {
            Some(pair) => pair,
            None => break,
        };
        if ext_pair.0 != TAG_SEQUENCE {
            continue;
        }
        let mut fp = Asn1Der::new(ext_pair.1);

        // extnID OID
        let oid_pair = match fp.read_tlv() {
            Some(p) => p,
            None => continue,
        };
        if oid_pair.0 != TAG_OID {
            continue;
        }
        let oid = oid_pair.1;

        // critical BOOLEAN (optional) — skip if present
        let next = match fp.peek_tag() {
            Some(TAG_BOOLEAN) => {
                fp.skip_tlv();
                fp.read_tlv()
            }
            _ => fp.read_tlv(),
        };
        let oct_pair = match next {
            Some(p) => p,
            None => continue,
        };
        if oct_pair.0 != TAG_OCTET_STRING {
            continue;
        }

        if oid == OID_BASIC_CONSTRAINTS.as_slice() {
            // BasicConstraints ::= SEQUENCE { cA BOOLEAN DEFAULT FALSE, ... }
            let mut bp = Asn1Der::new(oct_pair.1);
            if let Some((TAG_SEQUENCE, bc_val)) = bp.read_tlv() {
                let mut bcp = Asn1Der::new(bc_val);
                if let Some((TAG_BOOLEAN, ca_val)) = bcp.read_tlv() {
                    cert.is_ca = ca_val.first().copied().unwrap_or(0) != 0;
                }
            }
        } else if oid == OID_KEY_USAGE.as_slice() {
            // KeyUsage ::= BIT STRING
            let mut kp = Asn1Der::new(oct_pair.1);
            if let Some((TAG_BIT_STRING, ku_val)) = kp.read_tlv() {
                if ku_val.len() >= 2 {
                    // ku_val[0] = unused bits, ku_val[1] = first usage byte
                    let unused = ku_val[0] as u16;
                    let b0 = ku_val[1] as u16;
                    let b1 = if ku_val.len() >= 3 {
                        ku_val[2] as u16
                    } else {
                        0
                    };
                    // Shift out unused bits from the last byte
                    cert.key_usage = ((b0 << 8) | b1) >> unused;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Name parser
// ---------------------------------------------------------------------------

/// Parse an X.509 Name (SEQUENCE OF RelativeDistinguishedName) into
/// `X509Name`.
fn parse_name(data: &[u8]) -> X509Name {
    let mut name = X509Name::empty();
    let mut p = Asn1Der::new(data);

    while p.remaining() > 0 {
        // Each RDN is a SET
        let set_pair = match p.read_tlv() {
            Some(pair) if pair.0 == TAG_SET => pair,
            _ => continue,
        };
        let mut sp = Asn1Der::new(set_pair.1);

        // Each AttributeTypeAndValue is a SEQUENCE
        while sp.remaining() > 0 {
            let seq_pair = match sp.read_tlv() {
                Some(pair) if pair.0 == TAG_SEQUENCE => pair,
                _ => break,
            };
            let mut ap = Asn1Der::new(seq_pair.1);

            let oid_pair = match ap.read_tlv() {
                Some(pair) if pair.0 == TAG_OID => pair,
                _ => continue,
            };
            let val_pair = match ap.read_tlv() {
                Some(pair) => pair,
                None => continue,
            };

            // Only accept printable/UTF8/IA5 string types
            let is_string = matches!(
                val_pair.0,
                TAG_PRINTABLE_STRING | TAG_UTF8_STRING | TAG_IA5_STRING
            );
            if !is_string {
                continue;
            }

            if oid_pair.1 == OID_COMMON_NAME.as_slice() {
                let copy_len = val_pair.1.len().min(64);
                name.common_name[..copy_len].copy_from_slice(&val_pair.1[..copy_len]);
                name.cn_len = copy_len;
            } else if oid_pair.1 == OID_ORGANIZATION.as_slice() {
                let copy_len = val_pair.1.len().min(64);
                name.organization[..copy_len].copy_from_slice(&val_pair.1[..copy_len]);
                name.org_len = copy_len;
            }
        }
    }
    name
}

// ---------------------------------------------------------------------------
// Timestamp helpers — integer-only, no float
// ---------------------------------------------------------------------------

/// Decode ASCII digit pair → u64.
#[inline]
fn dec2(bytes: &[u8], off: usize) -> u64 {
    let hi = bytes.get(off).copied().unwrap_or(b'0').wrapping_sub(b'0') as u64;
    let lo = bytes
        .get(off.saturating_add(1))
        .copied()
        .unwrap_or(b'0')
        .wrapping_sub(b'0') as u64;
    hi.saturating_mul(10).saturating_add(lo)
}

/// Compute days from 1970-01-01 to the given year-month-day (integer only).
/// Month is 1-based, day is 1-based.
fn days_since_epoch(year: u64, month: u64, day: u64) -> u64 {
    // Days in each month (non-leap year)
    const MDAYS: [u64; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

    if year < 1970 || month == 0 || month > 12 || day == 0 {
        return 0;
    }

    // Years since epoch
    let y0 = year.saturating_sub(1970);
    // Leap years since 1970 (include the partial year only if month > Feb)
    // A year y is leap if y%4==0 and (y%100!=0 or y%400==0)
    let leap_before = {
        let y = year.saturating_sub(1); // count leaps up through year-1
        let since_1970_end = y.saturating_sub(1969); // years 1970..=y-1
                                                     // years divisible by 4
        let div4 = since_1970_end / 4;
        // years divisible by 100 (subtract)
        let div100 = since_1970_end / 100;
        // years divisible by 400 (add back)
        let div400 = since_1970_end / 400;
        div4.saturating_sub(div100).saturating_add(div400)
    };

    // Is `year` itself a leap year?
    let is_leap = (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0);

    // Days from complete years
    let mut days = y0.saturating_mul(365).saturating_add(leap_before);

    // Days from complete months within this year
    for m in 1..month {
        let extra = if m == 2 && is_leap { 1u64 } else { 0u64 };
        days = days.saturating_add(MDAYS[m as usize]).saturating_add(extra);
    }

    // Days from the day-of-month (day 1 = epoch day 0 within the month)
    days = days.saturating_add(day.saturating_sub(1));
    days
}

/// Parse UTCTime "YYMMDDHHMMSSZ" → Unix timestamp.
/// Two-digit years: 00-49 → 2000-2049, 50-99 → 1950-1999.
fn parse_utc_time(data: &[u8]) -> u64 {
    if data.len() < 12 {
        return 0;
    }
    let yy = dec2(data, 0);
    let month = dec2(data, 2);
    let day = dec2(data, 4);
    let hour = dec2(data, 6);
    let min = dec2(data, 8);
    let sec = dec2(data, 10);

    let year = if yy < 50 {
        2000u64.saturating_add(yy)
    } else {
        1900u64.saturating_add(yy)
    };
    let days = days_since_epoch(year, month, day);
    days.saturating_mul(86400)
        .saturating_add(hour.saturating_mul(3600))
        .saturating_add(min.saturating_mul(60))
        .saturating_add(sec)
}

/// Parse GeneralizedTime "YYYYMMDDHHMMSSZ" → Unix timestamp.
fn parse_generalized_time(data: &[u8]) -> u64 {
    if data.len() < 14 {
        return 0;
    }
    // Four-digit year
    let y0 = dec2(data, 0).saturating_mul(100);
    let y1 = dec2(data, 2);
    let year = y0.saturating_add(y1);
    let month = dec2(data, 4);
    let day = dec2(data, 6);
    let hour = dec2(data, 8);
    let min = dec2(data, 10);
    let sec = dec2(data, 12);

    let days = days_since_epoch(year, month, day);
    days.saturating_mul(86400)
        .saturating_add(hour.saturating_mul(3600))
        .saturating_add(min.saturating_mul(60))
        .saturating_add(sec)
}

// ---------------------------------------------------------------------------
// Name equality helper
// ---------------------------------------------------------------------------

/// Compare two X509Names byte-for-byte (used for issuer/subject matching).
fn x509_names_match(a: &X509Name, b: &X509Name) -> bool {
    if a.cn_len != b.cn_len || a.org_len != b.org_len {
        return false;
    }
    // Common name comparison
    let mut diff: u8 = 0;
    for i in 0..a.cn_len {
        diff |= a.common_name[i] ^ b.common_name[i];
    }
    for i in 0..a.org_len {
        diff |= a.organization[i] ^ b.organization[i];
    }
    diff == 0
}
