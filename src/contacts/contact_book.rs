/// contact_book.rs — static contact book with fixed-size byte arrays.
///
/// Provides a no-alloc, no_std compatible contact store backed by a
/// 256-slot static array.  Field storage uses fixed-size `[u8; N]`
/// arrays so the entire book lives in BSS with no heap allocation.
///
/// vCard 3.0 minimal import is handled by `import_vcard()` which
/// recognises the `FN:`, `N:`, `TEL:`, and `EMAIL:` property lines.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// ContactEntry
// ---------------------------------------------------------------------------

/// A single contact stored in the static contact book.
///
/// All text fields are UTF-8 encoded in fixed-size byte arrays.
/// Unused trailing bytes are set to 0.
#[derive(Clone, Copy)]
pub struct ContactEntry {
    /// Display name (FN field from vCard, or first + last).
    pub name: [u8; 64],
    pub name_len: usize,

    /// Primary phone number as ASCII digits / symbols.
    pub phone: [u8; 32],
    pub phone_len: usize,

    /// Primary email address.
    pub email: [u8; 64],
    pub email_len: usize,

    /// Whether this slot is occupied.
    pub valid: bool,
}

impl ContactEntry {
    /// Construct an empty, invalid entry.
    pub const fn empty() -> Self {
        Self {
            name: [0u8; 64],
            name_len: 0,
            phone: [0u8; 32],
            phone_len: 0,
            email: [0u8; 64],
            email_len: 0,
            valid: false,
        }
    }

    /// Build a valid entry from byte slices (truncated if too long).
    pub fn new(name: &[u8], phone: &[u8], email: &[u8]) -> Self {
        let mut entry = Self::empty();
        entry.valid = true;

        let nlen = name.len().min(64);
        entry.name[..nlen].copy_from_slice(&name[..nlen]);
        entry.name_len = nlen;

        let plen = phone.len().min(32);
        entry.phone[..plen].copy_from_slice(&phone[..plen]);
        entry.phone_len = plen;

        let elen = email.len().min(64);
        entry.email[..elen].copy_from_slice(&email[..elen]);
        entry.email_len = elen;

        entry
    }

    /// Return the name as a `&str` if valid UTF-8, otherwise empty.
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }

    /// Return the phone as a `&str`.
    pub fn phone_str(&self) -> &str {
        core::str::from_utf8(&self.phone[..self.phone_len]).unwrap_or("")
    }

    /// Return the email as a `&str`.
    pub fn email_str(&self) -> &str {
        core::str::from_utf8(&self.email[..self.email_len]).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Static contact book
// ---------------------------------------------------------------------------

/// Maximum number of contacts in the static book.
pub const CONTACT_BOOK_SIZE: usize = 256;

/// The global static contact book.  Slot 0 is reserved/unused.
static mut CONTACT_BOOK: [Option<ContactEntry>; CONTACT_BOOK_SIZE] = [None; CONTACT_BOOK_SIZE];

/// Total number of valid contacts currently stored.
static mut CONTACT_COUNT: usize = 0;

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// Add a contact to the first available slot.
///
/// Returns the slot index on success, or `None` if the book is full.
pub fn add_contact(name: &[u8], phone: &[u8], email: &[u8]) -> Option<usize> {
    // Safety: single-threaded kernel context.
    unsafe {
        for idx in 1..CONTACT_BOOK_SIZE {
            if CONTACT_BOOK[idx].is_none() {
                CONTACT_BOOK[idx] = Some(ContactEntry::new(name, phone, email));
                CONTACT_COUNT = CONTACT_COUNT.saturating_add(1);
                serial_println!("[contact_book] add_contact slot={}", idx);
                return Some(idx);
            }
        }
    }
    serial_println!("[contact_book] add_contact: book full");
    None
}

/// Remove the contact at `slot`.  Returns `true` if a contact was present.
pub fn remove_contact(slot: usize) -> bool {
    if slot == 0 || slot >= CONTACT_BOOK_SIZE {
        return false;
    }
    unsafe {
        if CONTACT_BOOK[slot].is_some() {
            CONTACT_BOOK[slot] = None;
            CONTACT_COUNT = CONTACT_COUNT.saturating_sub(1);
            serial_println!("[contact_book] remove_contact slot={}", slot);
            return true;
        }
    }
    false
}

/// Find the first contact whose name starts with `name`.
///
/// Comparison is byte-wise and case-sensitive.  Returns the slot index,
/// or `None` if not found.
pub fn find_contact(name: &str) -> Option<usize> {
    let needle = name.as_bytes();
    unsafe {
        for idx in 1..CONTACT_BOOK_SIZE {
            if let Some(ref entry) = CONTACT_BOOK[idx] {
                let hay = &entry.name[..entry.name_len];
                if hay.len() >= needle.len() && &hay[..needle.len()] == needle {
                    return Some(idx);
                }
            }
        }
    }
    None
}

/// Return a shared reference to the contact at `slot`.
pub fn get_contact(slot: usize) -> Option<&'static ContactEntry> {
    if slot >= CONTACT_BOOK_SIZE {
        return None;
    }
    unsafe { CONTACT_BOOK[slot].as_ref() }
}

/// Return the current count of valid contacts.
pub fn contact_count() -> usize {
    unsafe { CONTACT_COUNT }
}

// ---------------------------------------------------------------------------
// vCard 3.0 minimal importer
// ---------------------------------------------------------------------------

/// Minimal vCard 3.0 / 4.0 parser.
///
/// Recognises:
/// - `BEGIN:VCARD` — start of a card
/// - `FN:<display-name>` — formatted name (used as `name`)
/// - `N:<last>;<first>;...` — structured name (used only if FN absent)
/// - `TEL[;TYPE=...]:<number>` — telephone
/// - `EMAIL[;TYPE=...]:<address>` — email
/// - `END:VCARD` — flush the pending contact
///
/// All other lines are ignored.  Returns the number of contacts imported.
pub fn import_vcard(data: &[u8]) -> usize {
    let mut imported: usize = 0;

    // Per-card accumulators (reset on BEGIN/END).
    let mut in_card = false;
    let mut fn_buf = [0u8; 64];
    let mut fn_len = 0usize;
    let mut n_buf = [0u8; 64];
    let mut n_len = 0usize;
    let mut tel_buf = [0u8; 32];
    let mut tel_len = 0usize;
    let mut mail_buf = [0u8; 64];
    let mut mail_len = 0usize;

    // Split `data` on '\n', strip '\r'.
    let mut start = 0usize;
    for end in 0..=data.len() {
        let at_end = end == data.len();
        if at_end || data[end] == b'\n' {
            // Extract the line bytes, stripping trailing '\r'.
            let raw = &data[start..end];
            let line = if raw.last() == Some(&b'\r') {
                &raw[..raw.len() - 1]
            } else {
                raw
            };

            // ── Dispatch on the property name ────────────────────────
            if line.eq_ignore_ascii_case(b"BEGIN:VCARD") {
                in_card = true;
                fn_len = 0;
                n_len = 0;
                tel_len = 0;
                mail_len = 0;
            } else if line.eq_ignore_ascii_case(b"END:VCARD") && in_card {
                in_card = false;
                // Choose name: prefer FN, fall back to N.
                let (name, nlen) = if fn_len > 0 {
                    (&fn_buf, fn_len)
                } else {
                    (&n_buf, n_len)
                };
                if nlen > 0 {
                    if add_contact(&name[..nlen], &tel_buf[..tel_len], &mail_buf[..mail_len])
                        .is_some()
                    {
                        imported = imported.saturating_add(1);
                    }
                }
            } else if in_card {
                parse_vcard_line(
                    line,
                    &mut fn_buf,
                    &mut fn_len,
                    &mut n_buf,
                    &mut n_len,
                    &mut tel_buf,
                    &mut tel_len,
                    &mut mail_buf,
                    &mut mail_len,
                );
            }

            start = end.saturating_add(1);
        }
    }

    serial_println!(
        "[contact_book] import_vcard: imported {} contact(s)",
        imported
    );
    imported
}

/// Extract a single vCard property line into the accumulators.
fn parse_vcard_line(
    line: &[u8],
    fn_buf: &mut [u8; 64],
    fn_len: &mut usize,
    n_buf: &mut [u8; 64],
    n_len: &mut usize,
    tel_buf: &mut [u8; 32],
    tel_len: &mut usize,
    mail_buf: &mut [u8; 64],
    mail_len: &mut usize,
) {
    // Find the first ':' — that separates property (+ params) from value.
    let colon = match line.iter().position(|&b| b == b':') {
        Some(p) => p,
        None => return,
    };
    let prop = &line[..colon];
    let value = &line[colon + 1..];

    // Property name is everything before the first ';' (parameter delimiter).
    let prop_name = match prop.iter().position(|&b| b == b';') {
        Some(sc) => &prop[..sc],
        None => prop,
    };

    if prop_name.eq_ignore_ascii_case(b"FN") {
        let vlen = value.len().min(64);
        fn_buf[..vlen].copy_from_slice(&value[..vlen]);
        *fn_len = vlen;
    } else if prop_name.eq_ignore_ascii_case(b"N") {
        // Structured: last;first;middle;prefix;suffix — use first;last
        // For simplicity, join the first two non-empty components.
        let mut out = [0u8; 64];
        let mut olen = 0usize;
        let mut part = 0u8;
        for &b in value {
            if b == b';' {
                part = part.saturating_add(1);
                if part >= 2 {
                    break;
                }
                // Insert a space between last and first
                if olen < 64 {
                    out[olen] = b' ';
                    olen += 1;
                }
            } else if olen < 64 {
                out[olen] = b;
                olen += 1;
            }
        }
        n_buf[..olen].copy_from_slice(&out[..olen]);
        *n_len = olen;
    } else if prop_name.eq_ignore_ascii_case(b"TEL") {
        // Only store the first telephone encountered.
        if *tel_len == 0 {
            let vlen = value.len().min(32);
            tel_buf[..vlen].copy_from_slice(&value[..vlen]);
            *tel_len = vlen;
        }
    } else if prop_name.eq_ignore_ascii_case(b"EMAIL") {
        if *mail_len == 0 {
            let vlen = value.len().min(64);
            mail_buf[..vlen].copy_from_slice(&value[..vlen]);
            *mail_len = vlen;
        }
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // Ensure the static array is zero-initialised (it is by default in Rust
    // statics, but we log to confirm the subsystem is live).
    serial_println!(
        "[contact_book] static contact book ready (slots={})",
        CONTACT_BOOK_SIZE
    );
}
