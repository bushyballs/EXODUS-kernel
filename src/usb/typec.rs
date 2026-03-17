/// USB Type-C controller and Power Delivery negotiation
///
/// Part of the AIOS hardware layer.
/// Implements USB Type-C port management including CC pin state
/// detection, power role management, USB Power Delivery (PD)
/// negotiation protocol, alternate mode support, and VBUS/VCONN control.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Type-C CC pin state
#[derive(Clone, Copy, PartialEq)]
pub enum CcState {
    Open,
    Rd, // Pull-down (device/sink)
    Ra, // Audio adapter accessory
}

/// PD power role
#[derive(Clone, Copy, PartialEq)]
pub enum PowerRole {
    Source,
    Sink,
}

/// Data role
#[derive(Clone, Copy, PartialEq)]
pub enum DataRole {
    Dfp, // Downstream Facing Port (host)
    Ufp, // Upstream Facing Port (device)
}

/// CC line voltage level (Rp advertisement)
#[derive(Clone, Copy, PartialEq)]
pub enum CcVoltageLevel {
    Default, // Default USB power (500mA / 900mA)
    Mid,     // 1.5A @ 5V
    High,    // 3.0A @ 5V
}

/// PD message type
#[derive(Clone, Copy, PartialEq)]
pub enum PdMessageType {
    // Control messages
    GoodCrc,
    GotoMin,
    Accept,
    Reject,
    Ping,
    PsRdy,
    GetSourceCap,
    GetSinkCap,
    DrSwap,
    PrSwap,
    VconnSwap,
    SoftReset,
    // Data messages
    SourceCapabilities,
    Request,
    Bist,
    SinkCapabilities,
    VendorDefined,
}

/// PD power data object (PDO) - source capability
#[derive(Clone, Copy)]
pub struct PowerDataObject {
    /// PDO type (0=fixed, 1=battery, 2=variable, 3=augmented)
    pub pdo_type: u8,
    /// Voltage in millivolts
    pub voltage_mv: u32,
    /// Maximum current in milliamps
    pub current_ma: u32,
    /// Flags (dual-role power, USB suspend, etc.)
    pub flags: u8,
}

impl PowerDataObject {
    fn fixed(voltage_mv: u32, current_ma: u32) -> Self {
        PowerDataObject {
            pdo_type: 0,
            voltage_mv,
            current_ma,
            flags: 0,
        }
    }

    /// Serialize PDO to 32-bit format
    fn to_u32(&self) -> u32 {
        match self.pdo_type {
            0 => {
                // Fixed supply PDO
                let voltage_50mv = (self.voltage_mv / 50) as u32;
                let current_10ma = (self.current_ma / 10) as u32;
                (0 << 30)
                    | ((voltage_50mv & 0x3FF) << 10)
                    | (current_10ma & 0x3FF)
                    | ((self.flags as u32 & 0x3F) << 20)
            }
            _ => 0,
        }
    }

    /// Parse PDO from 32-bit format
    fn from_u32(raw: u32) -> Self {
        let pdo_type = ((raw >> 30) & 0x3) as u8;
        match pdo_type {
            0 => {
                let voltage_50mv = (raw >> 10) & 0x3FF;
                let current_10ma = raw & 0x3FF;
                let flags = ((raw >> 20) & 0x3F) as u8;
                PowerDataObject {
                    pdo_type: 0,
                    voltage_mv: voltage_50mv * 50,
                    current_ma: current_10ma * 10,
                    flags,
                }
            }
            _ => PowerDataObject {
                pdo_type,
                voltage_mv: 5000,
                current_ma: 500,
                flags: 0,
            },
        }
    }

    /// Power in milliwatts
    fn power_mw(&self) -> u32 {
        (self.voltage_mv / 1000) * self.current_ma
    }
}

/// PD negotiation state machine
#[derive(Clone, Copy, PartialEq)]
enum PdState {
    Disabled,
    WaitForSourceCap,
    EvaluateSourceCap,
    RequestSent,
    WaitForAccept,
    TransitionPower,
    Ready,
    Error,
}

/// Alternate mode (DisplayPort, Thunderbolt, etc.)
#[derive(Clone, Copy, PartialEq)]
pub enum AltMode {
    None,
    DisplayPort,
    Thunderbolt,
    Mhl,
    AudioAccessory,
}

/// Type-C port state
pub struct TypeCPort {
    pub cc_state: CcState,
    pub power_role: PowerRole,
    pub voltage_mv: u32,
    pub current_ma: u32,
    /// Data role
    data_role: DataRole,
    /// CC voltage level detected
    cc_level: CcVoltageLevel,
    /// Which CC line is active (1 or 2)
    active_cc: u8,
    /// PD negotiation state
    pd_state: PdState,
    /// Source capabilities received
    source_caps: [PowerDataObject; 7],
    source_cap_count: u8,
    /// Sink capabilities (what we can accept)
    sink_caps: [PowerDataObject; 4],
    sink_cap_count: u8,
    /// Selected PDO index (1-based, 0 = none)
    selected_pdo: u8,
    /// VBUS enabled
    vbus_enabled: bool,
    /// VCONN enabled (for powered cables/accessories)
    vconn_enabled: bool,
    /// Active alternate mode
    alt_mode: AltMode,
    /// PD message ID counter (wraps at 7)
    msg_id: u8,
    /// Port connected
    connected: bool,
    /// PD revision supported (2 = PD 2.0, 3 = PD 3.0)
    pd_revision: u8,
    /// Negotiation attempts
    negotiation_attempts: u32,
    /// Successful negotiations
    successful_negotiations: u32,
    /// Timer (ms)
    timer_ms: u32,
    /// Operational
    operational: bool,
}

static PORT: Mutex<Option<TypeCPort>> = Mutex::new(None);

impl TypeCPort {
    fn new() -> Self {
        // Default sink capabilities
        let mut sink_caps = [PowerDataObject::fixed(0, 0); 4];
        sink_caps[0] = PowerDataObject::fixed(5000, 3000); // 5V 3A
        sink_caps[1] = PowerDataObject::fixed(9000, 3000); // 9V 3A
        sink_caps[2] = PowerDataObject::fixed(15000, 2000); // 15V 2A
        sink_caps[3] = PowerDataObject::fixed(20000, 2250); // 20V 2.25A (45W)

        TypeCPort {
            cc_state: CcState::Open,
            power_role: PowerRole::Sink,
            voltage_mv: 5000,
            current_ma: 500,
            data_role: DataRole::Ufp,
            cc_level: CcVoltageLevel::Default,
            active_cc: 0,
            pd_state: PdState::Disabled,
            source_caps: [PowerDataObject::fixed(0, 0); 7],
            source_cap_count: 0,
            sink_caps,
            sink_cap_count: 4,
            selected_pdo: 0,
            vbus_enabled: false,
            vconn_enabled: false,
            alt_mode: AltMode::None,
            msg_id: 0,
            connected: false,
            pd_revision: 3,
            negotiation_attempts: 0,
            successful_negotiations: 0,
            timer_ms: 0,
            operational: true,
        }
    }

    /// Detect CC pin state and determine connection
    fn detect_cc(&mut self) {
        // In real hardware: read CC1/CC2 ADC values from TCPC
        // Simulate detection based on cc_state
        match self.cc_state {
            CcState::Open => {
                self.connected = false;
                self.active_cc = 0;
                serial_println!("    [typec] CC: open (no connection)");
            }
            CcState::Rd => {
                // Rd detected -> we are source, partner is sink
                // Or partner has Rd -> partner is sink
                self.connected = true;
                self.active_cc = 1;
                serial_println!("    [typec] CC: Rd detected on CC{}", self.active_cc);
            }
            CcState::Ra => {
                // Ra -> audio accessory
                self.connected = true;
                self.active_cc = 1;
                self.alt_mode = AltMode::AudioAccessory;
                serial_println!("    [typec] CC: Ra detected (audio accessory)");
            }
        }
    }

    /// Start PD negotiation
    fn start_pd_negotiation(&mut self) {
        if !self.connected {
            serial_println!("    [typec] cannot negotiate PD: not connected");
            return;
        }
        self.pd_state = PdState::WaitForSourceCap;
        self.negotiation_attempts = self.negotiation_attempts.saturating_add(1);
        serial_println!(
            "    [typec] PD negotiation started (attempt {})",
            self.negotiation_attempts
        );
    }

    /// Receive source capabilities from the connected source
    fn receive_source_caps(&mut self, caps: &[PowerDataObject]) {
        let count = caps.len().min(7);
        for i in 0..count {
            self.source_caps[i] = caps[i];
        }
        self.source_cap_count = count as u8;
        self.pd_state = PdState::EvaluateSourceCap;

        serial_println!("    [typec] received {} source capabilities:", count);
        for i in 0..count {
            serial_println!(
                "      PDO{}: {}mV {}mA ({}mW)",
                i + 1,
                self.source_caps[i].voltage_mv,
                self.source_caps[i].current_ma,
                self.source_caps[i].power_mw()
            );
        }
    }

    /// Select the best PDO matching our requirements
    fn evaluate_and_request(
        &mut self,
        target_voltage_mv: u32,
        target_current_ma: u32,
    ) -> Result<(), ()> {
        if self.pd_state != PdState::EvaluateSourceCap || self.source_cap_count == 0 {
            return Err(());
        }

        // Find the best matching PDO
        let mut best_idx: Option<u8> = None;
        let mut best_power: u32 = 0;

        for i in 0..self.source_cap_count as usize {
            let pdo = &self.source_caps[i];
            if pdo.voltage_mv <= target_voltage_mv && pdo.current_ma >= target_current_ma {
                let power = pdo.power_mw();
                if power > best_power {
                    best_power = power;
                    best_idx = Some(i as u8 + 1); // 1-based PDO index
                }
            }
        }

        // If no exact match, find closest voltage that doesn't exceed target
        if best_idx.is_none() {
            let mut closest_voltage_diff = u32::MAX;
            for i in 0..self.source_cap_count as usize {
                let pdo = &self.source_caps[i];
                if pdo.voltage_mv <= target_voltage_mv {
                    let diff = target_voltage_mv - pdo.voltage_mv;
                    if diff < closest_voltage_diff {
                        closest_voltage_diff = diff;
                        best_idx = Some(i as u8 + 1);
                    }
                }
            }
        }

        // Fallback: always accept first PDO (5V default)
        let selected = best_idx.unwrap_or(1);
        self.selected_pdo = selected;
        self.pd_state = PdState::RequestSent;

        let pdo_idx = (selected - 1) as usize;
        if pdo_idx < self.source_cap_count as usize {
            serial_println!(
                "    [typec] requesting PDO{}: {}mV {}mA",
                selected,
                self.source_caps[pdo_idx].voltage_mv,
                self.source_caps[pdo_idx].current_ma
            );
        }

        self.msg_id = (self.msg_id + 1) & 0x07;
        Ok(())
    }

    /// Handle Accept message from source
    fn handle_accept(&mut self) {
        if self.pd_state == PdState::RequestSent {
            self.pd_state = PdState::TransitionPower;
            serial_println!("    [typec] PD request accepted, transitioning power");
        }
    }

    /// Handle PS_RDY message from source (power supply ready)
    fn handle_ps_rdy(&mut self) {
        if self.pd_state == PdState::TransitionPower {
            let pdo_idx = (self.selected_pdo.saturating_sub(1)) as usize;
            if pdo_idx < self.source_cap_count as usize {
                self.voltage_mv = self.source_caps[pdo_idx].voltage_mv;
                self.current_ma = self.source_caps[pdo_idx].current_ma;
            }
            self.vbus_enabled = true;
            self.pd_state = PdState::Ready;
            self.successful_negotiations = self.successful_negotiations.saturating_add(1);
            serial_println!(
                "    [typec] PD ready: {}mV {}mA ({}mW)",
                self.voltage_mv,
                self.current_ma,
                (self.voltage_mv / 1000) * self.current_ma
            );
        }
    }

    /// Handle Reject message
    fn handle_reject(&mut self) {
        if self.pd_state == PdState::RequestSent || self.pd_state == PdState::WaitForAccept {
            serial_println!("    [typec] PD request rejected");
            // Fall back to default 5V
            self.voltage_mv = 5000;
            self.current_ma = match self.cc_level {
                CcVoltageLevel::Default => 500,
                CcVoltageLevel::Mid => 1500,
                CcVoltageLevel::High => 3000,
            };
            self.pd_state = PdState::Ready;
        }
    }

    /// Perform power role swap
    fn power_role_swap(&mut self) -> Result<(), ()> {
        if self.pd_state != PdState::Ready {
            return Err(());
        }
        match self.power_role {
            PowerRole::Source => {
                self.power_role = PowerRole::Sink;
                self.vbus_enabled = false;
            }
            PowerRole::Sink => {
                self.power_role = PowerRole::Source;
                self.vbus_enabled = true;
            }
        }
        serial_println!(
            "    [typec] power role swapped to {:?}",
            match self.power_role {
                PowerRole::Source => "source",
                PowerRole::Sink => "sink",
            }
        );
        Ok(())
    }

    /// Perform data role swap
    fn data_role_swap(&mut self) -> Result<(), ()> {
        if self.pd_state != PdState::Ready {
            return Err(());
        }
        match self.data_role {
            DataRole::Dfp => self.data_role = DataRole::Ufp,
            DataRole::Ufp => self.data_role = DataRole::Dfp,
        }
        serial_println!(
            "    [typec] data role swapped to {:?}",
            match self.data_role {
                DataRole::Dfp => "DFP (host)",
                DataRole::Ufp => "UFP (device)",
            }
        );
        Ok(())
    }

    /// Enter an alternate mode
    fn enter_alt_mode(&mut self, mode: AltMode) -> Result<(), ()> {
        if self.pd_state != PdState::Ready {
            return Err(());
        }
        self.alt_mode = mode;
        serial_println!(
            "    [typec] entered alt mode: {:?}",
            match mode {
                AltMode::None => "none",
                AltMode::DisplayPort => "DisplayPort",
                AltMode::Thunderbolt => "Thunderbolt",
                AltMode::Mhl => "MHL",
                AltMode::AudioAccessory => "audio",
            }
        );
        Ok(())
    }

    /// Get negotiated power in milliwatts
    fn negotiated_power_mw(&self) -> u32 {
        (self.voltage_mv / 1000) * self.current_ma
    }

    /// Check if PD negotiation is complete
    fn is_pd_ready(&self) -> bool {
        self.pd_state == PdState::Ready
    }
}

/// Negotiate USB Power Delivery (public API)
pub fn negotiate_pd(voltage_mv: u32, current_ma: u32) -> Result<(), ()> {
    let mut guard = PORT.lock();
    match guard.as_mut() {
        Some(port) => {
            // Simulate full PD negotiation sequence
            if port.source_cap_count == 0 {
                // Provide default source caps for simulation
                let default_caps = [
                    PowerDataObject::fixed(5000, 3000),
                    PowerDataObject::fixed(9000, 3000),
                    PowerDataObject::fixed(15000, 2000),
                    PowerDataObject::fixed(20000, 2250),
                ];
                port.receive_source_caps(&default_caps);
            }
            port.evaluate_and_request(voltage_mv, current_ma)?;
            port.handle_accept();
            port.handle_ps_rdy();
            Ok(())
        }
        None => {
            serial_println!("    [typec] port not initialized");
            Err(())
        }
    }
}

/// Get current negotiated voltage/current
pub fn negotiated_power() -> (u32, u32) {
    let guard = PORT.lock();
    match guard.as_ref() {
        Some(port) => (port.voltage_mv, port.current_ma),
        None => (5000, 500),
    }
}

/// Check if PD negotiation is complete
pub fn is_pd_ready() -> bool {
    let guard = PORT.lock();
    match guard.as_ref() {
        Some(port) => port.is_pd_ready(),
        None => false,
    }
}

/// Get negotiated power in milliwatts
pub fn power_mw() -> u32 {
    let guard = PORT.lock();
    match guard.as_ref() {
        Some(port) => port.negotiated_power_mw(),
        None => 2500, // 5V * 500mA default
    }
}

/// Initialize the USB Type-C subsystem
pub fn init() {
    let mut guard = PORT.lock();
    let mut port = TypeCPort::new();
    // Simulate CC detection (default: sink, Rd on CC1)
    port.cc_state = CcState::Rd;
    port.detect_cc();
    port.connected = true;
    *guard = Some(port);
    serial_println!("    [typec] USB Type-C port initialized: PD 3.0, sink, 5V/500mA default");
}
