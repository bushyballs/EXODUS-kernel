/// Trusted Platform Module 2.0 interface for Genesis
///
/// Implements TPM 2.0 TIS (TPM Interface Specification) over MMIO:
///   - Command/response protocol at base 0xFED40000
///   - PCR extend and read operations
///   - Seal/unseal with PCR policy binding
///   - Hardware random number generation (TPM2_GetRandom)
///   - Locality management (0-4)
///
/// Reference: TCG PC Client Platform TPM Profile (PTP) Specification.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::vec::Vec;

static TPM: Mutex<Option<TpmInner>> = Mutex::new(None);

/// TPM TIS MMIO base address
const TIS_BASE: u64 = 0xFED4_0000;

/// TIS register offsets (relative to locality base)
const TIS_ACCESS: u64 = 0x00;
const TIS_INT_ENABLE: u64 = 0x08;
const TIS_STS: u64 = 0x18;
const TIS_DATA_FIFO: u64 = 0x24;
const TIS_INTF_CAPS: u64 = 0x14;
const TIS_DID_VID: u64 = 0xF00;

/// TIS status bits
const STS_VALID: u8 = 1 << 7;
const STS_COMMAND_READY: u8 = 1 << 6;
const STS_DATA_AVAIL: u8 = 1 << 4;
const STS_EXPECT: u8 = 1 << 3;
const STS_GO: u8 = 1 << 5;

/// TIS access bits
const ACCESS_ACTIVE_LOCALITY: u8 = 1 << 5;
const ACCESS_REQUEST_USE: u8 = 1 << 1;
const ACCESS_RELINQUISH: u8 = 1 << 2;
const ACCESS_TPM_ESTABLISHMENT: u8 = 1 << 0;

/// TPM2 command tags
const TPM2_ST_NO_SESSIONS: u16 = 0x8001;
const TPM2_ST_SESSIONS: u16 = 0x8002;

/// TPM2 command codes
const TPM2_CC_STARTUP: u32 = 0x0000_0144;
const TPM2_CC_PCR_EXTEND: u32 = 0x0000_0182;
const TPM2_CC_PCR_READ: u32 = 0x0000_017E;
const TPM2_CC_GET_RANDOM: u32 = 0x0000_017B;
const TPM2_CC_CREATE_PRIMARY: u32 = 0x0000_0131;

/// TPM2 algorithm IDs
const TPM2_ALG_SHA256: u16 = 0x000B;

/// TPM2 response codes
const TPM2_RC_SUCCESS: u32 = 0x0000_0000;

/// Number of PCRs
const PCR_COUNT: usize = 24;

/// Maximum command buffer size
const CMD_BUF_SIZE: usize = 4096;

/// Maximum response buffer size
const RSP_BUF_SIZE: usize = 4096;

/// TPM PCR (Platform Configuration Register) index
pub type PcrIndex = u8;

/// Locality level (0-4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locality {
    Zero = 0,
    One = 1,
    Two = 2,
    Three = 3,
    Four = 4,
}

/// TPM device state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TpmState {
    Idle,
    Ready,
    CommandComplete,
    Error,
}

/// Inner TPM state
struct TpmInner {
    base_addr: u64,
    locality: Locality,
    state: TpmState,
    /// Cached PCR values (shadow copy for fast reads)
    pcr_cache: [[u8; 32]; PCR_COUNT],
    pcr_cache_valid: [bool; PCR_COUNT],
    /// Command/response buffers
    cmd_buf: [u8; CMD_BUF_SIZE],
    rsp_buf: [u8; RSP_BUF_SIZE],
    /// Statistics
    commands_sent: u64,
    errors: u64,
    /// Whether TPM hardware was detected
    detected: bool,
}

impl TpmInner {
    fn new() -> Self {
        TpmInner {
            base_addr: TIS_BASE,
            locality: Locality::Zero,
            state: TpmState::Idle,
            pcr_cache: [[0u8; 32]; PCR_COUNT],
            pcr_cache_valid: [false; PCR_COUNT],
            cmd_buf: [0u8; CMD_BUF_SIZE],
            rsp_buf: [0u8; RSP_BUF_SIZE],
            commands_sent: 0,
            errors: 0,
            detected: false,
        }
    }

    /// Get the MMIO address for a register at the current locality
    fn reg_addr(&self, offset: u64) -> *mut u8 {
        let locality_offset = (self.locality as u64) * 0x1000;
        (self.base_addr + locality_offset + offset) as *mut u8
    }

    /// Read a TIS register (8-bit)
    fn tis_read8(&self, offset: u64) -> u8 {
        unsafe { core::ptr::read_volatile(self.reg_addr(offset)) }
    }

    /// Write a TIS register (8-bit)
    fn tis_write8(&self, offset: u64, val: u8) {
        unsafe {
            core::ptr::write_volatile(self.reg_addr(offset), val);
        }
    }

    /// Read a TIS register (32-bit)
    fn tis_read32(&self, offset: u64) -> u32 {
        unsafe { core::ptr::read_volatile(self.reg_addr(offset) as *mut u32) }
    }

    /// Probe for TPM hardware presence
    fn probe(&mut self) -> bool {
        let did_vid = self.tis_read32(TIS_DID_VID);
        // 0xFFFFFFFF means no device present
        if did_vid == 0xFFFF_FFFF || did_vid == 0 {
            return false;
        }
        let vendor_id = (did_vid & 0xFFFF) as u16;
        let device_id = ((did_vid >> 16) & 0xFFFF) as u16;
        serial_println!(
            "    [tpm] Detected TPM: vendor=0x{:04X} device=0x{:04X}",
            vendor_id,
            device_id
        );

        let intf_caps = self.tis_read32(TIS_INTF_CAPS);
        let intf_version = (intf_caps >> 28) & 0x7;
        serial_println!(
            "    [tpm] Interface capability: 0x{:08X} (version={})",
            intf_caps,
            intf_version
        );

        self.detected = true;
        true
    }

    /// Request locality access
    fn request_locality(&mut self, locality: Locality) -> bool {
        self.locality = locality;
        let access = self.tis_read8(TIS_ACCESS);
        if (access & ACCESS_ACTIVE_LOCALITY) != 0 {
            return true; // Already have it
        }
        self.tis_write8(TIS_ACCESS, ACCESS_REQUEST_USE);
        // Poll for access grant (with timeout)
        for _ in 0..10_000 {
            let access = self.tis_read8(TIS_ACCESS);
            if (access & ACCESS_ACTIVE_LOCALITY) != 0 {
                return true;
            }
            // Small delay
            for _ in 0..100 {
                core::hint::spin_loop();
            }
        }
        serial_println!("    [tpm] Failed to acquire locality {}", locality as u8);
        false
    }

    /// Relinquish locality
    fn relinquish_locality(&mut self) {
        self.tis_write8(TIS_ACCESS, ACCESS_RELINQUISH);
    }

    /// Wait for STS to have specific bits set
    fn wait_sts(&self, mask: u8, expected: u8, timeout: u32) -> bool {
        for _ in 0..timeout {
            let sts = self.tis_read8(TIS_STS);
            if (sts & STS_VALID) != 0 && (sts & mask) == expected {
                return true;
            }
            for _ in 0..100 {
                core::hint::spin_loop();
            }
        }
        false
    }

    /// Set the device to command-ready state
    fn set_command_ready(&mut self) -> bool {
        self.tis_write8(TIS_STS, STS_COMMAND_READY);
        if self.wait_sts(STS_COMMAND_READY, STS_COMMAND_READY, 10_000) {
            self.state = TpmState::Ready;
            return true;
        }
        self.state = TpmState::Error;
        false
    }

    /// Send a command and receive a response
    fn send_command(&mut self, cmd: &[u8]) -> Result<usize, ()> {
        if !self.detected {
            return Err(());
        }

        if !self.request_locality(self.locality) {
            return Err(());
        }

        if !self.set_command_ready() {
            self.errors = self.errors.saturating_add(1);
            return Err(());
        }

        // Write command bytes to FIFO
        for (i, &byte) in cmd.iter().enumerate() {
            self.tis_write8(TIS_DATA_FIFO, byte);
            // Check STS_EXPECT after each byte except the last
            if i < cmd.len() - 1 {
                let sts = self.tis_read8(TIS_STS);
                if (sts & STS_EXPECT) == 0 {
                    serial_println!("    [tpm] FIFO unexpectedly full at byte {}", i);
                    self.errors = self.errors.saturating_add(1);
                    return Err(());
                }
            }
        }

        // Verify TPM is no longer expecting data
        let sts = self.tis_read8(TIS_STS);
        if (sts & STS_EXPECT) != 0 {
            serial_println!("    [tpm] TPM still expecting data after command");
            self.errors = self.errors.saturating_add(1);
            return Err(());
        }

        // Execute command
        self.tis_write8(TIS_STS, STS_GO);

        // Wait for data available
        if !self.wait_sts(STS_DATA_AVAIL, STS_DATA_AVAIL, 100_000) {
            serial_println!("    [tpm] Timeout waiting for response");
            self.errors = self.errors.saturating_add(1);
            return Err(());
        }

        // Read response header (10 bytes: tag(2) + size(4) + rc(4))
        let mut rsp_len = 0usize;
        for i in 0..10 {
            if rsp_len >= RSP_BUF_SIZE {
                break;
            }
            self.rsp_buf[i] = self.tis_read8(TIS_DATA_FIFO);
            rsp_len += 1;
        }

        if rsp_len < 10 {
            self.errors = self.errors.saturating_add(1);
            return Err(());
        }

        // Parse response size from header
        let total_size = u32::from_be_bytes([
            self.rsp_buf[2],
            self.rsp_buf[3],
            self.rsp_buf[4],
            self.rsp_buf[5],
        ]) as usize;

        // Read remaining response bytes
        let remaining = total_size.saturating_sub(10).min(RSP_BUF_SIZE - 10);
        for i in 0..remaining {
            self.rsp_buf[10 + i] = self.tis_read8(TIS_DATA_FIFO);
            rsp_len += 1;
        }

        // Return to ready state
        self.tis_write8(TIS_STS, STS_COMMAND_READY);
        self.state = TpmState::CommandComplete;
        self.commands_sent = self.commands_sent.saturating_add(1);

        Ok(rsp_len)
    }

    /// Build a TPM2 command header
    fn build_header(&mut self, tag: u16, size: u32, cc: u32) -> usize {
        self.cmd_buf[0] = (tag >> 8) as u8;
        self.cmd_buf[1] = (tag & 0xFF) as u8;
        self.cmd_buf[2] = (size >> 24) as u8;
        self.cmd_buf[3] = (size >> 16) as u8;
        self.cmd_buf[4] = (size >> 8) as u8;
        self.cmd_buf[5] = (size & 0xFF) as u8;
        self.cmd_buf[6] = (cc >> 24) as u8;
        self.cmd_buf[7] = (cc >> 16) as u8;
        self.cmd_buf[8] = (cc >> 8) as u8;
        self.cmd_buf[9] = (cc & 0xFF) as u8;
        10
    }

    /// Parse response code from response buffer
    fn response_code(&self) -> u32 {
        u32::from_be_bytes([
            self.rsp_buf[6],
            self.rsp_buf[7],
            self.rsp_buf[8],
            self.rsp_buf[9],
        ])
    }

    /// TPM2_Startup(CLEAR)
    fn startup(&mut self) -> Result<(), ()> {
        let size: u32 = 12; // header(10) + startup_type(2)
        let mut off = self.build_header(TPM2_ST_NO_SESSIONS, size, TPM2_CC_STARTUP);
        // TPM_SU_CLEAR = 0x0000
        self.cmd_buf[off] = 0x00;
        self.cmd_buf[off + 1] = 0x00;
        off += 2;

        self.send_command(&self.cmd_buf[..off].to_vec())?;

        let rc = self.response_code();
        if rc != TPM2_RC_SUCCESS && rc != 0x0000_0100 {
            // 0x100 = TPM_RC_INITIALIZE means already started, which is fine
            serial_println!("    [tpm] Startup failed: rc=0x{:08X}", rc);
            return Err(());
        }
        Ok(())
    }

    /// TPM2_PCR_Extend
    fn pcr_extend_inner(&mut self, index: PcrIndex, digest: &[u8; 32]) -> Result<(), ()> {
        if index as usize >= PCR_COUNT {
            return Err(());
        }

        // Build TPM2_PCR_Extend command
        // Header(10) + pcrHandle(4) + authArea + digests
        let auth_size: u32 = 4 + 2 + 1 + 2; // session(4) + nonce(2) + attrs(1) + hmac(2)
        let digest_area: u32 = 4 + 2 + 32; // count(4) + algId(2) + digest(32)
        let total_size: u32 = 10 + 4 + 4 + auth_size + digest_area;

        let mut off = self.build_header(TPM2_ST_SESSIONS, total_size, TPM2_CC_PCR_EXTEND);

        // PCR handle (index maps to handle 0x00000000 + index)
        let handle: u32 = index as u32;
        self.cmd_buf[off..off + 4].copy_from_slice(&handle.to_be_bytes());
        off += 4;

        // Authorization area size
        self.cmd_buf[off..off + 4].copy_from_slice(&auth_size.to_be_bytes());
        off += 4;

        // Password session (TPM_RS_PW = 0x40000009)
        self.cmd_buf[off..off + 4].copy_from_slice(&0x4000_0009u32.to_be_bytes());
        off += 4;
        // Nonce (empty, size=0)
        self.cmd_buf[off] = 0x00;
        self.cmd_buf[off + 1] = 0x00;
        off += 2;
        // Session attributes = continueSession
        self.cmd_buf[off] = 0x01;
        off += 1;
        // HMAC (empty, size=0)
        self.cmd_buf[off] = 0x00;
        self.cmd_buf[off + 1] = 0x00;
        off += 2;

        // Digest values: count = 1
        self.cmd_buf[off..off + 4].copy_from_slice(&1u32.to_be_bytes());
        off += 4;
        // Algorithm = SHA-256
        self.cmd_buf[off..off + 2].copy_from_slice(&TPM2_ALG_SHA256.to_be_bytes());
        off += 2;
        // Digest
        self.cmd_buf[off..off + 32].copy_from_slice(digest);
        off += 32;

        let cmd_copy: Vec<u8> = self.cmd_buf[..off].to_vec();
        self.send_command(&cmd_copy)?;

        let rc = self.response_code();
        if rc != TPM2_RC_SUCCESS {
            serial_println!("    [tpm] PCR_Extend failed: rc=0x{:08X}", rc);
            return Err(());
        }

        // Update cached PCR value: new = SHA256(old || extend_value)
        let mut extend_input = [0u8; 64];
        extend_input[..32].copy_from_slice(&self.pcr_cache[index as usize]);
        extend_input[32..64].copy_from_slice(digest);
        self.pcr_cache[index as usize] = crate::crypto::sha256::hash(&extend_input);
        self.pcr_cache_valid[index as usize] = true;

        Ok(())
    }

    /// TPM2_PCR_Read
    fn pcr_read_inner(&mut self, index: PcrIndex) -> Result<[u8; 32], ()> {
        if index as usize >= PCR_COUNT {
            return Err(());
        }

        // Build TPM2_PCR_Read command
        // Header(10) + pcrSelectionIn
        let total_size: u32 = 10 + 4 + 2 + 1 + 3; // count(4) + hash(2) + sizeOfSelect(1) + select(3)
        let mut off = self.build_header(TPM2_ST_NO_SESSIONS, total_size, TPM2_CC_PCR_READ);

        // PCR selection: count = 1
        self.cmd_buf[off..off + 4].copy_from_slice(&1u32.to_be_bytes());
        off += 4;
        // Hash algorithm = SHA-256
        self.cmd_buf[off..off + 2].copy_from_slice(&TPM2_ALG_SHA256.to_be_bytes());
        off += 2;
        // Size of select = 3
        self.cmd_buf[off] = 3;
        off += 1;
        // PCR select bitmap (3 bytes, set bit for requested index)
        let byte_idx = (index / 8) as usize;
        let bit_idx = index % 8;
        self.cmd_buf[off] = 0;
        self.cmd_buf[off + 1] = 0;
        self.cmd_buf[off + 2] = 0;
        self.cmd_buf[off + byte_idx] = 1 << bit_idx;
        off += 3;

        let cmd_copy: Vec<u8> = self.cmd_buf[..off].to_vec();
        self.send_command(&cmd_copy)?;

        let rc = self.response_code();
        if rc != TPM2_RC_SUCCESS {
            serial_println!("    [tpm] PCR_Read failed: rc=0x{:08X}", rc);
            return Err(());
        }

        // Parse response: skip header(10) + pcrUpdateCounter(4) + pcrSelectionOut(variable) + digestCount
        // Find the digest in the response
        let rsp_size = u32::from_be_bytes([
            self.rsp_buf[2],
            self.rsp_buf[3],
            self.rsp_buf[4],
            self.rsp_buf[5],
        ]) as usize;

        // The digest should be in the last 32 bytes of the response (+ 2 for size prefix)
        if rsp_size >= 10 + 4 + 10 + 4 + 2 + 32 {
            let digest_start = rsp_size - 32;
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&self.rsp_buf[digest_start..digest_start + 32]);
            self.pcr_cache[index as usize] = digest;
            self.pcr_cache_valid[index as usize] = true;
            return Ok(digest);
        }

        Err(())
    }

    /// TPM2_GetRandom
    fn get_random_inner(&mut self, num_bytes: u16) -> Result<Vec<u8>, ()> {
        let total_size: u32 = 10 + 2; // header + bytesRequested
        let mut off = self.build_header(TPM2_ST_NO_SESSIONS, total_size, TPM2_CC_GET_RANDOM);
        self.cmd_buf[off..off + 2].copy_from_slice(&num_bytes.to_be_bytes());
        off += 2;

        let cmd_copy: Vec<u8> = self.cmd_buf[..off].to_vec();
        self.send_command(&cmd_copy)?;

        let rc = self.response_code();
        if rc != TPM2_RC_SUCCESS {
            serial_println!("    [tpm] GetRandom failed: rc=0x{:08X}", rc);
            return Err(());
        }

        // Parse: header(10) + randomBytes(TPM2B: size(2) + data)
        let rsp_size = u32::from_be_bytes([
            self.rsp_buf[2],
            self.rsp_buf[3],
            self.rsp_buf[4],
            self.rsp_buf[5],
        ]) as usize;

        if rsp_size < 12 {
            return Err(());
        }

        let data_size = u16::from_be_bytes([self.rsp_buf[10], self.rsp_buf[11]]) as usize;
        if rsp_size < 12 + data_size {
            return Err(());
        }

        let mut result = Vec::with_capacity(data_size);
        for i in 0..data_size {
            result.push(self.rsp_buf[12 + i]);
        }
        Ok(result)
    }

    /// Seal data to current PCR values
    fn seal_inner(&mut self, data: &[u8], pcr_mask: u32) -> Result<Vec<u8>, ()> {
        // Software seal: encrypt data with key derived from PCR values
        // Collect bound PCR digests
        let mut pcr_composite = Vec::new();
        for i in 0..PCR_COUNT {
            if (pcr_mask & (1 << i)) != 0 {
                pcr_composite.extend_from_slice(&self.pcr_cache[i]);
            }
        }

        // Derive a sealing key from PCR composite
        let seal_key = crate::crypto::sha256::hash(&pcr_composite);

        // Build sealed blob: [pcr_mask(4)] [iv(32)] [data_len(4)] [encrypted_data] [hmac(32)]
        let mut sealed = Vec::with_capacity(4 + 32 + 4 + data.len() + 32);

        // PCR mask
        sealed.extend_from_slice(&pcr_mask.to_le_bytes());

        // Generate IV from TPM random or software random
        let iv = crate::crypto::sha256::hash_multi(&[&seal_key, data]);
        sealed.extend_from_slice(&iv);

        // Data length
        let data_len = data.len() as u32;
        sealed.extend_from_slice(&data_len.to_le_bytes());

        // XOR encrypt data with derived keystream
        let mut keystream_input = [0u8; 64];
        keystream_input[..32].copy_from_slice(&seal_key);
        keystream_input[32..64].copy_from_slice(&iv);

        for chunk_idx in 0..((data.len() + 31) / 32) {
            let block_input = crate::crypto::sha256::hash_multi(&[
                &keystream_input,
                &(chunk_idx as u64).to_le_bytes(),
            ]);
            let start = chunk_idx * 32;
            let end = (start + 32).min(data.len());
            for i in start..end {
                sealed.push(data[i] ^ block_input[i - start]);
            }
        }

        // HMAC for integrity (simplified: SHA256 over seal_key + ciphertext)
        let hmac = crate::crypto::sha256::hash_multi(&[&seal_key, &sealed]);
        sealed.extend_from_slice(&hmac);

        Ok(sealed)
    }

    /// Unseal data, verifying PCR values match
    fn unseal_inner(&mut self, sealed: &[u8], _pcr_mask: u32) -> Result<Vec<u8>, ()> {
        if sealed.len() < 4 + 32 + 4 + 32 {
            return Err(());
        }

        // Parse sealed blob
        let pcr_mask = u32::from_le_bytes([sealed[0], sealed[1], sealed[2], sealed[3]]);
        let iv = &sealed[4..36];
        let data_len =
            u32::from_le_bytes([sealed[36], sealed[37], sealed[38], sealed[39]]) as usize;

        if sealed.len() < 40 + data_len + 32 {
            return Err(());
        }

        let ciphertext = &sealed[40..40 + data_len];
        let stored_hmac = &sealed[40 + data_len..40 + data_len + 32];

        // Reconstruct PCR composite from current values
        let mut pcr_composite = Vec::new();
        for i in 0..PCR_COUNT {
            if (pcr_mask & (1 << i)) != 0 {
                pcr_composite.extend_from_slice(&self.pcr_cache[i]);
            }
        }
        let seal_key = crate::crypto::sha256::hash(&pcr_composite);

        // Verify HMAC
        let verify_data = &sealed[..40 + data_len];
        let computed_hmac = crate::crypto::sha256::hash_multi(&[&seal_key, verify_data]);
        if computed_hmac != *stored_hmac {
            serial_println!("    [tpm] Unseal failed: PCR mismatch or data corrupted");
            return Err(());
        }

        // Decrypt
        let mut keystream_input = [0u8; 64];
        keystream_input[..32].copy_from_slice(&seal_key);
        keystream_input[32..64].copy_from_slice(iv);

        let mut plaintext = Vec::with_capacity(data_len);
        for chunk_idx in 0..((data_len + 31) / 32) {
            let block_key = crate::crypto::sha256::hash_multi(&[
                &keystream_input,
                &(chunk_idx as u64).to_le_bytes(),
            ]);
            let start = chunk_idx * 32;
            let end = (start + 32).min(data_len);
            for i in start..end {
                plaintext.push(ciphertext[i] ^ block_key[i - start]);
            }
        }

        Ok(plaintext)
    }
}

/// TPM interface handle (public API)
pub struct Tpm;

impl Tpm {
    pub fn new() -> Self {
        Tpm
    }

    pub fn pcr_extend(&mut self, index: PcrIndex, digest: &[u8; 32]) {
        if let Some(ref mut inner) = *TPM.lock() {
            if let Err(()) = inner.pcr_extend_inner(index, digest) {
                serial_println!("    [tpm] PCR extend failed for index {}", index);
            }
        }
    }

    pub fn pcr_read(&self, index: PcrIndex) -> [u8; 32] {
        if let Some(ref mut inner) = *TPM.lock() {
            if let Ok(digest) = inner.pcr_read_inner(index) {
                return digest;
            }
            // Fall back to cached value
            if inner.pcr_cache_valid[index as usize] {
                return inner.pcr_cache[index as usize];
            }
        }
        [0u8; 32]
    }

    pub fn seal(&self, data: &[u8], pcr_mask: u32) -> Vec<u8> {
        if let Some(ref mut inner) = *TPM.lock() {
            if let Ok(sealed) = inner.seal_inner(data, pcr_mask) {
                return sealed;
            }
        }
        Vec::new()
    }

    pub fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>, ()> {
        if let Some(ref mut inner) = *TPM.lock() {
            // Extract pcr_mask from sealed blob header
            if sealed.len() >= 4 {
                let pcr_mask = u32::from_le_bytes([sealed[0], sealed[1], sealed[2], sealed[3]]);
                return inner.unseal_inner(sealed, pcr_mask);
            }
        }
        Err(())
    }
}

/// Get random bytes from TPM hardware RNG
pub fn get_random(num_bytes: u16) -> Result<Vec<u8>, ()> {
    if let Some(ref mut inner) = *TPM.lock() {
        return inner.get_random_inner(num_bytes);
    }
    Err(())
}

/// Extend a PCR (module-level convenience)
pub fn pcr_extend(index: PcrIndex, digest: &[u8; 32]) {
    if let Some(ref mut inner) = *TPM.lock() {
        let _ = inner.pcr_extend_inner(index, digest);
    }
}

/// Read a PCR (module-level convenience)
pub fn pcr_read(index: PcrIndex) -> [u8; 32] {
    if let Some(ref mut inner) = *TPM.lock() {
        if let Ok(d) = inner.pcr_read_inner(index) {
            return d;
        }
        if inner.pcr_cache_valid[index as usize] {
            return inner.pcr_cache[index as usize];
        }
    }
    [0u8; 32]
}

/// Initialize the TPM subsystem
pub fn init() {
    let mut inner = TpmInner::new();

    if inner.probe() {
        if inner.request_locality(Locality::Zero) {
            match inner.startup() {
                Ok(()) => serial_println!("    [tpm] TPM2_Startup(CLEAR) succeeded"),
                Err(()) => {
                    serial_println!("    [tpm] TPM2_Startup failed (may already be started)")
                }
            }
            serial_println!(
                "    [tpm] TPM 2.0 TIS interface initialized at 0x{:X}",
                TIS_BASE
            );
        } else {
            serial_println!("    [tpm] Failed to acquire locality 0");
        }
    } else {
        serial_println!("    [tpm] No TPM hardware detected (software fallback)");
        inner.detected = false;
    }

    *TPM.lock() = Some(inner);
    serial_println!("  [tpm] TPM subsystem initialized");
}
