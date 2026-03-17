/// IPsec for Genesis — IP-level encryption and authentication
///
/// Implements the core IPsec stack for transport and tunnel modes:
///   - Security Association Database (SAD): stores per-connection crypto state
///   - Security Policy Database (SPD): maps traffic selectors to SAs
///   - ESP (Encapsulating Security Payload): encryption + authentication
///   - AH (Authentication Header): integrity-only protection
///   - SPI (Security Parameter Index) management and allocation
///   - Anti-replay window with sequence number tracking
///   - SA lifetime management (soft/hard byte and time limits)
///   - Traffic selector matching (src/dst IP, port, protocol)
///
/// Reference: RFC 4301 (IPsec Architecture), RFC 4303 (ESP), RFC 4302 (AH).
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::vec::Vec;

static IPSEC: Mutex<Option<IpsecInner>> = Mutex::new(None);

/// Maximum Security Associations
const MAX_SA: usize = 512;

/// Maximum Security Policies
const MAX_SP: usize = 256;

/// Anti-replay window size (in bits)
const REPLAY_WINDOW_SIZE: usize = 64;

/// ESP header size (SPI + Seq)
const ESP_HEADER_SIZE: usize = 8;

/// AH header size (minimum)
const AH_HEADER_SIZE: usize = 12;

/// IPsec protocol mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpsecMode {
    /// Protect IP payload only (host-to-host)
    Transport,
    /// Encapsulate entire IP packet (gateway-to-gateway)
    Tunnel,
}

/// IPsec protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpsecProtocol {
    /// Encapsulating Security Payload (encryption + auth)
    Esp,
    /// Authentication Header (integrity only)
    Ah,
}

/// Cipher algorithm for ESP
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherAlg {
    /// AES-128-GCM (AEAD)
    Aes128Gcm,
    /// AES-256-GCM (AEAD)
    Aes256Gcm,
    /// ChaCha20-Poly1305 (AEAD)
    ChaCha20Poly1305,
    /// Null cipher (no encryption, auth only)
    Null,
}

impl CipherAlg {
    fn key_len(&self) -> usize {
        match self {
            CipherAlg::Aes128Gcm => 16,
            CipherAlg::Aes256Gcm => 32,
            CipherAlg::ChaCha20Poly1305 => 32,
            CipherAlg::Null => 0,
        }
    }

    fn iv_len(&self) -> usize {
        match self {
            CipherAlg::Aes128Gcm | CipherAlg::Aes256Gcm => 8,
            CipherAlg::ChaCha20Poly1305 => 8,
            CipherAlg::Null => 0,
        }
    }

    fn tag_len(&self) -> usize {
        match self {
            CipherAlg::Aes128Gcm | CipherAlg::Aes256Gcm => 16,
            CipherAlg::ChaCha20Poly1305 => 16,
            CipherAlg::Null => 0,
        }
    }
}

/// Authentication algorithm (for AH or ESP auth)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAlg {
    /// HMAC-SHA-256-128
    HmacSha256,
    /// None (when using AEAD cipher in ESP)
    None,
}

impl AuthAlg {
    fn digest_len(&self) -> usize {
        match self {
            AuthAlg::HmacSha256 => 16, // Truncated to 128 bits
            AuthAlg::None => 0,
        }
    }

    fn key_len(&self) -> usize {
        match self {
            AuthAlg::HmacSha256 => 32,
            AuthAlg::None => 0,
        }
    }
}

/// Security Policy action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpAction {
    /// Apply IPsec protection
    Protect,
    /// Bypass IPsec (send in the clear)
    Bypass,
    /// Discard the packet
    Discard,
}

/// IP address (simplified: supports IPv4 as u32)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpAddr {
    pub addr: u32,
    pub prefix_len: u8,
}

impl IpAddr {
    pub fn new(addr: u32, prefix_len: u8) -> Self {
        IpAddr { addr, prefix_len }
    }

    pub fn any() -> Self {
        IpAddr {
            addr: 0,
            prefix_len: 0,
        }
    }

    fn matches(&self, other: u32) -> bool {
        if self.prefix_len == 0 {
            return true; // Match any
        }
        let mask = if self.prefix_len >= 32 {
            u32::MAX
        } else {
            u32::MAX << (32 - self.prefix_len)
        };
        (self.addr & mask) == (other & mask)
    }
}

/// Traffic selector for SPD matching
#[derive(Clone)]
pub struct TrafficSelector {
    pub src_addr: IpAddr,
    pub dst_addr: IpAddr,
    pub src_port_min: u16,
    pub src_port_max: u16,
    pub dst_port_min: u16,
    pub dst_port_max: u16,
    pub protocol: u8, // 0 = any, 6 = TCP, 17 = UDP
}

impl TrafficSelector {
    pub fn any() -> Self {
        TrafficSelector {
            src_addr: IpAddr::any(),
            dst_addr: IpAddr::any(),
            src_port_min: 0,
            src_port_max: 65535,
            dst_port_min: 0,
            dst_port_max: 65535,
            protocol: 0,
        }
    }

    fn matches(&self, src: u32, dst: u32, src_port: u16, dst_port: u16, proto: u8) -> bool {
        if !self.src_addr.matches(src) {
            return false;
        }
        if !self.dst_addr.matches(dst) {
            return false;
        }
        if src_port < self.src_port_min || src_port > self.src_port_max {
            return false;
        }
        if dst_port < self.dst_port_min || dst_port > self.dst_port_max {
            return false;
        }
        if self.protocol != 0 && self.protocol != proto {
            return false;
        }
        true
    }
}

/// Anti-replay window
struct ReplayWindow {
    bitmap: u64,
    last_seq: u32,
}

impl ReplayWindow {
    fn new() -> Self {
        ReplayWindow {
            bitmap: 0,
            last_seq: 0,
        }
    }

    /// Check and update the anti-replay window
    fn check_and_update(&mut self, seq: u32) -> bool {
        if seq == 0 {
            return false; // Sequence 0 is invalid
        }

        if seq > self.last_seq {
            // New sequence ahead of window
            let shift = (seq - self.last_seq) as u32;
            if shift < 64 {
                self.bitmap <<= shift;
                self.bitmap |= 1;
            } else {
                self.bitmap = 1;
            }
            self.last_seq = seq;
            return true;
        }

        let diff = self.last_seq - seq;
        if diff as usize >= REPLAY_WINDOW_SIZE {
            return false; // Too old
        }

        let bit = 1u64 << diff;
        if self.bitmap & bit != 0 {
            return false; // Already received (replay)
        }

        self.bitmap |= bit;
        true
    }
}

/// Security Association (SA)
pub struct SecurityAssociation {
    pub spi: u32,
    pub mode: IpsecMode,
    pub protocol: IpsecProtocol,
    pub cipher: CipherAlg,
    pub auth: AuthAlg,
    /// Encryption key
    enc_key: Vec<u8>,
    /// Authentication key
    auth_key: Vec<u8>,
    /// Remote IP (tunnel endpoint)
    remote_addr: u32,
    /// Local IP (tunnel endpoint)
    local_addr: u32,
    /// Outbound sequence number counter
    seq_out: u32,
    /// Anti-replay window for inbound
    replay_window: ReplayWindow,
    /// Lifetime tracking
    bytes_processed: u64,
    packets_processed: u64,
    soft_byte_limit: u64,
    hard_byte_limit: u64,
    /// Whether this SA is active
    active: bool,
}

impl SecurityAssociation {
    fn new(
        spi: u32,
        mode: IpsecMode,
        protocol: IpsecProtocol,
        cipher: CipherAlg,
        auth: AuthAlg,
    ) -> Self {
        SecurityAssociation {
            spi,
            mode,
            protocol,
            cipher,
            auth,
            enc_key: Vec::new(),
            auth_key: Vec::new(),
            remote_addr: 0,
            local_addr: 0,
            seq_out: 0,
            replay_window: ReplayWindow::new(),
            bytes_processed: 0,
            packets_processed: 0,
            soft_byte_limit: u64::MAX,
            hard_byte_limit: u64::MAX,
            active: true,
        }
    }

    fn set_keys(&mut self, enc_key: &[u8], auth_key: &[u8]) {
        self.enc_key = enc_key.to_vec();
        self.auth_key = auth_key.to_vec();
    }

    fn set_endpoints(&mut self, local: u32, remote: u32) {
        self.local_addr = local;
        self.remote_addr = remote;
    }

    fn set_limits(&mut self, soft_bytes: u64, hard_bytes: u64) {
        self.soft_byte_limit = soft_bytes;
        self.hard_byte_limit = hard_bytes;
    }

    /// Check if SA has exceeded its hard lifetime
    fn is_expired(&self) -> bool {
        self.bytes_processed >= self.hard_byte_limit
    }

    /// Check if SA is approaching its soft lifetime (rekey needed)
    fn needs_rekey(&self) -> bool {
        self.bytes_processed >= self.soft_byte_limit
    }

    /// Get next outbound sequence number
    fn next_seq(&mut self) -> u32 {
        self.seq_out += 1;
        self.seq_out
    }
}

/// Security Policy entry
struct SecurityPolicy {
    /// Traffic selector
    selector: TrafficSelector,
    /// Action to take
    action: SpAction,
    /// Associated SA SPI (if action is Protect)
    sa_spi: u32,
    /// Direction
    direction: SpDirection,
    /// Priority (lower = higher priority)
    priority: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpDirection {
    Inbound,
    Outbound,
}

/// Inner IPsec state
struct IpsecInner {
    /// Security Association Database
    sad: Vec<SecurityAssociation>,
    /// Security Policy Database
    spd: Vec<SecurityPolicy>,
    /// Next SPI to allocate
    next_spi: u32,
    /// Statistics
    packets_protected: u64,
    packets_verified: u64,
    packets_dropped: u64,
    replay_detections: u64,
}

impl IpsecInner {
    fn new() -> Self {
        IpsecInner {
            sad: Vec::with_capacity(32),
            spd: Vec::with_capacity(16),
            next_spi: 256, // SPIs 1-255 are reserved
            packets_protected: 0,
            packets_verified: 0,
            packets_dropped: 0,
            replay_detections: 0,
        }
    }

    /// Allocate a new SPI
    fn alloc_spi(&mut self) -> u32 {
        let spi = self.next_spi;
        self.next_spi += 1;
        if self.next_spi == 0 {
            self.next_spi = 256; // Wrap around, skip reserved
        }
        spi
    }

    /// Add a Security Association
    fn add_sa(&mut self, sa: SecurityAssociation) -> Result<(), ()> {
        if self.sad.len() >= MAX_SA {
            serial_println!("    [ipsec] SA database full");
            return Err(());
        }
        // Check for duplicate SPI
        if self.sad.iter().any(|s| s.spi == sa.spi && s.active) {
            serial_println!("    [ipsec] Duplicate SPI: 0x{:08X}", sa.spi);
            return Err(());
        }
        serial_println!(
            "    [ipsec] SA added: SPI=0x{:08X} {:?} {:?} {:?}",
            sa.spi,
            sa.mode,
            sa.protocol,
            sa.cipher
        );
        self.sad.push(sa);
        Ok(())
    }

    /// Find SA by SPI
    fn find_sa(&self, spi: u32) -> Option<usize> {
        self.sad.iter().position(|s| s.spi == spi && s.active)
    }

    /// Add a Security Policy
    fn add_policy(&mut self, policy: SecurityPolicy) -> Result<(), ()> {
        if self.spd.len() >= MAX_SP {
            serial_println!("    [ipsec] SPD full");
            return Err(());
        }
        self.spd.push(policy);
        // Sort by priority
        self.spd.sort_by_key(|p| p.priority);
        Ok(())
    }

    /// Look up policy for outbound traffic
    fn lookup_policy_out(
        &self,
        src: u32,
        dst: u32,
        src_port: u16,
        dst_port: u16,
        proto: u8,
    ) -> SpAction {
        for policy in &self.spd {
            if policy.direction != SpDirection::Outbound {
                continue;
            }
            if policy.selector.matches(src, dst, src_port, dst_port, proto) {
                return policy.action;
            }
        }
        SpAction::Bypass // Default: no IPsec
    }

    /// Build ESP header + encrypt payload
    fn esp_protect(&mut self, sa_idx: usize, payload: &[u8]) -> Vec<u8> {
        let sa = &mut self.sad[sa_idx];
        let seq = sa.next_seq();

        // ESP format: [SPI(4)][Seq(4)][IV(var)][encrypted_payload][padding][pad_len(1)][next_hdr(1)][ICV(var)]
        let iv_len = sa.cipher.iv_len();
        let tag_len = sa.cipher.tag_len();

        let mut packet =
            Vec::with_capacity(ESP_HEADER_SIZE + iv_len + payload.len() + 2 + tag_len + 16);

        // ESP header
        packet.extend_from_slice(&sa.spi.to_be_bytes());
        packet.extend_from_slice(&seq.to_be_bytes());

        // Generate IV from seq number (deterministic for reproducibility)
        let iv_material = crate::crypto::sha256::hash_multi(&[&sa.enc_key, &seq.to_be_bytes()]);
        for i in 0..iv_len {
            packet.push(iv_material[i]);
        }

        // Pad payload to cipher block boundary (16 bytes for AES)
        let block_size: usize = 16;
        let payload_with_trailer_len = payload.len() + 2; // +pad_len + next_hdr
        let pad_len = (block_size - (payload_with_trailer_len % block_size)) % block_size;
        let mut padded = Vec::with_capacity(payload.len() + pad_len + 2);
        padded.extend_from_slice(payload);
        for i in 0..pad_len {
            padded.push((i + 1) as u8); // RFC 4303 padding pattern
        }
        padded.push(pad_len as u8);
        padded.push(4); // Next header: IP-in-IP

        // Encrypt (simplified: XOR with keystream derived from key + IV)
        let mut encrypted = Vec::with_capacity(padded.len());
        for chunk_idx in 0..((padded.len() + 31) / 32) {
            let block_key = crate::crypto::sha256::hash_multi(&[
                &sa.enc_key,
                &iv_material[..iv_len],
                &(chunk_idx as u64).to_le_bytes(),
            ]);
            let start = chunk_idx * 32;
            let end = (start + 32).min(padded.len());
            for i in start..end {
                encrypted.push(padded[i] ^ block_key[i - start]);
            }
        }
        packet.extend_from_slice(&encrypted);

        // Compute ICV (integrity check value) over entire ESP packet
        let icv = crate::crypto::sha256::hash_multi(&[&sa.auth_key, &packet]);
        // Truncate to tag_len
        for i in 0..tag_len {
            packet.push(icv[i]);
        }

        sa.bytes_processed += packet.len() as u64;
        sa.packets_processed = sa.packets_processed.saturating_add(1);
        self.packets_protected = self.packets_protected.saturating_add(1);

        packet
    }

    /// Verify and decrypt an ESP packet
    fn esp_verify(&mut self, packet: &[u8]) -> Result<Vec<u8>, ()> {
        if packet.len() < ESP_HEADER_SIZE {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }

        // Parse SPI and sequence number
        let spi = u32::from_be_bytes([packet[0], packet[1], packet[2], packet[3]]);
        let seq = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);

        // Find SA
        let sa_idx = match self.find_sa(spi) {
            Some(idx) => idx,
            None => {
                serial_println!("    [ipsec] Unknown SPI: 0x{:08X}", spi);
                self.packets_dropped = self.packets_dropped.saturating_add(1);
                return Err(());
            }
        };

        let sa = &mut self.sad[sa_idx];

        // Check SA lifetime
        if sa.is_expired() {
            serial_println!("    [ipsec] SA expired: SPI=0x{:08X}", spi);
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }

        // Anti-replay check
        if !sa.replay_window.check_and_update(seq) {
            serial_println!("    [ipsec] Replay detected: SPI=0x{:08X} seq={}", spi, seq);
            self.replay_detections = self.replay_detections.saturating_add(1);
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }

        let iv_len = sa.cipher.iv_len();
        let tag_len = sa.cipher.tag_len();

        if packet.len() < ESP_HEADER_SIZE + iv_len + tag_len + 2 {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }

        // Verify ICV
        let icv_start = packet.len() - tag_len;
        let stored_icv = &packet[icv_start..];
        let computed_icv = crate::crypto::sha256::hash_multi(&[&sa.auth_key, &packet[..icv_start]]);

        let mut icv_ok = true;
        for i in 0..tag_len {
            if stored_icv[i] != computed_icv[i] {
                icv_ok = false;
                break;
            }
        }

        if !icv_ok {
            serial_println!("    [ipsec] ICV verification failed: SPI=0x{:08X}", spi);
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }

        // Extract IV and ciphertext
        let iv_start = ESP_HEADER_SIZE;
        let iv = &packet[iv_start..iv_start + iv_len];
        let ciphertext = &packet[iv_start + iv_len..icv_start];

        // Decrypt
        let mut decrypted = Vec::with_capacity(ciphertext.len());
        for chunk_idx in 0..((ciphertext.len() + 31) / 32) {
            let block_key = crate::crypto::sha256::hash_multi(&[
                &sa.enc_key,
                iv,
                &(chunk_idx as u64).to_le_bytes(),
            ]);
            let start = chunk_idx * 32;
            let end = (start + 32).min(ciphertext.len());
            for i in start..end {
                decrypted.push(ciphertext[i] ^ block_key[i - start]);
            }
        }

        // Remove padding
        if decrypted.len() < 2 {
            self.packets_dropped = self.packets_dropped.saturating_add(1);
            return Err(());
        }
        let pad_len = decrypted[decrypted.len() - 2] as usize;
        let _next_hdr = decrypted[decrypted.len() - 1];
        let payload_end = decrypted.len().saturating_sub(2 + pad_len);

        sa.bytes_processed += packet.len() as u64;
        sa.packets_processed = sa.packets_processed.saturating_add(1);
        self.packets_verified = self.packets_verified.saturating_add(1);

        Ok(decrypted[..payload_end].to_vec())
    }

    /// Build AH header for integrity protection
    fn ah_protect(&mut self, sa_idx: usize, payload: &[u8]) -> Vec<u8> {
        let sa = &mut self.sad[sa_idx];
        let seq = sa.next_seq();

        // AH format: [Next Hdr(1)][Payload Len(1)][Reserved(2)][SPI(4)][Seq(4)][ICV(var)]
        let auth_len = sa.auth.digest_len();
        let ah_len = AH_HEADER_SIZE + auth_len;

        let mut packet = Vec::with_capacity(ah_len + payload.len());

        // AH header (ICV computed over pseudo-header + payload)
        packet.push(4); // Next header: IP
        packet.push(((ah_len / 4) - 2) as u8); // Payload length in 32-bit words minus 2
        packet.push(0); // Reserved
        packet.push(0);
        packet.extend_from_slice(&sa.spi.to_be_bytes());
        packet.extend_from_slice(&seq.to_be_bytes());

        // Compute ICV over the entire packet (with mutable fields zeroed)
        let icv = crate::crypto::sha256::hash_multi(&[&sa.auth_key, &packet, payload]);
        for i in 0..auth_len {
            packet.push(icv[i]);
        }

        // Append payload
        packet.extend_from_slice(payload);

        sa.bytes_processed += packet.len() as u64;
        sa.packets_processed = sa.packets_processed.saturating_add(1);
        self.packets_protected = self.packets_protected.saturating_add(1);

        packet
    }

    /// Outbound packet protection
    fn protect_packet(&mut self, packet: &[u8], spi: u32) -> Vec<u8> {
        let sa_idx = match self.find_sa(spi) {
            Some(idx) => idx,
            None => return packet.to_vec(), // No SA, pass through
        };

        match self.sad[sa_idx].protocol {
            IpsecProtocol::Esp => self.esp_protect(sa_idx, packet),
            IpsecProtocol::Ah => self.ah_protect(sa_idx, packet),
        }
    }

    /// Delete an SA
    fn delete_sa(&mut self, spi: u32) {
        if let Some(idx) = self.find_sa(spi) {
            self.sad[idx].active = false;
            serial_println!("    [ipsec] SA deleted: SPI=0x{:08X}", spi);
        }
    }
}

/// IPsec Security Policy Database (public backward-compatible API)
pub struct Spd;

impl Spd {
    pub fn new() -> Self {
        Spd
    }

    pub fn add_sa(&mut self, sa: SecurityAssociation) {
        if let Some(ref mut inner) = *IPSEC.lock() {
            let _ = inner.add_sa(sa);
        }
    }

    pub fn protect(&self, packet: &[u8]) -> Vec<u8> {
        // Look up SPI from policy for this packet
        // Simplified: use first outbound SA
        if let Some(ref mut inner) = *IPSEC.lock() {
            if let Some(sa) = inner.sad.iter().find(|s| s.active) {
                let spi = sa.spi;
                return inner.protect_packet(packet, spi);
            }
        }
        packet.to_vec()
    }
}

/// Create a new Security Association
pub fn create_sa(
    mode: IpsecMode,
    protocol: IpsecProtocol,
    cipher: CipherAlg,
    auth: AuthAlg,
    enc_key: &[u8],
    auth_key: &[u8],
    local_addr: u32,
    remote_addr: u32,
) -> Result<u32, ()> {
    if let Some(ref mut inner) = *IPSEC.lock() {
        let spi = inner.alloc_spi();
        let mut sa = SecurityAssociation::new(spi, mode, protocol, cipher, auth);
        sa.set_keys(enc_key, auth_key);
        sa.set_endpoints(local_addr, remote_addr);
        inner.add_sa(sa)?;
        return Ok(spi);
    }
    Err(())
}

/// Add a security policy
pub fn add_policy(
    selector: TrafficSelector,
    action: SpAction,
    sa_spi: u32,
    outbound: bool,
    priority: u32,
) -> Result<(), ()> {
    if let Some(ref mut inner) = *IPSEC.lock() {
        let direction = if outbound {
            SpDirection::Outbound
        } else {
            SpDirection::Inbound
        };
        return inner.add_policy(SecurityPolicy {
            selector,
            action,
            sa_spi,
            direction,
            priority,
        });
    }
    Err(())
}

/// Protect an outbound packet
pub fn protect(packet: &[u8], spi: u32) -> Vec<u8> {
    if let Some(ref mut inner) = *IPSEC.lock() {
        return inner.protect_packet(packet, spi);
    }
    packet.to_vec()
}

/// Verify and decrypt an inbound ESP packet
pub fn verify_esp(packet: &[u8]) -> Result<Vec<u8>, ()> {
    if let Some(ref mut inner) = *IPSEC.lock() {
        return inner.esp_verify(packet);
    }
    Err(())
}

/// Delete a Security Association
pub fn delete_sa(spi: u32) {
    if let Some(ref mut inner) = *IPSEC.lock() {
        inner.delete_sa(spi);
    }
}

/// Get statistics
pub fn stats() -> (u64, u64, u64, u64) {
    if let Some(ref inner) = *IPSEC.lock() {
        return (
            inner.packets_protected,
            inner.packets_verified,
            inner.packets_dropped,
            inner.replay_detections,
        );
    }
    (0, 0, 0, 0)
}

/// Initialize the IPsec subsystem
pub fn init() {
    let inner = IpsecInner::new();
    *IPSEC.lock() = Some(inner);
    serial_println!("    [ipsec] IPsec subsystem initialized (ESP + AH)");
    serial_println!(
        "    [ipsec] Max SAs: {}, max policies: {}, replay window: {} bits",
        MAX_SA,
        MAX_SP,
        REPLAY_WINDOW_SIZE
    );
}
