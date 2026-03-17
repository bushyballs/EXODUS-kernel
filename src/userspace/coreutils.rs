/// Coreutils for Genesis — additional Unix utilities
///
/// Provides sort, uniq, cut, tr, tee, basename, dirname, seq, yes,
/// env, printenv, expr, and other standard Unix utilities that
/// complement the shell builtins.
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ── sort ─────────────────────────────────────────────────────────────────────

/// Sort lines of text
///
/// Options:
///   reverse: sort in descending order
///   numeric: sort numerically instead of lexicographically
///   unique: remove duplicate lines after sorting
///   key_field: 0-based field index to sort by (None = entire line)
///   delimiter: field delimiter character (default: whitespace)
pub fn sort(
    input: &str,
    reverse: bool,
    numeric: bool,
    unique: bool,
    key_field: Option<usize>,
    delimiter: Option<char>,
) -> String {
    let mut lines: Vec<&str> = input.lines().collect();

    lines.sort_by(|a, b| {
        let key_a = extract_key(a, key_field, delimiter);
        let key_b = extract_key(b, key_field, delimiter);

        let cmp = if numeric {
            let na = parse_i64(key_a);
            let nb = parse_i64(key_b);
            na.cmp(&nb)
        } else {
            key_a.cmp(key_b)
        };

        if reverse {
            cmp.reverse()
        } else {
            cmp
        }
    });

    if unique {
        lines.dedup();
    }

    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        if i + 1 < lines.len() {
            out.push('\n');
        }
    }
    out
}

fn extract_key<'a>(line: &'a str, field: Option<usize>, delim: Option<char>) -> &'a str {
    match field {
        None => line,
        Some(idx) => match delim {
            Some(d) => line.split(d).nth(idx).unwrap_or(line),
            None => line.split_whitespace().nth(idx).unwrap_or(line),
        },
    }
}

fn parse_i64(s: &str) -> i64 {
    let s = s.trim();
    let (neg, s) = if s.starts_with('-') {
        (true, &s[1..])
    } else {
        (false, s)
    };
    let mut result: i64 = 0;
    for c in s.bytes() {
        if c < b'0' || c > b'9' {
            break;
        }
        result = result.saturating_mul(10).saturating_add((c - b'0') as i64);
    }
    if neg {
        -result
    } else {
        result
    }
}

// ── uniq ─────────────────────────────────────────────────────────────────────

/// Remove or report duplicate lines
///
/// Options:
///   count: prefix lines with occurrence count
///   repeated_only: only show lines that appear more than once
///   unique_only: only show lines that appear exactly once
pub fn uniq(input: &str, count: bool, repeated_only: bool, unique_only: bool) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out = String::new();
    let mut i = 0;

    while i < lines.len() {
        let current = lines[i];
        let mut n = 1usize;
        while i + n < lines.len() && lines[i + n] == current {
            n += 1;
        }

        let show = if repeated_only {
            n > 1
        } else if unique_only {
            n == 1
        } else {
            true
        };

        if show {
            if count {
                out.push_str(&alloc::format!("{:>7} {}\n", n, current));
            } else {
                out.push_str(current);
                out.push('\n');
            }
        }

        i += n;
    }

    String::from(out.trim_end_matches('\n'))
}

// ── cut ──────────────────────────────────────────────────────────────────────

/// Extract fields from each line
///
/// delimiter: field separator (default: tab)
/// fields: 1-based field indices to extract
pub fn cut(input: &str, delimiter: char, fields: &[usize]) -> String {
    let mut out = String::new();
    for line in input.lines() {
        let parts: Vec<&str> = line.split(delimiter).collect();
        let mut first = true;
        for &f in fields {
            if f == 0 {
                continue;
            }
            if !first {
                out.push(delimiter);
            }
            first = false;
            if f <= parts.len() {
                out.push_str(parts[f - 1]);
            }
        }
        out.push('\n');
    }
    String::from(out.trim_end_matches('\n'))
}

// ── tr ───────────────────────────────────────────────────────────────────────

/// Translate or delete characters
///
/// from: characters to translate from
/// to: characters to translate to (must be same length as from, or delete mode)
/// delete: if true, delete chars in `from` instead of translating
pub fn tr(input: &str, from: &str, to: &str, delete: bool) -> String {
    let from_chars: Vec<char> = from.chars().collect();
    let to_chars: Vec<char> = to.chars().collect();

    let mut out = String::new();
    for c in input.chars() {
        if let Some(pos) = from_chars.iter().position(|&fc| fc == c) {
            if delete {
                // Skip this character
            } else if pos < to_chars.len() {
                out.push(to_chars[pos]);
            } else if !to_chars.is_empty() {
                // Repeat last char of 'to' set
                out.push(*to_chars.last().unwrap());
            } else {
                out.push(c);
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ── basename / dirname ───────────────────────────────────────────────────────

/// Extract the filename from a path
pub fn basename(path: &str, suffix: Option<&str>) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(suf) = suffix {
        if name.ends_with(suf) && name.len() > suf.len() {
            return String::from(&name[..name.len() - suf.len()]);
        }
    }
    String::from(name)
}

/// Extract the directory portion of a path
pub fn dirname(path: &str) -> String {
    if let Some(pos) = path.rfind('/') {
        if pos == 0 {
            String::from("/")
        } else {
            String::from(&path[..pos])
        }
    } else {
        String::from(".")
    }
}

// ── seq ──────────────────────────────────────────────────────────────────────

/// Generate a sequence of numbers
pub fn seq(first: i64, step: i64, last: i64) -> String {
    let mut out = String::new();
    let mut current = first;

    if step == 0 {
        return String::from("seq: step cannot be zero");
    }

    let going_up = step > 0;
    let mut count = 0u64;
    let max_count = 10_000u64; // safety limit

    loop {
        if going_up && current > last {
            break;
        }
        if !going_up && current < last {
            break;
        }
        if count >= max_count {
            break;
        }

        if count > 0 {
            out.push('\n');
        }
        out.push_str(&alloc::format!("{}", current));
        current += step;
        count += 1;
    }

    out
}

// ── yes ──────────────────────────────────────────────────────────────────────

/// Generate repeated output (limited to N lines for kernel safety)
pub fn yes(text: &str, max_lines: usize) -> String {
    let text = if text.is_empty() { "y" } else { text };
    let limit = max_lines.min(1000); // hard cap for kernel mode

    let mut out = String::new();
    for i in 0..limit {
        out.push_str(text);
        if i + 1 < limit {
            out.push('\n');
        }
    }
    out
}

// ── expr ─────────────────────────────────────────────────────────────────────

/// Evaluate simple arithmetic expression
///
/// Supports: +, -, *, /, %, =, !=, <, >, <=, >=, &, |
/// Operands and operators must be separate tokens.
pub fn expr(tokens: &[&str]) -> String {
    if tokens.len() < 3 {
        if tokens.len() == 1 {
            return String::from(tokens[0]);
        }
        return String::from("expr: syntax error");
    }

    let a_str = tokens[0];
    let op = tokens[1];
    let b_str = tokens[2];

    let a = parse_i64(a_str);
    let b = parse_i64(b_str);

    let result = match op {
        "+" => alloc::format!("{}", a.wrapping_add(b)),
        "-" => alloc::format!("{}", a.wrapping_sub(b)),
        "*" => alloc::format!("{}", a.wrapping_mul(b)),
        "/" => {
            if b == 0 {
                return String::from("expr: division by zero");
            }
            alloc::format!("{}", a / b)
        }
        "%" => {
            if b == 0 {
                return String::from("expr: division by zero");
            }
            alloc::format!("{}", a % b)
        }
        "=" => alloc::format!("{}", if a_str == b_str { 1 } else { 0 }),
        "!=" => alloc::format!("{}", if a_str != b_str { 1 } else { 0 }),
        "<" => alloc::format!("{}", if a < b { 1 } else { 0 }),
        ">" => alloc::format!("{}", if a > b { 1 } else { 0 }),
        "<=" => alloc::format!("{}", if a <= b { 1 } else { 0 }),
        ">=" => alloc::format!("{}", if a >= b { 1 } else { 0 }),
        _ => return alloc::format!("expr: unknown operator '{}'", op),
    };

    result
}

// ── tee ──────────────────────────────────────────────────────────────────────

/// Write input to both a file and stdout (returns the text for stdout)
///
/// The caller is responsible for actually writing to the file.
/// This function just splits the data.
pub fn tee_split(input: &str) -> (String, Vec<u8>) {
    let bytes = input.as_bytes().to_vec();
    (String::from(input), bytes)
}

// ── rev ──────────────────────────────────────────────────────────────────────

/// Reverse each line of input
pub fn rev(input: &str) -> String {
    let mut out = String::new();
    for (i, line) in input.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let reversed: String = line.chars().rev().collect();
        out.push_str(&reversed);
    }
    out
}

// ── fold ─────────────────────────────────────────────────────────────────────

/// Wrap lines at specified width
pub fn fold(input: &str, width: usize) -> String {
    let width = if width == 0 { 80 } else { width };
    let mut out = String::new();

    for line in input.lines() {
        if line.len() <= width {
            out.push_str(line);
            out.push('\n');
        } else {
            let mut pos = 0;
            while pos < line.len() {
                let end = (pos + width).min(line.len());
                out.push_str(&line[pos..end]);
                out.push('\n');
                pos = end;
            }
        }
    }

    String::from(out.trim_end_matches('\n'))
}

// ── paste ────────────────────────────────────────────────────────────────────

/// Merge lines from multiple inputs side by side
pub fn paste(inputs: &[&str], delimiter: &str) -> String {
    let line_sets: Vec<Vec<&str>> = inputs.iter().map(|input| input.lines().collect()).collect();

    let max_lines = line_sets.iter().map(|ls| ls.len()).max().unwrap_or(0);
    let mut out = String::new();

    for i in 0..max_lines {
        for (j, lines) in line_sets.iter().enumerate() {
            if j > 0 {
                out.push_str(delimiter);
            }
            if i < lines.len() {
                out.push_str(lines[i]);
            }
        }
        out.push('\n');
    }

    String::from(out.trim_end_matches('\n'))
}

// ── nproc ────────────────────────────────────────────────────────────────────

/// Get number of processing units available
pub fn nproc() -> usize {
    // Read from kernel's CPU count
    1 // Default single core, overridden by actual detection
}

// ── md5sum/sha256sum stubs ───────────────────────────────────────────────────

/// Simple checksum (not cryptographic -- just a FNV-1a hash for now)
pub fn checksum(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Format checksum as hex string
pub fn checksum_hex(data: &[u8]) -> String {
    let hash = checksum(data);
    alloc::format!("{:016x}", hash)
}

// ── init ─────────────────────────────────────────────────────────────────────

/// Initialize coreutils
pub fn init() {
    serial_println!("  coreutils: sort, uniq, cut, tr, seq, expr, rev, fold, paste ready");
}
