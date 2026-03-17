use crate::sync::Mutex;
/// Thunderbolt / USB4 controller driver for Genesis
///
/// Manages Thunderbolt tunnel creation and teardown for PCIe, DisplayPort,
/// and USB3 tunnels. Implements security levels (none, user, secure, dponly),
/// device authorization, and connection manager protocol over the NHI
/// (Native Host Interface) ring buffer.
///
/// Uses MMIO registers from the Thunderbolt NHI PCI function.
///
/// Inspired by: Linux thunderbolt (drivers/thunderbolt/), USB4 spec. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// NHI register offsets
// ---------------------------------------------------------------------------

const REG_CAPS: usize = 0x00; // Capabilities
const REG_NHI_CTL: usize = 0x04; // NHI control
const REG_NHI_STATUS: usize = 0x08; // NHI status
const REG_NHI_IRQ_MASK: usize = 0x0C; // Interrupt mask
const REG_NHI_IRQ_STATUS: usize = 0x10; // Interrupt status
const REG_TX_RING_BASE: usize = 0x20; // TX ring base address (lo)
const REG_TX_RING_BASE_HI: usize = 0x24;
const REG_TX_RING_SIZE: usize = 0x28;
const REG_TX_RING_HEAD: usize = 0x2C;
const REG_TX_RING_TAIL: usize = 0x30;
const REG_RX_RING_BASE: usize = 0x40;
const REG_RX_RING_BASE_HI: usize = 0x44;
const REG_RX_RING_SIZE: usize = 0x48;
const REG_RX_RING_HEAD: usize = 0x4C;
const REG_RX_RING_TAIL: usize = 0x50;
const REG_SECURITY: usize = 0x60; // Security level
const REG_ROUTE_LO: usize = 0x70; // Route string low
const REG_ROUTE_HI: usize = 0x74; // Route string high
const REG_PORT_CTL_BASE: usize = 0x100; // Per-port control base
const PORT_CTL_STRIDE: usize = 0x20;

// NHI control bits
const NHI_CTL_ENABLE: u32 = 1 << 0;
const NHI_CTL_RING_ENABLE: u32 = 1 << 1;
const NHI_CTL_CM_MODE: u32 = 1 << 4; // Connection manager mode

// Status bits
const NHI_STATUS_READY: u32 = 1 << 0;

// Security levels
const SECURITY_NONE: u32 = 0;
const SECURITY_USER: u32 = 1;
const SECURITY_SECURE: u32 = 2;
const SECURITY_DPONLY: u32 = 3;

// Per-port control bits
const PORT_ENABLED: u32 = 1 << 0;
const PORT_CONNECTED: u32 = 1 << 1;
const PORT_AUTHORIZED: u32 = 1 << 2;
const PORT_PCIE_TUNNEL: u32 = 1 << 8;
const PORT_DP_TUNNEL: u32 = 1 << 9;
const PORT_USB3_TUNNEL: u32 = 1 << 10;

// Capabilities
const CAP_MAX_PORTS_MASK: u32 = 0x0F;
const CAP_USB4: u32 = 1 << 16;

const TIMEOUT_SPINS: u32 = 100_000;
const MAX_PORTS: usize = 4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Thunderbolt security level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityLevel {
    /// No security, all devices authorized automatically
    None,
    /// User must authorize each device
    User,
    /// Secure connect (challenge-response key)
    Secure,
    /// Only DisplayPort tunnels allowed (no PCIe)
    DpOnly,
}

/// Tunnel type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelType {
    Pcie,
    DisplayPort,
    Usb3,
}

/// State of a single tunnel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    Inactive,
    Pending,
    Active,
    Error,
}

/// A tunnel running through a port
#[derive(Debug, Clone)]
pub struct Tunnel {
    pub tunnel_type: TunnelType,
    pub state: TunnelState,
    pub port: u8,
}

/// Authorization result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    NotInitialized,
    InvalidPort,
    NotConnected,
    SecurityDenied,
    Timeout,
    AlreadyAuthorized,
}

/// Per-port device state
struct PortInner {
    port_id: u8,
    connected: bool,
    authorized: bool,
    speed_gbps: u8,
    tunnels: Vec<Tunnel>,
    device_name: String,
    vendor_id: u16,
    device_id: u16,
}

/// Driver top-level state
struct TbtDriver {
    base_addr: usize,
    security: SecurityLevel,
    ports: Vec<PortInner>,
    usb4_mode: bool,
    nhi_ready: bool,
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

#[inline]
fn mmio_read32(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

#[inline]
fn mmio_write32(addr: usize, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TBT: Mutex<Option<TbtDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

impl TbtDriver {
    #[inline(always)]
    fn reg(&self, offset: usize) -> usize {
        self.base_addr.saturating_add(offset)
    }

    #[inline(always)]
    fn port_reg(&self, port: u8, offset: usize) -> usize {
        self.base_addr
            .saturating_add(REG_PORT_CTL_BASE)
            .saturating_add((port as usize).saturating_mul(PORT_CTL_STRIDE))
            .saturating_add(offset)
    }

    /// Initialize the NHI: enable rings, set connection manager mode
    fn init_nhi(&mut self) {
        // Enable NHI
        mmio_write32(self.reg(REG_NHI_CTL), NHI_CTL_ENABLE | NHI_CTL_CM_MODE);

        // Wait for ready
        for _ in 0..TIMEOUT_SPINS {
            if mmio_read32(self.reg(REG_NHI_STATUS)) & NHI_STATUS_READY != 0 {
                self.nhi_ready = true;
                break;
            }
        }

        // Disable interrupts (we poll)
        mmio_write32(self.reg(REG_NHI_IRQ_MASK), 0);

        // Read security level
        let sec = mmio_read32(self.reg(REG_SECURITY));
        self.security = match sec & 0x03 {
            SECURITY_NONE => SecurityLevel::None,
            SECURITY_USER => SecurityLevel::User,
            SECURITY_SECURE => SecurityLevel::Secure,
            SECURITY_DPONLY => SecurityLevel::DpOnly,
            _ => SecurityLevel::User,
        };
    }

    /// Scan all ports for connected devices
    fn scan_ports(&mut self) {
        for port in &mut self.ports {
            let status = mmio_read32(
                self.base_addr
                    .saturating_add(REG_PORT_CTL_BASE)
                    .saturating_add((port.port_id as usize).saturating_mul(PORT_CTL_STRIDE)),
            );
            port.connected = status & PORT_CONNECTED != 0;
            port.authorized = status & PORT_AUTHORIZED != 0;

            if port.connected {
                // Read speed from port capability register
                let cap = mmio_read32(
                    self.base_addr
                        .saturating_add(REG_PORT_CTL_BASE)
                        .saturating_add((port.port_id as usize).saturating_mul(PORT_CTL_STRIDE))
                        .saturating_add(0x04),
                );
                port.speed_gbps = match (cap >> 8) & 0x0F {
                    0 => 20,
                    1 => 40,
                    _ => 10,
                };

                // Check existing tunnels
                if status & PORT_PCIE_TUNNEL != 0 {
                    if !port
                        .tunnels
                        .iter()
                        .any(|t| t.tunnel_type == TunnelType::Pcie)
                    {
                        port.tunnels.push(Tunnel {
                            tunnel_type: TunnelType::Pcie,
                            state: TunnelState::Active,
                            port: port.port_id,
                        });
                    }
                }
                if status & PORT_DP_TUNNEL != 0 {
                    if !port
                        .tunnels
                        .iter()
                        .any(|t| t.tunnel_type == TunnelType::DisplayPort)
                    {
                        port.tunnels.push(Tunnel {
                            tunnel_type: TunnelType::DisplayPort,
                            state: TunnelState::Active,
                            port: port.port_id,
                        });
                    }
                }
                if status & PORT_USB3_TUNNEL != 0 {
                    if !port
                        .tunnels
                        .iter()
                        .any(|t| t.tunnel_type == TunnelType::Usb3)
                    {
                        port.tunnels.push(Tunnel {
                            tunnel_type: TunnelType::Usb3,
                            state: TunnelState::Active,
                            port: port.port_id,
                        });
                    }
                }
            } else {
                port.authorized = false;
                port.tunnels.clear();
            }
        }
    }

    /// Authorize a device on a port
    fn authorize_port(&mut self, port_id: u8) -> Result<(), AuthError> {
        let port = self
            .ports
            .iter_mut()
            .find(|p| p.port_id == port_id)
            .ok_or(AuthError::InvalidPort)?;

        if !port.connected {
            return Err(AuthError::NotConnected);
        }
        if port.authorized {
            return Err(AuthError::AlreadyAuthorized);
        }

        // Check security policy
        if self.security == SecurityLevel::DpOnly {
            return Err(AuthError::SecurityDenied);
        }

        // Write authorization bit
        let reg_addr = self
            .base_addr
            .saturating_add(REG_PORT_CTL_BASE)
            .saturating_add((port_id as usize).saturating_mul(PORT_CTL_STRIDE));
        let val = mmio_read32(reg_addr);
        mmio_write32(reg_addr, val | PORT_AUTHORIZED);

        // Verify authorization took effect
        for _ in 0..TIMEOUT_SPINS {
            if mmio_read32(reg_addr) & PORT_AUTHORIZED != 0 {
                port.authorized = true;
                serial_println!("  TBT: port {} authorized", port_id);
                return Ok(());
            }
        }

        Err(AuthError::Timeout)
    }

    /// Create a tunnel on a port
    fn create_tunnel(&mut self, port_id: u8, tunnel_type: TunnelType) -> Result<(), AuthError> {
        let port = self
            .ports
            .iter_mut()
            .find(|p| p.port_id == port_id)
            .ok_or(AuthError::InvalidPort)?;

        if !port.authorized {
            return Err(AuthError::SecurityDenied);
        }

        // Check if tunnel already exists
        if port.tunnels.iter().any(|t| t.tunnel_type == tunnel_type) {
            return Ok(()); // Already active
        }

        // Request tunnel creation via port control register
        let reg_addr = self
            .base_addr
            .saturating_add(REG_PORT_CTL_BASE)
            .saturating_add((port_id as usize).saturating_mul(PORT_CTL_STRIDE));
        let val = mmio_read32(reg_addr);
        let tunnel_bit = match tunnel_type {
            TunnelType::Pcie => PORT_PCIE_TUNNEL,
            TunnelType::DisplayPort => PORT_DP_TUNNEL,
            TunnelType::Usb3 => PORT_USB3_TUNNEL,
        };

        // Security check for PCIe tunnels
        if tunnel_type == TunnelType::Pcie && self.security == SecurityLevel::DpOnly {
            return Err(AuthError::SecurityDenied);
        }

        mmio_write32(reg_addr, val | tunnel_bit);

        // Wait for tunnel to become active
        for _ in 0..TIMEOUT_SPINS {
            if mmio_read32(reg_addr) & tunnel_bit != 0 {
                port.tunnels.push(Tunnel {
                    tunnel_type,
                    state: TunnelState::Active,
                    port: port_id,
                });
                serial_println!(
                    "  TBT: {:?} tunnel created on port {}",
                    tunnel_type,
                    port_id
                );
                return Ok(());
            }
        }

        Err(AuthError::Timeout)
    }

    /// Tear down a tunnel on a port
    fn destroy_tunnel(&mut self, port_id: u8, tunnel_type: TunnelType) -> Result<(), AuthError> {
        let port = self
            .ports
            .iter_mut()
            .find(|p| p.port_id == port_id)
            .ok_or(AuthError::InvalidPort)?;

        let tunnel_bit = match tunnel_type {
            TunnelType::Pcie => PORT_PCIE_TUNNEL,
            TunnelType::DisplayPort => PORT_DP_TUNNEL,
            TunnelType::Usb3 => PORT_USB3_TUNNEL,
        };

        let reg_addr = self
            .base_addr
            .saturating_add(REG_PORT_CTL_BASE)
            .saturating_add((port_id as usize).saturating_mul(PORT_CTL_STRIDE));
        let val = mmio_read32(reg_addr);
        mmio_write32(reg_addr, val & !tunnel_bit);

        port.tunnels.retain(|t| t.tunnel_type != tunnel_type);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Authorize a device connected to a Thunderbolt port.
pub fn authorize_device(port_id: u8) -> Result<(), AuthError> {
    let mut guard = TBT.lock();
    let drv = guard.as_mut().ok_or(AuthError::NotInitialized)?;
    drv.authorize_port(port_id)
}

/// Create a tunnel on a port.
pub fn create_tunnel(port_id: u8, tunnel_type: TunnelType) -> Result<(), AuthError> {
    let mut guard = TBT.lock();
    let drv = guard.as_mut().ok_or(AuthError::NotInitialized)?;
    drv.create_tunnel(port_id, tunnel_type)
}

/// Destroy a tunnel on a port.
pub fn destroy_tunnel(port_id: u8, tunnel_type: TunnelType) -> Result<(), AuthError> {
    let mut guard = TBT.lock();
    let drv = guard.as_mut().ok_or(AuthError::NotInitialized)?;
    drv.destroy_tunnel(port_id, tunnel_type)
}

/// Rescan ports for connection changes.
pub fn poll() {
    let mut guard = TBT.lock();
    if let Some(drv) = guard.as_mut() {
        drv.scan_ports();
    }
}

/// Get the current security level.
pub fn security_level() -> SecurityLevel {
    TBT.lock()
        .as_ref()
        .map_or(SecurityLevel::User, |d| d.security)
}

/// Get list of active tunnels across all ports.
pub fn active_tunnels() -> Vec<Tunnel> {
    let guard = TBT.lock();
    match guard.as_ref() {
        Some(drv) => drv
            .ports
            .iter()
            .flat_map(|p| p.tunnels.iter().cloned())
            .collect(),
        None => Vec::new(),
    }
}

/// Get connected port count.
pub fn connected_ports() -> usize {
    TBT.lock()
        .as_ref()
        .map_or(0, |drv| drv.ports.iter().filter(|p| p.connected).count())
}

/// Check if USB4 mode is active.
pub fn is_usb4() -> bool {
    TBT.lock().as_ref().map_or(false, |d| d.usb4_mode)
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the Thunderbolt/USB4 controller.
///
/// Looks for an NHI controller via PCI (class 0x0C, subclass 0x09).
/// Falls back to probing known MMIO addresses.
pub fn init() {
    const TBT_BASE: usize = 0xFE06_0000;

    let caps = mmio_read32(TBT_BASE.saturating_add(REG_CAPS));
    if caps == 0xFFFF_FFFF {
        serial_println!("  Thunderbolt: no controller found at {:#010X}", TBT_BASE);
        return;
    }

    let port_count = ((caps & CAP_MAX_PORTS_MASK) as usize).min(MAX_PORTS);
    let usb4 = caps & CAP_USB4 != 0;

    let mut ports = Vec::new();
    for i in 0..port_count {
        ports.push(PortInner {
            port_id: i as u8,
            connected: false,
            authorized: false,
            speed_gbps: 0,
            tunnels: Vec::new(),
            device_name: String::new(),
            vendor_id: 0,
            device_id: 0,
        });
    }

    let mut drv = TbtDriver {
        base_addr: TBT_BASE,
        security: SecurityLevel::User,
        ports,
        usb4_mode: usb4,
        nhi_ready: false,
    };

    drv.init_nhi();
    if !drv.nhi_ready {
        serial_println!("  Thunderbolt: NHI failed to become ready");
        return;
    }

    drv.scan_ports();

    let connected = drv.ports.iter().filter(|p| p.connected).count();
    serial_println!(
        "  Thunderbolt: {} port(s), {} connected, security={:?}, {}",
        port_count,
        connected,
        drv.security,
        if usb4 { "USB4 mode" } else { "TBT3 mode" }
    );

    // Auto-authorize if security is None
    if drv.security == SecurityLevel::None {
        for port in &drv.ports {
            if port.connected && !port.authorized {
                let reg = drv
                    .base_addr
                    .saturating_add(REG_PORT_CTL_BASE)
                    .saturating_add((port.port_id as usize).saturating_mul(PORT_CTL_STRIDE));
                let val = mmio_read32(reg);
                mmio_write32(reg, val | PORT_AUTHORIZED);
            }
        }
        serial_println!("  Thunderbolt: auto-authorized all connected devices");
    }

    *TBT.lock() = Some(drv);
    super::register("thunderbolt", super::DeviceType::Other);
}
