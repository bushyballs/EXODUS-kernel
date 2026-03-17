/// In-app billing for Genesis store
///
/// Purchase flow, subscription management, receipt
/// validation, refund processing, price tiers,
/// promotional pricing, payment ledger.
///
/// Original implementation for Hoags OS.

use alloc::vec::Vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers (i32 with 16 fractional bits, NO floats)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum PurchaseType {
    OneTime,
    Subscription,
    Consumable,
    InAppUpgrade,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PurchaseStatus {
    Pending,
    Completed,
    Failed,
    Refunded,
    Cancelled,
    Disputed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SubscriptionPeriod {
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
    Lifetime,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RefundReason {
    NotAsDescribed,
    TechnicalIssue,
    AccidentalPurchase,
    SubscriptionCancel,
    Fraud,
    Other,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PriceTier {
    Free,
    Tier1,   // $0.99
    Tier2,   // $1.99
    Tier3,   // $2.99
    Tier4,   // $4.99
    Tier5,   // $9.99
    Tier6,   // $14.99
    Tier7,   // $19.99
    Tier8,   // $29.99
    Tier9,   // $49.99
    Tier10,  // $99.99
    Custom,
}

struct Purchase {
    id: u32,
    user_hash: u64,
    listing_id: u32,
    product_id: u32,
    purchase_type: PurchaseType,
    status: PurchaseStatus,
    price_cents: u32,
    currency_code: u16,       // ISO 4217 numeric code
    timestamp: u64,
    receipt_hash: u64,
    receipt_valid: bool,
    refund_reason: Option<RefundReason>,
    refund_timestamp: u64,
}

struct Subscription {
    id: u32,
    user_hash: u64,
    listing_id: u32,
    product_id: u32,
    period: SubscriptionPeriod,
    price_cents: u32,
    start_time: u64,
    next_renewal: u64,
    last_payment: u64,
    auto_renew: bool,
    active: bool,
    trial_end: u64,
    cancel_time: u64,
    grace_period_end: u64,
    payments_made: u32,
}

struct PriceTierEntry {
    tier: PriceTier,
    price_cents: u32,
    display_name: [u8; 16],
    name_len: usize,
}

struct PromoCode {
    code_hash: u64,
    listing_id: u32,
    discount_percent: u8,     // 0-100
    valid_from: u64,
    valid_until: u64,
    max_uses: u32,
    uses: u32,
    active: bool,
}

struct ReceiptValidation {
    receipt_hash: u64,
    purchase_id: u32,
    signature_hash: u64,
    valid: bool,
    validation_time: u64,
}

struct BillingLedger {
    total_revenue_cents: u64,
    total_refunds_cents: u64,
    total_purchases: u32,
    total_subscriptions: u32,
    active_subscriptions: u32,
}

struct BillingEngine {
    purchases: Vec<Purchase>,
    subscriptions: Vec<Subscription>,
    price_tiers: Vec<PriceTierEntry>,
    promo_codes: Vec<PromoCode>,
    validations: Vec<ReceiptValidation>,
    ledger: BillingLedger,
    next_purchase_id: u32,
    next_subscription_id: u32,
    platform_fee_pct_q16: i32,   // Q16 platform fee (e.g., 30%)
}

static BILLING: Mutex<Option<BillingEngine>> = Mutex::new(None);

impl BillingEngine {
    fn new() -> Self {
        let mut engine = BillingEngine {
            purchases: Vec::new(),
            subscriptions: Vec::new(),
            price_tiers: Vec::new(),
            promo_codes: Vec::new(),
            validations: Vec::new(),
            ledger: BillingLedger {
                total_revenue_cents: 0,
                total_refunds_cents: 0,
                total_purchases: 0,
                total_subscriptions: 0,
                active_subscriptions: 0,
            },
            next_purchase_id: 1,
            next_subscription_id: 1,
            platform_fee_pct_q16: q16_div(30, 100),  // 30%
        };
        engine.init_price_tiers();
        engine
    }

    fn init_price_tiers(&mut self) {
        let tiers = [
            (PriceTier::Free,   0u32,     b"Free\0\0\0\0\0\0\0\0\0\0\0\0" as &[u8]),
            (PriceTier::Tier1,  99,       b"$0.99\0\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier2,  199,      b"$1.99\0\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier3,  299,      b"$2.99\0\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier4,  499,      b"$4.99\0\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier5,  999,      b"$9.99\0\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier6,  1499,     b"$14.99\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier7,  1999,     b"$19.99\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier8,  2999,     b"$29.99\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier9,  4999,     b"$49.99\0\0\0\0\0\0\0\0\0\0"),
            (PriceTier::Tier10, 9999,     b"$99.99\0\0\0\0\0\0\0\0\0\0"),
        ];
        for (tier, price, name_bytes) in &tiers {
            let mut dn = [0u8; 16];
            let nlen = name_bytes.len().min(16);
            dn[..nlen].copy_from_slice(&name_bytes[..nlen]);
            self.price_tiers.push(PriceTierEntry {
                tier: *tier,
                price_cents: *price,
                display_name: dn,
                name_len: nlen,
            });
        }
    }

    fn tier_price(&self, tier: PriceTier) -> u32 {
        self.price_tiers.iter()
            .find(|t| t.tier == tier)
            .map(|t| t.price_cents)
            .unwrap_or(0)
    }

    fn begin_purchase(
        &mut self,
        user_hash: u64,
        listing_id: u32,
        product_id: u32,
        purchase_type: PurchaseType,
        price_cents: u32,
        timestamp: u64,
    ) -> u32 {
        let id = self.next_purchase_id;
        self.next_purchase_id = self.next_purchase_id.saturating_add(1);

        self.purchases.push(Purchase {
            id,
            user_hash,
            listing_id,
            product_id,
            purchase_type,
            status: PurchaseStatus::Pending,
            price_cents,
            currency_code: 840,  // USD
            timestamp,
            receipt_hash: 0,
            receipt_valid: false,
            refund_reason: None,
            refund_timestamp: 0,
        });
        id
    }

    fn complete_purchase(&mut self, purchase_id: u32, receipt_hash: u64, timestamp: u64) -> bool {
        if let Some(p) = self.purchases.iter_mut().find(|p| p.id == purchase_id) {
            if p.status != PurchaseStatus::Pending { return false; }
            p.status = PurchaseStatus::Completed;
            p.receipt_hash = receipt_hash;
            p.receipt_valid = true;

            self.ledger.total_revenue_cents = self.ledger.total_revenue_cents.saturating_add(p.price_cents as u64);
            self.ledger.total_purchases = self.ledger.total_purchases.saturating_add(1);

            // Validate receipt
            self.validations.push(ReceiptValidation {
                receipt_hash,
                purchase_id,
                signature_hash: receipt_hash ^ 0xABCD_EF01_2345_6789,
                valid: true,
                validation_time: timestamp,
            });
            return true;
        }
        false
    }

    fn fail_purchase(&mut self, purchase_id: u32) -> bool {
        if let Some(p) = self.purchases.iter_mut().find(|p| p.id == purchase_id) {
            if p.status != PurchaseStatus::Pending { return false; }
            p.status = PurchaseStatus::Failed;
            return true;
        }
        false
    }

    fn request_refund(&mut self, purchase_id: u32, reason: RefundReason, timestamp: u64) -> bool {
        if let Some(p) = self.purchases.iter_mut().find(|p| p.id == purchase_id) {
            if p.status != PurchaseStatus::Completed { return false; }
            p.status = PurchaseStatus::Refunded;
            p.refund_reason = Some(reason);
            p.refund_timestamp = timestamp;

            self.ledger.total_refunds_cents = self.ledger.total_refunds_cents.saturating_add(p.price_cents as u64);
            return true;
        }
        false
    }

    fn start_subscription(
        &mut self,
        user_hash: u64,
        listing_id: u32,
        product_id: u32,
        period: SubscriptionPeriod,
        price_cents: u32,
        timestamp: u64,
        trial_days: u32,
    ) -> u32 {
        let id = self.next_subscription_id;
        self.next_subscription_id = self.next_subscription_id.saturating_add(1);

        let trial_end = if trial_days > 0 {
            timestamp + (trial_days as u64) * 86400
        } else {
            0
        };

        let renewal_interval = match period {
            SubscriptionPeriod::Weekly => 7 * 86400,
            SubscriptionPeriod::Monthly => 30 * 86400,
            SubscriptionPeriod::Quarterly => 90 * 86400,
            SubscriptionPeriod::Yearly => 365 * 86400,
            SubscriptionPeriod::Lifetime => u64::MAX,
        };

        let next_renewal = if trial_days > 0 {
            trial_end
        } else {
            timestamp + renewal_interval
        };

        self.subscriptions.push(Subscription {
            id,
            user_hash,
            listing_id,
            product_id,
            period,
            price_cents,
            start_time: timestamp,
            next_renewal,
            last_payment: timestamp,
            auto_renew: true,
            active: true,
            trial_end,
            cancel_time: 0,
            grace_period_end: 0,
            payments_made: if trial_days > 0 { 0 } else { 1 },
        });

        self.ledger.total_subscriptions = self.ledger.total_subscriptions.saturating_add(1);
        self.ledger.active_subscriptions = self.ledger.active_subscriptions.saturating_add(1);
        id
    }

    fn cancel_subscription(&mut self, sub_id: u32, timestamp: u64) -> bool {
        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.id == sub_id) {
            if !sub.active { return false; }
            sub.auto_renew = false;
            sub.cancel_time = timestamp;
            // Subscription stays active until next_renewal (grace period)
            sub.grace_period_end = sub.next_renewal;
            return true;
        }
        false
    }

    fn deactivate_expired(&mut self, current_time: u64) -> u32 {
        let mut deactivated = 0u32;
        for sub in &mut self.subscriptions {
            if !sub.active { continue; }
            if !sub.auto_renew && current_time >= sub.grace_period_end && sub.grace_period_end > 0 {
                sub.active = false;
                deactivated += 1;
                if self.ledger.active_subscriptions > 0 {
                    self.ledger.active_subscriptions -= 1;
                }
            }
        }
        deactivated
    }

    fn renew_subscription(&mut self, sub_id: u32, timestamp: u64) -> bool {
        if let Some(sub) = self.subscriptions.iter_mut().find(|s| s.id == sub_id && s.active && s.auto_renew) {
            let renewal_interval = match sub.period {
                SubscriptionPeriod::Weekly => 7 * 86400,
                SubscriptionPeriod::Monthly => 30 * 86400,
                SubscriptionPeriod::Quarterly => 90 * 86400,
                SubscriptionPeriod::Yearly => 365 * 86400,
                SubscriptionPeriod::Lifetime => return false,
            };
            sub.last_payment = timestamp;
            sub.next_renewal = timestamp + renewal_interval;
            sub.payments_made = sub.payments_made.saturating_add(1);
            return true;
        }
        false
    }

    fn validate_receipt(&self, receipt_hash: u64) -> bool {
        self.validations.iter().any(|v| v.receipt_hash == receipt_hash && v.valid)
    }

    fn apply_promo(&mut self, code_hash: u64, listing_id: u32, price_cents: u32, timestamp: u64) -> u32 {
        if let Some(promo) = self.promo_codes.iter_mut().find(|p| {
            p.code_hash == code_hash
                && p.listing_id == listing_id
                && p.active
                && timestamp >= p.valid_from
                && timestamp <= p.valid_until
                && p.uses < p.max_uses
        }) {
            promo.uses += 1;
            let discount = (price_cents as u64 * promo.discount_percent as u64) / 100;
            return price_cents - discount as u32;
        }
        price_cents // no discount
    }

    fn add_promo_code(
        &mut self,
        code_hash: u64,
        listing_id: u32,
        discount_pct: u8,
        valid_from: u64,
        valid_until: u64,
        max_uses: u32,
    ) {
        self.promo_codes.push(PromoCode {
            code_hash,
            listing_id,
            discount_percent: discount_pct.min(100),
            valid_from,
            valid_until,
            max_uses,
            uses: 0,
            active: true,
        });
    }

    fn user_purchases(&self, user_hash: u64) -> Vec<u32> {
        self.purchases.iter()
            .filter(|p| p.user_hash == user_hash && p.status == PurchaseStatus::Completed)
            .map(|p| p.id)
            .collect()
    }

    fn user_active_subs(&self, user_hash: u64) -> Vec<u32> {
        self.subscriptions.iter()
            .filter(|s| s.user_hash == user_hash && s.active)
            .map(|s| s.id)
            .collect()
    }

    fn net_revenue_cents(&self) -> u64 {
        self.ledger.total_revenue_cents.saturating_sub(self.ledger.total_refunds_cents)
    }

    fn platform_cut_cents(&self, amount_cents: u32) -> u32 {
        let amount_q16 = q16_from_int(amount_cents as i32);
        let cut = q16_mul(amount_q16, self.platform_fee_pct_q16);
        (cut >> Q16_SHIFT) as u32
    }
}

pub fn init() {
    let mut b = BILLING.lock();
    *b = Some(BillingEngine::new());
    serial_println!("    App store: billing engine (purchases, subscriptions, receipts) ready");
}
