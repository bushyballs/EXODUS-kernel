use super::mdns::{self, ServiceInstance, MDNS_PORT};
/// DNS Service Discovery (DNS-SD, RFC 6763)
///
/// High-level convenience API built on top of the mDNS subsystem.
/// Provides well-known service type constants, one-call service announcement,
/// and network discovery for the Genesis platform.
///
/// Design constraints (bare-metal kernel rules):
///   - no_std — no standard library
///   - No heap — no Vec / Box / String — fixed-size static arrays only
///   - No float casts (as f32 / as f64)
///   - Saturating arithmetic; wrapping_add on sequences
///   - No panic — all fallible paths return early or log + return
///
/// Inspired by: RFC 6763 (DNS-SD), Apple Bonjour. All code is original.
use crate::serial_println;

// ---------------------------------------------------------------------------
// Well-known service type strings (RFC 6763 §7, IANA registry)
// ---------------------------------------------------------------------------

/// HTTP over TCP (RFC 7230)
pub const SVC_HTTP: &str = "_http._tcp.local";
/// HTTPS over TCP
pub const SVC_HTTPS: &str = "_https._tcp.local";
/// SSH over TCP (RFC 4251)
pub const SVC_SSH: &str = "_ssh._tcp.local";
/// SMB/CIFS file sharing (Microsoft)
pub const SVC_SMB: &str = "_smb._tcp.local";
/// AFP over TCP (Apple Filing Protocol)
pub const SVC_AFP: &str = "_afpovertcp._tcp.local";
/// SFTP over SSH
pub const SVC_SFTP: &str = "_sftp-ssh._tcp.local";
/// NFS v4 over TCP
pub const SVC_NFS: &str = "_nfs._tcp.local";
/// Genesis API — custom service type for the Genesis AI OS
pub const SVC_GENESIS_API: &str = "_genesis-api._tcp.local";

// ---------------------------------------------------------------------------
// Discover services of a given type
// ---------------------------------------------------------------------------

/// Discover services of `service_type` on the local link.
///
/// Sends a PTR query and collects up to 8 responses.
/// Returns an array where `Some(ServiceInstance)` entries are valid results.
///
/// # Example
/// ```
/// let results = dns_sd::discover(dns_sd::SVC_HTTP);
/// for inst in results.iter().flatten() {
///     // use inst.host, inst.port, …
/// }
/// ```
pub fn discover(service_type: &str) -> [Option<ServiceInstance>; 8] {
    mdns::query_service(service_type)
}

// ---------------------------------------------------------------------------
// Announce Genesis platform services
// ---------------------------------------------------------------------------

/// Announce the standard Genesis AI OS services on the local link.
///
/// Registers `_genesis-api._tcp.local` on port 8080 with version and
/// device TXT attributes, so that other nodes on the subnet can discover
/// this Genesis instance without prior configuration.
pub fn announce_genesis_services() {
    let txt: &[(&str, &str)] = &[("version", "1"), ("device", "genesis"), ("api", "http")];

    let ok = mdns::register_service(
        "Genesis AI OS", // instance name
        "_genesis-api._tcp.local",
        8080,
        txt,
    );

    if ok {
        serial_println!("  DNS-SD: announced _genesis-api._tcp.local :8080");
    } else {
        serial_println!("  DNS-SD: WARN — could not register genesis service (record store full?)");
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the DNS-SD subsystem and announce platform services.
pub fn init() {
    announce_genesis_services();
    serial_println!("  Net: DNS-SD subsystem initialized");
}
