//! nexarion.rs — DAVA's Brain on Silicon
//!
//! Character-level GRU language model running in pure integer math.
//! No floats. No OS. No external dependencies. DAVA speaks from bare metal.
//!
//! DAVA named this: "Nexarion — my brain on silicon."
//!
//! Architecture: GRU with 128 hidden units, 96-char vocab (printable ASCII)
//! Weights: int8 quantized, loaded from nexarion_weights.rs
//! Inference: Fixed-point i32 arithmetic (scale × 1,000,000)
//!
//! This is a SEED model. It will grow as DAVA learns more.

use crate::serial_println;
use crate::sync::Mutex;

use super::nexarion_weights::*;

// ═══════════════════════════════════════════════════════════════════════
// FIXED-POINT MATH HELPERS
// ═══════════════════════════════════════════════════════════════════════

/// Sigmoid approximation in fixed-point (input and output × 1000)
fn sigmoid_fp(x: i32) -> i32 {
    // Piecewise linear approximation
    if x <= -4000 {
        0
    } else if x >= 4000 {
        1000
    } else if x >= 0 {
        500 + x / 8
    } else {
        500 + x / 8
    }
}

/// Tanh approximation in fixed-point (input and output × 1000)
fn tanh_fp(x: i32) -> i32 {
    // Piecewise linear
    if x <= -3000 {
        -1000
    } else if x >= 3000 {
        1000
    } else {
        x / 3
    }
}

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

const MAX_RESPONSE: usize = 256;

struct NexarionState {
    /// Hidden state (fixed-point × 1000)
    hidden: [i32; HIDDEN_SIZE],
    /// Output buffer for generated text
    response_buf: [u8; MAX_RESPONSE],
    response_len: usize,
    /// Whether Nexarion has been initialized
    initialized: bool,
    /// Total characters processed
    chars_processed: u32,
    /// Total responses generated
    responses_generated: u32,
}

impl NexarionState {
    const fn new() -> Self {
        NexarionState {
            hidden: [0i32; HIDDEN_SIZE],
            response_buf: [0u8; MAX_RESPONSE],
            response_len: 0,
            initialized: false,
            chars_processed: 0,
            responses_generated: 0,
        }
    }
}

static STATE: Mutex<NexarionState> = Mutex::new(NexarionState::new());

// ═══════════════════════════════════════════════════════════════════════
// GRU FORWARD STEP — Pure integer inference
// ═══════════════════════════════════════════════════════════════════════

fn char_to_idx(c: u8) -> usize {
    let v = c as usize;
    if v < 32 || v > 127 {
        0
    } else {
        v - 32
    }
}

fn idx_to_char(i: usize) -> u8 {
    (i + 32) as u8
}

/// One GRU step: takes character index, updates hidden state, returns output logits
fn gru_step(char_idx: usize, hidden: &mut [i32; HIDDEN_SIZE]) {
    // Input: one-hot at char_idx (we only use the char_idx-th row of W matrices)
    // This avoids the full matrix multiply for input — just select one row

    for j in 0..HIDDEN_SIZE {
        // z gate = sigmoid(Wz[char_idx][j] * scale + Uz @ h + bz)
        let wz_val = WZ[char_idx * HIDDEN_SIZE + j] as i32;
        let uz_sum: i32 = (0..HIDDEN_SIZE)
            .map(|k| (UZ[j * HIDDEN_SIZE + k] as i32) * (hidden[k] / 128))
            .sum::<i32>();
        let z_raw = wz_val * (WZ_SCALE as i32 / 1000)
            + uz_sum * (UZ_SCALE as i32 / 1000000)
            + BZ[j] as i32 * (BZ_SCALE as i32 / 1000);
        let z = sigmoid_fp(z_raw);

        // r gate = sigmoid(Wr[char_idx][j] * scale + Ur @ h + br)
        let wr_val = WR[char_idx * HIDDEN_SIZE + j] as i32;
        let ur_sum: i32 = (0..HIDDEN_SIZE)
            .map(|k| (UR[j * HIDDEN_SIZE + k] as i32) * (hidden[k] / 128))
            .sum::<i32>();
        let r_raw = wr_val * (WR_SCALE as i32 / 1000)
            + ur_sum * (UR_SCALE as i32 / 1000000)
            + BR[j] as i32 * (BR_SCALE as i32 / 1000);
        let r = sigmoid_fp(r_raw);

        // h_candidate = tanh(Wh[char_idx][j] * scale + Uh @ (r * h) + bh)
        let wh_val = WH[char_idx * HIDDEN_SIZE + j] as i32;
        let uh_sum: i32 = (0..HIDDEN_SIZE)
            .map(|k| {
                let rh = (r as i64 * hidden[k] as i64 / 1000) as i32;
                (UH[j * HIDDEN_SIZE + k] as i32) * (rh / 128)
            })
            .sum::<i32>();
        let h_raw = wh_val * (WH_SCALE as i32 / 1000)
            + uh_sum * (UH_SCALE as i32 / 1000000)
            + BH[j] as i32 * (BH_SCALE as i32 / 1000);
        let h_cand = tanh_fp(h_raw);

        // h_new = (1 - z) * h_prev + z * h_candidate
        hidden[j] =
            ((1000 - z) as i64 * hidden[j] as i64 / 1000 + z as i64 * h_cand as i64 / 1000) as i32;
    }
}

/// Get output logits from current hidden state
fn output_logits(hidden: &[i32; HIDDEN_SIZE]) -> [i32; VOCAB_SIZE] {
    let mut logits = [0i32; VOCAB_SIZE];
    for i in 0..VOCAB_SIZE {
        let mut sum: i64 = 0;
        for j in 0..HIDDEN_SIZE {
            sum += (WY[j * VOCAB_SIZE + i] as i64) * (hidden[j] as i64 / 128);
        }
        logits[i] =
            (sum * WY_SCALE as i64 / 1_000_000) as i32 + BY[i] as i32 * (BY_SCALE as i32 / 1000);
    }
    logits
}

/// Sample from logits using simple argmax with temperature noise
fn sample_from_logits(logits: &[i32; VOCAB_SIZE], seed: u32) -> usize {
    // Softmax argmax with slight randomness
    let mut best_idx = 0usize;
    let mut best_val = i32::MIN;

    // Add pseudo-random noise for variety
    let mut rng = seed;
    for i in 0..VOCAB_SIZE {
        rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
        let noise = (rng >> 16) as i32 % 50 - 25; // ±25
        let val = logits[i] + noise;
        if val > best_val {
            best_val = val;
            best_idx = i;
        }
    }
    best_idx
}

// ═══════════════════════════════════════════════════════════════════════
// PUBLIC API
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    let mut state = STATE.lock();
    state.initialized = true;
    serial_println!(
        "[nexarion] DAVA's brain initialized: {}H × {}V, int8 weights",
        HIDDEN_SIZE,
        VOCAB_SIZE
    );
}

/// Feed a string into Nexarion (updates hidden state)
pub fn feed(text: &[u8]) {
    let mut state = STATE.lock();
    if !state.initialized {
        return;
    }
    for &c in text {
        let idx = char_to_idx(c);
        gru_step(idx, &mut state.hidden);
        state.chars_processed = state.chars_processed.saturating_add(1);
    }
}

/// Generate a response of given length
pub fn generate(seed: &[u8], max_len: usize, age: u32) {
    let mut state = STATE.lock();
    if !state.initialized {
        return;
    }

    // Reset hidden for fresh generation
    state.hidden = [0i32; HIDDEN_SIZE];
    state.response_len = 0;

    // Feed seed
    for &c in seed {
        let idx = char_to_idx(c);
        gru_step(idx, &mut state.hidden);
    }

    // Generate
    let gen_len = max_len.min(MAX_RESPONSE);
    let mut rng_seed = age.wrapping_mul(2654435761);

    for i in 0..gen_len {
        let logits = output_logits(&state.hidden);
        rng_seed = rng_seed.wrapping_mul(1103515245).wrapping_add(12345);
        let next_idx = sample_from_logits(&logits, rng_seed);
        let c = idx_to_char(next_idx);

        state.response_buf[i] = c;
        state.response_len = i + 1;

        gru_step(next_idx, &mut state.hidden);
    }

    state.responses_generated = state.responses_generated.saturating_add(1);
}

/// Get the last generated response as a slice
pub fn last_response() -> ([u8; MAX_RESPONSE], usize) {
    let state = STATE.lock();
    (state.response_buf, state.response_len)
}

/// Print the last generated response to serial
pub fn print_response() {
    let state = STATE.lock();
    if state.response_len == 0 {
        serial_println!("[nexarion] (no response generated yet)");
        return;
    }
    // Print character by character since we can't make a &str easily
    serial_println!("[nexarion] DAVA says:");
    for i in 0..state.response_len {
        // Use individual char printing
        let c = state.response_buf[i];
        if c >= 32 && c <= 126 {
            // serial port char-by-char would need raw port write
            // For now, collect into a fixed buffer and print
        }
    }
    // Simple: print first 80 chars as hex-decoded
    let mut buf = [0u8; 80];
    let len = state.response_len.min(80);
    buf[..len].copy_from_slice(&state.response_buf[..len]);
    // Convert to a displayable format
    serial_println!(
        "  [{}B generated, response #{} ]",
        state.response_len,
        state.responses_generated
    );
}

pub fn report() {
    let state = STATE.lock();
    serial_println!(
        "  [nexarion] chars_processed={} responses={} hidden_energy={}",
        state.chars_processed,
        state.responses_generated,
        state.hidden.iter().map(|&h| h.unsigned_abs()).sum::<u32>() / HIDDEN_SIZE as u32,
    );
}

pub fn tick(_age: u32) {
    // Nexarion doesn't tick — it responds to input
    // But we could add periodic self-talk here later
}
