use crate::net::Ipv4Addr;
use crate::sync::Mutex;
/// Routing table for Genesis networking
///
/// Maintains a table of routes for IP packet forwarding.
/// Supports default gateway, per-network routes, and host routes.
///
/// Features:
///   - Route table entries (destination/mask/gateway/interface/metric)
///   - Longest prefix match lookup
///   - Default gateway
///   - Route add/delete/flush
///   - Connected routes (auto-added when interface gets IP)
///   - Policy-based routing (source-based route selection)
///   - Route cache for fast lookups
///
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Route flags
pub const RTF_UP: u32 = 0x0001; // Route is up
pub const RTF_GATEWAY: u32 = 0x0002; // Destination is a gateway
pub const RTF_HOST: u32 = 0x0004; // Host route (not network)
pub const RTF_DEFAULT: u32 = 0x0008; // Default route
pub const RTF_STATIC: u32 = 0x0010; // Manually configured
pub const RTF_CONNECTED: u32 = 0x0020; // Directly connected network
pub const RTF_REJECT: u32 = 0x0040; // Reject route (blackhole)
pub const RTF_LOCAL: u32 = 0x0080; // Local address route

/// Maximum route cache entries
const MAX_CACHE_ENTRIES: usize = 256;

/// Route cache TTL in ticks (ms) — 30 seconds
const CACHE_TTL_MS: u64 = 30_000;

/// Maximum number of routes
const MAX_ROUTES: usize = 1024;

/// Maximum number of policy rules
const MAX_POLICY_RULES: usize = 64;

// ---------------------------------------------------------------------------
// Route entry
// ---------------------------------------------------------------------------

/// A routing table entry
#[derive(Debug, Clone)]
pub struct Route {
    /// Destination network
    pub destination: Ipv4Addr,
    /// Subnet mask
    pub netmask: Ipv4Addr,
    /// Gateway (next hop) — 0.0.0.0 for directly connected
    pub gateway: Ipv4Addr,
    /// Output interface name
    pub iface: String,
    /// Flags
    pub flags: u32,
    /// Metric (lower = preferred)
    pub metric: u32,
}

impl Route {
    /// Check if this route matches a destination address
    pub fn matches(&self, addr: Ipv4Addr) -> bool {
        let dest = u32::from_be_bytes(self.destination.0);
        let mask = u32::from_be_bytes(self.netmask.0);
        let target = u32::from_be_bytes(addr.0);
        (target & mask) == (dest & mask)
    }

    /// How specific is this route (number of 1-bits in mask)
    pub fn prefix_len(&self) -> u32 {
        let mask = u32::from_be_bytes(self.netmask.0);
        mask.count_ones()
    }

    /// Is this a default route (0.0.0.0/0)?
    pub fn is_default(&self) -> bool {
        self.flags & RTF_DEFAULT != 0
            || (self.destination == Ipv4Addr::ANY && self.netmask == Ipv4Addr::ANY)
    }

    /// Is this a host route (/32)?
    pub fn is_host_route(&self) -> bool {
        self.flags & RTF_HOST != 0 || self.prefix_len() == 32
    }

    /// Is this a directly connected network?
    pub fn is_connected(&self) -> bool {
        self.flags & RTF_CONNECTED != 0
    }

    /// Is this a reject/blackhole route?
    pub fn is_reject(&self) -> bool {
        self.flags & RTF_REJECT != 0
    }
}

// ---------------------------------------------------------------------------
// Policy routing rule
// ---------------------------------------------------------------------------

/// A policy routing rule: match source IP → use specific routing table/gateway
#[derive(Debug, Clone)]
pub struct PolicyRule {
    /// Source network to match
    pub src_network: Ipv4Addr,
    /// Source netmask
    pub src_mask: Ipv4Addr,
    /// Priority (lower = evaluated first)
    pub priority: u32,
    /// Gateway to use for matching traffic
    pub gateway: Ipv4Addr,
    /// Interface to use
    pub iface: String,
    /// Whether this rule is enabled
    pub enabled: bool,
}

impl PolicyRule {
    /// Check if a source IP matches this rule
    pub fn matches_source(&self, src: Ipv4Addr) -> bool {
        if !self.enabled {
            return false;
        }
        let net = self.src_network.to_u32();
        let mask = self.src_mask.to_u32();
        let addr = src.to_u32();
        (addr & mask) == (net & mask)
    }
}

// ---------------------------------------------------------------------------
// Route cache entry
// ---------------------------------------------------------------------------

/// Cached route lookup result
#[derive(Clone)]
struct CachedRoute {
    route: Route,
    created_tick: u64,
}

// ---------------------------------------------------------------------------
// Global routing tables
// ---------------------------------------------------------------------------

/// Global routing table
static ROUTES: Mutex<Vec<Route>> = Mutex::new(Vec::new());

/// Policy routing rules
static POLICY_RULES: Mutex<Vec<PolicyRule>> = Mutex::new(Vec::new());

/// Route cache: maps dest_ip_u32 -> cached route
static ROUTE_CACHE: Mutex<BTreeMap<u32, CachedRoute>> = Mutex::new(BTreeMap::new());

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize routing with default routes
pub fn init() {
    let mut routes = ROUTES.lock();

    // Default: localhost
    routes.push(Route {
        destination: Ipv4Addr::new(127, 0, 0, 0),
        netmask: Ipv4Addr::new(255, 0, 0, 0),
        gateway: Ipv4Addr::new(0, 0, 0, 0),
        iface: String::from("lo"),
        flags: RTF_UP | RTF_LOCAL,
        metric: 0,
    });

    // Default route — 10.0.2.2 is typical QEMU gateway
    routes.push(Route {
        destination: Ipv4Addr::new(0, 0, 0, 0),
        netmask: Ipv4Addr::new(0, 0, 0, 0),
        gateway: Ipv4Addr::new(10, 0, 2, 2),
        iface: String::from("eth0"),
        flags: RTF_UP | RTF_GATEWAY | RTF_DEFAULT,
        metric: 100,
    });

    // Local subnet — 10.0.2.0/24
    routes.push(Route {
        destination: Ipv4Addr::new(10, 0, 2, 0),
        netmask: Ipv4Addr::new(255, 255, 255, 0),
        gateway: Ipv4Addr::new(0, 0, 0, 0),
        iface: String::from("eth0"),
        flags: RTF_UP | RTF_CONNECTED,
        metric: 0,
    });

    serial_println!("  Routing: {} routes configured", routes.len());
}

// ---------------------------------------------------------------------------
// Route lookup
// ---------------------------------------------------------------------------

/// Look up the best route for a destination (longest prefix match).
/// Uses the route cache first, then falls back to the full table.
pub fn lookup(dest: Ipv4Addr) -> Option<Route> {
    // Check cache first
    let dest_key = dest.to_u32();
    {
        let cache = ROUTE_CACHE.lock();
        if let Some(cached) = cache.get(&dest_key) {
            let now = crate::time::clock::uptime_ms();
            if now.saturating_sub(cached.created_tick) < CACHE_TTL_MS {
                return Some(cached.route.clone());
            }
        }
    }

    // Full table lookup
    let result = lookup_no_cache(dest);

    // Cache the result
    if let Some(ref route) = result {
        let mut cache = ROUTE_CACHE.lock();
        if cache.len() >= MAX_CACHE_ENTRIES {
            // Evict oldest entry
            let oldest = cache
                .iter()
                .min_by_key(|(_, v)| v.created_tick)
                .map(|(k, _)| *k);
            if let Some(k) = oldest {
                cache.remove(&k);
            }
        }
        cache.insert(
            dest_key,
            CachedRoute {
                route: route.clone(),
                created_tick: crate::time::clock::uptime_ms(),
            },
        );
    }

    result
}

/// Look up a route without using the cache.
fn lookup_no_cache(dest: Ipv4Addr) -> Option<Route> {
    let routes = ROUTES.lock();
    let mut best: Option<&Route> = None;
    let mut best_prefix = 0u32;
    let mut best_metric = u32::MAX;

    for route in routes.iter() {
        if route.flags & RTF_UP == 0 {
            continue;
        }
        if route.matches(dest) {
            let plen = route.prefix_len();
            if plen > best_prefix || (plen == best_prefix && route.metric < best_metric) {
                best = Some(route);
                best_prefix = plen;
                best_metric = route.metric;
            }
        }
    }

    best.cloned()
}

/// Look up a route with policy-based routing (source-based selection).
/// Checks policy rules first; if no policy matches, uses standard lookup.
pub fn lookup_policy(src: Ipv4Addr, dest: Ipv4Addr) -> Option<Route> {
    let rules = POLICY_RULES.lock();
    let mut best_rule: Option<&PolicyRule> = None;
    let mut best_priority = u32::MAX;

    for rule in rules.iter() {
        if rule.matches_source(src) && rule.priority < best_priority {
            best_rule = Some(rule);
            best_priority = rule.priority;
        }
    }

    if let Some(rule) = best_rule {
        // Return a synthetic route from the policy rule
        return Some(Route {
            destination: dest,
            netmask: Ipv4Addr::new(255, 255, 255, 255),
            gateway: rule.gateway,
            iface: rule.iface.clone(),
            flags: RTF_UP | RTF_GATEWAY,
            metric: 0,
        });
    }

    // No policy match — standard lookup
    lookup(dest)
}

/// Get the default gateway.
pub fn default_gateway() -> Option<(Ipv4Addr, String)> {
    let routes = ROUTES.lock();
    for route in routes.iter() {
        if route.is_default() && route.flags & RTF_UP != 0 {
            return Some((route.gateway, route.iface.clone()));
        }
    }
    None
}

/// Get the next hop (gateway) and interface for a destination.
/// If the route is directly connected, returns the destination itself as next hop.
pub fn next_hop(dest: Ipv4Addr) -> Option<(Ipv4Addr, String)> {
    let route = lookup(dest)?;
    if route.is_reject() {
        return None;
    }
    let gw = if route.gateway == Ipv4Addr::ANY || route.is_connected() {
        dest // Directly connected — send directly to the destination
    } else {
        route.gateway
    };
    Some((gw, route.iface))
}

// ---------------------------------------------------------------------------
// Route manipulation
// ---------------------------------------------------------------------------

/// Add a route.
pub fn add(route: Route) -> Result<(), &'static str> {
    let mut routes = ROUTES.lock();
    if routes.len() >= MAX_ROUTES {
        return Err("Routing table full");
    }

    // Check for duplicate
    for existing in routes.iter() {
        if existing.destination.0 == route.destination.0
            && existing.netmask.0 == route.netmask.0
            && existing.gateway.0 == route.gateway.0
        {
            return Err("Route already exists");
        }
    }

    routes.push(route);

    // Invalidate cache
    ROUTE_CACHE.lock().clear();

    Ok(())
}

/// Remove a route matching destination/mask.
pub fn remove(dest: Ipv4Addr, mask: Ipv4Addr) {
    ROUTES
        .lock()
        .retain(|r| !(r.destination.0 == dest.0 && r.netmask.0 == mask.0));
    ROUTE_CACHE.lock().clear();
}

/// Remove a specific route matching destination/mask/gateway.
pub fn remove_exact(dest: Ipv4Addr, mask: Ipv4Addr, gateway: Ipv4Addr) {
    ROUTES.lock().retain(|r| {
        !(r.destination.0 == dest.0 && r.netmask.0 == mask.0 && r.gateway.0 == gateway.0)
    });
    ROUTE_CACHE.lock().clear();
}

/// Flush all routes (except loopback).
pub fn flush() {
    ROUTES.lock().retain(|r| r.iface == "lo");
    ROUTE_CACHE.lock().clear();
}

/// Flush all routes for a specific interface.
pub fn flush_interface(iface_name: &str) {
    ROUTES.lock().retain(|r| r.iface != iface_name);
    ROUTE_CACHE.lock().clear();
}

/// Set the default gateway.
pub fn set_default_gateway(gateway: Ipv4Addr, iface: &str) {
    let mut routes = ROUTES.lock();

    // Remove existing default route
    routes.retain(|r| !r.is_default());

    // Add new default route
    routes.push(Route {
        destination: Ipv4Addr::ANY,
        netmask: Ipv4Addr::ANY,
        gateway,
        iface: String::from(iface),
        flags: RTF_UP | RTF_GATEWAY | RTF_DEFAULT,
        metric: 100,
    });

    ROUTE_CACHE.lock().clear();
}

// ---------------------------------------------------------------------------
// Connected routes
// ---------------------------------------------------------------------------

/// Add a connected route when an interface is configured with an IP.
/// This creates a route for the directly-connected subnet.
pub fn add_connected(ip: Ipv4Addr, netmask: Ipv4Addr, iface: &str) {
    let network = Ipv4Addr::from_u32(ip.to_u32() & netmask.to_u32());
    let route = Route {
        destination: network,
        netmask,
        gateway: Ipv4Addr::ANY,
        iface: String::from(iface),
        flags: RTF_UP | RTF_CONNECTED,
        metric: 0,
    };
    let _ = add(route);

    // Also add a local/host route for the IP itself
    let host_route = Route {
        destination: ip,
        netmask: Ipv4Addr::new(255, 255, 255, 255),
        gateway: Ipv4Addr::ANY,
        iface: String::from(iface),
        flags: RTF_UP | RTF_LOCAL | RTF_HOST,
        metric: 0,
    };
    let _ = add(host_route);

    serial_println!(
        "  Routing: connected route {}/{} via {}",
        network,
        netmask,
        iface
    );
}

/// Remove connected routes for an interface.
pub fn remove_connected(iface: &str) {
    ROUTES
        .lock()
        .retain(|r| !(r.is_connected() && r.iface == iface));
    ROUTE_CACHE.lock().clear();
}

// ---------------------------------------------------------------------------
// Policy routing
// ---------------------------------------------------------------------------

/// Add a policy routing rule.
pub fn add_policy_rule(rule: PolicyRule) -> Result<(), &'static str> {
    let mut rules = POLICY_RULES.lock();
    if rules.len() >= MAX_POLICY_RULES {
        return Err("Policy rule table full");
    }
    rules.push(rule);
    // Sort by priority
    rules.sort_by_key(|r| r.priority);
    ROUTE_CACHE.lock().clear();
    Ok(())
}

/// Remove a policy rule matching a source network.
pub fn remove_policy_rule(src_network: Ipv4Addr, src_mask: Ipv4Addr) {
    POLICY_RULES
        .lock()
        .retain(|r| !(r.src_network.0 == src_network.0 && r.src_mask.0 == src_mask.0));
    ROUTE_CACHE.lock().clear();
}

/// List all policy rules.
pub fn list_policy_rules() -> Vec<PolicyRule> {
    POLICY_RULES.lock().clone()
}

/// Flush all policy rules.
pub fn flush_policy_rules() {
    POLICY_RULES.lock().clear();
    ROUTE_CACHE.lock().clear();
}

// ---------------------------------------------------------------------------
// Route cache management
// ---------------------------------------------------------------------------

/// Flush the route cache.
pub fn cache_flush() {
    ROUTE_CACHE.lock().clear();
}

/// Remove expired entries from the route cache.
pub fn cache_gc() {
    let now = crate::time::clock::uptime_ms();
    ROUTE_CACHE
        .lock()
        .retain(|_, entry| now.saturating_sub(entry.created_tick) < CACHE_TTL_MS);
}

/// Get route cache size.
pub fn cache_size() -> usize {
    ROUTE_CACHE.lock().len()
}

// ---------------------------------------------------------------------------
// Display / diagnostics
// ---------------------------------------------------------------------------

/// Get all routes (for display)
pub fn list() -> Vec<Route> {
    ROUTES.lock().clone()
}

/// Get the number of routes.
pub fn route_count() -> usize {
    ROUTES.lock().len()
}

/// Format routing table for display
pub fn format_table() -> String {
    let routes = list();
    let mut out =
        String::from("Destination     Gateway         Netmask         Flags  Metric Iface\n");
    for r in &routes {
        let flags_str = {
            let mut f = String::new();
            if r.flags & RTF_UP != 0 {
                f.push('U');
            }
            if r.flags & RTF_GATEWAY != 0 {
                f.push('G');
            }
            if r.flags & RTF_HOST != 0 {
                f.push('H');
            }
            if r.flags & RTF_DEFAULT != 0 {
                f.push('D');
            }
            if r.flags & RTF_STATIC != 0 {
                f.push('S');
            }
            if r.flags & RTF_CONNECTED != 0 {
                f.push('C');
            }
            if r.flags & RTF_REJECT != 0 {
                f.push('!');
            }
            if r.flags & RTF_LOCAL != 0 {
                f.push('L');
            }
            f
        };
        out.push_str(&alloc::format!(
            "{:<15} {:<15} {:<15} {:<6} {:>6} {}\n",
            r.destination,
            r.gateway,
            r.netmask,
            flags_str,
            r.metric,
            r.iface
        ));
    }
    out
}

/// Format policy rules for display.
pub fn format_policy_rules() -> String {
    let rules = list_policy_rules();
    if rules.is_empty() {
        return String::from("No policy routing rules configured.\n");
    }
    let mut out =
        String::from("Priority  Source          Mask            Gateway         Iface   Active\n");
    for r in &rules {
        out.push_str(&alloc::format!(
            "{:<9} {:<15} {:<15} {:<15} {:<7} {}\n",
            r.priority,
            r.src_network,
            r.src_mask,
            r.gateway,
            r.iface,
            if r.enabled { "yes" } else { "no" }
        ));
    }
    out
}
