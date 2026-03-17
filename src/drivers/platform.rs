/// Platform device/driver framework for Genesis
///
/// Provides a simple bus for platform devices — non-PCI, non-USB devices
/// that are discovered via ACPI or the device tree (e.g. i8042, RTC, PIT,
/// UART).  Mirrors the Linux platform_device / platform_driver model while
/// remaining entirely allocation-free.
///
/// Rules strictly observed:
///   - No heap: no Vec, Box, String, format!, alloc::*  — fixed-size arrays
///   - No floats: no f32/f64 literals or casts
///   - No panics: no unwrap(), expect(), panic!()
///   - Counters:  saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile
///   - Structs in static Mutex are Copy + have const fn empty()
///   - No division without guarding divisor != 0
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum platform devices that can be registered simultaneously
pub const MAX_PLATFORM_DEVICES: usize = 32;

/// Maximum platform drivers that can be registered simultaneously
pub const MAX_PLATFORM_DRIVERS: usize = 16;

/// Maximum length (bytes) of a device or driver name
pub const PLATFORM_NAME_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Function pointer types
// ---------------------------------------------------------------------------

/// Called when a driver is matched to a device.
/// `dev_id` — the id of the device being probed.
/// Returns `true` if the driver successfully claimed the device.
pub type PlatformProbeFn = fn(dev_id: u32) -> bool;

/// Called when a driver is unregistered or a device is removed.
/// `dev_id` — the id of the device being released.
pub type PlatformRemoveFn = fn(dev_id: u32);

// ---------------------------------------------------------------------------
// Platform device
// ---------------------------------------------------------------------------

/// A single platform device record
#[derive(Copy, Clone)]
pub struct PlatformDevice {
    /// Unique numeric identifier assigned at registration
    pub id: u32,
    /// NUL-padded ASCII name used for driver matching
    pub name: [u8; PLATFORM_NAME_LEN],
    /// Number of valid bytes in `name`
    pub name_len: u8,
    /// Primary resource base address (MMIO base or I/O port base)
    pub res_base: u64,
    /// Size of the primary resource region in bytes
    pub res_size: u32,
    /// Interrupt number associated with this device (0 = none)
    pub irq: u32,
    /// ID of the matched driver (0 = unmatched)
    pub driver_id: u32,
    /// True after `probe()` returned `true`
    pub probed: bool,
    /// True when this table slot is occupied
    pub active: bool,
}

impl PlatformDevice {
    /// Return an empty, inactive slot suitable for static initialisation.
    pub const fn empty() -> Self {
        PlatformDevice {
            id: 0,
            name: [0u8; PLATFORM_NAME_LEN],
            name_len: 0,
            res_base: 0,
            res_size: 0,
            irq: 0,
            driver_id: 0,
            probed: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Platform driver
// ---------------------------------------------------------------------------

/// A single platform driver record
#[derive(Copy, Clone)]
pub struct PlatformDriver {
    /// Unique numeric identifier assigned at registration
    pub id: u32,
    /// NUL-padded ASCII name — must match the device name to bind
    pub name: [u8; PLATFORM_NAME_LEN],
    /// Number of valid bytes in `name`
    pub name_len: u8,
    /// Probe callback (may be None for probe-less drivers)
    pub probe: Option<PlatformProbeFn>,
    /// Remove callback (may be None)
    pub remove: Option<PlatformRemoveFn>,
    /// True when this table slot is occupied
    pub active: bool,
}

impl PlatformDriver {
    /// Return an empty, inactive slot suitable for static initialisation.
    pub const fn empty() -> Self {
        PlatformDriver {
            id: 0,
            name: [0u8; PLATFORM_NAME_LEN],
            name_len: 0,
            probe: None,
            remove: None,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state — fixed-size arrays, no heap
// ---------------------------------------------------------------------------

static PLATFORM_DEVICES: Mutex<[PlatformDevice; MAX_PLATFORM_DEVICES]> =
    Mutex::new([PlatformDevice::empty(); MAX_PLATFORM_DEVICES]);

static PLATFORM_DRIVERS: Mutex<[PlatformDriver; MAX_PLATFORM_DRIVERS]> =
    Mutex::new([PlatformDriver::empty(); MAX_PLATFORM_DRIVERS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to `PLATFORM_NAME_LEN` bytes from `src` into a name array.
/// Returns the number of bytes copied.
fn copy_name(dst: &mut [u8; PLATFORM_NAME_LEN], src: &[u8]) -> u8 {
    let n = if src.len() < PLATFORM_NAME_LEN {
        src.len()
    } else {
        PLATFORM_NAME_LEN
    };
    let mut i = 0usize;
    while i < n {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    n as u8
}

/// Compare `a[..a_len]` with `b[..b_len]`.
/// Matches when the shared prefix (min of the two lengths) is identical.
fn names_match(
    a: &[u8; PLATFORM_NAME_LEN],
    a_len: u8,
    b: &[u8; PLATFORM_NAME_LEN],
    b_len: u8,
) -> bool {
    let cmp_len = if a_len < b_len { a_len } else { b_len } as usize;
    if cmp_len == 0 {
        return false;
    }
    let mut i = 0usize;
    while i < cmp_len {
        if a[i] != b[i] {
            return false;
        }
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a platform device.
///
/// `name`  — ASCII device name used for driver matching.
/// `base`  — primary resource base (MMIO or I/O port).
/// `size`  — resource region size in bytes.
/// `irq`   — interrupt number (0 = none).
///
/// Returns `Some(dev_id)` on success, `None` if the device table is full or
/// `name` is empty.
pub fn platform_device_register(name: &[u8], base: u64, size: u32, irq: u32) -> Option<u32> {
    if name.is_empty() {
        return None;
    }
    let mut devs = PLATFORM_DEVICES.lock();
    // Find a free slot and derive the id from its index
    for i in 0..MAX_PLATFORM_DEVICES {
        if !devs[i].active {
            let id = i as u32;
            let mut dev = PlatformDevice::empty();
            dev.id = id;
            dev.name_len = copy_name(&mut dev.name, name);
            dev.res_base = base;
            dev.res_size = size;
            dev.irq = irq;
            dev.active = true;
            devs[i] = dev;
            return Some(id);
        }
    }
    None
}

/// Unregister a platform device.
///
/// If the device was probed, the bound driver's `remove()` callback is
/// invoked before the slot is cleared.
///
/// Returns `true` on success, `false` if `dev_id` is out of range or not
/// registered.
pub fn platform_device_unregister(dev_id: u32) -> bool {
    if dev_id as usize >= MAX_PLATFORM_DEVICES {
        return false;
    }
    // Capture driver_id and probed status before dropping the device lock
    let (driver_id, probed) = {
        let devs = PLATFORM_DEVICES.lock();
        let slot = &devs[dev_id as usize];
        if !slot.active {
            return false;
        }
        (slot.driver_id, slot.probed)
    };

    // Call remove() if a driver was bound
    if probed && driver_id != 0 {
        let remove_fn: Option<PlatformRemoveFn> = {
            let drvs = PLATFORM_DRIVERS.lock();
            let mut found: Option<PlatformRemoveFn> = None;
            for i in 0..MAX_PLATFORM_DRIVERS {
                if drvs[i].active && drvs[i].id == driver_id {
                    found = drvs[i].remove;
                    break;
                }
            }
            found
        };
        if let Some(f) = remove_fn {
            f(dev_id);
        }
    }

    let mut devs = PLATFORM_DEVICES.lock();
    devs[dev_id as usize] = PlatformDevice::empty();
    true
}

/// Register a platform driver.
///
/// `name`   — ASCII driver name; matched against device names.
/// `probe`  — optional probe callback.
/// `remove` — optional remove callback.
///
/// Returns `Some(drv_id)` on success, `None` if the driver table is full or
/// `name` is empty.
pub fn platform_driver_register(
    name: &[u8],
    probe: Option<PlatformProbeFn>,
    remove: Option<PlatformRemoveFn>,
) -> Option<u32> {
    if name.is_empty() {
        return None;
    }
    let mut drvs = PLATFORM_DRIVERS.lock();
    for i in 0..MAX_PLATFORM_DRIVERS {
        if !drvs[i].active {
            let id = i as u32;
            let mut drv = PlatformDriver::empty();
            drv.id = id;
            drv.name_len = copy_name(&mut drv.name, name);
            drv.probe = probe;
            drv.remove = remove;
            drv.active = true;
            drvs[i] = drv;
            return Some(id);
        }
    }
    None
}

/// Unregister a platform driver.
///
/// All devices that were probed by this driver have their `remove()` callback
/// invoked and their `probed` / `driver_id` fields cleared.
///
/// Returns `true` on success, `false` if `drv_id` is out of range or not
/// registered.
pub fn platform_driver_unregister(drv_id: u32) -> bool {
    if drv_id as usize >= MAX_PLATFORM_DRIVERS {
        return false;
    }
    // Capture remove fn before we clear the driver slot
    let remove_fn: Option<PlatformRemoveFn> = {
        let drvs = PLATFORM_DRIVERS.lock();
        let slot = &drvs[drv_id as usize];
        if !slot.active {
            return false;
        }
        slot.remove
    };

    // Call remove() on every device bound to this driver
    {
        let mut devs = PLATFORM_DEVICES.lock();
        for i in 0..MAX_PLATFORM_DEVICES {
            if devs[i].active && devs[i].probed && devs[i].driver_id == drv_id {
                if let Some(f) = remove_fn {
                    f(devs[i].id);
                }
                devs[i].probed = false;
                devs[i].driver_id = 0;
            }
        }
    }

    let mut drvs = PLATFORM_DRIVERS.lock();
    drvs[drv_id as usize] = PlatformDriver::empty();
    true
}

/// Match unprobed devices to registered drivers and call `probe()`.
///
/// For each device that has not yet been probed, search the driver table for
/// a driver whose name matches (byte-comparison of the shared prefix).  On a
/// match, call `probe(dev_id)`; if it returns `true`, mark the device as
/// probed and record the `driver_id`.
pub fn platform_match_and_probe() {
    // Collect (dev_id, name, name_len) for unprobed devices to avoid
    // holding both locks simultaneously.
    let mut unprobed: [(u32, [u8; PLATFORM_NAME_LEN], u8); MAX_PLATFORM_DEVICES] =
        [(0, [0u8; PLATFORM_NAME_LEN], 0); MAX_PLATFORM_DEVICES];
    let mut unprobed_count: usize = 0;

    {
        let devs = PLATFORM_DEVICES.lock();
        for i in 0..MAX_PLATFORM_DEVICES {
            if devs[i].active && !devs[i].probed {
                if unprobed_count < MAX_PLATFORM_DEVICES {
                    unprobed[unprobed_count] = (devs[i].id, devs[i].name, devs[i].name_len);
                    unprobed_count = unprobed_count.saturating_add(1);
                }
            }
        }
    }

    for ui in 0..unprobed_count {
        let (dev_id, dev_name, dev_name_len) = unprobed[ui];

        // Find a matching driver
        let mut matched_drv_id: Option<u32> = None;
        let mut probe_fn: Option<PlatformProbeFn> = None;
        {
            let drvs = PLATFORM_DRIVERS.lock();
            for j in 0..MAX_PLATFORM_DRIVERS {
                if drvs[j].active
                    && names_match(&dev_name, dev_name_len, &drvs[j].name, drvs[j].name_len)
                {
                    matched_drv_id = Some(drvs[j].id);
                    probe_fn = drvs[j].probe;
                    break;
                }
            }
        }

        if let Some(drv_id) = matched_drv_id {
            // Call probe (may be None — treat that as success)
            let ok = match probe_fn {
                Some(f) => f(dev_id),
                None => true,
            };
            if ok {
                let mut devs = PLATFORM_DEVICES.lock();
                if (dev_id as usize) < MAX_PLATFORM_DEVICES {
                    devs[dev_id as usize].probed = true;
                    devs[dev_id as usize].driver_id = drv_id;
                }
            }
        }
    }
}

/// Return the primary resource `(base, size)` for a registered device, or
/// `None` if `dev_id` is out of range or not registered.
pub fn platform_device_get_resource(dev_id: u32) -> Option<(u64, u32)> {
    if dev_id as usize >= MAX_PLATFORM_DEVICES {
        return None;
    }
    let devs = PLATFORM_DEVICES.lock();
    let slot = &devs[dev_id as usize];
    if !slot.active {
        None
    } else {
        Some((slot.res_base, slot.res_size))
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the platform bus and register the standard QEMU x86 devices:
///
/// | Name        | Base   | Size | IRQ | Notes                       |
/// |-------------|--------|------|-----|-----------------------------|
/// | `i8042`     | 0x60   | 0x10 |   1 | PS/2 keyboard controller    |
/// | `rtc`       | 0x70   | 0x02 |   8 | CMOS real-time clock        |
/// | `pit`       | 0x40   | 0x04 |   0 | Programmable interval timer |
/// | `serial8250`| 0x3F8  | 0x08 |   4 | 16550 UART (COM1)           |
pub fn init() {
    let mut count: u32 = 0;

    if platform_device_register(b"i8042", 0x60, 0x10, 1).is_some() {
        count = count.saturating_add(1);
    }
    if platform_device_register(b"rtc", 0x70, 0x02, 8).is_some() {
        count = count.saturating_add(1);
    }
    if platform_device_register(b"pit", 0x40, 0x04, 0).is_some() {
        count = count.saturating_add(1);
    }
    if platform_device_register(b"serial8250", 0x3F8, 0x08, 4).is_some() {
        count = count.saturating_add(1);
    }

    serial_println!(
        "[platform] platform bus initialized, {} devices registered",
        count,
    );
}
