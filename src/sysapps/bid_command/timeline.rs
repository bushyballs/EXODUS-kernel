use crate::sync::Mutex;
/// Deadline extraction and tracking
///
/// Part of the Bid Command AIOS app. Parses deadlines from
/// solicitation text and tracks them with alerts.
/// Uses heuristic date pattern recognition.
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum number of deadlines tracked per timeline
const MAX_DEADLINES: usize = 64;

/// A tracked deadline
pub struct Deadline {
    pub label: String,
    pub due_timestamp: u64,
    pub alerted: bool,
}

pub struct Timeline {
    pub deadlines: Vec<Deadline>,
}

/// Date keyword patterns used for heuristic extraction
const DATE_KEYWORDS: &[(&str, u64)] = &[
    // keyword, approximate seconds from "now" (heuristic offsets)
    ("due date", 30 * 86400),            // ~30 days
    ("response deadline", 21 * 86400),   // ~21 days
    ("closing date", 30 * 86400),        // ~30 days
    ("questions due", 14 * 86400),       // ~14 days
    ("site visit", 7 * 86400),           // ~7 days
    ("pre-bid conference", 10 * 86400),  // ~10 days
    ("award date", 60 * 86400),          // ~60 days
    ("start date", 90 * 86400),          // ~90 days
    ("completion date", 365 * 86400),    // ~365 days
    ("submission deadline", 28 * 86400), // ~28 days
];

/// Helper: case-insensitive substring search
fn text_contains(text: &str, needle: &str) -> bool {
    if needle.is_empty() || text.len() < needle.len() {
        return false;
    }
    let text_bytes = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    let nlen = needle_bytes.len();

    for i in 0..=(text_bytes.len() - nlen) {
        let mut matched = true;
        for j in 0..nlen {
            let a = if text_bytes[i + j].is_ascii_uppercase() {
                text_bytes[i + j] + 32
            } else {
                text_bytes[i + j]
            };
            let b = if needle_bytes[j].is_ascii_uppercase() {
                needle_bytes[j] + 32
            } else {
                needle_bytes[j]
            };
            if a != b {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Monotonic base timestamp for deadlines (simulates "now")
static TIMELINE_BASE: Mutex<u64> = Mutex::new(1_700_000_000);

fn base_timestamp() -> u64 {
    *TIMELINE_BASE.lock()
}

impl Timeline {
    pub fn new() -> Self {
        crate::serial_println!("    [timeline] timeline tracker created");
        Self {
            deadlines: Vec::new(),
        }
    }

    /// Extract deadlines from solicitation text.
    /// Scans for known date-related keywords and creates deadline
    /// entries with heuristic timestamps. Returns count of new deadlines.
    pub fn extract(&mut self, text: &str) -> usize {
        let base = base_timestamp();
        let mut count = 0usize;

        for &(keyword, offset_secs) in DATE_KEYWORDS {
            if self.deadlines.len() >= MAX_DEADLINES {
                break;
            }
            if text_contains(text, keyword) {
                // Check we don't already have a deadline with this label
                let mut already_exists = false;
                for d in &self.deadlines {
                    if d.label.as_str() == keyword {
                        already_exists = true;
                        break;
                    }
                }
                if already_exists {
                    continue;
                }

                let mut label = String::new();
                for c in keyword.chars() {
                    label.push(c);
                }

                self.deadlines.push(Deadline {
                    label,
                    due_timestamp: base + offset_secs,
                    alerted: false,
                });
                count += 1;
            }
        }

        // Sort deadlines by due_timestamp ascending (insertion sort)
        let len = self.deadlines.len();
        for i in 1..len {
            let mut j = i;
            while j > 0 && self.deadlines[j].due_timestamp < self.deadlines[j - 1].due_timestamp {
                self.deadlines.swap(j, j - 1);
                j -= 1;
            }
        }

        crate::serial_println!(
            "    [timeline] extracted {} new deadlines from text ({} total)",
            count,
            self.deadlines.len()
        );
        count
    }

    /// Get deadlines due within the next N seconds from the base timestamp.
    pub fn upcoming(&self, within_secs: u64) -> Vec<&Deadline> {
        let base = base_timestamp();
        let cutoff = base + within_secs;
        let mut results = Vec::new();

        for deadline in &self.deadlines {
            if deadline.due_timestamp <= cutoff && !deadline.alerted {
                results.push(deadline);
            }
        }

        crate::serial_println!(
            "    [timeline] {} upcoming deadlines within {} seconds",
            results.len(),
            within_secs
        );
        results
    }

    /// Mark a deadline as alerted so it is not returned by upcoming() again.
    pub fn mark_alerted(&mut self, index: usize) {
        if index < self.deadlines.len() {
            self.deadlines[index].alerted = true;
            crate::serial_println!(
                "    [timeline] marked deadline '{}' as alerted",
                self.deadlines[index].label
            );
        } else {
            crate::serial_println!(
                "    [timeline] cannot mark deadline at index {}: out of bounds (have {})",
                index,
                self.deadlines.len()
            );
        }
    }

    /// Add a manual deadline
    pub fn add_manual(&mut self, label: &str, due_timestamp: u64) -> bool {
        if self.deadlines.len() >= MAX_DEADLINES {
            crate::serial_println!("    [timeline] cannot add manual deadline: at capacity");
            return false;
        }
        let mut l = String::new();
        for c in label.chars() {
            l.push(c);
        }
        self.deadlines.push(Deadline {
            label: l,
            due_timestamp,
            alerted: false,
        });
        crate::serial_println!(
            "    [timeline] added manual deadline '{}' at timestamp {}",
            label,
            due_timestamp
        );
        true
    }

    /// Get the total number of deadlines
    pub fn deadline_count(&self) -> usize {
        self.deadlines.len()
    }

    /// Get number of un-alerted deadlines
    pub fn pending_count(&self) -> usize {
        self.deadlines.iter().filter(|d| !d.alerted).count()
    }

    /// Clear all deadlines
    pub fn clear(&mut self) {
        self.deadlines.clear();
        crate::serial_println!("    [timeline] all deadlines cleared");
    }
}

/// Global timeline singleton
static TIMELINE: Mutex<Option<Timeline>> = Mutex::new(None);

pub fn init() {
    let mut tl = TIMELINE.lock();
    *tl = Some(Timeline::new());
    crate::serial_println!("    [timeline] deadline tracking subsystem initialized");
}
