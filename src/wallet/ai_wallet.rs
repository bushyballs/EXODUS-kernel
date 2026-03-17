use crate::sync::Mutex;
/// AI-enhanced wallet for Genesis
///
/// Fraud detection, spending analytics, smart budgeting,
/// merchant categorization, price comparison.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum MerchantCategory {
    Groceries,
    Dining,
    Transportation,
    Entertainment,
    Shopping,
    Healthcare,
    Utilities,
    Subscription,
    Travel,
    Other,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FraudRisk {
    None,
    Low,
    Medium,
    High,
}

struct SpendingPattern {
    category: MerchantCategory,
    avg_monthly_cents: u64,
    last_month_cents: u64,
    transaction_count: u32,
}

struct FraudFeatures {
    amount_cents: u64,
    hour_of_day: u8,
    is_international: bool,
    new_merchant: bool,
    distance_from_last_km: u32,
    time_since_last_sec: u32,
}

struct AiWalletEngine {
    spending: Vec<SpendingPattern>,
    monthly_budget_cents: u64,
    monthly_spent_cents: u64,
    fraud_flags: u32,
    total_analyzed: u32,
}

static AI_WALLET: Mutex<Option<AiWalletEngine>> = Mutex::new(None);

impl AiWalletEngine {
    fn new() -> Self {
        AiWalletEngine {
            spending: Vec::new(),
            monthly_budget_cents: 300000, // $3000
            monthly_spent_cents: 0,
            fraud_flags: 0,
            total_analyzed: 0,
        }
    }

    fn assess_fraud(&mut self, features: &FraudFeatures) -> FraudRisk {
        self.total_analyzed = self.total_analyzed.saturating_add(1);
        let mut risk = 0u32;
        // Large unusual amount
        if features.amount_cents > 50000 {
            risk += 15;
        }
        if features.amount_cents > 200000 {
            risk += 25;
        }
        // Late night
        if features.hour_of_day > 23 || features.hour_of_day < 5 {
            risk += 10;
        }
        // International
        if features.is_international {
            risk += 15;
        }
        // New merchant
        if features.new_merchant {
            risk += 10;
        }
        // Geographic anomaly
        if features.distance_from_last_km > 500 && features.time_since_last_sec < 3600 {
            risk += 30; // impossible travel
        }
        // Rapid succession
        if features.time_since_last_sec < 60 {
            risk += 20;
        }

        if risk > 60 {
            self.fraud_flags = self.fraud_flags.saturating_add(1);
            FraudRisk::High
        } else if risk > 35 {
            FraudRisk::Medium
        } else if risk > 15 {
            FraudRisk::Low
        } else {
            FraudRisk::None
        }
    }

    fn categorize_merchant(&self, merchant_hash: u64) -> MerchantCategory {
        // Simple hash-based categorization (real impl would use NLP on merchant name)
        match merchant_hash % 10 {
            0 => MerchantCategory::Groceries,
            1 => MerchantCategory::Dining,
            2 => MerchantCategory::Transportation,
            3 => MerchantCategory::Entertainment,
            4 => MerchantCategory::Shopping,
            5 => MerchantCategory::Healthcare,
            6 => MerchantCategory::Utilities,
            7 => MerchantCategory::Subscription,
            8 => MerchantCategory::Travel,
            _ => MerchantCategory::Other,
        }
    }

    fn budget_remaining_cents(&self) -> u64 {
        self.monthly_budget_cents
            .saturating_sub(self.monthly_spent_cents)
    }

    fn projected_monthly_spend(&self) -> u64 {
        // Simple linear projection from current spend
        // Assuming 30-day month, if we're day N, project = spent * 30 / N
        let days_elapsed = 15u64; // would be calculated from date
        if days_elapsed == 0 {
            return self.monthly_spent_cents;
        }
        self.monthly_spent_cents * 30 / days_elapsed
    }
}

pub fn init() {
    let mut engine = AI_WALLET.lock();
    *engine = Some(AiWalletEngine::new());
    serial_println!("    AI wallet: fraud detection, spending analytics ready");
}
