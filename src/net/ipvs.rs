use crate::sync::Mutex;
/// IP Virtual Server (IPVS) - Kernel-level L4 load balancing
///
/// Provides virtual service management, real server backends with
/// health tracking, scheduling algorithms (round-robin, weighted
/// round-robin, least connections, weighted least connections,
/// source hash), connection tracking, and statistics.
///
/// Inspired by: Linux IPVS (LVS), RFC 2391. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Forwarding method
// ---------------------------------------------------------------------------

/// IPVS forwarding method
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardMethod {
    /// NAT (Network Address Translation) - modify dst IP/port
    Nat,
    /// Direct Routing - rewrite MAC, keep IP (requires DSR-capable backends)
    DirectRouting,
    /// IP Tunneling (IPIP encapsulation)
    Tunnel,
}

// ---------------------------------------------------------------------------
// Scheduling algorithm
// ---------------------------------------------------------------------------

/// Load balancing scheduler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduler {
    /// Round Robin
    RoundRobin,
    /// Weighted Round Robin
    WeightedRoundRobin,
    /// Least Connections
    LeastConnections,
    /// Weighted Least Connections
    WeightedLeastConnections,
    /// Source IP Hash (sticky sessions)
    SourceHash,
}

// ---------------------------------------------------------------------------
// Real server (backend)
// ---------------------------------------------------------------------------

/// Health state of a real server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// Server is healthy and accepting traffic
    Up,
    /// Server is unhealthy (failed health checks)
    Down,
    /// Server is being drained (no new connections)
    Draining,
}

/// Real server (backend)
#[derive(Debug, Clone)]
pub struct RealServer {
    pub addr: [u8; 4],
    pub port: u16,
    pub weight: u32,
    pub health: HealthState,
    /// Active connections
    pub active_conns: u32,
    /// Inactive (time-wait) connections
    pub inactive_conns: u32,
    /// Total connections served
    pub total_conns: u64,
    /// Total bytes in
    pub bytes_in: u64,
    /// Total bytes out
    pub bytes_out: u64,
    /// Total packets in
    pub pkts_in: u64,
    /// Total packets out
    pub pkts_out: u64,
    /// Consecutive failed health checks
    pub failed_checks: u32,
    /// Maximum failed checks before marking down
    pub max_failed: u32,
}

impl RealServer {
    pub fn new(addr: [u8; 4], port: u16, weight: u32) -> Self {
        RealServer {
            addr,
            port,
            weight,
            health: HealthState::Up,
            active_conns: 0,
            inactive_conns: 0,
            total_conns: 0,
            bytes_in: 0,
            bytes_out: 0,
            pkts_in: 0,
            pkts_out: 0,
            failed_checks: 0,
            max_failed: 3,
        }
    }

    /// Check if server is available for new connections
    pub fn is_available(&self) -> bool {
        self.health == HealthState::Up && self.weight > 0
    }

    /// Record a successful health check
    pub fn health_check_pass(&mut self) {
        self.failed_checks = 0;
        if self.health == HealthState::Down {
            self.health = HealthState::Up;
        }
    }

    /// Record a failed health check
    pub fn health_check_fail(&mut self) {
        self.failed_checks = self.failed_checks.saturating_add(1);
        if self.failed_checks >= self.max_failed {
            self.health = HealthState::Down;
        }
    }

    /// Mark server as draining (finish existing, no new)
    pub fn drain(&mut self) {
        self.health = HealthState::Draining;
    }
}

// ---------------------------------------------------------------------------
// Connection entry
// ---------------------------------------------------------------------------

/// IPVS connection tracking entry
#[derive(Debug, Clone)]
pub struct IpvsConnection {
    /// Client address
    pub client_addr: [u8; 4],
    pub client_port: u16,
    /// Virtual address
    pub virt_addr: [u8; 4],
    pub virt_port: u16,
    /// Real server address
    pub real_addr: [u8; 4],
    pub real_port: u16,
    /// Protocol (6=TCP, 17=UDP)
    pub protocol: u8,
    /// Remaining TTL in ticks
    pub ttl: u32,
}

// ---------------------------------------------------------------------------
// Virtual service
// ---------------------------------------------------------------------------

/// Protocol for virtual service
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceProtocol {
    Tcp,
    Udp,
}

/// IPVS virtual service
pub struct VirtualService {
    pub id: u32,
    /// Virtual IP address
    pub vip: [u8; 4],
    /// Virtual port
    pub port: u16,
    /// Protocol
    pub protocol: ServiceProtocol,
    /// Scheduling algorithm
    pub scheduler: Scheduler,
    /// Forwarding method
    pub fwd_method: ForwardMethod,
    /// Backend real servers
    pub backends: Vec<RealServer>,
    /// Connection table
    pub connections: Vec<IpvsConnection>,
    /// Round-robin index
    rr_index: usize,
    /// Weighted round-robin state
    wrr_current_weight: i32,
    wrr_gcd: u32,
    wrr_max_weight: u32,
    /// Stats
    pub total_conns: u64,
    pub total_pkts_in: u64,
    pub total_pkts_out: u64,
    pub total_bytes_in: u64,
    pub total_bytes_out: u64,
}

impl VirtualService {
    pub fn new(
        id: u32,
        vip: [u8; 4],
        port: u16,
        protocol: ServiceProtocol,
        scheduler: Scheduler,
        fwd_method: ForwardMethod,
    ) -> Self {
        VirtualService {
            id,
            vip,
            port,
            protocol,
            scheduler,
            fwd_method,
            backends: Vec::new(),
            connections: Vec::new(),
            rr_index: 0,
            wrr_current_weight: 0,
            wrr_gcd: 1,
            wrr_max_weight: 0,
            total_conns: 0,
            total_pkts_in: 0,
            total_pkts_out: 0,
            total_bytes_in: 0,
            total_bytes_out: 0,
        }
    }

    /// Add a backend real server
    pub fn add_backend(&mut self, server: RealServer) {
        self.backends.push(server);
        self.recalc_wrr();
    }

    /// Remove a backend by address
    pub fn remove_backend(&mut self, addr: [u8; 4], port: u16) {
        self.backends
            .retain(|s| !(s.addr == addr && s.port == port));
        self.recalc_wrr();
    }

    /// Recalculate WRR parameters
    fn recalc_wrr(&mut self) {
        if self.backends.is_empty() {
            self.wrr_gcd = 1;
            self.wrr_max_weight = 0;
            return;
        }
        let weights: Vec<u32> = self.backends.iter().map(|s| s.weight).collect();
        self.wrr_max_weight = *weights.iter().max().unwrap_or(&1);
        self.wrr_gcd = weights.iter().fold(0u32, |acc, &w| gcd(acc, w));
        if self.wrr_gcd == 0 {
            self.wrr_gcd = 1;
        }
    }

    /// Select a backend using the configured scheduler
    pub fn select_backend(&mut self, client_addr: [u8; 4]) -> Option<usize> {
        // Check for existing connection (sticky)
        if self.scheduler == Scheduler::SourceHash {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.client_addr == client_addr)
            {
                let real = conn.real_addr;
                let real_port = conn.real_port;
                if let Some(idx) = self
                    .backends
                    .iter()
                    .position(|s| s.addr == real && s.port == real_port && s.is_available())
                {
                    return Some(idx);
                }
            }
        }

        let available: Vec<usize> = self
            .backends
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_available())
            .map(|(i, _)| i)
            .collect();
        if available.is_empty() {
            return None;
        }

        match self.scheduler {
            Scheduler::RoundRobin => {
                let idx = available[self.rr_index % available.len()];
                self.rr_index = self.rr_index.wrapping_add(1);
                Some(idx)
            }
            Scheduler::WeightedRoundRobin => self.select_wrr(&available),
            Scheduler::LeastConnections => {
                let mut best = available[0];
                let mut best_conns = self.backends[best].active_conns;
                for &idx in &available[1..] {
                    if self.backends[idx].active_conns < best_conns {
                        best = idx;
                        best_conns = self.backends[idx].active_conns;
                    }
                }
                Some(best)
            }
            Scheduler::WeightedLeastConnections => {
                // Select server with lowest (active_conns / weight)
                let mut best = available[0];
                let mut best_ratio =
                    ratio(self.backends[best].active_conns, self.backends[best].weight);
                for &idx in &available[1..] {
                    let r = ratio(self.backends[idx].active_conns, self.backends[idx].weight);
                    if r < best_ratio {
                        best = idx;
                        best_ratio = r;
                    }
                }
                Some(best)
            }
            Scheduler::SourceHash => {
                // Simple hash of source IP
                let hash = source_hash(client_addr);
                let idx = available[hash % available.len()];
                Some(idx)
            }
        }
    }

    /// Weighted round-robin selection
    fn select_wrr(&mut self, available: &[usize]) -> Option<usize> {
        if available.is_empty() {
            return None;
        }
        // Iterate through backends with WRR algorithm
        for _ in 0..self.backends.len() {
            self.rr_index = (self.rr_index + 1) % self.backends.len();
            if self.rr_index == 0 {
                self.wrr_current_weight -= self.wrr_gcd as i32;
                if self.wrr_current_weight <= 0 {
                    self.wrr_current_weight = self.wrr_max_weight as i32;
                }
            }
            if available.contains(&self.rr_index)
                && self.backends[self.rr_index].weight as i32 >= self.wrr_current_weight
            {
                return Some(self.rr_index);
            }
        }
        // Fallback to first available
        Some(available[0])
    }

    /// Create a connection tracking entry
    pub fn create_connection(
        &mut self,
        client_addr: [u8; 4],
        client_port: u16,
        backend_idx: usize,
        ttl: u32,
    ) {
        if backend_idx >= self.backends.len() {
            return;
        }
        let rs = &mut self.backends[backend_idx];
        rs.active_conns = rs.active_conns.saturating_add(1);
        rs.total_conns = rs.total_conns.saturating_add(1);
        self.total_conns = self.total_conns.saturating_add(1);

        self.connections.push(IpvsConnection {
            client_addr,
            client_port,
            virt_addr: self.vip,
            virt_port: self.port,
            real_addr: rs.addr,
            real_port: rs.port,
            protocol: match self.protocol {
                ServiceProtocol::Tcp => 6,
                ServiceProtocol::Udp => 17,
            },
            ttl,
        });
    }

    /// Expire old connections
    pub fn expire_connections(&mut self) {
        let backends = &mut self.backends;
        self.connections.retain_mut(|conn| {
            if conn.ttl > 0 {
                conn.ttl = conn.ttl.saturating_sub(1);
                true
            } else {
                // Decrement active_conns on the backend
                if let Some(rs) = backends
                    .iter_mut()
                    .find(|s| s.addr == conn.real_addr && s.port == conn.real_port)
                {
                    rs.active_conns = rs.active_conns.saturating_sub(1);
                    rs.inactive_conns = rs.inactive_conns.saturating_add(1);
                }
                false
            }
        });
    }

    /// Lookup a connection by client address/port
    pub fn lookup_connection(
        &self,
        client_addr: [u8; 4],
        client_port: u16,
    ) -> Option<&IpvsConnection> {
        self.connections
            .iter()
            .find(|c| c.client_addr == client_addr && c.client_port == client_port)
    }
}

/// GCD for weight calculations
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Compute ratio for WLC (returns conns * 1000 / weight to avoid float)
fn ratio(conns: u32, weight: u32) -> u64 {
    if weight == 0 {
        return u64::MAX;
    }
    (conns as u64 * 1000) / weight as u64
}

/// Simple source IP hash
fn source_hash(addr: [u8; 4]) -> usize {
    let mut h: u32 = 2166136261;
    for &b in &addr {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h as usize
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpvsError {
    NotInitialized,
    ServiceNotFound,
    BackendNotFound,
    NoBackendsAvailable,
}

// ---------------------------------------------------------------------------
// Global IPVS subsystem
// ---------------------------------------------------------------------------

struct IpvsSubsystem {
    services: Vec<VirtualService>,
    next_id: u32,
}

static IPVS: Mutex<Option<IpvsSubsystem>> = Mutex::new(None);

pub fn init() {
    *IPVS.lock() = Some(IpvsSubsystem {
        services: Vec::new(),
        next_id: 1,
    });
    serial_println!("  Net: IPVS load balancer initialized");
}

/// Create a virtual service
pub fn create_service(
    vip: [u8; 4],
    port: u16,
    protocol: ServiceProtocol,
    scheduler: Scheduler,
    fwd_method: ForwardMethod,
) -> Result<u32, IpvsError> {
    let mut guard = IPVS.lock();
    let sys = guard.as_mut().ok_or(IpvsError::NotInitialized)?;
    let id = sys.next_id;
    sys.next_id = sys.next_id.saturating_add(1);
    sys.services.push(VirtualService::new(
        id, vip, port, protocol, scheduler, fwd_method,
    ));
    Ok(id)
}

/// Delete a virtual service
pub fn delete_service(service_id: u32) -> Result<(), IpvsError> {
    let mut guard = IPVS.lock();
    let sys = guard.as_mut().ok_or(IpvsError::NotInitialized)?;
    let pos = sys
        .services
        .iter()
        .position(|s| s.id == service_id)
        .ok_or(IpvsError::ServiceNotFound)?;
    sys.services.remove(pos);
    Ok(())
}

/// Add a backend to a virtual service
pub fn add_backend(
    service_id: u32,
    addr: [u8; 4],
    port: u16,
    weight: u32,
) -> Result<(), IpvsError> {
    let mut guard = IPVS.lock();
    let sys = guard.as_mut().ok_or(IpvsError::NotInitialized)?;
    let svc = sys
        .services
        .iter_mut()
        .find(|s| s.id == service_id)
        .ok_or(IpvsError::ServiceNotFound)?;
    svc.add_backend(RealServer::new(addr, port, weight));
    Ok(())
}

/// Remove a backend from a virtual service
pub fn remove_backend(service_id: u32, addr: [u8; 4], port: u16) -> Result<(), IpvsError> {
    let mut guard = IPVS.lock();
    let sys = guard.as_mut().ok_or(IpvsError::NotInitialized)?;
    let svc = sys
        .services
        .iter_mut()
        .find(|s| s.id == service_id)
        .ok_or(IpvsError::ServiceNotFound)?;
    svc.remove_backend(addr, port);
    Ok(())
}

/// Schedule a connection (select a backend for a client)
pub fn schedule(
    service_id: u32,
    client_addr: [u8; 4],
    client_port: u16,
) -> Result<([u8; 4], u16), IpvsError> {
    let mut guard = IPVS.lock();
    let sys = guard.as_mut().ok_or(IpvsError::NotInitialized)?;
    let svc = sys
        .services
        .iter_mut()
        .find(|s| s.id == service_id)
        .ok_or(IpvsError::ServiceNotFound)?;

    // Check existing connection
    if let Some(conn) = svc.lookup_connection(client_addr, client_port) {
        return Ok((conn.real_addr, conn.real_port));
    }

    // Select a new backend
    let idx = svc
        .select_backend(client_addr)
        .ok_or(IpvsError::NoBackendsAvailable)?;
    let addr = svc.backends[idx].addr;
    let port = svc.backends[idx].port;
    svc.create_connection(client_addr, client_port, idx, 300); // 5-minute TTL
    Ok((addr, port))
}

/// Tick: expire connections, run health checks
pub fn tick() {
    let mut guard = IPVS.lock();
    if let Some(sys) = guard.as_mut() {
        for svc in &mut sys.services {
            svc.expire_connections();
        }
    }
}
