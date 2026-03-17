pub mod acpi;
pub mod acpi_ec;
pub mod ahci;
pub mod ata;
pub mod battery;
pub mod block_io;
pub mod bluetooth;
pub mod bochs_vga;
pub mod bt_hci;
pub mod camera_driver;
pub mod can;
/// Clock framework (PLLs, dividers, gates, muxes)
pub mod clk;
/// CPU frequency scaling driver (per-CPU policy layer: Ondemand/Performance/Powersave/etc.)
pub mod cpufreq;
pub mod display_port;
pub mod dm;
pub mod dm_crypt;
pub mod dm_verity;
pub mod dma_engine;
pub mod drm;
pub mod e1000;
pub mod edac;
pub mod fbconsole;
pub mod framebuffer;
pub mod gpio;
pub mod gpu;
pub mod hdmi;
pub mod hwmon;
pub mod i2c;
pub mod input;
pub mod iommu;
pub mod ir;
/// Driver framework for Genesis
///
/// All hardware drivers implement the Driver trait and register themselves
/// during kernel boot. The driver framework handles:
///   - Device discovery (PCI bus scan, ACPI, device tree)
///   - Driver registration and matching
///   - Interrupt routing to drivers
///   - Power management
///
/// Inspired by: Linux driver model (bus/device/driver), Fuchsia's DDK,
/// Redox's scheme-based drivers. All code is original.
pub mod keyboard;
pub mod led;
pub mod leds;
/// MDIO/MII PHY management bus driver
pub mod mdio;
pub mod mouse;
pub mod npu;
pub mod nvme;
/// Non-Volatile Memory (EEPROM/OTP/EFUSE) framework
pub mod nvmem;
pub mod pci;
pub mod pci_hotplug;
pub mod pci_msi;
pub mod pcie_aer;
pub mod pcie_hotplug;
pub mod pinctrl;
/// Platform device/driver bus: ACPI/device-tree style non-PCI devices.
pub mod platform;
/// Power domain manager (PSCI-inspired hardware-block power gating)
pub mod power_domain;
pub mod power_supply;
/// PTP/IEEE 1588 hardware timestamping driver.
pub mod ptp;
pub mod pty;
pub mod pwm;
/// Voltage/current regulator framework (LDOs, buck/boost converters)
pub mod regulator;
pub mod rfkill;
pub mod rtc;
/// Generic SCSI mid-layer: host adapter registry, device table, CDB dispatch.
pub mod scsi;
pub mod sdcard;
pub mod sensor_hub;
/// Serial device bus: UART-attached protocol drivers (GPS, BT HCI UART, modems)
pub mod serdev;
pub mod sound;
pub mod spi;
pub mod storage;
pub mod thermal;
pub mod thermal_zones;
pub mod thunderbolt;
pub mod touchscreen;
pub mod tpu;
pub mod tty;
/// 8250/16550 UART serial driver (COM1-COM4)
pub mod uart8250;
/// USB Audio Class 2.0 gadget function (48kHz stereo PCM)
pub mod usb_audio;
pub mod usb_cdc_acm;
pub mod usb_cdc_eth;
/// USB gadget framework (device-mode endpoint management)
pub mod usb_gadget;
pub mod usb_mass_storage;
pub mod virtio;
pub mod virtio_9p;
pub mod virtio_balloon;
pub mod virtio_blk;
pub mod virtio_console;
pub mod virtio_input;
pub mod virtio_net;
pub mod virtio_rng;
pub mod watchdog;
pub mod wifi;
pub mod wifi_driver;

use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

/// Driver status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverStatus {
    Uninitialized,
    Initializing,
    Running,
    Error,
    Suspended,
}

/// Device type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Keyboard,
    Mouse,
    Display,
    Storage,
    Network,
    Audio,
    Usb,
    Serial,
    Timer,
    Other,
}

/// A registered driver
pub struct DriverEntry {
    pub name: String,
    pub device_type: DeviceType,
    pub status: DriverStatus,
}

/// Global driver registry
static DRIVERS: Mutex<Vec<DriverEntry>> = Mutex::new(Vec::new());

/// Register a driver
pub fn register(name: &str, device_type: DeviceType) {
    DRIVERS.lock().push(DriverEntry {
        name: String::from(name),
        device_type,
        status: DriverStatus::Running,
    });
}

/// Initialize the driver subsystem
pub fn init() {
    // PS/2 keyboard (already partially handled by interrupts module)
    keyboard::init();

    // Framebuffer (VGA text mode is already running, this adds graphics mode)
    framebuffer::init();

    // PCI bus scan
    pci::init();

    // PCI hot-plug slot scan (must run after pci::init so PCI_DEVICES is populated)
    pci_hotplug::scan_hotplug_slots();

    // ATA disk driver
    ata::init();
    let ata_count = ata::drive_count();
    if ata_count > 0 {
        register("ata-disk", DeviceType::Storage);
        serial_println!("  Drivers: {} ATA drive(s) detected", ata_count);
    }

    // Bochs VBE graphics (switches to 1024x768x32)
    if bochs_vga::init() {
        register("bochs-vga", DeviceType::Display);
    }

    // Intel e1000 Ethernet
    if e1000::init() {
        register("e1000-nic", DeviceType::Network);
    }

    // PS/2 mouse
    mouse::init();

    // AHCI/SATA
    ahci::init();

    // Virtio block device
    virtio::init();

    // VirtIO network device
    virtio_net::init();

    // VirtIO balloon memory driver (host-guest memory pressure co-operation)
    virtio_balloon::init();

    // Hardware monitor framework (CPU temp, freq, memory sensors)
    hwmon::init();

    // Framebuffer text console
    fbconsole::init();

    // HD Audio
    sound::init();

    // TTY subsystem
    tty::init();

    // PTY (pseudo-terminal) subsystem
    pty::init();

    // NVMe SSD driver
    nvme::init();

    // SCSI mid-layer (host adapter registry, device table, CDB dispatch)
    scsi::init();

    // Unified storage manager (AHCI/NVMe/Virtio device registry)
    storage::init();

    // Block I/O scheduler
    block_io::init();

    // Device mapper
    dm::init();

    // dm-verity integrity checking
    dm_verity::init();

    // dm-crypt transparent encryption
    dm_crypt::init();

    // Input subsystem
    input::init();

    // Touchscreen (I2C touch controller)
    touchscreen::init();

    // Camera sensor (I2C, MIPI CSI-2)
    camera_driver::init();

    // Battery / power supply (EC via ports 0x62/0x66)
    battery::init();

    // GPIO subsystem (registers simulated chip; real chips discovered via ACPI/PCI)
    gpio::init();

    // SPI bus master framework (simulated loopback controller)
    spi::init();

    // I2C bus master framework (simulated loopback adapter)
    i2c::init();

    // Pin controller (pad mux, pull, drive-strength, slew rate, Schmitt trigger)
    pinctrl::init();

    // Intel VT-d IOMMU (module load; parse_dmar_table + iommu_init called from ACPI init)
    iommu::init();

    // DRM/KMS modesetting subsystem (connector detect, CRTC + plane init)
    drm::init();
    register("drm-kms", DeviceType::Display);

    // LED framework (virtual + GPIO/MMIO LEDs with trigger support)
    leds::init();

    // Thermal zone management (no-heap trip-point framework)
    thermal_zones::init();

    // Power supply class driver (BAT0 + AC, backed by battery EC driver)
    power_supply::init();

    // Watchdog timer framework (software + hardware multi-watchdog)
    watchdog::init();

    // RFKILL radio kill-switch framework
    rfkill::init();

    // 802.11 WiFi driver
    wifi::init();

    // CAN bus controller driver (loopback controller registered at CAN_IO_BASE)
    can::init();

    // EDAC memory error detection/correction framework (SECDED ECC, 256 MB csrow)
    edac::init();

    // Bluetooth HCI framework (adapter registry, connection table, scan results)
    bluetooth::init();

    // USB CDC-ECM Ethernet gadget driver
    usb_cdc_eth::init();

    // ACPI Embedded Controller (battery, thermal, fan, lid, power button)
    acpi_ec::init();

    // VirtIO-9P Plan 9 filesystem driver (host directory sharing via QEMU)
    virtio_9p::init();

    // PCIe Advanced Error Reporting (hardware error polling and logging)
    pcie_aer::init();

    // PCIe hot-plug controller (simulated slot manager with state machine)
    pcie_hotplug::init();

    // USB Mass Storage (BOT protocol — simulated 1 GiB drive)
    usb_mass_storage::init();

    // USB CDC-ACM virtual serial port (port 0 = debug console)
    usb_cdc_acm::init();

    // VirtIO RNG entropy source (falls back to TSC-seeded LFSR if device absent)
    virtio_rng::init();

    // VirtIO serial console (opens port 0 as primary console)
    virtio_console::init();

    // VirtIO input device (keyboard, mouse, tablet events)
    virtio_input::init();

    // Platform bus: register standard QEMU x86 platform devices (i8042, rtc, pit, serial8250)
    platform::init();

    // 8250/16550 UART serial driver (COM1-COM4)
    uart8250::init();

    // MDIO/MII PHY management bus driver
    mdio::init();

    // PTP/IEEE 1588 hardware timestamping (nanosecond-accurate clock sync)
    ptp::init();

    // CPU frequency scaling policy layer (per-CPU Ondemand/Performance/Powersave governors)
    cpufreq::init();

    // Power domain manager (PSCI-inspired hardware-block power gating)
    power_domain::init();

    // Voltage/current regulator framework (vdd-core, vdd-io, vdd-usb, vdd-pll)
    regulator::init();

    // Clock framework (osc24m, pll-cpu, pll-ddr, ahb, apb, uart-clk, usb-clk)
    clk::init();

    // USB gadget framework (device-mode, composite gadgets)
    usb_gadget::init();

    // USB Audio Class 2.0 gadget (48kHz stereo S16LE)
    usb_audio::init();

    // NVMEM framework (EEPROM/OTP/EFUSE cell storage)
    nvmem::init();
    serdev::init();

    serial_println!("  Drivers: {} drivers registered", DRIVERS.lock().len());
}

/// Periodic driver tick — wire this into the system timer interrupt handler.
///
/// Should be called on every timer IRQ (typically every 1 ms or at whatever
/// granularity the PIT/HPET is programmed for).  Each subsystem receives
/// the current uptime in milliseconds and decides internally whether its
/// own poll interval has elapsed.
///
/// Subsystems driven here:
///   - `watchdog::watchdog_tick`      — keepalive deadline evaluation, expiry logging
///   - `leds::led_tick`               — blink pattern, heartbeat, activity auto-off
///   - `thermal_zones::thermal_tick`  — trip point evaluation, CPU temp poll
///   - `power_supply::ps_tick`        — battery state sync, critical-level logging
///   - `pcie_hotplug::hp_tick`        — PCIe hot-plug slot state machine (1 s poll)
pub fn drivers_tick(current_ms: u64) {
    watchdog::watchdog_tick(current_ms);
    leds::led_tick(current_ms);
    thermal_zones::thermal_tick(current_ms);
    power_supply::ps_tick();
    pcie_hotplug::hp_tick(current_ms);
}

/// List all registered drivers
pub fn list() -> Vec<(String, DeviceType, DriverStatus)> {
    DRIVERS
        .lock()
        .iter()
        .map(|d| (d.name.clone(), d.device_type, d.status))
        .collect()
}
