use crate::sync::Mutex;
/// GPIO subsystem for Genesis — no-heap, fixed-size static arrays
///
/// Manages GPIO pins through a chip-abstraction layer supporting:
///   - Multiple GPIO controllers (chips) up to MAX_GPIO_CHIPS
///   - Up to MAX_GPIO_PINS global GPIO numbers
///   - Pin request/release ownership model
///   - Input/output direction control
///   - Active-low logic inversion
///   - IRQ trigger type configuration with registered handlers
///   - Simulated GPIO chip for testing/QEMU
///   - Intel ICH/PCH MMIO GPIO (GP_IO_SEL / GP_LVL registers)
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - MMIO via read_volatile / write_volatile only
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of GPIO controllers (chips) supported
const MAX_GPIO_CHIPS: usize = 8;

/// Maximum number of global GPIO pin entries
const MAX_GPIO_PINS: usize = 128;

/// ICH/PCH GPIO register offsets from chip MMIO base
const ICH_GP_IO_SEL: u64 = 0x04; // bit=0 → output, bit=1 → input
const ICH_GP_LVL: u64 = 0x0C; // bit reflects pin level

// Stub IRQ base offset: gpio_to_irq returns gpio_num + GPIO_IRQ_BASE
const GPIO_IRQ_BASE: u32 = 100;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Logical direction of a GPIO pin
#[derive(Copy, Clone, PartialEq)]
pub enum GpioDirection {
    Input,
    Output,
}

/// Interrupt trigger mode for a GPIO line
#[derive(Copy, Clone, PartialEq)]
pub enum GpioIrqTrigger {
    None,
    RisingEdge,
    FallingEdge,
    BothEdges,
    HighLevel,
    LowLevel,
}

/// Hardware type of a GPIO controller
#[derive(Copy, Clone, PartialEq)]
pub enum GpioChipType {
    /// Generic / unknown controller
    Generic,
    /// Intel 8255-compatible parallel port GPIO
    Intel8255,
    /// Intel PCH southbridge GPIO
    Pch,
    /// GPIO exposed through ACPI methods
    Acpi,
    /// In-kernel simulated GPIO (no real hardware)
    Simulated,
}

// ---------------------------------------------------------------------------
// GpioPin
// ---------------------------------------------------------------------------

/// State for a single GPIO pin slot
#[derive(Copy, Clone)]
pub struct GpioPin {
    /// Global GPIO number (unique across all chips)
    pub gpio_num: u32,
    /// Which chip manages this pin
    pub chip_id: u8,
    /// Pin index within the chip (0-based)
    pub chip_offset: u8,
    /// Current direction
    pub direction: GpioDirection,
    /// Stored logical value (used for Simulated chips; MMIO chips read hw)
    pub value: bool,
    /// Configured IRQ trigger
    pub irq_trigger: GpioIrqTrigger,
    /// Optional IRQ handler — receives gpio_num as argument
    pub irq_handler: Option<fn(u32)>,
    /// Human-readable label (null-padded)
    pub label: [u8; 16],
    /// When true the logical level is inverted from the electrical level
    pub active_low: bool,
    /// True when a driver has claimed this pin via gpio_request()
    pub requested: bool,
    /// True when this slot is populated (gpio_num is valid)
    pub active: bool,
}

impl GpioPin {
    /// Return an empty (zeroed) pin slot
    pub const fn empty() -> Self {
        GpioPin {
            gpio_num: 0,
            chip_id: 0,
            chip_offset: 0,
            direction: GpioDirection::Input,
            value: false,
            irq_trigger: GpioIrqTrigger::None,
            irq_handler: None,
            label: [0u8; 16],
            active_low: false,
            requested: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// GpioChip
// ---------------------------------------------------------------------------

/// A GPIO controller (chip) registration entry
#[derive(Copy, Clone)]
pub struct GpioChip {
    /// Unique chip identifier (index into GPIO_CHIPS)
    pub chip_id: u8,
    /// First global GPIO number managed by this chip
    pub base: u32,
    /// Number of GPIO lines this chip exposes
    pub ngpio: u32,
    /// Human-readable label (null-padded)
    pub label: [u8; 16],
    /// MMIO register base address (0 if the chip uses port I/O or is simulated)
    pub mmio_base: u64,
    /// Hardware type
    pub chip_type: GpioChipType,
    /// True when this slot is populated
    pub active: bool,
}

impl GpioChip {
    /// Return an empty (zeroed) chip slot
    pub const fn empty() -> Self {
        GpioChip {
            chip_id: 0,
            base: 0,
            ngpio: 0,
            label: [0u8; 16],
            mmio_base: 0,
            chip_type: GpioChipType::Generic,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GPIO_CHIPS: Mutex<[GpioChip; MAX_GPIO_CHIPS]> =
    Mutex::new([GpioChip::empty(); MAX_GPIO_CHIPS]);

static GPIO_PINS: Mutex<[GpioPin; MAX_GPIO_PINS]> = Mutex::new([GpioPin::empty(); MAX_GPIO_PINS]);

// ---------------------------------------------------------------------------
// MMIO helpers (ICH/PCH specific)
// ---------------------------------------------------------------------------

/// Read a 32-bit MMIO register at `addr` using volatile semantics
#[inline]
fn mmio_read32(addr: u64) -> u32 {
    // Safety: bare-metal kernel; caller is responsible for valid MMIO address
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Write a 32-bit MMIO register at `addr` using volatile semantics
#[inline]
fn mmio_write32(addr: u64, val: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, val) }
}

// ---------------------------------------------------------------------------
// ICH/PCH GPIO operations
// ---------------------------------------------------------------------------

/// Read the current input level of an ICH GPIO offset from its chip
fn ich_gpio_get(chip: &GpioChip, offset: u8) -> bool {
    let lvl = mmio_read32(chip.mmio_base.saturating_add(ICH_GP_LVL));
    (lvl >> (offset as u32)) & 1 != 0
}

/// Set the output level of an ICH GPIO offset (read-modify-write GP_LVL)
fn ich_gpio_set(chip: &GpioChip, offset: u8, val: bool) {
    let addr = chip.mmio_base.saturating_add(ICH_GP_LVL);
    let mut lvl = mmio_read32(addr);
    if val {
        lvl |= 1u32 << (offset as u32);
    } else {
        lvl &= !(1u32 << (offset as u32));
    }
    mmio_write32(addr, lvl);
}

/// Configure direction for an ICH GPIO offset (read-modify-write GP_IO_SEL)
/// output=true → clear bit (0 = output); output=false → set bit (1 = input)
fn ich_gpio_direction(chip: &GpioChip, offset: u8, output: bool) {
    let addr = chip.mmio_base.saturating_add(ICH_GP_IO_SEL);
    let mut sel = mmio_read32(addr);
    if output {
        sel &= !(1u32 << (offset as u32)); // 0 = output
    } else {
        sel |= 1u32 << (offset as u32); // 1 = input
    }
    mmio_write32(addr, sel);
}

// ---------------------------------------------------------------------------
// Chip registry
// ---------------------------------------------------------------------------

/// Register a GPIO chip (controller) with the subsystem.
///
/// Returns `true` on success, `false` if the chip table is full or
/// a chip with the same chip_id is already registered.
pub fn gpio_chip_register(chip: GpioChip) -> bool {
    let mut chips = GPIO_CHIPS.lock();
    // Check for duplicate chip_id
    for slot in chips.iter() {
        if slot.active && slot.chip_id == chip.chip_id {
            return false;
        }
    }
    // Find free slot
    for slot in chips.iter_mut() {
        if !slot.active {
            *slot = chip;
            slot.active = true;
            return true;
        }
    }
    false // table full
}

/// Find the chip that owns `gpio_num` (i.e., base <= gpio_num < base + ngpio).
///
/// Returns a copy of the matching `GpioChip`, or `None`.
pub fn gpio_chip_find(gpio_num: u32) -> Option<GpioChip> {
    let chips = GPIO_CHIPS.lock();
    for slot in chips.iter() {
        if !slot.active {
            continue;
        }
        // Check gpio_num is in [base, base + ngpio)
        if gpio_num >= slot.base {
            let end = slot.base.saturating_add(slot.ngpio);
            if gpio_num < end {
                return Some(*slot);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Pin table helpers (internal)
// ---------------------------------------------------------------------------

/// Find the index of a pin entry by gpio_num; returns None if not present
fn pin_find_index(pins: &[GpioPin; MAX_GPIO_PINS], gpio_num: u32) -> Option<usize> {
    for (i, p) in pins.iter().enumerate() {
        if p.active && p.gpio_num == gpio_num {
            return Some(i);
        }
    }
    None
}

/// Find a free index in the pin table
fn pin_free_index(pins: &[GpioPin; MAX_GPIO_PINS]) -> Option<usize> {
    for (i, p) in pins.iter().enumerate() {
        if !p.active {
            return Some(i);
        }
    }
    None
}

/// Copy up to `src.len()` bytes from `src` into `dst`, null-padding the rest
fn copy_label(dst: &mut [u8; 16], src: &[u8]) {
    let len = if src.len() < 16 { src.len() } else { 16 };
    let mut i = 0;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 16 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Public GPIO API
// ---------------------------------------------------------------------------

/// Claim ownership of a GPIO pin.
///
/// Creates a pin entry if one does not already exist.
/// Returns `false` if the pin is already requested by another driver, or
/// if the chip owning this GPIO cannot be found, or if the pin table is full.
pub fn gpio_request(gpio_num: u32, label: &[u8]) -> bool {
    // Verify a chip owns this GPIO
    if gpio_chip_find(gpio_num).is_none() {
        return false;
    }
    let mut pins = GPIO_PINS.lock();

    // Check existing entry
    if let Some(idx) = pin_find_index(&pins, gpio_num) {
        if pins[idx].requested {
            return false; // already owned
        }
        pins[idx].requested = true;
        copy_label(&mut pins[idx].label, label);
        return true;
    }

    // Allocate new pin entry
    let chip = {
        // We need chip info — drop pins lock, acquire chips lock
        drop(pins);
        let found = gpio_chip_find(gpio_num);
        pins = GPIO_PINS.lock();
        found
    };
    let chip = match chip {
        Some(c) => c,
        None => return false,
    };

    let idx = match pin_free_index(&pins) {
        Some(i) => i,
        None => return false, // pin table full
    };

    let offset = match gpio_num.checked_sub(chip.base) {
        Some(o) if o <= 0xFF => o as u8,
        _ => return false,
    };

    let mut entry = GpioPin::empty();
    entry.gpio_num = gpio_num;
    entry.chip_id = chip.chip_id;
    entry.chip_offset = offset;
    entry.requested = true;
    entry.active = true;
    copy_label(&mut entry.label, label);

    pins[idx] = entry;
    true
}

/// Release ownership of a GPIO pin.
///
/// The pin entry remains in the table but `requested` is cleared.
pub fn gpio_free(gpio_num: u32) {
    let mut pins = GPIO_PINS.lock();
    if let Some(idx) = pin_find_index(&pins, gpio_num) {
        pins[idx].requested = false;
        pins[idx].irq_handler = None;
        pins[idx].irq_trigger = GpioIrqTrigger::None;
    }
}

/// Set a GPIO pin to input direction.
///
/// Returns `false` if the pin is not requested or no chip is found.
pub fn gpio_direction_input(gpio_num: u32) -> bool {
    let chip = match gpio_chip_find(gpio_num) {
        Some(c) => c,
        None => return false,
    };
    let mut pins = GPIO_PINS.lock();
    let idx = match pin_find_index(&pins, gpio_num) {
        Some(i) => i,
        None => return false,
    };
    if !pins[idx].requested {
        return false;
    }
    pins[idx].direction = GpioDirection::Input;

    match chip.chip_type {
        GpioChipType::Pch => {
            ich_gpio_direction(&chip, pins[idx].chip_offset, false);
        }
        GpioChipType::Simulated
        | GpioChipType::Generic
        | GpioChipType::Intel8255
        | GpioChipType::Acpi => {
            // Simulated / generic: direction stored in software state only
        }
    }
    true
}

/// Set a GPIO pin to output direction with an initial logical `value`.
///
/// Returns `false` if the pin is not requested or no chip is found.
pub fn gpio_direction_output(gpio_num: u32, value: bool) -> bool {
    let chip = match gpio_chip_find(gpio_num) {
        Some(c) => c,
        None => return false,
    };
    let mut pins = GPIO_PINS.lock();
    let idx = match pin_find_index(&pins, gpio_num) {
        Some(i) => i,
        None => return false,
    };
    if !pins[idx].requested {
        return false;
    }
    pins[idx].direction = GpioDirection::Output;
    pins[idx].value = value;

    match chip.chip_type {
        GpioChipType::Pch => {
            // Set direction first, then level
            ich_gpio_direction(&chip, pins[idx].chip_offset, true);
            let hw_val = value ^ pins[idx].active_low;
            ich_gpio_set(&chip, pins[idx].chip_offset, hw_val);
        }
        GpioChipType::Simulated
        | GpioChipType::Generic
        | GpioChipType::Intel8255
        | GpioChipType::Acpi => {
            // Stored in software only
        }
    }
    true
}

/// Read the logical level of a GPIO pin.
///
/// For Simulated chips: returns `value XOR active_low`.
/// For MMIO chips: reads hardware input register then applies active_low.
/// Returns `None` if the pin is not configured (not active in pin table).
pub fn gpio_get_value(gpio_num: u32) -> Option<bool> {
    let chip = gpio_chip_find(gpio_num)?;
    let pins = GPIO_PINS.lock();
    let idx = pin_find_index(&pins, gpio_num)?;
    if !pins[idx].active {
        return None;
    }
    let hw_val = match chip.chip_type {
        GpioChipType::Simulated
        | GpioChipType::Generic
        | GpioChipType::Intel8255
        | GpioChipType::Acpi => pins[idx].value,
        GpioChipType::Pch => ich_gpio_get(&chip, pins[idx].chip_offset),
    };
    Some(hw_val ^ pins[idx].active_low)
}

/// Write a logical level to a GPIO pin.
///
/// For Simulated chips: stores `value` in pin entry.
/// For MMIO chips: writes output register (XOR active_low for electrical level).
/// No-op if the pin is not in the pin table.
pub fn gpio_set_value(gpio_num: u32, value: bool) {
    let chip = match gpio_chip_find(gpio_num) {
        Some(c) => c,
        None => return,
    };
    let mut pins = GPIO_PINS.lock();
    let idx = match pin_find_index(&pins, gpio_num) {
        Some(i) => i,
        None => return,
    };
    let hw_val = value ^ pins[idx].active_low;
    match chip.chip_type {
        GpioChipType::Simulated
        | GpioChipType::Generic
        | GpioChipType::Intel8255
        | GpioChipType::Acpi => {
            pins[idx].value = value;
        }
        GpioChipType::Pch => {
            ich_gpio_set(&chip, pins[idx].chip_offset, hw_val);
            pins[idx].value = value;
        }
    }
}

/// Return the configured direction for a GPIO pin.
///
/// Returns `None` if the pin has no entry in the pin table.
pub fn gpio_get_direction(gpio_num: u32) -> Option<GpioDirection> {
    let pins = GPIO_PINS.lock();
    let idx = pin_find_index(&pins, gpio_num)?;
    if !pins[idx].active {
        return None;
    }
    Some(pins[idx].direction)
}

/// Return the Linux-style IRQ number for a GPIO.
///
/// Stub implementation: returns `gpio_num + GPIO_IRQ_BASE`.
/// Returns `None` if the pin has no registered chip.
pub fn gpio_to_irq(gpio_num: u32) -> Option<u32> {
    gpio_chip_find(gpio_num)?;
    Some(gpio_num.saturating_add(GPIO_IRQ_BASE))
}

/// Configure the interrupt trigger type for a GPIO pin.
///
/// Returns `false` if the pin is not found or not requested.
pub fn gpio_irq_set_type(gpio_num: u32, trigger: GpioIrqTrigger) -> bool {
    let mut pins = GPIO_PINS.lock();
    let idx = match pin_find_index(&pins, gpio_num) {
        Some(i) => i,
        None => return false,
    };
    if !pins[idx].requested {
        return false;
    }
    pins[idx].irq_trigger = trigger;
    true
}

/// Called by the interrupt controller when a GPIO IRQ fires for `gpio_num`.
///
/// Looks up the registered handler and invokes it with the GPIO number.
/// No-op if no handler is registered.
pub fn gpio_irq_handle(gpio_num: u32) {
    let handler = {
        let pins = GPIO_PINS.lock();
        match pin_find_index(&pins, gpio_num) {
            Some(idx) => pins[idx].irq_handler,
            None => None,
        }
    };
    if let Some(f) = handler {
        f(gpio_num);
    }
}

/// Set or clear the active-low flag for a GPIO pin.
///
/// No-op if the pin has no entry.
pub fn gpio_set_active_low(gpio_num: u32, active_low: bool) {
    let mut pins = GPIO_PINS.lock();
    if let Some(idx) = pin_find_index(&pins, gpio_num) {
        pins[idx].active_low = active_low;
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the GPIO subsystem.
///
/// Registers the built-in simulated chip (chip_id=0, GPIOs 0-31).
/// On real hardware, additional chips are registered from ACPI or PCI probing.
pub fn init() {
    let sim_label = *b"simulated       ";
    let sim_chip = GpioChip {
        chip_id: 0,
        base: 0,
        ngpio: 32,
        label: sim_label,
        mmio_base: 0,
        chip_type: GpioChipType::Simulated,
        active: true,
    };

    if gpio_chip_register(sim_chip) {
        serial_println!("  GPIO: simulated chip registered (GPIOs 0-31)");
    } else {
        serial_println!("  GPIO: simulated chip already registered");
    }

    super::register("gpio", super::DeviceType::Other);
}
