use crate::io::{inb, outb};
/// GDB Remote Serial Protocol (RSP) stub for Genesis kernel debugging
///
/// Listens on COM2 (0x2F8) for a GDB client connection.  Also compatible
/// with QEMU's built-in GDB server when launched with `-s` (listens on
/// TCP:1234) — in that case this stub is bypassed and QEMU handles RSP.
///
/// Implements the minimal RSP packet set required for `gdb` to attach,
/// inspect registers and memory, insert/remove software breakpoints, and
/// single-step:
///
///   ?        — halt reason          → T05 (SIGTRAP)
///   g        — read all registers   → 16 GPRs + RIP + RFLAGS + seg regs
///   G XX..   — write all registers  ← hex register image
///   m a,l    — read memory          → hex bytes
///   M a,l:XX — write memory         ← hex bytes
///   c        — continue             → (empty, resumes execution)
///   s        — single step          → T05 after one instruction
///   Z0,a,4   — insert SW breakpoint (INT3 at addr)
///   z0,a,4   — remove SW breakpoint (restore original byte)
///
/// Packet framing follows the RSP wire format:
///   `$<data>#<checksum2hex>`
/// Every packet received is ACKed with `+` before processing.
///
/// ## Serial port usage
///
/// COM2 (0x2F8) is used so that the kernel's existing COM1 (0x3F8) debug
/// output is not disturbed.  The stub programs COM2 to 115200 baud, 8N1.
///
/// ## Safety and `no_std`
///
/// The stub is fully `no_std`.  It interacts with memory using raw pointer
/// reads/writes, gated by the same unsafe rules as the rest of the kernel.
/// The breakpoint table is a fixed-size array — no heap allocation needed.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// COM2 base address
// ---------------------------------------------------------------------------

/// I/O base of COM2 (second UART, 16550-compatible)
pub const GDB_PORT: u16 = 0x2F8;

// ---------------------------------------------------------------------------
// Atomic flag: is a GDB client currently attached?
// ---------------------------------------------------------------------------

/// Set to `true` once GDB has sent its first `?` packet (i.e. has attached).
pub static GDB_ACTIVE: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Maximum software breakpoints tracked simultaneously
// ---------------------------------------------------------------------------

const MAX_BREAKPOINTS: usize = 64;

// ---------------------------------------------------------------------------
// GdbState — all mutable stub state lives here, behind a single Mutex
// ---------------------------------------------------------------------------

struct Breakpoint {
    addr: u64,
    orig_byte: u8,
    active: bool,
}

impl Breakpoint {
    const fn empty() -> Self {
        Breakpoint {
            addr: 0,
            orig_byte: 0,
            active: false,
        }
    }
}

/// All mutable state for the GDB stub.
struct GdbState {
    /// Is the target currently halted / stopped?
    halted: bool,
    /// Signal number to report to GDB (5 = SIGTRAP)
    last_signal: u8,
    /// Saved register state (populated on halt)
    regs: Registers,
    /// Software breakpoint table
    bps: [Breakpoint; MAX_BREAKPOINTS],
    bp_count: usize,
    /// Receive packet buffer (one RSP payload at a time, no framing bytes)
    rx_buf: [u8; 512],
    rx_len: usize,
    /// Transmit packet buffer (one RSP payload at a time, no framing bytes)
    tx_buf: [u8; 512],
    tx_len: usize,
}

impl GdbState {
    const fn new() -> Self {
        GdbState {
            halted: false,
            last_signal: 5, // SIGTRAP
            regs: Registers::zeroed(),
            bps: [const { Breakpoint::empty() }; MAX_BREAKPOINTS],
            bp_count: 0,
            rx_buf: [0u8; 512],
            rx_len: 0,
            tx_buf: [0u8; 512],
            tx_len: 0,
        }
    }
}

/// x86-64 register snapshot used by the GDB stub.
///
/// GDB's x86-64 register file (as defined in `gdb/features/i386/64bit-core.xml`):
///   Registers 0–15  : RAX RCX RDX RBX RSP RBP RSI RDI R8–R15  (8 bytes each)
///   Register  16     : RIP                                       (8 bytes)
///   Register  17     : EFLAGS                                    (4 bytes, zero-extended)
///   Registers 18–23  : CS SS DS ES FS GS                        (4 bytes each, zero-extended)
struct Registers {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rbp: u64,
    rsp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    rflags: u64,
    cs: u16,
    ss: u16,
    ds: u16,
    es: u16,
}

impl Registers {
    const fn zeroed() -> Self {
        Registers {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0,
            cs: 0,
            ss: 0,
            ds: 0,
            es: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global stub state
// ---------------------------------------------------------------------------

static STUB_STATE: Mutex<GdbState> = Mutex::new(GdbState::new());

// ---------------------------------------------------------------------------
// UART helpers — thin wrappers around COM2
// ---------------------------------------------------------------------------

/// Initialize COM2: 115200 baud, 8N1, FIFOs enabled.
fn uart_init() {
    outb(GDB_PORT + 1, 0x00); // disable interrupts
    outb(GDB_PORT + 3, 0x80); // DLAB on
    outb(GDB_PORT + 0, 0x01); // divisor lo: 1 → 115200 baud
    outb(GDB_PORT + 1, 0x00); // divisor hi
    outb(GDB_PORT + 3, 0x03); // 8N1, DLAB off
    outb(GDB_PORT + 2, 0xC7); // FIFO on, clear, 14-byte threshold
    outb(GDB_PORT + 4, 0x0B); // RTS/DSR set
}

/// Block until COM2 has a received byte ready, then return it.
#[inline]
fn uart_recv() -> u8 {
    while inb(GDB_PORT + 5) & 0x01 == 0 {
        core::hint::spin_loop();
    }
    inb(GDB_PORT)
}

/// Block until COM2 transmit holding register is empty, then send one byte.
#[inline]
fn uart_send(byte: u8) {
    while inb(GDB_PORT + 5) & 0x20 == 0 {
        core::hint::spin_loop();
    }
    outb(GDB_PORT, byte);
}

// ---------------------------------------------------------------------------
// RSP framing helpers
// ---------------------------------------------------------------------------

/// Compute the RSP checksum: sum of all payload bytes, mod 256.
fn rsp_checksum(data: &[u8]) -> u8 {
    let mut sum: u8 = 0;
    for &b in data {
        sum = sum.wrapping_add(b);
    }
    sum
}

/// Encode a nibble (0–15) as its lowercase hex ASCII character.
#[inline]
fn hex_nibble(n: u8) -> u8 {
    if n < 10 {
        b'0' + n
    } else {
        b'a' + n - 10
    }
}

/// Decode a single hex ASCII character to its nibble value.
/// Returns `None` for invalid characters.
#[inline]
fn nibble_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Send a complete RSP packet: `$<payload>#<checksum2hex>`.
/// Also sends the leading `+` ACK that GDB expects before each reply.
fn send_packet(payload: &[u8]) {
    let cksum = rsp_checksum(payload);
    uart_send(b'$');
    for &b in payload {
        // Byte stuffing: `$`, `#`, `}`, `*` must be escaped as `}` XOR 0x20
        if b == b'$' || b == b'#' || b == b'}' || b == b'*' {
            uart_send(b'}');
            uart_send(b ^ 0x20);
        } else {
            uart_send(b);
        }
    }
    uart_send(b'#');
    uart_send(hex_nibble(cksum >> 4));
    uart_send(hex_nibble(cksum & 0x0F));
}

/// Send a plain ASCII string as an RSP packet (convenience wrapper).
fn send_str(s: &[u8]) {
    send_packet(s);
}

/// Send the "empty" response (i.e. `$#00`) — used when a command is
/// not supported or when the CPU resumes from `c`.
fn send_empty() {
    send_packet(b"");
}

/// Send an RSP error reply `Exx` (where xx is the error number in hex).
fn send_error(code: u8) {
    let pkt = [b'E', hex_nibble(code >> 4), hex_nibble(code & 0x0F)];
    send_packet(&pkt);
}

/// Send an RSP OK reply `OK`.
fn send_ok() {
    send_packet(b"OK");
}

/// Block until a complete RSP packet has been received.
///
/// Discards everything before the `$` framing byte.  Returns the payload
/// length (the bytes are stored in `state.rx_buf[..state.rx_len]`).
/// Also handles the `+` (ACK) and `-` (NAK) control bytes from GDB by
/// ignoring ACKs and resending the last packet on NAK (simplified: we just
/// ignore NAKs for now — the wire is reliable in QEMU).
fn recv_packet(state: &mut GdbState) -> usize {
    loop {
        // Wait for '$'
        loop {
            let b = uart_recv();
            if b == b'+' {
                continue;
            } // ACK from previous packet — ignore
            if b == b'-' {
                continue;
            } // NAK — retransmit not implemented
            if b == b'$' {
                break;
            }
            // Any other byte before '$' is noise — discard
        }

        // Read payload bytes until '#'
        let mut len = 0usize;
        let mut esc = false;
        loop {
            let b = uart_recv();
            if b == b'#' {
                break;
            }
            if len >= state.rx_buf.len() {
                // Overflow — restart
                len = 0;
                continue;
            }
            if esc {
                state.rx_buf[len] = b ^ 0x20;
                len += 1;
                esc = false;
            } else if b == b'}' {
                esc = true;
            } else {
                state.rx_buf[len] = b;
                len += 1;
            }
        }

        // Read 2-character checksum
        let c_hi = uart_recv();
        let c_lo = uart_recv();
        let recv_cksum = match (nibble_val(c_hi), nibble_val(c_lo)) {
            (Some(hi), Some(lo)) => (hi << 4) | lo,
            _ => 0xFF, // invalid hex → force mismatch
        };

        let calc_cksum = rsp_checksum(&state.rx_buf[..len]);

        if calc_cksum == recv_cksum {
            // Good packet — ACK and return
            uart_send(b'+');
            state.rx_len = len;
            return len;
        } else {
            // Bad checksum — NAK, retry
            uart_send(b'-');
        }
    }
}

// ---------------------------------------------------------------------------
// Hex encoding helpers for register/memory replies
// ---------------------------------------------------------------------------

/// Append a `u64` value in little-endian byte order as 16 hex characters.
fn append_u64_le(buf: &mut [u8], pos: &mut usize, val: u64) {
    let bytes = val.to_le_bytes();
    for b in bytes {
        if *pos + 2 <= buf.len() {
            buf[*pos] = hex_nibble(b >> 4);
            buf[*pos + 1] = hex_nibble(b & 0x0F);
            *pos += 2;
        }
    }
}

/// Append a `u32` (zero-extended to u64) in little-endian as 8 hex chars.
fn append_u32_le(buf: &mut [u8], pos: &mut usize, val: u32) {
    append_u64_le(buf, pos, val as u64);
}

/// Parse a `u64` from `len` little-endian hex bytes starting at `src[off]`.
/// Returns `None` if there are fewer than `len*2` valid hex chars available.
fn parse_u64_le(src: &[u8], off: usize, len: usize) -> Option<u64> {
    if src.len() < off + len * 2 {
        return None;
    }
    let mut val = 0u64;
    for i in 0..len {
        let hi = nibble_val(src[off + i * 2])?;
        let lo = nibble_val(src[off + i * 2 + 1])?;
        let byte = ((hi << 4) | lo) as u64;
        val |= byte << (i * 8);
    }
    Some(val)
}

/// Parse a hex integer (`u64`) from ASCII bytes, stopping at a non-hex char.
/// Returns `(value, bytes_consumed)`.
fn parse_hex_u64(src: &[u8]) -> (u64, usize) {
    let mut val = 0u64;
    let mut n = 0usize;
    for &b in src {
        if let Some(nib) = nibble_val(b) {
            val = (val << 4) | nib as u64;
            n += 1;
        } else {
            break;
        }
    }
    (val, n)
}

// ---------------------------------------------------------------------------
// Register packet builder: `g` reply
// ---------------------------------------------------------------------------

/// Serialise the current register state into GDB's x86-64 register file.
///
/// GDB expects (in this exact order, LE hex):
///   rax rcx rdx rbx rsp rbp rsi rdi r8..r15  (8 bytes each = 16 hex chars each)
///   rip                                        (8 bytes)
///   eflags                                     (4 bytes = 8 hex chars)
///   cs ss ds es fs gs                          (4 bytes each)
fn build_g_reply(state: &GdbState, out: &mut [u8]) -> usize {
    let r = &state.regs;
    let mut pos = 0usize;
    // GDB x86-64 order: rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15
    append_u64_le(out, &mut pos, r.rax);
    append_u64_le(out, &mut pos, r.rcx);
    append_u64_le(out, &mut pos, r.rdx);
    append_u64_le(out, &mut pos, r.rbx);
    append_u64_le(out, &mut pos, r.rsp);
    append_u64_le(out, &mut pos, r.rbp);
    append_u64_le(out, &mut pos, r.rsi);
    append_u64_le(out, &mut pos, r.rdi);
    append_u64_le(out, &mut pos, r.r8);
    append_u64_le(out, &mut pos, r.r9);
    append_u64_le(out, &mut pos, r.r10);
    append_u64_le(out, &mut pos, r.r11);
    append_u64_le(out, &mut pos, r.r12);
    append_u64_le(out, &mut pos, r.r13);
    append_u64_le(out, &mut pos, r.r14);
    append_u64_le(out, &mut pos, r.r15);
    // rip
    append_u64_le(out, &mut pos, r.rip);
    // eflags (4 bytes, LE)
    append_u32_le(out, &mut pos, r.rflags as u32);
    // segment registers (4 bytes each)
    append_u32_le(out, &mut pos, r.cs as u32);
    append_u32_le(out, &mut pos, r.ss as u32);
    append_u32_le(out, &mut pos, r.ds as u32);
    append_u32_le(out, &mut pos, r.es as u32);
    // fs and gs — report as 0 (no per-thread base in this stub)
    append_u32_le(out, &mut pos, 0u32);
    append_u32_le(out, &mut pos, 0u32);
    pos
}

// ---------------------------------------------------------------------------
// RSP command dispatch
// ---------------------------------------------------------------------------

/// Parse and dispatch one RSP command stored in `state.rx_buf[..state.rx_len]`.
fn handle_packet(state: &mut GdbState) {
    if state.rx_len == 0 {
        send_empty();
        return;
    }

    let cmd = state.rx_buf[0];
    let payload = &state.rx_buf[1..state.rx_len];

    match cmd {
        // ------------------------------------------------------------------ ?
        // Halt reason: always reply T05 (stopped with SIGTRAP)
        b'?' => {
            GDB_ACTIVE.store(true, Ordering::SeqCst);
            state.halted = true;
            let sig = state.last_signal;
            let reply = [b'T', hex_nibble(sig >> 4), hex_nibble(sig & 0x0F)];
            send_packet(&reply);
        }

        // ------------------------------------------------------------------ g
        // Read all registers
        b'g' => {
            let mut buf = [0u8; 512];
            let len = build_g_reply(state, &mut buf);
            send_packet(&buf[..len]);
        }

        // ------------------------------------------------------------------ G
        // Write all registers from hex data (GXX... where XX are hex bytes)
        b'G' => {
            let r = &mut state.regs;
            let d = payload;
            // Each 64-bit register is 16 hex chars; 32-bit regs are 8 hex chars.
            // Order mirrors build_g_reply.
            let mut off = 0usize;
            macro_rules! rd64 {
                ($f:expr) => {
                    if let Some(v) = parse_u64_le(d, off, 8) {
                        $f = v;
                    }
                    off += 16;
                };
            }
            macro_rules! rd32 {
                ($f:expr) => {
                    if let Some(v) = parse_u64_le(d, off, 4) {
                        $f = v as u16;
                    }
                    off += 8;
                };
            }
            rd64!(r.rax);
            rd64!(r.rcx);
            rd64!(r.rdx);
            rd64!(r.rbx);
            rd64!(r.rsp);
            rd64!(r.rbp);
            rd64!(r.rsi);
            rd64!(r.rdi);
            rd64!(r.r8);
            rd64!(r.r9);
            rd64!(r.r10);
            rd64!(r.r11);
            rd64!(r.r12);
            rd64!(r.r13);
            rd64!(r.r14);
            rd64!(r.r15);
            rd64!(r.rip);
            if let Some(v) = parse_u64_le(d, off, 4) {
                r.rflags = v;
            }
            off += 8;
            rd32!(r.cs);
            rd32!(r.ss);
            rd32!(r.ds);
            rd32!(r.es);
            let _ = off; // fs/gs consumed but not stored
            send_ok();
        }

        // ------------------------------------------------------------------ m
        // Read memory: `mADDR,LEN`
        b'm' => {
            // Parse ADDR,LEN
            let (addr, a_len) = parse_hex_u64(payload);
            if a_len == 0 || payload.get(a_len) != Some(&b',') {
                send_error(0x01);
                return;
            }
            let (len, _) = parse_hex_u64(&payload[a_len + 1..]);
            let len = (len as usize).min(state.tx_buf.len() / 2);

            let mut out_pos = 0usize;
            for i in 0..len {
                let byte = unsafe { core::ptr::read_volatile((addr + i as u64) as *const u8) };
                if out_pos + 2 <= state.tx_buf.len() {
                    state.tx_buf[out_pos] = hex_nibble(byte >> 4);
                    state.tx_buf[out_pos + 1] = hex_nibble(byte & 0x0F);
                    out_pos += 2;
                }
            }
            state.tx_len = out_pos;
            send_packet(&state.tx_buf[..out_pos]);
        }

        // ------------------------------------------------------------------ M
        // Write memory: `MADDR,LEN:HEXBYTES`
        b'M' => {
            let (addr, a_len) = parse_hex_u64(payload);
            if a_len == 0 {
                send_error(0x01);
                return;
            }
            let rest = &payload[a_len..];
            if rest.get(0) != Some(&b',') {
                send_error(0x01);
                return;
            }
            let (len, l_len) = parse_hex_u64(&rest[1..]);
            let rest2 = &rest[1 + l_len..];
            if rest2.get(0) != Some(&b':') {
                send_error(0x01);
                return;
            }
            let hex_data = &rest2[1..];
            let len = len as usize;

            for i in 0..len {
                if let (Some(hi), Some(lo)) = (
                    hex_data.get(i * 2).copied(),
                    hex_data.get(i * 2 + 1).copied(),
                ) {
                    if let (Some(h), Some(l)) = (nibble_val(hi), nibble_val(lo)) {
                        let byte = (h << 4) | l;
                        unsafe {
                            core::ptr::write_volatile((addr + i as u64) as *mut u8, byte);
                        }
                    }
                }
            }
            send_ok();
        }

        // ------------------------------------------------------------------ c
        // Continue: resume execution.  We reply with an empty packet and
        // set halted=false.  The stub will stop again when an INT3 fires or
        // when the debugger re-halts the target.
        b'c' => {
            state.halted = false;
            state.regs.rflags &= !0x100; // clear TF (trap flag) — we are continuing, not stepping
            send_empty();
        }

        // ------------------------------------------------------------------ s
        // Single step: set TF in RFLAGS, then "continue" for one instruction.
        // We report T05 immediately here (the real execution happens when the
        // calling code re-enters normal execution after the interrupt handler
        // exits).
        b's' => {
            state.regs.rflags |= 0x100; // set TF (trap flag) — fires DB# after next instruction
            state.halted = false;
            state.last_signal = 5; // SIGTRAP
                                   // Reply T05 to tell GDB we stopped after the step
            send_packet(b"T05");
        }

        // ------------------------------------------------------------------ Z
        // Insert breakpoint: `Z0,ADDR,KIND`
        // We only handle Z0 (software INT3 breakpoint).
        b'Z' => {
            let bp_type = *payload.get(0).unwrap_or(&b'?');
            if bp_type != b'0' {
                // Hardware / watchpoint breakpoints not implemented
                send_empty();
                return;
            }
            // payload: "0,ADDR,KIND"
            let after_type = &payload[1..];
            if after_type.get(0) != Some(&b',') {
                send_error(0x01);
                return;
            }
            let (addr, a_len) = parse_hex_u64(&after_type[1..]);
            if a_len == 0 {
                send_error(0x01);
                return;
            }

            // Check duplicate
            let already =
                (0..state.bp_count).any(|i| state.bps[i].active && state.bps[i].addr == addr);
            if already {
                send_ok();
                return;
            }

            if state.bp_count >= MAX_BREAKPOINTS {
                send_error(0x0E); // too many breakpoints
                return;
            }

            // Read and save the original byte, then write INT3 (0xCC)
            let orig = unsafe { core::ptr::read_volatile(addr as *const u8) };
            unsafe {
                core::ptr::write_volatile(addr as *mut u8, 0xCC);
            }

            let idx = state.bp_count;
            state.bps[idx] = Breakpoint {
                addr,
                orig_byte: orig,
                active: true,
            };
            state.bp_count += 1;

            send_ok();
        }

        // ------------------------------------------------------------------ z
        // Remove breakpoint: `z0,ADDR,KIND`
        b'z' => {
            let bp_type = *payload.get(0).unwrap_or(&b'?');
            if bp_type != b'0' {
                send_empty();
                return;
            }
            let after_type = &payload[1..];
            if after_type.get(0) != Some(&b',') {
                send_error(0x01);
                return;
            }
            let (addr, a_len) = parse_hex_u64(&after_type[1..]);
            if a_len == 0 {
                send_error(0x01);
                return;
            }

            // Find and remove the breakpoint
            let mut found = false;
            for i in 0..state.bp_count {
                if state.bps[i].active && state.bps[i].addr == addr {
                    // Restore the original byte
                    unsafe {
                        core::ptr::write_volatile(addr as *mut u8, state.bps[i].orig_byte);
                    }
                    state.bps[i].active = false;
                    found = true;
                    break;
                }
            }
            if found {
                send_ok()
            } else {
                send_error(0x01);
            }
        }

        // ------------------------------------------------------------------ q
        // General query packets — handle the minimum set GDB requires
        b'q' => {
            if payload.starts_with(b"Supported") {
                // Announce supported features; keep it minimal
                send_str(b"PacketSize=1ff");
            } else if payload == b"C" {
                // Current thread ID — report thread 1
                send_str(b"QC1");
            } else if payload == b"fThreadInfo" {
                // First thread in list
                send_str(b"m1");
            } else if payload == b"sThreadInfo" {
                // End of thread list
                send_str(b"l");
            } else if payload.starts_with(b"ThreadExtraInfo") {
                send_str(b"47656e65736973"); // "Genesis" in hex
            } else if payload.starts_with(b"Rcmd,") {
                // Remote monitor commands — unsupported
                send_ok();
            } else {
                send_empty();
            }
        }

        // ------------------------------------------------------------------ H
        // Set thread for subsequent operations — accept all, reply OK
        b'H' => {
            send_ok();
        }

        // ------------------------------------------------------------------ T
        // Is thread alive? — always say yes
        b'T' => {
            send_ok();
        }

        // ------------------------------------------------------------------ D
        // Detach: GDB is disconnecting
        b'D' => {
            GDB_ACTIVE.store(false, Ordering::SeqCst);
            state.halted = false;
            send_ok();
        }

        // ------------------------------------------------------------------ k
        // Kill — treat as detach in a kernel stub
        b'k' => {
            GDB_ACTIVE.store(false, Ordering::SeqCst);
            state.halted = false;
            // No reply — GDB does not wait for one after 'k'
        }

        // ------------------------------------------------------------------ everything else
        _ => {
            // Unknown packet — empty reply signals "not supported"
            send_empty();
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the GDB stub: configure COM2 and print the "waiting" message.
///
/// Called from `debug::init()`.  Does NOT block — the stub enters its packet
/// loop only when the kernel explicitly calls `gdb_wait()` or when an INT3 /
/// single-step exception fires and calls `gdb_exception_entry()`.
pub fn init() {
    uart_init();
    crate::serial_println!("  [gdb_stub] GDB RSP stub ready on COM2 (0x2F8) 115200-8N1");
    crate::serial_println!("  [gdb_stub] Connect: (gdb) target remote /dev/ttyS1");
    crate::serial_println!("  [gdb_stub] Or:      (gdb) target remote | socat - /dev/ttyS1");
}

/// Block waiting for GDB to send a packet and process it.
///
/// Call this from a breakpoint handler or from code that wants to enter the
/// debug loop (e.g. `kernel panic`).  Returns only when GDB sends `c`
/// (continue) or `D` (detach).
pub fn gdb_wait() {
    GDB_ACTIVE.store(true, Ordering::SeqCst);
    crate::serial_println!("  [gdb_stub] Entering GDB packet loop — waiting for client");
    loop {
        let halted = {
            let mut state = STUB_STATE.lock();
            state.halted = true;
            recv_packet(&mut state);
            handle_packet(&mut state);
            state.halted
        };
        if !halted {
            // GDB sent 'c' or 'D' — resume execution
            break;
        }
    }
}

/// Called from the INT3 / debug-exception handler to update the saved
/// register state and enter the packet loop.
///
/// `rip` should point to the INT3 instruction (the handler must back it up
/// by 1 if necessary before calling this function, per x86 INT3 conventions).
pub fn gdb_exception_entry(
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rbp: u64,
    rsp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    rflags: u64,
    cs: u16,
    ss: u16,
) {
    {
        let mut state = STUB_STATE.lock();
        state.regs = Registers {
            rax,
            rbx,
            rcx,
            rdx,
            rsi,
            rdi,
            rbp,
            rsp,
            r8,
            r9,
            r10,
            r11,
            r12,
            r13,
            r14,
            r15,
            rip,
            rflags,
            cs,
            ss,
            ds: 0,
            es: 0,
        };
        state.last_signal = 5; // SIGTRAP
        state.halted = true;
    }
    // Notify GDB of the stop, then enter the packet loop
    send_packet(b"T05");
    gdb_wait();
}

/// Update the saved RIP in the stub state.
///
/// Use this when the interrupt handler has adjusted the saved RIP (e.g. to
/// skip past the INT3 byte after a software breakpoint is hit).
pub fn set_saved_rip(rip: u64) {
    STUB_STATE.lock().regs.rip = rip;
}

/// Return `true` if a GDB client is currently attached.
pub fn is_active() -> bool {
    GDB_ACTIVE.load(Ordering::SeqCst)
}

/// Return the number of currently installed software breakpoints.
pub fn breakpoint_count() -> usize {
    let state = STUB_STATE.lock();
    state.bps[..state.bp_count]
        .iter()
        .filter(|b| b.active)
        .count()
}
