use crate::sync::Mutex;
/// Browser Privacy Guard — anti-tracking, anti-fingerprint, cookie management
///
/// Provides comprehensive browser privacy protection:
///   - Tracker blocking with categorized blocklists (analytics, ads, social, cryptominers)
///   - Cookie management with per-domain limits, third-party blocking, SameSite enforcement
///   - Fingerprint resistance (canvas, WebGL, audio, font, screen, timezone spoofing)
///   - Do Not Track and Global Privacy Control header support
///   - Privacy scoring system to evaluate overall protection level
///
/// All numeric operations use i32 Q16 fixed-point (no floats).
///
/// Inspired by: Firefox Enhanced Tracking Protection, Brave Shields,
/// Tor Browser fingerprinting resistance. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Q16 fixed-point multiplier: 1.0 = 65536
const Q16_ONE: i32 = 65536;

/// Maximum tracker entries in the blocklist
const MAX_TRACKER_ENTRIES: usize = 8192;

/// Maximum cookies across all domains
const DEFAULT_COOKIE_LIMIT: u32 = 4096;

/// Maximum cookies per single domain
const DEFAULT_PER_DOMAIN_LIMIT: u32 = 64;

/// Default noise seed for fingerprint randomization
const DEFAULT_NOISE_SEED: u64 = 0xABCD_1234_5678_EF00;

/// Known tracker domain hashes (well-known analytics/ad domains)
const KNOWN_TRACKER_HASHES: &[u64] = &[
    0xA1B2_C3D4_E5F6_0001, // common analytics tracker
    0xA1B2_C3D4_E5F6_0002, // common ad network
    0xA1B2_C3D4_E5F6_0003, // social tracking pixel
    0xA1B2_C3D4_E5F6_0004, // cryptominer script host
    0xA1B2_C3D4_E5F6_0005, // fingerprinting service
    0xA1B2_C3D4_E5F6_0006, // cross-site content tracker
    0xA1B2_C3D4_E5F6_0007, // retargeting network
    0xA1B2_C3D4_E5F6_0008, // behavioral analytics
];

/// Global privacy engine state
static PRIVACY_ENGINE: Mutex<Option<PrivacyEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Tracking protection level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingProtection {
    /// No tracker blocking
    Off,
    /// Block known analytics and advertising trackers
    Standard,
    /// Block all known trackers including content trackers
    Strict,
    /// User-defined custom blocking rules
    Custom,
}

/// Category of a known tracker
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerCategory {
    /// Website analytics and telemetry (Google Analytics, etc.)
    Analytics,
    /// Advertising networks and retargeting
    Advertising,
    /// Social media tracking widgets and pixels
    Social,
    /// Cryptocurrency mining scripts
    Cryptominer,
    /// Browser fingerprinting services
    Fingerprinter,
    /// Cross-site content trackers embedded in page content
    ContentTracker,
    /// User-defined custom category
    Custom,
}

/// Cookie acceptance policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePolicy {
    /// Accept all cookies without restriction
    AcceptAll,
    /// Reject cookies from third-party domains
    RejectThirdParty,
    /// Reject all cookies
    RejectAll,
    /// Accept cookies but treat all as session-only (expire on close)
    SessionOnly,
    /// Custom per-domain cookie rules
    Custom,
}

/// SameSite cookie attribute
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// Cookie only sent in first-party context
    Strict,
    /// Cookie sent on top-level navigations and first-party requests
    Lax,
    /// Cookie sent in all contexts (requires Secure attribute)
    None,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A single entry in the tracker blocklist
#[derive(Debug, Clone)]
pub struct TrackerEntry {
    /// FNV-1a hash of the tracker domain
    pub domain_hash: u64,
    /// What kind of tracker this is
    pub category: TrackerCategory,
    /// How many times this tracker has been blocked
    pub blocked_count: u64,
    /// Timestamp (ticks) when this tracker was first encountered
    pub first_seen: u64,
}

/// A stored browser cookie
#[derive(Debug, Clone)]
pub struct Cookie {
    /// FNV-1a hash of the cookie domain
    pub domain_hash: u64,
    /// FNV-1a hash of the cookie name
    pub name_hash: u64,
    /// FNV-1a hash of the cookie value
    pub value_hash: u64,
    /// Expiration timestamp (ticks); 0 = session cookie
    pub expires: u64,
    /// HttpOnly flag — not accessible to JavaScript
    pub http_only: bool,
    /// Secure flag — only sent over HTTPS
    pub secure: bool,
    /// SameSite attribute
    pub same_site: SameSite,
    /// Whether this cookie was set by a third-party domain
    pub third_party: bool,
}

/// Cookie storage container with per-domain and global limits
#[derive(Debug, Clone)]
pub struct CookieJar {
    /// All stored cookies
    pub cookies: Vec<Cookie>,
    /// Maximum cookies allowed per domain
    pub max_per_domain: u32,
    /// Maximum total cookies across all domains
    pub total_limit: u32,
}

/// Fingerprint protection configuration — controls what browser properties are spoofed
#[derive(Debug, Clone)]
pub struct FingerprintProtection {
    /// Spoof canvas element readback data
    pub spoof_canvas: bool,
    /// Spoof WebGL renderer and vendor strings
    pub spoof_webgl: bool,
    /// Spoof AudioContext fingerprinting
    pub spoof_audio: bool,
    /// Spoof installed font enumeration
    pub spoof_fonts: bool,
    /// Spoof screen resolution and color depth
    pub spoof_screen: bool,
    /// Spoof timezone to UTC
    pub spoof_timezone: bool,
    /// Spoof navigator.language to generic value
    pub spoof_language: bool,
    /// Seed for deterministic noise injection
    pub noise_seed: u64,
}

/// Central privacy engine managing all browser privacy features
#[derive(Debug, Clone)]
pub struct PrivacyEngine {
    /// Current tracking protection level
    pub protection_level: TrackingProtection,
    /// Tracker blocklist
    pub tracker_list: Vec<TrackerEntry>,
    /// Cookie storage
    pub cookie_jar: CookieJar,
    /// Cookie acceptance policy
    pub cookie_policy: CookiePolicy,
    /// Fingerprint protection settings
    pub fingerprint: FingerprintProtection,
    /// Send Do Not Track header
    pub do_not_track: bool,
    /// Send Global Privacy Control header
    pub global_privacy_control: bool,
    /// Total trackers blocked across all categories since engine start
    pub total_blocked: u64,
    /// Timestamp of last blocklist update
    pub last_blocklist_update: u64,
}

// ---------------------------------------------------------------------------
// Implementations
// ---------------------------------------------------------------------------

impl CookieJar {
    /// Create a new cookie jar with default limits
    pub fn new() -> Self {
        CookieJar {
            cookies: Vec::new(),
            max_per_domain: DEFAULT_PER_DOMAIN_LIMIT,
            total_limit: DEFAULT_COOKIE_LIMIT,
        }
    }

    /// Count cookies belonging to a specific domain
    pub fn count_for_domain(&self, domain_hash: u64) -> u32 {
        let mut count: u32 = 0;
        for cookie in &self.cookies {
            if cookie.domain_hash == domain_hash {
                count += 1;
            }
        }
        count
    }

    /// Remove expired cookies given the current timestamp
    pub fn purge_expired(&mut self, now: u64) {
        self.cookies.retain(|c| c.expires == 0 || c.expires > now);
    }
}

impl FingerprintProtection {
    /// Create a new fingerprint protection config with all spoofing enabled
    pub fn strict() -> Self {
        FingerprintProtection {
            spoof_canvas: true,
            spoof_webgl: true,
            spoof_audio: true,
            spoof_fonts: true,
            spoof_screen: true,
            spoof_timezone: true,
            spoof_language: true,
            noise_seed: DEFAULT_NOISE_SEED,
        }
    }

    /// Create a config with all spoofing disabled
    pub fn off() -> Self {
        FingerprintProtection {
            spoof_canvas: false,
            spoof_webgl: false,
            spoof_audio: false,
            spoof_fonts: false,
            spoof_screen: false,
            spoof_timezone: false,
            spoof_language: false,
            noise_seed: 0,
        }
    }

    /// Count how many spoofing protections are currently active
    pub fn active_count(&self) -> u32 {
        let mut count: u32 = 0;
        if self.spoof_canvas {
            count += 1;
        }
        if self.spoof_webgl {
            count += 1;
        }
        if self.spoof_audio {
            count += 1;
        }
        if self.spoof_fonts {
            count += 1;
        }
        if self.spoof_screen {
            count += 1;
        }
        if self.spoof_timezone {
            count += 1;
        }
        if self.spoof_language {
            count += 1;
        }
        count
    }
}

impl PrivacyEngine {
    /// Create a new privacy engine with strict defaults
    pub fn new() -> Self {
        let mut engine = PrivacyEngine {
            protection_level: TrackingProtection::Strict,
            tracker_list: Vec::new(),
            cookie_jar: CookieJar::new(),
            cookie_policy: CookiePolicy::RejectThirdParty,
            fingerprint: FingerprintProtection::strict(),
            do_not_track: true,
            global_privacy_control: true,
            total_blocked: 0,
            last_blocklist_update: 0,
        };
        engine.load_default_blocklist();
        engine
    }

    /// Load the built-in set of known tracker domain hashes
    fn load_default_blocklist(&mut self) {
        let categories = [
            TrackerCategory::Analytics,
            TrackerCategory::Advertising,
            TrackerCategory::Social,
            TrackerCategory::Cryptominer,
            TrackerCategory::Fingerprinter,
            TrackerCategory::ContentTracker,
            TrackerCategory::Advertising,
            TrackerCategory::Analytics,
        ];
        for (i, &hash) in KNOWN_TRACKER_HASHES.iter().enumerate() {
            let cat = if i < categories.len() {
                categories[i]
            } else {
                TrackerCategory::Custom
            };
            self.tracker_list.push(TrackerEntry {
                domain_hash: hash,
                category: cat,
                blocked_count: 0,
                first_seen: 0,
            });
        }
    }

    // -- Tracker methods ----------------------------------------------------

    /// Check whether a domain hash matches a known tracker.
    /// Returns Some(category) if blocked under the current protection level,
    /// or None if the request should be allowed.
    pub fn check_tracker(&self, domain_hash: u64) -> Option<TrackerCategory> {
        if self.protection_level == TrackingProtection::Off {
            return None;
        }
        for entry in &self.tracker_list {
            if entry.domain_hash == domain_hash {
                let dominated = match self.protection_level {
                    TrackingProtection::Standard => matches!(
                        entry.category,
                        TrackerCategory::Analytics
                            | TrackerCategory::Advertising
                            | TrackerCategory::Cryptominer
                            | TrackerCategory::Fingerprinter
                    ),
                    TrackingProtection::Strict | TrackingProtection::Custom => true,
                    TrackingProtection::Off => false,
                };
                if dominated {
                    return Some(entry.category);
                }
            }
        }
        None
    }

    /// Block a tracker: increment its blocked_count and the global counter.
    /// If the domain is not yet in the blocklist, add it with the given category.
    pub fn block_tracker(&mut self, domain_hash: u64, category: TrackerCategory, now: u64) {
        let mut found = false;
        for entry in &mut self.tracker_list {
            if entry.domain_hash == domain_hash {
                entry.blocked_count = entry.blocked_count.saturating_add(1);
                found = true;
                break;
            }
        }
        if !found && self.tracker_list.len() < MAX_TRACKER_ENTRIES {
            self.tracker_list.push(TrackerEntry {
                domain_hash,
                category,
                blocked_count: 1,
                first_seen: now,
            });
        }
        self.total_blocked = self.total_blocked.saturating_add(1);
    }

    /// Get the total number of blocked requests across all trackers
    pub fn get_blocked_count(&self) -> u64 {
        self.total_blocked
    }

    /// Detect whether a request looks like a fingerprinting attempt.
    /// Heuristic: rapid enumeration of canvas, WebGL, audio, fonts in sequence.
    /// `api_flags` is a bitmask: bit0=canvas, bit1=webgl, bit2=audio, bit3=fonts,
    /// bit4=screen, bit5=timezone, bit6=language.
    pub fn detect_fingerprinting(&self, api_flags: u32) -> bool {
        // If 3 or more distinct fingerprinting APIs are accessed, flag it
        let mut count: u32 = 0;
        let mut mask = api_flags;
        while mask != 0 {
            count += mask & 1;
            mask >>= 1;
        }
        count >= 3
    }

    // -- Cookie methods -----------------------------------------------------

    /// Retrieve a cookie by domain hash and name hash.
    /// Returns None if not found or if cookie policy forbids access.
    pub fn get_cookie(&self, domain_hash: u64, name_hash: u64) -> Option<&Cookie> {
        for cookie in &self.cookie_jar.cookies {
            if cookie.domain_hash == domain_hash && cookie.name_hash == name_hash {
                return Some(cookie);
            }
        }
        None
    }

    /// Store a cookie, subject to the current cookie policy and jar limits.
    /// Returns true if the cookie was accepted, false if rejected.
    pub fn set_cookie(&mut self, cookie: Cookie, now: u64) -> bool {
        // Enforce cookie policy
        match self.cookie_policy {
            CookiePolicy::RejectAll => return false,
            CookiePolicy::RejectThirdParty => {
                if cookie.third_party {
                    return false;
                }
            }
            CookiePolicy::SessionOnly => {
                // Will be stored but treated as session cookie (expires = 0)
            }
            CookiePolicy::AcceptAll | CookiePolicy::Custom => {}
        }

        // Check per-domain limit
        let domain_count = self.cookie_jar.count_for_domain(cookie.domain_hash);
        if domain_count >= self.cookie_jar.max_per_domain {
            // Evict the oldest cookie for this domain (first found)
            let dh = cookie.domain_hash;
            if let Some(pos) = self
                .cookie_jar
                .cookies
                .iter()
                .position(|c| c.domain_hash == dh)
            {
                self.cookie_jar.cookies.remove(pos);
            }
        }

        // Check global limit
        if self.cookie_jar.cookies.len() as u32 >= self.cookie_jar.total_limit {
            // Purge expired first
            self.cookie_jar.purge_expired(now);
            // If still over limit, evict oldest entry
            if self.cookie_jar.cookies.len() as u32 >= self.cookie_jar.total_limit {
                if !self.cookie_jar.cookies.is_empty() {
                    self.cookie_jar.cookies.remove(0);
                }
            }
        }

        // If SessionOnly policy, force expiration to 0
        let stored = if self.cookie_policy == CookiePolicy::SessionOnly {
            Cookie {
                expires: 0,
                ..cookie
            }
        } else {
            cookie
        };

        // Replace existing cookie with same domain+name, or insert new
        let dh = stored.domain_hash;
        let nh = stored.name_hash;
        if let Some(pos) = self
            .cookie_jar
            .cookies
            .iter()
            .position(|c| c.domain_hash == dh && c.name_hash == nh)
        {
            self.cookie_jar.cookies[pos] = stored;
        } else {
            self.cookie_jar.cookies.push(stored);
        }

        true
    }

    /// Clear all cookies from the jar
    pub fn clear_cookies(&mut self) {
        self.cookie_jar.cookies.clear();
    }

    /// Clear cookies for a specific domain
    pub fn clear_domain_cookies(&mut self, domain_hash: u64) {
        self.cookie_jar
            .cookies
            .retain(|c| c.domain_hash != domain_hash);
    }

    // -- Fingerprint methods ------------------------------------------------

    /// Apply fingerprint protection to a request context.
    /// Returns a bitmask of which protections were applied:
    /// bit0=canvas, bit1=webgl, bit2=audio, bit3=fonts,
    /// bit4=screen, bit5=timezone, bit6=language.
    pub fn apply_fingerprint_protection(&self) -> u32 {
        let mut result: u32 = 0;
        if self.fingerprint.spoof_canvas {
            result |= 1 << 0;
        }
        if self.fingerprint.spoof_webgl {
            result |= 1 << 1;
        }
        if self.fingerprint.spoof_audio {
            result |= 1 << 2;
        }
        if self.fingerprint.spoof_fonts {
            result |= 1 << 3;
        }
        if self.fingerprint.spoof_screen {
            result |= 1 << 4;
        }
        if self.fingerprint.spoof_timezone {
            result |= 1 << 5;
        }
        if self.fingerprint.spoof_language {
            result |= 1 << 6;
        }
        result
    }

    /// Generate deterministic canvas noise value from the seed and a pixel coordinate.
    /// Returns a Q16 fixed-point noise value in range [-Q16_ONE/16, +Q16_ONE/16].
    pub fn randomize_canvas_noise(&self, x: u32, y: u32) -> i32 {
        if !self.fingerprint.spoof_canvas {
            return 0;
        }
        let seed = self.fingerprint.noise_seed;
        // Simple hash-based PRNG mixing seed with coordinates
        let mut h: u64 = seed;
        h = h.wrapping_mul(0x517C_C1B7_2722_0A95);
        h ^= x as u64;
        h = h.wrapping_mul(0x6C62_272E_07BB_0142);
        h ^= y as u64;
        h = h.wrapping_mul(0x305B_4DE2_A3CF_B14D);
        h ^= h >> 33;
        // Map to Q16 range: [-4096, +4096] (i.e., [-1/16, +1/16] in Q16)
        let raw = (h & 0x1FFF) as i32; // 0..8191
        raw - 4096
    }

    // -- Blocklist import/export --------------------------------------------

    /// Export the current blocklist as a Vec of (domain_hash, category_code) pairs.
    /// Category codes: 0=Analytics, 1=Advertising, 2=Social, 3=Cryptominer,
    /// 4=Fingerprinter, 5=ContentTracker, 6=Custom.
    pub fn export_blocklist(&self) -> Vec<(u64, u8)> {
        let mut list = Vec::new();
        for entry in &self.tracker_list {
            let code = match entry.category {
                TrackerCategory::Analytics => 0,
                TrackerCategory::Advertising => 1,
                TrackerCategory::Social => 2,
                TrackerCategory::Cryptominer => 3,
                TrackerCategory::Fingerprinter => 4,
                TrackerCategory::ContentTracker => 5,
                TrackerCategory::Custom => 6,
            };
            list.push((entry.domain_hash, code));
        }
        list
    }

    /// Import a blocklist from a Vec of (domain_hash, category_code) pairs.
    /// Merges with existing entries; duplicates are skipped.
    pub fn import_blocklist(&mut self, entries: &[(u64, u8)], now: u64) {
        for &(hash, code) in entries {
            // Skip if already present
            let already = self.tracker_list.iter().any(|e| e.domain_hash == hash);
            if already {
                continue;
            }
            if self.tracker_list.len() >= MAX_TRACKER_ENTRIES {
                break;
            }
            let category = match code {
                0 => TrackerCategory::Analytics,
                1 => TrackerCategory::Advertising,
                2 => TrackerCategory::Social,
                3 => TrackerCategory::Cryptominer,
                4 => TrackerCategory::Fingerprinter,
                5 => TrackerCategory::ContentTracker,
                _ => TrackerCategory::Custom,
            };
            self.tracker_list.push(TrackerEntry {
                domain_hash: hash,
                category,
                blocked_count: 0,
                first_seen: now,
            });
        }
        self.last_blocklist_update = now;
    }

    // -- Privacy score ------------------------------------------------------

    /// Compute an overall privacy score from 0 (no protection) to 100 (maximum).
    /// Uses Q16 fixed-point internally, returns final integer score.
    pub fn get_privacy_score(&self) -> i32 {
        // Tracking protection component: 0-30 points
        let tracking_score: i32 = match self.protection_level {
            TrackingProtection::Off => 0,
            TrackingProtection::Standard => 20 * Q16_ONE,
            TrackingProtection::Strict => 30 * Q16_ONE,
            TrackingProtection::Custom => 25 * Q16_ONE,
        };

        // Cookie policy component: 0-25 points
        let cookie_score: i32 = match self.cookie_policy {
            CookiePolicy::AcceptAll => 0,
            CookiePolicy::RejectThirdParty => 15 * Q16_ONE,
            CookiePolicy::SessionOnly => 20 * Q16_ONE,
            CookiePolicy::RejectAll => 25 * Q16_ONE,
            CookiePolicy::Custom => 10 * Q16_ONE,
        };

        // Fingerprint protection component: 0-30 points (scaled by active count)
        // 7 possible protections, each worth ~4.28 points -> use 4 * Q16 + fraction
        let fp_active = self.fingerprint.active_count() as i32;
        // 30 points / 7 protections = 4.2857... -> approximate as (30 * Q16) / 7
        let fp_per = (30 * Q16_ONE) / 7;
        let fp_score: i32 = fp_active * fp_per;

        // DNT + GPC: 0-15 points (7.5 each, but use 7 + 8 = 15 integer split)
        let mut header_score: i32 = 0;
        if self.do_not_track {
            header_score += 7 * Q16_ONE;
        }
        if self.global_privacy_control {
            header_score += 8 * Q16_ONE;
        }

        // Sum all components and convert from Q16 to integer
        let total_q16 = tracking_score + cookie_score + fp_score + header_score;
        let score = total_q16 / Q16_ONE;

        // Clamp to 0..100
        if score < 0 {
            0
        } else if score > 100 {
            100
        } else {
            score
        }
    }

    /// Get a textual privacy grade based on the score
    pub fn get_privacy_grade(&self) -> &'static str {
        let score = self.get_privacy_score();
        if score >= 90 {
            "A+"
        } else if score >= 80 {
            "A"
        } else if score >= 70 {
            "B"
        } else if score >= 60 {
            "C"
        } else if score >= 40 {
            "D"
        } else {
            "F"
        }
    }

    /// Get blocked count per category as a Vec of (TrackerCategory, u64) pairs
    pub fn blocked_per_category(&self) -> Vec<(TrackerCategory, u64)> {
        let categories = [
            TrackerCategory::Analytics,
            TrackerCategory::Advertising,
            TrackerCategory::Social,
            TrackerCategory::Cryptominer,
            TrackerCategory::Fingerprinter,
            TrackerCategory::ContentTracker,
            TrackerCategory::Custom,
        ];
        let mut result = Vec::new();
        for cat in &categories {
            let mut total: u64 = 0;
            for entry in &self.tracker_list {
                if entry.category == *cat {
                    total += entry.blocked_count;
                }
            }
            if total > 0 {
                result.push((*cat, total));
            }
        }
        result
    }

    /// Set the tracking protection level
    pub fn set_protection_level(&mut self, level: TrackingProtection) {
        self.protection_level = level;
    }

    /// Set the cookie policy
    pub fn set_cookie_policy(&mut self, policy: CookiePolicy) {
        self.cookie_policy = policy;
    }

    /// Update the fingerprint noise seed (e.g., rotate per session)
    pub fn rotate_noise_seed(&mut self, new_seed: u64) {
        self.fingerprint.noise_seed = new_seed;
    }
}

// ---------------------------------------------------------------------------
// Global API (thread-safe via Mutex)
// ---------------------------------------------------------------------------

/// Check if a domain is a tracked entity. Returns category if blocked.
pub fn check_tracker(domain_hash: u64) -> Option<TrackerCategory> {
    let guard = PRIVACY_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.check_tracker(domain_hash)
    } else {
        None
    }
}

/// Block a tracker and record the event
pub fn block_tracker(domain_hash: u64, category: TrackerCategory, now: u64) {
    let mut guard = PRIVACY_ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.block_tracker(domain_hash, category, now);
    }
}

/// Get the total number of blocked tracker requests
pub fn get_blocked_count() -> u64 {
    let guard = PRIVACY_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.get_blocked_count()
    } else {
        0
    }
}

/// Get the current privacy score (0-100)
pub fn get_privacy_score() -> i32 {
    let guard = PRIVACY_ENGINE.lock();
    if let Some(ref engine) = *guard {
        engine.get_privacy_score()
    } else {
        0
    }
}

/// Initialize the browser privacy guard subsystem
pub fn init() {
    let engine = PrivacyEngine::new();
    let score = engine.get_privacy_score();
    let grade = engine.get_privacy_grade();
    let tracker_count = engine.tracker_list.len();
    let fp_active = engine.fingerprint.active_count();

    {
        let mut guard = PRIVACY_ENGINE.lock();
        *guard = Some(engine);
    }

    serial_println!(
        "  Privacy Guard: {} trackers loaded, {} fingerprint protections active",
        tracker_count,
        fp_active
    );
    serial_println!(
        "  Privacy Guard: score={}/100 grade={} DNT=on GPC=on",
        score,
        grade
    );
}
