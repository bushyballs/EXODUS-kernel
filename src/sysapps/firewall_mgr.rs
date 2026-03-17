/// Firewall manager application for Genesis OS
///
/// Rule-based packet filtering with port open/close, IP whitelist/blacklist,
/// protocol filtering, logging, and security presets. Rules are evaluated
/// in priority order with first-match semantics. All network addresses
/// stored as u32 (IPv4) or hash values for kernel-level efficiency.
///
/// Inspired by: iptables, ufw, Windows Firewall. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum firewall rules
const MAX_RULES: usize = 4096;
/// Maximum whitelist entries
const MAX_WHITELIST: usize = 1024;
/// Maximum blacklist entries
const MAX_BLACKLIST: usize = 1024;
/// Maximum log entries kept in memory
const MAX_LOG_ENTRIES: usize = 10_000;
/// Maximum presets
const MAX_PRESETS: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Network protocol
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Protocol {
    Tcp,
    Udp,
    Icmp,
    Any,
}

/// Firewall action for a matching rule
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Allow,
    Deny,
    Drop,
    Log,
}

/// Traffic direction
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Direction {
    Inbound,
    Outbound,
    Both,
}

/// Firewall operation result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FwResult {
    Success,
    NotFound,
    AlreadyExists,
    LimitReached,
    InvalidRule,
    Disabled,
    IoError,
}

/// Log severity
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Info,
    Warning,
    Alert,
    Critical,
}

/// A single firewall rule
#[derive(Debug, Clone)]
pub struct FirewallRule {
    pub id: u64,
    pub name_hash: u64,
    pub priority: u32,
    pub action: Action,
    pub direction: Direction,
    pub protocol: Protocol,
    pub src_ip: u32,
    pub src_mask: u32,
    pub dst_ip: u32,
    pub dst_mask: u32,
    pub src_port_start: u16,
    pub src_port_end: u16,
    pub dst_port_start: u16,
    pub dst_port_end: u16,
    pub enabled: bool,
    pub hit_count: u64,
    pub created: u64,
}

/// An IP list entry (whitelist or blacklist)
#[derive(Debug, Clone, Copy)]
pub struct IpEntry {
    pub ip: u32,
    pub mask: u32,
    pub added: u64,
    pub comment_hash: u64,
}

/// A firewall log entry
#[derive(Debug, Clone, Copy)]
pub struct LogEntry {
    pub timestamp: u64,
    pub rule_id: u64,
    pub action: Action,
    pub protocol: Protocol,
    pub src_ip: u32,
    pub dst_ip: u32,
    pub src_port: u16,
    pub dst_port: u16,
    pub level: LogLevel,
    pub packet_size: u32,
}

/// A preset firewall configuration
#[derive(Debug, Clone)]
pub struct Preset {
    pub id: u64,
    pub name_hash: u64,
    pub description_hash: u64,
    pub rule_ids: Vec<u64>,
    pub default_action: Action,
    pub is_builtin: bool,
}

/// Firewall state
struct FirewallState {
    rules: Vec<FirewallRule>,
    whitelist: Vec<IpEntry>,
    blacklist: Vec<IpEntry>,
    log: Vec<LogEntry>,
    presets: Vec<Preset>,
    enabled: bool,
    default_inbound: Action,
    default_outbound: Action,
    logging_enabled: bool,
    next_rule_id: u64,
    next_preset_id: u64,
    timestamp: u64,
    packets_inspected: u64,
    packets_blocked: u64,
    packets_allowed: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FIREWALL: Mutex<Option<FirewallState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn next_timestamp(state: &mut FirewallState) -> u64 {
    state.timestamp += 1;
    state.timestamp
}

fn default_state() -> FirewallState {
    FirewallState {
        rules: Vec::new(),
        whitelist: Vec::new(),
        blacklist: Vec::new(),
        log: Vec::new(),
        presets: Vec::new(),
        enabled: true,
        default_inbound: Action::Deny,
        default_outbound: Action::Allow,
        logging_enabled: true,
        next_rule_id: 1,
        next_preset_id: 1,
        timestamp: 0,
        packets_inspected: 0,
        packets_blocked: 0,
        packets_allowed: 0,
    }
}

fn ip_matches(addr: u32, rule_ip: u32, rule_mask: u32) -> bool {
    if rule_mask == 0 {
        return true; // 0 mask = match all
    }
    (addr & rule_mask) == (rule_ip & rule_mask)
}

fn port_in_range(port: u16, start: u16, end: u16) -> bool {
    if start == 0 && end == 0 {
        return true; // 0-0 = match all
    }
    port >= start && port <= end
}

fn sort_rules_by_priority(rules: &mut Vec<FirewallRule>) {
    rules.sort_by(|a, b| a.priority.cmp(&b.priority));
}

fn append_log(state: &mut FirewallState, entry: LogEntry) {
    if !state.logging_enabled {
        return;
    }
    if state.log.len() >= MAX_LOG_ENTRIES {
        // Drop oldest 10% when full
        let drop_count = MAX_LOG_ENTRIES / 10;
        state.log.drain(..drop_count);
    }
    state.log.push(entry);
}

// ---------------------------------------------------------------------------
// Public API -- Firewall control
// ---------------------------------------------------------------------------

/// Enable or disable the firewall
pub fn set_enabled(enabled: bool) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    state.enabled = enabled;
    FwResult::Success
}

/// Check if firewall is enabled
pub fn is_enabled() -> bool {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.enabled,
        None => false,
    }
}

/// Set default action for inbound traffic
pub fn set_default_inbound(action: Action) {
    let mut guard = FIREWALL.lock();
    if let Some(state) = guard.as_mut() {
        state.default_inbound = action;
    }
}

/// Set default action for outbound traffic
pub fn set_default_outbound(action: Action) {
    let mut guard = FIREWALL.lock();
    if let Some(state) = guard.as_mut() {
        state.default_outbound = action;
    }
}

// ---------------------------------------------------------------------------
// Public API -- Rule management
// ---------------------------------------------------------------------------

/// Add a firewall rule
pub fn add_rule(
    name_hash: u64,
    priority: u32,
    action: Action,
    direction: Direction,
    protocol: Protocol,
    src_ip: u32,
    src_mask: u32,
    dst_ip: u32,
    dst_mask: u32,
    dst_port_start: u16,
    dst_port_end: u16,
) -> Result<u64, FwResult> {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(FwResult::IoError),
    };
    if state.rules.len() >= MAX_RULES {
        return Err(FwResult::LimitReached);
    }
    if dst_port_end < dst_port_start && dst_port_end != 0 {
        return Err(FwResult::InvalidRule);
    }
    let now = next_timestamp(state);
    let id = state.next_rule_id;
    state.next_rule_id += 1;
    state.rules.push(FirewallRule {
        id,
        name_hash,
        priority,
        action,
        direction,
        protocol,
        src_ip,
        src_mask,
        dst_ip,
        dst_mask,
        src_port_start: 0,
        src_port_end: 0,
        dst_port_start,
        dst_port_end,
        enabled: true,
        hit_count: 0,
        created: now,
    });
    sort_rules_by_priority(&mut state.rules);
    Ok(id)
}

/// Remove a firewall rule
pub fn remove_rule(rule_id: u64) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    let before = state.rules.len();
    state.rules.retain(|r| r.id != rule_id);
    if state.rules.len() < before {
        FwResult::Success
    } else {
        FwResult::NotFound
    }
}

/// Enable or disable a specific rule
pub fn toggle_rule(rule_id: u64, enabled: bool) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    if let Some(rule) = state.rules.iter_mut().find(|r| r.id == rule_id) {
        rule.enabled = enabled;
        FwResult::Success
    } else {
        FwResult::NotFound
    }
}

/// List all rules
pub fn list_rules() -> Vec<FirewallRule> {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.rules.clone(),
        None => Vec::new(),
    }
}

/// Open a port (convenience: adds allow rule)
pub fn open_port(port: u16, protocol: Protocol) -> Result<u64, FwResult> {
    let hash = port as u64 | 0xAAAA_0000_0000_0000;
    add_rule(hash, 100, Action::Allow, Direction::Inbound, protocol, 0, 0, 0, 0, port, port)
}

/// Close a port (convenience: adds deny rule)
pub fn close_port(port: u16, protocol: Protocol) -> Result<u64, FwResult> {
    let hash = port as u64 | 0xBBBB_0000_0000_0000;
    add_rule(hash, 50, Action::Deny, Direction::Inbound, protocol, 0, 0, 0, 0, port, port)
}

// ---------------------------------------------------------------------------
// Public API -- Whitelist / Blacklist
// ---------------------------------------------------------------------------

/// Add an IP to the whitelist
pub fn whitelist_add(ip: u32, mask: u32, comment_hash: u64) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    if state.whitelist.len() >= MAX_WHITELIST {
        return FwResult::LimitReached;
    }
    if state.whitelist.iter().any(|e| e.ip == ip && e.mask == mask) {
        return FwResult::AlreadyExists;
    }
    let now = next_timestamp(state);
    state.whitelist.push(IpEntry { ip, mask, added: now, comment_hash });
    FwResult::Success
}

/// Remove an IP from the whitelist
pub fn whitelist_remove(ip: u32, mask: u32) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    let before = state.whitelist.len();
    state.whitelist.retain(|e| !(e.ip == ip && e.mask == mask));
    if state.whitelist.len() < before {
        FwResult::Success
    } else {
        FwResult::NotFound
    }
}

/// Add an IP to the blacklist
pub fn blacklist_add(ip: u32, mask: u32, comment_hash: u64) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    if state.blacklist.len() >= MAX_BLACKLIST {
        return FwResult::LimitReached;
    }
    if state.blacklist.iter().any(|e| e.ip == ip && e.mask == mask) {
        return FwResult::AlreadyExists;
    }
    let now = next_timestamp(state);
    state.blacklist.push(IpEntry { ip, mask, added: now, comment_hash });
    FwResult::Success
}

/// Remove an IP from the blacklist
pub fn blacklist_remove(ip: u32, mask: u32) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    let before = state.blacklist.len();
    state.blacklist.retain(|e| !(e.ip == ip && e.mask == mask));
    if state.blacklist.len() < before {
        FwResult::Success
    } else {
        FwResult::NotFound
    }
}

/// Get whitelist entries
pub fn get_whitelist() -> Vec<IpEntry> {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.whitelist.clone(),
        None => Vec::new(),
    }
}

/// Get blacklist entries
pub fn get_blacklist() -> Vec<IpEntry> {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.blacklist.clone(),
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Public API -- Packet evaluation
// ---------------------------------------------------------------------------

/// Evaluate a packet against firewall rules (returns action to take)
pub fn evaluate_packet(
    direction: Direction,
    protocol: Protocol,
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    packet_size: u32,
) -> Action {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Action::Drop,
    };
    if !state.enabled {
        return Action::Allow;
    }
    state.packets_inspected += 1;

    // Check blacklist first
    if state.blacklist.iter().any(|e| ip_matches(src_ip, e.ip, e.mask)) {
        state.packets_blocked += 1;
        let now = next_timestamp(state);
        append_log(state, LogEntry {
            timestamp: now, rule_id: 0, action: Action::Drop, protocol,
            src_ip, dst_ip, src_port, dst_port, level: LogLevel::Alert, packet_size,
        });
        return Action::Drop;
    }

    // Check whitelist
    if state.whitelist.iter().any(|e| ip_matches(src_ip, e.ip, e.mask)) {
        state.packets_allowed += 1;
        return Action::Allow;
    }

    // Evaluate rules in priority order (already sorted)
    for rule in state.rules.iter_mut() {
        if !rule.enabled {
            continue;
        }
        // Direction check
        let dir_match = match rule.direction {
            Direction::Both => true,
            d => d == direction,
        };
        if !dir_match {
            continue;
        }
        // Protocol check
        let proto_match = rule.protocol == Protocol::Any || rule.protocol == protocol;
        if !proto_match {
            continue;
        }
        // IP check
        if !ip_matches(src_ip, rule.src_ip, rule.src_mask) {
            continue;
        }
        if !ip_matches(dst_ip, rule.dst_ip, rule.dst_mask) {
            continue;
        }
        // Port check
        if !port_in_range(dst_port, rule.dst_port_start, rule.dst_port_end) {
            continue;
        }
        // Match found
        rule.hit_count += 1;
        match rule.action {
            Action::Allow => state.packets_allowed += 1,
            Action::Deny | Action::Drop => state.packets_blocked += 1,
            Action::Log => state.packets_allowed += 1,
        }
        let now = next_timestamp(state);
        append_log(state, LogEntry {
            timestamp: now, rule_id: rule.id, action: rule.action, protocol,
            src_ip, dst_ip, src_port, dst_port, level: LogLevel::Info, packet_size,
        });
        return rule.action;
    }

    // Default action
    let default_action = match direction {
        Direction::Inbound => state.default_inbound,
        Direction::Outbound => state.default_outbound,
        Direction::Both => state.default_inbound,
    };
    match default_action {
        Action::Allow => state.packets_allowed += 1,
        _ => state.packets_blocked += 1,
    }
    default_action
}

// ---------------------------------------------------------------------------
// Public API -- Logging and stats
// ---------------------------------------------------------------------------

/// Get recent log entries (newest first)
pub fn get_log(max: usize) -> Vec<LogEntry> {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => {
            let start = if state.log.len() > max { state.log.len() - max } else { 0 };
            state.log[start..].iter().rev().cloned().collect()
        }
        None => Vec::new(),
    }
}

/// Clear log
pub fn clear_log() {
    let mut guard = FIREWALL.lock();
    if let Some(state) = guard.as_mut() {
        state.log.clear();
    }
}

/// Toggle logging
pub fn set_logging(enabled: bool) {
    let mut guard = FIREWALL.lock();
    if let Some(state) = guard.as_mut() {
        state.logging_enabled = enabled;
    }
}

/// Get packet statistics
pub fn stats() -> (u64, u64, u64) {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => (state.packets_inspected, state.packets_allowed, state.packets_blocked),
        None => (0, 0, 0),
    }
}

/// Get rule count
pub fn rule_count() -> usize {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.rules.len(),
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Public API -- Presets
// ---------------------------------------------------------------------------

/// Save current rules as a preset
pub fn save_preset(name_hash: u64, description_hash: u64) -> Result<u64, FwResult> {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Err(FwResult::IoError),
    };
    if state.presets.len() >= MAX_PRESETS {
        return Err(FwResult::LimitReached);
    }
    let id = state.next_preset_id;
    state.next_preset_id += 1;
    let rule_ids: Vec<u64> = state.rules.iter().map(|r| r.id).collect();
    state.presets.push(Preset {
        id,
        name_hash,
        description_hash,
        rule_ids,
        default_action: state.default_inbound,
        is_builtin: false,
    });
    Ok(id)
}

/// List all presets
pub fn list_presets() -> Vec<Preset> {
    let guard = FIREWALL.lock();
    match guard.as_ref() {
        Some(state) => state.presets.clone(),
        None => Vec::new(),
    }
}

/// Delete a preset (only non-builtin)
pub fn delete_preset(preset_id: u64) -> FwResult {
    let mut guard = FIREWALL.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return FwResult::IoError,
    };
    if let Some(preset) = state.presets.iter().find(|p| p.id == preset_id) {
        if preset.is_builtin {
            return FwResult::InvalidRule;
        }
    } else {
        return FwResult::NotFound;
    }
    state.presets.retain(|p| p.id != preset_id);
    FwResult::Success
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the firewall manager subsystem
pub fn init() {
    let mut guard = FIREWALL.lock();
    *guard = Some(default_state());
    serial_println!("    Firewall manager ready");
}
