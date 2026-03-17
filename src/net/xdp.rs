use crate::sync::Mutex;
/// eXpress Data Path — fast programmable packet processing
///
/// Provides early-stage packet processing at the NIC driver level,
/// before the full network stack. Programs can drop, pass, redirect,
/// or modify packets with minimal overhead. Includes XDP maps for
/// state sharing and per-CPU redirect targets.
///
/// Inspired by: Linux XDP/eBPF (net/core/filter.c), XDP tutorial.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum XDP programs per interface
const MAX_PROGRAMS_PER_IFACE: usize = 4;

/// Maximum total XDP programs
const MAX_PROGRAMS: usize = 64;

/// Maximum XDP maps
const MAX_MAPS: usize = 128;

/// Maximum map entries
const MAX_MAP_ENTRIES: usize = 65536;

/// XDP packet data maximum
const XDP_MAX_PACKET: usize = 9216; // jumbo frames

// ---------------------------------------------------------------------------
// XDP actions
// ---------------------------------------------------------------------------

/// XDP verdict for a processed packet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpAction {
    /// Pass the packet to the normal network stack
    Pass,
    /// Drop the packet
    Drop,
    /// Transmit the packet back out the same interface (bounce)
    Tx,
    /// Redirect to another interface or CPU
    Redirect,
    /// Program error — drop and log
    Aborted,
}

// ---------------------------------------------------------------------------
// XDP program modes
// ---------------------------------------------------------------------------

/// XDP attachment mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpMode {
    /// Native driver support (fastest)
    Native,
    /// Generic/SKB mode (works on all interfaces, slower)
    Generic,
    /// Hardware offload (NIC processes program)
    Offload,
}

// ---------------------------------------------------------------------------
// XDP metadata
// ---------------------------------------------------------------------------

/// Packet metadata available to XDP programs
#[derive(Debug, Clone)]
pub struct XdpMd {
    /// Pointer/index to packet data start
    pub data: usize,
    /// Pointer/index to packet data end
    pub data_end: usize,
    /// Ingress interface index
    pub ingress_ifindex: u32,
    /// RX queue index
    pub rx_queue_index: u32,
    /// Redirect target interface (set by program if action = Redirect)
    pub redirect_ifindex: u32,
    /// Redirect flags
    pub redirect_flags: u32,
}

// ---------------------------------------------------------------------------
// XDP rule (simplified BPF-like)
// ---------------------------------------------------------------------------

/// XDP match criteria (simplified BPF bytecode replacement)
#[derive(Debug, Clone)]
pub enum XdpMatch {
    /// Match all packets
    All,
    /// Match by EtherType
    EtherType(u16),
    /// Match by IP protocol
    IpProtocol(u8),
    /// Match by destination port
    DstPort(u16),
    /// Match by source IP (with mask)
    SrcIp { addr: [u8; 4], mask: [u8; 4] },
    /// Match by destination IP (with mask)
    DstIp { addr: [u8; 4], mask: [u8; 4] },
    /// Match by packet size range
    PacketSize { min: usize, max: usize },
}

impl XdpMatch {
    /// Evaluate the match against raw packet data
    pub fn matches(&self, data: &[u8]) -> bool {
        match self {
            XdpMatch::All => true,
            XdpMatch::EtherType(et) => {
                if data.len() < 14 {
                    return false;
                }
                let pkt_et = u16::from_be_bytes([data[12], data[13]]);
                pkt_et == *et
            }
            XdpMatch::IpProtocol(proto) => {
                if data.len() < 24 {
                    return false;
                }
                // Assume Ethernet + IPv4
                let pkt_et = u16::from_be_bytes([data[12], data[13]]);
                if pkt_et != 0x0800 {
                    return false;
                }
                data[23] == *proto
            }
            XdpMatch::DstPort(port) => {
                if data.len() < 36 {
                    return false;
                }
                let pkt_et = u16::from_be_bytes([data[12], data[13]]);
                if pkt_et != 0x0800 {
                    return false;
                }
                let ihl = ((data[14] & 0x0F) as usize) * 4;
                let transport_offset = 14 + ihl;
                if data.len() < transport_offset + 4 {
                    return false;
                }
                let dp =
                    u16::from_be_bytes([data[transport_offset + 2], data[transport_offset + 3]]);
                dp == *port
            }
            XdpMatch::SrcIp { addr, mask } => {
                if data.len() < 30 {
                    return false;
                }
                let pkt_et = u16::from_be_bytes([data[12], data[13]]);
                if pkt_et != 0x0800 {
                    return false;
                }
                for i in 0..4 {
                    if (data[26 + i] & mask[i]) != (addr[i] & mask[i]) {
                        return false;
                    }
                }
                true
            }
            XdpMatch::DstIp { addr, mask } => {
                if data.len() < 34 {
                    return false;
                }
                let pkt_et = u16::from_be_bytes([data[12], data[13]]);
                if pkt_et != 0x0800 {
                    return false;
                }
                for i in 0..4 {
                    if (data[30 + i] & mask[i]) != (addr[i] & mask[i]) {
                        return false;
                    }
                }
                true
            }
            XdpMatch::PacketSize { min, max } => data.len() >= *min && data.len() <= *max,
        }
    }
}

/// An XDP rule: match + action
#[derive(Debug, Clone)]
pub struct XdpRule {
    pub match_criteria: XdpMatch,
    pub action: XdpAction,
    /// Redirect target (if action is Redirect)
    pub redirect_target: u32,
    /// Packet counter
    pub packets: u64,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// XDP program
// ---------------------------------------------------------------------------

/// XDP program attached to a network interface
pub struct XdpProgram {
    /// Program ID
    pub id: u32,
    /// Interface index
    pub iface_index: u32,
    /// Program name
    pub name: String,
    /// Attachment mode
    pub mode: XdpMode,
    /// Rules (evaluated top-down, first match wins)
    pub rules: Vec<XdpRule>,
    /// Default action (if no rule matches)
    pub default_action: XdpAction,
    /// Whether program is loaded/active
    pub loaded: bool,
    /// Total packets processed
    pub total_packets: u64,
    pub total_bytes: u64,
    /// Action counters
    pub pass_count: u64,
    pub drop_count: u64,
    pub tx_count: u64,
    pub redirect_count: u64,
    pub abort_count: u64,
}

impl XdpProgram {
    pub fn new(id: u32, iface_index: u32, name: &str, mode: XdpMode) -> Self {
        XdpProgram {
            id,
            iface_index,
            name: String::from(name),
            mode,
            rules: Vec::new(),
            default_action: XdpAction::Pass,
            loaded: false,
            total_packets: 0,
            total_bytes: 0,
            pass_count: 0,
            drop_count: 0,
            tx_count: 0,
            redirect_count: 0,
            abort_count: 0,
        }
    }

    /// Add a rule to the program
    pub fn add_rule(&mut self, rule: XdpRule) {
        self.rules.push(rule);
    }

    /// Process a packet through this XDP program
    pub fn process_packet(&mut self, data: &[u8]) -> (XdpAction, u32) {
        self.total_packets = self.total_packets.saturating_add(1);
        self.total_bytes = self.total_bytes.saturating_add(data.len() as u64);

        for rule in &mut self.rules {
            if rule.match_criteria.matches(data) {
                rule.packets = rule.packets.saturating_add(1);
                rule.bytes = rule.bytes.saturating_add(data.len() as u64);
                let action = rule.action;
                let target = rule.redirect_target;
                self.update_counters(action);
                return (action, target);
            }
        }

        let action = self.default_action;
        self.update_counters(action);
        (action, 0)
    }

    fn update_counters(&mut self, action: XdpAction) {
        match action {
            XdpAction::Pass => self.pass_count = self.pass_count.saturating_add(1),
            XdpAction::Drop => self.drop_count = self.drop_count.saturating_add(1),
            XdpAction::Tx => self.tx_count = self.tx_count.saturating_add(1),
            XdpAction::Redirect => self.redirect_count = self.redirect_count.saturating_add(1),
            XdpAction::Aborted => self.abort_count = self.abort_count.saturating_add(1),
        }
    }

    /// Attach the program
    pub fn attach(&mut self) -> Result<(), XdpError> {
        self.loaded = true;
        serial_println!(
            "  XDP: program '{}' attached to iface {} ({:?})",
            self.name,
            self.iface_index,
            self.mode
        );
        Ok(())
    }

    /// Detach the program
    pub fn detach(&mut self) {
        self.loaded = false;
        serial_println!(
            "  XDP: program '{}' detached from iface {}",
            self.name,
            self.iface_index
        );
    }
}

// ---------------------------------------------------------------------------
// XDP maps (shared state)
// ---------------------------------------------------------------------------

/// XDP map type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpMapType {
    /// Hash map
    Hash,
    /// Array map
    Array,
    /// Per-CPU array
    PerCpuArray,
    /// LPM trie (longest prefix match)
    LpmTrie,
    /// Device map (for redirect targets)
    DevMap,
}

/// XDP map entry
#[derive(Debug, Clone)]
struct MapEntry {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// XDP map
pub struct XdpMap {
    pub id: u32,
    pub name: String,
    pub map_type: XdpMapType,
    pub key_size: usize,
    pub value_size: usize,
    pub max_entries: usize,
    entries: Vec<MapEntry>,
}

impl XdpMap {
    pub fn new(
        id: u32,
        name: &str,
        map_type: XdpMapType,
        key_size: usize,
        value_size: usize,
        max_entries: usize,
    ) -> Self {
        XdpMap {
            id,
            name: String::from(name),
            map_type,
            key_size,
            value_size,
            max_entries: max_entries.min(MAX_MAP_ENTRIES),
            entries: Vec::new(),
        }
    }

    /// Insert or update a key-value pair
    pub fn update(&mut self, key: &[u8], value: &[u8]) -> Result<(), XdpError> {
        if key.len() != self.key_size || value.len() != self.value_size {
            return Err(XdpError::InvalidSize);
        }
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.value = value.to_vec();
        } else {
            if self.entries.len() >= self.max_entries {
                return Err(XdpError::MapFull);
            }
            self.entries.push(MapEntry {
                key: key.to_vec(),
                value: value.to_vec(),
            });
        }
        Ok(())
    }

    /// Lookup a value by key
    pub fn lookup(&self, key: &[u8]) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_slice())
    }

    /// Delete a key
    pub fn delete(&mut self, key: &[u8]) -> bool {
        let len_before = self.entries.len();
        self.entries.retain(|e| e.key != key);
        self.entries.len() < len_before
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpError {
    NotInitialized,
    ProgramNotFound,
    MaxProgramsReached,
    InvalidSize,
    MapFull,
    MapNotFound,
    AlreadyAttached,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

struct XdpSubsystem {
    programs: Vec<XdpProgram>,
    maps: Vec<XdpMap>,
    next_prog_id: u32,
    next_map_id: u32,
}

static XDP: Mutex<Option<XdpSubsystem>> = Mutex::new(None);

/// Initialize the XDP subsystem
pub fn init() {
    *XDP.lock() = Some(XdpSubsystem {
        programs: Vec::new(),
        maps: Vec::new(),
        next_prog_id: 1,
        next_map_id: 1,
    });
    serial_println!("  Net: XDP subsystem initialized");
}

/// Create and register an XDP program (does not attach it yet)
pub fn create_program(iface_index: u32, name: &str, mode: XdpMode) -> Result<u32, XdpError> {
    let mut guard = XDP.lock();
    let sys = guard.as_mut().ok_or(XdpError::NotInitialized)?;
    if sys.programs.len() >= MAX_PROGRAMS {
        return Err(XdpError::MaxProgramsReached);
    }
    let id = sys.next_prog_id;
    sys.next_prog_id = sys.next_prog_id.saturating_add(1);
    sys.programs
        .push(XdpProgram::new(id, iface_index, name, mode));
    Ok(id)
}

/// Attach an XDP program to its interface
pub fn attach_program(prog_id: u32) -> Result<(), XdpError> {
    let mut guard = XDP.lock();
    let sys = guard.as_mut().ok_or(XdpError::NotInitialized)?;
    let prog = sys
        .programs
        .iter_mut()
        .find(|p| p.id == prog_id)
        .ok_or(XdpError::ProgramNotFound)?;
    prog.attach()
}

/// Detach an XDP program
pub fn detach_program(prog_id: u32) -> Result<(), XdpError> {
    let mut guard = XDP.lock();
    let sys = guard.as_mut().ok_or(XdpError::NotInitialized)?;
    let prog = sys
        .programs
        .iter_mut()
        .find(|p| p.id == prog_id)
        .ok_or(XdpError::ProgramNotFound)?;
    prog.detach();
    Ok(())
}

/// Run all attached XDP programs for a given interface on a packet
///
/// Returns the final action. Programs are run in order; first non-Pass
/// action terminates processing.
pub fn run_xdp(iface_index: u32, data: &[u8]) -> XdpAction {
    let mut guard = XDP.lock();
    let sys = match guard.as_mut() {
        Some(s) => s,
        None => return XdpAction::Pass,
    };

    for prog in &mut sys.programs {
        if prog.iface_index == iface_index && prog.loaded {
            let (action, _target) = prog.process_packet(data);
            if action != XdpAction::Pass {
                return action;
            }
        }
    }

    XdpAction::Pass
}

/// Create an XDP map
pub fn create_map(
    name: &str,
    map_type: XdpMapType,
    key_size: usize,
    value_size: usize,
    max_entries: usize,
) -> Result<u32, XdpError> {
    let mut guard = XDP.lock();
    let sys = guard.as_mut().ok_or(XdpError::NotInitialized)?;
    if sys.maps.len() >= MAX_MAPS {
        return Err(XdpError::MapFull);
    }
    let id = sys.next_map_id;
    sys.next_map_id = sys.next_map_id.saturating_add(1);
    sys.maps.push(XdpMap::new(
        id,
        name,
        map_type,
        key_size,
        value_size,
        max_entries,
    ));
    Ok(id)
}
