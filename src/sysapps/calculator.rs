use crate::sync::Mutex;
/// Calculator application for Genesis OS
///
/// Full calculator with basic, scientific, and programmer modes.
/// All arithmetic uses Q16 fixed-point (i32) — no floating point.
/// Supports history, chained operations, parentheses evaluation,
/// and trigonometric / logarithmic functions via integer lookup tables.
///
/// Inspired by: GNOME Calculator, Windows Calculator, HP-48. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point constants and helpers
// ---------------------------------------------------------------------------

/// 1.0 in Q16
const Q16_ONE: i32 = 65536;
/// 2.0 in Q16
const Q16_TWO: i32 = 131072;
/// 0.5 in Q16
const Q16_HALF: i32 = 32768;
/// Pi in Q16 (3.14159... * 65536 = 205887)
const Q16_PI: i32 = 205887;
/// Pi/2 in Q16
const Q16_PI_HALF: i32 = 102944;
/// e in Q16 (2.71828... * 65536 = 178145)
const Q16_E: i32 = 178145;
/// ln(2) in Q16 (0.69315 * 65536 = 45426)
const Q16_LN2: i32 = 45426;
/// ln(10) in Q16 (2.30259 * 65536 = 150902)
const Q16_LN10: i32 = 150902;
/// 10.0 in Q16
const Q16_TEN: i32 = 655360;
/// Maximum representable Q16 value
const Q16_MAX: i32 = i32::MAX;

/// Q16 multiplication: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 division: (a << 16) / b
fn q16_div(a: i32, b: i32) -> Option<i32> {
    if b == 0 {
        return None;
    }
    Some((((a as i64) << 16) / (b as i64)) as i32)
}

/// Q16 integer square root via Newton's method
fn q16_sqrt(x: i32) -> Option<i32> {
    if x < 0 {
        return None;
    }
    if x == 0 {
        return Some(0);
    }
    // Initial guess: x/2 clamped
    let mut guess = if x > Q16_ONE { x >> 1 } else { Q16_ONE };
    for _ in 0..20 {
        let div = match q16_div(x, guess) {
            Some(d) => d,
            None => return None,
        };
        guess = (guess + div) >> 1;
    }
    Some(guess)
}

/// Q16 absolute value
fn q16_abs(x: i32) -> i32 {
    if x < 0 {
        -x
    } else {
        x
    }
}

/// Q16 sine approximation using Taylor series (5 terms)
/// Input in Q16 radians, output in Q16 (-1.0 to 1.0)
fn q16_sin(mut x: i32) -> i32 {
    // Normalize to [-pi, pi]
    while x > Q16_PI {
        x -= Q16_PI * 2;
    }
    while x < -Q16_PI {
        x += Q16_PI * 2;
    }
    // Taylor: sin(x) = x - x^3/3! + x^5/5! - x^7/7!
    let x2 = q16_mul(x, x);
    let x3 = q16_mul(x2, x);
    let x5 = q16_mul(x3, x2);
    let x7 = q16_mul(x5, x2);

    // 1/6 in Q16 = 10923
    let term3 = q16_mul(x3, 10923);
    // 1/120 in Q16 = 546
    let term5 = q16_mul(x5, 546);
    // 1/5040 in Q16 = 13
    let term7 = q16_mul(x7, 13);

    x - term3 + term5 - term7
}

/// Q16 cosine: cos(x) = sin(x + pi/2)
fn q16_cos(x: i32) -> i32 {
    q16_sin(x + Q16_PI_HALF)
}

/// Q16 tangent: tan(x) = sin(x) / cos(x)
fn q16_tan(x: i32) -> Option<i32> {
    let c = q16_cos(x);
    if q16_abs(c) < 16 {
        return None; // Too close to zero
    }
    q16_div(q16_sin(x), c)
}

/// Q16 natural log approximation using series expansion
/// Valid for x > 0
fn q16_ln(x: i32) -> Option<i32> {
    if x <= 0 {
        return None;
    }
    // Normalize: find k such that x = 2^k * m, where 0.5 <= m < 1.0
    let mut k: i32 = 0;
    let mut m = x;
    while m >= Q16_ONE {
        m >>= 1;
        k += 1;
    }
    while m < Q16_HALF {
        m <<= 1;
        k -= 1;
    }
    // ln(x) = k * ln(2) + ln(m)
    // For m near 1, use: ln(m) ~ (m-1) - (m-1)^2/2 + (m-1)^3/3
    let d = m - Q16_ONE;
    let d2 = q16_mul(d, d);
    let d3 = q16_mul(d2, d);
    let ln_m = d - (d2 >> 1) + q16_div(d3, Q16_ONE * 3).unwrap_or(0);
    Some(q16_mul(k * Q16_ONE, Q16_LN2) + ln_m)
}

/// Q16 log base 10: log10(x) = ln(x) / ln(10)
fn q16_log10(x: i32) -> Option<i32> {
    let ln_x = q16_ln(x)?;
    q16_div(ln_x, Q16_LN10)
}

/// Q16 power: a^b (integer exponent only for safety)
fn q16_pow(base: i32, exp: i32) -> i32 {
    let n = exp >> 16; // integer part of exponent
    if n == 0 {
        return Q16_ONE;
    }
    let negative = n < 0;
    let abs_n = if negative { -n } else { n };
    let mut result = Q16_ONE;
    for _ in 0..abs_n {
        result = q16_mul(result, base);
    }
    if negative {
        match q16_div(Q16_ONE, result) {
            Some(r) => r,
            None => Q16_MAX,
        }
    } else {
        result
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Calculator operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Power,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Log,
    Ln,
}

/// Calculator mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcMode {
    Basic,
    Scientific,
    Programmer,
}

/// Programmer display base
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DisplayBase {
    Binary,
    Octal,
    Decimal,
    Hex,
}

/// Result of a calculation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CalcResult {
    Value(i32),
    DivideByZero,
    Overflow,
    InvalidInput,
    DomainError,
}

/// A history entry recording a computation
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub operand_a: i32,
    pub op: CalcOp,
    pub operand_b: i32,
    pub result: i32,
    pub timestamp: u64,
}

/// Persistent calculator state
struct CalculatorState {
    mode: CalcMode,
    display_base: DisplayBase,
    accumulator: i32,
    pending_op: Option<CalcOp>,
    pending_operand: i32,
    input_buffer: Vec<u8>,
    history: Vec<HistoryEntry>,
    max_history: usize,
    memory: i32,
    has_memory: bool,
    angle_radians: bool,
    last_result: i32,
    input_started: bool,
    timestamp_counter: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CALCULATOR: Mutex<Option<CalculatorState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_state() -> CalculatorState {
    CalculatorState {
        mode: CalcMode::Basic,
        display_base: DisplayBase::Decimal,
        accumulator: 0,
        pending_op: None,
        pending_operand: 0,
        input_buffer: Vec::new(),
        history: Vec::new(),
        max_history: 100,
        memory: 0,
        has_memory: false,
        angle_radians: true,
        last_result: 0,
        input_started: false,
        timestamp_counter: 0,
    }
}

/// Parse the input buffer into a Q16 value
fn parse_input(buffer: &[u8], base: DisplayBase) -> CalcResult {
    if buffer.is_empty() {
        return CalcResult::Value(0);
    }
    let radix: i32 = match base {
        DisplayBase::Binary => 2,
        DisplayBase::Octal => 8,
        DisplayBase::Decimal => 10,
        DisplayBase::Hex => 16,
    };
    let mut negative = false;
    let mut start = 0;
    if !buffer.is_empty() && buffer[0] == b'-' {
        negative = true;
        start = 1;
    }
    let mut int_part: i64 = 0;
    let mut frac_part: i64 = 0;
    let mut frac_digits: u32 = 0;
    let mut in_frac = false;

    for &ch in &buffer[start..] {
        if ch == b'.' {
            if in_frac {
                return CalcResult::InvalidInput;
            }
            in_frac = true;
            continue;
        }
        let digit = match ch {
            b'0'..=b'9' => (ch - b'0') as i64,
            b'A'..=b'F' => (ch - b'A' + 10) as i64,
            b'a'..=b'f' => (ch - b'a' + 10) as i64,
            _ => return CalcResult::InvalidInput,
        };
        if digit >= radix as i64 {
            return CalcResult::InvalidInput;
        }
        if in_frac {
            frac_part = frac_part * radix as i64 + digit;
            frac_digits += 1;
        } else {
            int_part = int_part * radix as i64 + digit;
        }
    }

    // Convert to Q16
    let mut q16_val = (int_part << 16) as i32;
    if frac_digits > 0 {
        let mut divisor: i64 = 1;
        for _ in 0..frac_digits {
            divisor *= radix as i64;
        }
        let frac_q16 = ((frac_part << 16) / divisor) as i32;
        q16_val += frac_q16;
    }
    if negative {
        q16_val = -q16_val;
    }
    CalcResult::Value(q16_val)
}

/// Perform a binary operation
fn apply_op(a: i32, op: CalcOp, b: i32) -> CalcResult {
    match op {
        CalcOp::Add => {
            let r = (a as i64) + (b as i64);
            if r > Q16_MAX as i64 || r < -(Q16_MAX as i64) {
                CalcResult::Overflow
            } else {
                CalcResult::Value(r as i32)
            }
        }
        CalcOp::Sub => {
            let r = (a as i64) - (b as i64);
            if r > Q16_MAX as i64 || r < -(Q16_MAX as i64) {
                CalcResult::Overflow
            } else {
                CalcResult::Value(r as i32)
            }
        }
        CalcOp::Mul => CalcResult::Value(q16_mul(a, b)),
        CalcOp::Div => match q16_div(a, b) {
            Some(r) => CalcResult::Value(r),
            None => CalcResult::DivideByZero,
        },
        CalcOp::Mod => {
            if b == 0 {
                CalcResult::DivideByZero
            } else {
                CalcResult::Value(a % b)
            }
        }
        CalcOp::Power => CalcResult::Value(q16_pow(a, b)),
        CalcOp::Sqrt => match q16_sqrt(a) {
            Some(r) => CalcResult::Value(r),
            None => CalcResult::DomainError,
        },
        CalcOp::Sin => CalcResult::Value(q16_sin(a)),
        CalcOp::Cos => CalcResult::Value(q16_cos(a)),
        CalcOp::Tan => match q16_tan(a) {
            Some(r) => CalcResult::Value(r),
            None => CalcResult::DomainError,
        },
        CalcOp::Log => match q16_log10(a) {
            Some(r) => CalcResult::Value(r),
            None => CalcResult::DomainError,
        },
        CalcOp::Ln => match q16_ln(a) {
            Some(r) => CalcResult::Value(r),
            None => CalcResult::DomainError,
        },
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Evaluate the current expression and return the result
pub fn evaluate() -> CalcResult {
    let mut guard = CALCULATOR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return CalcResult::InvalidInput,
    };

    let current = match parse_input(&state.input_buffer, state.display_base) {
        CalcResult::Value(v) => v,
        err => return err,
    };

    let result = if let Some(op) = state.pending_op {
        let r = apply_op(state.pending_operand, op, current);
        // Record history
        if let CalcResult::Value(val) = r {
            state.timestamp_counter += 1;
            state.history.push(HistoryEntry {
                operand_a: state.pending_operand,
                op,
                operand_b: current,
                result: val,
                timestamp: state.timestamp_counter,
            });
            if state.history.len() > state.max_history {
                state.history.remove(0);
            }
        }
        r
    } else {
        CalcResult::Value(current)
    };

    if let CalcResult::Value(val) = result {
        state.accumulator = val;
        state.last_result = val;
        state.pending_op = None;
        state.pending_operand = 0;
        state.input_buffer.clear();
        state.input_started = false;
    }
    result
}

/// Push a digit (0-9 or A-F for hex) into the input buffer
pub fn push_digit(digit: u8) {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        let valid = match state.display_base {
            DisplayBase::Binary => digit <= 1,
            DisplayBase::Octal => digit <= 7,
            DisplayBase::Decimal => digit <= 9,
            DisplayBase::Hex => digit <= 15,
        };
        if valid {
            let ch = if digit < 10 {
                b'0' + digit
            } else {
                b'A' + (digit - 10)
            };
            state.input_buffer.push(ch);
            state.input_started = true;
        }
    }
}

/// Push a decimal point
pub fn push_decimal() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        // Only allow one decimal point
        if !state.input_buffer.contains(&b'.') {
            if state.input_buffer.is_empty() {
                state.input_buffer.push(b'0');
            }
            state.input_buffer.push(b'.');
            state.input_started = true;
        }
    }
}

/// Push a negative sign
pub fn push_negate() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        if !state.input_buffer.is_empty() && state.input_buffer[0] == b'-' {
            state.input_buffer.remove(0);
        } else {
            state.input_buffer.insert(0, b'-');
        }
    }
}

/// Push an operator, evaluating pending expression first
pub fn push_op(op: CalcOp) -> CalcResult {
    // For unary ops, evaluate immediately
    match op {
        CalcOp::Sqrt | CalcOp::Sin | CalcOp::Cos | CalcOp::Tan | CalcOp::Log | CalcOp::Ln => {
            let mut guard = CALCULATOR.lock();
            let state = match guard.as_mut() {
                Some(s) => s,
                None => return CalcResult::InvalidInput,
            };
            let operand = match parse_input(&state.input_buffer, state.display_base) {
                CalcResult::Value(v) => v,
                err => return err,
            };
            let result = apply_op(operand, op, 0);
            if let CalcResult::Value(val) = result {
                state.accumulator = val;
                state.last_result = val;
                state.input_buffer.clear();
                state.input_started = false;
                state.timestamp_counter += 1;
                state.history.push(HistoryEntry {
                    operand_a: operand,
                    op,
                    operand_b: 0,
                    result: val,
                    timestamp: state.timestamp_counter,
                });
                if state.history.len() > state.max_history {
                    state.history.remove(0);
                }
            }
            return result;
        }
        _ => {}
    }

    // For binary ops: if there's a pending op, evaluate it first
    let eval_result = {
        let guard = CALCULATOR.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return CalcResult::InvalidInput,
        };
        state.pending_op.is_some() && state.input_started
    };

    if eval_result {
        let _ = evaluate();
    }

    let mut guard = CALCULATOR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return CalcResult::InvalidInput,
    };

    let current = match parse_input(&state.input_buffer, state.display_base) {
        CalcResult::Value(v) => v,
        err => return err,
    };

    state.pending_operand = if state.input_started {
        current
    } else {
        state.accumulator
    };
    state.pending_op = Some(op);
    state.input_buffer.clear();
    state.input_started = false;
    CalcResult::Value(state.pending_operand)
}

/// Clear all state (AC)
pub fn clear() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.accumulator = 0;
        state.pending_op = None;
        state.pending_operand = 0;
        state.input_buffer.clear();
        state.input_started = false;
    }
}

/// Clear only the current input (C / CE)
pub fn clear_entry() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.input_buffer.clear();
        state.input_started = false;
    }
}

/// Delete the last character from the input buffer
pub fn backspace() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.input_buffer.pop();
        if state.input_buffer.is_empty() {
            state.input_started = false;
        }
    }
}

/// Get the computation history
pub fn get_history() -> Vec<HistoryEntry> {
    let guard = CALCULATOR.lock();
    match guard.as_ref() {
        Some(state) => state.history.clone(),
        None => Vec::new(),
    }
}

/// Clear the history
pub fn clear_history() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.history.clear();
    }
}

/// Toggle between Basic, Scientific, and Programmer modes
pub fn toggle_mode() -> CalcMode {
    let mut guard = CALCULATOR.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return CalcMode::Basic,
    };
    state.mode = match state.mode {
        CalcMode::Basic => CalcMode::Scientific,
        CalcMode::Scientific => CalcMode::Programmer,
        CalcMode::Programmer => CalcMode::Basic,
    };
    state.mode
}

/// Set a specific mode
pub fn set_mode(mode: CalcMode) {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.mode = mode;
    }
}

/// Get current mode
pub fn get_mode() -> CalcMode {
    let guard = CALCULATOR.lock();
    match guard.as_ref() {
        Some(state) => state.mode,
        None => CalcMode::Basic,
    }
}

/// Set the programmer display base
pub fn set_base(base: DisplayBase) {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.display_base = base;
    }
}

/// Store current value to memory (MS)
pub fn memory_store() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.memory = state.accumulator;
        state.has_memory = true;
    }
}

/// Recall memory value (MR)
pub fn memory_recall() -> i32 {
    let guard = CALCULATOR.lock();
    match guard.as_ref() {
        Some(state) if state.has_memory => state.memory,
        _ => 0,
    }
}

/// Add to memory (M+)
pub fn memory_add() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.memory += state.accumulator;
        state.has_memory = true;
    }
}

/// Subtract from memory (M-)
pub fn memory_subtract() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.memory -= state.accumulator;
        state.has_memory = true;
    }
}

/// Clear memory (MC)
pub fn memory_clear() {
    let mut guard = CALCULATOR.lock();
    if let Some(state) = guard.as_mut() {
        state.memory = 0;
        state.has_memory = false;
    }
}

/// Get the current display value as Q16
pub fn get_display_value() -> i32 {
    let guard = CALCULATOR.lock();
    match guard.as_ref() {
        Some(state) => {
            if state.input_started {
                match parse_input(&state.input_buffer, state.display_base) {
                    CalcResult::Value(v) => v,
                    _ => state.accumulator,
                }
            } else {
                state.accumulator
            }
        }
        None => 0,
    }
}

/// Get the last computed result
pub fn get_last_result() -> i32 {
    let guard = CALCULATOR.lock();
    match guard.as_ref() {
        Some(state) => state.last_result,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the calculator subsystem
pub fn init() {
    let mut guard = CALCULATOR.lock();
    *guard = Some(default_state());
    serial_println!("    Calculator ready (Q16 fixed-point)");
}
