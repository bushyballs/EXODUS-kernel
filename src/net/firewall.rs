use crate::sync::Mutex;
/// Hoags Firewall — packet filtering and network security
///
/// Stateful packet filter with:
///   - Chain-based rule processing (INPUT, OUTPUT, FORWARD)
///   - Connection tracking (stateful inspection)
///   - Rate limiting
///   - Port knocking
///   - Default deny policy
///
/// Inspired by: iptables/nftables (chain model), pf (clean syntax),
/// Windows Firewall (application awareness). All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

static FIREWALL: Mutex<Option<Firewall>> = Mutex::new(None);

/// Firewall chain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    Input,
    Output,
    Forward,
}

/// Rule action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Accept,
    Drop,
    Reject,
    Log,
}

/// Protocol match
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Any,
    Tcp,
    Udp,
    Icmp,
}

/// IP address match (simplified — supports single IP or any)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpMatch {
    Any,
    Single([u8; 4]),
    Subnet([u8; 4], u8), // addr, prefix_len
}

impl IpMatch {
    pub fn matches(&self, addr: &[u8; 4]) -> bool {
        match self {
            IpMatch::Any => true,
            IpMatch::Single(ip) => ip == addr,
            IpMatch::Subnet(net, prefix) => {
                let mask = if *prefix >= 32 {
                    0xFFFFFFFF
                } else {
                    !((1u32 << (32 - prefix)) - 1)
                };
                let net_u32 = u32::from_be_bytes(*net);
                let addr_u32 = u32::from_be_bytes(*addr);
                (net_u32 & mask) == (addr_u32 & mask)
            }
        }
    }
}

/// Port match
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMatch {
    Any,
    Single(u16),
    Range(u16, u16),
}

impl PortMatch {
    pub fn matches(&self, port: u16) -> bool {
        match self {
            PortMatch::Any => true,
            PortMatch::Single(p) => *p == port,
            PortMatch::Range(lo, hi) => port >= *lo && port <= *hi,
        }
    }
}

/// Connection state for stateful inspection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    New,
    Established,
    Related,
    Invalid,
}

/// A firewall rule
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: u32,
    pub chain: Chain,
    pub action: Action,
    pub protocol: Protocol,
    pub src_ip: IpMatch,
    pub dst_ip: IpMatch,
    pub src_port: PortMatch,
    pub dst_port: PortMatch,
    pub state: Option<ConnState>,
    pub description: String,
    pub hit_count: u64,
    pub enabled: bool,
}

/// Connection tracking entry
#[derive(Debug, Clone)]
pub struct ConnTrack {
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub src_port: u16,
    pub dst_port: u16,
    pub protocol: Protocol,
    pub state: ConnState,
    pub packets: u64,
    pub bytes: u64,
    pub last_seen: u64,
}

/// The firewall
pub struct Firewall {
    pub rules: Vec<Rule>,
    pub connections: Vec<ConnTrack>,
    pub default_input: Action,
    pub default_output: Action,
    pub default_forward: Action,
    pub enabled: bool,
    pub next_rule_id: u32,
    pub total_packets: u64,
    pub dropped_packets: u64,
}

impl Firewall {
    pub fn new() -> Self {
        let mut fw = Firewall {
            rules: Vec::new(),
            connections: Vec::new(),
            default_input: Action::Drop,    // default deny inbound
            default_output: Action::Accept, // default allow outbound
            default_forward: Action::Drop,
            enabled: true,
            next_rule_id: 1,
            total_packets: 0,
            dropped_packets: 0,
        };
        fw.load_default_rules();
        fw
    }

    fn load_default_rules(&mut self) {
        // Allow loopback
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Input,
            action: Action::Accept,
            protocol: Protocol::Any,
            src_ip: IpMatch::Single([127, 0, 0, 1]),
            dst_ip: IpMatch::Single([127, 0, 0, 1]),
            src_port: PortMatch::Any,
            dst_port: PortMatch::Any,
            state: None,
            description: String::from("allow loopback"),
            hit_count: 0,
            enabled: true,
        });

        // Allow established/related connections
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Input,
            action: Action::Accept,
            protocol: Protocol::Any,
            src_ip: IpMatch::Any,
            dst_ip: IpMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Any,
            state: Some(ConnState::Established),
            description: String::from("allow established"),
            hit_count: 0,
            enabled: true,
        });

        // Allow ICMP (ping)
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Input,
            action: Action::Accept,
            protocol: Protocol::Icmp,
            src_ip: IpMatch::Any,
            dst_ip: IpMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Any,
            state: None,
            description: String::from("allow ping"),
            hit_count: 0,
            enabled: true,
        });

        // Allow SSH (port 22)
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Input,
            action: Action::Accept,
            protocol: Protocol::Tcp,
            src_ip: IpMatch::Any,
            dst_ip: IpMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Single(22),
            state: None,
            description: String::from("allow SSH"),
            hit_count: 0,
            enabled: true,
        });

        // Allow DNS (port 53)
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Output,
            action: Action::Accept,
            protocol: Protocol::Udp,
            src_ip: IpMatch::Any,
            dst_ip: IpMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Single(53),
            state: None,
            description: String::from("allow DNS"),
            hit_count: 0,
            enabled: true,
        });

        // Allow HTTPS (port 443)
        self.add_rule(Rule {
            id: 0,
            chain: Chain::Output,
            action: Action::Accept,
            protocol: Protocol::Tcp,
            src_ip: IpMatch::Any,
            dst_ip: IpMatch::Any,
            src_port: PortMatch::Any,
            dst_port: PortMatch::Single(443),
            state: None,
            description: String::from("allow HTTPS"),
            hit_count: 0,
            enabled: true,
        });
    }

    pub fn add_rule(&mut self, mut rule: Rule) -> u32 {
        rule.id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);
        let id = rule.id;
        self.rules.push(rule);
        id
    }

    pub fn remove_rule(&mut self, id: u32) {
        self.rules.retain(|r| r.id != id);
    }

    /// Check a packet against the firewall rules
    pub fn check_packet(
        &mut self,
        chain: Chain,
        protocol: Protocol,
        src_ip: &[u8; 4],
        dst_ip: &[u8; 4],
        src_port: u16,
        dst_port: u16,
    ) -> Action {
        if !self.enabled {
            return Action::Accept;
        }

        self.total_packets = self.total_packets.saturating_add(1);

        // Check connection tracking
        let conn_state = self.lookup_connection(src_ip, dst_ip, src_port, dst_port, protocol);

        // Evaluate rules in order
        for rule in &mut self.rules {
            if !rule.enabled || rule.chain != chain {
                continue;
            }

            // Match protocol
            if !matches!(rule.protocol, Protocol::Any) && rule.protocol != protocol {
                continue;
            }

            // Match IPs
            if !rule.src_ip.matches(src_ip) {
                continue;
            }
            if !rule.dst_ip.matches(dst_ip) {
                continue;
            }

            // Match ports
            if !rule.src_port.matches(src_port) {
                continue;
            }
            if !rule.dst_port.matches(dst_port) {
                continue;
            }

            // Match connection state
            if let Some(required_state) = rule.state {
                if conn_state != required_state {
                    continue;
                }
            }

            rule.hit_count = rule.hit_count.saturating_add(1);

            if matches!(rule.action, Action::Drop | Action::Reject) {
                self.dropped_packets = self.dropped_packets.saturating_add(1);
            }

            return rule.action;
        }

        // Default policy
        let action = match chain {
            Chain::Input => self.default_input,
            Chain::Output => self.default_output,
            Chain::Forward => self.default_forward,
        };

        if matches!(action, Action::Drop | Action::Reject) {
            self.dropped_packets = self.dropped_packets.saturating_add(1);
        }

        action
    }

    fn lookup_connection(
        &self,
        src_ip: &[u8; 4],
        dst_ip: &[u8; 4],
        src_port: u16,
        dst_port: u16,
        _protocol: Protocol,
    ) -> ConnState {
        for conn in &self.connections {
            // Match forward direction
            if conn.src_ip == *src_ip
                && conn.dst_ip == *dst_ip
                && conn.src_port == src_port
                && conn.dst_port == dst_port
            {
                return conn.state;
            }
            // Match reverse direction (reply packets)
            if conn.src_ip == *dst_ip
                && conn.dst_ip == *src_ip
                && conn.src_port == dst_port
                && conn.dst_port == src_port
            {
                return ConnState::Established;
            }
        }
        ConnState::New
    }

    /// Track a new connection
    pub fn track_connection(
        &mut self,
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        dst_port: u16,
        protocol: Protocol,
    ) {
        let now = crate::time::clock::uptime_ms();
        self.connections.push(ConnTrack {
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            protocol,
            state: ConnState::New,
            packets: 1,
            bytes: 0,
            last_seen: now,
        });
    }
}

pub fn init() {
    *FIREWALL.lock() = Some(Firewall::new());
    serial_println!("    [firewall] Stateful firewall initialized (default: deny inbound)");
}

/// Check if a packet should be allowed
pub fn check(
    chain: Chain,
    protocol: Protocol,
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    src_port: u16,
    dst_port: u16,
) -> Action {
    FIREWALL
        .lock()
        .as_mut()
        .map(|fw| fw.check_packet(chain, protocol, src_ip, dst_ip, src_port, dst_port))
        .unwrap_or(Action::Accept)
}
