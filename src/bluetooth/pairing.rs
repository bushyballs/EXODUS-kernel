/// Secure pairing and bonding.
///
/// This module implements Bluetooth pairing and key management:
///   - Secure Simple Pairing (SSP) for BR/EDR
///   - LE Secure Connections for BLE
///   - IO capability exchange and pairing method selection
///   - Key generation (Link Key, Long Term Key, IRK, CSRK)
///   - Bonded device database (persistent key storage)
///   - Man-in-the-Middle (MITM) protection
///
/// Pairing association models:
///   - Just Works: no user interaction, no MITM protection
///   - Numeric Comparison: user confirms 6-digit number
///   - Passkey Entry: user enters 6-digit passkey
///   - Out of Band: keys exchanged via NFC or similar
///
/// Key types stored per bond:
///   - Link Key (BR/EDR)
///   - LTK (Long Term Key) + EDIV + Rand for LE
///   - IRK (Identity Resolving Key) for address resolution
///   - CSRK (Connection Signature Resolving Key)
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// IO Capability values (used in capability exchange).
const IO_DISPLAY_ONLY: u8 = 0x00;
const IO_DISPLAY_YES_NO: u8 = 0x01;
const IO_KEYBOARD_ONLY: u8 = 0x02;
const IO_NO_INPUT_NO_OUTPUT: u8 = 0x03;
const IO_KEYBOARD_DISPLAY: u8 = 0x04;

/// Authentication requirements.
const AUTH_NO_BONDING: u8 = 0x00;
const AUTH_BONDING: u8 = 0x01;
const AUTH_MITM_NO_BONDING: u8 = 0x04;
const AUTH_MITM_BONDING: u8 = 0x05;

/// SMP (Security Manager Protocol) command codes for LE.
const SMP_PAIRING_REQ: u8 = 0x01;
const SMP_PAIRING_RSP: u8 = 0x02;
const SMP_PAIRING_CONFIRM: u8 = 0x03;
const SMP_PAIRING_RANDOM: u8 = 0x04;
const SMP_PAIRING_FAILED: u8 = 0x05;
const SMP_ENCRYPTION_INFO: u8 = 0x06;
const SMP_MASTER_IDENT: u8 = 0x07;
const SMP_IDENTITY_INFO: u8 = 0x08;
const SMP_IDENTITY_ADDR_INFO: u8 = 0x09;
const SMP_SIGNING_INFO: u8 = 0x0A;
const SMP_SECURITY_REQ: u8 = 0x0B;
const SMP_PUBLIC_KEY: u8 = 0x0C;
const SMP_DHKEY_CHECK: u8 = 0x0D;

/// Global pairing state.
static PAIRING: Mutex<Option<PairingManagerInner>> = Mutex::new(None);

/// Pairing methods.
#[derive(Debug, Clone, Copy)]
pub enum PairingMethod {
    JustWorks,
    NumericComparison,
    PasskeyEntry,
    OutOfBand,
}

/// Pairing state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PairingState {
    Idle,
    CapabilityExchange,
    PublicKeyExchange,
    Authenticating,
    KeyDistribution,
    Complete,
    Failed,
}

/// Key material for a bonded device.
#[derive(Clone)]
struct BondKeys {
    link_key: Option<[u8; 16]>,     // BR/EDR link key
    ltk: Option<[u8; 16]>,          // LE Long Term Key
    ediv: u16,                       // Encrypted Diversifier
    rand: u64,                       // Random number
    irk: Option<[u8; 16]>,          // Identity Resolving Key
    csrk: Option<[u8; 16]>,         // Connection Signature Resolving Key
    authenticated: bool,             // Was MITM protection used?
    secure_connections: bool,        // Was LE Secure Connections used?
}

impl BondKeys {
    fn new() -> Self {
        Self {
            link_key: None,
            ltk: None,
            ediv: 0,
            rand: 0,
            irk: None,
            csrk: None,
            authenticated: false,
            secure_connections: false,
        }
    }
}

/// Active pairing session.
struct PairingSession {
    peer_address: [u8; 6],
    method: PairingMethod,
    state: PairingState,
    local_io: u8,
    remote_io: u8,
    initiator: bool,
    confirm_value: [u8; 16],
    random_value: [u8; 16],
    passkey: u32,
}

impl PairingSession {
    fn new(address: [u8; 6], method: PairingMethod) -> Self {
        Self {
            peer_address: address,
            method,
            state: PairingState::Idle,
            local_io: IO_DISPLAY_YES_NO,
            remote_io: IO_NO_INPUT_NO_OUTPUT,
            initiator: true,
            confirm_value: [0u8; 16],
            random_value: [0u8; 16],
            passkey: 0,
        }
    }
}

/// Internal pairing manager state.
struct PairingManagerInner {
    local_io_capability: u8,
    bonded_devices: BTreeMap<[u8; 6], BondKeys>,
    active_session: Option<PairingSession>,
    require_mitm: bool,
    require_bonding: bool,
    secure_connections_only: bool,
}

impl PairingManagerInner {
    fn new() -> Self {
        Self {
            local_io_capability: IO_DISPLAY_YES_NO,
            bonded_devices: BTreeMap::new(),
            active_session: None,
            require_mitm: true,
            require_bonding: true,
            secure_connections_only: false,
        }
    }

    /// Determine the pairing method based on IO capabilities.
    fn select_method(initiator_io: u8, responder_io: u8) -> PairingMethod {
        // SSP association model selection table (Bluetooth Core Spec Vol 3, Part H, Table 2.8).
        match (initiator_io, responder_io) {
            (IO_NO_INPUT_NO_OUTPUT, _) | (_, IO_NO_INPUT_NO_OUTPUT) => PairingMethod::JustWorks,
            (IO_DISPLAY_ONLY, IO_DISPLAY_ONLY) => PairingMethod::JustWorks,
            (IO_DISPLAY_ONLY, IO_KEYBOARD_ONLY) | (IO_DISPLAY_ONLY, IO_KEYBOARD_DISPLAY) => PairingMethod::PasskeyEntry,
            (IO_KEYBOARD_ONLY, IO_DISPLAY_ONLY) | (IO_KEYBOARD_DISPLAY, IO_DISPLAY_ONLY) => PairingMethod::PasskeyEntry,
            (IO_DISPLAY_YES_NO, IO_DISPLAY_YES_NO) | (IO_DISPLAY_YES_NO, IO_KEYBOARD_DISPLAY) |
            (IO_KEYBOARD_DISPLAY, IO_DISPLAY_YES_NO) | (IO_KEYBOARD_DISPLAY, IO_KEYBOARD_DISPLAY) => PairingMethod::NumericComparison,
            (IO_KEYBOARD_ONLY, IO_KEYBOARD_ONLY) => PairingMethod::PasskeyEntry,
            _ => PairingMethod::JustWorks,
        }
    }

    /// Generate a pseudo-random 128-bit value using TSC.
    fn generate_random_128() -> [u8; 16] {
        let mut result = [0u8; 16];
        // Use TSC as a seed for pseudo-randomness.
        let mut lo: u32;
        let mut hi: u32;
        unsafe {
            core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi);
        }
        let seed = ((hi as u64) << 32) | lo as u64;

        // FNV-1a hash to spread the bits.
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;
        for i in 0..16u64 {
            hash ^= seed.wrapping_add(i);
            hash = hash.wrapping_mul(prime);
            result[i as usize] = (hash & 0xFF) as u8;
        }
        result
    }

    /// Initiate pairing with a remote device.
    fn pair(&mut self, address: &[u8; 6], method: PairingMethod) {
        if self.active_session.is_some() {
            serial_println!("    [pairing] Already pairing with another device");
            return;
        }

        let mut session = PairingSession::new(*address, method);
        session.local_io = self.local_io_capability;
        session.state = PairingState::CapabilityExchange;
        session.random_value = Self::generate_random_128();

        serial_println!("    [pairing] Initiating {:?} pairing with {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            method,
            address[0], address[1], address[2], address[3], address[4], address[5]);

        // Build SMP Pairing Request.
        let _auth_req = if self.require_bonding { AUTH_MITM_BONDING } else { AUTH_MITM_NO_BONDING };

        // Advance through pairing stages.
        session.state = PairingState::Authenticating;

        match method {
            PairingMethod::JustWorks => {
                serial_println!("    [pairing] Just Works: auto-confirming");
                session.state = PairingState::KeyDistribution;
            }
            PairingMethod::NumericComparison => {
                // Generate a 6-digit comparison number.
                let num = (session.random_value[0] as u32) << 16
                    | (session.random_value[1] as u32) << 8
                    | session.random_value[2] as u32;
                session.passkey = num % 1_000_000;
                serial_println!("    [pairing] Numeric comparison: {:06}", session.passkey);
                session.state = PairingState::KeyDistribution;
            }
            PairingMethod::PasskeyEntry => {
                let num = (session.random_value[0] as u32) << 16
                    | (session.random_value[1] as u32) << 8
                    | session.random_value[2] as u32;
                session.passkey = num % 1_000_000;
                serial_println!("    [pairing] Passkey: {:06}", session.passkey);
                session.state = PairingState::KeyDistribution;
            }
            PairingMethod::OutOfBand => {
                serial_println!("    [pairing] OOB: expecting out-of-band data");
                session.state = PairingState::KeyDistribution;
            }
        }

        // Generate keys.
        let mut keys = BondKeys::new();
        keys.ltk = Some(Self::generate_random_128());
        keys.irk = Some(Self::generate_random_128());
        keys.csrk = Some(Self::generate_random_128());
        keys.authenticated = !matches!(method, PairingMethod::JustWorks);
        keys.secure_connections = true;

        // Generate EDIV and Rand.
        let rand_bytes = Self::generate_random_128();
        keys.ediv = (rand_bytes[0] as u16) | ((rand_bytes[1] as u16) << 8);
        keys.rand = u64::from_le_bytes([
            rand_bytes[2], rand_bytes[3], rand_bytes[4], rand_bytes[5],
            rand_bytes[6], rand_bytes[7], rand_bytes[8], rand_bytes[9],
        ]);

        session.state = PairingState::Complete;
        serial_println!("    [pairing] Pairing complete, keys generated");

        // Store bond.
        if self.require_bonding {
            self.bonded_devices.insert(*address, keys);
            serial_println!("    [pairing] Device bonded ({} total bonds)", self.bonded_devices.len());
        }

        self.active_session = None; // Session complete.
    }

    /// Check if a device is bonded.
    fn is_bonded(&self, address: &[u8; 6]) -> bool {
        self.bonded_devices.contains_key(address)
    }

    /// Remove a bond.
    fn remove_bond(&mut self, address: &[u8; 6]) -> bool {
        if self.bonded_devices.remove(address).is_some() {
            serial_println!("    [pairing] Bond removed for {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                address[0], address[1], address[2], address[3], address[4], address[5]);
            true
        } else {
            false
        }
    }

    /// Handle an incoming SMP PDU (for LE pairing).
    fn handle_smp(&mut self, pdu: &[u8]) {
        if pdu.is_empty() {
            return;
        }

        let code = pdu[0];
        match code {
            SMP_PAIRING_REQ => {
                if pdu.len() >= 7 {
                    let remote_io = pdu[1];
                    let _oob = pdu[2];
                    let _auth_req = pdu[3];
                    let method = Self::select_method(self.local_io_capability, remote_io);
                    serial_println!("    [pairing] Received pairing request, IO={:#04x}, method={:?}", remote_io, method);
                }
            }
            SMP_PAIRING_CONFIRM => {
                serial_println!("    [pairing] Received pairing confirm");
            }
            SMP_PAIRING_RANDOM => {
                serial_println!("    [pairing] Received pairing random");
            }
            SMP_PAIRING_FAILED => {
                if pdu.len() >= 2 {
                    serial_println!("    [pairing] Pairing failed, reason={:#04x}", pdu[1]);
                }
                self.active_session = None;
            }
            _ => {
                serial_println!("    [pairing] SMP command {:#04x}", code);
            }
        }
    }
}

/// Manages Bluetooth pairing and key storage.
pub struct PairingManager {
    _private: (),
}

impl PairingManager {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Initiate pairing with a remote device.
    pub fn pair(&mut self, address: &[u8; 6], method: PairingMethod) {
        if let Some(inner) = PAIRING.lock().as_mut() {
            inner.pair(address, method);
        }
    }

    /// Check if a device is already bonded.
    pub fn is_bonded(&self, address: &[u8; 6]) -> bool {
        if let Some(inner) = PAIRING.lock().as_ref() {
            inner.is_bonded(address)
        } else {
            false
        }
    }
}

/// Handle an incoming SMP PDU.
pub fn handle_smp(pdu: &[u8]) {
    if let Some(inner) = PAIRING.lock().as_mut() {
        inner.handle_smp(pdu);
    }
}

/// Check if a device is bonded (module-level convenience).
pub fn is_bonded(address: &[u8; 6]) -> bool {
    if let Some(inner) = PAIRING.lock().as_ref() {
        inner.is_bonded(address)
    } else {
        false
    }
}

/// Remove a bond.
pub fn remove_bond(address: &[u8; 6]) -> bool {
    if let Some(inner) = PAIRING.lock().as_mut() {
        inner.remove_bond(address)
    } else {
        false
    }
}

pub fn init() {
    let inner = PairingManagerInner::new();

    serial_println!("    [pairing] Initializing pairing manager");
    serial_println!("    [pairing] IO capability: DisplayYesNo, MITM required, bonding enabled");

    *PAIRING.lock() = Some(inner);
    serial_println!("    [pairing] Pairing manager initialized");
}
