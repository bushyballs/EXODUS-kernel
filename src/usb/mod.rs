/// Hoags USB — Universal Serial Bus stack for Genesis
///
/// Architecture:
///   1. Host Controller Driver (xHCI for USB 3.x)
///   2. USB Core: device enumeration, descriptors, transfers
///   3. Class Drivers: HID, Mass Storage, Audio, Video, CDC, Printer, Hub
///
/// Inspired by: Linux USB subsystem (layered architecture),
/// USB specification (descriptor model). All code is original.
use crate::{serial_print, serial_println};
pub mod audio_class;
pub mod cdc;
pub mod core_usb;
pub mod gadget;
pub mod hid;
pub mod hid_gamepad;
pub mod hub;
pub mod mass_storage;
pub mod otg;
pub mod printer_class;
pub mod serial;
pub mod typec;
pub mod video_class;
pub mod xhci;

/// Enumeration result: which class driver was matched for a new device.
pub enum UsbDriverKind {
    Hid,
    MassStorage,
    CdcAcm,
    Audio,
    Video,
    Hub,
    Printer,
    Unknown,
}

/// Dispatch a newly enumerated device to the appropriate class driver.
/// `class`, `subclass`, and `protocol` come from either the device descriptor
/// (device-level class) or from each interface descriptor when device class
/// is 0x00 (per-interface classification).
pub fn handle_new_device(class: u8, subclass: u8, protocol: u8) -> UsbDriverKind {
    match class {
        // HID (keyboards, mice, gamepads)
        core_usb::CLASS_HID => {
            serial_println!(
                "  [usb] class driver: HID (subclass={:#04x}, protocol={:#04x})",
                subclass,
                protocol
            );
            UsbDriverKind::Hid
        }
        // Mass Storage (flash drives, external disks)
        core_usb::CLASS_MASS_STORAGE => {
            serial_println!(
                "  [usb] class driver: Mass Storage (subclass={:#04x}, protocol={:#04x})",
                subclass,
                protocol
            );
            UsbDriverKind::MassStorage
        }
        // CDC (virtual serial ports) — class 0x02 (control) or 0x0A (data)
        core_usb::CLASS_CDC | core_usb::CLASS_CDC_DATA => {
            serial_println!("  [usb] class driver: CDC/ACM (subclass={:#04x})", subclass);
            UsbDriverKind::CdcAcm
        }
        // Audio (headsets, DACs, microphones)
        core_usb::CLASS_AUDIO => {
            serial_println!("  [usb] class driver: Audio (subclass={:#04x})", subclass);
            UsbDriverKind::Audio
        }
        // Video (webcams)
        core_usb::CLASS_VIDEO => {
            serial_println!("  [usb] class driver: Video (subclass={:#04x})", subclass);
            UsbDriverKind::Video
        }
        // Hub
        core_usb::CLASS_HUB => {
            serial_println!("  [usb] class driver: Hub");
            UsbDriverKind::Hub
        }
        // Printer
        core_usb::CLASS_PRINTER => {
            serial_println!("  [usb] class driver: Printer (protocol={:#04x})", protocol);
            UsbDriverKind::Printer
        }
        // Composite / per-interface: caller must walk each InterfaceDescriptor
        core_usb::CLASS_PER_INTERFACE => {
            serial_println!("  [usb] composite device — per-interface class assignment");
            UsbDriverKind::Unknown
        }
        other => {
            serial_println!(
                "  [usb] unknown device class {:#04x}/{:#04x}/{:#04x}",
                other,
                subclass,
                protocol
            );
            UsbDriverKind::Unknown
        }
    }
}

pub fn init() {
    xhci::init();
    hid::init();
    hid_gamepad::init();
    mass_storage::init();
    audio_class::init();
    video_class::init();
    cdc::init();
    printer_class::init();
    hub::init();
    otg::init();
    gadget::init();
    typec::init();
    serial::init();
    serial_println!("  USB: xHCI, HID (kbd/mouse), HID gamepad, mass storage, audio, video, CDC, printer, hub, OTG, gadget, type-C, serial");
}
