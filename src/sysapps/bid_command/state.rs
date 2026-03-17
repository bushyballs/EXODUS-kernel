use crate::sync::Mutex;
/// Bid state management
///
/// Part of the Bid Command AIOS app. Tracks the current bid
/// lifecycle phase and all volatile in-progress data.
use alloc::string::String;
use alloc::vec::Vec;

/// Current phase of the bid workflow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidPhase {
    Scout,
    Analysis,
    Vendor,
    Pricing,
    Package,
    Submitted,
}

impl BidPhase {
    /// Return the next phase in the workflow, or None if already submitted.
    fn next(self) -> Option<BidPhase> {
        match self {
            BidPhase::Scout => Some(BidPhase::Analysis),
            BidPhase::Analysis => Some(BidPhase::Vendor),
            BidPhase::Vendor => Some(BidPhase::Pricing),
            BidPhase::Pricing => Some(BidPhase::Package),
            BidPhase::Package => Some(BidPhase::Submitted),
            BidPhase::Submitted => None,
        }
    }

    /// Human-readable label for this phase.
    pub fn label(self) -> &'static str {
        match self {
            BidPhase::Scout => "Scout",
            BidPhase::Analysis => "Analysis",
            BidPhase::Vendor => "Vendor",
            BidPhase::Pricing => "Pricing",
            BidPhase::Package => "Package",
            BidPhase::Submitted => "Submitted",
        }
    }
}

/// Global bid-id counter for generating unique IDs
static NEXT_BID_ID: Mutex<u64> = Mutex::new(1);

fn allocate_bid_id() -> u64 {
    let mut id = NEXT_BID_ID.lock();
    let current = *id;
    *id += 1;
    current
}

pub struct BidState {
    pub bid_id: u64,
    pub phase: BidPhase,
    pub solicitation_number: String,
}

impl BidState {
    pub fn new() -> Self {
        let bid_id = allocate_bid_id();
        crate::serial_println!("    [bid-state] new bid state created, bid_id={}", bid_id);
        Self {
            bid_id,
            phase: BidPhase::Scout,
            solicitation_number: String::new(),
        }
    }

    /// Advance to the next workflow phase
    pub fn advance(&mut self) -> BidPhase {
        if let Some(next) = self.phase.next() {
            let prev = self.phase;
            self.phase = next;
            crate::serial_println!(
                "    [bid-state] bid {} advanced from {} to {}",
                self.bid_id,
                prev.label(),
                self.phase.label()
            );
        } else {
            crate::serial_println!(
                "    [bid-state] bid {} already at terminal phase ({})",
                self.bid_id,
                self.phase.label()
            );
        }
        self.phase
    }

    /// Reset state for a new bid
    pub fn reset(&mut self) {
        let old_id = self.bid_id;
        self.bid_id = allocate_bid_id();
        self.phase = BidPhase::Scout;
        self.solicitation_number = String::new();
        crate::serial_println!(
            "    [bid-state] reset (old bid_id={}, new bid_id={})",
            old_id,
            self.bid_id
        );
    }

    /// Get current phase label
    pub fn phase_label(&self) -> &'static str {
        self.phase.label()
    }

    /// Check if the bid has been submitted
    pub fn is_submitted(&self) -> bool {
        self.phase == BidPhase::Submitted
    }

    /// Set the solicitation number
    pub fn set_solicitation(&mut self, sol_num: &str) {
        self.solicitation_number = String::new();
        for c in sol_num.chars() {
            self.solicitation_number.push(c);
        }
        crate::serial_println!(
            "    [bid-state] bid {}: solicitation set to '{}'",
            self.bid_id,
            self.solicitation_number
        );
    }
}

/// Global active bid state singleton
static ACTIVE_BID: Mutex<Option<BidState>> = Mutex::new(None);

pub fn init() {
    let mut active = ACTIVE_BID.lock();
    *active = Some(BidState::new());
    crate::serial_println!("    [bid-state] bid state subsystem initialized");
}

/// Get the active bid id (if any)
pub fn active_bid_id() -> Option<u64> {
    let guard = ACTIVE_BID.lock();
    guard.as_ref().map(|b| b.bid_id)
}

/// Get the active bid phase label
pub fn active_phase() -> Option<&'static str> {
    let guard = ACTIVE_BID.lock();
    guard.as_ref().map(|b| b.phase.label())
}
