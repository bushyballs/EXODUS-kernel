/// USB On-The-Go (device/host role switching)
///
/// Part of the AIOS hardware layer.
/// Implements USB OTG role negotiation including ID pin detection,
/// VBUS sensing, Session Request Protocol (SRP), Host Negotiation
/// Protocol (HNP), and role-switch state machine.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Current OTG role
#[derive(Clone, Copy, PartialEq)]
pub enum OtgRole {
    Host,
    Device,
    Idle,
}

/// VBUS state
#[derive(Clone, Copy, PartialEq)]
pub enum VbusState {
    /// No VBUS detected
    Off,
    /// VBUS below session valid threshold (< 0.8V)
    BelowSession,
    /// VBUS at session valid level (0.8V - 4.0V)
    SessionValid,
    /// VBUS at valid level (4.0V - 5.25V)
    Valid,
    /// VBUS over-voltage detected
    OverVoltage,
}

/// SRP (Session Request Protocol) state
#[derive(Clone, Copy, PartialEq)]
enum SrpState {
    Idle,
    DataPulse,
    VbusPulse,
    Complete,
    Failed,
}

/// HNP (Host Negotiation Protocol) state
#[derive(Clone, Copy, PartialEq)]
enum HnpState {
    Idle,
    Requested,
    Accepted,
    Switching,
    Complete,
    Failed,
}

/// OTG event for state machine transitions
#[derive(Clone, Copy, PartialEq)]
pub enum OtgEvent {
    IdPinLow,    // ID pin grounded -> A-device (default host)
    IdPinHigh,   // ID pin floating -> B-device (default device)
    VbusRise,    // VBUS voltage detected
    VbusFall,    // VBUS removed
    SrpDetected, // Session request from B-device
    HnpRequest,  // Host negotiation request
    BusReset,    // USB bus reset
    Disconnect,  // Device disconnected
    Suspend,     // Bus suspended
    Resume,      // Bus resumed
}

/// OTG A-device / B-device designation
#[derive(Clone, Copy, PartialEq)]
enum OtgDevice {
    ADevice, // Connected to ID pin ground (mini-A plug)
    BDevice, // ID pin floating (mini-B plug)
    Unknown,
}

/// OTG controller state
pub struct OtgController {
    pub role: OtgRole,
    pub id_pin: bool,
    /// A-device or B-device designation
    device_type: OtgDevice,
    /// Current VBUS state
    vbus_state: VbusState,
    /// SRP state machine
    srp_state: SrpState,
    /// HNP state machine
    hnp_state: HnpState,
    /// Whether HNP is enabled (set by host via SetFeature)
    hnp_enabled: bool,
    /// Whether SRP is capable
    srp_capable: bool,
    /// Whether HNP is capable
    hnp_capable: bool,
    /// VBUS drive state (for A-device: supply VBUS)
    vbus_drive: bool,
    /// VBUS charge state (for SRP: charge VBUS capacitor)
    vbus_charge: bool,
    /// VBUS discharge state
    vbus_discharge: bool,
    /// Timer counter for protocol timeouts (ms)
    timer_ms: u32,
    /// SRP timeout (ms)
    srp_timeout_ms: u32,
    /// HNP timeout (ms)
    hnp_timeout_ms: u32,
    /// Bus suspend detected
    bus_suspended: bool,
    /// Number of role switches performed
    role_switch_count: u32,
    /// Last event processed
    last_event: Option<OtgEvent>,
    /// Operational flag
    operational: bool,
}

static OTG: Mutex<Option<OtgController>> = Mutex::new(None);

impl OtgController {
    pub fn new() -> Self {
        OtgController {
            role: OtgRole::Idle,
            id_pin: true, // floating = B-device by default
            device_type: OtgDevice::Unknown,
            vbus_state: VbusState::Off,
            srp_state: SrpState::Idle,
            hnp_state: HnpState::Idle,
            hnp_enabled: false,
            srp_capable: true,
            hnp_capable: true,
            vbus_drive: false,
            vbus_charge: false,
            vbus_discharge: false,
            timer_ms: 0,
            srp_timeout_ms: 5000,
            hnp_timeout_ms: 2000,
            bus_suspended: false,
            role_switch_count: 0,
            last_event: None,
            operational: true,
        }
    }

    /// Switch to a new OTG role
    pub fn switch_role(&mut self, role: OtgRole) -> Result<(), ()> {
        if !self.operational {
            serial_println!("    [usb-otg] controller not operational");
            return Err(());
        }

        let old_role = self.role;
        match (&old_role, &role) {
            (OtgRole::Idle, OtgRole::Host) => {
                // Transition to host mode
                self.enter_host_mode()?;
            }
            (OtgRole::Idle, OtgRole::Device) => {
                // Transition to device mode
                self.enter_device_mode()?;
            }
            (OtgRole::Host, OtgRole::Device) => {
                // Host -> Device requires HNP
                if !self.hnp_capable {
                    serial_println!("    [usb-otg] HNP not supported, cannot switch host->device");
                    return Err(());
                }
                self.perform_hnp_to_device()?;
            }
            (OtgRole::Device, OtgRole::Host) => {
                // Device -> Host requires HNP
                if !self.hnp_capable || !self.hnp_enabled {
                    serial_println!("    [usb-otg] HNP not enabled, cannot switch device->host");
                    return Err(());
                }
                self.perform_hnp_to_host()?;
            }
            (_, OtgRole::Idle) => {
                // Transition to idle
                self.enter_idle();
            }
            _ => {
                // Same role, no-op
                return Ok(());
            }
        }

        self.role_switch_count = self.role_switch_count.saturating_add(1);
        serial_println!(
            "    [usb-otg] role switched: {:?} -> {:?} (total switches: {})",
            role_name(&old_role),
            role_name(&role),
            self.role_switch_count
        );

        Ok(())
    }

    /// Process an OTG event
    fn process_event(&mut self, event: OtgEvent) {
        self.last_event = Some(event);

        match event {
            OtgEvent::IdPinLow => {
                // A-device detected (host by default)
                self.id_pin = false;
                self.device_type = OtgDevice::ADevice;
                serial_println!("    [usb-otg] ID pin low: A-device detected");
                if self.role == OtgRole::Idle {
                    let _ = self.enter_host_mode();
                }
            }
            OtgEvent::IdPinHigh => {
                // B-device detected (device by default)
                self.id_pin = true;
                self.device_type = OtgDevice::BDevice;
                serial_println!("    [usb-otg] ID pin high: B-device detected");
                if self.role == OtgRole::Idle {
                    let _ = self.enter_device_mode();
                }
            }
            OtgEvent::VbusRise => {
                self.vbus_state = VbusState::Valid;
                serial_println!("    [usb-otg] VBUS rise detected");
            }
            OtgEvent::VbusFall => {
                self.vbus_state = VbusState::Off;
                serial_println!("    [usb-otg] VBUS fall detected");
                if self.role == OtgRole::Device {
                    self.enter_idle();
                }
            }
            OtgEvent::SrpDetected => {
                serial_println!("    [usb-otg] SRP detected from B-device");
                if self.device_type == OtgDevice::ADevice && self.role == OtgRole::Idle {
                    // A-device should supply VBUS
                    self.vbus_drive = true;
                    self.vbus_state = VbusState::Valid;
                    let _ = self.enter_host_mode();
                }
            }
            OtgEvent::HnpRequest => {
                serial_println!("    [usb-otg] HNP request received");
                if self.hnp_capable {
                    self.hnp_state = HnpState::Requested;
                }
            }
            OtgEvent::BusReset => {
                serial_println!("    [usb-otg] bus reset");
                self.bus_suspended = false;
            }
            OtgEvent::Disconnect => {
                serial_println!("    [usb-otg] disconnect");
                self.enter_idle();
            }
            OtgEvent::Suspend => {
                self.bus_suspended = true;
                serial_println!("    [usb-otg] bus suspended");
                // During HNP, suspend signals role switch
                if self.hnp_state == HnpState::Accepted {
                    self.hnp_state = HnpState::Switching;
                }
            }
            OtgEvent::Resume => {
                self.bus_suspended = false;
                serial_println!("    [usb-otg] bus resumed");
            }
        }
    }

    /// Enter host mode
    fn enter_host_mode(&mut self) -> Result<(), ()> {
        // Enable VBUS supply (A-device supplies power)
        self.vbus_drive = true;
        self.vbus_state = VbusState::Valid;

        // Configure port for host mode
        self.role = OtgRole::Host;
        self.bus_suspended = false;

        serial_println!("    [usb-otg] entering host mode, VBUS enabled");
        Ok(())
    }

    /// Enter device mode
    fn enter_device_mode(&mut self) -> Result<(), ()> {
        // Disable VBUS supply (B-device doesn't supply power)
        self.vbus_drive = false;

        // Configure port for device mode
        self.role = OtgRole::Device;
        self.bus_suspended = false;

        serial_println!("    [usb-otg] entering device mode");
        Ok(())
    }

    /// Enter idle state
    fn enter_idle(&mut self) {
        self.role = OtgRole::Idle;
        self.vbus_drive = false;
        self.vbus_charge = false;
        self.vbus_discharge = false;
        self.hnp_state = HnpState::Idle;
        self.srp_state = SrpState::Idle;
        self.bus_suspended = false;
        serial_println!("    [usb-otg] entering idle state");
    }

    /// Perform SRP (B-device requests session from A-device)
    fn perform_srp(&mut self) -> Result<(), ()> {
        if self.device_type != OtgDevice::BDevice || !self.srp_capable {
            return Err(());
        }
        if self.vbus_state != VbusState::Off {
            return Err(());
        }

        // Step 1: Data-line pulsing
        self.srp_state = SrpState::DataPulse;
        serial_println!("    [usb-otg] SRP: data-line pulse");

        // Step 2: VBUS pulsing
        self.srp_state = SrpState::VbusPulse;
        self.vbus_charge = true;
        serial_println!("    [usb-otg] SRP: VBUS pulse");

        // In real hardware, we'd wait for A-device response
        self.vbus_charge = false;
        self.srp_state = SrpState::Complete;
        serial_println!("    [usb-otg] SRP complete");

        Ok(())
    }

    /// Perform HNP to switch from host to device
    fn perform_hnp_to_device(&mut self) -> Result<(), ()> {
        self.hnp_state = HnpState::Requested;
        serial_println!("    [usb-otg] HNP: host requesting switch to device");

        // Suspend the bus to signal B-device
        self.bus_suspended = true;
        self.hnp_state = HnpState::Switching;

        // Turn off VBUS drive briefly
        self.vbus_drive = false;

        // Switch role
        self.role = OtgRole::Device;
        self.hnp_state = HnpState::Complete;

        serial_println!("    [usb-otg] HNP complete: now in device mode");
        Ok(())
    }

    /// Perform HNP to switch from device to host
    fn perform_hnp_to_host(&mut self) -> Result<(), ()> {
        self.hnp_state = HnpState::Requested;
        serial_println!("    [usb-otg] HNP: device requesting switch to host");

        // Wait for bus suspend from current host
        self.hnp_state = HnpState::Accepted;

        // After suspend detected, take over as host
        self.hnp_state = HnpState::Switching;
        self.vbus_drive = true;
        self.vbus_state = VbusState::Valid;
        self.role = OtgRole::Host;
        self.hnp_state = HnpState::Complete;
        self.bus_suspended = false;

        serial_println!("    [usb-otg] HNP complete: now in host mode");
        Ok(())
    }

    /// Advance timer (called periodically, e.g., every 1ms)
    fn tick(&mut self, elapsed_ms: u32) {
        self.timer_ms += elapsed_ms;

        // Check SRP timeout
        if self.srp_state == SrpState::VbusPulse && self.timer_ms > self.srp_timeout_ms {
            self.srp_state = SrpState::Failed;
            self.vbus_charge = false;
            serial_println!("    [usb-otg] SRP timeout");
        }

        // Check HNP timeout
        if self.hnp_state == HnpState::Switching && self.timer_ms > self.hnp_timeout_ms {
            self.hnp_state = HnpState::Failed;
            serial_println!("    [usb-otg] HNP timeout");
        }
    }

    /// Get current role as string
    fn role_str(&self) -> &'static str {
        role_name(&self.role)
    }

    /// Get VBUS state
    fn vbus(&self) -> VbusState {
        self.vbus_state
    }

    /// Check if HNP is enabled
    fn is_hnp_enabled(&self) -> bool {
        self.hnp_enabled
    }

    /// Enable HNP (typically set by host via SET_FEATURE)
    fn enable_hnp(&mut self) {
        self.hnp_enabled = true;
        serial_println!("    [usb-otg] HNP enabled");
    }
}

fn role_name(role: &OtgRole) -> &'static str {
    match role {
        OtgRole::Host => "host",
        OtgRole::Device => "device",
        OtgRole::Idle => "idle",
    }
}

/// Process an OTG event (public API)
pub fn process_event(event: OtgEvent) {
    let mut guard = OTG.lock();
    if let Some(ctrl) = guard.as_mut() {
        ctrl.process_event(event);
    }
}

/// Get current OTG role
pub fn current_role() -> OtgRole {
    let guard = OTG.lock();
    match guard.as_ref() {
        Some(ctrl) => ctrl.role,
        None => OtgRole::Idle,
    }
}

/// Switch OTG role (public API)
pub fn switch_role(role: OtgRole) -> Result<(), ()> {
    let mut guard = OTG.lock();
    match guard.as_mut() {
        Some(ctrl) => ctrl.switch_role(role),
        None => Err(()),
    }
}

/// Perform SRP (public API)
pub fn perform_srp() -> Result<(), ()> {
    let mut guard = OTG.lock();
    match guard.as_mut() {
        Some(ctrl) => ctrl.perform_srp(),
        None => Err(()),
    }
}

/// Initialize the OTG subsystem
pub fn init() {
    let mut guard = OTG.lock();
    let ctrl = OtgController::new();
    *guard = Some(ctrl);
    serial_println!("    [usb-otg] OTG controller initialized: SRP/HNP capable, idle");
}
