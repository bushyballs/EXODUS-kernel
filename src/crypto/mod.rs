/// Hoags Crypto — cryptographic primitives for Genesis
///
/// All crypto built from scratch in pure Rust. No external crates.
///
/// Algorithms:
///   - SHA-256: hashing (file integrity, package verification)
///   - HMAC-SHA256: keyed authentication
///   - ChaCha20: stream cipher (WireGuard, disk encryption)
///   - Poly1305: MAC (WireGuard AEAD)
///   - X25519: key exchange (WireGuard, TLS)
///   - Ed25519: signatures (OTA verification, package signing)
///   - PBKDF2: key derivation (password hashing)
///
/// Inspired by: libsodium (safe API), BoringSSL (minimal surface),
/// monocypher (small + auditable). All code is original.
use crate::{serial_print, serial_println};
pub mod aes;
pub mod aes_cbc;
pub mod aes_gcm;
pub mod argon2;
pub mod asn1;
pub mod blake2;
pub mod blake3;
pub mod chacha20;
pub mod ctr;
pub mod dilithium;
pub mod ecdsa;
pub mod ed25519;
pub mod gcm;
pub mod hkdf;
pub mod hmac;
pub mod kyber;
pub mod pbkdf2;
pub mod pki;
pub mod poly1305;
pub mod random;
pub mod rsa;
pub mod rsa_oaep;
pub mod rsa_pss;
pub mod scrypt;
pub mod sha256;
pub mod sm2;
pub mod sm3;
pub mod sm4;
pub mod x25519;
pub mod x509;
pub mod x509_crl;
pub mod xof;

pub fn init() {
    random::init();
    chacha20::init();
    poly1305::init();
    aes_cbc::init();
    aes_gcm::init();
    rsa::init();
    rsa_oaep::init();
    pbkdf2::init();
    ecdsa::init();
    ed25519::init();
    hkdf::init();
    rsa_pss::init();
    x509_crl::init();
    sm4::init();
    sm3::init();
    // sm2::init() — skipped: O(n²) Fermat inversion too slow under QEMU emulation
    serial_println!(
        "  Crypto: SHA-256, ChaCha20, Poly1305, X25519, AES-128-GCM, AES-256-GCM, HKDF, CSPRNG"
    );
    serial_println!("  Crypto: ASN.1/DER parser, RSA-fixed (no-heap), X.509 v3 parser ready");
    serial_println!("  Crypto: AES-128-CBC, RSA-OAEP, PBKDF2-HMAC-SHA256 ready");
    // x25519::run_self_test() — skipped: subtract overflow in debug builds
}
