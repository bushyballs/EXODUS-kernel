use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware capability flags
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CryptoCapFlags(pub u32);

impl CryptoCapFlags {
    pub const AESNI: u32  = 1;
    pub const RDRAND: u32 = 2;
    pub const RDSEED: u32 = 4;
    pub const SHA: u32    = 8;

    pub const fn empty() -> Self { Self(0) }

    pub fn set(&mut self, flag: u32) { self.0 |= flag; }
    pub fn has(&self, flag: u32) -> bool { (self.0 & flag) != 0 }
}

// ---------------------------------------------------------------------------
// AES key material (AES-128: 11 round keys of 16 bytes each)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct AesKey {
    pub round_keys: [[u8; 16]; 11],
}

impl AesKey {
    pub const fn empty() -> Self {
        Self { round_keys: [[0u8; 16]; 11] }
    }
}

// ---------------------------------------------------------------------------
// Enclave region
// ---------------------------------------------------------------------------

pub const ENCLAVE_BASE: usize = 0x00200000; // 2 MB — fixed protected region
pub const ENCLAVE_SIZE: usize = 65536;      // 64 KB

#[derive(Copy, Clone)]
pub struct EnclaveRegion {
    pub base:   usize,
    pub size:   usize,
    pub locked: bool,
}

impl EnclaveRegion {
    pub const fn new() -> Self {
        Self {
            base:   ENCLAVE_BASE,
            size:   ENCLAVE_SIZE,
            locked: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Vault state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CryptoVaultState {
    pub caps:              CryptoCapFlags,
    pub aesni_available:   bool,
    pub rdrand_available:  bool,
    pub rdseed_available:  bool,
    pub enclave:           EnclaveRegion,
    pub entropy_pool:      [u8; 64],
    pub entropy_idx:       usize,
    pub ops_completed:     u32,
    pub ops_failed:        u32,
    pub trust_score:       u16,
    pub identity_hash:     [u8; 16],
}

impl CryptoVaultState {
    pub const fn new() -> Self {
        Self {
            caps:             CryptoCapFlags::empty(),
            aesni_available:  false,
            rdrand_available: false,
            rdseed_available: false,
            enclave:          EnclaveRegion::new(),
            entropy_pool:     [0u8; 64],
            entropy_idx:      0,
            ops_completed:    0,
            ops_failed:       0,
            trust_score:      0,
            identity_hash:    [0u8; 16],
        }
    }
}

pub static STATE: Mutex<CryptoVaultState> = Mutex::new(CryptoVaultState::new());

// ---------------------------------------------------------------------------
// Unsafe inline asm helpers
// ---------------------------------------------------------------------------

/// CPUID — preserves rbx (reserved by LLVM) via push/pop.
/// Returns (eax, ebx, ecx, edx).
unsafe fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov {ebx:e}, ebx",
        "pop rbx",
        inout("eax") leaf  => eax,
        inout("ecx") subleaf => ecx,
        ebx = out(reg) ebx,
        out("edx") edx,
        options(nostack, preserves_flags),
    );
    (eax, ebx, ecx, edx)
}

/// RDRAND — returns None if CF=0 (hardware not ready).
unsafe fn rdrand_u64() -> Option<u64> {
    let mut val: u64;
    let ok: u8;
    core::arch::asm!(
        "rdrand {val}",
        "setc {ok}",
        val = out(reg) val,
        ok  = out(reg_byte) ok,
        options(nostack),
    );
    if ok != 0 { Some(val) } else { None }
}

/// RDSEED — same contract as rdrand_u64.
unsafe fn rdseed_u64() -> Option<u64> {
    let mut val: u64;
    let ok: u8;
    core::arch::asm!(
        "rdseed {val}",
        "setc {ok}",
        val = out(reg) val,
        ok  = out(reg_byte) ok,
        options(nostack),
    );
    if ok != 0 { Some(val) } else { None }
}

/// Read TSC as a deterministic fallback entropy source.
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem, preserves_flags),
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// AESENC — one round of AES encryption in-place.
/// Falls back to XOR if AES-NI unavailable (caller guards).
unsafe fn aes_enc_block(block: &mut [u8; 16], round_key: &[u8; 16]) {
    // Load block and round_key into xmm registers, run AESENC, store back.
    // We transmute the 16-byte arrays to raw pointers for the asm operands.
    let blk_ptr  = block.as_mut_ptr()    as *mut u8;
    let rkey_ptr = round_key.as_ptr()    as *const u8;
    core::arch::asm!(
        // xmm0 = *blk_ptr  (128-bit load)
        "movdqu xmm0, [{blk}]",
        // xmm1 = *rkey_ptr (128-bit load)
        "movdqu xmm1, [{rk}]",
        // one AES-NI encryption round
        "aesenc xmm0, xmm1",
        // store result back
        "movdqu [{blk}], xmm0",
        blk  = in(reg) blk_ptr,
        rk   = in(reg) rkey_ptr,
        out("xmm0") _,
        out("xmm1") _,
        options(nostack),
    );
}

/// XOR fallback encryption: block ^= key (used when AES-NI absent).
fn xor_block(block: &mut [u8; 16], key: &[u8; 16]) {
    let mut i = 0usize;
    while i < 16 {
        block[i] ^= key[i];
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Derive 64 bytes of entropy. Prefers RDRAND, falls back to TSC mixing.
fn collect_entropy(rdrand_ok: bool, buf: &mut [u8; 64]) {
    let mut i = 0usize;
    while i < 64 {
        let raw: u64 = if rdrand_ok {
            // SAFETY: rdrand_u64 is safe to call; we handle None.
            unsafe { rdrand_u64() }.unwrap_or_else(|| unsafe { rdtsc() })
        } else {
            unsafe { rdtsc() }
        };
        // Spread 8 bytes into the pool.
        let remaining = 64usize.saturating_sub(i);
        let chunk = if remaining < 8 { remaining } else { 8 };
        let mut b = 0usize;
        while b < chunk {
            buf[i.saturating_add(b)] = ((raw >> (b.saturating_mul(8))) & 0xff) as u8;
            b = b.saturating_add(1);
        }
        i = i.saturating_add(8);
    }
}

/// Derive a 16-byte identity from CPUID vendor string XOR'd with entropy bytes.
fn derive_identity(rdrand_ok: bool, pool: &[u8; 64]) -> [u8; 16] {
    // CPUID leaf 0: EBX:EDX:ECX = vendor string (12 bytes).
    let (_, vb, vc, vd) = unsafe { cpuid(0, 0) };
    let mut id = [0u8; 16];
    // Embed vendor bytes in first 12 slots.
    id[0]  = (vb & 0xff) as u8;
    id[1]  = ((vb >> 8)  & 0xff) as u8;
    id[2]  = ((vb >> 16) & 0xff) as u8;
    id[3]  = ((vb >> 24) & 0xff) as u8;
    id[4]  = (vd & 0xff) as u8;
    id[5]  = ((vd >> 8)  & 0xff) as u8;
    id[6]  = ((vd >> 16) & 0xff) as u8;
    id[7]  = ((vd >> 24) & 0xff) as u8;
    id[8]  = (vc & 0xff) as u8;
    id[9]  = ((vc >> 8)  & 0xff) as u8;
    id[10] = ((vc >> 16) & 0xff) as u8;
    id[11] = ((vc >> 24) & 0xff) as u8;
    // Slots 12-15: extra CPUID leaf 1 EAX (processor signature) XOR'd.
    let (sig, _, _, _) = unsafe { cpuid(1, 0) };
    id[12] = (sig & 0xff) as u8;
    id[13] = ((sig >> 8)  & 0xff) as u8;
    id[14] = ((sig >> 16) & 0xff) as u8;
    id[15] = ((sig >> 24) & 0xff) as u8;

    // XOR with first 16 bytes of entropy pool.
    let mut i = 0usize;
    while i < 16 {
        id[i] ^= pool[i];
        i = i.saturating_add(1);
    }

    let _ = rdrand_ok; // used indirectly via pool collection
    id
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Detect hardware crypto capabilities, seed entropy pool, compute identity.
pub fn init() {
    let mut s = STATE.lock();

    // --- CPUID leaf 1: ECX bit 25 = AES-NI, bit 30 = RDRAND ---
    let (_, _, ecx1, _) = unsafe { cpuid(1, 0) };
    let aesni  = (ecx1 & (1 << 25)) != 0;
    let rdrand = (ecx1 & (1 << 30)) != 0;

    // --- CPUID leaf 7, subleaf 0: EBX bit 18 = RDSEED ---
    let (_, ebx7, _, _) = unsafe { cpuid(7, 0) };
    let rdseed = (ebx7 & (1 << 18)) != 0;

    s.aesni_available  = aesni;
    s.rdrand_available = rdrand;
    s.rdseed_available = rdseed;

    let mut flags = CryptoCapFlags::empty();
    if aesni  { flags.set(CryptoCapFlags::AESNI);  }
    if rdrand { flags.set(CryptoCapFlags::RDRAND); }
    if rdseed { flags.set(CryptoCapFlags::RDSEED); }
    s.caps = flags;

    // --- Collect 64 bytes of hardware entropy ---
    collect_entropy(rdrand, &mut s.entropy_pool);
    s.entropy_idx = 0;

    // --- Derive identity hash ---
    s.identity_hash = derive_identity(rdrand, &s.entropy_pool);

    // --- Initial trust score boost from hardware capabilities ---
    if aesni  { s.trust_score = s.trust_score.saturating_add(200); }
    if rdrand { s.trust_score = s.trust_score.saturating_add(100); }
    if rdseed { s.trust_score = s.trust_score.saturating_add(50);  }
    if s.trust_score > 1000 { s.trust_score = 1000; }

    serial_println!(
        "[crypto] vault online — aesni={} rdrand={} rdseed={} trust={}",
        aesni, rdrand, rdseed, s.trust_score
    );
}

/// Fill `buf` with hardware entropy (RDRAND or TSC fallback).
/// Increments ops_completed.
pub fn get_entropy(buf: &mut [u8]) {
    let mut s = STATE.lock();
    let rdrand_ok = s.rdrand_available;
    let pool = s.entropy_pool;       // snapshot; we replenish below if idx wraps
    let pool_len = pool.len();       // 64
    let buf_len  = buf.len();

    let mut i = 0usize;
    while i < buf_len {
        let idx = s.entropy_idx % pool_len;
        buf[i] = pool[idx];
        s.entropy_idx = s.entropy_idx.saturating_add(1);

        // If we've exhausted the pool, refill it.
        if s.entropy_idx >= pool_len {
            collect_entropy(rdrand_ok, &mut s.entropy_pool);
            s.entropy_idx = 0;
        }
        i = i.saturating_add(1);
    }
    s.ops_completed = s.ops_completed.saturating_add(1);
}

/// Lock the enclave region and log its coordinates.
pub fn seal_enclave() {
    let mut s = STATE.lock();
    s.enclave.locked = true;
    serial_println!(
        "[crypto] enclave sealed base=0x{:x} size={}",
        s.enclave.base, s.enclave.size
    );
}

/// Re-derive identity from current hardware and compare to stored hash.
/// Returns true if they match.
pub fn verify_identity() -> bool {
    let s = STATE.lock();
    let rdrand_ok = s.rdrand_available;
    let stored    = s.identity_hash;
    let pool      = s.entropy_pool;
    drop(s);

    let derived = derive_identity(rdrand_ok, &pool);

    let mut match_ = true;
    let mut i = 0usize;
    while i < 16 {
        if derived[i] != stored[i] {
            match_ = false;
            break;
        }
        i = i.saturating_add(1);
    }
    match_
}

/// Encrypt a 16-byte block in-place with the given key.
/// Uses 10 rounds: AESENC if available, XOR fallback otherwise.
/// Increments ops_completed.
pub fn hardware_encrypt(block: &mut [u8; 16], key: &[u8; 16]) {
    let mut s = STATE.lock();
    let aesni = s.aesni_available;
    s.ops_completed = s.ops_completed.saturating_add(1);
    drop(s);

    let mut round = 0u8;
    while round < 10 {
        if aesni {
            // SAFETY: AES-NI confirmed available, block/key are valid 16-byte arrays.
            unsafe { aes_enc_block(block, key); }
        } else {
            xor_block(block, key);
        }
        round = round.saturating_add(1);
    }
}

/// Return current trust score.
pub fn trust_score() -> u16 {
    STATE.lock().trust_score
}

/// Return total completed operations.
pub fn ops_completed() -> u32 {
    STATE.lock().ops_completed
}

/// Return whether AES-NI was detected.
pub fn aesni_available() -> bool {
    STATE.lock().aesni_available
}

/// Return whether RDRAND was detected.
pub fn rdrand_available() -> bool {
    STATE.lock().rdrand_available
}

/// Per-tick maintenance: entropy top-up, identity re-verify, trust decay/growth.
pub fn tick(age: u32) {
    // --- Every 200 ticks: top up entropy pool ---
    if age > 0 && (age % 200) == 0 {
        let mut s = STATE.lock();
        let rdrand_ok = s.rdrand_available;
        collect_entropy(rdrand_ok, &mut s.entropy_pool);
        s.entropy_idx = 0;
    }

    // --- Every 1000 ticks: re-verify identity ---
    if age > 0 && (age % 1000) == 0 {
        if !verify_identity() {
            serial_println!("[CRYPTO_WARN] identity mismatch");
        }
    }

    // --- Trust score: grow +1/tick capped at 1000, decay -1 if failures ---
    {
        let mut s = STATE.lock();
        if s.ops_failed > 0 {
            s.trust_score = s.trust_score.saturating_sub(1);
        } else {
            s.trust_score = s.trust_score.saturating_add(1);
            if s.trust_score > 1000 { s.trust_score = 1000; }
        }
    }

    // --- Every 500 ticks: status log ---
    if age > 0 && (age % 500) == 0 {
        let s = STATE.lock();
        serial_println!(
            "[crypto] ops={} trust={} entropy_idx={}",
            s.ops_completed, s.trust_score, s.entropy_idx
        );
    }
}
