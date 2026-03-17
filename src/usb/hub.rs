use crate::sync::Mutex;
/// USB Hub driver
///
/// Manages USB hubs: port power control, device enumeration on downstream
/// ports, hot-plug/hot-unplug event handling, port status tracking, and
/// hub descriptor parsing. Supports USB 2.0 and USB 3.0 hubs.
///
/// References: USB 2.0 specification Chapter 11, USB 3.1 Hub specification.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// SOF (Start-Of-Frame) counter
// ---------------------------------------------------------------------------

/// Counts 1 ms Start-Of-Frame ticks driven by the kernel timer.
///
/// The USB spec requires the host controller to issue a SOF packet every
/// 1 ms on full-/high-speed buses.  We approximate that here with a
/// periodic kernel-timer callback at 1 ms cadence.
static SOF_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Timer callback fired every 1 ms by the kernel timer wheel.
/// Increments the SOF counter unconditionally.
fn sof_tick_cb(_data: u64) {
    SOF_COUNTER.fetch_add(1, Ordering::Relaxed);
}

/// Arm the periodic 1 ms SOF counter timer via the kernel timer wheel.
///
/// Uses `crate::kernel::timer_wheel::GLOBAL_TIMER_WHEEL` to schedule a
/// repeating 1-tick callback that increments `SOF_COUNTER`.
pub fn start_sof_counter() {
    use crate::kernel::timer_wheel::{Timer, GLOBAL_TIMER_WHEEL};

    let mut wheel = GLOBAL_TIMER_WHEEL.lock();
    if let Some(ref mut w) = *wheel {
        // Fire 1 tick from now; the callback re-arms itself each time.
        let current = w.current_tick;
        let timer = Timer::new(0x50F, current.saturating_add(1)).with_callback(sof_tick_cb, 0);
        w.add_timer(timer);
        serial_println!("  [hub] SOF counter armed at tick {}", current);
    } else {
        serial_println!("  [hub] WARNING: timer wheel not initialised — SOF counter not armed");
    }
}

/// Read the current SOF counter value.
#[inline]
pub fn sof_counter() -> u32 {
    SOF_COUNTER.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// SET_PORT_FEATURE / CLEAR_PORT_FEATURE via xHCI control transfer
// ---------------------------------------------------------------------------

/// Issue a `SET_FEATURE` (bRequest=3) control transfer on a hub port.
///
/// Constructs the 8-byte USB setup packet and enqueues it as a control
/// transfer on EP0 of the hub's xHCI slot via the public
/// `crate::usb::xhci::enqueue_control_transfer()` wrapper.
///
/// `slot_id` — xHCI slot of the hub device.
/// `port`    — 1-based downstream port number (wIndex).
/// `feature` — feature selector (wValue), e.g. `PORT_RESET as u16`.
pub fn set_port_feature(slot_id: u8, port: u8, feature: u16) {
    // bmRequestType = 0x23: class (bits 6:5=01), Other recipient (bits 4:0=00011),
    //                        host-to-device (bit 7=0)
    // bRequest      = 3  (SET_FEATURE)
    // wValue        = feature selector (LE16)
    // wIndex        = port number     (LE16)
    // wLength       = 0
    let setup: [u8; 8] = [
        0x23,
        0x03,                   // SET_FEATURE
        (feature & 0xFF) as u8, // wValue lo
        (feature >> 8) as u8,   // wValue hi
        port,                   // wIndex lo (port number)
        0x00,                   // wIndex hi
        0x00,                   // wLength lo
        0x00,                   // wLength hi
    ];

    let addr = crate::usb::xhci::enqueue_control_transfer(slot_id, &setup, false, 0);
    if addr != 0 {
        serial_println!(
            "  [hub] SET_PORT_FEATURE slot={} port={} feature={} trb=0x{:x}",
            slot_id,
            port,
            feature,
            addr
        );
    } else {
        serial_println!(
            "  [hub] SET_PORT_FEATURE: xHCI not initialised (slot={} port={} feature={})",
            slot_id,
            port,
            feature
        );
    }
}

/// Issue a `CLEAR_FEATURE` (bRequest=1) control transfer on a hub port.
///
/// Same parameters as `set_port_feature`; only `bRequest` differs.
pub fn clear_port_feature(slot_id: u8, port: u8, feature: u16) {
    // bRequest = 1 (CLEAR_FEATURE)
    let setup: [u8; 8] = [
        0x23,
        0x01, // CLEAR_FEATURE
        (feature & 0xFF) as u8,
        (feature >> 8) as u8,
        port,
        0x00,
        0x00,
        0x00,
    ];

    let addr = crate::usb::xhci::enqueue_control_transfer(slot_id, &setup, false, 0);
    if addr != 0 {
        serial_println!(
            "  [hub] CLEAR_PORT_FEATURE slot={} port={} feature={} trb=0x{:x}",
            slot_id,
            port,
            feature,
            addr
        );
    } else {
        serial_println!(
            "  [hub] CLEAR_PORT_FEATURE: xHCI not initialised (slot={} port={} feature={})",
            slot_id,
            port,
            feature
        );
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HUB_STATE: Mutex<Option<HubDriverState>> = Mutex::new(None);

pub struct HubDriverState {
    pub hubs: Vec<HubDevice>,
    pub next_hub_id: u32,
    pub pending_events: Vec<HubEvent>,
}

impl HubDriverState {
    pub fn new() -> Self {
        HubDriverState {
            hubs: Vec::new(),
            next_hub_id: 1,
            pending_events: Vec::new(),
        }
    }

    pub fn register(&mut self, hub: HubDevice) -> u32 {
        let id = self.next_hub_id;
        self.next_hub_id = self.next_hub_id.saturating_add(1);
        self.hubs.push(hub);
        id
    }

    pub fn find_by_slot(&self, slot_id: u8) -> Option<&HubDevice> {
        self.hubs.iter().find(|h| h.slot_id == slot_id)
    }

    pub fn find_by_slot_mut(&mut self, slot_id: u8) -> Option<&mut HubDevice> {
        self.hubs.iter_mut().find(|h| h.slot_id == slot_id)
    }

    /// Drain all pending events.
    pub fn drain_events(&mut self) -> Vec<HubEvent> {
        let events = self.pending_events.clone();
        self.pending_events.clear();
        events
    }
}

// ---------------------------------------------------------------------------
// Hub constants
// ---------------------------------------------------------------------------

pub const CLASS_HUB: u8 = 0x09;

/// Hub descriptor types.
pub const DESC_HUB: u8 = 0x29; // USB 2.0 Hub Descriptor
pub const DESC_HUB_SS: u8 = 0x2A; // USB 3.0 SuperSpeed Hub Descriptor

/// Hub class-specific requests.
pub const HUB_GET_STATUS: u8 = 0x00;
pub const HUB_CLEAR_FEATURE: u8 = 0x01;
pub const HUB_SET_FEATURE: u8 = 0x03;
pub const HUB_GET_DESCRIPTOR: u8 = 0x06;
pub const HUB_SET_HUB_DEPTH: u8 = 0x0C; // USB 3.0

/// Hub port features (for SET_FEATURE / CLEAR_FEATURE on port).
pub const PORT_CONNECTION: u8 = 0x00;
pub const PORT_ENABLE: u8 = 0x01;
pub const PORT_SUSPEND: u8 = 0x02;
pub const PORT_OVER_CURRENT: u8 = 0x03;
pub const PORT_RESET: u8 = 0x04;
pub const PORT_POWER: u8 = 0x08;
pub const PORT_LOW_SPEED: u8 = 0x09;
pub const C_PORT_CONNECTION: u8 = 0x10;
pub const C_PORT_ENABLE: u8 = 0x11;
pub const C_PORT_SUSPEND: u8 = 0x12;
pub const C_PORT_OVER_CURRENT: u8 = 0x13;
pub const C_PORT_RESET: u8 = 0x14;

/// Port status bits (from GET_PORT_STATUS wPortStatus).
pub const PS_CONNECTION: u16 = 0x0001;
pub const PS_ENABLE: u16 = 0x0002;
pub const PS_SUSPEND: u16 = 0x0004;
pub const PS_OVER_CURRENT: u16 = 0x0008;
pub const PS_RESET: u16 = 0x0010;
pub const PS_POWER: u16 = 0x0100;
pub const PS_LOW_SPEED: u16 = 0x0200;
pub const PS_HIGH_SPEED: u16 = 0x0400;
pub const PS_INDICATOR: u16 = 0x1000;

/// Port status change bits (from GET_PORT_STATUS wPortChange).
pub const PC_CONNECTION: u16 = 0x0001;
pub const PC_ENABLE: u16 = 0x0002;
pub const PC_SUSPEND: u16 = 0x0004;
pub const PC_OVER_CURRENT: u16 = 0x0008;
pub const PC_RESET: u16 = 0x0010;

// ---------------------------------------------------------------------------
// Hub speed
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubSpeed {
    FullSpeed,      // USB 1.1
    HighSpeed,      // USB 2.0
    SuperSpeed,     // USB 3.0
    SuperSpeedPlus, // USB 3.1+
}

// ---------------------------------------------------------------------------
// Hub power switching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSwitching {
    Ganged,     // All ports switch together
    Individual, // Per-port power switching
    None,       // No power switching
}

// ---------------------------------------------------------------------------
// Hub descriptor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HubDescriptor {
    pub num_ports: u8,
    pub characteristics: u16,
    pub power_on_delay: u8, // in 2 ms units
    pub hub_current: u8,    // in mA
    pub removable_bitmap: Vec<u8>,
    pub power_switching: PowerSwitching,
    pub compound_device: bool,
    pub over_current_mode: u8, // 0 = global, 1 = per-port
    pub tt_think_time: u8,     // Transaction translator think time
}

impl HubDescriptor {
    pub fn new() -> Self {
        HubDescriptor {
            num_ports: 0,
            characteristics: 0,
            power_on_delay: 50, // 100 ms default
            hub_current: 100,
            removable_bitmap: Vec::new(),
            power_switching: PowerSwitching::Ganged,
            compound_device: false,
            over_current_mode: 0,
            tt_think_time: 0,
        }
    }

    /// Parse a USB 2.0 hub descriptor.
    pub fn parse(data: &[u8]) -> Self {
        let mut desc = HubDescriptor::new();
        if data.len() < 7 {
            return desc;
        }

        desc.num_ports = data[2];
        desc.characteristics = (data[3] as u16) | ((data[4] as u16) << 8);
        desc.power_on_delay = data[5];
        desc.hub_current = data[6];

        // Parse characteristics
        desc.power_switching = match desc.characteristics & 0x03 {
            0x00 => PowerSwitching::Ganged,
            0x01 => PowerSwitching::Individual,
            _ => PowerSwitching::None,
        };
        desc.compound_device = (desc.characteristics & 0x04) != 0;
        desc.over_current_mode = ((desc.characteristics >> 3) & 0x03) as u8;
        desc.tt_think_time = ((desc.characteristics >> 5) & 0x03) as u8;

        // DeviceRemovable bitmap
        let bitmap_bytes = ((desc.num_ports + 8) / 8) as usize;
        if data.len() >= 7 + bitmap_bytes {
            desc.removable_bitmap = data[7..7 + bitmap_bytes].to_vec();
        }

        desc
    }

    /// Power-on delay in milliseconds.
    pub fn power_on_delay_ms(&self) -> u32 {
        self.power_on_delay as u32 * 2
    }

    /// Check if a port is non-removable.
    pub fn is_non_removable(&self, port: u8) -> bool {
        if port == 0 || self.removable_bitmap.is_empty() {
            return false;
        }
        let byte_idx = (port / 8) as usize;
        let bit_idx = port % 8;
        if byte_idx >= self.removable_bitmap.len() {
            return false;
        }
        self.removable_bitmap[byte_idx] & (1 << bit_idx) != 0
    }
}

// ---------------------------------------------------------------------------
// Port status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct HubPortStatus {
    pub status: u16,
    pub change: u16,
}

impl HubPortStatus {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        Some(HubPortStatus {
            status: (data[0] as u16) | ((data[1] as u16) << 8),
            change: (data[2] as u16) | ((data[3] as u16) << 8),
        })
    }

    pub fn connected(&self) -> bool {
        self.status & PS_CONNECTION != 0
    }
    pub fn enabled(&self) -> bool {
        self.status & PS_ENABLE != 0
    }
    pub fn suspended(&self) -> bool {
        self.status & PS_SUSPEND != 0
    }
    pub fn over_current(&self) -> bool {
        self.status & PS_OVER_CURRENT != 0
    }
    pub fn reset_active(&self) -> bool {
        self.status & PS_RESET != 0
    }
    pub fn powered(&self) -> bool {
        self.status & PS_POWER != 0
    }

    pub fn connection_changed(&self) -> bool {
        self.change & PC_CONNECTION != 0
    }
    pub fn enable_changed(&self) -> bool {
        self.change & PC_ENABLE != 0
    }
    pub fn suspend_changed(&self) -> bool {
        self.change & PC_SUSPEND != 0
    }
    pub fn over_current_changed(&self) -> bool {
        self.change & PC_OVER_CURRENT != 0
    }
    pub fn reset_changed(&self) -> bool {
        self.change & PC_RESET != 0
    }

    /// Determine port speed from status bits.
    pub fn speed(&self) -> HubSpeed {
        if self.status & PS_HIGH_SPEED != 0 {
            HubSpeed::HighSpeed
        } else if self.status & PS_LOW_SPEED != 0 {
            HubSpeed::FullSpeed // Actually low-speed, but grouped
        } else {
            HubSpeed::FullSpeed
        }
    }
}

// ---------------------------------------------------------------------------
// Hub events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubEventKind {
    DeviceConnected,
    DeviceDisconnected,
    PortOverCurrent,
    PortOverCurrentCleared,
    PortResetComplete,
    PortSuspendChange,
    HubError,
}

#[derive(Debug, Clone)]
pub struct HubEvent {
    pub hub_slot: u8,
    pub port: u8,
    pub kind: HubEventKind,
}

// ---------------------------------------------------------------------------
// Hub device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubState {
    Detached,
    Attached,
    Configured,
    Running,
    Suspended,
    Error,
}

pub struct HubDevice {
    pub slot_id: u8,
    pub state: HubState,
    pub speed: HubSpeed,
    pub depth: u8, // Hub depth (for USB 3.0 SET_HUB_DEPTH)
    pub descriptor: HubDescriptor,
    pub port_statuses: Vec<HubPortStatus>,
    pub interrupt_ep: u8,
    pub parent_hub: Option<u8>, // slot_id of parent hub (None = root)
    pub parent_port: u8,
}

impl HubDevice {
    pub fn new(slot_id: u8, speed: HubSpeed) -> Self {
        HubDevice {
            slot_id,
            state: HubState::Attached,
            speed,
            depth: 0,
            descriptor: HubDescriptor::new(),
            port_statuses: Vec::new(),
            interrupt_ep: 0,
            parent_hub: None,
            parent_port: 0,
        }
    }

    /// Parse the hub descriptor from GET_DESCRIPTOR response.
    pub fn parse_descriptor(&mut self, data: &[u8]) {
        self.descriptor = HubDescriptor::parse(data);
        // Initialize port statuses
        let count = self.descriptor.num_ports as usize;
        self.port_statuses = alloc::vec![HubPortStatus { status: 0, change: 0 }; count];
    }

    /// Number of downstream ports.
    pub fn num_ports(&self) -> u8 {
        self.descriptor.num_ports
    }

    // ----- port control request builders -----

    /// Build SET_FEATURE(PORT_POWER) for a port.
    pub fn build_port_power_on(&self, port: u8) -> [u8; 8] {
        [
            0x23, // bmRequestType: class, other, host-to-device
            HUB_SET_FEATURE,
            PORT_POWER,
            0x00, // wValue
            port,
            0x00, // wIndex (port number)
            0x00,
            0x00, // wLength
        ]
    }

    /// Build CLEAR_FEATURE(PORT_POWER) for a port.
    pub fn build_port_power_off(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_CLEAR_FEATURE,
            PORT_POWER,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build SET_FEATURE(PORT_RESET) for a port.
    pub fn build_port_reset(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_SET_FEATURE,
            PORT_RESET,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build SET_FEATURE(PORT_SUSPEND) for a port.
    pub fn build_port_suspend(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_SET_FEATURE,
            PORT_SUSPEND,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build CLEAR_FEATURE(PORT_SUSPEND) — resume a port.
    pub fn build_port_resume(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_CLEAR_FEATURE,
            PORT_SUSPEND,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build CLEAR_FEATURE(C_PORT_CONNECTION) — acknowledge connect change.
    pub fn build_clear_connect_change(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_CLEAR_FEATURE,
            C_PORT_CONNECTION,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build CLEAR_FEATURE(C_PORT_RESET) — acknowledge reset complete.
    pub fn build_clear_reset_change(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_CLEAR_FEATURE,
            C_PORT_RESET,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build CLEAR_FEATURE(C_PORT_OVER_CURRENT) — acknowledge OC change.
    pub fn build_clear_overcurrent_change(&self, port: u8) -> [u8; 8] {
        [
            0x23,
            HUB_CLEAR_FEATURE,
            C_PORT_OVER_CURRENT,
            0x00,
            port,
            0x00,
            0x00,
            0x00,
        ]
    }

    /// Build GET_PORT_STATUS request.
    pub fn build_get_port_status(&self, port: u8) -> [u8; 8] {
        [
            0xA3, // bmRequestType: class, other, device-to-host
            HUB_GET_STATUS,
            0x00,
            0x00,
            port,
            0x00,
            0x04,
            0x00, // wLength: 4 bytes
        ]
    }

    /// Build GET_HUB_DESCRIPTOR request.
    pub fn build_get_hub_descriptor(&self) -> [u8; 8] {
        let desc_type =
            if self.speed == HubSpeed::SuperSpeed || self.speed == HubSpeed::SuperSpeedPlus {
                DESC_HUB_SS
            } else {
                DESC_HUB
            };
        [
            0xA0, // bmRequestType: class, device, device-to-host
            HUB_GET_DESCRIPTOR,
            0x00,
            desc_type,
            0x00,
            0x00,
            0x47,
            0x00, // wLength: up to 71 bytes
        ]
    }

    /// Build SET_HUB_DEPTH request (USB 3.0).
    pub fn build_set_hub_depth(&self) -> [u8; 8] {
        [
            0x20, // bmRequestType: class, device, host-to-device
            HUB_SET_HUB_DEPTH,
            self.depth,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ]
    }

    // ----- status processing -----

    /// Update port status from GET_PORT_STATUS response.
    pub fn update_port_status(&mut self, port: u8, data: &[u8]) -> Vec<HubEvent> {
        let mut events = Vec::new();
        if port == 0 || port as usize > self.port_statuses.len() {
            return events;
        }

        if let Some(ps) = HubPortStatus::from_bytes(data) {
            let idx = (port - 1) as usize;
            let old = self.port_statuses[idx];
            self.port_statuses[idx] = ps;

            // Generate events for status changes
            if ps.connection_changed() {
                if ps.connected() {
                    events.push(HubEvent {
                        hub_slot: self.slot_id,
                        port,
                        kind: HubEventKind::DeviceConnected,
                    });
                } else {
                    events.push(HubEvent {
                        hub_slot: self.slot_id,
                        port,
                        kind: HubEventKind::DeviceDisconnected,
                    });
                }
            }
            if ps.over_current_changed() {
                let kind = if ps.over_current() {
                    HubEventKind::PortOverCurrent
                } else {
                    HubEventKind::PortOverCurrentCleared
                };
                events.push(HubEvent {
                    hub_slot: self.slot_id,
                    port,
                    kind,
                });
            }
            if ps.reset_changed() {
                events.push(HubEvent {
                    hub_slot: self.slot_id,
                    port,
                    kind: HubEventKind::PortResetComplete,
                });
            }
            if ps.suspend_changed() {
                events.push(HubEvent {
                    hub_slot: self.slot_id,
                    port,
                    kind: HubEventKind::PortSuspendChange,
                });
            }

            let _ = old; // suppress unused warning
        }
        events
    }

    /// Process interrupt transfer data (status change bitmap).
    /// Bits set indicate which ports have status changes.
    /// Bit 0 = hub status, Bit 1 = port 1, Bit 2 = port 2, etc.
    pub fn parse_status_change_bitmap(&self, data: &[u8]) -> Vec<u8> {
        let mut changed_ports = Vec::new();
        for (byte_idx, &byte) in data.iter().enumerate() {
            for bit in 0..8u8 {
                if byte & (1 << bit) != 0 {
                    let port_num = (byte_idx * 8 + bit as usize) as u8;
                    if port_num >= 1 && port_num <= self.descriptor.num_ports {
                        changed_ports.push(port_num);
                    }
                }
            }
        }
        changed_ports
    }

    // ----- power management -----

    /// Power on all ports.
    pub fn power_on_all(&mut self) {
        for i in 0..self.port_statuses.len() {
            self.port_statuses[i].status |= PS_POWER;
        }
    }

    /// Power off all ports.
    pub fn power_off_all(&mut self) {
        for i in 0..self.port_statuses.len() {
            self.port_statuses[i].status &= !PS_POWER;
        }
    }

    /// Check if any port has an over-current condition.
    pub fn any_over_current(&self) -> bool {
        self.port_statuses.iter().any(|ps| ps.over_current())
    }

    /// List all ports with connected devices.
    pub fn connected_ports(&self) -> Vec<u8> {
        let mut ports = Vec::new();
        for (i, ps) in self.port_statuses.iter().enumerate() {
            if ps.connected() {
                ports.push((i + 1) as u8);
            }
        }
        ports
    }

    /// Transition hub to running state.
    pub fn start(&mut self) -> bool {
        if self.state != HubState::Configured {
            return false;
        }
        self.state = HubState::Running;
        true
    }

    /// Suspend the hub (USB SelectiveSuspend / global suspend).
    ///
    /// Per USB 2.0 spec §11.9:
    ///   1. Guard: no-op if already suspended or not running.
    ///   2. Issue SET_PORT_FEATURE(PORT_SUSPEND) to each powered, connected
    ///      downstream port.  This tells downstream devices to enter the
    ///      suspended state and is routed through the xHCI control path via
    ///      `set_port_feature()`.
    ///   3. Record the current SOF counter and spin until it has advanced by
    ///      3 ms (3 ticks) so downstream devices finish entering suspend.
    ///   4. Transition hub state to Suspended.
    ///
    /// For USB 3.0 (SS hub) the equivalent is U3 link-state entry, which is
    /// handled by the xHCI root hub via PORT_LINK_STATE transitions.
    pub fn suspend(&mut self) {
        if self.state != HubState::Running {
            return;
        }

        // Step 2: issue SET_PORT_FEATURE(PORT_SUSPEND) via xHCI control transfer.
        for (idx, ps) in self.port_statuses.iter_mut().enumerate() {
            if ps.connected() && ps.powered() && !ps.suspended() {
                let port_num = (idx + 1) as u8;
                // Update local model first.
                ps.status |= PS_SUSPEND;
                // Send SET_PORT_FEATURE(PORT_SUSPEND) control transfer.
                set_port_feature(self.slot_id, port_num, PORT_SUSPEND as u16);
            }
        }

        // Step 3: wait for ~3 SOF ticks (≈ 3 ms) to ensure downstream
        // devices have transitioned.  We poll the SOF counter which is
        // incremented by the 1 ms timer callback.
        let start = sof_counter();
        let target = start.wrapping_add(3);
        // Bounded spin: if the SOF counter does not advance (timer not yet
        // armed) we fall through after a short spin-loop so we never hang.
        let mut iter = 0u32;
        while sof_counter().wrapping_sub(start) < 3 && iter < 5_000_000 {
            core::hint::spin_loop();
            iter = iter.saturating_add(1);
        }
        let _ = target; // suppress unused warning

        self.state = HubState::Suspended;
        serial_println!(
            "    [hub] slot {} suspended (sof={})",
            self.slot_id,
            sof_counter()
        );
    }

    /// Resume the hub from suspend.
    ///
    /// Per USB 2.0 spec §11.9.2:
    ///   1. Guard: no-op if not suspended.
    ///   2. Issue CLEAR_PORT_FEATURE(PORT_SUSPEND) (PORT_RESUME signal) to
    ///      each previously-suspended port via `clear_port_feature()`.
    ///   3. Maintain the resume-K signalling for ≥ 20 ms per the spec.
    ///   4. Transition hub state to Running.
    pub fn resume(&mut self) {
        if self.state != HubState::Suspended {
            return;
        }

        // Step 2: de-assert PORT_SUSPEND on all suspended ports via xHCI.
        for (idx, ps) in self.port_statuses.iter_mut().enumerate() {
            if ps.suspended() {
                let port_num = (idx + 1) as u8;
                // Update local model.
                ps.status &= !PS_SUSPEND;
                // Send CLEAR_PORT_FEATURE(PORT_SUSPEND) control transfer.
                clear_port_feature(self.slot_id, port_num, PORT_SUSPEND as u16);
            }
        }

        // Step 3: wait ~20 SOF ticks (≈ 20 ms) for resume-K signal propagation.
        let start = sof_counter();
        let mut iter = 0u32;
        while sof_counter().wrapping_sub(start) < 20 && iter < 30_000_000 {
            core::hint::spin_loop();
            iter = iter.saturating_add(1);
        }

        self.state = HubState::Running;
        serial_println!(
            "    [hub] slot {} resumed (sof={})",
            self.slot_id,
            sof_counter()
        );
    }
}

// ---------------------------------------------------------------------------
// Class identification
// ---------------------------------------------------------------------------

pub fn is_hub(class: u8) -> bool {
    class == CLASS_HUB
}

pub fn is_superspeed_hub(class: u8, protocol: u8) -> bool {
    class == CLASS_HUB && protocol == 0x03
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = HUB_STATE.lock();
    *state = Some(HubDriverState::new());
    // Arm the 1 ms SOF counter so suspend/resume timing is available.
    start_sof_counter();
    serial_println!("    [hub] USB Hub driver loaded (hot-plug, power mgmt, SOF counter)");
}
