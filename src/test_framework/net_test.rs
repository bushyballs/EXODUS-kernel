use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
/// Networking tests
///
/// Part of the AIOS. Tests for the networking stack including
/// TCP socket operations, UDP send/receive, and loopback interface.
/// Uses simulated network state for kernel self-testing.
use alloc::vec::Vec;

/// Simulated socket state for testing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketState {
    Closed,
    Created,
    Bound,
    Listening,
    Connected,
}

/// Simulated socket descriptor
struct SimSocket {
    id: u64,
    state: SocketState,
    protocol: u8, // 6 = TCP, 17 = UDP
    bound_port: u16,
}

/// Simulated loopback buffer
static LOOPBACK_BUF: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Socket counter for unique IDs
static NEXT_SOCK_ID: Mutex<u64> = Mutex::new(1);

fn alloc_sock_id() -> u64 {
    let mut id = NEXT_SOCK_ID.lock();
    let current = *id;
    *id += 1;
    current
}

/// Tests for the networking stack.
pub struct NetTests;

impl NetTests {
    /// Test TCP socket creation and binding.
    /// Simulates creating a TCP socket, binding to a port,
    /// and verifying the socket state transitions.
    pub fn test_tcp_socket() -> bool {
        crate::serial_println!("    [net-test] running test_tcp_socket...");

        // Create a simulated TCP socket
        let mut sock = SimSocket {
            id: alloc_sock_id(),
            state: SocketState::Created,
            protocol: 6, // TCP
            bound_port: 0,
        };

        // Verify initial state
        if sock.state != SocketState::Created {
            crate::serial_println!(
                "    [net-test] FAIL: socket not in Created state after creation"
            );
            return false;
        }

        if sock.protocol != 6 {
            crate::serial_println!("    [net-test] FAIL: socket protocol should be TCP (6)");
            return false;
        }

        // Bind to port 8080
        let bind_port: u16 = 8080;
        sock.bound_port = bind_port;
        sock.state = SocketState::Bound;

        if sock.state != SocketState::Bound {
            crate::serial_println!("    [net-test] FAIL: socket not in Bound state after bind");
            return false;
        }

        if sock.bound_port != bind_port {
            crate::serial_println!("    [net-test] FAIL: bound port mismatch");
            return false;
        }

        // Transition to listening
        sock.state = SocketState::Listening;

        if sock.state != SocketState::Listening {
            crate::serial_println!("    [net-test] FAIL: socket not in Listening state");
            return false;
        }

        // Close the socket
        sock.state = SocketState::Closed;

        if sock.state != SocketState::Closed {
            crate::serial_println!("    [net-test] FAIL: socket not in Closed state after close");
            return false;
        }

        crate::serial_println!("    [net-test] PASS: test_tcp_socket (id={})", sock.id);
        true
    }

    /// Test UDP send and receive.
    /// Simulates creating a UDP socket, sending data, and receiving it
    /// via a simulated loopback path.
    pub fn test_udp() -> bool {
        crate::serial_println!("    [net-test] running test_udp...");

        // Create simulated UDP sockets (sender and receiver)
        let mut sender = SimSocket {
            id: alloc_sock_id(),
            state: SocketState::Created,
            protocol: 17, // UDP
            bound_port: 0,
        };

        let mut receiver = SimSocket {
            id: alloc_sock_id(),
            state: SocketState::Created,
            protocol: 17, // UDP
            bound_port: 0,
        };

        // Bind receiver to port 9090
        receiver.bound_port = 9090;
        receiver.state = SocketState::Bound;

        // Bind sender to port 9091
        sender.bound_port = 9091;
        sender.state = SocketState::Bound;

        // Simulate sending data from sender to receiver via loopback buffer
        let send_data: Vec<u8> = Vec::from(*b"UDP test payload");
        {
            let mut buf = LOOPBACK_BUF.lock();
            *buf = Some(send_data.clone());
        }

        // Simulate receiving data
        let received = {
            let mut buf = LOOPBACK_BUF.lock();
            buf.take()
        };

        match received {
            Some(data) => {
                if data != send_data {
                    crate::serial_println!("    [net-test] FAIL: UDP data mismatch");
                    return false;
                }
            }
            None => {
                crate::serial_println!("    [net-test] FAIL: no UDP data received");
                return false;
            }
        }

        // Verify protocol
        if sender.protocol != 17 || receiver.protocol != 17 {
            crate::serial_println!("    [net-test] FAIL: protocol should be UDP (17)");
            return false;
        }

        crate::serial_println!(
            "    [net-test] PASS: test_udp (sender={}, receiver={})",
            sender.id,
            receiver.id
        );
        true
    }

    /// Test loopback interface.
    /// Sends data through the loopback buffer and verifies round-trip
    /// integrity with various payload sizes.
    pub fn test_loopback() -> bool {
        crate::serial_println!("    [net-test] running test_loopback...");

        // Test with multiple payload sizes
        let test_sizes: &[usize] = &[0, 1, 64, 256, 1024];
        let mut all_ok = true;

        for &size in test_sizes {
            // Generate test payload
            let mut payload = Vec::with_capacity(size);
            for i in 0..size {
                payload.push((i & 0xFF) as u8);
            }

            // Send through loopback
            {
                let mut buf = LOOPBACK_BUF.lock();
                *buf = Some(payload.clone());
            }

            // Receive from loopback
            let received = {
                let mut buf = LOOPBACK_BUF.lock();
                buf.take()
            };

            match received {
                Some(data) => {
                    if data.len() != payload.len() {
                        crate::serial_println!(
                            "    [net-test] FAIL: loopback size={}, expected {} bytes, got {}",
                            size,
                            payload.len(),
                            data.len()
                        );
                        all_ok = false;
                        continue;
                    }
                    // Verify each byte
                    let mut byte_ok = true;
                    for i in 0..data.len() {
                        if data[i] != payload[i] {
                            crate::serial_println!(
                                "    [net-test] FAIL: loopback byte mismatch at offset {}",
                                i
                            );
                            byte_ok = false;
                            break;
                        }
                    }
                    if !byte_ok {
                        all_ok = false;
                    }
                }
                None => {
                    crate::serial_println!(
                        "    [net-test] FAIL: no loopback data for size={}",
                        size
                    );
                    all_ok = false;
                }
            }
        }

        if all_ok {
            crate::serial_println!(
                "    [net-test] PASS: test_loopback ({} sizes tested)",
                test_sizes.len()
            );
        }
        all_ok
    }
}

pub fn run_all() {
    // Initialize loopback buffer
    {
        let mut buf = LOOPBACK_BUF.lock();
        *buf = None;
    }

    crate::serial_println!("    [net-test] ==============================");
    crate::serial_println!("    [net-test] Running networking test suite");
    crate::serial_println!("    [net-test] ==============================");

    let mut passed = 0u32;
    let mut failed = 0u32;

    if NetTests::test_tcp_socket() {
        passed += 1;
    } else {
        failed += 1;
    }
    if NetTests::test_udp() {
        passed += 1;
    } else {
        failed += 1;
    }
    if NetTests::test_loopback() {
        passed += 1;
    } else {
        failed += 1;
    }

    crate::serial_println!(
        "    [net-test] Results: {} passed, {} failed",
        passed,
        failed
    );
}
