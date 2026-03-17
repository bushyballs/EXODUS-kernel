pub mod dht;
/// P2P / Mesh Networking Subsystem for Genesis
///
/// Provides decentralized peer-to-peer communication:
///   - mesh_net:       Mesh networking layer (routing, flooding, topology)
///   - dht:            Distributed hash table (Kademlia-style)
///   - peer_discovery: Peer discovery (mDNS, broadcast, BLE, DHT)
///   - relay:          Relay / NAT traversal (STUN, TURN, hole-punching)
///
/// All code is original. No external crates.
pub mod mesh_net;
pub mod peer_discovery;
pub mod relay;

use crate::{serial_print, serial_println};

pub fn init() {
    mesh_net::init();
    dht::init();
    peer_discovery::init();
    relay::init();
    serial_println!("  P2P/mesh networking subsystem initialized");
}
