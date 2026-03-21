# Lock Chain — Exodus Kernel Security Subsystem
**Date:** 2026-03-19
**Status:** Spec Review Pass 2
**Owner:** Collin Hoag (Genesis / Supreme Commander)

---

## Overview

Lock Chain is a bare-metal cryptographic security subsystem embedded directly in the Exodus kernel. It provides three interlocking guarantees across the entire Hoags ecosystem:

1. **Access control** — Only authorized entities can unlock ecosystem data
2. **Tamper detection** — Any unauthorized modification to any asset is detected within one verification interval
3. **Privilege enforcement** — Bots cannot escalate beyond their clearance depth in the Genesis hierarchy

Lock Chain lives in `src/life/lock_chain.rs` and runs on a staggered interval of **every 97 ticks with offset 0** — consistent with the existing prime-interval scheduling pattern in life_tick.rs. Integrity check runs after IMMUNE logic and before behavioral output (CHEMISTRY).

---

## Architecture

### Three Layers

```
┌─────────────────────────────────────────┐
│            EXODUS LOCK CHAIN            │
├─────────────────────────────────────────┤
│  LAYER 1: SHARE ENGINE                  │
│  4 auth factors gate 4 stored shares    │
│  Any 2 factors pass → reconstruct key   │
│  Factor A: Phone signing key (ed25519)  │
│  Factor B: Voice biometric (fixed-pt)   │
│  Factor C: System state proof (BLAKE2b) │
│  Factor D: Certificate fingerprint      │
├─────────────────────────────────────────┤
│  LAYER 2: MERKLE VAULT                  │
│  Ecosystem data organized as leaves     │
│  Root hash = single truth fingerprint   │
│  Any tamper → root mismatch → alarm     │
├─────────────────────────────────────────┤
│  LAYER 3: CLEARANCE GATE                │
│  Bot IDs hardcoded to clearance depth   │
│  Genesis = depth 0 (full access)        │
│  Rogue process = subtree hash invalid   │
└─────────────────────────────────────────┘
```

---

## Layer 1: Share Engine

### Architecture Clarification — Auth Factors vs. Shamir Shares

The four "shares" are implemented as a two-step system:

1. **Auth factors** — each factor is independently verified (phone signature, voice match, system state hash, cert fingerprint)
2. **Stored field elements** — four actual Shamir field elements are stored encrypted in kernel memory at enrollment time, each protected behind one auth factor

When an auth factor passes, it releases its corresponding stored field element. Any 2 released elements are fed into Lagrange interpolation to reconstruct the MasterKey. This cleanly separates "biometric authentication" from "Shamir mathematics."

```
Auth Factor passes → releases stored field element (32 bytes in GF(p))
2 field elements → Lagrange interpolation over GF(p) → MasterKey
```

### Field: GF(p) using Ed25519 Prime

Shamir arithmetic runs over GF(p) where p = ED25519_L (the Ed25519 group order), already defined in `src/crypto/ed25519.rs`. This reuses existing constants and avoids implementing non-standard GF(2^256) arithmetic.

```
p = 2^252 + 27742317777372353535851937790883648493
```

Degree-1 polynomial: `f(x) = MasterKey + a1*x mod p`
Four shares: `s_i = f(i) mod p` for i = 1,2,3,4
Reconstruction: Lagrange with any 2 (x_i, s_i) pairs.

### The Four Auth Factors

| Factor | Source | Verification | Releases |
|--------|--------|--------------|---------|
| A | Phone (LAN, ed25519) | Challenge-response signature | Stored share s_1 |
| B | Voice biometric (fixed-point) | Match against enrolled feature hash | Stored share s_2 |
| C | System state (BLAKE2b) | Recompute and compare | Stored share s_3 |
| D | Certificate fingerprint (BLAKE2b) | Compare to enrolled cert hash | Stored share s_4 |

### Factor A — Phone Key (LAN Challenge-Response)

Uses existing `src/crypto/ed25519.rs` — **no external crates added**.

```
Protocol:
1. Exodus sends UDP challenge: { nonce: [u8; 32], timestamp_ticks: u64 }
   UDP port: 47473 (hardcoded, no allocation needed for fixed packet)
2. Phone signs with stored ed25519 private key
3. Phone returns: { signature: [u8; 64], pubkey: [u8; 32] }
4. Exodus verifies: ed25519_verify(nonce, signature, enrolled_pubkey)
5. Nonce deduplication: compare against last_nonce: [u8; 32] stored in LockChain struct
   If nonce == last_nonce → reject (replay attack)
   Accept window: 5 seconds (measured in ticks at 1kHz = 5000 ticks)
6. On success: last_nonce updated, share s_1 released

No heap allocation: fixed packet sizes, stack-only buffers.
Note: UDP send/receive goes through existing src/net/udp.rs which uses alloc internally.
The no-heap constraint applies to lock_chain.rs struct and critical path logic only.
```

### Factor B — Voice Biometric (Fixed-Point)

**No floats.** All computation uses Q16.16 fixed-point matching the existing `src/math/q16.rs` pattern.

```
Enrollment (3 captures):
1. Capture 64 audio samples per frame (fixed-size buffer)
2. Compute energy per frequency band using fixed-point arithmetic (8 bands)
3. Produce 64-element Q16 feature vector — consensus of 3 captures
4. Store: enrolled_voice_hash = BLAKE2b(feature_vector_bytes)
5. Store: enrolled_voice_params (tolerance bounds, per-band min/max)

Verification:
1. Capture new audio, compute 64-element Q16 feature vector
2. Check each band within stored tolerance bounds (fixed-point comparison)
3. If within tolerance: compute BLAKE2b(feature_vector_bytes) and compare to enrolled hash
4. Match → release share s_2
```

No fuzzy extractor library needed — tolerance bounds stored at enrollment achieve the same stability guarantee.

### Factor C — System State Proof

```
tick_mod: u16 = (kernel_age_at_enrollment % 65536) as u16
Stored at enrollment in: enrolled_tick_mod: u16 (plaintext, public parameter)

Share C verification:
  state_input = BLAKE2b(elf_boot_hash || enrolled_tick_mod.to_le_bytes())
  compare to enrolled_state_hash stored at enrollment
  Match → release share s_3

elf_boot_hash: measured hash of ELF sections captured once at first boot,
               stored in lock_chain enrollment data (replaces "kernel modules as leaves").
```

### Factor D — Certificate Fingerprint

```
enrolled_cert_hash = BLAKE2b(cert_pubkey_bytes) captured at enrollment
Verification: recompute BLAKE2b(cert_pubkey_bytes), compare to stored hash
Match → release share s_4
```

### Valid Unlock Combinations

| Combo | Factors | Trust Level | Use Case |
|-------|---------|-------------|---------|
| A + C | Phone + System | High (automated) | Primary daily unlock |
| A + B | Phone + Voice | Highest (human present) | Sensitive operations |
| B + C | Voice + System | Medium | Phone unavailable fallback |
| A + D | Phone + Cert | High | Inter-bot auth |
| B + D | Voice + Cert | Medium | Identity-bound access |
| C + D | System + Cert | Low — background processes only, no sensitive subtrees | Automated background |

**Trust-to-operation mapping:** C+D (lowest trust) may only access Depth 2 (Persona) subtrees. All higher-trust combos required for Commander (Depth 1) or Genesis (Depth 0) operations.

### Enrollment Procedure

```
1. Generate MasterKey: 32 bytes via RDRAND (existing src/crypto/random.rs)
2. Compute f(x) = MasterKey + a1*x mod ED25519_L (random a1 from RDRAND)
3. Compute 4 shares: s_i = f(i) mod ED25519_L for i = 1..4
4. Enroll Factor A: generate ed25519 keypair on phone, store pubkey in kernel
5. Enroll Factor B: 3 voice captures → Q16 feature vector → enrolled_voice_hash + bounds
6. Compute enrolled_state_hash (Factor C) using current elf_boot_hash + tick_mod
7. Compute enrolled_cert_hash (Factor D) from cert pubkey bytes
8. Encrypt each share s_i under its factor's derived key, store encrypted blobs
9. Test all 6 combos: each must reconstruct MasterKey — verify before destroying plaintext
10. Zeroize MasterKey, a1, and all plaintext s_i from memory (explicit volatile write + fence)
11. Store: 4 encrypted share blobs, enrolled_pubkey, enrolled_voice_hash,
           enrolled_voice_params, enrolled_state_hash, enrolled_cert_hash, enrolled_tick_mod
```

### MasterKey Memory Safety

```rust
// After use, explicit zeroize — volatile write prevents compiler optimization
fn zeroize_key(key: &mut [u8; 32]) {
    for byte in key.iter_mut() {
        unsafe { core::ptr::write_volatile(byte, 0u8) };
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
```

---

## Layer 2: Merkle Vault

### Tree Structure

```
                    ROOT HASH (32 bytes, BLAKE2b)
                   /                              \
          HOAGS SUBTREE                    EXODUS SUBTREE
         /              \                 /               \
    BOT HIERARCHY     BID DATA      ELF BOOT HASH     LIFE MODULE STATE
    /    |    \           |               |                    |
Genesis Cmdr  Persona  Docs/Prices   (measured once        endocrine...
  hash  hash   hash     hash          at boot, static)       hash
```

### Leaf Types

| Leaf | Content Hashed | Update Frequency |
|------|----------------|-----------------|
| Bot state | Serialized bot struct snapshot | Every 100 ticks per bot |
| Bid documents | File contents + metadata | On write |
| ELF boot hash | Measured ELF section hash (captured at boot, static) | Boot only |
| Life module state | Module field snapshot | Every 97 ticks (same interval) |
| DAVA/Nexus state | Sanctuary layer snapshots | On sync event |
| Enrolled keys | All enrollment hashes and pubkeys | Enrollment only |

Note: "kernel modules as runtime leaves" is dropped — the kernel is a single flat binary. ELF section measurement at boot covers kernel integrity.

### Verification Interval

Lock Chain runs **every 97 ticks with offset 0**: `if age % 97 == 0 { lock_chain.verify(); }`

This is consistent with the staggered prime-interval pattern in life_tick.rs. 97 is prime and uncrowded relative to other module intervals.

**Performance budget:** Merkle recompute must complete in fewer than 500 instruction cycles for leaf sets up to 64 nodes (measured against existing kernel benchmark patterns).

### Tamper Response

```
TamperDetected:
  1. Identify diverged subtree
  2. Freeze all write access to that subtree (clearance gate blocks writes)
  3. Emit LockEvent::TamperDetected to neural bus (ANIMA threat signal)
  4. Feed into IMMUNE module (raises inflammation analog)
  5. Log: subtree ID, expected hash, observed hash, tick timestamp
  6. Lockdown: set lockdown = true → all Factor A-D auth attempts blocked
  7. Await Genesis (Collin) to physically lift lockdown at console
```

---

## Layer 3: Clearance Gate

### Bot Hierarchy — Hardcoded Kernel Constants

The kernel does not learn the bot hierarchy from the FastAPI layer. Bot IDs and clearance depths are compile-time constants in `lock_chain.rs`, mirroring the Genesis hierarchy:

```rust
const BOT_CLEARANCES: &[(BotId, u8, &[SubtreeId])] = &[
    (BOT_GENESIS,   0, &[SUBTREE_ALL]),
    (BOT_OPS,       1, &[SUBTREE_OPS, SUBTREE_BOT_HIERARCHY]),
    (BOT_FAMILY,    1, &[SUBTREE_FAMILY, SUBTREE_BOT_HIERARCHY]),
    (BOT_WORK,      1, &[SUBTREE_WORK, SUBTREE_BOT_HIERARCHY]),
    (BOT_CODE,      1, &[SUBTREE_CODE, SUBTREE_BOT_HIERARCHY]),
    (BOT_OFFGRID,   2, &[SUBTREE_OPS]),
    (BOT_EMERGENCY, 2, &[SUBTREE_OPS]),
    // ... all 26 bots
];
```

`ClearanceMap` is a fixed-size array lookup — no heap, O(n) linear scan over 26 entries.

### Privilege Violation Detection

```
Every vault write:
  1. Caller presents BotId
  2. Gate scans BOT_CLEARANCES for matching BotId → gets (depth, allowed_subtrees)
  3. If target subtree not in allowed_subtrees → LockEvent::PrivilegeViolation
  4. Kill calling process, inject threat into IMMUNE module
  5. 3 violations from same BotId within 1000 ticks → auto-Lockdown
```

---

## Kernel Integration

### Scheduling

```rust
// In life_tick() or equivalent dispatcher:
if age % 97 == 0 {
    lock_chain.tick(age, &mut neural_bus, &mut immune);
}
```

### Core Struct

```rust
pub struct LockChain {
    // Enrollment data (set once, never changed)
    enrolled_phone_pubkey: [u8; 32],
    enrolled_voice_hash: [u8; 32],
    enrolled_voice_params: VoiceParams,   // Q16 band tolerance bounds
    enrolled_state_hash: [u8; 32],
    enrolled_cert_hash: [u8; 32],
    enrolled_tick_mod: u16,

    // Encrypted share blobs (each 64 bytes: 32 share + 32 auth tag)
    share_blobs: [[u8; 64]; 4],

    // Runtime state
    merkle_root: [u8; 32],
    last_nonce: [u8; 32],
    last_nonce_tick: u64,
    violation_counts: [u8; 26],    // per-bot violation counter
    tick_last_verified: u64,
    lockdown: bool,
}

pub enum LockEvent {
    Unlocked { factors_used: [FactorId; 2] },
    TamperDetected { subtree: SubtreeId, expected: [u8; 32], got: [u8; 32] },
    PrivilegeViolation { bot: BotId, attempted_subtree: SubtreeId },
    PhoneChallengeIssued { nonce: [u8; 32] },
    PhoneVerified,
    VoiceVerified,
    SystemStateVerified,
    Lockdown,
    LockdownLifted,
}
```

### no_std Constraints (Resolved)

| Constraint | Resolution |
|------------|-----------|
| No external crypto crates | Use `src/crypto/ed25519.rs`, `src/crypto/blake2.rs` (existing) |
| No floats in life path | Q16.16 fixed-point via `src/math/q16.rs` for voice features |
| UDP uses heap internally | No-heap claim scoped to `lock_chain.rs` struct and critical path only |
| RDRAND uses unsafe | Existing pattern in `src/crypto/random.rs` — no new unsafe blocks |
| No heap in LockChain struct | All fields are fixed-size arrays — confirmed |

### Watchdog — Tick Starvation Protection

```rust
// At each lock_chain.tick() call:
if age - tick_last_verified > 110 {
    // Verification overdue (>1 missed interval) — emit warning to neural bus
    neural_bus.emit(LockEvent::VerificationOverdue { ticks_late: age - tick_last_verified - 97 });
}
```

---

## Threat Model

| Threat | Mechanism | Response |
|--------|-----------|----------|
| Stolen files | Encrypted under MasterKey, never stored plaintext | Unreadable without chain unlock |
| Rogue bot privilege escalation | Clearance gate (compile-time constants) | Kill + IMMUNE signal |
| Data tampering | Merkle root every 97 ticks | Freeze + Lockdown |
| Replay attack (phone nonce) | last_nonce deduplication + 5000-tick window | Stale/duplicate → reject |
| Voice spoofing | Q16 tolerance bounds from enrollment | Out-of-tolerance → Factor B invalid |
| Cold boot memory remanence | Explicit zeroize (volatile write + fence) after MasterKey use | No plaintext persists in RAM |
| Tick starvation (interrupt flood) | Watchdog: overdue alert at 200+ ticks without verification | Neural bus warning emitted |
| Share C forgery | Requires knowing elf_boot_hash AND enrolled_tick_mod AND state hash | C+D restricted to Depth 2 only |

---

## Files to Create / Modify

| File | Action | Purpose |
|------|--------|---------|
| `src/life/lock_chain.rs` | CREATE | Core Lock Chain module (~500 lines) |
| `src/life/merkle.rs` | CREATE | Merkle tree, BLAKE2b leaves (~200 lines) |
| `src/crypto/shamir.rs` | CREATE | Shamir SSS over GF(ED25519_L) (~150 lines) |
| `src/life/voice_enroll.rs` | CREATE | Q16 voice feature extraction + enrollment (~150 lines) |
| `src/life/mod.rs` | MODIFY | Register new life modules |
| `src/main.rs` / kernel init | MODIFY | Initialize LockChain before bot hierarchy loads |
| `src/neural_bus.rs` | MODIFY | Add LockEvent variants |
| `tests/test_lock_chain.rs` | CREATE | Test stub for success criteria validation |
| Phone companion app | CREATE (separate repo/dir) | Python/Termux script: listen UDP 47473, sign challenge, respond |

Note: `src/crypto/shamir.rs` placed in `src/crypto/` to match existing crypto module architecture.

### Phone Companion App — Protocol Spec

```
UDP Port: 47473
Challenge packet (Exodus → Phone): 40 bytes
  [0..32]  nonce: [u8; 32]
  [32..40] timestamp_ticks: u64 (little-endian)

Response packet (Phone → Exodus): 96 bytes
  [0..64]  signature: [u8; 64]  (ed25519 signature of nonce)
  [64..96] pubkey: [u8; 32]     (sender's ed25519 public key)

Timeout: 5000 ticks (~5 seconds at 1kHz)
Phone implementation: Python script in Termux (Android) or equivalent
Private key stored in: ~/.lock_chain_key (chmod 600)
```

---

## Success Criteria

- [ ] Kernel boots and Lock Chain initializes (enrollment data loaded) before any bot process runs
- [ ] A + C unlock completes in under 5000 ticks from challenge issue to MasterKey reconstructed
- [ ] B + C unlock (voice fallback) produces matching MasterKey to A + C for same enrollment
- [ ] Deliberate single-byte file tamper detected within 97 ticks (one verification interval)
- [ ] Persona (depth 2) attempting Commander (depth 1) subtree write: process killed, LockEvent emitted
- [ ] Replay of a used phone nonce within 5000-tick window: rejected with no share released
- [ ] Lockdown state: all auth attempts blocked, neural bus receives TamperDetected event
- [ ] Merkle recompute for 64-node tree completes in under 500 instruction cycles
- [ ] No new `unsafe` blocks added beyond existing hardware I/O pattern in `src/crypto/random.rs`
- [ ] `test_lock_chain.rs` passes: enrollment → A+C unlock → tamper inject → detection

---

## Out of Scope

- LAN peer access (Gary, Nyvona) — not needed, deferred indefinitely
- Full PKI / Certificate Authority — Share D uses fingerprint only
- Remote key management — phone key is LAN-only, no internet
- Python / FastAPI bridge — kernel-native only
