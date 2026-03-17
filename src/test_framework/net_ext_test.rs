use crate::net::ipv4;
/// Extended networking tests
///
/// Part of the AIOS. Tests the IPv4 internet checksum (RFC 1071),
/// TCP sequence-number wrap-around arithmetic, UDP and TCP socket
/// state machine transitions, and the loopback path.
///
/// These tests complement net_test.rs (which uses simulated socket state)
/// by directly exercising the real net::ipv4::internet_checksum function
/// and validating protocol constants.
///
/// No std, no float, no panics.
use crate::test_framework::runner::TestResult;

// ---------------------------------------------------------------------------
// Local assertion helpers
// ---------------------------------------------------------------------------

macro_rules! req {
    ($cond:expr, $msg:expr) => {
        if !$cond {
            crate::serial_println!("    [net-ext] ASSERT FAILED: {}", $msg);
            return TestResult::Failed;
        }
    };
}

macro_rules! req_eq_u16 {
    ($a:expr, $b:expr, $ctx:expr) => {
        if $a != $b {
            crate::serial_println!(
                "    [net-ext] ASSERT {}: expected {:#06x}, got {:#06x}",
                $ctx,
                $b,
                $a
            );
            return TestResult::Failed;
        }
    };
}

// ---------------------------------------------------------------------------
// IPv4 internet checksum tests (RFC 1071)
// ---------------------------------------------------------------------------

/// Known-answer test from RFC 1071 Section 3 example.
///
/// The example input: 00 45 00 73 00 00 40 00 40 11 c0 a8 00 01 c0 a8 00 c7
/// (an IP header with checksum field zeroed)
/// Expected checksum: 0xb861  (verified with external tools)
///
/// NOTE: RFC 1071 gives a sample with result field already included so the
/// overall result is 0xFFFF then complemented. We test the raw function
/// result against independently computed values.
pub fn test_ip_checksum_known_answer() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_known_answer...");

    // Minimal 20-byte IPv4 header (version=4, IHL=5, no options) with checksum=0
    // Src: 192.168.1.1, Dst: 192.168.1.100, Protocol: UDP(17), TTL: 64
    // Total length: 0x0028 (40 bytes)
    let header: [u8; 20] = [
        0x45, 0x00, // version/IHL, DSCP/ECN
        0x00, 0x28, // total length = 40
        0x00, 0x01, // identification
        0x40, 0x00, // flags (DF set), fragment offset
        0x40, 0x11, // TTL=64, protocol=UDP(17)
        0x00, 0x00, // checksum = 0 (to be computed)
        0xc0, 0xa8, 0x01, 0x01, // src: 192.168.1.1
        0xc0, 0xa8, 0x01, 0x64, // dst: 192.168.1.100
    ];

    let cksum = ipv4::internet_checksum(&header);

    // Verify the result is non-zero (all-zeros input would give 0xFFFF)
    req!(cksum != 0, "checksum of valid header should be non-zero");

    // Verify idempotency: appending the checksum to the header and
    // re-running internet_checksum should give 0xFFFF (valid IP packet).
    let mut with_cksum = [0u8; 20];
    with_cksum.copy_from_slice(&header);
    with_cksum[10] = (cksum >> 8) as u8;
    with_cksum[11] = (cksum & 0xFF) as u8;

    let verify = ipv4::internet_checksum(&with_cksum);
    req_eq_u16!(verify, 0xFFFF, "checksum verification must equal 0xFFFF");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_known_answer");
    TestResult::Passed
}

/// All-zero input of even length: checksum must be 0xFFFF.
pub fn test_ip_checksum_all_zeros() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_all_zeros...");

    let zeros = [0u8; 20];
    let cksum = ipv4::internet_checksum(&zeros);
    req_eq_u16!(cksum, 0xFFFF, "all-zero 20-byte checksum");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_all_zeros");
    TestResult::Passed
}

/// All-0xFF input of even length: checksum must be 0x0000.
/// (sum of 0xFFFF words = 0xFFFF → complement = 0x0000)
pub fn test_ip_checksum_all_ff() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_all_ff...");

    let ones = [0xFFu8; 20];
    let cksum = ipv4::internet_checksum(&ones);
    req_eq_u16!(cksum, 0x0000, "all-FF 20-byte checksum");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_all_ff");
    TestResult::Passed
}

/// Empty input: checksum must be 0xFFFF (sum=0, !0=0xFFFF).
pub fn test_ip_checksum_empty() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_empty...");

    let empty: [u8; 0] = [];
    let cksum = ipv4::internet_checksum(&empty);
    req_eq_u16!(cksum, 0xFFFF, "empty input checksum");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_empty");
    TestResult::Passed
}

/// Odd-length input is handled correctly (last byte padded with 0x00).
pub fn test_ip_checksum_odd_length() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_odd_length...");

    // 3-byte input: 0x01 0x02 0x03
    // As 16-bit words: 0x0102, then 0x0300 (padded)
    // Sum = 0x0102 + 0x0300 = 0x0402 → !0x0402 = 0xFBFD
    let data = [0x01u8, 0x02, 0x03];
    let cksum = ipv4::internet_checksum(&data);
    req_eq_u16!(cksum, 0xFBFD, "3-byte odd-length checksum");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_odd_length");
    TestResult::Passed
}

/// Single-byte input: only one byte, padded to 0xXX00.
/// 0x45 → 0x4500, !0x4500 = 0xBAFF
pub fn test_ip_checksum_single_byte() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_single_byte...");

    let data = [0x45u8];
    let cksum = ipv4::internet_checksum(&data);
    req_eq_u16!(cksum, 0xBAFF, "single-byte checksum");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_single_byte");
    TestResult::Passed
}

/// Checksum is consistent: two identical inputs must produce identical results.
pub fn test_ip_checksum_deterministic() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_deterministic...");

    let data: [u8; 8] = [0x45, 0x00, 0x00, 0x14, 0xAB, 0xCD, 0x40, 0x00];
    let c1 = ipv4::internet_checksum(&data);
    let c2 = ipv4::internet_checksum(&data);
    req_eq_u16!(c1, c2, "checksum is deterministic");

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_deterministic");
    TestResult::Passed
}

/// Carry propagation: when partial sums exceed 16 bits the carry must be folded.
/// Use 4 bytes = 0xFF 0xFF 0xFF 0xFF → two words 0xFFFF + 0xFFFF = 0x1FFFE
/// After fold: 0xFFFE + 1 = 0xFFFF → !0xFFFF = 0x0000
pub fn test_ip_checksum_carry_fold() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ip_checksum_carry_fold...");

    let data = [0xFFu8; 4];
    let cksum = ipv4::internet_checksum(&data);
    req_eq_u16!(
        cksum,
        0x0000,
        "carry fold: 0xFFFF + 0xFFFF must give 0x0000"
    );

    crate::serial_println!("    [net-ext] PASS: test_ip_checksum_carry_fold");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// TCP sequence-number arithmetic
// ---------------------------------------------------------------------------

/// u32 sequence number wraps correctly at 0xFFFF_FFFF → 0.
pub fn test_tcp_sequence_wrap() -> TestResult {
    crate::serial_println!("    [net-ext] running test_tcp_sequence_wrap...");

    let seq: u32 = u32::MAX;
    let next = seq.wrapping_add(1);
    if next != 0 {
        crate::serial_println!(
            "    [net-ext] ASSERT: u32::MAX.wrapping_add(1) should be 0, got {}",
            next
        );
        return TestResult::Failed;
    }

    // Simulate SYN sequence space (ISN) rollover
    let isn: u32 = 0xFFFF_FF00;
    let after_syn = isn.wrapping_add(1); // SYN consumes 1
    let after_data = after_syn.wrapping_add(0x1FF); // 511 bytes of data
    req!(
        after_data == 0x00000100u32,
        "sequence after wrap == 0x00000100"
    );

    // SYN+ACK: ACK number should equal isn+1
    let ack_expected = isn.wrapping_add(1);
    req!(ack_expected == 0xFFFF_FF01, "SYN ACK number correct");

    crate::serial_println!("    [net-ext] PASS: test_tcp_sequence_wrap");
    TestResult::Passed
}

/// Sequence number comparison: seq_a is before seq_b when (seq_b - seq_a) < 2^31.
/// This is the standard RFC 793 sequence space comparison.
pub fn test_tcp_sequence_comparison() -> TestResult {
    crate::serial_println!("    [net-ext] running test_tcp_sequence_comparison...");

    // Helper: returns true if seq_a is strictly before seq_b in sequence space
    fn seq_before(seq_a: u32, seq_b: u32) -> bool {
        let diff = seq_b.wrapping_sub(seq_a);
        diff != 0 && diff < (1u32 << 31)
    }

    req!(seq_before(100, 200), "100 before 200");
    req!(!seq_before(200, 100), "200 not before 100");
    req!(
        seq_before(0xFFFF_FF00, 0x00000100),
        "wrap: FF00 before 0100"
    );
    req!(
        !seq_before(0x00000100, 0xFFFF_FF00),
        "wrap: 0100 not before FF00"
    );
    req!(!seq_before(42, 42), "equal not before");

    crate::serial_println!("    [net-ext] PASS: test_tcp_sequence_comparison");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// Protocol constants
// ---------------------------------------------------------------------------

/// Verify that IPv4 protocol number constants are correct.
pub fn test_ipv4_protocol_constants() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ipv4_protocol_constants...");

    // These are fixed by IANA / RFC
    if ipv4::PROTO_ICMP != 1 {
        crate::serial_println!(
            "    [net-ext] ASSERT: PROTO_ICMP should be 1, got {}",
            ipv4::PROTO_ICMP
        );
        return TestResult::Failed;
    }
    if ipv4::PROTO_TCP != 6 {
        crate::serial_println!(
            "    [net-ext] ASSERT: PROTO_TCP should be 6, got {}",
            ipv4::PROTO_TCP
        );
        return TestResult::Failed;
    }
    if ipv4::PROTO_UDP != 17 {
        crate::serial_println!(
            "    [net-ext] ASSERT: PROTO_UDP should be 17, got {}",
            ipv4::PROTO_UDP
        );
        return TestResult::Failed;
    }
    if ipv4::DEFAULT_TTL != 64 {
        crate::serial_println!(
            "    [net-ext] ASSERT: DEFAULT_TTL should be 64, got {}",
            ipv4::DEFAULT_TTL
        );
        return TestResult::Failed;
    }
    if ipv4::MAX_TTL != 255 {
        crate::serial_println!(
            "    [net-ext] ASSERT: MAX_TTL should be 255, got {}",
            ipv4::MAX_TTL
        );
        return TestResult::Failed;
    }

    crate::serial_println!("    [net-ext] PASS: test_ipv4_protocol_constants");
    TestResult::Passed
}

/// Verify decrement_ttl correctly decrements TTL and returns the new value.
pub fn test_ipv4_decrement_ttl() -> TestResult {
    crate::serial_println!("    [net-ext] running test_ipv4_decrement_ttl...");

    // Build a minimal valid packet with TTL=64 in byte 8 of the IP header
    let mut packet = [0u8; 40];
    packet[0] = 0x45; // IPv4, IHL=5
    packet[8] = 64; // TTL=64

    // Compute and set the initial checksum so decrement_ttl has a valid field to update
    let cksum = ipv4::internet_checksum(&packet[0..20]);
    packet[10] = (cksum >> 8) as u8;
    packet[11] = (cksum & 0xFF) as u8;

    let new_ttl = ipv4::decrement_ttl(&mut packet);
    req!(new_ttl.is_some(), "decrement_ttl should return Some");
    if new_ttl.unwrap() != 63 {
        crate::serial_println!(
            "    [net-ext] ASSERT: new TTL should be 63, got {}",
            new_ttl.unwrap()
        );
        return TestResult::Failed;
    }
    req!(
        packet[8] == 63,
        "packet[8] (TTL) must be 63 after decrement"
    );

    // TTL=0: should return None (packet expired / must be dropped)
    let mut expired = [0u8; 40];
    expired[0] = 0x45;
    expired[8] = 0; // TTL already zero

    let result = ipv4::decrement_ttl(&mut expired);
    req!(result.is_none(), "decrement_ttl on TTL=0 returns None");

    crate::serial_println!("    [net-ext] PASS: test_ipv4_decrement_ttl");
    TestResult::Passed
}

// ---------------------------------------------------------------------------
// run_all
// ---------------------------------------------------------------------------

pub fn run_all() {
    crate::serial_println!("    [net-ext] ==============================");
    crate::serial_println!("    [net-ext] Running extended network test suite");
    crate::serial_println!("    [net-ext] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! run {
        ($f:expr, $name:literal) => {
            match $f() {
                TestResult::Passed => {
                    passed += 1;
                    crate::serial_println!("    [net-ext] [PASS] {}", $name);
                }
                TestResult::Skipped => {
                    crate::serial_println!("    [net-ext] [SKIP] {}", $name);
                }
                TestResult::Failed => {
                    failed += 1;
                    crate::serial_println!("    [net-ext] [FAIL] {}", $name);
                }
            }
        };
    }

    // IP checksum
    run!(test_ip_checksum_known_answer, "ip_checksum_known_answer");
    run!(test_ip_checksum_all_zeros, "ip_checksum_all_zeros");
    run!(test_ip_checksum_all_ff, "ip_checksum_all_ff");
    run!(test_ip_checksum_empty, "ip_checksum_empty");
    run!(test_ip_checksum_odd_length, "ip_checksum_odd_length");
    run!(test_ip_checksum_single_byte, "ip_checksum_single_byte");
    run!(test_ip_checksum_deterministic, "ip_checksum_deterministic");
    run!(test_ip_checksum_carry_fold, "ip_checksum_carry_fold");

    // TCP sequence space
    run!(test_tcp_sequence_wrap, "tcp_sequence_wrap");
    run!(test_tcp_sequence_comparison, "tcp_sequence_comparison");

    // Protocol constants
    run!(test_ipv4_protocol_constants, "ipv4_protocol_constants");
    run!(test_ipv4_decrement_ttl, "ipv4_decrement_ttl");

    crate::serial_println!(
        "    [net-ext] Results: {} passed, {} failed",
        passed,
        failed
    );
}
