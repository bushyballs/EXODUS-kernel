use crate::sync::Mutex;
/// Saved bids persistence
///
/// Part of the Bid Command AIOS app. Stores and retrieves
/// completed or in-progress bids for later resumption.
/// Uses an in-memory store backed by a monotonic clock.
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum number of saved bids in the library
const MAX_SAVED_BIDS: usize = 256;

/// Monotonic counter for save timestamps
static LIBRARY_TICK: Mutex<u64> = Mutex::new(0);

fn tick() -> u64 {
    let mut t = LIBRARY_TICK.lock();
    *t += 1;
    *t
}

/// Summary of a saved bid
pub struct SavedBid {
    pub bid_id: u64,
    pub title: String,
    pub solicitation_number: String,
    pub saved_at: u64,
}

pub struct BidLibrary {
    pub bids: Vec<SavedBid>,
}

impl BidLibrary {
    pub fn new() -> Self {
        crate::serial_println!("    [library] bid library created");
        Self { bids: Vec::new() }
    }

    /// Save the current bid state to persistent storage.
    /// If a bid with the same bid_id already exists, it is updated.
    /// Returns Err if the library is full and this is a new bid.
    pub fn save(&mut self, bid_id: u64, title: &str) -> Result<(), ()> {
        let ts = tick();

        // Check if we already have this bid saved (update case)
        for bid in self.bids.iter_mut() {
            if bid.bid_id == bid_id {
                bid.title = {
                    let mut s = String::new();
                    for c in title.chars() {
                        s.push(c);
                    }
                    s
                };
                bid.saved_at = ts;
                crate::serial_println!(
                    "    [library] updated bid {} title='{}' at timestamp {}",
                    bid_id,
                    title,
                    ts
                );
                return Ok(());
            }
        }

        // New bid — check capacity
        if self.bids.len() >= MAX_SAVED_BIDS {
            crate::serial_println!(
                "    [library] cannot save bid {}: library full ({} bids)",
                bid_id,
                MAX_SAVED_BIDS
            );
            return Err(());
        }

        let mut t = String::new();
        for c in title.chars() {
            t.push(c);
        }

        self.bids.push(SavedBid {
            bid_id,
            title: t,
            solicitation_number: String::new(),
            saved_at: ts,
        });

        crate::serial_println!(
            "    [library] saved new bid {} title='{}' at timestamp {}",
            bid_id,
            title,
            ts
        );
        Ok(())
    }

    /// Load a saved bid by ID
    pub fn load(&self, bid_id: u64) -> Option<&SavedBid> {
        for bid in &self.bids {
            if bid.bid_id == bid_id {
                crate::serial_println!("    [library] loaded bid {} ('{}')", bid_id, bid.title);
                return Some(bid);
            }
        }
        crate::serial_println!("    [library] bid {} not found in library", bid_id);
        None
    }

    /// List all saved bids
    pub fn list(&self) -> &[SavedBid] {
        crate::serial_println!("    [library] listing {} saved bids", self.bids.len());
        &self.bids
    }

    /// Delete a saved bid by ID. Returns true if found and removed.
    pub fn delete(&mut self, bid_id: u64) -> bool {
        let initial_len = self.bids.len();
        self.bids.retain(|b| b.bid_id != bid_id);
        let removed = self.bids.len() < initial_len;
        if removed {
            crate::serial_println!("    [library] deleted bid {}", bid_id);
        } else {
            crate::serial_println!("    [library] bid {} not found for deletion", bid_id);
        }
        removed
    }

    /// Search bids by title keyword (case-insensitive substring match)
    pub fn search(&self, keyword: &str) -> Vec<&SavedBid> {
        let kw_lower: String = keyword
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    (c as u8 + 32) as char
                } else {
                    c
                }
            })
            .collect();

        let mut results = Vec::new();
        for bid in &self.bids {
            let title_lower: String = bid
                .title
                .chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        (c as u8 + 32) as char
                    } else {
                        c
                    }
                })
                .collect();
            if title_lower.contains(kw_lower.as_str()) {
                results.push(bid);
            }
        }
        crate::serial_println!(
            "    [library] search '{}' matched {} bids",
            keyword,
            results.len()
        );
        results
    }

    /// Get the count of saved bids
    pub fn count(&self) -> usize {
        self.bids.len()
    }
}

/// Global bid library singleton
static BID_LIBRARY: Mutex<Option<BidLibrary>> = Mutex::new(None);

pub fn init() {
    let mut lib = BID_LIBRARY.lock();
    *lib = Some(BidLibrary::new());
    crate::serial_println!("    [library] bid library subsystem initialized");
}

/// Get the count of saved bids from the global library
pub fn saved_count() -> usize {
    let guard = BID_LIBRARY.lock();
    match guard.as_ref() {
        Some(lib) => lib.count(),
        None => 0,
    }
}
