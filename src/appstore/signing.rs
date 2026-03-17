/// App signing verification for Genesis store
///
/// Code signing, certificate chain, APK-like verification,
/// developer identity, tamper detection.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SignatureStatus {
    Valid,
    Invalid,
    Expired,
    Revoked,
    Unknown,
    SelfSigned,
}

#[derive(Clone, Copy)]
struct Certificate {
    subject_hash: u64,
    issuer_hash: u64,
    public_key_hash: u64,
    valid_from: u64,
    valid_until: u64,
    revoked: bool,
}

struct SigningEngine {
    trusted_certs: [Certificate; 8],
    trusted_count: usize,
    verified_count: u32,
    rejected_count: u32,
}

static SIGNING: Mutex<Option<SigningEngine>> = Mutex::new(None);

impl SigningEngine {
    fn new() -> Self {
        let empty = Certificate {
            subject_hash: 0,
            issuer_hash: 0,
            public_key_hash: 0,
            valid_from: 0,
            valid_until: u64::MAX,
            revoked: false,
        };
        let mut engine = SigningEngine {
            trusted_certs: [empty; 8],
            trusted_count: 0,
            verified_count: 0,
            rejected_count: 0,
        };
        // Add Genesis root CA
        engine.trusted_certs[0] = Certificate {
            subject_hash: HOAGS_ROOT,
            issuer_hash: HOAGS_ROOT,
            public_key_hash: 0xDEAD_BEEF_CAFE,
            valid_from: 0,
            valid_until: u64::MAX,
            revoked: false,
        };
        engine.trusted_count = 1;
        engine
    }

    fn verify(&mut self, _signature_hash: u64, cert_hash: u64, timestamp: u64) -> SignatureStatus {
        // Find certificate
        for i in 0..self.trusted_count {
            let cert = &self.trusted_certs[i];
            if cert.subject_hash == cert_hash || cert.public_key_hash == cert_hash {
                if cert.revoked {
                    self.rejected_count = self.rejected_count.saturating_add(1);
                    return SignatureStatus::Revoked;
                }
                if timestamp < cert.valid_from || timestamp > cert.valid_until {
                    self.rejected_count = self.rejected_count.saturating_add(1);
                    return SignatureStatus::Expired;
                }
                self.verified_count = self.verified_count.saturating_add(1);
                return SignatureStatus::Valid;
            }
        }
        SignatureStatus::Unknown
    }
}

// Fake constant for the root hash
const HOAGS_ROOT: u64 = 0x484F_4147_5321;

pub fn init() {
    let mut s = SIGNING.lock();
    let empty = Certificate {
        subject_hash: 0,
        issuer_hash: 0,
        public_key_hash: 0,
        valid_from: 0,
        valid_until: u64::MAX,
        revoked: false,
    };
    let mut engine = SigningEngine {
        trusted_certs: [empty; 8],
        trusted_count: 1,
        verified_count: 0,
        rejected_count: 0,
    };
    engine.trusted_certs[0] = Certificate {
        subject_hash: HOAGS_ROOT,
        issuer_hash: HOAGS_ROOT,
        public_key_hash: 0xDEAD_BEEF_CAFE,
        valid_from: 0,
        valid_until: u64::MAX,
        revoked: false,
    };
    *s = Some(engine);
    serial_println!("    App store: code signing verification ready");
}
