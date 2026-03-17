/// Socket-based service activation (systemd socket activation equivalent)
///
/// Part of the AIOS init_system subsystem.
///
/// Listens on configured sockets (ports) and lazily starts the associated
/// service when the first connection arrives. Supports TCP/UDP port
/// listeners, connection queueing during service startup, and automatic
/// deactivation after idle timeout.
///
/// Original implementation for Hoags OS. No external crates.

use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── FNV-1a helper ──────────────────────────────────────────────────────────

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ── TSC helpers ────────────────────────────────────────────────────────────

fn read_tsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

const TSC_PER_MS: u64 = 2_000_000;

fn ms_to_tsc(ms: u64) -> u64 {
    ms.saturating_mul(TSC_PER_MS)
}

// ── Socket protocol ────────────────────────────────────────────────────────

/// Protocol type for a socket listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketProtocol {
    Tcp,
    Udp,
    Unix,
}

// ── Listener state ─────────────────────────────────────────────────────────

/// State of a socket listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerState {
    /// Listening for connections, service not yet started.
    Listening,
    /// Connection received, service is starting.
    Activating,
    /// Service is running, passing through connections.
    Active,
    /// Idle timeout expired, returning to listening.
    Idle,
    /// Listener is disabled.
    Disabled,
}

// ── Socket listener entry ──────────────────────────────────────────────────

/// A managed socket listener that activates a service on connection.
#[derive(Clone)]
struct SocketListener {
    /// Port number to listen on.
    port: u16,
    /// Protocol (TCP, UDP, Unix).
    protocol: SocketProtocol,
    /// Name of the service to activate.
    service_name: String,
    service_hash: u64,
    /// Current state.
    state: ListenerState,
    /// Number of connections received while waiting for service activation.
    queued_connections: u32,
    /// Maximum queue depth before dropping connections.
    max_queue: u32,
    /// Total connections handled.
    total_connections: u64,
    /// Idle timeout in TSC ticks (0 = no idle shutdown).
    idle_timeout_tsc: u64,
    /// TSC of last activity.
    last_activity: u64,
    /// Whether to accept connections after service is running.
    pass_through: bool,
}

// ── Socket activation manager ──────────────────────────────────────────────

struct SocketActivationInner {
    listeners: Vec<SocketListener>,
    /// Services that need to be started.
    activation_queue: Vec<String>,
}

impl SocketActivationInner {
    fn new() -> Self {
        SocketActivationInner {
            listeners: Vec::new(),
            activation_queue: Vec::new(),
        }
    }

    /// Register a socket that will activate a service on connection.
    fn register(
        &mut self,
        port: u16,
        protocol: SocketProtocol,
        service_name: &str,
    ) -> usize {
        // Check for duplicate port
        for (i, l) in self.listeners.iter().enumerate() {
            if l.port == port && l.protocol == protocol {
                serial_println!(
                    "[init_system::socket_activation] port {} already registered",
                    port
                );
                return i;
            }
        }

        let idx = self.listeners.len();
        let now = read_tsc();

        self.listeners.push(SocketListener {
            port,
            protocol,
            service_name: String::from(service_name),
            service_hash: fnv1a_hash(service_name.as_bytes()),
            state: ListenerState::Listening,
            queued_connections: 0,
            max_queue: 128,
            total_connections: 0,
            idle_timeout_tsc: ms_to_tsc(300_000), // 5 minutes default
            last_activity: now,
            pass_through: true,
        });

        serial_println!(
            "[init_system::socket_activation] registered port {} ({:?}) -> {}",
            port, protocol, service_name
        );

        idx
    }

    /// Handle an incoming connection on a port.
    fn on_connection(&mut self, port: u16) {
        for listener in self.listeners.iter_mut() {
            if listener.port != port {
                continue;
            }

            let now = read_tsc();
            listener.last_activity = now;
            listener.total_connections = listener.total_connections.saturating_add(1);

            match listener.state {
                ListenerState::Listening | ListenerState::Idle => {
                    // First connection: activate the service
                    serial_println!(
                        "[init_system::socket_activation] connection on port {}, activating {}",
                        port, listener.service_name
                    );
                    listener.state = ListenerState::Activating;
                    listener.queued_connections = 1;
                    self.activation_queue.push(listener.service_name.clone());
                }
                ListenerState::Activating => {
                    // Service still starting, queue the connection
                    if listener.queued_connections < listener.max_queue {
                        listener.queued_connections = listener.queued_connections.saturating_add(1);
                    } else {
                        serial_println!(
                            "[init_system::socket_activation] port {} queue full, dropping",
                            port
                        );
                    }
                }
                ListenerState::Active => {
                    // Service running, pass through
                    // Nothing to do here; the service handles it directly.
                }
                ListenerState::Disabled => {
                    // Ignore
                }
            }

            return;
        }
    }

    /// Notify that a service has started successfully.
    fn on_service_started(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for listener in self.listeners.iter_mut() {
            if listener.service_hash == hash && listener.state == ListenerState::Activating {
                listener.state = ListenerState::Active;
                serial_println!(
                    "[init_system::socket_activation] {} activated, {} queued connections",
                    service, listener.queued_connections
                );
                listener.queued_connections = 0;
            }
        }
    }

    /// Notify that a service has stopped.
    fn on_service_stopped(&mut self, service: &str) {
        let hash = fnv1a_hash(service.as_bytes());
        for listener in self.listeners.iter_mut() {
            if listener.service_hash == hash {
                listener.state = ListenerState::Listening;
                listener.queued_connections = 0;
                serial_println!(
                    "[init_system::socket_activation] {} stopped, returning port {} to listening",
                    service, listener.port
                );
            }
        }
    }

    /// Check for idle listeners and deactivate their services.
    fn check_idle(&mut self) {
        let now = read_tsc();

        for listener in self.listeners.iter_mut() {
            if listener.state != ListenerState::Active {
                continue;
            }

            if listener.idle_timeout_tsc == 0 {
                continue; // no idle timeout configured
            }

            let idle_duration = now.saturating_sub(listener.last_activity);
            if idle_duration > listener.idle_timeout_tsc {
                serial_println!(
                    "[init_system::socket_activation] {} idle on port {}, deactivating",
                    listener.service_name, listener.port
                );
                listener.state = ListenerState::Idle;
                // Caller should stop the service
            }
        }
    }

    /// Drain the activation queue (services to start).
    fn drain_activations(&mut self) -> Vec<String> {
        let result = self.activation_queue.clone();
        self.activation_queue.clear();
        result
    }

    /// Get the service name associated with a port.
    fn service_for_port(&self, port: u16) -> Option<&str> {
        self.listeners.iter()
            .find(|l| l.port == port)
            .map(|l| l.service_name.as_str())
    }

    /// Disable a listener.
    fn disable(&mut self, port: u16) {
        for listener in self.listeners.iter_mut() {
            if listener.port == port {
                listener.state = ListenerState::Disabled;
                serial_println!(
                    "[init_system::socket_activation] disabled listener on port {}",
                    port
                );
                return;
            }
        }
    }

    /// Enable a previously disabled listener.
    fn enable(&mut self, port: u16) {
        for listener in self.listeners.iter_mut() {
            if listener.port == port && listener.state == ListenerState::Disabled {
                listener.state = ListenerState::Listening;
                listener.last_activity = read_tsc();
                serial_println!(
                    "[init_system::socket_activation] enabled listener on port {}",
                    port
                );
                return;
            }
        }
    }

    /// Set idle timeout for a listener.
    fn set_idle_timeout(&mut self, port: u16, timeout_ms: u64) {
        for listener in self.listeners.iter_mut() {
            if listener.port == port {
                listener.idle_timeout_tsc = ms_to_tsc(timeout_ms);
                return;
            }
        }
    }

    /// Get count of active (listening or active) sockets.
    fn active_count(&self) -> usize {
        self.listeners.iter()
            .filter(|l| l.state != ListenerState::Disabled)
            .count()
    }

    /// Get total connections handled across all listeners.
    fn total_connections(&self) -> u64 {
        self.listeners.iter().map(|l| l.total_connections).sum()
    }
}

/// Public wrapper matching original stub API.
pub struct SocketActivation {
    inner: SocketActivationInner,
}

impl SocketActivation {
    pub fn new() -> Self {
        SocketActivation {
            inner: SocketActivationInner::new(),
        }
    }

    pub fn register(&mut self, port: u16, service_name: &str) {
        self.inner.register(port, SocketProtocol::Tcp, service_name);
    }

    pub fn on_connection(&mut self, port: u16) {
        self.inner.on_connection(port);
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static SOCKET_ACT: Mutex<Option<SocketActivationInner>> = Mutex::new(None);

/// Initialize the socket activation subsystem.
pub fn init() {
    let mut guard = SOCKET_ACT.lock();
    *guard = Some(SocketActivationInner::new());
    serial_println!("[init_system::socket_activation] socket activation initialized");
}

/// Register a socket listener.
pub fn register(port: u16, protocol: SocketProtocol, service: &str) -> usize {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.register(port, protocol, service)
}

/// Handle an incoming connection on a port.
pub fn on_connection(port: u16) {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.on_connection(port);
}

/// Notify that a service has started.
pub fn on_service_started(service: &str) {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.on_service_started(service);
}

/// Notify that a service has stopped.
pub fn on_service_stopped(service: &str) {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.on_service_stopped(service);
}

/// Check for idle services and deactivate them.
pub fn check_idle() {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.check_idle();
}

/// Drain pending service activations.
pub fn drain_activations() -> Vec<String> {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.drain_activations()
}

/// Disable a listener.
pub fn disable(port: u16) {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.disable(port);
}

/// Enable a previously disabled listener.
pub fn enable(port: u16) {
    let mut guard = SOCKET_ACT.lock();
    let mgr = guard.as_mut().expect("socket activation not initialized");
    mgr.enable(port);
}

/// Get count of active listeners.
pub fn active_count() -> usize {
    let guard = SOCKET_ACT.lock();
    let mgr = guard.as_ref().expect("socket activation not initialized");
    mgr.active_count()
}

/// Get total connections handled.
pub fn total_connections() -> u64 {
    let guard = SOCKET_ACT.lock();
    let mgr = guard.as_ref().expect("socket activation not initialized");
    mgr.total_connections()
}
