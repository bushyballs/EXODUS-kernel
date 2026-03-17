use crate::sync::Mutex;
/// SAM.gov opportunity browser
///
/// Part of the Bid Command AIOS app. Searches and filters
/// SAM.gov opportunities by NAICS, set-aside, location, etc.
/// Uses a local cached opportunity database (no network required).
use alloc::string::String;
use alloc::vec::Vec;

/// Monotonic counter for bid ID assignment
static NEXT_CLAIM_ID: Mutex<u64> = Mutex::new(1000);

fn allocate_claim_id() -> u64 {
    let mut id = NEXT_CLAIM_ID.lock();
    let current = *id;
    *id += 1;
    current
}

/// A SAM.gov opportunity summary
pub struct Opportunity {
    pub notice_id: String,
    pub title: String,
    pub naics_code: String,
    pub posted_date: u64,
    pub response_deadline: u64,
}

/// Static opportunity database entries (simulates cached SAM.gov data)
struct OpportunityRecord {
    notice_id: &'static str,
    title: &'static str,
    naics_code: &'static str,
    keywords: &'static str,
    posted_offset: u64,
    deadline_offset: u64,
}

/// Base timestamp for opportunity dates
const BASE_TIMESTAMP: u64 = 1_700_000_000;

const OPPORTUNITY_DB: &[OpportunityRecord] = &[
    OpportunityRecord {
        notice_id: "SAM-2024-001",
        title: "HVAC System Replacement - Building 42",
        naics_code: "238220",
        keywords: "hvac heating cooling ventilation mechanical",
        posted_offset: 0,
        deadline_offset: 30 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-002",
        title: "Electrical Infrastructure Upgrade Phase II",
        naics_code: "238210",
        keywords: "electrical wiring panel upgrade infrastructure",
        posted_offset: 2 * 86400,
        deadline_offset: 45 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-003",
        title: "Roof Replacement and Waterproofing",
        naics_code: "238160",
        keywords: "roofing waterproofing membrane replacement",
        posted_offset: 5 * 86400,
        deadline_offset: 21 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-004",
        title: "IT Network Cabling Installation",
        naics_code: "238210",
        keywords: "network cabling fiber optic it infrastructure data",
        posted_offset: 3 * 86400,
        deadline_offset: 28 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-005",
        title: "Landscaping and Grounds Maintenance Services",
        naics_code: "561730",
        keywords: "landscaping grounds maintenance mowing irrigation",
        posted_offset: 1 * 86400,
        deadline_offset: 14 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-006",
        title: "Concrete Sidewalk and Parking Lot Repair",
        naics_code: "238110",
        keywords: "concrete sidewalk parking repair paving",
        posted_offset: 7 * 86400,
        deadline_offset: 35 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-007",
        title: "Security Camera System Installation",
        naics_code: "561621",
        keywords: "security camera surveillance access control",
        posted_offset: 4 * 86400,
        deadline_offset: 25 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-008",
        title: "Interior Painting and Wall Repair",
        naics_code: "238320",
        keywords: "painting interior walls coating finishing drywall",
        posted_offset: 6 * 86400,
        deadline_offset: 20 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-009",
        title: "Plumbing System Overhaul - Medical Facility",
        naics_code: "238220",
        keywords: "plumbing piping water medical facility overhaul",
        posted_offset: 8 * 86400,
        deadline_offset: 40 * 86400,
    },
    OpportunityRecord {
        notice_id: "SAM-2024-010",
        title: "Structural Steel Fabrication and Erection",
        naics_code: "238120",
        keywords: "steel structural fabrication erection welding framing",
        posted_offset: 10 * 86400,
        deadline_offset: 60 * 86400,
    },
];

/// Case-insensitive substring check
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let nlen = n.len();

    for i in 0..=(h.len() - nlen) {
        let mut ok = true;
        for j in 0..nlen {
            let a = if h[i + j].is_ascii_uppercase() {
                h[i + j] + 32
            } else {
                h[i + j]
            };
            let b = if n[j].is_ascii_uppercase() {
                n[j] + 32
            } else {
                n[j]
            };
            if a != b {
                ok = false;
                break;
            }
        }
        if ok {
            return true;
        }
    }
    false
}

/// Set of claimed notice IDs
static CLAIMED: Mutex<Option<Vec<String>>> = Mutex::new(None);

pub struct SamScout {
    pub results: Vec<Opportunity>,
}

impl SamScout {
    pub fn new() -> Self {
        crate::serial_println!("    [sam-scout] SAM.gov scout created");
        Self {
            results: Vec::new(),
        }
    }

    /// Search opportunities with the given query string.
    /// Matches against title, keywords, and NAICS code.
    /// Returns the matched opportunities sorted by relevance.
    pub fn search(&mut self, query: &str) -> &[Opportunity] {
        crate::serial_println!("    [sam-scout] searching for '{}'", query);
        self.results.clear();

        // Split query into words for multi-keyword matching
        let query_words: Vec<&str> = query.split(' ').filter(|w| !w.is_empty()).collect();

        // Score and collect matches
        let mut scored: Vec<(usize, u32)> = Vec::new(); // (index into OPPORTUNITY_DB, score)

        for (idx, record) in OPPORTUNITY_DB.iter().enumerate() {
            let mut score = 0u32;

            for &word in &query_words {
                if word.len() < 2 {
                    continue;
                }
                // Title match (high weight)
                if contains_ci(record.title, word) {
                    score += 10;
                }
                // Keyword match (medium weight)
                if contains_ci(record.keywords, word) {
                    score += 5;
                }
                // NAICS match (medium weight)
                if contains_ci(record.naics_code, word) {
                    score += 8;
                }
                // Notice ID match
                if contains_ci(record.notice_id, word) {
                    score += 15;
                }
            }

            if score > 0 {
                scored.push((idx, score));
            }
        }

        // Sort by score descending (insertion sort)
        let len = scored.len();
        for i in 1..len {
            let mut j = i;
            while j > 0 && scored[j].1 > scored[j - 1].1 {
                scored.swap(j, j - 1);
                j -= 1;
            }
        }

        // Convert to Opportunity structs
        for &(idx, _score) in &scored {
            let record = &OPPORTUNITY_DB[idx];
            let mut notice_id = String::new();
            for c in record.notice_id.chars() {
                notice_id.push(c);
            }
            let mut title = String::new();
            for c in record.title.chars() {
                title.push(c);
            }
            let mut naics = String::new();
            for c in record.naics_code.chars() {
                naics.push(c);
            }

            self.results.push(Opportunity {
                notice_id,
                title,
                naics_code: naics,
                posted_date: BASE_TIMESTAMP + record.posted_offset,
                response_deadline: BASE_TIMESTAMP + record.deadline_offset,
            });
        }

        crate::serial_println!(
            "    [sam-scout] found {} matching opportunities",
            self.results.len()
        );
        &self.results
    }

    /// Claim an opportunity to start a bid.
    /// Returns a new bid ID if the notice exists, Err otherwise.
    pub fn claim(&self, notice_id: &str) -> Result<u64, ()> {
        // Check if this notice is in our results or DB
        let mut found = false;
        for record in OPPORTUNITY_DB {
            if record.notice_id == notice_id {
                found = true;
                break;
            }
        }

        if !found {
            crate::serial_println!(
                "    [sam-scout] cannot claim '{}': notice not found",
                notice_id
            );
            return Err(());
        }

        // Check if already claimed
        let mut claimed = CLAIMED.lock();
        if let Some(ref list) = *claimed {
            for c in list {
                if c.as_str() == notice_id {
                    crate::serial_println!(
                        "    [sam-scout] notice '{}' already claimed",
                        notice_id
                    );
                    return Err(());
                }
            }
        }

        // Record the claim
        let bid_id = allocate_claim_id();
        if let Some(ref mut list) = *claimed {
            let mut nid = String::new();
            for c in notice_id.chars() {
                nid.push(c);
            }
            list.push(nid);
        }

        crate::serial_println!(
            "    [sam-scout] claimed notice '{}' -> bid_id={}",
            notice_id,
            bid_id
        );
        Ok(bid_id)
    }

    /// Get the number of results from the last search
    pub fn result_count(&self) -> usize {
        self.results.len()
    }
}

/// Global SAM scout singleton
static SAM_SCOUT: Mutex<Option<SamScout>> = Mutex::new(None);

pub fn init() {
    let mut scout = SAM_SCOUT.lock();
    *scout = Some(SamScout::new());
    let mut claimed = CLAIMED.lock();
    *claimed = Some(Vec::new());
    crate::serial_println!(
        "    [sam-scout] SAM.gov scout initialized with {} cached opportunities",
        OPPORTUNITY_DB.len()
    );
}
