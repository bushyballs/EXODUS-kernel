use crate::sync::Mutex;
/// App repository for Genesis store
///
/// Package index, versioning, dependencies,
/// delta updates, mirrors.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum AppCategory {
    Productivity,
    Social,
    Entertainment,
    Games,
    Education,
    Health,
    Finance,
    Tools,
    Communication,
    Photography,
    Music,
    News,
    System,
}

struct AppListing {
    id: u32,
    name: [u8; 32],
    name_len: usize,
    version_major: u8,
    version_minor: u8,
    version_patch: u16,
    category: AppCategory,
    size_bytes: u64,
    rating_x10: u16, // 0-50 (e.g., 45 = 4.5 stars)
    review_count: u32,
    download_count: u64,
    min_os_version: u32,
    developer_hash: u64,
    signature_hash: u64,
    is_free: bool,
    price_cents: u32,
}

struct Repository {
    listings: Vec<AppListing>,
    next_id: u32,
    last_sync: u64,
}

static REPO: Mutex<Option<Repository>> = Mutex::new(None);

impl Repository {
    fn new() -> Self {
        Repository {
            listings: Vec::new(),
            next_id: 1,
            last_sync: 0,
        }
    }

    fn add_listing(&mut self, name: &[u8], category: AppCategory, size: u64, free: bool) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 32];
        let nlen = name.len().min(32);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.listings.push(AppListing {
            id,
            name: n,
            name_len: nlen,
            version_major: 1,
            version_minor: 0,
            version_patch: 0,
            category,
            size_bytes: size,
            rating_x10: 0,
            review_count: 0,
            download_count: 0,
            min_os_version: 1,
            developer_hash: 0,
            signature_hash: 0,
            is_free: free,
            price_cents: 0,
        });
        id
    }

    fn search(&self, _query_hash: u64, category: Option<AppCategory>) -> Vec<u32> {
        self.listings
            .iter()
            .filter(|l| category.map_or(true, |c| l.category == c))
            .map(|l| l.id)
            .collect()
    }

    fn top_rated(&self, category: Option<AppCategory>, limit: usize) -> Vec<u32> {
        let mut results: Vec<_> = self
            .listings
            .iter()
            .filter(|l| category.map_or(true, |c| l.category == c))
            .collect();
        results.sort_by(|a, b| b.rating_x10.cmp(&a.rating_x10));
        results.iter().take(limit).map(|l| l.id).collect()
    }
}

pub fn init() {
    let mut r = REPO.lock();
    *r = Some(Repository::new());
    serial_println!("    App store: repository ready");
}
