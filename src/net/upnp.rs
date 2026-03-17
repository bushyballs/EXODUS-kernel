use crate::sync::Mutex;
/// Universal Plug and Play (UPnP) device management
///
/// Provides UPnP IGD (Internet Gateway Device) port mapping, device
/// description parsing, SOAP action building, service discovery
/// integration, and port mapping lifecycle management.
///
/// Inspired by: UPnP Device Architecture 2.0, WANIPConnection:2.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// UPnP service types
// ---------------------------------------------------------------------------

/// Well-known UPnP service types
pub const SVC_WAN_IP_CONNECTION: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";
pub const SVC_WAN_PPP_CONNECTION: &str = "urn:schemas-upnp-org:service:WANPPPConnection:1";
pub const SVC_WAN_COMMON_IFC: &str = "urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1";

/// UPnP device types
pub const DEV_IGD: &str = "urn:schemas-upnp-org:device:InternetGatewayDevice:1";
pub const DEV_WAN: &str = "urn:schemas-upnp-org:device:WANDevice:1";
pub const DEV_WAN_CONNECTION: &str = "urn:schemas-upnp-org:device:WANConnectionDevice:1";

// ---------------------------------------------------------------------------
// Port mapping
// ---------------------------------------------------------------------------

/// Protocol for port mapping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Tcp => "TCP",
            Protocol::Udp => "UDP",
        }
    }
}

/// UPnP port mapping entry
#[derive(Debug, Clone)]
pub struct PortMapping {
    /// External port on the gateway
    pub external_port: u16,
    /// Internal (client) IP address
    pub internal_addr: [u8; 4],
    /// Internal (client) port
    pub internal_port: u16,
    /// TCP or UDP
    pub protocol: Protocol,
    /// Human-readable description
    pub description: String,
    /// Lease duration in seconds (0 = indefinite)
    pub lease_duration: u32,
    /// Whether the mapping is enabled
    pub enabled: bool,
}

impl PortMapping {
    pub fn new(
        external_port: u16,
        internal_addr: [u8; 4],
        internal_port: u16,
        protocol: Protocol,
        description: &str,
        lease_duration: u32,
    ) -> Self {
        PortMapping {
            external_port,
            internal_addr,
            internal_port,
            protocol,
            description: String::from(description),
            lease_duration,
            enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// SOAP action building
// ---------------------------------------------------------------------------

/// Build a SOAP envelope for a UPnP action
pub fn build_soap_action(service_type: &str, action: &str, args: &[(&str, &str)]) -> Vec<u8> {
    let mut xml = String::from("<?xml version=\"1.0\"?>\r\n");
    xml.push_str("<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" ");
    xml.push_str("s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\r\n");
    xml.push_str("<s:Body>\r\n");
    xml.push_str("<u:");
    xml.push_str(action);
    xml.push_str(" xmlns:u=\"");
    xml.push_str(service_type);
    xml.push_str("\">\r\n");
    for (name, value) in args {
        xml.push_str("<");
        xml.push_str(name);
        xml.push_str(">");
        xml_escape_into(&mut xml, value);
        xml.push_str("</");
        xml.push_str(name);
        xml.push_str(">\r\n");
    }
    xml.push_str("</u:");
    xml.push_str(action);
    xml.push_str(">\r\n");
    xml.push_str("</s:Body>\r\n");
    xml.push_str("</s:Envelope>\r\n");
    xml.into_bytes()
}

/// Build AddPortMapping SOAP action
pub fn build_add_port_mapping(service_type: &str, mapping: &PortMapping) -> Vec<u8> {
    let ext_port = alloc::format!("{}", mapping.external_port);
    let int_port = alloc::format!("{}", mapping.internal_port);
    let int_addr = alloc::format!(
        "{}.{}.{}.{}",
        mapping.internal_addr[0],
        mapping.internal_addr[1],
        mapping.internal_addr[2],
        mapping.internal_addr[3]
    );
    let lease = alloc::format!("{}", mapping.lease_duration);
    let enabled = if mapping.enabled { "1" } else { "0" };

    build_soap_action(
        service_type,
        "AddPortMapping",
        &[
            ("NewRemoteHost", ""),
            ("NewExternalPort", &ext_port),
            ("NewProtocol", mapping.protocol.as_str()),
            ("NewInternalPort", &int_port),
            ("NewInternalClient", &int_addr),
            ("NewEnabled", enabled),
            ("NewPortMappingDescription", &mapping.description),
            ("NewLeaseDuration", &lease),
        ],
    )
}

/// Build DeletePortMapping SOAP action
pub fn build_delete_port_mapping(
    service_type: &str,
    external_port: u16,
    protocol: Protocol,
) -> Vec<u8> {
    let port_str = alloc::format!("{}", external_port);
    build_soap_action(
        service_type,
        "DeletePortMapping",
        &[
            ("NewRemoteHost", ""),
            ("NewExternalPort", &port_str),
            ("NewProtocol", protocol.as_str()),
        ],
    )
}

/// Build GetExternalIPAddress SOAP action
pub fn build_get_external_ip(service_type: &str) -> Vec<u8> {
    build_soap_action(service_type, "GetExternalIPAddress", &[])
}

/// Build GetSpecificPortMappingEntry SOAP action
pub fn build_get_port_mapping(
    service_type: &str,
    external_port: u16,
    protocol: Protocol,
) -> Vec<u8> {
    let port_str = alloc::format!("{}", external_port);
    build_soap_action(
        service_type,
        "GetSpecificPortMappingEntry",
        &[
            ("NewRemoteHost", ""),
            ("NewExternalPort", &port_str),
            ("NewProtocol", protocol.as_str()),
        ],
    )
}

/// Escape XML special characters
fn xml_escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
}

// ---------------------------------------------------------------------------
// SOAP response parsing (simplified)
// ---------------------------------------------------------------------------

/// Extract a value between XML tags from a SOAP response
pub fn extract_xml_value(xml: &str, tag: &str) -> Option<String> {
    let open_tag = alloc::format!("<{}", tag);
    let close_tag = alloc::format!("</{}>", tag);

    let start = xml.find(open_tag.as_str())?;
    // Find end of opening tag
    let tag_end = xml[start..].find('>')? + start + 1;
    let end = xml[tag_end..].find(close_tag.as_str())? + tag_end;
    let value = &xml[tag_end..end];
    Some(String::from(value))
}

/// Parse a UPnP error code from a SOAP fault
pub fn parse_upnp_error(xml: &str) -> Option<u32> {
    if let Some(code_str) = extract_xml_value(xml, "errorCode") {
        parse_u32_simple(&code_str)
    } else {
        None
    }
}

fn parse_u32_simple(s: &str) -> Option<u32> {
    let mut result: u32 = 0;
    let mut found = false;
    for &b in s.as_bytes() {
        if b >= b'0' && b <= b'9' {
            result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
            found = true;
        } else if found {
            break;
        }
    }
    if found {
        Some(result)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// UPnP IGD client
// ---------------------------------------------------------------------------

/// Discovered UPnP gateway service
#[derive(Debug, Clone)]
pub struct GatewayService {
    /// Control URL for SOAP actions
    pub control_url: String,
    /// Service type (WANIPConnection or WANPPPConnection)
    pub service_type: String,
}

/// UPnP IGD client state
pub struct UpnpClient {
    /// Discovered gateway
    pub gateway: Option<GatewayService>,
    /// Active port mappings
    pub mappings: Vec<PortMapping>,
    /// External IP address (if known)
    pub external_ip: Option<[u8; 4]>,
    /// Pending SOAP requests (control_url, soap_action_header, body)
    pub pending_requests: Vec<(String, String, Vec<u8>)>,
}

impl UpnpClient {
    fn new() -> Self {
        UpnpClient {
            gateway: None,
            mappings: Vec::new(),
            external_ip: None,
            pending_requests: Vec::new(),
        }
    }

    /// Set the discovered gateway
    pub fn set_gateway(&mut self, control_url: &str, service_type: &str) {
        self.gateway = Some(GatewayService {
            control_url: String::from(control_url),
            service_type: String::from(service_type),
        });
    }

    /// Add a port mapping
    pub fn add_port_mapping(&mut self, mapping: PortMapping) -> Result<(), UpnpError> {
        let gw = self.gateway.as_ref().ok_or(UpnpError::NoGateway)?;
        let body = build_add_port_mapping(&gw.service_type, &mapping);
        let action = alloc::format!("\"{}#AddPortMapping\"", gw.service_type);
        self.pending_requests
            .push((gw.control_url.clone(), action, body));
        self.mappings.push(mapping);
        Ok(())
    }

    /// Remove a port mapping
    pub fn remove_port_mapping(
        &mut self,
        external_port: u16,
        protocol: Protocol,
    ) -> Result<(), UpnpError> {
        let gw = self.gateway.as_ref().ok_or(UpnpError::NoGateway)?;
        let body = build_delete_port_mapping(&gw.service_type, external_port, protocol);
        let action = alloc::format!("\"{}#DeletePortMapping\"", gw.service_type);
        self.pending_requests
            .push((gw.control_url.clone(), action, body));
        self.mappings
            .retain(|m| !(m.external_port == external_port && m.protocol == protocol));
        Ok(())
    }

    /// Request external IP address
    pub fn request_external_ip(&mut self) -> Result<(), UpnpError> {
        let gw = self.gateway.as_ref().ok_or(UpnpError::NoGateway)?;
        let body = build_get_external_ip(&gw.service_type);
        let action = alloc::format!("\"{}#GetExternalIPAddress\"", gw.service_type);
        self.pending_requests
            .push((gw.control_url.clone(), action, body));
        Ok(())
    }

    /// Remove all port mappings (cleanup)
    pub fn remove_all_mappings(&mut self) {
        let mappings: Vec<(u16, Protocol)> = self
            .mappings
            .iter()
            .map(|m| (m.external_port, m.protocol))
            .collect();
        for (port, proto) in mappings {
            let _ = self.remove_port_mapping(port, proto);
        }
    }

    /// Dequeue next pending SOAP request
    pub fn dequeue_request(&mut self) -> Option<(String, String, Vec<u8>)> {
        if self.pending_requests.is_empty() {
            None
        } else {
            Some(self.pending_requests.remove(0))
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpnpError {
    NotInitialized,
    NoGateway,
    ActionFailed,
    MappingConflict,
}

// ---------------------------------------------------------------------------
// Global subsystem
// ---------------------------------------------------------------------------

static UPNP: Mutex<Option<UpnpClient>> = Mutex::new(None);

pub fn init() {
    *UPNP.lock() = Some(UpnpClient::new());
    serial_println!("  Net: UPnP subsystem initialized");
}
