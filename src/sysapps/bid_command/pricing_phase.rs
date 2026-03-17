use crate::sync::Mutex;
/// Cost estimation phase
///
/// Part of the Bid Command AIOS app. Builds line-item
/// pricing using labor rates, materials, and overhead.
/// Computes subtotals and applies overhead percentage.
use alloc::string::String;
use alloc::vec::Vec;

/// Default overhead percentage if none specified
const DEFAULT_OVERHEAD_PCT: u8 = 15;

/// Maximum number of line items allowed
const MAX_LINE_ITEMS: usize = 500;

/// A single contract line item
pub struct ClinItem {
    pub description: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

impl ClinItem {
    /// Compute the extended price for this line item (qty * unit price)
    pub fn extended_cents(&self) -> u64 {
        self.quantity as u64 * self.unit_price_cents
    }
}

pub struct PricingPhase {
    pub items: Vec<ClinItem>,
    pub overhead_pct: u8,
}

impl PricingPhase {
    pub fn new() -> Self {
        crate::serial_println!(
            "    [pricing-phase] pricing phase created (overhead={}%)",
            DEFAULT_OVERHEAD_PCT
        );
        Self {
            items: Vec::new(),
            overhead_pct: DEFAULT_OVERHEAD_PCT,
        }
    }

    /// Add a line item to the pricing table.
    /// Silently rejects if the max line item count is reached.
    pub fn add_item(&mut self, desc: &str, qty: u32, unit_cents: u64) {
        if self.items.len() >= MAX_LINE_ITEMS {
            crate::serial_println!(
                "    [pricing-phase] cannot add item: max {} items reached",
                MAX_LINE_ITEMS
            );
            return;
        }

        let mut description = String::new();
        for c in desc.chars() {
            description.push(c);
        }

        self.items.push(ClinItem {
            description,
            quantity: qty,
            unit_price_cents: unit_cents,
        });

        crate::serial_println!(
            "    [pricing-phase] added CLIN: '{}' qty={} unit={} cents, extended={} cents",
            desc,
            qty,
            unit_cents,
            qty as u64 * unit_cents
        );
    }

    /// Compute the subtotal before overhead (sum of all extended prices)
    pub fn subtotal_cents(&self) -> u64 {
        let mut total: u64 = 0;
        for item in &self.items {
            total = total.saturating_add(item.extended_cents());
        }
        total
    }

    /// Compute the overhead amount in cents
    pub fn overhead_cents(&self) -> u64 {
        let sub = self.subtotal_cents();
        (sub * self.overhead_pct as u64) / 100
    }

    /// Compute the total bid price including overhead
    pub fn total_cents(&self) -> u64 {
        let sub = self.subtotal_cents();
        let overhead = (sub * self.overhead_pct as u64) / 100;
        let total = sub.saturating_add(overhead);
        crate::serial_println!(
            "    [pricing-phase] total: subtotal={} + overhead({}%)={} = {} cents",
            sub,
            self.overhead_pct,
            overhead,
            total
        );
        total
    }

    /// Set a custom overhead percentage (0..100)
    pub fn set_overhead(&mut self, pct: u8) {
        let clamped = if pct > 100 { 100 } else { pct };
        self.overhead_pct = clamped;
        crate::serial_println!("    [pricing-phase] overhead set to {}%", clamped);
    }

    /// Get the number of line items
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Remove a line item by index
    pub fn remove_item(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            let removed = self.items.remove(index);
            crate::serial_println!(
                "    [pricing-phase] removed CLIN at index {}: '{}'",
                index,
                removed.description
            );
            true
        } else {
            false
        }
    }

    /// Clear all line items
    pub fn clear(&mut self) {
        self.items.clear();
        crate::serial_println!("    [pricing-phase] all CLINs cleared");
    }
}

/// Global pricing phase singleton
static PRICING_PHASE: Mutex<Option<PricingPhase>> = Mutex::new(None);

pub fn init() {
    let mut pp = PRICING_PHASE.lock();
    *pp = Some(PricingPhase::new());
    crate::serial_println!("    [pricing-phase] pricing subsystem initialized");
}
