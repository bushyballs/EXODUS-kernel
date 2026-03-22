#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct KeyLockerState {
    kl_supported: u16,
    kl_aeskle:    u16,
    kl_features:  u16,
    kl_ema:       u16,
}

static STATE: Mutex<KeyLockerState> = Mutex::new(KeyLockerState {
    kl_supported: 0,
    kl_aeskle:    0,
    kl_features:  0,
    kl_ema:       0,
});

// ── CPUID helpers ─────────────────────────────────────────────────────────────

fn has_key_locker() -> bool {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x19 {
        return false;
    }
    let ecx7: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") ecx7,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx7 >> 23) & 1 != 0
}

fn read_leaf19() -> (u32, u32) {
    let eax19: u32;
    let ecx19: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x19u32 => eax19,
            in("ecx") 0u32,
            lateout("ecx") ecx19,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax19, ecx19)
}

// ── Fixed-point helpers ───────────────────────────────────────────────────────

/// Count set bits in the low 5 bits of v (bits 4:0 of EAX).
fn popcount5(mut v: u32) -> u32 {
    v &= 0x1F;
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, returned as u16.
fn ema(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Returns (kl_supported, kl_aeskle, kl_features).
/// All values are u16 in range 0–1000.
fn compute_signals() -> (u16, u16, u16) {
    if !has_key_locker() {
        return (0, 0, 0);
    }

    let (eax19, ecx19) = read_leaf19();

    // kl_supported: any of EAX bits[4:0] set -> 1000 (key locker features exist), else 0
    let kl_supported: u16 = if (eax19 & 0x1F) != 0 { 1000 } else { 0 };

    // kl_aeskle: ECX bit 0 (AESKLE, OS has enabled Key Locker) -> 0 or 1000
    let kl_aeskle: u16 = if (ecx19 & 0x1) != 0 { 1000 } else { 0 };

    // kl_features: popcount of EAX bits[4:0] * 200, capped at 1000 (5 features max)
    //   EAX bit[0] = LOADIWKEY_NoBackup
    //   EAX bit[1] = KeySource encoding
    //   EAX bit[2] = IWKeyOutput
    //   EAX bit[3] = (reserved, counts if set)
    //   EAX bit[4] = IWKEY_randomization
    let feat_count = popcount5(eax19);
    let kl_features_raw = feat_count * 200;
    let kl_features: u16 = if kl_features_raw > 1000 { 1000 } else { kl_features_raw as u16 };

    (kl_supported, kl_aeskle, kl_features)
}

/// Combine signals into the EMA input value (0–1000).
fn compute_ema_input(kl_supported: u16, kl_aeskle: u16, kl_features: u16) -> u16 {
    // kl_supported/4 + kl_aeskle/4 + kl_features/2
    let s = (kl_supported as u32) / 4;
    let a = (kl_aeskle as u32) / 4;
    let f = (kl_features as u32) / 2;
    let sum = s + a + f;
    if sum > 1000 { 1000u16 } else { sum as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Read CPUID leaf 0x19 once and seed the module state.
pub fn init() {
    let (kl_supported, kl_aeskle, kl_features) = compute_signals();
    let ema_input = compute_ema_input(kl_supported, kl_aeskle, kl_features);
    // Seed EMA with first reading so it starts at the real value, not 0.
    let kl_ema = ema_input;

    let mut s = STATE.lock();
    s.kl_supported = kl_supported;
    s.kl_aeskle    = kl_aeskle;
    s.kl_features  = kl_features;
    s.kl_ema       = kl_ema;

    crate::serial_println!(
        "[cpuid_key_locker] init: supported={} aeskle={} features={} ema={}",
        kl_supported, kl_aeskle, kl_features, kl_ema
    );
}

/// Called every tick from the ANIMA life_tick() pipeline.
/// Sampling gate: only executes every 10000 ticks.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (kl_supported, kl_aeskle, kl_features) = compute_signals();
    let ema_input = compute_ema_input(kl_supported, kl_aeskle, kl_features);

    let mut s = STATE.lock();
    s.kl_supported = kl_supported;
    s.kl_aeskle    = kl_aeskle;
    s.kl_features  = kl_features;
    s.kl_ema       = ema(s.kl_ema, ema_input);

    let kl_ema = s.kl_ema;

    crate::serial_println!(
        "[cpuid_key_locker] age={} supported={} aeskle={} features={} ema={}",
        age, kl_supported, kl_aeskle, kl_features, kl_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// 1000 if Key Locker features present (any EAX bits[4:0] set), else 0.
pub fn get_kl_supported() -> u16 {
    STATE.lock().kl_supported
}

/// 1000 if OS has enabled Key Locker instructions (AESKLE), else 0.
pub fn get_kl_aeskle() -> u16 {
    STATE.lock().kl_aeskle
}

/// Feature density: popcount(EAX bits[4:0]) * 200, capped at 1000.
pub fn get_kl_features() -> u16 {
    STATE.lock().kl_features
}

/// EMA of (kl_supported/4 + kl_aeskle/4 + kl_features/2), 0–1000.
pub fn get_kl_ema() -> u16 {
    STATE.lock().kl_ema
}
