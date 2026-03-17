pub mod ai_network;
pub mod arp;
pub mod bluetooth;
pub mod bonding;
pub mod bridge;
pub mod can;
pub mod dccp;
pub mod dhcp;
pub mod dns;
pub mod dns_sd;
/// Network stack for Genesis
///
/// Full TCP/IP implementation from scratch. No lwIP, no smoltcp — ours.
///
/// Layer model:
///   Application (sockets API)
///   Transport (TCP, UDP)
///   Network (IPv4, IPv6, ARP, ICMP)
///   Link (Ethernet frames)
///   Driver (NIC-specific: E1000, Virtio-net)
///
/// Inspired by: Linux networking stack, BSD sockets API, lwIP concepts,
/// Plan 9 /net namespace. All code is original.
pub mod ethernet;
pub mod firewall;
pub mod ftp;
pub mod gre;
pub mod grpc;
pub mod hardening;
pub mod http;
pub mod http2;
pub mod http3;
pub mod icmp;
pub mod ieee802154;
pub mod ieee8021x;
pub mod igmp;
pub mod ipsec;
pub mod ipv4;
pub mod ipv6;
pub mod ipvs;
pub mod lldp;
pub mod mdns;
pub mod mpls;
pub mod mqtt;
pub mod multicast;
pub mod nat;
pub mod netdev;
pub mod netfilter;
pub mod netlink;
pub mod nfs_client;
pub mod ntp;
pub mod packet;
pub mod packet_frag;
pub mod ppp;
pub mod pppoe;
pub mod proxy;
pub mod qos;
pub mod quic;
pub mod raw_socket;
pub mod rdp;
pub mod routing;
pub mod rtnetlink;
pub mod sctp;
pub mod slip;
pub mod socket;
pub mod sockopt;
pub mod ssdp;
pub mod tc;
pub mod tcp;
pub mod tcp_options;
pub mod tls;
pub mod traffic_shaping;
pub mod tunnel;
pub mod tuntap;
pub mod udp;
pub mod unix;
pub mod unix_socket;
pub mod upnp;
pub mod vlan;
pub mod vxlan;
pub mod websocket;
pub mod websocket_proto;
pub mod wifi;
pub mod wireguard;
pub mod xdp;
pub mod xfrm;

use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// MAC address (6 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xFF; 6]);
    pub const ZERO: MacAddr = MacAddr([0; 6]);

    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF; 6]
    }
}

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// IPv4 address (4 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const LOCALHOST: Ipv4Addr = Ipv4Addr([127, 0, 0, 1]);
    pub const BROADCAST: Ipv4Addr = Ipv4Addr([255, 255, 255, 255]);
    pub const ANY: Ipv4Addr = Ipv4Addr([0, 0, 0, 0]);

    pub fn new(a: u8, b: u8, c: u8, d: u8) -> Self {
        Ipv4Addr([a, b, c, d])
    }

    pub fn to_u32(&self) -> u32 {
        u32::from_be_bytes(self.0)
    }

    pub fn from_u32(val: u32) -> Self {
        Ipv4Addr(val.to_be_bytes())
    }
}

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

/// Network interface configuration
pub struct NetworkInterface {
    pub name: &'static str,
    pub mac: MacAddr,
    pub ipv4: Option<Ipv4Addr>,
    pub netmask: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
    pub mtu: u16,
}

/// Global network configuration
static INTERFACES: Mutex<Vec<NetworkInterface>> = Mutex::new(Vec::new());

/// Initialize the network stack
pub fn init() {
    sockopt::init();
    netdev::init();
    arp::init();
    bridge::init();
    wifi::init();
    dhcp::init();
    ntp::init();
    ipv6::init();
    http::init();
    rdp::init();
    ai_network::init();
    mqtt::init();
    ftp::init();
    proxy::init();
    traffic_shaping::init();
    igmp::init();
    mdns::init();
    dns_sd::init();
    vlan::init();
    tuntap::init();
    bonding::init();
    unix_socket::init();
    tunnel::init();
    sctp::init();
    netlink::init();
    ipsec::init();
    netfilter::init();
    dccp::init();
    nfs_client::init();
    tc::init();
    vxlan::init();
    gre::init();
    mpls::init();
    lldp::init();
    pppoe::init();
    rtnetlink::init();
    xfrm::init();
    ieee8021x::init();
    serial_println!("  Net: TCP/IP stack initialized (AI IDS, QoS, DNS prefetch, IGMP, mDNS/DNS-SD, VLAN, TUN/TAP, bonding, tunnels, SCTP, Netlink, IPsec ESP/AH, netfilter, DCCP, NFS v3, TC, VXLAN, GRE, MPLS, LLDP)");
    serial_println!("  Net: AF_UNIX domain socket subsystem ready");
    serial_println!("  Net: ARP cache ready");
}

/// Process an incoming Ethernet frame through the network stack.
///
/// Dispatches by EtherType: ARP → arp module, IPv4 → ipv4/icmp/tcp/udp.
/// Generates any reply frames and sends them back via the NIC driver.
pub fn process_frame(frame_data: &[u8]) {
    let (eth_hdr, payload) = match ethernet::EthernetHeader::parse(frame_data) {
        Some(h) => h,
        None => return,
    };

    match eth_hdr.ethertype_u16() {
        ethernet::ETHERTYPE_ARP => {
            process_arp_frame(eth_hdr, payload);
        }
        ethernet::ETHERTYPE_IPV4 => {
            process_ipv4_frame(eth_hdr, payload);
        }
        _ => {} // ignore unknown EtherTypes
    }
}

/// Re-inject a raw IPv4 packet (no Ethernet header) back into the IP receive path.
///
/// Used by the tunnel subsystem to deliver decapsulated inner packets after
/// IPIP or GRE decapsulation.
pub fn process_frame_ip(ip_data: &[u8]) {
    // Build a dummy Ethernet header (all zeros) — process_ipv4_frame ignores
    // the `_eth_hdr` argument (note the leading underscore in its parameter).
    let dummy_eth = ethernet::EthernetHeader {
        dst: [0u8; 6],
        src: [0u8; 6],
        ethertype: ethernet::ETHERTYPE_IPV4.to_be_bytes(),
    };
    process_ipv4_frame(&dummy_eth, ip_data);
}

/// Handle an incoming ARP frame
fn process_arp_frame(_eth_hdr: &ethernet::EthernetHeader, payload: &[u8]) {
    let ifaces = INTERFACES.lock();
    let iface = match ifaces.first() {
        Some(i) => i,
        None => return,
    };
    let our_ip = match iface.ipv4 {
        Some(ip) => ip,
        None => return,
    };
    let our_mac = iface.mac;
    drop(ifaces);

    // Parse the ARP packet to determine if it's a reply (so we can drain pending)
    let sender_ip = arp::ArpPacket::parse(payload).map(|p| p.sender_ipv4());

    if let Some(reply_pkt) = arp::process_arp(payload, our_ip, our_mac) {
        // Build reply Ethernet frame and send it
        let reply_dst = MacAddr(reply_pkt.target_hw);
        let mut buf = [0u8; 64]; // min Ethernet frame
        let arp_bytes = unsafe {
            core::slice::from_raw_parts(
                &reply_pkt as *const arp::ArpPacket as *const u8,
                core::mem::size_of::<arp::ArpPacket>(),
            )
        };
        let len = ethernet::build_frame(
            reply_dst,
            our_mac,
            ethernet::ETHERTYPE_ARP,
            arp_bytes,
            &mut buf,
        );
        send_raw(&buf[..len.max(60)]); // pad to min Ethernet frame size
    }

    // If this was an ARP reply that resolved a pending IP address, drain and
    // send any packets that were queued waiting for MAC resolution.
    if let Some(resolved_ip) = sender_ip {
        let pending = arp::drain_pending(resolved_ip);
        if !pending.is_empty() {
            let dst_mac = arp::lookup(resolved_ip).unwrap_or(MacAddr::BROADCAST);
            for pkt in pending {
                // Each queued packet is a complete IPv4 packet (no Ethernet header)
                send_ip_frame(our_mac, dst_mac, &pkt);
            }
        }
    }
}

/// Handle an incoming IPv4 frame
fn process_ipv4_frame(_eth_hdr: &ethernet::EthernetHeader, payload: &[u8]) {
    let (ip_hdr, ip_payload) = match ipv4::Ipv4Header::parse(payload) {
        Some(h) => h,
        None => return,
    };

    let ifaces = INTERFACES.lock();
    let iface = match ifaces.first() {
        Some(i) => i,
        None => return,
    };
    let our_ip = match iface.ipv4 {
        Some(ip) => ip,
        None => return,
    };
    let our_mac = iface.mac;
    drop(ifaces);

    // Only process packets addressed to us
    if ip_hdr.dst_addr() != our_ip && ip_hdr.dst_addr() != Ipv4Addr::BROADCAST {
        return;
    }

    // --- netfilter INPUT hook ---
    // Run the packet through the NF_HOOK_INPUT chains.  Drop the packet if
    // the verdict is NF_DROP (0) or negative (NF_STOLEN, NF_QUEUE).
    // Use interface index 0 (primary interface) for now.
    {
        let verdict = netfilter::nf_hook(
            netfilter::NF_HOOK_INPUT,
            payload,
            payload.len(),
            0, // iface index — primary interface
        );
        if verdict != netfilter::NF_ACCEPT {
            return;
        }
    }

    match ip_hdr.protocol {
        ipv4::PROTO_ICMP => {
            process_icmp(ip_hdr, ip_payload, our_ip, our_mac);
        }
        ipv4::PROTO_TCP => {
            process_tcp(ip_hdr, ip_payload, our_ip, our_mac);
        }
        ipv4::PROTO_UDP => {
            process_udp(ip_hdr, ip_payload, our_ip, our_mac);
        }
        // SCTP (proto=132): route to SCTP subsystem.
        sctp::IPPROTO_SCTP => {
            let src = ip_hdr.src_addr();
            sctp::sctp_input(ip_payload, ip_payload.len(), src.0);
        }
        // IPIP (proto=4) and GRE (proto=47): route to the tunnel subsystem.
        // Pass the full outer payload (including the outer IP header) so
        // tunnel_receive can parse src/dst addresses for tunnel matching.
        4 /* IPPROTO_IPIP */ | ipv4::PROTO_GRE => {
            tunnel::tunnel_receive(payload, payload.len(), ip_hdr.protocol);
        }
        // DCCP (proto=33): route to the DCCP subsystem.
        dccp::IPPROTO_DCCP => {
            let src = ip_hdr.src_addr();
            dccp::dccp_input(ip_payload, ip_payload.len(), src.0);
        }
        _ => {}
    }
}

/// Handle an ICMP packet (ping request → ping reply)
fn process_icmp(ip_hdr: &ipv4::Ipv4Header, payload: &[u8], our_ip: Ipv4Addr, our_mac: MacAddr) {
    if let Some(icmp_reply) = icmp::build_echo_reply(payload) {
        // Look up destination MAC (we should have it from the ARP cache)
        let dst_ip = ip_hdr.src_addr();
        let dst_mac = arp::lookup(dst_ip).unwrap_or(MacAddr::BROADCAST);

        // Build IPv4 header for reply
        let ip_reply = ipv4::build_header(
            our_ip,
            dst_ip,
            ipv4::PROTO_ICMP,
            icmp_reply.len() as u16,
            64, // TTL
        );

        // Assemble full packet: Ethernet + IPv4 + ICMP
        let ip_bytes = unsafe {
            core::slice::from_raw_parts(&ip_reply as *const ipv4::Ipv4Header as *const u8, 20)
        };

        let mut frame_buf = [0u8; 1514];
        let mut offset = 14; // leave room for Ethernet header

        // IPv4 header
        frame_buf[offset..offset + 20].copy_from_slice(ip_bytes);
        offset += 20;

        // ICMP payload
        let icmp_len = icmp_reply.len().min(frame_buf.len() - offset);
        frame_buf[offset..offset + icmp_len].copy_from_slice(&icmp_reply[..icmp_len]);
        offset += icmp_len;

        // Ethernet header
        frame_buf[0..6].copy_from_slice(&dst_mac.0);
        frame_buf[6..12].copy_from_slice(&our_mac.0);
        frame_buf[12..14].copy_from_slice(&ethernet::ETHERTYPE_IPV4.to_be_bytes());

        send_raw(&frame_buf[..offset.max(60)]);
        serial_println!("  Net: ICMP echo reply -> {}", dst_ip);
    }
}

/// Handle an incoming TCP segment
fn process_tcp(ip_hdr: &ipv4::Ipv4Header, payload: &[u8], our_ip: Ipv4Addr, our_mac: MacAddr) {
    let (tcp_hdr, tcp_data) = match tcp::TcpHeader::parse(payload) {
        Some(h) => h,
        None => return,
    };

    let src_ip = ip_hdr.src_addr();
    let dst_port = tcp_hdr.dst_port();
    let src_port = tcp_hdr.src_port();

    // Find a matching connection or listening socket
    let mut conns = tcp::TCP_CONNECTIONS.lock();
    let mut matched_id: Option<u32> = None;

    // First: look for exact match (established connection)
    for (&id, conn) in conns.iter() {
        if conn.local_port == dst_port
            && conn.remote_port == src_port
            && conn.remote_ip == src_ip
            && conn.state != tcp::TcpState::Listen
            && conn.state != tcp::TcpState::Closed
        {
            matched_id = Some(id);
            break;
        }
    }

    // Second: look for listening socket
    if matched_id.is_none() && tcp_hdr.has_flag(tcp::flags::SYN) {
        for (&id, conn) in conns.iter() {
            if conn.local_port == dst_port && conn.state == tcp::TcpState::Listen {
                matched_id = Some(id);
                break;
            }
        }
    }

    if let Some(id) = matched_id {
        // Captured values we need after releasing the lock
        struct ConnResponse {
            old_state: tcp::TcpState,
            new_state: tcp::TcpState,
            local_port: u16,
            remote_port: u16,
            snd_nxt: u32,
            rcv_nxt: u32,
            rcv_wnd: u16,
            snd_nxt_after_syn: u32, // snd_nxt after incrementing for SYN
            data_segments: Vec<(u32, Vec<u8>, u16)>, // prepared outbound data
            send_ack: bool,         // whether a bare ACK is warranted
            send_syn_ack: bool,
            send_fin_ack: bool,
        }

        let response = if let Some(conn) = conns.get_mut(&id) {
            let old_state = conn.state;
            conn.process_segment(tcp_hdr, tcp_data);
            let new_state = conn.state;

            // For SYN_RECEIVED, set the remote IP (was not set during listen())
            if old_state == tcp::TcpState::Listen && new_state == tcp::TcpState::SynReceived {
                conn.remote_ip = src_ip;
            }

            // Drain any buffered outbound data (e.g. application wrote while ESTABLISHED)
            let data_segments = if new_state == tcp::TcpState::Established
                || new_state == tcp::TcpState::CloseWait
            {
                conn.prepare_segments()
            } else {
                Vec::new()
            };

            // Determine what control packets to send
            let send_syn_ack =
                old_state == tcp::TcpState::Listen && new_state == tcp::TcpState::SynReceived;

            // After processing an in-order data segment or FIN we must ACK
            let send_ack = !tcp_data.is_empty()
                && (new_state == tcp::TcpState::Established
                    || new_state == tcp::TcpState::CloseWait
                    || new_state == tcp::TcpState::FinWait2
                    || new_state == tcp::TcpState::TimeWait);

            // FIN arrived → we transitioned to CLOSE_WAIT and need to ACK the FIN
            let send_fin_ack = (old_state == tcp::TcpState::Established
                && new_state == tcp::TcpState::CloseWait)
                || (old_state == tcp::TcpState::FinWait2 && new_state == tcp::TcpState::TimeWait);

            // For SYN+ACK we will advance snd_nxt ourselves here so conn is consistent
            let snd_nxt_after_syn = if send_syn_ack {
                conn.snd_nxt.wrapping_add(1)
            } else {
                conn.snd_nxt
            };
            if send_syn_ack {
                conn.snd_nxt = snd_nxt_after_syn;
            }

            Some(ConnResponse {
                old_state,
                new_state,
                local_port: conn.local_port,
                remote_port: src_port,
                snd_nxt: conn.snd_nxt,
                rcv_nxt: conn.rcv_nxt,
                rcv_wnd: conn.rcv_wnd,
                snd_nxt_after_syn,
                data_segments,
                send_ack,
                send_syn_ack,
                send_fin_ack,
            })
        } else {
            None
        };

        drop(conns);

        if let Some(r) = response {
            let dst_mac = arp::lookup(src_ip).unwrap_or(MacAddr::BROADCAST);

            if r.send_syn_ack {
                // Send SYN+ACK: seq = ISN (snd_nxt before increment), ack = rcv_nxt
                let syn_ack = build_tcp_packet(
                    our_ip,
                    src_ip,
                    r.local_port,
                    r.remote_port,
                    r.snd_nxt_after_syn.wrapping_sub(1), // the ISN before incrementing
                    r.rcv_nxt,
                    tcp::flags::SYN | tcp::flags::ACK,
                    r.rcv_wnd,
                    &[],
                );
                send_ip_frame(our_mac, dst_mac, &syn_ack);
                serial_println!("  TCP: SYN-ACK -> {}:{}", src_ip, src_port);
            }

            if r.new_state == tcp::TcpState::Established
                && r.old_state == tcp::TcpState::SynReceived
            {
                serial_println!("  TCP: connection established from {}:{}", src_ip, src_port);
            }

            // Send any queued outbound data (application data segments)
            for (seq, data, flags) in &r.data_segments {
                let pkt = build_tcp_packet(
                    our_ip,
                    src_ip,
                    r.local_port,
                    r.remote_port,
                    *seq,
                    r.rcv_nxt,
                    *flags,
                    r.rcv_wnd,
                    data,
                );
                send_ip_frame(our_mac, dst_mac, &pkt);
            }

            // Send a bare ACK for received data (if we didn't already piggyback one)
            if (r.send_ack || r.send_fin_ack) && r.data_segments.is_empty() {
                let ack_pkt = build_tcp_packet(
                    our_ip,
                    src_ip,
                    r.local_port,
                    r.remote_port,
                    r.snd_nxt,
                    r.rcv_nxt,
                    tcp::flags::ACK,
                    r.rcv_wnd,
                    &[],
                );
                send_ip_frame(our_mac, dst_mac, &ack_pkt);
                if r.send_fin_ack {
                    serial_println!("  TCP: FIN-ACK -> {}:{}", src_ip, src_port);
                } else if !tcp_data.is_empty() {
                    serial_println!(
                        "  TCP: ACK {} bytes from {}:{}",
                        tcp_data.len(),
                        src_ip,
                        src_port
                    );
                }
            }
        }
    } else {
        drop(conns);
        // No matching connection — send RST (unless incoming segment is already RST)
        if !tcp_hdr.has_flag(tcp::flags::RST) {
            let rst = build_tcp_packet(
                our_ip,
                src_ip,
                dst_port,
                src_port,
                // If ACK set, RST seq = ack number; otherwise seq=0, ack=incoming_seq+len
                if tcp_hdr.has_flag(tcp::flags::ACK) {
                    tcp_hdr.ack()
                } else {
                    0
                },
                if tcp_hdr.has_flag(tcp::flags::ACK) {
                    0
                } else {
                    tcp_hdr
                        .seq()
                        .wrapping_add(tcp_data.len() as u32)
                        .wrapping_add(1)
                },
                if tcp_hdr.has_flag(tcp::flags::ACK) {
                    tcp::flags::RST
                } else {
                    tcp::flags::RST | tcp::flags::ACK
                },
                0,
                &[],
            );
            let dst_mac = arp::lookup(src_ip).unwrap_or(MacAddr::BROADCAST);
            send_ip_frame(our_mac, dst_mac, &rst);
        }
    }
}

/// Build a TCP segment wrapped in an IPv4 packet (header bytes only, no Ethernet)
fn build_tcp_packet(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u16,
    window: u16,
    data: &[u8],
) -> Vec<u8> {
    let tcp_len = 20 + data.len();

    // Build TCP header
    let data_offset_flags: u16 = (5 << 12) | (flags & 0x01FF);
    let mut tcp_hdr = [0u8; 20];
    tcp_hdr[0..2].copy_from_slice(&src_port.to_be_bytes());
    tcp_hdr[2..4].copy_from_slice(&dst_port.to_be_bytes());
    tcp_hdr[4..8].copy_from_slice(&seq.to_be_bytes());
    tcp_hdr[8..12].copy_from_slice(&ack.to_be_bytes());
    tcp_hdr[12..14].copy_from_slice(&data_offset_flags.to_be_bytes());
    tcp_hdr[14..16].copy_from_slice(&window.to_be_bytes());
    // checksum [16..18] = 0 for now, urgent [18..20] = 0

    // TCP pseudo-header checksum
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&src_ip.0);
    pseudo.extend_from_slice(&dst_ip.0);
    pseudo.push(0);
    pseudo.push(ipv4::PROTO_TCP);
    pseudo.extend_from_slice(&(tcp_len as u16).to_be_bytes());
    pseudo.extend_from_slice(&tcp_hdr);
    pseudo.extend_from_slice(data);
    if pseudo.len() % 2 != 0 {
        pseudo.push(0);
    }
    let cksum = ipv4::internet_checksum(&pseudo);
    tcp_hdr[16..18].copy_from_slice(&cksum.to_be_bytes());

    // Build IPv4 header
    let ip_hdr = ipv4::build_header(src_ip, dst_ip, ipv4::PROTO_TCP, tcp_len as u16, 64);
    let ip_bytes =
        unsafe { core::slice::from_raw_parts(&ip_hdr as *const ipv4::Ipv4Header as *const u8, 20) };

    let mut packet = Vec::new();
    packet.extend_from_slice(ip_bytes);
    packet.extend_from_slice(&tcp_hdr);
    packet.extend_from_slice(data);
    packet
}

/// Send an IPv4 packet wrapped in an Ethernet frame (public alias for use by sub-modules)
pub fn send_ip_frame_pub(src_mac: MacAddr, dst_mac: MacAddr, ip_packet: &[u8]) {
    send_ip_frame(src_mac, dst_mac, ip_packet);
}

/// Send an IPv4 packet wrapped in an Ethernet frame
fn send_ip_frame(src_mac: MacAddr, dst_mac: MacAddr, ip_packet: &[u8]) {
    let total = 14 + ip_packet.len();
    let mut frame = alloc::vec![0u8; total.max(60)];
    frame[0..6].copy_from_slice(&dst_mac.0);
    frame[6..12].copy_from_slice(&src_mac.0);
    frame[12..14].copy_from_slice(&ethernet::ETHERTYPE_IPV4.to_be_bytes());
    frame[14..14 + ip_packet.len()].copy_from_slice(ip_packet);
    send_raw(&frame);
}

/// Send a raw Ethernet frame via the e1000 driver
fn send_raw(frame: &[u8]) {
    let driver = crate::drivers::e1000::driver().lock();
    if let Some(ref nic) = *driver {
        let _ = nic.send(frame);
    }
}

/// Poll the NIC for incoming frames and process them through the stack.
/// Called periodically from the main loop or a network thread.
pub fn poll() {
    let mut buf = [0u8; 2048];
    loop {
        let len = {
            let driver = crate::drivers::e1000::driver().lock();
            match driver.as_ref() {
                Some(nic) => match nic.recv(&mut buf) {
                    Ok(len) => len,
                    Err(_) => break, // no more frames
                },
                None => break,
            }
        };
        if len > 0 {
            process_frame(&buf[..len]);
        }
    }
}

/// Configure a network interface
pub fn configure_interface(
    name: &'static str,
    mac: MacAddr,
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
    gateway: Ipv4Addr,
) {
    let iface = NetworkInterface {
        name,
        mac,
        ipv4: Some(ip),
        netmask: Some(netmask),
        gateway: Some(gateway),
        mtu: 1500,
    };

    INTERFACES.lock().push(iface);
    serial_println!(
        "  Net: {} configured — {} / {} gw {}",
        name,
        ip,
        netmask,
        gateway
    );
}

/// Return the MAC address of the primary (first) network interface.
/// Used by sub-modules (e.g. tunnel) that need a source MAC without
/// holding the INTERFACES lock beyond this call.
pub fn primary_mac() -> Option<MacAddr> {
    INTERFACES.lock().first().map(|i| i.mac)
}

/// Return the IPv4 address of the primary (first) network interface.
/// Used by sub-modules (e.g. dccp, nfs_client) that need a source IP without
/// holding the INTERFACES lock beyond this call.
pub fn primary_ip() -> Option<Ipv4Addr> {
    INTERFACES.lock().first().and_then(|i| i.ipv4)
}

/// A snapshot of interface info (cloneable for listing)
pub struct InterfaceInfo {
    pub name: alloc::string::String,
    pub mac: MacAddr,
    pub ip: Option<Ipv4Addr>,
}

impl InterfaceInfo {
    pub fn mac_string(&self) -> alloc::string::String {
        alloc::format!("{}", self.mac)
    }

    pub fn ip_string(&self) -> alloc::string::String {
        match self.ip {
            Some(ip) => alloc::format!("{}", ip),
            None => alloc::string::String::from("none"),
        }
    }
}

/// List all configured network interfaces
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    INTERFACES
        .lock()
        .iter()
        .map(|iface| InterfaceInfo {
            name: alloc::string::String::from(iface.name),
            mac: iface.mac,
            ip: iface.ipv4,
        })
        .collect()
}

/// A no-heap, Copy snapshot of interface configuration for use by sub-modules
/// (e.g. netlink, sctp) that cannot use Vec or alloc.
#[derive(Copy, Clone)]
pub struct IfaceSnapshot {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub mtu: u16,
    pub valid: bool,
}

impl IfaceSnapshot {
    pub const fn empty() -> Self {
        IfaceSnapshot {
            mac: [0u8; 6],
            ip: [0u8; 4],
            netmask: [0u8; 4],
            gateway: [0u8; 4],
            mtu: 1500,
            valid: false,
        }
    }
}

/// Fill up to N interface snapshots without heap allocation.
/// Returns the number of entries filled.
pub fn snapshot_interfaces<const N: usize>(out: &mut [IfaceSnapshot; N]) -> usize {
    let ifaces = INTERFACES.lock();
    let mut count = 0;
    for iface in ifaces.iter() {
        if count >= N {
            break;
        }
        let ip = iface.ipv4.map(|a| a.0).unwrap_or([0u8; 4]);
        let nm = iface.netmask.map(|a| a.0).unwrap_or([0u8; 4]);
        let gw = iface.gateway.map(|a| a.0).unwrap_or([0u8; 4]);
        out[count] = IfaceSnapshot {
            mac: iface.mac.0,
            ip,
            netmask: nm,
            gateway: gw,
            mtu: iface.mtu,
            valid: true,
        };
        count += 1;
    }
    count
}

// ============================================================================
// UDP processing
// ============================================================================

/// UDP receive queue — maps (port) -> Vec<(src_ip, src_port, data)>
static UDP_RECV_QUEUE: Mutex<alloc::collections::BTreeMap<u16, Vec<(Ipv4Addr, u16, Vec<u8>)>>> =
    Mutex::new(alloc::collections::BTreeMap::new());

/// Handle an incoming UDP datagram
fn process_udp(ip_hdr: &ipv4::Ipv4Header, payload: &[u8], _our_ip: Ipv4Addr, _our_mac: MacAddr) {
    let (udp_hdr, udp_data) = match udp::UdpHeader::parse(payload) {
        Some(h) => h,
        None => return,
    };

    let src_ip = ip_hdr.src_addr();
    let src_port = udp_hdr.src_port();
    let dst_port = udp_hdr.dst_port();

    serial_println!(
        "  UDP: {}:{} -> port {} ({} bytes)",
        src_ip,
        src_port,
        dst_port,
        udp_data.len()
    );

    // Check for DNS response (port 53)
    if src_port == 53 {
        if let Some(resolver) = DNS_PENDING.lock().as_mut() {
            resolver.extend_from_slice(udp_data);
        }
    }

    // Check for DHCP response (port 68)
    if dst_port == 68 {
        process_dhcp_response(udp_data, _our_mac);
    }

    // Queue for userspace
    let mut queue = UDP_RECV_QUEUE.lock();
    queue
        .entry(dst_port)
        .or_insert_with(Vec::new)
        .push((src_ip, src_port, Vec::from(udp_data)));
}

/// Send a UDP packet.  Builds a properly checksummed UDP datagram and wraps it
/// in an IPv4 packet before handing to the NIC driver.
pub fn send_udp(
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
    data: &[u8],
) -> Result<(), NetError> {
    let ifaces = INTERFACES.lock();
    let iface = ifaces.first().ok_or(NetError::NoInterface)?;
    let our_ip = iface.ipv4.ok_or(NetError::AddrNotAvailable)?;
    let our_mac = iface.mac;
    drop(ifaces);

    // Build checksummed UDP datagram (header + payload).
    let udp_pkt = udp::build_packet_with_checksum(our_ip, dst_ip, src_port, dst_port, data);

    // Build IPv4 header (TTL=64, DF set by build_header default).
    let ip_hdr = ipv4::build_header(
        our_ip,
        dst_ip,
        ipv4::PROTO_UDP,
        udp_pkt.len() as u16,
        ipv4::DEFAULT_TTL,
    );
    let ip_bytes =
        unsafe { core::slice::from_raw_parts(&ip_hdr as *const ipv4::Ipv4Header as *const u8, 20) };

    let mut packet = Vec::with_capacity(20 + udp_pkt.len());
    packet.extend_from_slice(ip_bytes);
    packet.extend_from_slice(&udp_pkt);

    let dst_mac = arp::lookup(dst_ip).unwrap_or(MacAddr::BROADCAST);
    send_ip_frame(our_mac, dst_mac, &packet);
    Ok(())
}

/// Bind to a UDP port (register for receiving)
pub fn udp_bind(port: u16) {
    let mut queue = UDP_RECV_QUEUE.lock();
    queue.entry(port).or_insert_with(Vec::new);
}

/// Receive a UDP packet (non-blocking)
pub fn udp_recv(port: u16) -> Option<(Ipv4Addr, u16, Vec<u8>)> {
    let mut queue = UDP_RECV_QUEUE.lock();
    queue.get_mut(&port).and_then(|q| {
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    })
}

// ============================================================================
// DNS resolver
// ============================================================================

static DNS_PENDING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Resolve a hostname to an IPv4 address
pub fn dns_resolve(hostname: &str) -> Option<Ipv4Addr> {
    let query = dns::build_query(hostname, 0x1234);

    // Send DNS query to first configured server (1.1.1.1 default)
    let dns_server = Ipv4Addr::new(10, 0, 2, 3); // QEMU user-mode DNS

    // Set up response buffer
    *DNS_PENDING.lock() = Some(Vec::new());

    if send_udp(12345, dns_server, 53, &query).is_err() {
        return None;
    }

    // Poll for response (timeout after ~500 polls)
    for _ in 0..500 {
        poll(); // process incoming frames

        let response = DNS_PENDING.lock().clone();
        if let Some(ref data) = response {
            if data.len() >= 12 {
                let records = dns::parse_response(data);
                *DNS_PENDING.lock() = None;
                for record in records {
                    if let dns::DnsRecord::A(ip) = record {
                        serial_println!("  DNS: {} -> {}", hostname, ip);
                        return Some(ip);
                    }
                }
                return None;
            }
        }

        // Brief busy-wait
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
    }

    *DNS_PENDING.lock() = None;
    None
}

// ============================================================================
// DHCP client
// ============================================================================

/// Send a DHCP DISCOVER packet
pub fn dhcp_discover() {
    let ifaces = INTERFACES.lock();
    let mac = if let Some(iface) = ifaces.first() {
        iface.mac
    } else {
        return;
    };
    drop(ifaces);

    // DHCP discover message
    let mut dhcp = alloc::vec![0u8; 300];
    dhcp[0] = 1; // op: BOOTREQUEST
    dhcp[1] = 1; // htype: Ethernet
    dhcp[2] = 6; // hlen: MAC length
    dhcp[3] = 0; // hops
                 // xid (transaction ID)
    dhcp[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    // flags: broadcast
    dhcp[10] = 0x80;
    // chaddr (client MAC)
    dhcp[28..34].copy_from_slice(&mac.0);
    // Magic cookie
    dhcp[236..240].copy_from_slice(&[99, 130, 83, 99]);
    // Option 53: DHCP Message Type = DISCOVER (1)
    dhcp[240] = 53;
    dhcp[241] = 1;
    dhcp[242] = 1;
    // Option 55: Parameter Request List
    dhcp[243] = 55;
    dhcp[244] = 4;
    dhcp[245] = 1;
    dhcp[246] = 3;
    dhcp[247] = 6;
    dhcp[248] = 15;
    // End
    dhcp[249] = 255;

    let _ = send_udp(68, Ipv4Addr::BROADCAST, 67, &dhcp);
    serial_println!("  DHCP: DISCOVER sent");
}

/// Process a DHCP response
fn process_dhcp_response(data: &[u8], _our_mac: MacAddr) {
    if data.len() < 240 {
        return;
    }

    // Check it's a BOOTREPLY
    if data[0] != 2 {
        return;
    }

    // Extract offered IP (yiaddr)
    let offered_ip = Ipv4Addr([data[16], data[17], data[18], data[19]]);

    // Parse options starting at 240 (after magic cookie at 236-239)
    let mut i = 240;
    let mut msg_type = 0u8;
    let mut subnet = Ipv4Addr::ANY;
    let mut gateway = Ipv4Addr::ANY;
    let mut dns_addr = Ipv4Addr::ANY;

    while i < data.len() && data[i] != 255 {
        if data[i] == 0 {
            i += 1;
            continue;
        } // padding
        let opt = data[i];
        let len = data[i + 1] as usize;
        let val = &data[i + 2..i + 2 + len];
        match opt {
            53 => msg_type = val[0],
            1 if len == 4 => subnet = Ipv4Addr([val[0], val[1], val[2], val[3]]),
            3 if len >= 4 => gateway = Ipv4Addr([val[0], val[1], val[2], val[3]]),
            6 if len >= 4 => dns_addr = Ipv4Addr([val[0], val[1], val[2], val[3]]),
            _ => {}
        }
        i = i.saturating_add(2 + len);
    }

    if msg_type == 2 {
        // DHCP OFFER — accept it
        serial_println!(
            "  DHCP: OFFER ip={} mask={} gw={}",
            offered_ip,
            subnet,
            gateway
        );

        // Configure the interface
        let ifaces = INTERFACES.lock();
        if let Some(iface) = ifaces.first() {
            let _name = iface.name;
            let _mac = iface.mac;
            drop(ifaces);
            // Update would go here; for now, log it
            serial_println!("  DHCP: configured {}", offered_ip);
        }
    }
}

// ============================================================================
// Loopback interface
// ============================================================================

/// Send a packet to the loopback interface (127.0.0.1)
pub fn loopback_send(data: &[u8]) {
    // Process the frame directly through the stack
    process_frame(data);
}

// ============================================================================
// Unix domain sockets
// ============================================================================

/// Unix socket — local IPC using filesystem paths
struct UnixSocket {
    path: alloc::string::String,
    recv_buf: Vec<u8>,
    connected: bool,
    peer_id: Option<usize>,
}

static UNIX_SOCKETS: Mutex<Vec<UnixSocket>> = Mutex::new(Vec::new());

pub fn unix_socket_create(path: &str) -> usize {
    let mut socks = UNIX_SOCKETS.lock();
    let id = socks.len();
    socks.push(UnixSocket {
        path: alloc::string::String::from(path),
        recv_buf: Vec::new(),
        connected: false,
        peer_id: None,
    });
    id
}

pub fn unix_socket_connect(id: usize, peer_path: &str) -> Result<(), NetError> {
    let mut socks = UNIX_SOCKETS.lock();
    // Find peer by path
    let peer_id = socks
        .iter()
        .position(|s| s.path == peer_path)
        .ok_or(NetError::ConnectionRefused)?;
    if let Some(sock) = socks.get_mut(id) {
        sock.connected = true;
        sock.peer_id = Some(peer_id);
    }
    Ok(())
}

pub fn unix_socket_send(id: usize, data: &[u8]) -> Result<usize, NetError> {
    let mut socks = UNIX_SOCKETS.lock();
    let peer_id = socks
        .get(id)
        .and_then(|s| s.peer_id)
        .ok_or(NetError::ConnectionRefused)?;
    if let Some(peer) = socks.get_mut(peer_id) {
        peer.recv_buf.extend_from_slice(data);
        Ok(data.len())
    } else {
        Err(NetError::ConnectionRefused)
    }
}

pub fn unix_socket_recv(id: usize) -> Vec<u8> {
    let mut socks = UNIX_SOCKETS.lock();
    if let Some(sock) = socks.get_mut(id) {
        let data = sock.recv_buf.clone();
        sock.recv_buf.clear();
        data
    } else {
        Vec::new()
    }
}

// ============================================================================
// HTTP/1.1 client
// ============================================================================

/// Simple HTTP GET request (returns response body)
pub fn http_get(host: &str, path: &str) -> Result<Vec<u8>, NetError> {
    // Resolve hostname
    let ip = dns_resolve(host).ok_or(NetError::Unreachable)?;

    // Create TCP connection
    let conn_id = tcp::connect(12346, ip, 80);

    let ifaces = INTERFACES.lock();
    let iface = ifaces.first().ok_or(NetError::NoInterface)?;
    let our_ip = iface.ipv4.ok_or(NetError::AddrNotAvailable)?;
    let our_mac = iface.mac;
    drop(ifaces);

    // Send SYN
    let conns = tcp::TCP_CONNECTIONS.lock();
    let conn = conns.get(&conn_id).ok_or(NetError::ConnectionRefused)?;
    let syn_pkt = build_tcp_packet(
        our_ip,
        ip,
        conn.local_port,
        80,
        conn.snd_nxt,
        0,
        tcp::flags::SYN,
        65535,
        &[],
    );
    drop(conns);
    let dst_mac = arp::lookup(ip).unwrap_or(MacAddr::BROADCAST);
    send_ip_frame(our_mac, dst_mac, &syn_pkt);

    // Wait for connection (simplified — poll)
    for _ in 0..1000 {
        poll();
        if tcp::get_state(conn_id) == Some(tcp::TcpState::Established) {
            break;
        }
        for _ in 0..5000 {
            core::hint::spin_loop();
        }
    }

    if tcp::get_state(conn_id) != Some(tcp::TcpState::Established) {
        return Err(NetError::Timeout);
    }

    // Send HTTP request
    let request = alloc::format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path,
        host
    );

    let conns = tcp::TCP_CONNECTIONS.lock();
    if let Some(conn) = conns.get(&conn_id) {
        let data_pkt = build_tcp_packet(
            our_ip,
            ip,
            conn.local_port,
            80,
            conn.snd_nxt,
            conn.rcv_nxt,
            tcp::flags::ACK | tcp::flags::PSH,
            conn.rcv_wnd,
            request.as_bytes(),
        );
        drop(conns);
        send_ip_frame(our_mac, dst_mac, &data_pkt);
    } else {
        drop(conns);
    }

    // Collect response
    for _ in 0..2000 {
        poll();
        for _ in 0..5000 {
            core::hint::spin_loop();
        }
    }

    let data = tcp::read_data(conn_id);
    Ok(data)
}

// ============================================================================
// epoll — I/O event multiplexing
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpollEventType {
    Read,
    Write,
    Error,
    HangUp,
}

pub struct EpollFd {
    pub fd: u32,
    pub events: u32, // bitmask
}

pub struct EpollInstance {
    pub fds: Vec<EpollFd>,
}

static EPOLL_INSTANCES: Mutex<Vec<EpollInstance>> = Mutex::new(Vec::new());

pub fn epoll_create() -> usize {
    let mut instances = EPOLL_INSTANCES.lock();
    let id = instances.len();
    instances.push(EpollInstance { fds: Vec::new() });
    id
}

pub fn epoll_add(epoll_id: usize, fd: u32, events: u32) {
    let mut instances = EPOLL_INSTANCES.lock();
    if let Some(ep) = instances.get_mut(epoll_id) {
        ep.fds.push(EpollFd { fd, events });
    }
}

pub fn epoll_wait(epoll_id: usize) -> Vec<(u32, u32)> {
    // Non-blocking check which fds are ready
    let instances = EPOLL_INSTANCES.lock();
    let ep = match instances.get(epoll_id) {
        Some(ep) => ep,
        None => return Vec::new(),
    };

    let mut ready = Vec::new();
    for efd in &ep.fds {
        // Check if fd has data (simplified — just check keyboard for fd 0)
        if efd.fd == 0 && crate::drivers::keyboard::has_key() {
            ready.push((efd.fd, 1)); // EPOLLIN
        }
    }
    ready
}

// ============================================================================
// TCP retransmission & congestion
// ============================================================================

/// TCP retransmission timer check — called periodically (every ~100 ms).
/// Drives: keep-alive probes, retransmission timeouts, TIME_WAIT cleanup.
pub fn tcp_retransmit_check() {
    // Collect (conn_id, local_port, remote_ip, remote_port, segments) outside the lock.
    let ifaces = INTERFACES.lock();
    let iface = match ifaces.first() {
        Some(i) => i,
        None => return,
    };
    let our_ip = match iface.ipv4 {
        Some(ip) => ip,
        None => return,
    };
    let our_mac = iface.mac;
    drop(ifaces);

    // Run the timer tick (cleans up TIME_WAIT, marks connections for retx)
    tcp::tcp_timer_tick();

    // Collect connections that have pending data segments or retransmissions
    struct PendingConn {
        local_port: u16,
        remote_ip: Ipv4Addr,
        remote_port: u16,
        rcv_nxt: u32,
        rcv_wnd: u16,
        segments: Vec<(u32, Vec<u8>, u16)>, // (seq, data, flags)
        retx_segments: Vec<(u32, Vec<u8>, u16)>,
        keepalive: Option<(u32, u32, u16, u16)>, // (seq, ack, flags, wnd)
    }

    let mut pending: Vec<PendingConn> = Vec::new();

    {
        let mut conns = tcp::TCP_CONNECTIONS.lock();
        for (_, conn) in conns.iter_mut() {
            // Drain outbound data segments ready to send
            let new_segs = conn.prepare_segments();
            // Check retransmissions
            let retx_segs = conn.check_retransmissions();
            // Check keep-alive
            let ka = if conn.check_keepalive() {
                Some(conn.build_keepalive_probe())
            } else {
                None
            };

            if !new_segs.is_empty() || !retx_segs.is_empty() || ka.is_some() {
                pending.push(PendingConn {
                    local_port: conn.local_port,
                    remote_ip: conn.remote_ip,
                    remote_port: conn.remote_port,
                    rcv_nxt: conn.rcv_nxt,
                    rcv_wnd: conn.rcv_wnd,
                    segments: new_segs,
                    retx_segments: retx_segs,
                    keepalive: ka,
                });
            }
        }
    }

    // Send all collected segments (no lock held)
    for pc in pending {
        let dst_mac = arp::lookup(pc.remote_ip).unwrap_or(MacAddr::BROADCAST);

        for (seq, data, seg_flags) in pc.segments.iter().chain(pc.retx_segments.iter()) {
            let pkt = build_tcp_packet(
                our_ip,
                pc.remote_ip,
                pc.local_port,
                pc.remote_port,
                *seq,
                pc.rcv_nxt,
                *seg_flags,
                pc.rcv_wnd,
                data,
            );
            send_ip_frame(our_mac, dst_mac, &pkt);
        }

        if let Some((ka_seq, ka_ack, ka_flags, ka_wnd)) = pc.keepalive {
            let pkt = build_tcp_packet(
                our_ip,
                pc.remote_ip,
                pc.local_port,
                pc.remote_port,
                ka_seq,
                ka_ack,
                ka_flags,
                ka_wnd,
                &[],
            );
            send_ip_frame(our_mac, dst_mac, &pkt);
        }
    }
}

/// Network driver trait — each NIC driver implements this
pub trait NetworkDriver: Send + Sync {
    fn send(&self, frame: &[u8]) -> Result<(), NetError>;
    fn recv(&self, buf: &mut [u8]) -> Result<usize, NetError>;
    fn mac_addr(&self) -> MacAddr;
}

/// Network errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    NoInterface,
    Timeout,
    ConnectionRefused,
    ConnectionReset,
    AddrInUse,
    AddrNotAvailable,
    BufferTooSmall,
    InvalidPacket,
    Unreachable,
    IoError,
}

/// Periodic network tick — drive TC token-bucket refills and any other
/// time-based network subsystems.
///
/// `current_ms` is the current system time in milliseconds since boot.
/// Call from the timer interrupt handler (or a dedicated network timer task).
pub fn net_tick(current_ms: u64) {
    tc::tc_tick(current_ms);
}
