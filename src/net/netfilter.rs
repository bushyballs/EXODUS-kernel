/// Packet filtering framework — nftables-inspired, no-heap bare-metal.
///
/// Implements a simplified netfilter/nftables-like packet filtering layer.
/// Five hook points: PREROUTING, INPUT, FORWARD, OUTPUT, POSTROUTING.
/// Chains contain ordered rules. Rules match on src/dst IP (as u32 with mask),
/// src/dst port, IP protocol, and interface index.
/// All storage is fixed-size static arrays. No Vec, no String, no alloc.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Capacity constants
// ---------------------------------------------------------------------------

pub const MAX_CHAINS: usize = 8;
pub const MAX_RULES_PER_CHAIN: usize = 32;
pub const MAX_SETS: usize = 16;
pub const MAX_SET_ELEMENTS: usize = 128;

// ---------------------------------------------------------------------------
// Hook points
// ---------------------------------------------------------------------------

pub const NF_HOOK_PREROUTING: u8 = 0;
pub const NF_HOOK_INPUT: u8 = 1;
pub const NF_HOOK_FORWARD: u8 = 2;
pub const NF_HOOK_OUTPUT: u8 = 3;
pub const NF_HOOK_POSTROUTING: u8 = 4;

// ---------------------------------------------------------------------------
// Verdicts
// ---------------------------------------------------------------------------

pub const NF_ACCEPT: i32 = 1;
pub const NF_DROP: i32 = 0;
pub const NF_STOLEN: i32 = -1;
pub const NF_QUEUE: i32 = -2;

// ---------------------------------------------------------------------------
// Match types
// ---------------------------------------------------------------------------

pub const NF_MATCH_SADDR: u8 = 1; // match source IP
pub const NF_MATCH_DADDR: u8 = 2; // match dest IP
pub const NF_MATCH_SPORT: u8 = 3; // match source port
pub const NF_MATCH_DPORT: u8 = 4; // match dest port
pub const NF_MATCH_PROTO: u8 = 5; // match IP protocol
pub const NF_MATCH_IFACE: u8 = 6; // match interface index

// ---------------------------------------------------------------------------
// NfMatch
// ---------------------------------------------------------------------------

/// A single match condition within a rule.
///
/// For SADDR/DADDR: `value` is the network address as big-endian u32,
/// `mask` is the subnet mask as big-endian u32 (e.g. 0xFFFFFF00 for /24).
/// For SPORT/DPORT: `value` holds the port number in the low 16 bits; `mask` is ignored.
/// For PROTO:       `value` holds the protocol byte; `mask` is ignored.
/// For IFACE:       `value` holds the interface index; `mask` is ignored.
/// A zero `match_type` means this slot is unused (always matches = true).
#[derive(Clone, Copy)]
pub struct NfMatch {
    pub match_type: u8,
    pub value: u32,
    pub mask: u32,
}

impl NfMatch {
    pub const fn empty() -> Self {
        NfMatch {
            match_type: 0,
            value: 0,
            mask: 0,
        }
    }

    /// Evaluate this match condition against the given packet fields.
    /// An unused slot (match_type == 0) is treated as a wildcard (always true).
    pub fn matches(
        &self,
        saddr: u32,
        daddr: u32,
        sport: u16,
        dport: u16,
        proto: u8,
        iface: u32,
    ) -> bool {
        match self.match_type {
            0 => true, // unused slot — wildcard
            NF_MATCH_SADDR => (saddr & self.mask) == (self.value & self.mask),
            NF_MATCH_DADDR => (daddr & self.mask) == (self.value & self.mask),
            NF_MATCH_SPORT => (sport as u32) == self.value,
            NF_MATCH_DPORT => (dport as u32) == self.value,
            NF_MATCH_PROTO => (proto as u32) == self.value,
            NF_MATCH_IFACE => iface == self.value,
            _ => false, // unknown match type — conservative deny
        }
    }
}

// ---------------------------------------------------------------------------
// NfRule
// ---------------------------------------------------------------------------

/// A filtering rule: up to 4 match conditions (AND logic) + verdict.
///
/// `jump_chain` is only used when `verdict` signals a chain-jump (reserved for
/// future use; current evaluation returns the verdict directly).
/// `counter_pkts` / `counter_bytes` are updated on each match using saturating_add.
#[derive(Clone, Copy)]
pub struct NfRule {
    pub matches: [NfMatch; 4],
    pub nmatch: u8,     // number of active match slots (0..=4)
    pub verdict: i32,   // NF_ACCEPT / NF_DROP / NF_STOLEN / NF_QUEUE
    pub jump_chain: u8, // reserved: chain index to jump to
    pub counter_pkts: u64,
    pub counter_bytes: u64,
    pub active: bool,
}

impl NfRule {
    pub const fn empty() -> Self {
        NfRule {
            matches: [NfMatch::empty(); 4],
            nmatch: 0,
            verdict: NF_ACCEPT,
            jump_chain: 0,
            counter_pkts: 0,
            counter_bytes: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NfChain
// ---------------------------------------------------------------------------

/// An ordered list of rules registered at a specific hook point.
///
/// `name` is a NUL-terminated or space-padded byte array (max 15 chars + NUL).
/// `priority` controls evaluation order among chains at the same hook (lower = first).
/// `policy` is the default verdict when no rule matches.
#[derive(Clone, Copy)]
pub struct NfChain {
    pub name: [u8; 16],
    pub hook: u8,
    pub priority: i32,
    pub policy: i32,
    pub rules: [NfRule; MAX_RULES_PER_CHAIN],
    pub nrules: u8,
    pub active: bool,
}

impl NfChain {
    pub const fn empty() -> Self {
        NfChain {
            name: [0u8; 16],
            hook: 0,
            priority: 0,
            policy: NF_ACCEPT,
            rules: [NfRule::empty(); MAX_RULES_PER_CHAIN],
            nrules: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NfSetElement / NfSet
// ---------------------------------------------------------------------------

/// One element in an NfSet.  Stores up to 16 bytes of opaque value data
/// (e.g. a raw IPv4 address, a port number encoded in 2 bytes, etc.).
#[derive(Clone, Copy)]
pub struct NfSetElement {
    pub value: [u8; 16],
    pub len: u8,
    pub valid: bool,
}

impl NfSetElement {
    pub const fn empty() -> Self {
        NfSetElement {
            value: [0u8; 16],
            len: 0,
            valid: false,
        }
    }
}

/// A named set of opaque byte values, usable for IP-list or port-list matching.
#[derive(Clone, Copy)]
pub struct NfSet {
    pub name: [u8; 16],
    pub elements: [NfSetElement; MAX_SET_ELEMENTS],
    pub nelems: u16,
    pub active: bool,
}

impl NfSet {
    pub const fn empty() -> Self {
        NfSet {
            name: [0u8; 16],
            elements: [NfSetElement::empty(); MAX_SET_ELEMENTS],
            nelems: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NF_CHAINS: Mutex<[NfChain; MAX_CHAINS]> = Mutex::new([NfChain::empty(); MAX_CHAINS]);
static NF_SETS: Mutex<[NfSet; MAX_SETS]> = Mutex::new([NfSet::empty(); MAX_SETS]);
static NF_ENABLED: AtomicBool = AtomicBool::new(false);

// SAFETY: NfChain / NfSet contain only primitive Copy types and are accessed
// only through their respective Mutex guards.
unsafe impl Send for NfChain {}
unsafe impl Sync for NfChain {}
unsafe impl Send for NfSet {}
unsafe impl Sync for NfSet {}

// ---------------------------------------------------------------------------
// Enable / disable
// ---------------------------------------------------------------------------

/// Enable the netfilter subsystem.  Packets will be filtered from this point.
pub fn nf_enable() {
    NF_ENABLED.store(true, Ordering::Relaxed);
}

/// Disable the netfilter subsystem.  All packets are accepted.
pub fn nf_disable() {
    NF_ENABLED.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Chain management
// ---------------------------------------------------------------------------

/// Register a new chain at `hook` with the given `priority` and default `policy`.
///
/// `name` is truncated to 15 bytes.  Returns `Some(chain_id)` on success,
/// `None` if the chain table is full.
pub fn nf_register_chain(name: &[u8], hook: u8, priority: i32, policy: i32) -> Option<u32> {
    let mut chains = NF_CHAINS.lock();
    // Find an empty slot.
    let slot = (0..MAX_CHAINS).find(|&i| !chains[i].active)?;

    let mut chain_name = [0u8; 16];
    let copy_len = name.len().min(15);
    chain_name[..copy_len].copy_from_slice(&name[..copy_len]);

    chains[slot] = NfChain {
        name: chain_name,
        hook,
        priority,
        policy,
        rules: [NfRule::empty(); MAX_RULES_PER_CHAIN],
        nrules: 0,
        active: true,
    };

    Some(slot as u32)
}

/// Append a rule to the chain identified by `chain_id`.
///
/// Returns `true` on success, `false` if the chain is full, inactive, or
/// `chain_id` is out of range.
pub fn nf_add_rule(chain_id: u32, rule: NfRule) -> bool {
    if chain_id as usize >= MAX_CHAINS {
        return false;
    }
    let mut chains = NF_CHAINS.lock();
    let chain = &mut chains[chain_id as usize];
    if !chain.active {
        return false;
    }
    let idx = chain.nrules as usize;
    if idx >= MAX_RULES_PER_CHAIN {
        return false;
    }
    chain.rules[idx] = rule;
    chain.nrules = chain.nrules.saturating_add(1);
    true
}

/// Remove all rules from the chain identified by `chain_id`.
///
/// Returns `true` on success.
pub fn nf_flush_chain(chain_id: u32) -> bool {
    if chain_id as usize >= MAX_CHAINS {
        return false;
    }
    let mut chains = NF_CHAINS.lock();
    let chain = &mut chains[chain_id as usize];
    if !chain.active {
        return false;
    }
    chain.rules = [NfRule::empty(); MAX_RULES_PER_CHAIN];
    chain.nrules = 0;
    true
}

// ---------------------------------------------------------------------------
// Set management
// ---------------------------------------------------------------------------

/// Create a named set.  Returns `Some(set_id)` on success, `None` if full.
pub fn nf_set_create(name: &[u8]) -> Option<u32> {
    let mut sets = NF_SETS.lock();
    let slot = (0..MAX_SETS).find(|&i| !sets[i].active)?;

    let mut set_name = [0u8; 16];
    let copy_len = name.len().min(15);
    set_name[..copy_len].copy_from_slice(&name[..copy_len]);

    sets[slot] = NfSet {
        name: set_name,
        elements: [NfSetElement::empty(); MAX_SET_ELEMENTS],
        nelems: 0,
        active: true,
    };

    Some(slot as u32)
}

/// Add `value` to a set.  Returns `true` on success.
pub fn nf_set_add(set_id: u32, value: &[u8]) -> bool {
    if set_id as usize >= MAX_SETS {
        return false;
    }
    if value.len() > 16 {
        return false;
    }
    let mut sets = NF_SETS.lock();
    let set = &mut sets[set_id as usize];
    if !set.active {
        return false;
    }

    // Reject duplicates.
    let vlen = value.len() as u8;
    for i in 0..set.nelems as usize {
        let e = &set.elements[i];
        if e.valid && e.len == vlen && e.value[..vlen as usize] == value[..] {
            return true; // already present
        }
    }

    let idx = set.nelems as usize;
    if idx >= MAX_SET_ELEMENTS {
        return false;
    }

    let mut elem_val = [0u8; 16];
    elem_val[..value.len()].copy_from_slice(value);
    set.elements[idx] = NfSetElement {
        value: elem_val,
        len: vlen,
        valid: true,
    };
    set.nelems = set.nelems.saturating_add(1);
    true
}

/// Remove `value` from a set.  Returns `true` if found and removed.
pub fn nf_set_del(set_id: u32, value: &[u8]) -> bool {
    if set_id as usize >= MAX_SETS {
        return false;
    }
    if value.len() > 16 {
        return false;
    }
    let mut sets = NF_SETS.lock();
    let set = &mut sets[set_id as usize];
    if !set.active {
        return false;
    }

    let vlen = value.len() as u8;
    let nelems = set.nelems as usize;
    for i in 0..nelems {
        let e = &set.elements[i];
        if e.valid && e.len == vlen && e.value[..vlen as usize] == value[..] {
            // Swap with last valid element to keep the array compact.
            let last = nelems.saturating_sub(1);
            set.elements[i] = set.elements[last];
            set.elements[last] = NfSetElement::empty();
            set.nelems = set.nelems.saturating_sub(1);
            return true;
        }
    }
    false
}

/// Test whether `value` is a member of the set.
pub fn nf_set_contains(set_id: u32, value: &[u8]) -> bool {
    if set_id as usize >= MAX_SETS {
        return false;
    }
    if value.len() > 16 {
        return false;
    }
    let sets = NF_SETS.lock();
    let set = &sets[set_id as usize];
    if !set.active {
        return false;
    }

    let vlen = value.len() as u8;
    for i in 0..set.nelems as usize {
        let e = &set.elements[i];
        if e.valid && e.len == vlen && e.value[..vlen as usize] == value[..] {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Packet parsing helpers
// ---------------------------------------------------------------------------

/// Extract (src_ip, dst_ip, proto, src_port, dst_port) from a raw IP packet
/// slice of at least `pkt_len` bytes.
///
/// Returns (0, 0, 0, 0, 0) if the packet is too short to parse.
fn parse_packet(pkt: &[u8], pkt_len: usize) -> (u32, u32, u8, u16, u16) {
    // Need at least a 20-byte IPv4 header.
    if pkt_len < 20 || pkt.len() < 20 {
        return (0, 0, 0, 0, 0);
    }

    // Bytes 12-15: source IP, 16-19: dest IP (big-endian).
    let src_ip = u32::from_be_bytes([pkt[12], pkt[13], pkt[14], pkt[15]]);
    let dst_ip = u32::from_be_bytes([pkt[16], pkt[17], pkt[18], pkt[19]]);
    let proto = pkt[9];

    // IP header length = (version_ihl & 0x0F) * 4.
    let ihl = ((pkt[0] & 0x0F) as usize).saturating_mul(4);
    if ihl < 20 {
        return (src_ip, dst_ip, proto, 0, 0);
    }

    // TCP (6) or UDP (17): sport at ihl+0..2, dport at ihl+2..4.
    if (proto == 6 || proto == 17)
        && pkt_len >= ihl.saturating_add(4)
        && pkt.len() >= ihl.saturating_add(4)
    {
        let sport = u16::from_be_bytes([pkt[ihl], pkt[ihl + 1]]);
        let dport = u16::from_be_bytes([pkt[ihl + 2], pkt[ihl + 3]]);
        (src_ip, dst_ip, proto, sport, dport)
    } else {
        (src_ip, dst_ip, proto, 0, 0)
    }
}

// ---------------------------------------------------------------------------
// Core hook evaluation
// ---------------------------------------------------------------------------

/// Run a packet through all chains registered at `hook`.
///
/// If the netfilter subsystem is disabled, returns `NF_ACCEPT` immediately.
///
/// Chains at the same hook are evaluated in ascending priority order
/// (lower numeric priority = evaluated first).  Within each chain, rules
/// are evaluated in insertion order.  The first matching rule's verdict is
/// returned; if no rule matches the chain's policy applies.
///
/// `pkt` — raw bytes of the IP packet (starting at the IP header).
/// `pkt_len` — logical packet length (may be <= pkt.len()).
/// `iface` — interface index on which the packet arrived/is leaving.
pub fn nf_hook(hook: u8, pkt: &[u8], pkt_len: usize, iface: u32) -> i32 {
    if !NF_ENABLED.load(Ordering::Relaxed) {
        return NF_ACCEPT;
    }

    let (saddr, daddr, proto, sport, dport) = parse_packet(pkt, pkt_len);

    // We need to evaluate chains sorted by priority.  With MAX_CHAINS == 8
    // a simple insertion-sort scratch array is fine and avoids any alloc.
    //
    // Build a list of (priority, chain_index) for active chains at this hook.
    let chains = NF_CHAINS.lock();

    // Scratch: up to MAX_CHAINS entries, (priority: i32, chain_idx: usize).
    let mut order: [(i32, usize); MAX_CHAINS] = [(0, 0); MAX_CHAINS];
    let mut order_len: usize = 0;

    for i in 0..MAX_CHAINS {
        if chains[i].active && chains[i].hook == hook {
            // Insertion-sort by priority (ascending).
            let prio = chains[i].priority;
            let mut pos = order_len;
            // Find insertion position.
            let mut j = 0usize;
            while j < order_len {
                if prio < order[j].0 {
                    pos = j;
                    break;
                }
                j = j.saturating_add(1);
            }
            // Shift right to make room.
            let mut k = order_len;
            while k > pos {
                order[k] = order[k - 1];
                k = k.saturating_sub(1);
            }
            order[pos] = (prio, i);
            order_len = order_len.saturating_add(1);
        }
    }

    // Evaluate each chain in priority order.
    let mut verdict = NF_ACCEPT; // default if no chains match at this hook
    let mut i = 0usize;
    while i < order_len {
        let chain_idx = order[i].1;
        let chain = &chains[chain_idx];

        // Evaluate rules in order.
        let mut rule_verdict: Option<i32> = None;
        let mut r = 0usize;
        while r < chain.nrules as usize && r < MAX_RULES_PER_CHAIN {
            let rule = &chain.rules[r];
            if !rule.active {
                r = r.saturating_add(1);
                continue;
            }

            // AND all active match conditions.
            let mut all_match = true;
            let mut m = 0usize;
            while m < rule.nmatch as usize && m < 4 {
                if !rule.matches[m].matches(saddr, daddr, sport, dport, proto, iface) {
                    all_match = false;
                    break;
                }
                m = m.saturating_add(1);
            }

            if all_match {
                // Update counters via interior mutability is not available
                // without UnsafeCell through the Mutex guard.  We hold the
                // Mutex, but the borrow checker sees the data as shared here.
                // We must drop the lock, update counters, re-lock.  However,
                // because the Mutex is held through `chains`, we cannot call
                // lock() again.  The standard bare-metal pattern is to collect
                // the verdict first (read-only), then do a second lock pass to
                // update counters.  We do that below.
                rule_verdict = Some(rule.verdict);
                break;
            }
            r = r.saturating_add(1);
        }

        verdict = rule_verdict.unwrap_or(chain.policy);

        // If a rule or the chain policy drops the packet, stop immediately.
        if verdict == NF_DROP || verdict == NF_STOLEN || verdict == NF_QUEUE {
            drop(chains);
            // Second pass: update counters for the matched rule.
            update_counters(
                chain_idx, hook, saddr, daddr, sport, dport, proto, iface, pkt_len,
            );
            return verdict;
        }

        i = i.saturating_add(1);
    }

    drop(chains);

    // Update counters for any rules that matched (best-effort).
    // We iterate again with a fresh lock.
    let mut j = 0usize;
    while j < order_len {
        update_counters(
            order[j].1, hook, saddr, daddr, sport, dport, proto, iface, pkt_len,
        );
        j = j.saturating_add(1);
    }

    verdict
}

/// Update `counter_pkts` / `counter_bytes` for the first matching rule in
/// `chain_idx`, or for no rule if none matches (chain policy hit).
fn update_counters(
    chain_idx: usize,
    _hook: u8,
    saddr: u32,
    daddr: u32,
    sport: u16,
    dport: u16,
    proto: u8,
    iface: u32,
    pkt_len: usize,
) {
    let mut chains = NF_CHAINS.lock();
    if chain_idx >= MAX_CHAINS {
        return;
    }
    let chain = &mut chains[chain_idx];
    if !chain.active {
        return;
    }

    let mut r = 0usize;
    while r < chain.nrules as usize && r < MAX_RULES_PER_CHAIN {
        let rule = &mut chain.rules[r];
        if !rule.active {
            r = r.saturating_add(1);
            continue;
        }
        let mut all_match = true;
        let mut m = 0usize;
        while m < rule.nmatch as usize && m < 4 {
            if !rule.matches[m].matches(saddr, daddr, sport, dport, proto, iface) {
                all_match = false;
                break;
            }
            m = m.saturating_add(1);
        }
        if all_match {
            rule.counter_pkts = rule.counter_pkts.saturating_add(1);
            rule.counter_bytes = rule.counter_bytes.saturating_add(pkt_len as u64);
            return;
        }
        r = r.saturating_add(1);
    }
    // No rule matched — policy hit, nothing to count at rule level.
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Return the sum of all rule counters for the chain: `(total_pkts, total_bytes)`.
///
/// Returns `None` if `chain_id` is out of range or the chain is inactive.
pub fn nf_get_stats(chain_id: u32) -> Option<(u64, u64)> {
    if chain_id as usize >= MAX_CHAINS {
        return None;
    }
    let chains = NF_CHAINS.lock();
    let chain = &chains[chain_id as usize];
    if !chain.active {
        return None;
    }

    let mut pkts: u64 = 0;
    let mut bytes: u64 = 0;
    let mut r = 0usize;
    while r < chain.nrules as usize && r < MAX_RULES_PER_CHAIN {
        pkts = pkts.saturating_add(chain.rules[r].counter_pkts);
        bytes = bytes.saturating_add(chain.rules[r].counter_bytes);
        r = r.saturating_add(1);
    }
    Some((pkts, bytes))
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the netfilter subsystem with two default chains:
/// - "INPUT"  at `NF_HOOK_INPUT`  with ACCEPT policy.
/// - "OUTPUT" at `NF_HOOK_OUTPUT` with ACCEPT policy.
///
/// The subsystem starts **disabled**; call `nf_enable()` to activate filtering.
pub fn init() {
    // Reset all chains and sets to empty.
    {
        let mut chains = NF_CHAINS.lock();
        for i in 0..MAX_CHAINS {
            chains[i] = NfChain::empty();
        }
    }
    {
        let mut sets = NF_SETS.lock();
        for i in 0..MAX_SETS {
            sets[i] = NfSet::empty();
        }
    }

    // Register default INPUT chain.
    nf_register_chain(b"INPUT", NF_HOOK_INPUT, 0, NF_ACCEPT);
    // Register default OUTPUT chain.
    nf_register_chain(b"OUTPUT", NF_HOOK_OUTPUT, 0, NF_ACCEPT);

    serial_println!("[netfilter] packet filtering initialized");
}
