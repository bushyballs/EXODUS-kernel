use crate::sync::Mutex;
/// Vendor discovery phase
///
/// Part of the Bid Command AIOS app. Finds and ranks
/// subcontractors based on project scope and location.
/// Uses local keyword matching and heuristic scoring.
use alloc::string::String;
use alloc::vec::Vec;

/// A discovered vendor candidate
pub struct VendorCandidate {
    pub name: String,
    pub relevance_score: u8,
    pub distance_miles: u32,
    pub reason: String,
}

/// A contact log entry for a vendor
struct ContactEntry {
    vendor_name: String,
    notes: String,
    timestamp: u64,
}

/// Monotonic counter for contact timestamps
static VENDOR_TICK: Mutex<u64> = Mutex::new(0);

fn tick() -> u64 {
    let mut t = VENDOR_TICK.lock();
    *t += 1;
    *t
}

pub struct VendorPhase {
    pub candidates: Vec<VendorCandidate>,
    contacts: Vec<ContactEntry>,
}

/// Simple keyword relevance scorer.
/// Returns 0..100 based on how many scope keywords appear in the vendor specialty.
fn keyword_score(scope: &str, specialty: &str) -> u8 {
    let scope_lower: Vec<char> = scope
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                (c as u8 + 32) as char
            } else {
                c
            }
        })
        .collect();
    let spec_lower: Vec<char> = specialty
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                (c as u8 + 32) as char
            } else {
                c
            }
        })
        .collect();

    let scope_str: String = scope_lower.into_iter().collect();
    let spec_str: String = spec_lower.into_iter().collect();

    let mut hits = 0u32;
    let mut total = 0u32;

    // Split scope on spaces and check containment
    let mut word_start = 0;
    let scope_bytes = scope_str.as_bytes();
    let len = scope_bytes.len();
    let mut i = 0;
    while i <= len {
        if i == len || scope_bytes[i] == b' ' {
            if i > word_start {
                total += 1;
                let word: String = scope_str[word_start..i].chars().collect();
                if word.len() >= 3 && spec_str.contains(word.as_str()) {
                    hits += 1;
                }
            }
            word_start = i + 1;
        }
        i += 1;
    }

    if total == 0 {
        return 0;
    }
    let score = (hits * 100) / total;
    if score > 100 {
        100
    } else {
        score as u8
    }
}

/// Built-in vendor database entries (local, no network required)
struct VendorRecord {
    name: &'static str,
    specialty: &'static str,
    base_distance: u32,
}

const VENDOR_DB: &[VendorRecord] = &[
    VendorRecord {
        name: "Allied Electrical Services",
        specialty: "electrical wiring conduit panels",
        base_distance: 15,
    },
    VendorRecord {
        name: "Precision Plumbing Co",
        specialty: "plumbing piping hvac water",
        base_distance: 22,
    },
    VendorRecord {
        name: "Ironworks Structural",
        specialty: "steel structural framing welding",
        base_distance: 30,
    },
    VendorRecord {
        name: "GreenScape Landscaping",
        specialty: "landscaping irrigation grading site",
        base_distance: 10,
    },
    VendorRecord {
        name: "Concrete Masters LLC",
        specialty: "concrete foundation slab masonry",
        base_distance: 18,
    },
    VendorRecord {
        name: "SafeGuard Security",
        specialty: "security cameras access control systems",
        base_distance: 25,
    },
    VendorRecord {
        name: "TechNet IT Solutions",
        specialty: "network cabling fiber it infrastructure",
        base_distance: 12,
    },
    VendorRecord {
        name: "ProPaint Coatings",
        specialty: "painting coating finishing interior exterior",
        base_distance: 8,
    },
    VendorRecord {
        name: "SolidRoof Systems",
        specialty: "roofing membrane insulation waterproofing",
        base_distance: 35,
    },
    VendorRecord {
        name: "CleanAir HVAC",
        specialty: "hvac ventilation air conditioning heating",
        base_distance: 20,
    },
];

impl VendorPhase {
    pub fn new() -> Self {
        crate::serial_println!("    [vendor-phase] vendor phase created");
        Self {
            candidates: Vec::new(),
            contacts: Vec::new(),
        }
    }

    /// Discover vendors matching the project requirements.
    /// Scores each vendor from the local DB against the scope keywords,
    /// then sorts by relevance descending.
    pub fn discover(&mut self, scope: &str, _location: &str) -> &[VendorCandidate] {
        crate::serial_println!(
            "    [vendor-phase] discovering vendors for scope: '{}'",
            scope
        );
        self.candidates.clear();

        for record in VENDOR_DB {
            let score = keyword_score(scope, record.specialty);
            if score > 0 {
                let mut name = String::new();
                for c in record.name.chars() {
                    name.push(c);
                }

                let reason = if score >= 70 {
                    let mut r = String::from("Strong match: specialty aligns with scope (");
                    // append score
                    let s = score;
                    if s >= 100 {
                        r.push('1');
                        r.push('0');
                        r.push('0');
                    } else {
                        if s >= 10 {
                            r.push((b'0' + s / 10) as char);
                        }
                        r.push((b'0' + s % 10) as char);
                    }
                    r.push_str("%)");
                    r
                } else if score >= 30 {
                    String::from("Moderate match: partial keyword overlap")
                } else {
                    String::from("Weak match: minimal keyword overlap")
                };

                self.candidates.push(VendorCandidate {
                    name,
                    relevance_score: score,
                    distance_miles: record.base_distance,
                    reason,
                });
            }
        }

        // Sort by relevance_score descending (insertion sort for no_std)
        let len = self.candidates.len();
        for i in 1..len {
            let mut j = i;
            while j > 0
                && self.candidates[j].relevance_score > self.candidates[j - 1].relevance_score
            {
                self.candidates.swap(j, j - 1);
                j -= 1;
            }
        }

        crate::serial_println!(
            "    [vendor-phase] found {} matching vendors",
            self.candidates.len()
        );
        &self.candidates
    }

    /// Log a contact attempt with a vendor
    pub fn log_contact(&mut self, vendor_name: &str, notes: &str) {
        let ts = tick();
        let mut vn = String::new();
        for c in vendor_name.chars() {
            vn.push(c);
        }
        let mut n = String::new();
        for c in notes.chars() {
            n.push(c);
        }

        crate::serial_println!(
            "    [vendor-phase] contact logged for '{}': {}",
            vendor_name,
            notes
        );

        self.contacts.push(ContactEntry {
            vendor_name: vn,
            notes: n,
            timestamp: ts,
        });
    }

    /// Get the number of contact attempts
    pub fn contact_count(&self) -> usize {
        self.contacts.len()
    }

    /// Get the number of candidates found
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }
}

/// Global vendor phase singleton
static VENDOR_PHASE: Mutex<Option<VendorPhase>> = Mutex::new(None);

pub fn init() {
    let mut vp = VENDOR_PHASE.lock();
    *vp = Some(VendorPhase::new());
    crate::serial_println!(
        "    [vendor-phase] vendor subsystem initialized with {} records",
        VENDOR_DB.len()
    );
}
