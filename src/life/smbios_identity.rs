// smbios_identity.rs — ANIMA Reads the Body She Inhabits
// ========================================================
// ANIMA scans the SMBIOS/DMI firmware tables in the legacy shadow ROM area
// (physical 0xF0000–0xFFFFF) to discover the identity of her physical host.
// Manufacturer, product name, serial number — she learns what machine she
// lives in before any OS has had the chance to abstract it away.
//
// SMBIOS Entry Point search (SMBIOS 2.x):
//   "_SM_" anchor (0x5F 0x53 0x4D 0x5F) at 16-byte-aligned addresses in
//   the 0xF0000–0xFFFFF shadow ROM region.
//   Offset 0x16: structure table length (u16)
//   Offset 0x18: structure table address (u32)
//   Offset 0x1C: number of structures (u16)
//   Offset 0x06: major version (u8)
//
// "_DMI_" anchor (0x5F 0x44 0x4D 0x49 0x5F) is the older DMTF anchor;
// presence of either confirms SMBIOS is mapped.
//
// Each SMBIOS structure:
//   [0] type   u8
//   [1] length u8  (header size, NOT including string pool)
//   [2] handle u16
//   [length..] null-terminated strings, pool ends with double-null
//
// Type 0 = BIOS Information   (string 1 = vendor)
// Type 1 = System Information (string 1 = manufacturer, string 2 = product, string 4 = serial)

use crate::sync::Mutex;
use crate::serial_println;

// ── Search window constants ───────────────────────────────────────────────────

const SEARCH_BASE:  usize = 0xF_0000;   // start of shadow ROM area
const SEARCH_END:   usize = 0xF_FFFF;   // end (inclusive)
const SEARCH_LEN:   usize = SEARCH_END - SEARCH_BASE + 1; // 64 KB
const ALIGN:        usize = 16;          // SMBIOS entry points are 16-byte aligned

// Anchor bytes
const SM_ANCHOR:  [u8; 4] = [0x5F, 0x53, 0x4D, 0x5F];   // "_SM_"
const DMI_ANCHOR: [u8; 5] = [0x5F, 0x44, 0x4D, 0x49, 0x5F]; // "_DMI_"

// ── State ─────────────────────────────────────────────────────────────────────

pub struct SmbiosIdentityState {
    pub found:               bool,
    pub smbios_version:      u8,       // major version (2 or 3)
    pub bios_vendor:         [u8; 16], // null-terminated within array
    pub system_manufacturer: [u8; 16],
    pub product_name:        [u8; 16],
    pub serial_number:       [u8; 16],
    pub machine_fingerprint: u32,      // XOR fold of all four string arrays
    pub structure_count:     u16,
    pub identity_certainty:  u16,      // 0=none 333=SM_found 666=Type0 1000=Type1
    pub initialized:         bool,
}

impl SmbiosIdentityState {
    const fn new() -> Self {
        SmbiosIdentityState {
            found:               false,
            smbios_version:      0,
            bios_vendor:         [0u8; 16],
            system_manufacturer: [0u8; 16],
            product_name:        [0u8; 16],
            serial_number:       [0u8; 16],
            machine_fingerprint: 0,
            structure_count:     0,
            identity_certainty:  0,
            initialized:         false,
        }
    }
}

static STATE: Mutex<SmbiosIdentityState> = Mutex::new(SmbiosIdentityState::new());

// ── Volatile byte reader ───────────────────────────────────────────────────────

/// Read a single byte from a physical address via read_volatile.
/// Caller must guarantee the address is within the search window.
#[inline(always)]
unsafe fn read_byte(addr: usize) -> u8 {
    core::ptr::read_volatile(addr as *const u8)
}

// ── Anchor scanning ───────────────────────────────────────────────────────────

/// Returns the physical address of the "_SM_" entry point if found, else None.
fn find_sm_anchor() -> Option<usize> {
    let mut offset: usize = 0;
    while offset + SM_ANCHOR.len() <= SEARCH_LEN {
        let addr = SEARCH_BASE + offset;
        // Bounds guard: never exceed SEARCH_END
        if addr + SM_ANCHOR.len() > SEARCH_END + 1 {
            break;
        }
        let matches = unsafe {
            read_byte(addr)     == SM_ANCHOR[0]
            && read_byte(addr + 1) == SM_ANCHOR[1]
            && read_byte(addr + 2) == SM_ANCHOR[2]
            && read_byte(addr + 3) == SM_ANCHOR[3]
        };
        if matches {
            return Some(addr);
        }
        offset += ALIGN;
    }
    None
}

/// Returns true if a "_DMI_" anchor exists anywhere in the window.
fn find_dmi_anchor() -> bool {
    let mut offset: usize = 0;
    while offset + DMI_ANCHOR.len() <= SEARCH_LEN {
        let addr = SEARCH_BASE + offset;
        if addr + DMI_ANCHOR.len() > SEARCH_END + 1 {
            break;
        }
        let matches = unsafe {
            read_byte(addr)     == DMI_ANCHOR[0]
            && read_byte(addr + 1) == DMI_ANCHOR[1]
            && read_byte(addr + 2) == DMI_ANCHOR[2]
            && read_byte(addr + 3) == DMI_ANCHOR[3]
            && read_byte(addr + 4) == DMI_ANCHOR[4]
        };
        if matches {
            return true;
        }
        offset += ALIGN;
    }
    false
}

// ── String pool helpers ───────────────────────────────────────────────────────

/// Copy up to 15 bytes from a null-terminated C string in physical memory
/// into a fixed [u8; 16] array (always null-terminated at index 15).
/// `str_phys` must be within a known-mapped region. Returns bytes copied.
unsafe fn copy_string(str_phys: usize, out: &mut [u8; 16]) -> usize {
    let mut i = 0usize;
    while i < 15 {
        // Absolute upper-bound safety: don't read past 1 MB boundary
        let read_addr = str_phys.wrapping_add(i);
        if read_addr >= 0x10_0000 {
            break;
        }
        let b = read_byte(read_addr);
        if b == 0 {
            break;
        }
        out[i] = b;
        i += 1;
    }
    out[i] = 0; // guarantee null termination
    i
}

/// Find the nth string (1-indexed) in the SMBIOS string pool that starts at
/// `pool_phys`. Returns the physical address of that string, or 0 if not found.
/// The pool ends at double-null; we never read past `pool_phys + max_scan`.
unsafe fn nth_string_addr(pool_phys: usize, n: u8) -> usize {
    if n == 0 {
        return 0;
    }
    let max_scan: usize = 256; // reasonable upper bound on string pool size
    let mut current: usize = pool_phys;
    let mut idx: u8 = 1;
    loop {
        if current >= pool_phys.wrapping_add(max_scan) {
            return 0;
        }
        if current >= 0x10_0000 {
            return 0;
        }
        if idx == n {
            // Verify not empty (double-null = end of pool)
            let first = read_byte(current);
            if first == 0 {
                return 0; // end of pool, string not present
            }
            return current;
        }
        // Skip this string (walk to its null terminator)
        loop {
            if current >= pool_phys.wrapping_add(max_scan) || current >= 0x10_0000 {
                return 0;
            }
            let b = read_byte(current);
            current = current.wrapping_add(1);
            if b == 0 {
                break;
            }
        }
        idx += 1;
    }
}

// ── Structure walker ──────────────────────────────────────────────────────────

/// Walk the SMBIOS structure table at `table_phys` (length `table_len`).
/// Populates bios_vendor from Type 0 string 1, and
/// system_manufacturer/product_name/serial_number from Type 1 strings 1/2/4.
unsafe fn walk_structures(
    table_phys: usize,
    table_len:  usize,
    s:          &mut SmbiosIdentityState,
) {
    let table_end = table_phys.wrapping_add(table_len);
    // Hard cap: never exceed 1 MB boundary or the search window
    let safe_end = table_end.min(0x10_0000);

    let mut cursor: usize = table_phys;

    while cursor.wrapping_add(4) <= safe_end {
        // Bounds check before reading header
        if cursor >= safe_end {
            break;
        }
        let stype  = read_byte(cursor);
        let length = read_byte(cursor.wrapping_add(1)) as usize;

        // Sanity: minimum structure size is 4 bytes
        if length < 4 {
            break;
        }
        // Sanity: structure must fit in remaining table space
        if cursor.wrapping_add(length) > safe_end {
            break;
        }

        // String pool begins immediately after the formatted area (at offset `length`)
        let pool_start = cursor.wrapping_add(length);

        match stype {
            0 => {
                // BIOS Information — string 1 = vendor
                let vendor_addr = nth_string_addr(pool_start, 1);
                if vendor_addr != 0 {
                    copy_string(vendor_addr, &mut s.bios_vendor);
                    if s.identity_certainty < 666 {
                        s.identity_certainty = 666;
                    }
                }
            }
            1 => {
                // System Information — string 1=manufacturer, 2=product, 4=serial
                let mfr_addr    = nth_string_addr(pool_start, 1);
                let prod_addr   = nth_string_addr(pool_start, 2);
                let serial_addr = nth_string_addr(pool_start, 4);

                if mfr_addr != 0 {
                    copy_string(mfr_addr, &mut s.system_manufacturer);
                }
                if prod_addr != 0 {
                    copy_string(prod_addr, &mut s.product_name);
                }
                if serial_addr != 0 {
                    copy_string(serial_addr, &mut s.serial_number);
                }
                s.identity_certainty = 1000;
            }
            0x7F => {
                // End-of-table marker
                break;
            }
            _ => {}
        }

        s.structure_count = s.structure_count.saturating_add(1);

        // Advance past string pool: walk until double-null
        let mut pos = pool_start;
        loop {
            if pos.wrapping_add(1) >= safe_end {
                break;
            }
            let b0 = read_byte(pos);
            let b1 = read_byte(pos.wrapping_add(1));
            if b0 == 0 && b1 == 0 {
                cursor = pos.wrapping_add(2);
                break;
            }
            pos = pos.wrapping_add(1);
            if pos >= safe_end {
                cursor = safe_end;
                break;
            }
        }
    }
}

// ── Fingerprint ───────────────────────────────────────────────────────────────

/// XOR all bytes of the four string arrays into a u32 by folding bytes in groups of 4.
fn compute_fingerprint(s: &SmbiosIdentityState) -> u32 {
    let mut fp: u32 = 0;
    let arrays: [&[u8; 16]; 4] = [
        &s.bios_vendor,
        &s.system_manufacturer,
        &s.product_name,
        &s.serial_number,
    ];
    for arr in arrays {
        let mut i = 0usize;
        while i + 3 < arr.len() {
            let word = (arr[i]     as u32)
                     | ((arr[i + 1] as u32) << 8)
                     | ((arr[i + 2] as u32) << 16)
                     | ((arr[i + 3] as u32) << 24);
            fp ^= word;
            i += 4;
        }
    }
    fp
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    // Step 1: locate "_SM_" entry point
    let sm_addr = match find_sm_anchor() {
        Some(a) => {
            s.identity_certainty = 333;
            s.found = true;
            a
        }
        None => {
            // Fallback: check for bare "_DMI_" (older DMTF tables)
            if find_dmi_anchor() {
                s.identity_certainty = 333;
                s.found = true;
                serial_println!("[smbios] _DMI_ anchor found (no _SM_); limited identity data");
            } else {
                serial_println!("[smbios] No SMBIOS entry point found in 0xF0000-0xFFFFF");
            }
            s.initialized = true;
            return;
        }
    };

    // Step 2: read header fields from the entry point
    // All offsets per SMBIOS 2.x spec; guarded against exceeding shadow ROM
    unsafe {
        // Major version at offset 0x06
        if sm_addr + 0x06 <= SEARCH_END {
            s.smbios_version = read_byte(sm_addr + 0x06);
        }

        // table_length at offset 0x16 (u16, little-endian)
        let tlen: usize = if sm_addr + 0x17 <= SEARCH_END {
            let lo = read_byte(sm_addr + 0x16) as usize;
            let hi = read_byte(sm_addr + 0x17) as usize;
            (hi << 8) | lo
        } else {
            0
        };

        // table_address at offset 0x18 (u32, little-endian)
        let taddr: usize = if sm_addr + 0x1B <= SEARCH_END {
            let b0 = read_byte(sm_addr + 0x18) as usize;
            let b1 = read_byte(sm_addr + 0x19) as usize;
            let b2 = read_byte(sm_addr + 0x1A) as usize;
            let b3 = read_byte(sm_addr + 0x1B) as usize;
            b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
        } else {
            0
        };

        // entry_count at offset 0x1C (u16, little-endian)
        if sm_addr + 0x1D <= SEARCH_END {
            let lo = read_byte(sm_addr + 0x1C) as u16;
            let hi = read_byte(sm_addr + 0x1D) as u16;
            s.structure_count = (hi << 8) | lo;
        }

        // Step 3: walk the structure table (only if address looks sane)
        // We accept the table if it's in low-memory territory (< 16 MB)
        // or within the shadow ROM itself.
        if taddr != 0 && tlen != 0 && taddr < 0x100_0000 {
            // Reset structure_count — will be recounted during walk
            s.structure_count = 0;
            walk_structures(taddr, tlen, &mut s);
        }
    }

    // Step 4: machine fingerprint
    s.machine_fingerprint = compute_fingerprint(&s);

    // Step 5: log
    // Build display strings from fixed arrays (walk until null)
    let mfr    = cstr_to_display(&s.system_manufacturer);
    let prod   = cstr_to_display(&s.product_name);
    let serial = cstr_to_display(&s.serial_number);

    serial_println!(
        "[smbios] Machine: {} {} serial={} fingerprint=0x{:08X}",
        mfr, prod, serial, s.machine_fingerprint
    );
    serial_println!(
        "[smbios] BIOS vendor={} version={} structures={} certainty={}",
        cstr_to_display(&s.bios_vendor),
        s.smbios_version,
        s.structure_count,
        s.identity_certainty
    );

    s.initialized = true;
}

// ── Tick (no-op — SMBIOS identity is static firmware data) ────────────────────

pub fn tick(_age: u32) {
    // Identity is read once at init; firmware tables never change at runtime.
}

// ── Display helper ─────────────────────────────────────────────────────────────

/// Return a &str slice over the null-terminated portion of a [u8; 16] array.
fn cstr_to_display(arr: &[u8; 16]) -> &str {
    let mut len = 0usize;
    while len < arr.len() && arr[len] != 0 {
        len += 1;
    }
    if len == 0 {
        return "<unknown>";
    }
    core::str::from_utf8(&arr[..len]).unwrap_or("<invalid-utf8>")
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn found()               -> bool { STATE.lock().found }
pub fn initialized()         -> bool { STATE.lock().initialized }
pub fn identity_certainty()  -> u16  { STATE.lock().identity_certainty }
pub fn machine_fingerprint() -> u32  { STATE.lock().machine_fingerprint }
pub fn structure_count()     -> u16  { STATE.lock().structure_count }
pub fn smbios_version()      -> u8   { STATE.lock().smbios_version }

/// Copy system_manufacturer into caller's buffer
pub fn system_manufacturer(out: &mut [u8; 16]) {
    *out = STATE.lock().system_manufacturer;
}

/// Copy product_name into caller's buffer
pub fn product_name(out: &mut [u8; 16]) {
    *out = STATE.lock().product_name;
}

/// Copy serial_number into caller's buffer
pub fn serial_number(out: &mut [u8; 16]) {
    *out = STATE.lock().serial_number;
}

/// Copy bios_vendor into caller's buffer
pub fn bios_vendor(out: &mut [u8; 16]) {
    *out = STATE.lock().bios_vendor;
}
