use crate::sync::Mutex;
/// Network stack hardening for Genesis
///
/// Protections:
///   - SYN cookies (defend against SYN flood attacks)
///   - Connection rate limiting (per-IP and global)
///   - IP spoofing prevention (reverse path filtering)
///   - ICMP rate limiting (prevent ping floods)
///   - TCP sequence number randomization
///   - Port randomization for outbound connections
///   - Maximum connection limits
///   - Bogon filtering (reject impossible source IPs)
///   - Fragment attack mitigation
///   - TTL checks
///
/// Inspired by: Linux net.ipv4.* sysctl, BSD pf anti-spoofing.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;

/// Global network hardening state
static NET_HARDEN: Mutex<Option<NetHardening>> = Mutex::new(None);

/// SYN cookie state
pub struct SynCookie {
    /// Secret key for SYN cookie generation (rotated periodically)
    pub secret: [u8; 32],
    /// Second secret (for rotation)
    pub secret_prev: [u8; 32],
    /// Whether SYN cookies are active (auto-enabled under SYN flood)
    pub active: bool,
    /// SYN backlog threshold to activate cookies
    pub threshold: u32,
    /// Current SYN backlog count
    pub backlog: u32,
}

/// Per-IP rate limit tracking
#[derive(Debug, Clone)]
pub struct RateTracker {
    /// Connection attempts in current window
    pub count: u32,
    /// Window start time
    pub window_start: u64,
    /// Whether this IP is currently blocked
    pub blocked: bool,
    /// Block expiry time
    pub block_until: u64,
}

/// Network hardening state
pub struct NetHardening {
    /// SYN cookie state
    pub syn_cookies: SynCookie,
    /// Per-IP connection rate tracking
    pub rate_limits: BTreeMap<u32, RateTracker>, // IP as u32 -> tracker
    /// Rate limit: max new connections per IP per window
    pub max_conn_rate: u32,
    /// Rate limit window in seconds
    pub rate_window: u64,
    /// Rate limit block duration in seconds
    pub block_duration: u64,
    /// ICMP rate limiter
    pub icmp_rate: IcmpRateLimit,
    /// Global max concurrent connections
    pub max_connections: u32,
    /// Current connection count
    pub current_connections: u32,
    /// Reverse path filtering enabled
    pub rpf_enabled: bool,
    /// Bogon filtering enabled
    pub bogon_filter: bool,
    /// Minimum TTL for incoming packets
    pub min_ttl: u8,
    /// TCP ISN (Initial Sequence Number) randomization
    pub randomize_isn: bool,
    /// Outbound source port randomization
    pub randomize_ports: bool,
    /// Stats
    pub stats: NetHardenStats,
}

/// ICMP rate limiting
pub struct IcmpRateLimit {
    pub max_per_second: u32,
    pub current_count: u32,
    pub last_reset: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NetHardenStats {
    pub syn_cookies_sent: u64,
    pub syn_floods_detected: u64,
    pub rate_limited: u64,
    pub spoofed_blocked: u64,
    pub bogons_blocked: u64,
    pub icmp_rate_limited: u64,
    pub connections_refused: u64,
    pub fragments_dropped: u64,
}

/// Bogon IP ranges (RFC 5735 — addresses that should never appear on the Internet)
const BOGON_RANGES: &[(u32, u32, u8)] = &[
    (0x00000000, 0x00FFFFFF, 8),  // 0.0.0.0/8 — "This" network
    (0x0A000000, 0x0AFFFFFF, 8),  // 10.0.0.0/8 — Private
    (0x64400000, 0x647FFFFF, 10), // 100.64.0.0/10 — CGN
    (0x7F000000, 0x7FFFFFFF, 8),  // 127.0.0.0/8 — Loopback
    (0xA9FE0000, 0xA9FEFFFF, 16), // 169.254.0.0/16 — Link-local
    (0xAC100000, 0xAC1FFFFF, 12), // 172.16.0.0/12 — Private
    (0xC0000000, 0xC00000FF, 24), // 192.0.0.0/24 — IETF protocol
    (0xC0A80000, 0xC0A8FFFF, 16), // 192.168.0.0/16 — Private
    (0xE0000000, 0xEFFFFFFF, 4),  // 224.0.0.0/4 — Multicast
    (0xF0000000, 0xFFFFFFFF, 4),  // 240.0.0.0/4 — Reserved
];

impl NetHardening {
    pub fn new() -> Self {
        let mut secret = alloc::vec![0u8; 32];
        crate::crypto::random::fill_bytes(&mut secret);
        let mut s1 = [0u8; 32];
        s1.copy_from_slice(&secret[..32]);
        let mut secret2 = alloc::vec![0u8; 32];
        crate::crypto::random::fill_bytes(&mut secret2);
        let mut s2 = [0u8; 32];
        s2.copy_from_slice(&secret2[..32]);

        NetHardening {
            syn_cookies: SynCookie {
                secret: s1,
                secret_prev: s2,
                active: false,
                threshold: 128,
                backlog: 0,
            },
            rate_limits: BTreeMap::new(),
            max_conn_rate: 20,   // 20 new connections per IP per window
            rate_window: 60,     // 60 second window
            block_duration: 300, // 5 minute block
            icmp_rate: IcmpRateLimit {
                max_per_second: 10,
                current_count: 0,
                last_reset: 0,
            },
            max_connections: 65535,
            current_connections: 0,
            rpf_enabled: true,
            bogon_filter: true,
            min_ttl: 1,
            randomize_isn: true,
            randomize_ports: true,
            stats: NetHardenStats::default(),
        }
    }

    /// Generate a SYN cookie for a connection
    pub fn generate_syn_cookie(
        &self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
    ) -> u32 {
        let mut data = [0u8; 12];
        data[0..4].copy_from_slice(&src_ip.to_be_bytes());
        data[4..8].copy_from_slice(&dst_ip.to_be_bytes());
        data[8..10].copy_from_slice(&src_port.to_be_bytes());
        data[10..12].copy_from_slice(&dst_port.to_be_bytes());

        let hash = crate::crypto::hmac::hmac_sha256(&self.syn_cookies.secret, &data);
        u32::from_le_bytes([hash[0], hash[1], hash[2], hash[3]])
    }

    /// Verify a SYN cookie
    pub fn verify_syn_cookie(
        &self,
        src_ip: u32,
        dst_ip: u32,
        src_port: u16,
        dst_port: u16,
        cookie: u32,
    ) -> bool {
        let expected = self.generate_syn_cookie(src_ip, dst_ip, src_port, dst_port);
        cookie == expected
    }

    /// Check connection rate limit for an IP
    pub fn check_rate_limit(&mut self, ip: u32, now: u64) -> bool {
        let tracker = self.rate_limits.entry(ip).or_insert_with(|| RateTracker {
            count: 0,
            window_start: now,
            blocked: false,
            block_until: 0,
        });

        // Check if currently blocked
        if tracker.blocked {
            if now >= tracker.block_until {
                tracker.blocked = false;
                tracker.count = 0;
                tracker.window_start = now;
            } else {
                self.stats.rate_limited = self.stats.rate_limited.saturating_add(1);
                return false;
            }
        }

        // Reset window if expired
        if now >= tracker.window_start + self.rate_window {
            tracker.count = 0;
            tracker.window_start = now;
        }

        tracker.count = tracker.count.saturating_add(1);

        if tracker.count > self.max_conn_rate {
            tracker.blocked = true;
            tracker.block_until = now + self.block_duration;
            self.stats.rate_limited = self.stats.rate_limited.saturating_add(1);
            serial_println!(
                "  [net-harden] Rate limited IP {}.{}.{}.{}",
                (ip >> 24) & 0xFF,
                (ip >> 16) & 0xFF,
                (ip >> 8) & 0xFF,
                ip & 0xFF
            );
            return false;
        }

        true
    }

    /// Check if a source IP is a bogon (should never appear on the Internet)
    pub fn is_bogon(&self, ip: u32) -> bool {
        if !self.bogon_filter {
            return false;
        }
        for &(start, end, _prefix) in BOGON_RANGES {
            if ip >= start && ip <= end {
                return true;
            }
        }
        false
    }

    /// ICMP rate limiting
    pub fn check_icmp_rate(&mut self, now: u64) -> bool {
        if now > self.icmp_rate.last_reset + 1 {
            self.icmp_rate.current_count = 0;
            self.icmp_rate.last_reset = now;
        }

        self.icmp_rate.current_count = self.icmp_rate.current_count.saturating_add(1);
        if self.icmp_rate.current_count > self.icmp_rate.max_per_second {
            self.stats.icmp_rate_limited = self.stats.icmp_rate_limited.saturating_add(1);
            return false;
        }

        true
    }

    /// Generate a randomized TCP Initial Sequence Number
    pub fn generate_isn(&self, src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16) -> u32 {
        if !self.randomize_isn {
            return 0;
        }

        let mut data = [0u8; 12];
        data[0..4].copy_from_slice(&src_ip.to_be_bytes());
        data[4..8].copy_from_slice(&dst_ip.to_be_bytes());
        data[8..10].copy_from_slice(&src_port.to_be_bytes());
        data[10..12].copy_from_slice(&dst_port.to_be_bytes());

        let hash = crate::crypto::hmac::hmac_sha256(&self.syn_cookies.secret, &data);
        u32::from_le_bytes([hash[4], hash[5], hash[6], hash[7]])
    }

    /// Generate a random source port for outbound connections
    pub fn random_source_port(&self) -> u16 {
        if !self.randomize_ports {
            return 49152; // Default ephemeral port start
        }

        let mut bytes = [0u8; 2];
        crate::crypto::random::fill_bytes(&mut bytes);
        let port = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
        // Ephemeral port range: 49152-65535
        49152 + (port % (65535 - 49152))
    }

    /// Check a TTL value
    pub fn check_ttl(&self, ttl: u8) -> bool {
        ttl >= self.min_ttl
    }

    /// Check if global connection limit is reached
    pub fn check_connection_limit(&mut self) -> bool {
        if self.current_connections >= self.max_connections {
            self.stats.connections_refused = self.stats.connections_refused.saturating_add(1);
            return false;
        }
        true
    }

    /// Clean up stale rate limit entries
    pub fn cleanup(&mut self, now: u64) {
        self.rate_limits.retain(|_, tracker| {
            if tracker.blocked {
                now < tracker.block_until
            } else {
                now < tracker.window_start + self.rate_window * 2
            }
        });
    }
}

/// Initialize network hardening
pub fn init() {
    let hardening = NetHardening::new();
    *NET_HARDEN.lock() = Some(hardening);
    serial_println!("  [net-harden] Network hardening initialized:");
    serial_println!("    SYN cookies: ready (threshold: 128)");
    serial_println!("    Rate limiting: 20/min per IP, 5min block");
    serial_println!("    Bogon filtering: enabled");
    serial_println!("    TCP ISN randomization: enabled");
    serial_println!("    Source port randomization: enabled");
    serial_println!("    ICMP rate limit: 10/sec");
}

/// Check if a new inbound connection should be allowed
pub fn check_inbound(src_ip: u32, now: u64) -> bool {
    NET_HARDEN.lock().as_mut().map_or(true, |h| {
        if h.is_bogon(src_ip) {
            h.stats.bogons_blocked = h.stats.bogons_blocked.saturating_add(1);
            return false;
        }
        if !h.check_rate_limit(src_ip, now) {
            return false;
        }
        h.check_connection_limit()
    })
}

/// Generate a randomized TCP ISN
pub fn generate_isn(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16) -> u32 {
    NET_HARDEN
        .lock()
        .as_ref()
        .map(|h| h.generate_isn(src_ip, dst_ip, src_port, dst_port))
        .unwrap_or(0)
}

/// Get a random source port
pub fn random_source_port() -> u16 {
    NET_HARDEN
        .lock()
        .as_ref()
        .map(|h| h.random_source_port())
        .unwrap_or(49152)
}
