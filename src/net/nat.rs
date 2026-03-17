use crate::sync::Mutex;
/// Network Address Translation (NAT)
///
/// Source and destination NAT with connection tracking.
/// Supports SNAT, DNAT, masquerade, and port forwarding.
/// Connection tracking maintains state for bidirectional translation.
///
/// Inspired by: Linux netfilter conntrack/NAT, BSD pf NAT.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum connection tracking entries
const MAX_CONNTRACK_ENTRIES: usize = 8192;

/// Connection timeout (ticks) for established TCP
const TCP_ESTABLISHED_TIMEOUT: u64 = 432000; // ~12 hours at 100ms/tick

/// Connection timeout for UDP
const UDP_TIMEOUT: u64 = 1800; // ~3 minutes

/// Connection timeout for ICMP
const ICMP_TIMEOUT: u64 = 300; // ~30 seconds

/// Connection timeout for other protocols
const GENERIC_TIMEOUT: u64 = 600;

/// Starting ephemeral port for masquerade
const EPHEMERAL_PORT_START: u16 = 49152;

/// Ending ephemeral port
const EPHEMERAL_PORT_END: u16 = 65535;

// ---------------------------------------------------------------------------
// Protocol numbers
// ---------------------------------------------------------------------------
const PROTO_ICMP: u8 = 1;
const PROTO_TCP: u8 = 6;
const PROTO_UDP: u8 = 17;

// ---------------------------------------------------------------------------
// NAT types
// ---------------------------------------------------------------------------

/// NAT mapping type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NatType {
    /// Source NAT: rewrite source IP/port
    Snat,
    /// Destination NAT: rewrite destination IP/port
    Dnat,
    /// Masquerade: SNAT using the outgoing interface IP
    Masquerade,
}

// ---------------------------------------------------------------------------
// Connection tracking
// ---------------------------------------------------------------------------

/// Connection tracking state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    New,
    Established,
    Related,
    TimeWait,
    Closing,
}

/// Connection tracking tuple (identifies one direction of a flow)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnTuple {
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: u8,
}

impl ConnTuple {
    /// Create the reverse tuple (swap src/dst)
    pub fn reverse(&self) -> Self {
        ConnTuple {
            src_ip: self.dst_ip,
            dst_ip: self.src_ip,
            src_port: self.dst_port,
            dst_port: self.src_port,
            protocol: self.protocol,
        }
    }
}

/// A connection tracking entry
#[derive(Debug, Clone)]
pub struct ConntrackEntry {
    /// Original direction tuple
    pub original: ConnTuple,
    /// Reply direction tuple (after NAT translation)
    pub reply: ConnTuple,
    /// Connection state
    pub state: ConnState,
    /// NAT type applied
    pub nat_type: Option<NatType>,
    /// Translated source IP (for SNAT/masquerade)
    pub nat_src_ip: Option<[u8; 4]>,
    pub nat_src_port: Option<u16>,
    /// Translated destination IP (for DNAT)
    pub nat_dst_ip: Option<[u8; 4]>,
    pub nat_dst_port: Option<u16>,
    /// Packet counters
    pub packets_orig: u64,
    pub bytes_orig: u64,
    pub packets_reply: u64,
    pub bytes_reply: u64,
    /// Last activity tick
    pub last_seen: u64,
    /// Timeout value
    pub timeout: u64,
    /// Mark value
    pub mark: u32,
}

impl ConntrackEntry {
    fn is_expired(&self, current_tick: u64) -> bool {
        current_tick.wrapping_sub(self.last_seen) > self.timeout
    }

    fn timeout_for_protocol(protocol: u8, state: ConnState) -> u64 {
        match protocol {
            PROTO_TCP => match state {
                ConnState::Established => TCP_ESTABLISHED_TIMEOUT,
                ConnState::TimeWait | ConnState::Closing => 1200,
                _ => 1200,
            },
            PROTO_UDP => UDP_TIMEOUT,
            PROTO_ICMP => ICMP_TIMEOUT,
            _ => GENERIC_TIMEOUT,
        }
    }
}

// ---------------------------------------------------------------------------
// NAT rules
// ---------------------------------------------------------------------------

/// A NAT rule
#[derive(Debug, Clone)]
pub struct NatRule {
    /// Which packets to match (source IP/mask)
    pub match_src_ip: [u8; 4],
    pub match_src_mask: [u8; 4],
    /// Match destination (for DNAT rules)
    pub match_dst_ip: [u8; 4],
    pub match_dst_mask: [u8; 4],
    /// Match protocol (0 = any)
    pub match_protocol: u8,
    /// Match destination port (0 = any, for DNAT)
    pub match_dst_port: u16,
    /// NAT type
    pub nat_type: NatType,
    /// Translation target IP
    pub target_ip: [u8; 4],
    /// Translation target port (0 = keep original)
    pub target_port: u16,
    /// Interface constraint (outgoing, for masquerade)
    pub out_iface: Option<String>,
    /// Whether this rule is enabled
    pub enabled: bool,
    /// Counters
    pub packet_count: u64,
    pub byte_count: u64,
}

impl NatRule {
    fn ip_matches(ip: [u8; 4], rule_ip: [u8; 4], mask: [u8; 4]) -> bool {
        for i in 0..4 {
            if (ip[i] & mask[i]) != (rule_ip[i] & mask[i]) {
                return false;
            }
        }
        true
    }

    fn matches_packet(
        &self,
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        protocol: u8,
        dst_port: u16,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        if !Self::ip_matches(src_ip, self.match_src_ip, self.match_src_mask) {
            return false;
        }
        if !Self::ip_matches(dst_ip, self.match_dst_ip, self.match_dst_mask) {
            return false;
        }
        if self.match_protocol != 0 && self.match_protocol != protocol {
            return false;
        }
        if self.match_dst_port != 0 && self.match_dst_port != dst_port {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// NAT table (inner state)
// ---------------------------------------------------------------------------

struct NatInner {
    /// SNAT/masquerade rules
    snat_rules: Vec<NatRule>,
    /// DNAT/port-forwarding rules
    dnat_rules: Vec<NatRule>,
    /// Connection tracking table
    conntrack: Vec<ConntrackEntry>,
    /// Current tick
    tick: u64,
    /// Next ephemeral port to allocate
    next_ephemeral: u16,
    /// Masquerade interface IP (set dynamically)
    masquerade_ip: [u8; 4],
}

static NAT: Mutex<Option<NatInner>> = Mutex::new(None);

/// Initialize the NAT subsystem
pub fn init() {
    *NAT.lock() = Some(NatInner {
        snat_rules: Vec::new(),
        dnat_rules: Vec::new(),
        conntrack: Vec::new(),
        tick: 0,
        next_ephemeral: EPHEMERAL_PORT_START,
        masquerade_ip: [0; 4],
    });
    serial_println!("  Net: NAT/conntrack subsystem initialized");
}

// ---------------------------------------------------------------------------
// Connection tracking API
// ---------------------------------------------------------------------------

/// Look up a connection tracking entry by original tuple
pub fn conntrack_lookup(tuple: &ConnTuple) -> Option<ConntrackEntry> {
    let guard = NAT.lock();
    let inner = guard.as_ref()?;
    inner
        .conntrack
        .iter()
        .find(|e| e.original == *tuple || e.reply == *tuple)
        .cloned()
}

/// Get all active connections
pub fn conntrack_list() -> Vec<ConntrackEntry> {
    let guard = NAT.lock();
    match guard.as_ref() {
        Some(inner) => inner.conntrack.clone(),
        None => Vec::new(),
    }
}

/// Flush all connection tracking entries
pub fn conntrack_flush() {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.conntrack.clear();
        serial_println!("  NAT: conntrack table flushed");
    }
}

/// Get conntrack table size
pub fn conntrack_count() -> usize {
    let guard = NAT.lock();
    match guard.as_ref() {
        Some(inner) => inner.conntrack.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// NAT rule management
// ---------------------------------------------------------------------------

/// Add an SNAT rule
pub fn add_snat(match_src_ip: [u8; 4], match_src_mask: [u8; 4], target_ip: [u8; 4]) {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.snat_rules.push(NatRule {
            match_src_ip,
            match_src_mask,
            match_dst_ip: [0; 4],
            match_dst_mask: [0; 4],
            match_protocol: 0,
            match_dst_port: 0,
            nat_type: NatType::Snat,
            target_ip,
            target_port: 0,
            out_iface: None,
            enabled: true,
            packet_count: 0,
            byte_count: 0,
        });
        serial_println!(
            "  NAT: added SNAT rule -> {}.{}.{}.{}",
            target_ip[0],
            target_ip[1],
            target_ip[2],
            target_ip[3]
        );
    }
}

/// Add a DNAT (port forwarding) rule
pub fn add_dnat(match_dst_port: u16, target_ip: [u8; 4], target_port: u16, protocol: u8) {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.dnat_rules.push(NatRule {
            match_src_ip: [0; 4],
            match_src_mask: [0; 4],
            match_dst_ip: [0; 4],
            match_dst_mask: [0; 4],
            match_protocol: protocol,
            match_dst_port: match_dst_port,
            nat_type: NatType::Dnat,
            target_ip,
            target_port,
            out_iface: None,
            enabled: true,
            packet_count: 0,
            byte_count: 0,
        });
        serial_println!(
            "  NAT: added DNAT rule port {} -> {}.{}.{}.{}:{}",
            match_dst_port,
            target_ip[0],
            target_ip[1],
            target_ip[2],
            target_ip[3],
            target_port
        );
    }
}

/// Add a masquerade rule
pub fn add_masquerade(match_src_ip: [u8; 4], match_src_mask: [u8; 4], out_iface: &str) {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.snat_rules.push(NatRule {
            match_src_ip,
            match_src_mask,
            match_dst_ip: [0; 4],
            match_dst_mask: [0; 4],
            match_protocol: 0,
            match_dst_port: 0,
            nat_type: NatType::Masquerade,
            target_ip: [0; 4], // filled dynamically
            target_port: 0,
            out_iface: Some(String::from(out_iface)),
            enabled: true,
            packet_count: 0,
            byte_count: 0,
        });
        serial_println!("  NAT: added masquerade on {}", out_iface);
    }
}

/// Set the masquerade IP (called when interface IP changes)
pub fn set_masquerade_ip(ip: [u8; 4]) {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.masquerade_ip = ip;
    }
}

/// Remove all NAT rules
pub fn flush_rules() {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        inner.snat_rules.clear();
        inner.dnat_rules.clear();
        serial_println!("  NAT: all rules flushed");
    }
}

/// List all NAT rules
pub fn list_rules() -> Vec<(NatType, String)> {
    let guard = NAT.lock();
    let inner = match guard.as_ref() {
        Some(i) => i,
        None => return Vec::new(),
    };

    let mut result = Vec::new();
    for rule in &inner.snat_rules {
        let desc = alloc::format!(
            "{:?} src={}.{}.{}.{}/{}.{}.{}.{} -> {}.{}.{}.{} pkts={}",
            rule.nat_type,
            rule.match_src_ip[0],
            rule.match_src_ip[1],
            rule.match_src_ip[2],
            rule.match_src_ip[3],
            rule.match_src_mask[0],
            rule.match_src_mask[1],
            rule.match_src_mask[2],
            rule.match_src_mask[3],
            rule.target_ip[0],
            rule.target_ip[1],
            rule.target_ip[2],
            rule.target_ip[3],
            rule.packet_count
        );
        result.push((rule.nat_type, desc));
    }
    for rule in &inner.dnat_rules {
        let desc = alloc::format!(
            "DNAT port {} -> {}.{}.{}.{}:{} pkts={}",
            rule.match_dst_port,
            rule.target_ip[0],
            rule.target_ip[1],
            rule.target_ip[2],
            rule.target_ip[3],
            rule.target_port,
            rule.packet_count
        );
        result.push((rule.nat_type, desc));
    }
    result
}

// ---------------------------------------------------------------------------
// Packet translation
// ---------------------------------------------------------------------------

/// Allocate an ephemeral port
fn alloc_ephemeral_port(inner: &mut NatInner) -> u16 {
    let port = inner.next_ephemeral;
    inner.next_ephemeral = inner.next_ephemeral.saturating_add(1);
    if inner.next_ephemeral > EPHEMERAL_PORT_END {
        inner.next_ephemeral = EPHEMERAL_PORT_START;
    }
    port
}

/// Translate an outgoing packet (SNAT/masquerade direction)
///
/// Modifies the packet in place if NAT applies. Returns the original
/// source IP/port so the caller can update checksums.
pub fn translate_outgoing(
    _packet: &mut [u8],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    src_port: u16,
    dst_port: u16,
    packet_len: usize,
) -> Option<NatTranslation> {
    let mut guard = NAT.lock();
    let inner = guard.as_mut()?;
    inner.tick = inner.tick.saturating_add(1);

    // Check if there's already a conntrack entry
    let orig_tuple = ConnTuple {
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        protocol,
    };

    if let Some(ct_idx) = inner
        .conntrack
        .iter()
        .position(|e| e.original == orig_tuple)
    {
        let ct = &mut inner.conntrack[ct_idx];
        ct.last_seen = inner.tick;
        ct.packets_orig = ct.packets_orig.saturating_add(1);
        ct.bytes_orig = ct.bytes_orig.saturating_add(packet_len as u64);
        ct.state = ConnState::Established;

        return match (ct.nat_src_ip, ct.nat_src_port) {
            (Some(new_ip), Some(new_port)) => Some(NatTranslation {
                nat_type: ct.nat_type.unwrap_or(NatType::Snat),
                original_ip: src_ip,
                original_port: src_port,
                translated_ip: new_ip,
                translated_port: new_port,
            }),
            _ => None,
        };
    }

    // Find a matching SNAT rule
    let masquerade_ip = inner.masquerade_ip;
    let mut matched_rule_idx = None;
    for (idx, rule) in inner.snat_rules.iter().enumerate() {
        if rule.matches_packet(src_ip, dst_ip, protocol, dst_port) {
            matched_rule_idx = Some(idx);
            break;
        }
    }

    if let Some(rule_idx) = matched_rule_idx {
        // Extract needed data from the rule before calling alloc_ephemeral_port
        let rule_nat_type = inner.snat_rules[rule_idx].nat_type;
        let rule_target_ip = inner.snat_rules[rule_idx].target_ip;
        let rule_target_port = inner.snat_rules[rule_idx].target_port;

        let target_ip = if rule_nat_type == NatType::Masquerade {
            masquerade_ip
        } else {
            rule_target_ip
        };

        let target_port = if rule_target_port != 0 {
            rule_target_port
        } else {
            alloc_ephemeral_port(inner)
        };

        inner.snat_rules[rule_idx].packet_count =
            inner.snat_rules[rule_idx].packet_count.saturating_add(1);
        inner.snat_rules[rule_idx].byte_count = inner.snat_rules[rule_idx]
            .byte_count
            .saturating_add(packet_len as u64);
        let nat_type = rule_nat_type;

        // Create conntrack entry
        let reply_tuple = ConnTuple {
            src_ip: dst_ip,
            dst_ip: target_ip,
            src_port: dst_port,
            dst_port: target_port,
            protocol,
        };

        if inner.conntrack.len() < MAX_CONNTRACK_ENTRIES {
            inner.conntrack.push(ConntrackEntry {
                original: orig_tuple,
                reply: reply_tuple,
                state: ConnState::New,
                nat_type: Some(nat_type),
                nat_src_ip: Some(target_ip),
                nat_src_port: Some(target_port),
                nat_dst_ip: None,
                nat_dst_port: None,
                packets_orig: 1,
                bytes_orig: packet_len as u64,
                packets_reply: 0,
                bytes_reply: 0,
                last_seen: inner.tick,
                timeout: ConntrackEntry::timeout_for_protocol(protocol, ConnState::New),
                mark: 0,
            });
        }

        return Some(NatTranslation {
            nat_type,
            original_ip: src_ip,
            original_port: src_port,
            translated_ip: target_ip,
            translated_port: target_port,
        });
    }

    None
}

/// Translate an incoming packet (DNAT / reverse SNAT direction)
pub fn translate_incoming(
    _packet: &mut [u8],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    src_port: u16,
    dst_port: u16,
    packet_len: usize,
) -> Option<NatTranslation> {
    let mut guard = NAT.lock();
    let inner = guard.as_mut()?;

    // Check for existing conntrack entry (reply direction)
    let reply_tuple = ConnTuple {
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        protocol,
    };

    if let Some(ct_idx) = inner.conntrack.iter().position(|e| e.reply == reply_tuple) {
        let ct = &mut inner.conntrack[ct_idx];
        ct.last_seen = inner.tick;
        ct.packets_reply = ct.packets_reply.saturating_add(1);
        ct.bytes_reply = ct.bytes_reply.saturating_add(packet_len as u64);
        ct.state = ConnState::Established;

        // Reverse translate: change dst back to original src
        return Some(NatTranslation {
            nat_type: ct.nat_type.unwrap_or(NatType::Snat),
            original_ip: dst_ip,
            original_port: dst_port,
            translated_ip: ct.original.src_ip,
            translated_port: ct.original.src_port,
        });
    }

    // Check DNAT rules for new incoming connections
    for rule in &mut inner.dnat_rules {
        if rule.matches_packet(src_ip, dst_ip, protocol, dst_port) {
            rule.packet_count = rule.packet_count.saturating_add(1);
            rule.byte_count = rule.byte_count.saturating_add(packet_len as u64);

            let target_port = if rule.target_port != 0 {
                rule.target_port
            } else {
                dst_port
            };

            // Create conntrack entry for DNAT
            let orig_tuple = ConnTuple {
                src_ip,
                dst_ip,
                src_port,
                dst_port,
                protocol,
            };
            let reply_tuple = ConnTuple {
                src_ip: rule.target_ip,
                dst_ip: src_ip,
                src_port: target_port,
                dst_port: src_port,
                protocol,
            };

            if inner.conntrack.len() < MAX_CONNTRACK_ENTRIES {
                inner.conntrack.push(ConntrackEntry {
                    original: orig_tuple,
                    reply: reply_tuple,
                    state: ConnState::New,
                    nat_type: Some(NatType::Dnat),
                    nat_src_ip: None,
                    nat_src_port: None,
                    nat_dst_ip: Some(rule.target_ip),
                    nat_dst_port: Some(target_port),
                    packets_orig: 1,
                    bytes_orig: packet_len as u64,
                    packets_reply: 0,
                    bytes_reply: 0,
                    last_seen: inner.tick,
                    timeout: ConntrackEntry::timeout_for_protocol(protocol, ConnState::New),
                    mark: 0,
                });
            }

            return Some(NatTranslation {
                nat_type: NatType::Dnat,
                original_ip: dst_ip,
                original_port: dst_port,
                translated_ip: rule.target_ip,
                translated_port: target_port,
            });
        }
    }

    None
}

/// Result of a NAT translation
#[derive(Debug, Clone)]
pub struct NatTranslation {
    pub nat_type: NatType,
    pub original_ip: [u8; 4],
    pub original_port: u16,
    pub translated_ip: [u8; 4],
    pub translated_port: u16,
}

/// Expire old conntrack entries (call periodically)
pub fn conntrack_gc() {
    let mut guard = NAT.lock();
    if let Some(inner) = guard.as_mut() {
        let tick = inner.tick;
        let before = inner.conntrack.len();
        inner.conntrack.retain(|e| !e.is_expired(tick));
        let expired = before - inner.conntrack.len();
        if expired > 0 {
            serial_println!(
                "  NAT: GC expired {} conntrack entries ({} remaining)",
                expired,
                inner.conntrack.len()
            );
        }
    }
}
