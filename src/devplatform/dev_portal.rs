/// Developer portal / console for Genesis
///
/// Manages app publishing, developer accounts, analytics,
/// crash reports, pricing, beta tracks, and production promotion.
/// This is the backend that powers the developer-facing console
/// where app authors manage their published applications.
///
/// All monetary values and ratings use Q16 fixed-point (i32).
///
/// Original implementation for Hoags OS. No external crates.
use crate::sync::Mutex;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of developer accounts
const MAX_DEVELOPERS: usize = 1024;

/// Maximum number of app listings
const MAX_APP_LISTINGS: usize = 4096;

/// Maximum number of crash reports stored per app
const MAX_CRASH_REPORTS_PER_APP: usize = 256;

/// Maximum number of review responses per app
const MAX_REVIEW_RESPONSES: usize = 128;

/// Maximum number of beta tracks per app
const MAX_BETA_TRACKS: usize = 4;

/// Q16 fixed-point: 5.0 (default rating)
const DEFAULT_RATING_Q16: i32 = 5 << 16;

/// Q16 fixed-point: 0.0
const ZERO_Q16: i32 = 0;

/// App review status: pending review
const REVIEW_PENDING: u8 = 0;

/// App review status: approved
const REVIEW_APPROVED: u8 = 1;

/// App review status: rejected
const REVIEW_REJECTED: u8 = 2;

/// Pricing model: free
const PRICING_FREE: u8 = 0;

/// Pricing model: one-time purchase
const PRICING_PAID: u8 = 1;

/// Pricing model: subscription
const PRICING_SUBSCRIPTION: u8 = 2;

/// Pricing model: freemium (free with in-app purchases)
const PRICING_FREEMIUM: u8 = 3;

// ---------------------------------------------------------------------------
// DeveloperAccount
// ---------------------------------------------------------------------------

/// A registered developer account
#[derive(Debug, Clone)]
pub struct DeveloperAccount {
    /// Unique developer id
    pub id: u32,
    /// Hash of the developer's display name
    pub name_hash: u64,
    /// Hash of the developer's email address
    pub email_hash: u64,
    /// Whether the developer has completed identity verification
    pub verified: bool,
    /// Number of apps this developer has published
    pub apps_published: u32,
    /// Total download count across all apps
    pub total_downloads: u64,
    /// Account creation timestamp (kernel ticks)
    pub created_at: u64,
    /// Whether the account is active (not suspended)
    pub active: bool,
}

// ---------------------------------------------------------------------------
// AppListing
// ---------------------------------------------------------------------------

/// A published app listing in the developer portal
#[derive(Debug, Clone)]
pub struct AppListing {
    /// Unique listing id
    pub id: u32,
    /// Hash of the app name
    pub name_hash: u64,
    /// Hash of the developer's display name
    pub developer_hash: u64,
    /// Developer account id (foreign key)
    pub developer_id: u32,
    /// Current version string encoded as (major << 16 | minor << 8 | patch)
    pub version: u32,
    /// Total download count
    pub downloads: u64,
    /// Average user rating (Q16 fixed-point, range 0.0 - 5.0)
    pub rating: i32,
    /// Number of ratings received
    pub rating_count: u32,
    /// Hash of the category name
    pub category_hash: u64,
    /// Whether the app is currently published (visible to users)
    pub published: bool,
    /// Whether the app has passed review and been approved
    pub approved: bool,
    /// Review status (0=pending, 1=approved, 2=rejected)
    pub review_status: u8,
    /// Pricing model
    pub pricing_model: u8,
    /// Price in Q16 fixed-point (e.g. 2.99 => 2_99 << 16 is wrong; 2.99 * 65536)
    pub price_q16: i32,
    /// Hash of the app description
    pub description_hash: u64,
    /// Hash of the privacy policy URL
    pub privacy_policy_hash: u64,
    /// Minimum OS version required
    pub min_os_version: u32,
    /// Last update timestamp
    pub updated_at: u64,
}

// ---------------------------------------------------------------------------
// CrashReport
// ---------------------------------------------------------------------------

/// A crash report submitted by the system
#[derive(Debug, Clone)]
pub struct CrashReport {
    /// Unique crash report id
    pub id: u32,
    /// App listing id this crash belongs to
    pub app_id: u32,
    /// App version at the time of crash
    pub app_version: u32,
    /// Hash of the crash stack trace
    pub stack_hash: u64,
    /// Hash of the error message
    pub error_hash: u64,
    /// Number of occurrences of this crash
    pub occurrence_count: u32,
    /// First seen timestamp
    pub first_seen: u64,
    /// Last seen timestamp
    pub last_seen: u64,
    /// Whether the developer has acknowledged this crash
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// ReviewResponse
// ---------------------------------------------------------------------------

/// A developer's response to a user review
#[derive(Debug, Clone)]
pub struct ReviewResponse {
    /// Review id being responded to
    pub review_id: u32,
    /// App listing id
    pub app_id: u32,
    /// Hash of the response text
    pub response_hash: u64,
    /// Timestamp of the response
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// BetaTrack
// ---------------------------------------------------------------------------

/// A beta testing track for staged rollouts
#[derive(Debug, Clone)]
pub struct BetaTrack {
    /// Track name hash (e.g. "internal", "closed_beta", "open_beta")
    pub name_hash: u64,
    /// App listing id
    pub app_id: u32,
    /// Version deployed to this track
    pub version: u32,
    /// Number of testers enrolled
    pub tester_count: u32,
    /// Maximum testers allowed
    pub max_testers: u32,
    /// Whether the track is active
    pub active: bool,
    /// Rollout percentage (0-100 as Q16 fixed-point)
    pub rollout_pct_q16: i32,
}

// ---------------------------------------------------------------------------
// Analytics
// ---------------------------------------------------------------------------

/// Analytics data for an app listing
#[derive(Debug, Clone)]
pub struct AppAnalytics {
    /// App listing id
    pub app_id: u32,
    /// Downloads in the last 24 hours
    pub daily_downloads: u32,
    /// Downloads in the last 7 days
    pub weekly_downloads: u32,
    /// Downloads in the last 30 days
    pub monthly_downloads: u32,
    /// Daily active users
    pub daily_active_users: u32,
    /// Average session length in seconds (Q16 fixed-point)
    pub avg_session_q16: i32,
    /// Crash rate per 1000 sessions (Q16 fixed-point)
    pub crash_rate_q16: i32,
    /// Uninstall rate percentage (Q16 fixed-point)
    pub uninstall_rate_q16: i32,
    /// Average rating (Q16 fixed-point)
    pub avg_rating_q16: i32,
    /// Total revenue in cents (Q16 fixed-point)
    pub revenue_q16: i32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DEV_PORTAL: Mutex<Option<PortalState>> = Mutex::new(None);

struct PortalState {
    developers: Vec<DeveloperAccount>,
    listings: Vec<AppListing>,
    crash_reports: Vec<CrashReport>,
    review_responses: Vec<ReviewResponse>,
    beta_tracks: Vec<BetaTrack>,
    next_dev_id: u32,
    next_listing_id: u32,
    next_crash_id: u32,
    initialized: bool,
}

impl PortalState {
    fn new() -> Self {
        Self {
            developers: Vec::new(),
            listings: Vec::new(),
            crash_reports: Vec::new(),
            review_responses: Vec::new(),
            beta_tracks: Vec::new(),
            next_dev_id: 1,
            next_listing_id: 1,
            next_crash_id: 1,
            initialized: true,
        }
    }
}

// ---------------------------------------------------------------------------
// DevPortal — public API
// ---------------------------------------------------------------------------

/// Developer portal management API
pub struct DevPortal;

impl DevPortal {
    /// Register a new developer account
    ///
    /// Returns the developer id, or 0 on failure.
    pub fn register_developer(name_hash: u64, email_hash: u64) -> u32 {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        if state.developers.len() >= MAX_DEVELOPERS {
            serial_println!("[portal] developer limit reached");
            return 0;
        }

        // Check for duplicate email
        for dev in &state.developers {
            if dev.email_hash == email_hash {
                serial_println!("[portal] duplicate email hash");
                return 0;
            }
        }

        let id = state.next_dev_id;
        state.next_dev_id = state.next_dev_id.saturating_add(1);

        let account = DeveloperAccount {
            id,
            name_hash,
            email_hash,
            verified: false,
            apps_published: 0,
            total_downloads: 0,
            created_at: 0,
            active: true,
        };

        state.developers.push(account);
        serial_println!("[portal] registered developer id={}", id);
        id
    }

    /// Submit a new app for review
    ///
    /// Returns the listing id, or 0 on failure.
    pub fn submit_app(
        developer_id: u32,
        name_hash: u64,
        version: u32,
        category_hash: u64,
        description_hash: u64,
    ) -> u32 {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        if state.listings.len() >= MAX_APP_LISTINGS {
            serial_println!("[portal] app listing limit reached");
            return 0;
        }

        // Verify developer exists and is active
        let dev = match state.developers.iter().find(|d| d.id == developer_id) {
            Some(d) => d,
            None => {
                serial_println!("[portal] unknown developer id={}", developer_id);
                return 0;
            }
        };

        if !dev.active {
            serial_println!("[portal] developer {} is suspended", developer_id);
            return 0;
        }

        let listing_id = state.next_listing_id;
        state.next_listing_id = state.next_listing_id.saturating_add(1);

        let listing = AppListing {
            id: listing_id,
            name_hash,
            developer_hash: dev.name_hash,
            developer_id,
            version,
            downloads: 0,
            rating: DEFAULT_RATING_Q16,
            rating_count: 0,
            category_hash,
            published: false,
            approved: false,
            review_status: REVIEW_PENDING,
            pricing_model: PRICING_FREE,
            price_q16: ZERO_Q16,
            description_hash,
            privacy_policy_hash: 0,
            min_os_version: 0x00010000,
            updated_at: 0,
        };

        state.listings.push(listing);

        // Increment developer's published count
        if let Some(d) = state.developers.iter_mut().find(|d| d.id == developer_id) {
            d.apps_published = d.apps_published.saturating_add(1);
        }

        serial_println!(
            "[portal] app submitted id={} by dev={}",
            listing_id,
            developer_id
        );
        listing_id
    }

    /// Update an existing app listing (new version)
    pub fn update_app(
        listing_id: u32,
        developer_id: u32,
        new_version: u32,
        description_hash: u64,
    ) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let listing = match state.listings.iter_mut().find(|l| l.id == listing_id) {
            Some(l) => l,
            None => return false,
        };

        if listing.developer_id != developer_id {
            serial_println!(
                "[portal] permission denied: dev {} != owner {}",
                developer_id,
                listing.developer_id
            );
            return false;
        }

        if new_version <= listing.version {
            serial_println!(
                "[portal] version must increase (current=0x{:06X})",
                listing.version
            );
            return false;
        }

        listing.version = new_version;
        listing.description_hash = description_hash;
        listing.review_status = REVIEW_PENDING;
        listing.approved = false;
        listing.published = false;
        listing.updated_at = listing.updated_at.saturating_add(1); // increment as pseudo-timestamp

        serial_println!(
            "[portal] app {} updated to version 0x{:06X}",
            listing_id,
            new_version
        );
        true
    }

    /// Get analytics for an app listing
    pub fn get_analytics(listing_id: u32) -> Option<AppAnalytics> {
        let guard = DEV_PORTAL.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return None,
        };

        let listing = match state.listings.iter().find(|l| l.id == listing_id) {
            Some(l) => l,
            None => return None,
        };

        // Generate synthetic analytics based on listing data
        let daily = (listing.downloads / 30) as u32;
        let weekly = (listing.downloads / 4) as u32;
        let monthly = listing.downloads as u32;

        let crash_count = state
            .crash_reports
            .iter()
            .filter(|c| c.app_id == listing_id)
            .count() as i32;

        // Crash rate: crashes per 1000 sessions (Q16)
        let crash_rate_q16 = if monthly > 0 {
            ((crash_count * 1000) << 16) / (monthly as i32).max(1)
        } else {
            ZERO_Q16
        };

        Some(AppAnalytics {
            app_id: listing_id,
            daily_downloads: daily,
            weekly_downloads: weekly,
            monthly_downloads: monthly,
            daily_active_users: daily * 3,
            avg_session_q16: 300 << 16, // 300 seconds average
            crash_rate_q16,
            uninstall_rate_q16: 5 << 14, // ~1.25% (5 / 4 in Q16)
            avg_rating_q16: listing.rating,
            revenue_q16: listing.price_q16,
        })
    }

    /// Respond to a user review
    pub fn respond_to_review(
        developer_id: u32,
        app_id: u32,
        review_id: u32,
        response_hash: u64,
    ) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        // Verify the developer owns this app
        let listing = match state.listings.iter().find(|l| l.id == app_id) {
            Some(l) => l,
            None => return false,
        };

        if listing.developer_id != developer_id {
            return false;
        }

        if state.review_responses.len() >= MAX_REVIEW_RESPONSES {
            // Remove oldest response
            state.review_responses.remove(0);
        }

        let response = ReviewResponse {
            review_id,
            app_id,
            response_hash,
            timestamp: 0,
        };

        state.review_responses.push(response);
        serial_println!(
            "[portal] review response posted for app {} review {}",
            app_id,
            review_id
        );
        true
    }

    /// Get crash reports for an app
    pub fn get_crash_reports(listing_id: u32) -> Vec<CrashReport> {
        let guard = DEV_PORTAL.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return Vec::new(),
        };

        state
            .crash_reports
            .iter()
            .filter(|c| c.app_id == listing_id)
            .cloned()
            .collect()
    }

    /// Submit a crash report for an app
    pub fn submit_crash_report(
        app_id: u32,
        app_version: u32,
        stack_hash: u64,
        error_hash: u64,
    ) -> u32 {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return 0,
        };

        // Check if this crash (by stack hash) already exists for this app
        if let Some(existing) = state
            .crash_reports
            .iter_mut()
            .find(|c| c.app_id == app_id && c.stack_hash == stack_hash)
        {
            existing.occurrence_count = existing.occurrence_count.saturating_add(1);
            existing.last_seen = existing.last_seen.saturating_add(1);
            return existing.id;
        }

        // Enforce per-app limit
        let app_crashes = state
            .crash_reports
            .iter()
            .filter(|c| c.app_id == app_id)
            .count();
        if app_crashes >= MAX_CRASH_REPORTS_PER_APP {
            serial_println!("[portal] crash report limit for app {}", app_id);
            return 0;
        }

        let id = state.next_crash_id;
        state.next_crash_id = state.next_crash_id.saturating_add(1);

        let report = CrashReport {
            id,
            app_id,
            app_version,
            stack_hash,
            error_hash,
            occurrence_count: 1,
            first_seen: 0,
            last_seen: 0,
            acknowledged: false,
        };

        state.crash_reports.push(report);
        serial_println!("[portal] crash report {} for app {}", id, app_id);
        id
    }

    /// Set the pricing model and price for an app
    pub fn set_pricing(listing_id: u32, developer_id: u32, model: u8, price_q16: i32) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let listing = match state.listings.iter_mut().find(|l| l.id == listing_id) {
            Some(l) => l,
            None => return false,
        };

        if listing.developer_id != developer_id {
            return false;
        }

        // Validate pricing model
        if model > PRICING_FREEMIUM {
            serial_println!("[portal] invalid pricing model {}", model);
            return false;
        }

        // Free model must have zero price
        if model == PRICING_FREE && price_q16 != ZERO_Q16 {
            serial_println!("[portal] free apps cannot have a price");
            return false;
        }

        listing.pricing_model = model;
        listing.price_q16 = price_q16;

        serial_println!(
            "[portal] app {} pricing set: model={} price_q16={}",
            listing_id,
            model,
            price_q16
        );
        true
    }

    /// Create a beta testing track for an app
    pub fn create_beta_track(
        listing_id: u32,
        developer_id: u32,
        track_name_hash: u64,
        version: u32,
        max_testers: u32,
    ) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        // Verify ownership
        let listing = match state.listings.iter().find(|l| l.id == listing_id) {
            Some(l) => l,
            None => return false,
        };

        if listing.developer_id != developer_id {
            return false;
        }

        // Check track limit for this app
        let track_count = state
            .beta_tracks
            .iter()
            .filter(|t| t.app_id == listing_id)
            .count();
        if track_count >= MAX_BETA_TRACKS {
            serial_println!("[portal] beta track limit reached for app {}", listing_id);
            return false;
        }

        // Check for duplicate track name
        let duplicate = state
            .beta_tracks
            .iter()
            .any(|t| t.app_id == listing_id && t.name_hash == track_name_hash);
        if duplicate {
            serial_println!("[portal] duplicate beta track name");
            return false;
        }

        let track = BetaTrack {
            name_hash: track_name_hash,
            app_id: listing_id,
            version,
            tester_count: 0,
            max_testers,
            active: true,
            rollout_pct_q16: 100 << 16, // 100% to enrolled testers
        };

        state.beta_tracks.push(track);
        serial_println!(
            "[portal] beta track created for app {} (max {} testers)",
            listing_id,
            max_testers
        );
        true
    }

    /// Promote a beta track version to production
    pub fn promote_to_production(listing_id: u32, developer_id: u32, track_name_hash: u64) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        // Verify ownership
        let listing_dev = match state.listings.iter().find(|l| l.id == listing_id) {
            Some(l) => l.developer_id,
            None => return false,
        };

        if listing_dev != developer_id {
            return false;
        }

        // Find the beta track
        let track_version = match state
            .beta_tracks
            .iter()
            .find(|t| t.app_id == listing_id && t.name_hash == track_name_hash && t.active)
        {
            Some(t) => t.version,
            None => {
                serial_println!("[portal] beta track not found for app {}", listing_id);
                return false;
            }
        };

        // Promote: update the listing version and mark as published
        if let Some(listing) = state.listings.iter_mut().find(|l| l.id == listing_id) {
            listing.version = track_version;
            listing.review_status = REVIEW_APPROVED;
            listing.approved = true;
            listing.published = true;
            listing.updated_at += 1;
        }

        // Deactivate the beta track
        if let Some(track) = state
            .beta_tracks
            .iter_mut()
            .find(|t| t.app_id == listing_id && t.name_hash == track_name_hash)
        {
            track.active = false;
        }

        serial_println!(
            "[portal] app {} promoted to production (version 0x{:06X})",
            listing_id,
            track_version
        );
        true
    }

    /// Approve an app listing (admin action)
    pub fn approve_app(listing_id: u32) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let listing = match state.listings.iter_mut().find(|l| l.id == listing_id) {
            Some(l) => l,
            None => return false,
        };

        listing.review_status = REVIEW_APPROVED;
        listing.approved = true;
        listing.published = true;
        serial_println!("[portal] app {} approved and published", listing_id);
        true
    }

    /// Get total number of app listings
    pub fn listing_count() -> usize {
        let guard = DEV_PORTAL.lock();
        match guard.as_ref() {
            Some(s) => s.listings.len(),
            None => 0,
        }
    }

    /// Get total number of developer accounts
    pub fn developer_count() -> usize {
        let guard = DEV_PORTAL.lock();
        match guard.as_ref() {
            Some(s) => s.developers.len(),
            None => 0,
        }
    }

    /// Get a developer account by id
    pub fn get_developer(developer_id: u32) -> Option<DeveloperAccount> {
        let guard = DEV_PORTAL.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return None,
        };
        state
            .developers
            .iter()
            .find(|d| d.id == developer_id)
            .cloned()
    }

    /// Get an app listing by id
    pub fn get_listing(listing_id: u32) -> Option<AppListing> {
        let guard = DEV_PORTAL.lock();
        let state = match guard.as_ref() {
            Some(s) => s,
            None => return None,
        };
        state.listings.iter().find(|l| l.id == listing_id).cloned()
    }

    /// Verify a developer account (admin action)
    pub fn verify_developer(developer_id: u32) -> bool {
        let mut guard = DEV_PORTAL.lock();
        let state = match guard.as_mut() {
            Some(s) => s,
            None => return false,
        };

        match state.developers.iter_mut().find(|d| d.id == developer_id) {
            Some(dev) => {
                dev.verified = true;
                serial_println!("[portal] developer {} verified", developer_id);
                true
            }
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the developer portal subsystem
pub fn init() {
    let mut guard = DEV_PORTAL.lock();
    *guard = Some(PortalState::new());
    serial_println!("[portal] developer portal initialized");
}
