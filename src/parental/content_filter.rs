use crate::sync::Mutex;
/// Content filtering for Genesis parental controls
///
/// Web content filtering, SafeSearch enforcement,
/// explicit content blocking, age-rating enforcement.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ContentRating {
    Everyone,
    Age7,
    Age12,
    Age16,
    Age18,
    Unrated,
}

#[derive(Clone, Copy, PartialEq)]
pub enum FilterCategory {
    Violence,
    Adult,
    Gambling,
    Drugs,
    Hate,
    Malware,
    Social,
    Streaming,
    Gaming,
}

struct FilterRule {
    category: FilterCategory,
    blocked: bool,
    age_minimum: u8,
}

struct BlockedDomain {
    domain_hash: u64,
    category: FilterCategory,
}

struct ContentFilterEngine {
    rules: Vec<FilterRule>,
    blocked_domains: Vec<BlockedDomain>,
    safe_search_enforced: bool,
    max_content_rating: ContentRating,
    blocked_count: u32,
    allowed_count: u32,
}

static CONTENT_FILTER: Mutex<Option<ContentFilterEngine>> = Mutex::new(None);

impl ContentFilterEngine {
    fn new() -> Self {
        let mut engine = ContentFilterEngine {
            rules: Vec::new(),
            blocked_domains: Vec::new(),
            safe_search_enforced: true,
            max_content_rating: ContentRating::Everyone,
            blocked_count: 0,
            allowed_count: 0,
        };
        // Default rules
        engine.rules.push(FilterRule {
            category: FilterCategory::Adult,
            blocked: true,
            age_minimum: 18,
        });
        engine.rules.push(FilterRule {
            category: FilterCategory::Gambling,
            blocked: true,
            age_minimum: 18,
        });
        engine.rules.push(FilterRule {
            category: FilterCategory::Violence,
            blocked: true,
            age_minimum: 16,
        });
        engine.rules.push(FilterRule {
            category: FilterCategory::Drugs,
            blocked: true,
            age_minimum: 16,
        });
        engine.rules.push(FilterRule {
            category: FilterCategory::Hate,
            blocked: true,
            age_minimum: 18,
        });
        engine.rules.push(FilterRule {
            category: FilterCategory::Malware,
            blocked: true,
            age_minimum: 0,
        });
        engine
    }

    fn is_category_blocked(&self, category: FilterCategory, user_age: u8) -> bool {
        self.rules
            .iter()
            .find(|r| r.category == category)
            .map(|r| r.blocked && user_age < r.age_minimum)
            .unwrap_or(false)
    }

    fn check_domain(&mut self, domain_hash: u64, user_age: u8) -> bool {
        if let Some(bd) = self
            .blocked_domains
            .iter()
            .find(|d| d.domain_hash == domain_hash)
        {
            if self.is_category_blocked(bd.category, user_age) {
                self.blocked_count = self.blocked_count.saturating_add(1);
                return false; // blocked
            }
        }
        self.allowed_count = self.allowed_count.saturating_add(1);
        true // allowed
    }

    fn is_rating_allowed(&self, rating: ContentRating) -> bool {
        let max = match self.max_content_rating {
            ContentRating::Everyone => 0,
            ContentRating::Age7 => 1,
            ContentRating::Age12 => 2,
            ContentRating::Age16 => 3,
            ContentRating::Age18 => 4,
            ContentRating::Unrated => 5,
        };
        let target = match rating {
            ContentRating::Everyone => 0,
            ContentRating::Age7 => 1,
            ContentRating::Age12 => 2,
            ContentRating::Age16 => 3,
            ContentRating::Age18 => 4,
            ContentRating::Unrated => 5,
        };
        target <= max
    }
}

pub fn init() {
    let mut f = CONTENT_FILTER.lock();
    *f = Some(ContentFilterEngine::new());
    serial_println!("    Parental: content filtering (web, apps, ratings) ready");
}
