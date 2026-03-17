use crate::sync::Mutex;
/// PKI / X.509 Certificate Infrastructure
///
/// Minimal X.509 certificate parsing, validation, and chain-of-trust
/// verification for a bare-metal kernel. Supports:
///   - DER/ASN.1 parsing (TLV structure)
///   - X.509 v3 certificate field extraction
///   - Certificate chain validation
///   - Basic CRL (Certificate Revocation List) checking
///   - OCSP stapling response validation
///
/// Used for: TLS certificate verification, code signing, secure boot chain.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Maximum certificate chain depth
const MAX_CHAIN_DEPTH: usize = 10;

/// Global trusted root certificate store
static ROOT_STORE: Mutex<Option<CertificateStore>> = Mutex::new(None);

/// ASN.1 tag constants
mod asn1_tags {
    pub const BOOLEAN: u8 = 0x01;
    pub const INTEGER: u8 = 0x02;
    pub const BIT_STRING: u8 = 0x03;
    pub const OCTET_STRING: u8 = 0x04;
    pub const NULL: u8 = 0x05;
    pub const OID: u8 = 0x06;
    pub const UTF8_STRING: u8 = 0x0C;
    pub const PRINTABLE_STRING: u8 = 0x13;
    pub const IA5_STRING: u8 = 0x16;
    pub const UTC_TIME: u8 = 0x17;
    pub const GENERALIZED_TIME: u8 = 0x18;
    pub const SEQUENCE: u8 = 0x30;
    pub const SET: u8 = 0x31;
    pub const CONTEXT_0: u8 = 0xA0;
    pub const CONTEXT_1: u8 = 0xA1;
    pub const CONTEXT_3: u8 = 0xA3;
}

/// ASN.1 Tag-Length-Value parser
#[derive(Clone)]
struct Asn1Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Parsed ASN.1 TLV element
#[derive(Clone)]
struct Asn1Tlv<'a> {
    tag: u8,
    value: &'a [u8],
}

impl<'a> Asn1Parser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Asn1Parser { data, pos: 0 }
    }

    /// Read next TLV element
    fn read_tlv(&mut self) -> Option<Asn1Tlv<'a>> {
        if self.pos >= self.data.len() {
            return None;
        }

        let tag = self.data[self.pos];
        self.pos += 1;

        let length = self.read_length()?;
        if self.pos + length > self.data.len() {
            return None;
        }

        let value = &self.data[self.pos..self.pos + length];
        self.pos += length;

        Some(Asn1Tlv { tag, value })
    }

    /// Read ASN.1 length encoding (supports definite short and long forms)
    fn read_length(&mut self) -> Option<usize> {
        if self.pos >= self.data.len() {
            return None;
        }

        let first = self.data[self.pos];
        self.pos += 1;

        if first < 0x80 {
            // Short form
            Some(first as usize)
        } else if first == 0x80 {
            // Indefinite length (not supported in DER)
            None
        } else {
            // Long form
            let num_bytes = (first & 0x7F) as usize;
            if num_bytes > 4 || self.pos + num_bytes > self.data.len() {
                return None;
            }
            let mut length: usize = 0;
            for i in 0..num_bytes {
                length = (length << 8) | (self.data[self.pos + i] as usize);
            }
            self.pos += num_bytes;
            Some(length)
        }
    }

    fn remaining(&self) -> usize {
        if self.pos >= self.data.len() {
            0
        } else {
            self.data.len() - self.pos
        }
    }
}

/// Parse an OID from DER-encoded bytes into dotted string form
fn parse_oid(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }

    let mut components = Vec::new();
    // First byte encodes first two components: val = 40*x + y
    components.push((data[0] / 40) as u32);
    components.push((data[0] % 40) as u32);

    let mut value: u32 = 0;
    for &byte in &data[1..] {
        value = (value << 7) | ((byte & 0x7F) as u32);
        if byte & 0x80 == 0 {
            components.push(value);
            value = 0;
        }
    }

    let mut s = String::new();
    for (i, &c) in components.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        // Simple integer-to-string conversion
        if c == 0 {
            s.push('0');
        } else {
            let mut digits = Vec::new();
            let mut n = c;
            while n > 0 {
                digits.push((b'0' + (n % 10) as u8) as char);
                n /= 10;
            }
            for d in digits.iter().rev() {
                s.push(*d);
            }
        }
    }
    s
}

/// Signature algorithm identifiers
#[derive(Clone, Copy, PartialEq)]
pub enum SignatureAlgorithm {
    RsaSha256,
    RsaSha384,
    RsaSha512,
    EcdsaSha256,
    Ed25519,
    Unknown,
}

/// Validity period
#[derive(Clone)]
pub struct Validity {
    pub not_before: u64, // Unix timestamp
    pub not_after: u64,  // Unix timestamp
}

/// Distinguished Name (simplified)
#[derive(Clone)]
pub struct DistinguishedName {
    pub common_name: String,
    pub organization: String,
    pub country: String,
    pub raw: Vec<u8>,
}

/// X.509 v3 Certificate (parsed)
#[derive(Clone)]
pub struct Certificate {
    pub version: u8,
    pub serial_number: Vec<u8>,
    pub signature_algorithm: SignatureAlgorithm,
    pub issuer: DistinguishedName,
    pub subject: DistinguishedName,
    pub validity: Validity,
    pub public_key_algorithm: SignatureAlgorithm,
    pub public_key: Vec<u8>,
    pub is_ca: bool,
    pub max_path_length: Option<u8>,
    pub signature: Vec<u8>,
    pub tbs_certificate: Vec<u8>, // To-Be-Signed portion (for verification)
    pub raw: Vec<u8>,
}

/// Certificate store for trusted roots
pub struct CertificateStore {
    trusted_roots: Vec<Certificate>,
    revoked_serials: Vec<Vec<u8>>,
}

impl CertificateStore {
    pub fn new() -> Self {
        CertificateStore {
            trusted_roots: Vec::new(),
            revoked_serials: Vec::new(),
        }
    }

    /// Add a trusted root certificate
    pub fn add_trusted_root(&mut self, cert: Certificate) {
        self.trusted_roots.push(cert);
    }

    /// Add a revoked certificate serial number
    pub fn add_revoked_serial(&mut self, serial: Vec<u8>) {
        self.revoked_serials.push(serial);
    }

    /// Check if a certificate serial is revoked
    pub fn is_revoked(&self, serial: &[u8]) -> bool {
        self.revoked_serials.iter().any(|s| s.as_slice() == serial)
    }

    /// Find a trusted root by subject name match
    pub fn find_issuer(&self, issuer: &DistinguishedName) -> Option<&Certificate> {
        self.trusted_roots.iter().find(|cert| {
            cert.subject.common_name == issuer.common_name
                && cert.subject.organization == issuer.organization
        })
    }
}

/// Parse a Distinguished Name from DER SEQUENCE
fn parse_dn(data: &[u8]) -> DistinguishedName {
    let mut dn = DistinguishedName {
        common_name: String::new(),
        organization: String::new(),
        country: String::new(),
        raw: data.to_vec(),
    };

    let mut parser = Asn1Parser::new(data);
    while let Some(set_tlv) = parser.read_tlv() {
        if set_tlv.tag != asn1_tags::SET {
            continue;
        }
        let mut set_parser = Asn1Parser::new(set_tlv.value);
        if let Some(seq_tlv) = set_parser.read_tlv() {
            if seq_tlv.tag != asn1_tags::SEQUENCE {
                continue;
            }
            let mut seq_parser = Asn1Parser::new(seq_tlv.value);
            if let Some(oid_tlv) = seq_parser.read_tlv() {
                if oid_tlv.tag != asn1_tags::OID {
                    continue;
                }
                let oid = parse_oid(oid_tlv.value);
                if let Some(val_tlv) = seq_parser.read_tlv() {
                    let value = core::str::from_utf8(val_tlv.value).unwrap_or("");
                    let val_string = String::from(value);
                    // Common OIDs for DN attributes
                    if oid.as_str() == "2.5.4.3" {
                        dn.common_name = val_string;
                    } else if oid.as_str() == "2.5.4.10" {
                        dn.organization = val_string;
                    } else if oid.as_str() == "2.5.4.6" {
                        dn.country = val_string;
                    }
                }
            }
        }
    }
    dn
}

/// Identify signature algorithm from OID
fn identify_sig_algorithm(oid: &str) -> SignatureAlgorithm {
    match oid {
        "1.2.840.113549.1.1.11" => SignatureAlgorithm::RsaSha256,
        "1.2.840.113549.1.1.12" => SignatureAlgorithm::RsaSha384,
        "1.2.840.113549.1.1.13" => SignatureAlgorithm::RsaSha512,
        "1.2.840.10045.4.3.2" => SignatureAlgorithm::EcdsaSha256,
        "1.3.101.112" => SignatureAlgorithm::Ed25519,
        _ => SignatureAlgorithm::Unknown,
    }
}

/// Parse UTC time (YYMMDDHHMMSSZ) to unix timestamp (simplified)
fn parse_utc_time(data: &[u8]) -> u64 {
    if data.len() < 12 {
        return 0;
    }
    let s = core::str::from_utf8(data).unwrap_or("");
    let year = parse_decimal(&s[0..2]) as u64;
    let month = parse_decimal(&s[2..4]) as u64;
    let day = parse_decimal(&s[4..6]) as u64;
    let hour = parse_decimal(&s[6..8]) as u64;
    let min = parse_decimal(&s[8..10]) as u64;
    let sec = parse_decimal(&s[10..12]) as u64;

    // Adjust year (00-49 = 2000s, 50-99 = 1900s)
    let full_year = if year < 50 { 2000 + year } else { 1900 + year };

    // Simplified timestamp calculation (days from epoch)
    let days_from_years = (full_year - 1970) * 365 + (full_year - 1969) / 4;
    let month_days: [u64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let day_of_year = if month > 0 && month <= 12 {
        month_days[(month - 1) as usize] + day - 1
    } else {
        0
    };

    (days_from_years + day_of_year) * 86400 + hour * 3600 + min * 60 + sec
}

/// Parse a 2-digit decimal string
fn parse_decimal(s: &str) -> u32 {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let d0 = (bytes[0].wrapping_sub(b'0')) as u32;
        let d1 = (bytes[1].wrapping_sub(b'0')) as u32;
        d0 * 10 + d1
    } else {
        0
    }
}

/// Parse an X.509 certificate from DER-encoded bytes
pub fn parse_certificate(der: &[u8]) -> Option<Certificate> {
    let mut outer = Asn1Parser::new(der);
    let cert_seq = outer.read_tlv()?;
    if cert_seq.tag != asn1_tags::SEQUENCE {
        return None;
    }

    let mut cert_parser = Asn1Parser::new(cert_seq.value);

    // TBSCertificate
    let tbs_tlv = cert_parser.read_tlv()?;
    if tbs_tlv.tag != asn1_tags::SEQUENCE {
        return None;
    }
    let tbs_data = tbs_tlv.value;
    let mut tbs_parser = Asn1Parser::new(tbs_data);

    // Version (optional, context-tagged [0])
    let mut version: u8 = 0; // v1 default
    let first = tbs_parser.read_tlv()?;
    let serial_tlv;

    if first.tag == asn1_tags::CONTEXT_0 {
        // Version is present
        let mut ver_parser = Asn1Parser::new(first.value);
        if let Some(ver_int) = ver_parser.read_tlv() {
            if !ver_int.value.is_empty() {
                version = ver_int.value[0];
            }
        }
        serial_tlv = tbs_parser.read_tlv()?;
    } else {
        serial_tlv = first;
    }

    // Serial number
    let serial_number = serial_tlv.value.to_vec();

    // Signature algorithm
    let sig_alg_tlv = tbs_parser.read_tlv()?;
    let mut sig_alg = SignatureAlgorithm::Unknown;
    if sig_alg_tlv.tag == asn1_tags::SEQUENCE {
        let mut alg_parser = Asn1Parser::new(sig_alg_tlv.value);
        if let Some(oid_tlv) = alg_parser.read_tlv() {
            if oid_tlv.tag == asn1_tags::OID {
                sig_alg = identify_sig_algorithm(&parse_oid(oid_tlv.value));
            }
        }
    }

    // Issuer
    let issuer_tlv = tbs_parser.read_tlv()?;
    let issuer = parse_dn(issuer_tlv.value);

    // Validity
    let validity_tlv = tbs_parser.read_tlv()?;
    let mut validity = Validity {
        not_before: 0,
        not_after: 0,
    };
    if validity_tlv.tag == asn1_tags::SEQUENCE {
        let mut val_parser = Asn1Parser::new(validity_tlv.value);
        if let Some(nb_tlv) = val_parser.read_tlv() {
            if nb_tlv.tag == asn1_tags::UTC_TIME || nb_tlv.tag == asn1_tags::GENERALIZED_TIME {
                validity.not_before = parse_utc_time(nb_tlv.value);
            }
        }
        if let Some(na_tlv) = val_parser.read_tlv() {
            if na_tlv.tag == asn1_tags::UTC_TIME || na_tlv.tag == asn1_tags::GENERALIZED_TIME {
                validity.not_after = parse_utc_time(na_tlv.value);
            }
        }
    }

    // Subject
    let subject_tlv = tbs_parser.read_tlv()?;
    let subject = parse_dn(subject_tlv.value);

    // Subject Public Key Info
    let spki_tlv = tbs_parser.read_tlv()?;
    let mut pk_algorithm = SignatureAlgorithm::Unknown;
    let mut public_key = Vec::new();
    if spki_tlv.tag == asn1_tags::SEQUENCE {
        let mut spki_parser = Asn1Parser::new(spki_tlv.value);
        if let Some(pk_alg_tlv) = spki_parser.read_tlv() {
            if pk_alg_tlv.tag == asn1_tags::SEQUENCE {
                let mut pk_alg_parser = Asn1Parser::new(pk_alg_tlv.value);
                if let Some(oid_tlv) = pk_alg_parser.read_tlv() {
                    if oid_tlv.tag == asn1_tags::OID {
                        pk_algorithm = identify_sig_algorithm(&parse_oid(oid_tlv.value));
                    }
                }
            }
        }
        if let Some(pk_bits_tlv) = spki_parser.read_tlv() {
            if pk_bits_tlv.tag == asn1_tags::BIT_STRING && !pk_bits_tlv.value.is_empty() {
                // Skip the unused-bits byte
                public_key = pk_bits_tlv.value[1..].to_vec();
            }
        }
    }

    // Extensions (optional, context-tagged [3])
    let mut is_ca = false;
    let mut max_path_length = None;
    while tbs_parser.remaining() > 0 {
        if let Some(ext_container) = tbs_parser.read_tlv() {
            if ext_container.tag == asn1_tags::CONTEXT_3 {
                parse_extensions(ext_container.value, &mut is_ca, &mut max_path_length);
            }
        }
    }

    // Signature algorithm (outer, should match inner)
    let _outer_sig_alg = cert_parser.read_tlv()?;

    // Signature value
    let sig_tlv = cert_parser.read_tlv()?;
    let signature = if sig_tlv.tag == asn1_tags::BIT_STRING && !sig_tlv.value.is_empty() {
        sig_tlv.value[1..].to_vec() // Skip unused-bits byte
    } else {
        sig_tlv.value.to_vec()
    };

    Some(Certificate {
        version,
        serial_number,
        signature_algorithm: sig_alg,
        issuer,
        subject,
        validity,
        public_key_algorithm: pk_algorithm,
        public_key,
        is_ca,
        max_path_length,
        signature,
        tbs_certificate: tbs_data.to_vec(),
        raw: der.to_vec(),
    })
}

/// Parse X.509 v3 extensions
fn parse_extensions(data: &[u8], is_ca: &mut bool, max_path: &mut Option<u8>) {
    let mut parser = Asn1Parser::new(data);
    let seq_tlv = match parser.read_tlv() {
        Some(t) if t.tag == asn1_tags::SEQUENCE => t,
        _ => return,
    };

    let mut ext_parser = Asn1Parser::new(seq_tlv.value);
    while let Some(ext_tlv) = ext_parser.read_tlv() {
        if ext_tlv.tag != asn1_tags::SEQUENCE {
            continue;
        }
        let mut inner = Asn1Parser::new(ext_tlv.value);

        let oid_tlv = match inner.read_tlv() {
            Some(t) if t.tag == asn1_tags::OID => t,
            _ => continue,
        };
        let oid = parse_oid(oid_tlv.value);

        // Skip critical flag if present
        let next = match inner.read_tlv() {
            Some(t) => t,
            None => continue,
        };

        let ext_value = if next.tag == asn1_tags::BOOLEAN {
            match inner.read_tlv() {
                Some(t) => t,
                None => continue,
            }
        } else {
            next
        };

        // Basic Constraints (2.5.29.19)
        if oid.as_str() == "2.5.29.19" && ext_value.tag == asn1_tags::OCTET_STRING {
            let mut bc_parser = Asn1Parser::new(ext_value.value);
            if let Some(bc_seq) = bc_parser.read_tlv() {
                if bc_seq.tag == asn1_tags::SEQUENCE {
                    let mut bc_inner = Asn1Parser::new(bc_seq.value);
                    if let Some(ca_tlv) = bc_inner.read_tlv() {
                        if ca_tlv.tag == asn1_tags::BOOLEAN && !ca_tlv.value.is_empty() {
                            *is_ca = ca_tlv.value[0] != 0;
                        }
                    }
                    if let Some(path_tlv) = bc_inner.read_tlv() {
                        if path_tlv.tag == asn1_tags::INTEGER && !path_tlv.value.is_empty() {
                            *max_path = Some(path_tlv.value[0]);
                        }
                    }
                }
            }
        }
    }
}

/// Validate a certificate chain (leaf -> intermediates -> root)
pub fn validate_chain(chain: &[Certificate], current_time: u64) -> CertValidationResult {
    if chain.is_empty() {
        return CertValidationResult::EmptyChain;
    }
    if chain.len() > MAX_CHAIN_DEPTH {
        return CertValidationResult::ChainTooDeep;
    }

    let store = ROOT_STORE.lock();
    let store = match store.as_ref() {
        Some(s) => s,
        None => return CertValidationResult::NoTrustAnchors,
    };

    // Validate each certificate in the chain
    for (i, cert) in chain.iter().enumerate() {
        // Check validity period
        if current_time < cert.validity.not_before {
            return CertValidationResult::NotYetValid;
        }
        if current_time > cert.validity.not_after {
            return CertValidationResult::Expired;
        }

        // Check revocation
        if store.is_revoked(&cert.serial_number) {
            return CertValidationResult::Revoked;
        }

        // All but leaf must be CA
        if i > 0 && !cert.is_ca {
            return CertValidationResult::NotCA;
        }

        // Check path length constraint
        if let Some(max_path) = cert.max_path_length {
            if i > 0 && (i - 1) as u8 > max_path {
                return CertValidationResult::PathLengthExceeded;
            }
        }

        // Verify signature against issuer (next cert or root store)
        let issuer = if i + 1 < chain.len() {
            Some(&chain[i + 1])
        } else {
            store.find_issuer(&cert.issuer)
        };

        match issuer {
            Some(issuer_cert) => {
                if !verify_certificate_signature(cert, issuer_cert) {
                    return CertValidationResult::SignatureInvalid;
                }
            }
            None => {
                // Self-signed root check
                if cert.subject.common_name == cert.issuer.common_name {
                    if !verify_certificate_signature(cert, cert) {
                        return CertValidationResult::SignatureInvalid;
                    }
                } else {
                    return CertValidationResult::UntrustedRoot;
                }
            }
        }
    }

    CertValidationResult::Valid
}

/// Certificate validation results
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CertValidationResult {
    Valid,
    EmptyChain,
    ChainTooDeep,
    NoTrustAnchors,
    NotYetValid,
    Expired,
    Revoked,
    NotCA,
    PathLengthExceeded,
    SignatureInvalid,
    UntrustedRoot,
    UnsupportedAlgorithm,
}

/// Verify a certificate's signature using the issuer's public key
fn verify_certificate_signature(cert: &Certificate, issuer: &Certificate) -> bool {
    let tbs_hash = super::sha256::hash(&cert.tbs_certificate);

    match cert.signature_algorithm {
        SignatureAlgorithm::RsaSha256 => {
            // RSA-PKCS#1-v1.5 signature verification using the issuer's SubjectPublicKeyInfo.
            // The issuer's DER-encoded public key (from SPKI BIT STRING) contains the
            // RSA public key in DER SEQUENCE { INTEGER n, INTEGER e } form.
            // We delegate to the rsa module which handles PKCS#1 v1.5 verify.
            if issuer.public_key.len() < 4 {
                return false;
            }
            // Parse RSA public key from DER: SEQUENCE { INTEGER n, INTEGER e }
            let pk = match super::rsa::RsaPublicKey::from_der(&issuer.public_key) {
                Some(k) => k,
                None => return false,
            };
            super::rsa::rsa_verify(&pk, &cert.tbs_certificate, &cert.signature)
        }
        SignatureAlgorithm::Ed25519 => {
            if issuer.public_key.len() != 32 || cert.signature.len() != 64 {
                return false;
            }
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&issuer.public_key);
            let mut sig = [0u8; 64];
            sig.copy_from_slice(&cert.signature);
            super::ed25519::verify(&pk, &cert.tbs_certificate, &sig)
        }
        _ => false,
    }
}

/// OCSP response status
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum OcspStatus {
    Good,
    Revoked,
    Unknown,
    MalformedResponse,
}

/// Parse an OCSP response (simplified, DER-encoded)
pub fn parse_ocsp_response(data: &[u8]) -> OcspStatus {
    if data.len() < 4 {
        return OcspStatus::MalformedResponse;
    }

    let mut parser = Asn1Parser::new(data);
    let outer = match parser.read_tlv() {
        Some(t) if t.tag == asn1_tags::SEQUENCE => t,
        _ => return OcspStatus::MalformedResponse,
    };

    let mut inner = Asn1Parser::new(outer.value);

    // Response status (ENUMERATED)
    let status_tlv = match inner.read_tlv() {
        Some(t) => t,
        None => return OcspStatus::MalformedResponse,
    };

    if status_tlv.tag == 0x0A && !status_tlv.value.is_empty() {
        // OCSPResponseStatus: 0 = successful
        if status_tlv.value[0] != 0 {
            return OcspStatus::MalformedResponse;
        }
    }

    // Parse response bytes to find cert status
    // Look for the SingleResponse certStatus field
    // 0x80 (context 0) = good, 0x81 (context 1) = revoked, 0x82 (context 2) = unknown
    let response_data = outer.value;
    for i in 0..response_data.len().saturating_sub(1) {
        match response_data[i] {
            0x80 => return OcspStatus::Good,
            0xA1 => return OcspStatus::Revoked,
            0x82 => return OcspStatus::Unknown,
            _ => {}
        }
    }

    OcspStatus::Unknown
}

/// Parse a CRL (Certificate Revocation List) and add revoked serials to the store
pub fn process_crl(crl_der: &[u8]) -> usize {
    let mut count = 0;
    let mut parser = Asn1Parser::new(crl_der);
    let outer = match parser.read_tlv() {
        Some(t) if t.tag == asn1_tags::SEQUENCE => t,
        _ => return 0,
    };

    let mut crl_parser = Asn1Parser::new(outer.value);
    // TBSCertList
    let tbs = match crl_parser.read_tlv() {
        Some(t) if t.tag == asn1_tags::SEQUENCE => t,
        _ => return 0,
    };

    let mut tbs_parser = Asn1Parser::new(tbs.value);

    // Skip version, signature algorithm, issuer, thisUpdate, nextUpdate
    for _ in 0..5 {
        let _ = tbs_parser.read_tlv();
    }

    // Revoked certificates (SEQUENCE OF)
    if let Some(revoked_seq) = tbs_parser.read_tlv() {
        if revoked_seq.tag == asn1_tags::SEQUENCE {
            let mut rev_parser = Asn1Parser::new(revoked_seq.value);
            while let Some(entry) = rev_parser.read_tlv() {
                if entry.tag == asn1_tags::SEQUENCE {
                    let mut entry_parser = Asn1Parser::new(entry.value);
                    if let Some(serial_tlv) = entry_parser.read_tlv() {
                        if serial_tlv.tag == asn1_tags::INTEGER {
                            let serial = serial_tlv.value.to_vec();
                            let mut store = ROOT_STORE.lock();
                            if let Some(ref mut s) = *store {
                                s.add_revoked_serial(serial);
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    count
}

/// Initialize the PKI subsystem with an empty certificate store
pub fn init() {
    let mut store = ROOT_STORE.lock();
    *store = Some(CertificateStore::new());
    serial_println!("    [pki] X.509/PKI certificate infrastructure ready");
}

/// Add a trusted root certificate (DER-encoded)
pub fn add_trusted_root(der: &[u8]) -> bool {
    if let Some(cert) = parse_certificate(der) {
        let mut store = ROOT_STORE.lock();
        if let Some(ref mut s) = *store {
            serial_println!("    [pki] Added trusted root: {}", cert.subject.common_name);
            s.add_trusted_root(cert);
            return true;
        }
    }
    false
}
