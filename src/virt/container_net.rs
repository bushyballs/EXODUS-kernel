use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Container networking for Genesis
///
/// Provides virtual bridge networking, veth pair management, NAT/masquerade,
/// port forwarding rules, and a CNI (Container Network Interface) plugin
/// abstraction for container-to-container and container-to-host communication.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_BRIDGES: usize = 16;
const MAX_VETH_PAIRS: usize = 128;
const MAX_NAT_RULES: usize = 256;
const MAX_PORT_FORWARDS: usize = 256;
const MAX_CNI_PLUGINS: usize = 16;
const MAX_ENDPOINTS_PER_BRIDGE: usize = 64;

const DEFAULT_BRIDGE_SUBNET: u32 = 0xAC110000; // 172.17.0.0
const DEFAULT_BRIDGE_MASK: u32 = 0xFFFF0000; // /16
const DEFAULT_BRIDGE_GW: u32 = 0xAC110001; // 172.17.0.1
const DEFAULT_MTU: u16 = 1500;

// ---------------------------------------------------------------------------
// MAC address helpers (Q16 not applicable; use u64 packed)
// ---------------------------------------------------------------------------

fn generate_mac(bridge_id: u32, endpoint_id: u32) -> u64 {
    // 02:42:ac:11:XX:YY format (Docker-style locally administered MAC)
    let prefix: u64 = 0x0242AC110000;
    let suffix = ((bridge_id as u64 & 0xFF) << 8) | (endpoint_id as u64 & 0xFF);
    prefix | suffix
}

// ---------------------------------------------------------------------------
// Virtual bridge
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BridgeState {
    Down,
    Up,
    Error,
}

#[derive(Clone, Copy)]
pub struct BridgeEndpoint {
    pub container_id: u32,
    pub veth_pair_id: u32,
    pub ip_address: u32,
    pub mac_address: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_packets: u32,
    pub tx_packets: u32,
    pub active: bool,
}

#[derive(Clone)]
pub struct VirtualBridge {
    pub id: u32,
    pub name_hash: u64,
    pub state: BridgeState,
    pub subnet: u32,
    pub subnet_mask: u32,
    pub gateway: u32,
    pub mtu: u16,
    pub endpoints: Vec<BridgeEndpoint>,
    pub next_ip_offset: u32,
    pub enable_icc: bool, // inter-container communication
    pub enable_masquerade: bool,
    pub promiscuous: bool,
}

impl VirtualBridge {
    fn new(id: u32, name_hash: u64, subnet: u32, subnet_mask: u32, gateway: u32, mtu: u16) -> Self {
        Self {
            id,
            name_hash,
            state: BridgeState::Down,
            subnet,
            subnet_mask,
            gateway,
            mtu,
            endpoints: Vec::new(),
            next_ip_offset: 2, // .1 = gateway, start allocating at .2
            enable_icc: true,
            enable_masquerade: true,
            promiscuous: false,
        }
    }

    fn allocate_ip(&mut self) -> Result<u32, &'static str> {
        let host_bits = !self.subnet_mask;
        if self.next_ip_offset >= host_bits {
            return Err("Subnet address space exhausted");
        }
        let ip = self.subnet | self.next_ip_offset;
        self.next_ip_offset = self.next_ip_offset.saturating_add(1);
        Ok(ip)
    }

    fn attach_container(
        &mut self,
        container_id: u32,
        veth_pair_id: u32,
    ) -> Result<u32, &'static str> {
        if self.endpoints.len() >= MAX_ENDPOINTS_PER_BRIDGE {
            return Err("Bridge endpoint limit reached");
        }

        let ip = self.allocate_ip()?;
        let mac = generate_mac(self.id, self.endpoints.len() as u32);

        let endpoint = BridgeEndpoint {
            container_id,
            veth_pair_id,
            ip_address: ip,
            mac_address: mac,
            rx_bytes: 0,
            tx_bytes: 0,
            rx_packets: 0,
            tx_packets: 0,
            active: true,
        };

        self.endpoints.push(endpoint);
        Ok(ip)
    }

    fn detach_container(&mut self, container_id: u32) -> Result<(), &'static str> {
        let endpoint = self
            .endpoints
            .iter_mut()
            .find(|e| e.container_id == container_id)
            .ok_or("Container not attached to bridge")?;
        endpoint.active = false;
        Ok(())
    }

    fn active_endpoint_count(&self) -> usize {
        self.endpoints.iter().filter(|e| e.active).count()
    }

    fn get_endpoint(&self, container_id: u32) -> Option<&BridgeEndpoint> {
        self.endpoints
            .iter()
            .find(|e| e.container_id == container_id && e.active)
    }
}

// ---------------------------------------------------------------------------
// Virtual ethernet pair
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VethState {
    Created,
    Up,
    Down,
}

#[derive(Clone, Copy)]
pub struct VethPair {
    pub id: u32,
    pub host_iface_hash: u64,
    pub container_iface_hash: u64,
    pub host_mac: u64,
    pub container_mac: u64,
    pub mtu: u16,
    pub state: VethState,
    pub bridge_id: u32,
    pub container_id: u32,
}

// ---------------------------------------------------------------------------
// NAT rules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NatType {
    Masquerade,
    Snat,
    Dnat,
}

#[derive(Clone, Copy)]
pub struct NatRule {
    pub id: u32,
    pub nat_type: NatType,
    pub source_ip: u32,
    pub source_mask: u32,
    pub dest_ip: u32,
    pub dest_mask: u32,
    pub translated_ip: u32,
    pub interface_hash: u64,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Port forwarding
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PortProto {
    Tcp,
    Udp,
    Both,
}

#[derive(Clone, Copy)]
pub struct PortForward {
    pub id: u32,
    pub host_ip: u32,
    pub host_port: u16,
    pub container_ip: u32,
    pub container_port: u16,
    pub protocol: PortProto,
    pub container_id: u32,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// CNI plugin interface
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CniVersion {
    V0_3_1,
    V0_4_0,
    V1_0_0,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CniPluginType {
    Bridge,
    Ptp,
    Host,
    Macvlan,
    Ipvlan,
    Loopback,
    Bandwidth,
    Firewall,
    PortMap,
    Tuning,
}

#[derive(Clone, Copy)]
pub struct CniPlugin {
    pub id: u32,
    pub name_hash: u64,
    pub plugin_type: CniPluginType,
    pub version: CniVersion,
    pub config_hash: u64,
    pub enabled: bool,
    pub invoke_count: u32,
    pub error_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CniOperation {
    Add,
    Del,
    Check,
    Version,
}

// ---------------------------------------------------------------------------
// Container network manager
// ---------------------------------------------------------------------------

pub struct ContainerNetManager {
    bridges: Vec<VirtualBridge>,
    veth_pairs: Vec<VethPair>,
    nat_rules: Vec<NatRule>,
    port_forwards: Vec<PortForward>,
    cni_plugins: Vec<CniPlugin>,
    next_bridge_id: u32,
    next_veth_id: u32,
    next_nat_id: u32,
    next_forward_id: u32,
    next_plugin_id: u32,
    total_packets_routed: u64,
}

impl ContainerNetManager {
    fn new() -> Self {
        Self {
            bridges: Vec::new(),
            veth_pairs: Vec::new(),
            nat_rules: Vec::new(),
            port_forwards: Vec::new(),
            cni_plugins: Vec::new(),
            next_bridge_id: 1,
            next_veth_id: 1,
            next_nat_id: 1,
            next_forward_id: 1,
            next_plugin_id: 1,
            total_packets_routed: 0,
        }
    }

    // -- Bridge management --------------------------------------------------

    pub fn create_bridge(
        &mut self,
        name_hash: u64,
        subnet: u32,
        subnet_mask: u32,
        gateway: u32,
        mtu: u16,
    ) -> Result<u32, &'static str> {
        if self.bridges.len() >= MAX_BRIDGES {
            return Err("Bridge limit reached");
        }

        let id = self.next_bridge_id;
        self.next_bridge_id = self.next_bridge_id.saturating_add(1);

        self.bridges.push(VirtualBridge::new(
            id,
            name_hash,
            subnet,
            subnet_mask,
            gateway,
            mtu,
        ));
        Ok(id)
    }

    pub fn create_default_bridge(&mut self) -> Result<u32, &'static str> {
        self.create_bridge(
            0x646F636B657230, // "docker0" hash
            DEFAULT_BRIDGE_SUBNET,
            DEFAULT_BRIDGE_MASK,
            DEFAULT_BRIDGE_GW,
            DEFAULT_MTU,
        )
    }

    pub fn bring_bridge_up(&mut self, bridge_id: u32) -> Result<(), &'static str> {
        let bridge = self
            .bridges
            .iter_mut()
            .find(|b| b.id == bridge_id)
            .ok_or("Bridge not found")?;

        bridge.state = BridgeState::Up;
        Ok(())
    }

    pub fn bring_bridge_down(&mut self, bridge_id: u32) -> Result<(), &'static str> {
        let bridge = self
            .bridges
            .iter_mut()
            .find(|b| b.id == bridge_id)
            .ok_or("Bridge not found")?;

        bridge.state = BridgeState::Down;
        Ok(())
    }

    pub fn set_bridge_icc(&mut self, bridge_id: u32, enabled: bool) -> Result<(), &'static str> {
        let bridge = self
            .bridges
            .iter_mut()
            .find(|b| b.id == bridge_id)
            .ok_or("Bridge not found")?;

        bridge.enable_icc = enabled;
        Ok(())
    }

    // -- Veth pair management -----------------------------------------------

    pub fn create_veth_pair(
        &mut self,
        bridge_id: u32,
        container_id: u32,
        host_iface_hash: u64,
        container_iface_hash: u64,
    ) -> Result<(u32, u32), &'static str> {
        if self.veth_pairs.len() >= MAX_VETH_PAIRS {
            return Err("Veth pair limit reached");
        }

        // Validate bridge exists and is up
        let bridge = self
            .bridges
            .iter_mut()
            .find(|b| b.id == bridge_id)
            .ok_or("Bridge not found")?;

        if bridge.state != BridgeState::Up {
            return Err("Bridge not up");
        }

        let veth_id = self.next_veth_id;
        self.next_veth_id = self.next_veth_id.saturating_add(1);

        let host_mac = generate_mac(bridge_id, veth_id * 2);
        let container_mac = generate_mac(bridge_id, veth_id * 2 + 1);

        let veth = VethPair {
            id: veth_id,
            host_iface_hash,
            container_iface_hash,
            host_mac,
            container_mac,
            mtu: bridge.mtu,
            state: VethState::Created,
            bridge_id,
            container_id,
        };

        // Attach to bridge and get IP
        let ip = bridge.attach_container(container_id, veth_id)?;

        self.veth_pairs.push(veth);
        Ok((veth_id, ip))
    }

    pub fn bring_veth_up(&mut self, veth_id: u32) -> Result<(), &'static str> {
        let veth = self
            .veth_pairs
            .iter_mut()
            .find(|v| v.id == veth_id)
            .ok_or("Veth pair not found")?;
        veth.state = VethState::Up;
        Ok(())
    }

    pub fn bring_veth_down(&mut self, veth_id: u32) -> Result<(), &'static str> {
        let veth = self
            .veth_pairs
            .iter_mut()
            .find(|v| v.id == veth_id)
            .ok_or("Veth pair not found")?;
        veth.state = VethState::Down;
        Ok(())
    }

    pub fn destroy_veth_pair(&mut self, veth_id: u32) -> Result<(), &'static str> {
        let idx = self
            .veth_pairs
            .iter()
            .position(|v| v.id == veth_id)
            .ok_or("Veth pair not found")?;

        let veth = &self.veth_pairs[idx];
        let bridge_id = veth.bridge_id;
        let container_id = veth.container_id;

        // Detach from bridge
        if let Some(bridge) = self.bridges.iter_mut().find(|b| b.id == bridge_id) {
            let _ = bridge.detach_container(container_id);
        }

        self.veth_pairs.remove(idx);
        Ok(())
    }

    // -- NAT rules ----------------------------------------------------------

    pub fn add_masquerade(
        &mut self,
        source_subnet: u32,
        source_mask: u32,
        interface_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.nat_rules.len() >= MAX_NAT_RULES {
            return Err("NAT rule limit reached");
        }

        let id = self.next_nat_id;
        self.next_nat_id = self.next_nat_id.saturating_add(1);

        self.nat_rules.push(NatRule {
            id,
            nat_type: NatType::Masquerade,
            source_ip: source_subnet,
            source_mask,
            dest_ip: 0,
            dest_mask: 0,
            translated_ip: 0, // Use outgoing interface address
            interface_hash,
            enabled: true,
        });
        Ok(id)
    }

    pub fn add_snat(
        &mut self,
        source_ip: u32,
        source_mask: u32,
        translated_ip: u32,
        interface_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.nat_rules.len() >= MAX_NAT_RULES {
            return Err("NAT rule limit reached");
        }

        let id = self.next_nat_id;
        self.next_nat_id = self.next_nat_id.saturating_add(1);

        self.nat_rules.push(NatRule {
            id,
            nat_type: NatType::Snat,
            source_ip,
            source_mask,
            dest_ip: 0,
            dest_mask: 0,
            translated_ip,
            interface_hash,
            enabled: true,
        });
        Ok(id)
    }

    pub fn add_dnat(
        &mut self,
        dest_ip: u32,
        dest_mask: u32,
        translated_ip: u32,
        interface_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.nat_rules.len() >= MAX_NAT_RULES {
            return Err("NAT rule limit reached");
        }

        let id = self.next_nat_id;
        self.next_nat_id = self.next_nat_id.saturating_add(1);

        self.nat_rules.push(NatRule {
            id,
            nat_type: NatType::Dnat,
            source_ip: 0,
            source_mask: 0,
            dest_ip,
            dest_mask,
            translated_ip,
            interface_hash,
            enabled: true,
        });
        Ok(id)
    }

    pub fn remove_nat_rule(&mut self, rule_id: u32) -> Result<(), &'static str> {
        let idx = self
            .nat_rules
            .iter()
            .position(|r| r.id == rule_id)
            .ok_or("NAT rule not found")?;
        self.nat_rules.remove(idx);
        Ok(())
    }

    pub fn toggle_nat_rule(&mut self, rule_id: u32, enabled: bool) -> Result<(), &'static str> {
        let rule = self
            .nat_rules
            .iter_mut()
            .find(|r| r.id == rule_id)
            .ok_or("NAT rule not found")?;
        rule.enabled = enabled;
        Ok(())
    }

    // -- Port forwarding ----------------------------------------------------

    pub fn add_port_forward(
        &mut self,
        host_ip: u32,
        host_port: u16,
        container_ip: u32,
        container_port: u16,
        protocol: PortProto,
        container_id: u32,
    ) -> Result<u32, &'static str> {
        if self.port_forwards.len() >= MAX_PORT_FORWARDS {
            return Err("Port forward limit reached");
        }

        // Check for host port conflicts
        let conflict = self.port_forwards.iter().any(|pf| {
            pf.active
                && pf.host_port == host_port
                && pf.host_ip == host_ip
                && (pf.protocol == protocol
                    || pf.protocol == PortProto::Both
                    || protocol == PortProto::Both)
        });
        if conflict {
            return Err("Host port already forwarded");
        }

        let id = self.next_forward_id;
        self.next_forward_id = self.next_forward_id.saturating_add(1);

        self.port_forwards.push(PortForward {
            id,
            host_ip,
            host_port,
            container_ip,
            container_port,
            protocol,
            container_id,
            active: true,
        });
        Ok(id)
    }

    pub fn remove_port_forward(&mut self, forward_id: u32) -> Result<(), &'static str> {
        let idx = self
            .port_forwards
            .iter()
            .position(|pf| pf.id == forward_id)
            .ok_or("Port forward not found")?;
        self.port_forwards.remove(idx);
        Ok(())
    }

    pub fn remove_container_forwards(&mut self, container_id: u32) -> u32 {
        let before = self.port_forwards.len() as u32;
        self.port_forwards
            .retain(|pf| pf.container_id != container_id);
        before - self.port_forwards.len() as u32
    }

    pub fn list_port_forwards(&self) -> Vec<(u32, u16, u32, u16, PortProto)> {
        self.port_forwards
            .iter()
            .filter(|pf| pf.active)
            .map(|pf| {
                (
                    pf.host_ip,
                    pf.host_port,
                    pf.container_ip,
                    pf.container_port,
                    pf.protocol,
                )
            })
            .collect()
    }

    // -- CNI plugin interface -----------------------------------------------

    pub fn register_cni_plugin(
        &mut self,
        name_hash: u64,
        plugin_type: CniPluginType,
        version: CniVersion,
        config_hash: u64,
    ) -> Result<u32, &'static str> {
        if self.cni_plugins.len() >= MAX_CNI_PLUGINS {
            return Err("CNI plugin limit reached");
        }

        let id = self.next_plugin_id;
        self.next_plugin_id = self.next_plugin_id.saturating_add(1);

        self.cni_plugins.push(CniPlugin {
            id,
            name_hash,
            plugin_type,
            version,
            config_hash,
            enabled: true,
            invoke_count: 0,
            error_count: 0,
        });
        Ok(id)
    }

    pub fn invoke_cni_plugin(
        &mut self,
        plugin_id: u32,
        operation: CniOperation,
        _container_id: u32,
    ) -> Result<(), &'static str> {
        let plugin = self
            .cni_plugins
            .iter_mut()
            .find(|p| p.id == plugin_id)
            .ok_or("CNI plugin not found")?;

        if !plugin.enabled {
            return Err("CNI plugin disabled");
        }

        plugin.invoke_count = plugin.invoke_count.saturating_add(1);

        // Stub: would execute the CNI binary with appropriate stdin/env
        match operation {
            CniOperation::Add => { /* Setup networking for container */ }
            CniOperation::Del => { /* Teardown networking for container */ }
            CniOperation::Check => { /* Verify networking is correct */ }
            CniOperation::Version => { /* Report supported versions */ }
        }

        Ok(())
    }

    pub fn list_cni_plugins(&self) -> Vec<(u32, CniPluginType, bool, u32)> {
        self.cni_plugins
            .iter()
            .map(|p| (p.id, p.plugin_type, p.enabled, p.invoke_count))
            .collect()
    }

    // -- Packet routing stub ------------------------------------------------

    pub fn route_packet(
        &mut self,
        _src_ip: u32,
        _dst_ip: u32,
        _dst_port: u16,
        _protocol: PortProto,
    ) -> Result<u32, &'static str> {
        self.total_packets_routed = self.total_packets_routed.saturating_add(1);

        // Stub: would perform actual routing decisions
        // 1. Check port forwards
        // 2. Apply NAT rules
        // 3. Route to appropriate bridge/endpoint
        // Returns destination container_id or 0 for host

        Ok(0)
    }

    // -- Query methods ------------------------------------------------------

    pub fn get_container_ip(&self, bridge_id: u32, container_id: u32) -> Option<u32> {
        let bridge = self.bridges.iter().find(|b| b.id == bridge_id)?;
        bridge.get_endpoint(container_id).map(|e| e.ip_address)
    }

    pub fn bridge_count(&self) -> usize {
        self.bridges.len()
    }

    pub fn veth_count(&self) -> usize {
        self.veth_pairs.len()
    }

    pub fn nat_rule_count(&self) -> usize {
        self.nat_rules.len()
    }

    pub fn port_forward_count(&self) -> usize {
        self.port_forwards.len()
    }

    pub fn total_packets_routed(&self) -> u64 {
        self.total_packets_routed
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static CONTAINER_NET: Mutex<Option<ContainerNetManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = CONTAINER_NET.lock();
    let mut net = ContainerNetManager::new();

    // Create and bring up the default bridge
    match net.create_default_bridge() {
        Ok(bridge_id) => {
            let _ = net.bring_bridge_up(bridge_id);

            // Add default masquerade rule for the bridge subnet
            let _ = net.add_masquerade(
                DEFAULT_BRIDGE_SUBNET,
                DEFAULT_BRIDGE_MASK,
                0x657468300000, // "eth0" hash
            );

            serial_println!("[CONTAINER_NET] Default bridge docker0 created (172.17.0.0/16)");
        }
        Err(e) => {
            serial_println!("[CONTAINER_NET] Failed to create default bridge: {}", e);
        }
    }

    *mgr = Some(net);
    serial_println!("[CONTAINER_NET] Container networking initialized");
}

// -- Public API wrappers ----------------------------------------------------

pub fn create_bridge(
    name_hash: u64,
    subnet: u32,
    subnet_mask: u32,
    gateway: u32,
    mtu: u16,
) -> Result<u32, &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .create_bridge(name_hash, subnet, subnet_mask, gateway, mtu)
}

pub fn connect_container(
    bridge_id: u32,
    container_id: u32,
    host_iface_hash: u64,
    container_iface_hash: u64,
) -> Result<(u32, u32), &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .create_veth_pair(
            bridge_id,
            container_id,
            host_iface_hash,
            container_iface_hash,
        )
}

pub fn disconnect_container(veth_id: u32) -> Result<(), &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .destroy_veth_pair(veth_id)
}

pub fn add_port_forward(
    host_ip: u32,
    host_port: u16,
    container_ip: u32,
    container_port: u16,
    protocol: PortProto,
    container_id: u32,
) -> Result<u32, &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .add_port_forward(
            host_ip,
            host_port,
            container_ip,
            container_port,
            protocol,
            container_id,
        )
}

pub fn remove_port_forward(forward_id: u32) -> Result<(), &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .remove_port_forward(forward_id)
}

pub fn get_container_ip(bridge_id: u32, container_id: u32) -> Option<u32> {
    let mgr = CONTAINER_NET.lock();
    mgr.as_ref()
        .and_then(|m| m.get_container_ip(bridge_id, container_id))
}

pub fn register_cni_plugin(
    name_hash: u64,
    plugin_type: CniPluginType,
    version: CniVersion,
    config_hash: u64,
) -> Result<u32, &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .register_cni_plugin(name_hash, plugin_type, version, config_hash)
}

pub fn invoke_cni_plugin(
    plugin_id: u32,
    operation: CniOperation,
    container_id: u32,
) -> Result<(), &'static str> {
    let mut mgr = CONTAINER_NET.lock();
    mgr.as_mut()
        .ok_or("Container networking not initialized")?
        .invoke_cni_plugin(plugin_id, operation, container_id)
}

pub fn list_port_forwards() -> Vec<(u32, u16, u32, u16, PortProto)> {
    let mgr = CONTAINER_NET.lock();
    match mgr.as_ref() {
        Some(m) => m.list_port_forwards(),
        None => Vec::new(),
    }
}

pub fn bridge_count() -> usize {
    let mgr = CONTAINER_NET.lock();
    match mgr.as_ref() {
        Some(m) => m.bridge_count(),
        None => 0,
    }
}

pub fn veth_count() -> usize {
    let mgr = CONTAINER_NET.lock();
    match mgr.as_ref() {
        Some(m) => m.veth_count(),
        None => 0,
    }
}

pub fn total_packets_routed() -> u64 {
    let mgr = CONTAINER_NET.lock();
    match mgr.as_ref() {
        Some(m) => m.total_packets_routed(),
        None => 0,
    }
}
