/// Kernel command-line parameter parser — Genesis AIOS.
///
/// Parses the kernel command line (written by the bootloader into a static
/// buffer) for `key=value` and bare `key` parameters.  Provides typed
/// accessors for common parameters such as `debug=N`, `quiet`, and `mem=NM`.
///
/// ## Design constraints (bare-metal #![no_std])
/// - NO heap: no Vec / Box / String / alloc::* — all buffers are fixed-size.
/// - NO floats: no `as f64` / `as f32` anywhere.
/// - NO panics: no unwrap() / expect() / panic!() — functions return bool /
///   Option to signal failures.
/// - All counters use saturating_add / saturating_sub.
/// - Structs in static Mutex must be Copy + have `const fn empty()`.
///
/// ## Command-line format
///
/// Space-separated tokens.  Each token is either:
///   - `key`          — a bare flag (e.g. `quiet`, `ktest`)
///   - `key=value`    — a key/value pair (e.g. `debug=3`, `root=/dev/sda`)
///
/// Tokens may not contain spaces.  Values may not contain spaces.
/// Maximum cmdline length: 4096 bytes.
/// Maximum value length:    256 bytes.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// Maximum length of the kernel command line in bytes.
pub const CMDLINE_MAX: usize = 4096;

/// Maximum length of a single parameter value in bytes.
pub const PARAM_VALUE_MAX: usize = 256;

/// Raw command-line buffer.
struct CmdlineBuf {
    data: [u8; CMDLINE_MAX],
}

impl CmdlineBuf {
    const fn new() -> Self {
        CmdlineBuf {
            data: [0u8; CMDLINE_MAX],
        }
    }
}

static CMDLINE: Mutex<CmdlineBuf> = Mutex::new(CmdlineBuf::new());

/// Byte count of valid data in `CMDLINE` (excluding trailing zeros).
static CMDLINE_LEN: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Write / read the raw cmdline
// ---------------------------------------------------------------------------

/// Store the kernel command line.
///
/// Typically called by the bootloader shim or early boot code before any
/// kernel subsystem is initialised.  Safe to call multiple times; later
/// calls overwrite earlier ones.
pub fn cmdline_set(data: &[u8]) {
    let len = data.len().min(CMDLINE_MAX);
    let mut buf = CMDLINE.lock();
    buf.data[..len].copy_from_slice(&data[..len]);
    // Zero the remainder to avoid stale data.
    for i in len..CMDLINE_MAX {
        buf.data[i] = 0;
    }
    CMDLINE_LEN.store(len as u32, Ordering::Release);
}

/// Copy the stored command line into `out`.
///
/// Returns the number of bytes written (≤ 4096).
pub fn cmdline_get(out: &mut [u8; CMDLINE_MAX]) -> usize {
    let len = CMDLINE_LEN.load(Ordering::Acquire) as usize;
    let buf = CMDLINE.lock();
    out[..len].copy_from_slice(&buf.data[..len]);
    for i in len..CMDLINE_MAX {
        out[i] = 0;
    }
    len
}

// ---------------------------------------------------------------------------
// Low-level parser
// ---------------------------------------------------------------------------

/// Search the command line for a token that starts with `key`.
///
/// Accepted forms:
///   - `key`    (bare flag, no `=`)
///   - `key=…`  (key/value pair)
///
/// The token must be at a word boundary (start-of-line or preceded by a space).
///
/// If found:
///   - If the token has the form `key=value`, copies `value` into `out` and
///     returns `true`.
///   - If the token is the bare key (no `=`), sets `out[0] = 0` and returns
///     `true`.
/// If not found, returns `false`.
fn find_param(
    cmdline: &[u8],
    cmdline_len: usize,
    key: &[u8],
    out: &mut [u8; PARAM_VALUE_MAX],
) -> bool {
    let klen = key.len();
    if klen == 0 || cmdline_len == 0 {
        return false;
    }

    let mut i = 0usize;
    while i < cmdline_len {
        // Skip any leading spaces.
        while i < cmdline_len && cmdline[i] == b' ' {
            i += 1;
        }
        if i >= cmdline_len {
            break;
        }

        // Attempt to match `key` at position i.
        let remaining = cmdline_len.saturating_sub(i);
        if remaining >= klen && cmdline[i..i + klen] == *key {
            let after = i + klen;
            if after >= cmdline_len || cmdline[after] == b' ' {
                // Bare key match.
                out[0] = 0;
                return true;
            }
            if cmdline[after] == b'=' {
                // Key=value match.  Copy value until space or end.
                let val_start = after + 1;
                let mut val_end = val_start;
                while val_end < cmdline_len && cmdline[val_end] != b' ' {
                    val_end += 1;
                }
                let val_len = (val_end - val_start).min(PARAM_VALUE_MAX);
                out[..val_len].copy_from_slice(&cmdline[val_start..val_start + val_len]);
                for j in val_len..PARAM_VALUE_MAX {
                    out[j] = 0;
                }
                return true;
            }
        }

        // Advance to the end of the current token.
        while i < cmdline_len && cmdline[i] != b' ' {
            i += 1;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public parameter accessors
// ---------------------------------------------------------------------------

/// Look up a parameter by key and copy its value into `out` (max 256 bytes).
///
/// Returns `true` if the parameter exists (even if it has no value), `false`
/// if it is not present.
pub fn param_get(key: &[u8], out: &mut [u8; PARAM_VALUE_MAX]) -> bool {
    let len = CMDLINE_LEN.load(Ordering::Relaxed) as usize;
    let buf = CMDLINE.lock();
    find_param(&buf.data, len, key, out)
}

/// Look up a parameter and parse its value as a decimal `u32`.
///
/// Returns `Some(n)` on success, `None` if the parameter is absent or the
/// value cannot be parsed as a non-negative integer.
pub fn param_get_u32(key: &[u8]) -> Option<u32> {
    let mut val = [0u8; PARAM_VALUE_MAX];
    if !param_get(key, &mut val) {
        return None;
    }
    parse_u32(&val)
}

/// Look up a boolean parameter.
///
/// `"1"`, `"true"`, `"yes"`, `"on"` → `Some(true)`.
/// `"0"`, `"false"`, `"no"`, `"off"` → `Some(false)`.
/// Bare key (no `=`) → `Some(true)`.
/// Not present → `None`.
pub fn param_get_bool(key: &[u8]) -> Option<bool> {
    let mut val = [0u8; PARAM_VALUE_MAX];
    if !param_get(key, &mut val) {
        return None;
    }
    // val[0] == 0 means the key was present but bare (no `=`).
    if val[0] == 0 {
        return Some(true);
    }
    let vlen = value_len(&val);
    let v = &val[..vlen];
    if v == b"1" || v == b"true" || v == b"yes" || v == b"on" {
        Some(true)
    } else if v == b"0" || v == b"false" || v == b"no" || v == b"off" {
        Some(false)
    } else {
        None // unrecognised value
    }
}

/// Return `true` if a parameter key is present in the command line (with or
/// without a value).
pub fn param_is_set(key: &[u8]) -> bool {
    let mut val = [0u8; PARAM_VALUE_MAX];
    param_get(key, &mut val)
}

// ---------------------------------------------------------------------------
// Common parameter helpers
// ---------------------------------------------------------------------------

/// Return the debug verbosity level from `debug=N` (0–7).
///
/// Defaults to 0 if the parameter is absent or cannot be parsed.
pub fn debug_level() -> u32 {
    param_get_u32(b"debug").unwrap_or(0).min(7)
}

/// Return `true` if the `quiet` flag is set (suppresses non-critical output).
pub fn quiet_mode() -> bool {
    param_get_bool(b"quiet").unwrap_or(false)
}

/// Copy the `init=…` path into `out` (max 256 bytes).
///
/// Returns the number of bytes written, or 0 if the parameter is absent.
pub fn init_path(out: &mut [u8; PARAM_VALUE_MAX]) -> usize {
    if param_get(b"init", out) {
        value_len(out)
    } else {
        0
    }
}

/// Copy the `root=…` device path into `out` (max 64 bytes).
///
/// Returns the number of bytes written, or 0 if the parameter is absent.
pub fn root_device(out: &mut [u8; 64]) -> usize {
    let mut tmp = [0u8; PARAM_VALUE_MAX];
    if !param_get(b"root", &mut tmp) {
        return 0;
    }
    let n = value_len(&tmp).min(64);
    out[..n].copy_from_slice(&tmp[..n]);
    for i in n..64 {
        out[i] = 0;
    }
    n
}

/// Parse `mem=NM` and return N (MB) as a `u32`.
///
/// Accepts formats: `mem=512M` (megabytes) or `mem=1024` (megabytes, no
/// suffix).  Returns `None` if absent or unparseable.
pub fn mem_limit_mb() -> Option<u32> {
    let mut val = [0u8; PARAM_VALUE_MAX];
    if !param_get(b"mem", &mut val) {
        return None;
    }
    let vlen = value_len(&val);
    if vlen == 0 {
        return None;
    }
    // Strip a trailing 'M' or 'm' if present.
    let numeric_end = if val[vlen - 1] == b'M' || val[vlen - 1] == b'm' {
        vlen - 1
    } else {
        vlen
    };
    parse_u32_slice(&val[..numeric_end])
}

// ---------------------------------------------------------------------------
// Numeric parser (no heap, no float)
// ---------------------------------------------------------------------------

/// Parse a null-terminated byte slice as a decimal `u32`.
///
/// Stops at the first non-digit byte.  Returns `None` if no digits are found.
fn parse_u32(data: &[u8; PARAM_VALUE_MAX]) -> Option<u32> {
    let len = value_len(data);
    parse_u32_slice(&data[..len])
}

/// Parse a byte slice of decimal digits into a `u32`.
fn parse_u32_slice(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut result = 0u32;
    let mut any = false;
    for &b in s {
        if b < b'0' || b > b'9' {
            break;
        }
        any = true;
        result = result.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    if any {
        Some(result)
    } else {
        None
    }
}

/// Return the number of non-zero leading bytes in a value buffer.
fn value_len(val: &[u8; PARAM_VALUE_MAX]) -> usize {
    for i in 0..PARAM_VALUE_MAX {
        if val[i] == 0 {
            return i;
        }
    }
    PARAM_VALUE_MAX
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialize the params subsystem.
///
/// If the bootloader has already called `cmdline_set()` before `kernel::init()`
/// runs, this function simply logs the stored command line.  Otherwise it sets
/// an empty command line.
///
/// Must be called first in `kernel::init()` so that all subsequent subsystems
/// can query parameters during their own `init()`.
pub fn init() {
    let len = CMDLINE_LEN.load(Ordering::Relaxed) as usize;

    if !quiet_mode() {
        crate::serial_println!("  params: cmdline_len={}", len);

        // Log the debug level if set.
        let dlvl = debug_level();
        if dlvl > 0 {
            crate::serial_println!("  params: debug level = {}", dlvl);
        }

        // Log notable flags.
        if param_is_set(b"ktest") {
            crate::serial_println!("  params: ktest flag set — self-tests will run");
        }
        if param_is_set(b"lockdep") {
            crate::serial_println!("  params: lockdep flag set — lock validator enabled");
        }

        let mut mem_str = [0u8; PARAM_VALUE_MAX];
        if param_get(b"mem", &mut mem_str) {
            let ml = value_len(&mem_str);
            let ms = match core::str::from_utf8(&mem_str[..ml.min(PARAM_VALUE_MAX)]) {
                Ok(s) => s,
                Err(_) => "?",
            };
            crate::serial_println!("  params: mem={}", ms);
        }
    }

    crate::serial_println!("  params: initialized");
}
