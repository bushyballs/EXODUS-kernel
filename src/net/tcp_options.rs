/// TCP Options — RFC 7323 (Window Scaling, Timestamps) + SACK (RFC 2018)
///
/// This module provides:
///   - Option kind constants
///   - `TcpOptions` parsed representation
///   - `SackBlock` — a SACK range
///   - `parse_tcp_options()`     — decode options bytes from an incoming header
///   - `build_syn_options()`     — emit options for SYN/SYN-ACK (MSS + WSCALE + SACK_PERMITTED + TS)
///   - `build_data_options()`    — emit timestamp option for data/ACK segments
///   - `build_sack_options()`    — emit timestamp + SACK blocks for out-of-order ACKs
///
/// Encoding rules (RFC 793 §3.1):
///   - Options must be padded to a multiple of 4 bytes using NOP (kind=1).
///   - EOL (kind=0) terminates the options list.
///   - All multi-byte fields are big-endian.
///
/// No-std, no heap — all output is written into caller-supplied `&mut [u8]` slices.

// ---------------------------------------------------------------------------
// Option kind codes
// ---------------------------------------------------------------------------

pub const OPT_EOL: u8 = 0; // End of Option List
pub const OPT_NOP: u8 = 1; // No-Operation (padding)
pub const OPT_MSS: u8 = 2; // Maximum Segment Size          (RFC 793)
pub const OPT_WSCALE: u8 = 3; // Window Scale                  (RFC 1323 / 7323)
pub const OPT_SACK_PERMITTED: u8 = 4; // SACK Permitted                (RFC 2018)
pub const OPT_SACK: u8 = 5; // SACK block list               (RFC 2018)
pub const OPT_TS: u8 = 8; // Timestamps                    (RFC 1323 / 7323)

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A single SACK block: left-edge (inclusive) and right-edge (exclusive) seq#s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SackBlock {
    pub left: u32,
    pub right: u32,
}

impl SackBlock {
    #[inline]
    pub fn is_empty(self) -> bool {
        self.left == self.right
    }

    /// Returns true when `seq` falls within [left, right)
    #[inline]
    pub fn contains(self, seq: u32) -> bool {
        seq_geq(seq, self.left) && seq_lt(seq, self.right)
    }

    /// True when this block overlaps or is adjacent to `other`.
    #[inline]
    pub fn overlaps_or_adjacent(self, other: SackBlock) -> bool {
        seq_leq(self.left, other.right) && seq_leq(other.left, self.right)
    }

    /// Merge `other` into `self` (assumes overlap/adjacency).
    #[inline]
    pub fn merge(self, other: SackBlock) -> SackBlock {
        SackBlock {
            left: if seq_lt(self.left, other.left) {
                self.left
            } else {
                other.left
            },
            right: if seq_gt(self.right, other.right) {
                self.right
            } else {
                other.right
            },
        }
    }
}

/// All TCP options decoded from an incoming segment header.
#[derive(Clone, Copy, Debug)]
pub struct TcpOptions {
    /// Negotiated MSS (present in SYN / SYN-ACK).
    pub mss: Option<u16>,

    /// Window scale shift count (0–14), present in SYN / SYN-ACK.
    pub wscale: Option<u8>,

    /// Peer has offered SACK capability (set in SYN / SYN-ACK).
    pub sack_permitted: bool,

    /// SACK blocks carried in this segment (up to 4).
    pub sack_blocks: [SackBlock; 4],

    /// Number of valid entries in `sack_blocks`.
    pub sack_count: u8,

    /// Timestamp value sent by the peer (TSval).
    pub ts_val: Option<u32>,

    /// Timestamp echo-reply from the peer (TSecr).
    pub ts_ecr: Option<u32>,
}

impl TcpOptions {
    /// Return a zeroed / empty options struct.
    pub const fn empty() -> Self {
        TcpOptions {
            mss: None,
            wscale: None,
            sack_permitted: false,
            sack_blocks: [SackBlock { left: 0, right: 0 }; 4],
            sack_count: 0,
            ts_val: None,
            ts_ecr: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Decode TCP options from the option bytes that follow the mandatory 20-byte
/// TCP header.  `opts` should be the slice `header_bytes[20..data_offset]`.
///
/// Unknown options are skipped safely.  Malformed length fields are handled by
/// bounds-checking every access; parsing stops on any inconsistency.
pub fn parse_tcp_options(opts: &[u8]) -> TcpOptions {
    let mut result = TcpOptions::empty();
    let mut i = 0usize;

    while i < opts.len() {
        let kind = opts[i];
        match kind {
            OPT_EOL => break,
            OPT_NOP => {
                i += 1;
                continue;
            }
            _ => {
                // All other options have the form: kind(1) + len(1) + data(len-2)
                if i + 1 >= opts.len() {
                    break;
                }
                let len = opts[i + 1] as usize;
                if len < 2 {
                    break;
                } // malformed: len must be >= 2
                if i + len > opts.len() {
                    break;
                } // truncated

                match kind {
                    OPT_MSS => {
                        if len == 4 {
                            let v = u16::from_be_bytes([opts[i + 2], opts[i + 3]]);
                            result.mss = Some(v);
                        }
                    }
                    OPT_WSCALE => {
                        if len == 3 {
                            let shift = opts[i + 2];
                            // RFC 7323 §2.3: scale > 14 is treated as 14
                            result.wscale = Some(shift.min(14));
                        }
                    }
                    OPT_SACK_PERMITTED => {
                        if len == 2 {
                            result.sack_permitted = true;
                        }
                    }
                    OPT_SACK => {
                        // Each SACK block is 8 bytes (left u32 + right u32).
                        // len = 2 + n*8, so n = (len-2)/8.
                        let n_blocks = (len.saturating_sub(2)) / 8;
                        let n = n_blocks.min(4);
                        for b in 0..n {
                            let off = i + 2 + b * 8;
                            if off + 8 > opts.len() {
                                break;
                            }
                            let left = u32::from_be_bytes([
                                opts[off],
                                opts[off + 1],
                                opts[off + 2],
                                opts[off + 3],
                            ]);
                            let right = u32::from_be_bytes([
                                opts[off + 4],
                                opts[off + 5],
                                opts[off + 6],
                                opts[off + 7],
                            ]);
                            if (result.sack_count as usize) < 4 {
                                result.sack_blocks[result.sack_count as usize] =
                                    SackBlock { left, right };
                                result.sack_count += 1;
                            }
                        }
                    }
                    OPT_TS => {
                        if len == 10 {
                            let tsval = u32::from_be_bytes([
                                opts[i + 2],
                                opts[i + 3],
                                opts[i + 4],
                                opts[i + 5],
                            ]);
                            let tsecr = u32::from_be_bytes([
                                opts[i + 6],
                                opts[i + 7],
                                opts[i + 8],
                                opts[i + 9],
                            ]);
                            result.ts_val = Some(tsval);
                            result.ts_ecr = Some(tsecr);
                        }
                    }
                    _ => {} // ignore unknown
                }

                i += len;
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Write a u16 in big-endian at `buf[pos]`; returns `pos + 2`.
#[inline]
fn write_u16(buf: &mut [u8], pos: usize, val: u16) -> usize {
    if pos + 2 <= buf.len() {
        let b = val.to_be_bytes();
        buf[pos] = b[0];
        buf[pos + 1] = b[1];
    }
    pos + 2
}

/// Write a u32 in big-endian at `buf[pos]`; returns `pos + 4`.
#[inline]
fn write_u32(buf: &mut [u8], pos: usize, val: u32) -> usize {
    if pos + 4 <= buf.len() {
        let b = val.to_be_bytes();
        buf[pos] = b[0];
        buf[pos + 1] = b[1];
        buf[pos + 2] = b[2];
        buf[pos + 3] = b[3];
    }
    pos + 4
}

/// Pad `pos` up to the next multiple of 4 by writing NOP bytes.
/// Returns the new position.
#[inline]
fn pad_to_4(buf: &mut [u8], pos: usize) -> usize {
    let mut p = pos;
    while p % 4 != 0 {
        if p < buf.len() {
            buf[p] = OPT_NOP;
        }
        p += 1;
    }
    p
}

/// Build TCP SYN options into `buf`.
///
/// Emits (in order):
///   MSS (4 bytes) + NOP+WSCALE (4 bytes) + NOP+NOP+SACK_PERMITTED (4 bytes) + TS (12 bytes)
/// = 24 bytes total (multiple of 4, fits in data-offset = 5+6 = 11 words max; 24 bytes uses
///   data-offset = (20+24)/4 = 11 words — fine).
///
/// `our_mss`  — the MSS we advertise (typically 1460 for Ethernet).
/// `ts_val`   — our current timestamp counter.
/// `wscale`   — our window scale shift (0–14).
///
/// Returns the number of bytes written into `buf`.
pub fn build_syn_options(buf: &mut [u8], our_mss: u16, ts_val: u32, wscale: u8) -> usize {
    let mut p = 0usize;

    // --- MSS (kind=2, len=4) ---
    if p + 4 <= buf.len() {
        buf[p] = OPT_MSS;
        buf[p + 1] = 4;
        p = write_u16(buf, p + 2, our_mss);
    }

    // --- NOP + Window Scale (kind=3, len=3) → 4 bytes total ---
    if p + 4 <= buf.len() {
        buf[p] = OPT_NOP;
        buf[p + 1] = OPT_WSCALE;
        buf[p + 2] = 3;
        buf[p + 3] = wscale.min(14);
        p += 4;
    }

    // --- NOP + NOP + SACK Permitted (kind=4, len=2) → 4 bytes total ---
    if p + 4 <= buf.len() {
        buf[p] = OPT_NOP;
        buf[p + 1] = OPT_NOP;
        buf[p + 2] = OPT_SACK_PERMITTED;
        buf[p + 3] = 2;
        p += 4;
    }

    // --- Timestamps (kind=8, len=10) + 2 NOPs → 12 bytes total ---
    if p + 12 <= buf.len() {
        buf[p] = OPT_NOP;
        buf[p + 1] = OPT_NOP;
        buf[p + 2] = OPT_TS;
        buf[p + 3] = 10;
        p += 4;
        p = write_u32(buf, p, ts_val); // TSval
        p = write_u32(buf, p, 0); // TSecr = 0 in SYN (not yet known)
    }

    p
}

/// Build TCP data/ACK options (timestamp only, 12 bytes).
///
/// Emits: NOP + NOP + TS (kind=8, len=10) = 12 bytes.
///
/// `ts_val`  — our current timestamp.
/// `ts_ecr`  — the TSval we received from the peer (echo reply).
///
/// Returns bytes written.
pub fn build_data_options(buf: &mut [u8], ts_val: u32, ts_ecr: u32) -> usize {
    if buf.len() < 12 {
        return 0;
    }
    buf[0] = OPT_NOP;
    buf[1] = OPT_NOP;
    buf[2] = OPT_TS;
    buf[3] = 10;
    let mut p = 4;
    p = write_u32(buf, p, ts_val);
    p = write_u32(buf, p, ts_ecr);
    p
}

/// Build TCP SACK + timestamp options for out-of-order ACKs.
///
/// Layout: NOP+NOP+TS(12) + NOP+NOP+SACK(2+n*8) padded to 4-byte boundary.
/// Maximum output with 4 SACK blocks:
///   12 (TS) + 2 (NOP+NOP) + 2 (SACK kind+len) + 32 (4 blocks) = 48 bytes.
///
/// `blocks`  — slice of SACK blocks to include (up to 4).
/// Returns bytes written.
pub fn build_sack_options(buf: &mut [u8], blocks: &[SackBlock], ts_val: u32, ts_ecr: u32) -> usize {
    let n = blocks.len().min(4);
    if n == 0 {
        // Fall back to plain timestamp options
        return build_data_options(buf, ts_val, ts_ecr);
    }

    let mut p = 0usize;

    // Timestamp first (NOP+NOP+TS = 12 bytes)
    if p + 12 > buf.len() {
        return p;
    }
    buf[p] = OPT_NOP;
    buf[p + 1] = OPT_NOP;
    buf[p + 2] = OPT_TS;
    buf[p + 3] = 10;
    p += 4;
    p = write_u32(buf, p, ts_val);
    p = write_u32(buf, p, ts_ecr);

    // SACK option: NOP+NOP+kind+len+blocks
    let sack_body = 2 + n * 8; // kind(1) + len(1) + n*8
    if p + 2 + sack_body > buf.len() {
        return p;
    }

    buf[p] = OPT_NOP;
    buf[p + 1] = OPT_NOP;
    buf[p + 2] = OPT_SACK;
    buf[p + 3] = (2 + n * 8) as u8;
    p += 4;

    for b in 0..n {
        p = write_u32(buf, p, blocks[b].left);
        p = write_u32(buf, p, blocks[b].right);
    }

    // Pad to 4-byte boundary
    p = pad_to_4(buf, p);
    p
}

// ---------------------------------------------------------------------------
// SACK block management helpers (operate on TCB arrays directly)
// ---------------------------------------------------------------------------

/// Insert a new SACK block into the block array, then coalesce.
/// Most-recently-inserted block is placed at index 0 (per RFC 2018 §4).
///
/// `blocks` — the TCB's rcv_sack_blocks array.
/// `count`  — current number of valid blocks (in [0..4]).
pub fn sack_add_block(blocks: &mut [SackBlock; 4], count: &mut u8, new_left: u32, new_right: u32) {
    if new_left == new_right {
        return;
    } // empty range — ignore
    let new_blk = SackBlock {
        left: new_left,
        right: new_right,
    };

    // Shift existing blocks right to make room at index 0.
    // If already full (count==4), the oldest (index 3) falls off.
    let slots = if (*count as usize) < 4 {
        let c = *count as usize;
        *count = (c + 1) as u8;
        c + 1
    } else {
        4usize
    };

    // Shift [0..slots-1] → [1..slots]
    let n = slots.min(4);
    let mut i = n;
    while i > 1 {
        blocks[i - 1] = blocks[i - 2];
        i -= 1;
    }
    blocks[0] = new_blk;

    sack_coalesce(blocks, count);
}

/// Merge overlapping or adjacent SACK blocks in-place.
/// After merging, blocks are sorted in descending order of insertion (most
/// recent first), with duplicates removed.  Count is updated.
pub fn sack_coalesce(blocks: &mut [SackBlock; 4], count: &mut u8) {
    let n = (*count as usize).min(4);
    if n <= 1 {
        return;
    }

    // Simple O(n²) merge — n ≤ 4, so this is fine at no_std.
    let mut merged = [SackBlock::default(); 4];
    let mut out = 0usize;

    'outer: for i in 0..n {
        let blk = blocks[i];
        if blk.is_empty() {
            continue;
        }
        // Try to merge with an already-merged block
        for j in 0..out {
            if merged[j].overlaps_or_adjacent(blk) {
                merged[j] = merged[j].merge(blk);
                continue 'outer;
            }
        }
        if out < 4 {
            merged[out] = blk;
            out += 1;
        }
    }

    // Copy back
    for i in 0..4 {
        blocks[i] = if i < out {
            merged[i]
        } else {
            SackBlock::default()
        };
    }
    *count = out as u8;
}

/// Remove SACK blocks that are fully covered by a cumulative ACK advance.
///
/// Any block whose `right` seq is ≤ `ack_seq` (i.e. already acknowledged
/// cumulatively) is dropped.
pub fn sack_prune(blocks: &mut [SackBlock; 4], count: &mut u8, ack_seq: u32) {
    let n = (*count as usize).min(4);
    let mut out = 0usize;
    let mut tmp = [SackBlock::default(); 4];

    for i in 0..n {
        let b = blocks[i];
        if !b.is_empty() && seq_gt(b.right, ack_seq) {
            // If the left edge is below ack_seq, trim it
            let left = if seq_lt(b.left, ack_seq) {
                ack_seq
            } else {
                b.left
            };
            if seq_lt(left, b.right) && out < 4 {
                tmp[out] = SackBlock {
                    left,
                    right: b.right,
                };
                out += 1;
            }
        }
    }

    for i in 0..4 {
        blocks[i] = if i < out {
            tmp[i]
        } else {
            SackBlock::default()
        };
    }
    *count = out as u8;
}

/// Returns true if `seq` (the first byte of a segment) is already covered by
/// one of the SACK blocks, meaning we can skip retransmitting that segment.
pub fn sack_covers(blocks: &[SackBlock; 4], count: u8, seq: u32) -> bool {
    let n = (count as usize).min(4);
    for i in 0..n {
        if blocks[i].contains(seq) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Sequence number comparison helpers (mirror of tcp.rs; kept local to avoid
// a cross-module dependency on private functions)
// ---------------------------------------------------------------------------

#[inline]
fn seq_lt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
#[inline]
fn seq_leq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) <= 0
}
#[inline]
fn seq_gt(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}
#[inline]
fn seq_geq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}
