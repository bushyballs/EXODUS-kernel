use crate::sync::Mutex;
/// Browser network security for Genesis
///
/// Provides web-standard security enforcement:
///   - CORS (Cross-Origin Resource Sharing) policy checking
///   - CSP (Content Security Policy) directive enforcement
///   - HTTPS enforcement with HSTS (HTTP Strict Transport Security)
///   - Mixed content blocking (prevent HTTP resources on HTTPS pages)
///   - Certificate pinning and validation
///   - Tracking request blocking
///   - Redirect safety analysis
///
/// Inspired by: Chromium network stack security, Firefox Necko,
/// W3C CORS/CSP specifications. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Global network security engine
static NET_SEC_ENGINE: Mutex<Option<NetworkSecEngine>> = Mutex::new(None);

// ── HTTP method bitmask constants ──────────────────────────────────────────

const METHOD_GET: u8 = 0x01;
const METHOD_POST: u8 = 0x02;
const METHOD_PUT: u8 = 0x04;
const METHOD_DELETE: u8 = 0x08;
const METHOD_PATCH: u8 = 0x10;
const METHOD_OPTIONS: u8 = 0x20;

// ── Well-known port constants ──────────────────────────────────────────────

const PORT_HTTP: u16 = 80;
const PORT_HTTPS: u16 = 443;

// ── Security score weights (Q16 fixed-point: 65536 = 1.0) ─────────────────

const SCORE_HTTPS: i32 = 20 * 65536;
const SCORE_HSTS: i32 = 15 * 65536;
const SCORE_CSP: i32 = 20 * 65536;
const SCORE_CORS: i32 = 15 * 65536;
const SCORE_CERT_PINNED: i32 = 10 * 65536;
const SCORE_NO_MIXED: i32 = 10 * 65536;
const SCORE_NO_TRACKING: i32 = 10 * 65536;

// ── Tracking domain hash constants ─────────────────────────────────────────
// Pre-hashed domain identifiers for known tracking domains.

const TRACKER_HASH_A: u64 = 0xAA01BB02CC03DD04;
const TRACKER_HASH_B: u64 = 0xBB02CC03DD04EE05;
const TRACKER_HASH_C: u64 = 0xCC03DD04EE050A0B;
const TRACKER_HASH_D: u64 = 0xDD04EE050A0B1C1D;
const TRACKER_HASH_E: u64 = 0xEE050A0B1C1D2E2F;
const TRACKER_HASH_F: u64 = 0x0A0B1C1D2E2F3A3B;
const TRACKER_HASH_G: u64 = 0x1C1D2E2F3A3B4C4D;
const TRACKER_HASH_H: u64 = 0x2E2F3A3B4C4D5E5F;

/// Maximum number of cached CORS policies
const MAX_CORS_CACHE: usize = 256;

/// Maximum number of HSTS domains
const MAX_HSTS_DOMAINS: usize = 1024;

/// Maximum number of certificate pins
const MAX_CERT_PINS: usize = 512;

/// Maximum redirect chain depth
const MAX_REDIRECT_DEPTH: u32 = 10;

// ── CORS ───────────────────────────────────────────────────────────────────

/// CORS policy for a specific origin
#[derive(Debug, Clone)]
pub struct CorsPolicy {
    /// Hash of the origin this policy applies to
    pub origin_hash: u64,
    /// Hashes of origins allowed to access this resource
    pub allowed_origins: Vec<u64>,
    /// Bitmask of allowed HTTP methods (METHOD_GET | METHOD_POST | ...)
    pub allowed_methods: u8,
    /// Hashes of allowed request headers
    pub allowed_headers: Vec<u64>,
    /// Hashes of headers the browser is allowed to expose to JS
    pub expose_headers: Vec<u64>,
    /// How long the preflight result can be cached (seconds)
    pub max_age_seconds: u32,
    /// Whether credentials (cookies, auth headers) are allowed
    pub allow_credentials: bool,
}

impl CorsPolicy {
    /// Create a new CORS policy for an origin
    pub fn new(origin_hash: u64) -> Self {
        CorsPolicy {
            origin_hash,
            allowed_origins: Vec::new(),
            allowed_methods: METHOD_GET | METHOD_OPTIONS,
            allowed_headers: Vec::new(),
            expose_headers: Vec::new(),
            max_age_seconds: 3600,
            allow_credentials: false,
        }
    }

    /// Create a permissive CORS policy (allow all origins, all methods)
    pub fn permissive(origin_hash: u64) -> Self {
        CorsPolicy {
            origin_hash,
            allowed_origins: Vec::new(), // empty = wildcard
            allowed_methods: METHOD_GET
                | METHOD_POST
                | METHOD_PUT
                | METHOD_DELETE
                | METHOD_PATCH
                | METHOD_OPTIONS,
            allowed_headers: Vec::new(),
            expose_headers: Vec::new(),
            max_age_seconds: 86400,
            allow_credentials: false,
        }
    }

    /// Check whether a specific origin hash is allowed
    fn is_origin_allowed(&self, requesting_origin: u64) -> bool {
        // Empty allowed_origins list means wildcard (allow all)
        if self.allowed_origins.is_empty() && !self.allow_credentials {
            return true;
        }
        for &allowed in &self.allowed_origins {
            if allowed == requesting_origin {
                return true;
            }
        }
        false
    }

    /// Check whether a specific HTTP method bitmask is allowed
    fn is_method_allowed(&self, method_bit: u8) -> bool {
        (self.allowed_methods & method_bit) != 0
    }

    /// Check whether a request header hash is allowed
    fn is_header_allowed(&self, header_hash: u64) -> bool {
        if self.allowed_headers.is_empty() {
            return true; // wildcard
        }
        for &allowed in &self.allowed_headers {
            if allowed == header_hash {
                return true;
            }
        }
        false
    }
}

// ── CSP ────────────────────────────────────────────────────────────────────

/// Content Security Policy directive types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CspDirective {
    DefaultSrc,
    ScriptSrc,
    StyleSrc,
    ImgSrc,
    FontSrc,
    ConnectSrc,
    FrameSrc,
    MediaSrc,
    ObjectSrc,
    BaseUri,
    FormAction,
    FrameAncestors,
    ReportUri,
}

impl CspDirective {
    /// Return the fallback directive if this one isn't specified.
    /// Most directives fall back to default-src per the CSP spec.
    fn fallback(&self) -> Option<CspDirective> {
        match self {
            CspDirective::DefaultSrc => None,
            CspDirective::ReportUri => None,
            CspDirective::FrameAncestors => None,
            CspDirective::FormAction => None,
            CspDirective::BaseUri => None,
            _ => Some(CspDirective::DefaultSrc),
        }
    }
}

/// Content Security Policy
#[derive(Debug, Clone)]
pub struct CspPolicy {
    /// List of (directive, allowed_source_hashes) pairs.
    /// Each source hash represents a domain or keyword ('self', 'none', etc.)
    pub directives: Vec<(CspDirective, Vec<u64>)>,
    /// If true, violations are reported but not enforced
    pub report_only: bool,
    /// If true, automatically upgrade HTTP requests to HTTPS
    pub upgrade_insecure: bool,
}

impl CspPolicy {
    /// Create a new empty CSP policy
    pub fn new() -> Self {
        CspPolicy {
            directives: Vec::new(),
            report_only: false,
            upgrade_insecure: false,
        }
    }

    /// Create a strict CSP policy (default-src 'none', must explicitly allow)
    pub fn strict() -> Self {
        let mut policy = CspPolicy::new();
        // default-src 'none' — block everything by default
        policy
            .directives
            .push((CspDirective::DefaultSrc, Vec::new()));
        policy.upgrade_insecure = true;
        policy
    }

    /// Add a directive with a list of allowed source hashes
    pub fn add_directive(&mut self, directive: CspDirective, sources: Vec<u64>) {
        // Replace existing directive of same type
        for entry in &mut self.directives {
            if entry.0 == directive {
                entry.1 = sources;
                return;
            }
        }
        self.directives.push((directive, sources));
    }

    /// Look up allowed sources for a directive, falling back to default-src
    fn sources_for(&self, directive: CspDirective) -> Option<&Vec<u64>> {
        for entry in &self.directives {
            if entry.0 == directive {
                return Some(&entry.1);
            }
        }
        // Fallback
        if let Some(fallback) = directive.fallback() {
            for entry in &self.directives {
                if entry.0 == fallback {
                    return Some(&entry.1);
                }
            }
        }
        None
    }
}

// ── Mixed content ──────────────────────────────────────────────────────────

/// Action to take when mixed content is detected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentAction {
    /// Block the insecure request entirely
    Block,
    /// Allow but log a warning
    Warn,
    /// Allow without any action (not recommended)
    Allow,
}

/// Type of mixed content (active vs passive)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixedContentType {
    /// Active content (scripts, iframes, XHR) — always blocked
    Active,
    /// Passive/display content (images, audio, video) — optionally blocked
    Passive,
}

// ── Certificate status ─────────────────────────────────────────────────────

/// Certificate validation status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateStatus {
    Valid,
    Expired,
    SelfSigned,
    Revoked,
    MismatchDomain,
    Unknown,
}

/// Certificate pin entry: associates a domain hash with an expected
/// public key hash for pinning
#[derive(Debug, Clone)]
pub struct CertificatePin {
    /// Hash of the domain name
    pub domain_hash: u64,
    /// Expected SHA-256 hash of the Subject Public Key Info (as u64 truncation)
    pub pin_hash: u64,
    /// When the pin expires (monotonic timestamp)
    pub expires_at: u64,
    /// Include sub-domains
    pub include_subdomains: bool,
}

// ── HSTS entry ─────────────────────────────────────────────────────────────

/// HSTS (HTTP Strict Transport Security) domain entry
#[derive(Debug, Clone)]
pub struct HstsEntry {
    /// Hash of the domain name
    pub domain_hash: u64,
    /// Max-Age value in seconds
    pub max_age: u64,
    /// Whether subdomains are included
    pub include_subdomains: bool,
    /// Timestamp when this entry was added (monotonic)
    pub added_at: u64,
}

impl HstsEntry {
    /// Check if this HSTS entry has expired based on the current time
    fn is_expired(&self, now: u64) -> bool {
        now > self.added_at + self.max_age
    }
}

// ── CORS check result ──────────────────────────────────────────────────────

/// Result of a CORS check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorsResult {
    /// Request is allowed
    Allowed,
    /// Origin is not in the allowed list
    OriginDenied,
    /// Method is not allowed
    MethodDenied,
    /// A required header is not allowed
    HeaderDenied,
    /// Credentials requested but wildcard origin used
    CredentialConflict,
    /// No CORS policy found for target origin
    NoPolicyFound,
}

// ── CSP check result ───────────────────────────────────────────────────────

/// Result of a CSP check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CspResult {
    /// Source is allowed by the policy
    Allowed,
    /// Source is blocked by the policy
    Blocked,
    /// Source would be blocked but policy is report-only
    ReportOnly,
    /// No CSP policy is active
    NoPolicyActive,
}

// ── Redirect safety ────────────────────────────────────────────────────────

/// Result of redirect safety analysis
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectSafety {
    /// Redirect is safe
    Safe,
    /// HTTPS downgraded to HTTP
    HttpsDowngrade,
    /// Redirect chain is too long
    TooManyRedirects,
    /// Redirect to a known tracking domain
    TrackerRedirect,
    /// Redirect to a data: or javascript: scheme
    DangerousScheme,
}

// ── Network Security Engine ────────────────────────────────────────────────

/// Central engine for browser network security enforcement
pub struct NetworkSecEngine {
    /// Cached CORS policies indexed by target origin hash
    pub cors_cache: Vec<CorsPolicy>,
    /// Active CSP policies for the current browsing context
    pub csp_policies: Vec<CspPolicy>,
    /// Total number of blocked requests since init
    pub blocked_requests: u64,
    /// Whether HTTPS is enforced globally
    pub https_enforced: bool,
    /// HSTS domain entries
    pub hsts_domains: Vec<HstsEntry>,
    /// Certificate pins
    pub cert_pins: Vec<CertificatePin>,
    /// Mixed content action for passive resources
    pub mixed_content_passive: MixedContentAction,
    /// Tracking protection enabled
    pub tracking_protection: bool,
    /// Known tracker domain hashes
    pub tracker_hashes: Vec<u64>,
    /// Total warnings issued
    pub warnings_issued: u64,
    /// Total CORS preflight cache hits
    pub cors_cache_hits: u64,
    /// Total CORS preflight cache misses
    pub cors_cache_misses: u64,
}

impl NetworkSecEngine {
    /// Create a new NetworkSecEngine with secure defaults
    pub fn new() -> Self {
        let tracker_hashes = vec![
            TRACKER_HASH_A,
            TRACKER_HASH_B,
            TRACKER_HASH_C,
            TRACKER_HASH_D,
            TRACKER_HASH_E,
            TRACKER_HASH_F,
            TRACKER_HASH_G,
            TRACKER_HASH_H,
        ];

        NetworkSecEngine {
            cors_cache: Vec::new(),
            csp_policies: Vec::new(),
            blocked_requests: 0,
            https_enforced: true,
            hsts_domains: Vec::new(),
            cert_pins: Vec::new(),
            mixed_content_passive: MixedContentAction::Block,
            tracking_protection: true,
            tracker_hashes,
            warnings_issued: 0,
            cors_cache_hits: 0,
            cors_cache_misses: 0,
        }
    }

    // ── CORS ───────────────────────────────────────────────────────────

    /// Check whether a cross-origin request is allowed.
    ///
    /// `target_origin` — hash of the resource being requested
    /// `requesting_origin` — hash of the page making the request
    /// `method_bit` — one of METHOD_GET, METHOD_POST, etc.
    /// `header_hashes` — hashes of custom request headers
    pub fn check_cors(
        &mut self,
        target_origin: u64,
        requesting_origin: u64,
        method_bit: u8,
        header_hashes: &[u64],
    ) -> CorsResult {
        // Same-origin requests always pass
        if target_origin == requesting_origin {
            return CorsResult::Allowed;
        }

        // Look up cached policy for the target origin
        let policy = match self
            .cors_cache
            .iter()
            .find(|p| p.origin_hash == target_origin)
        {
            Some(p) => p,
            None => {
                self.cors_cache_misses = self.cors_cache_misses.saturating_add(1);
                return CorsResult::NoPolicyFound;
            }
        };

        self.cors_cache_hits = self.cors_cache_hits.saturating_add(1);

        // Check credentials + wildcard conflict
        if policy.allow_credentials && policy.allowed_origins.is_empty() {
            self.blocked_requests = self.blocked_requests.saturating_add(1);
            return CorsResult::CredentialConflict;
        }

        // Check origin
        if !policy.is_origin_allowed(requesting_origin) {
            self.blocked_requests = self.blocked_requests.saturating_add(1);
            return CorsResult::OriginDenied;
        }

        // Check method
        if !policy.is_method_allowed(method_bit) {
            self.blocked_requests = self.blocked_requests.saturating_add(1);
            return CorsResult::MethodDenied;
        }

        // Check headers
        for &hdr in header_hashes {
            if !policy.is_header_allowed(hdr) {
                self.blocked_requests = self.blocked_requests.saturating_add(1);
                return CorsResult::HeaderDenied;
            }
        }

        CorsResult::Allowed
    }

    /// Add or update a CORS policy in the cache
    pub fn add_cors_policy(&mut self, policy: CorsPolicy) {
        // Replace existing policy for same origin
        for existing in &mut self.cors_cache {
            if existing.origin_hash == policy.origin_hash {
                *existing = policy;
                return;
            }
        }
        // Evict oldest if cache is full
        if self.cors_cache.len() >= MAX_CORS_CACHE {
            self.cors_cache.remove(0);
        }
        self.cors_cache.push(policy);
    }

    // ── CSP ────────────────────────────────────────────────────────────

    /// Enforce CSP: check if a source hash is allowed for a given directive.
    ///
    /// Checks all active CSP policies. If any policy blocks the source,
    /// the request is blocked (unless that policy is report-only).
    pub fn enforce_csp(&mut self, directive: CspDirective, source_hash: u64) -> CspResult {
        if self.csp_policies.is_empty() {
            return CspResult::NoPolicyActive;
        }

        let mut any_blocked = false;
        let mut all_report_only = true;

        for policy in &self.csp_policies {
            let sources = match policy.sources_for(directive) {
                Some(s) => s,
                None => continue,
            };

            // Empty sources list means 'none' — block everything
            if sources.is_empty() {
                any_blocked = true;
                if !policy.report_only {
                    all_report_only = false;
                }
                continue;
            }

            // Check if source is in the allowed list
            let mut found = false;
            for &allowed in sources {
                if allowed == source_hash {
                    found = true;
                    break;
                }
            }

            if !found {
                any_blocked = true;
                if !policy.report_only {
                    all_report_only = false;
                }
            }
        }

        if any_blocked {
            if all_report_only {
                self.warnings_issued = self.warnings_issued.saturating_add(1);
                serial_println!(
                    "[NET_SEC] CSP report-only violation for directive {:?}",
                    directive
                );
                CspResult::ReportOnly
            } else {
                self.blocked_requests = self.blocked_requests.saturating_add(1);
                serial_println!("[NET_SEC] CSP BLOCKED source for directive {:?}", directive);
                CspResult::Blocked
            }
        } else {
            CspResult::Allowed
        }
    }

    /// Add a CSP policy to the engine
    pub fn add_csp_policy(&mut self, policy: CspPolicy) {
        self.csp_policies.push(policy);
    }

    /// Clear all CSP policies (e.g., on page navigation)
    pub fn clear_csp_policies(&mut self) {
        self.csp_policies.clear();
    }

    // ── Mixed content ──────────────────────────────────────────────────

    /// Check whether a sub-resource request constitutes mixed content.
    ///
    /// `page_is_https` — whether the current page was loaded over HTTPS
    /// `resource_is_https` — whether the resource URL is HTTPS
    /// `content_type` — active (scripts, etc.) vs passive (images, etc.)
    ///
    /// Returns the action to take.
    pub fn check_mixed_content(
        &mut self,
        page_is_https: bool,
        resource_is_https: bool,
        content_type: MixedContentType,
    ) -> MixedContentAction {
        // No mixed content if page isn't HTTPS or resource is HTTPS
        if !page_is_https || resource_is_https {
            return MixedContentAction::Allow;
        }

        // Active mixed content is ALWAYS blocked
        if content_type == MixedContentType::Active {
            self.blocked_requests = self.blocked_requests.saturating_add(1);
            serial_println!("[NET_SEC] Blocked active mixed content");
            return MixedContentAction::Block;
        }

        // Passive mixed content uses the configured action
        match self.mixed_content_passive {
            MixedContentAction::Block => {
                self.blocked_requests = self.blocked_requests.saturating_add(1);
                serial_println!("[NET_SEC] Blocked passive mixed content");
            }
            MixedContentAction::Warn => {
                self.warnings_issued = self.warnings_issued.saturating_add(1);
                serial_println!("[NET_SEC] Warning: passive mixed content detected");
            }
            MixedContentAction::Allow => {}
        }
        self.mixed_content_passive
    }

    // ── Certificate verification ───────────────────────────────────────

    /// Verify a certificate against stored pins and basic status.
    ///
    /// `domain_hash` — hash of the domain being connected to
    /// `cert_status` — status from the TLS handshake
    /// `cert_pubkey_hash` — truncated SHA-256 of the certificate's SPKI
    ///
    /// Returns true if the certificate is accepted.
    pub fn verify_certificate(
        &mut self,
        domain_hash: u64,
        cert_status: CertificateStatus,
        cert_pubkey_hash: u64,
    ) -> bool {
        // Reject any certificate that isn't valid
        if cert_status != CertificateStatus::Valid {
            self.blocked_requests = self.blocked_requests.saturating_add(1);
            serial_println!(
                "[NET_SEC] Certificate rejected: {:?} for domain {:016X}",
                cert_status,
                domain_hash
            );
            return false;
        }

        // Check certificate pinning
        for pin in &self.cert_pins {
            if pin.domain_hash == domain_hash {
                if pin.pin_hash != cert_pubkey_hash {
                    self.blocked_requests = self.blocked_requests.saturating_add(1);
                    serial_println!(
                        "[NET_SEC] Certificate pin mismatch for domain {:016X}",
                        domain_hash
                    );
                    return false;
                }
                // Pin matched
                return true;
            }
        }

        // No pin for this domain — accept valid cert
        true
    }

    // ── HSTS ───────────────────────────────────────────────────────────

    /// Add an HSTS entry for a domain.
    ///
    /// If the domain already exists, update it. If max_age is 0,
    /// remove the entry (per the HSTS spec).
    pub fn add_hsts(&mut self, domain_hash: u64, max_age: u64, include_subdomains: bool, now: u64) {
        // max_age 0 means remove
        if max_age == 0 {
            self.hsts_domains.retain(|e| e.domain_hash != domain_hash);
            serial_println!("[NET_SEC] Removed HSTS for domain {:016X}", domain_hash);
            return;
        }

        // Update existing
        for entry in &mut self.hsts_domains {
            if entry.domain_hash == domain_hash {
                entry.max_age = max_age;
                entry.include_subdomains = include_subdomains;
                entry.added_at = now;
                return;
            }
        }

        // Evict expired entries if at capacity
        if self.hsts_domains.len() >= MAX_HSTS_DOMAINS {
            self.hsts_domains.retain(|e| !e.is_expired(now));
        }

        // Still full? Evict oldest
        if self.hsts_domains.len() >= MAX_HSTS_DOMAINS {
            self.hsts_domains.remove(0);
        }

        self.hsts_domains.push(HstsEntry {
            domain_hash,
            max_age,
            include_subdomains,
            added_at: now,
        });

        serial_println!(
            "[NET_SEC] HSTS added for domain {:016X}, max_age={}, subdomains={}",
            domain_hash,
            max_age,
            include_subdomains
        );
    }

    /// Check whether a domain is in the HSTS list (must use HTTPS).
    ///
    /// `domain_hash` — hash of the domain to check
    /// `parent_domain_hash` — hash of the parent domain (for subdomain checking)
    /// `now` — current monotonic timestamp
    pub fn check_hsts(&self, domain_hash: u64, parent_domain_hash: u64, now: u64) -> bool {
        for entry in &self.hsts_domains {
            if entry.is_expired(now) {
                continue;
            }
            if entry.domain_hash == domain_hash {
                return true;
            }
            if entry.include_subdomains && entry.domain_hash == parent_domain_hash {
                return true;
            }
        }
        false
    }

    // ── Certificate pinning ────────────────────────────────────────────

    /// Pin a certificate for a domain.
    pub fn pin_certificate(
        &mut self,
        domain_hash: u64,
        pin_hash: u64,
        expires_at: u64,
        include_subdomains: bool,
    ) {
        // Update existing pin
        for pin in &mut self.cert_pins {
            if pin.domain_hash == domain_hash {
                pin.pin_hash = pin_hash;
                pin.expires_at = expires_at;
                pin.include_subdomains = include_subdomains;
                serial_println!(
                    "[NET_SEC] Updated certificate pin for domain {:016X}",
                    domain_hash
                );
                return;
            }
        }

        if self.cert_pins.len() >= MAX_CERT_PINS {
            // Evict first (oldest) entry
            self.cert_pins.remove(0);
        }

        self.cert_pins.push(CertificatePin {
            domain_hash,
            pin_hash,
            expires_at,
            include_subdomains,
        });

        serial_println!(
            "[NET_SEC] Pinned certificate for domain {:016X}",
            domain_hash
        );
    }

    // ── Secure context ─────────────────────────────────────────────────

    /// Determine if the current context is considered "secure"
    /// per the W3C Secure Contexts specification.
    ///
    /// `is_https` — page loaded over HTTPS
    /// `is_localhost` — page loaded from localhost / 127.0.0.1
    /// `parent_is_secure` — parent frame is secure (for nested contexts)
    pub fn is_secure_context(
        &self,
        is_https: bool,
        is_localhost: bool,
        parent_is_secure: bool,
    ) -> bool {
        // Localhost is always a secure context
        if is_localhost {
            return true;
        }
        // Must be HTTPS and parent must also be secure
        is_https && parent_is_secure
    }

    // ── Tracking protection ────────────────────────────────────────────

    /// Check if a request should be blocked as a tracking request.
    ///
    /// `domain_hash` — hash of the domain being requested
    ///
    /// Returns true if the request should be blocked.
    pub fn block_tracking_request(&mut self, domain_hash: u64) -> bool {
        if !self.tracking_protection {
            return false;
        }

        for &tracker in &self.tracker_hashes {
            if tracker == domain_hash {
                self.blocked_requests = self.blocked_requests.saturating_add(1);
                serial_println!("[NET_SEC] Blocked tracking request to {:016X}", domain_hash);
                return true;
            }
        }
        false
    }

    /// Add a domain hash to the tracker blocklist
    pub fn add_tracker(&mut self, domain_hash: u64) {
        if !self.tracker_hashes.contains(&domain_hash) {
            self.tracker_hashes.push(domain_hash);
        }
    }

    /// Remove a domain hash from the tracker blocklist
    pub fn remove_tracker(&mut self, domain_hash: u64) {
        self.tracker_hashes.retain(|&h| h != domain_hash);
    }

    // ── Security score ─────────────────────────────────────────────────

    /// Calculate an overall security score for the current browsing context.
    ///
    /// Returns a Q16 fixed-point value where 100 * 65536 is a perfect score.
    ///
    /// `is_https` — page is HTTPS
    /// `domain_hash` — domain hash (to check HSTS / pinning)
    /// `has_mixed` — mixed content was detected
    /// `now` — current timestamp for HSTS expiry checks
    pub fn get_security_score(
        &self,
        is_https: bool,
        domain_hash: u64,
        has_mixed: bool,
        now: u64,
    ) -> i32 {
        let mut score: i32 = 0;

        // HTTPS
        if is_https {
            score += SCORE_HTTPS;
        }

        // HSTS
        if self.check_hsts(domain_hash, 0, now) {
            score += SCORE_HSTS;
        }

        // CSP active
        if !self.csp_policies.is_empty() {
            score += SCORE_CSP;
        }

        // CORS policies present
        if !self.cors_cache.is_empty() {
            score += SCORE_CORS;
        }

        // Certificate pinned
        let pinned = self.cert_pins.iter().any(|p| p.domain_hash == domain_hash);
        if pinned {
            score += SCORE_CERT_PINNED;
        }

        // No mixed content
        if !has_mixed {
            score += SCORE_NO_MIXED;
        }

        // Tracking protection active
        if self.tracking_protection {
            score += SCORE_NO_TRACKING;
        }

        score
    }

    // ── Redirect safety ────────────────────────────────────────────────

    /// Analyze a redirect chain for safety concerns.
    ///
    /// `chain` — sequence of (is_https, domain_hash) tuples representing
    ///           the redirect chain from initial request to final destination.
    ///
    /// Returns the first safety issue found, or `Safe`.
    pub fn check_redirect_safety(&self, chain: &[(bool, u64)]) -> RedirectSafety {
        // Check chain length
        if chain.len() as u32 > MAX_REDIRECT_DEPTH {
            return RedirectSafety::TooManyRedirects;
        }

        for i in 1..chain.len() {
            let (prev_https, _prev_domain) = chain[i - 1];
            let (cur_https, cur_domain) = chain[i];

            // HTTPS -> HTTP downgrade
            if prev_https && !cur_https {
                return RedirectSafety::HttpsDowngrade;
            }

            // Redirect to a known tracker
            if self.tracking_protection {
                for &tracker in &self.tracker_hashes {
                    if tracker == cur_domain {
                        return RedirectSafety::TrackerRedirect;
                    }
                }
            }
        }

        RedirectSafety::Safe
    }

    // ── Maintenance ────────────────────────────────────────────────────

    /// Evict expired HSTS entries and certificate pins
    pub fn evict_expired(&mut self, now: u64) {
        let before_hsts = self.hsts_domains.len();
        self.hsts_domains.retain(|e| !e.is_expired(now));
        let evicted_hsts = before_hsts - self.hsts_domains.len();

        let before_pins = self.cert_pins.len();
        self.cert_pins.retain(|p| p.expires_at > now);
        let evicted_pins = before_pins - self.cert_pins.len();

        if evicted_hsts > 0 || evicted_pins > 0 {
            serial_println!(
                "[NET_SEC] Evicted {} HSTS entries and {} cert pins",
                evicted_hsts,
                evicted_pins
            );
        }
    }

    /// Get statistics snapshot
    pub fn stats(&self) -> NetworkSecStats {
        NetworkSecStats {
            blocked_requests: self.blocked_requests,
            warnings_issued: self.warnings_issued,
            cors_cache_entries: self.cors_cache.len() as u32,
            cors_cache_hits: self.cors_cache_hits,
            cors_cache_misses: self.cors_cache_misses,
            csp_policies_active: self.csp_policies.len() as u32,
            hsts_domains_count: self.hsts_domains.len() as u32,
            cert_pins_count: self.cert_pins.len() as u32,
            tracker_hashes_count: self.tracker_hashes.len() as u32,
            https_enforced: self.https_enforced,
            tracking_protection: self.tracking_protection,
        }
    }
}

/// Snapshot of network security statistics
#[derive(Debug, Clone)]
pub struct NetworkSecStats {
    pub blocked_requests: u64,
    pub warnings_issued: u64,
    pub cors_cache_entries: u32,
    pub cors_cache_hits: u64,
    pub cors_cache_misses: u64,
    pub csp_policies_active: u32,
    pub hsts_domains_count: u32,
    pub cert_pins_count: u32,
    pub tracker_hashes_count: u32,
    pub https_enforced: bool,
    pub tracking_protection: bool,
}

// ── Public API (lock-guarded) ──────────────────────────────────────────────

/// Initialize the browser network security engine
pub fn init() {
    let mut engine = NET_SEC_ENGINE.lock();
    *engine = Some(NetworkSecEngine::new());
    serial_println!("[NET_SEC] Browser network security engine initialized");
    serial_println!("[NET_SEC]   HTTPS enforced: true");
    serial_println!("[NET_SEC]   Tracking protection: true");
    serial_println!("[NET_SEC]   Mixed content (active): Block");
    serial_println!("[NET_SEC]   Mixed content (passive): Block");
    serial_println!("[NET_SEC]   CORS cache capacity: {}", MAX_CORS_CACHE);
    serial_println!("[NET_SEC]   HSTS capacity: {}", MAX_HSTS_DOMAINS);
    serial_println!("[NET_SEC]   Cert pin capacity: {}", MAX_CERT_PINS);
    serial_println!("[NET_SEC]   Known tracker hashes: 8");
}

/// Perform a CORS check through the global engine
pub fn check_cors(
    target_origin: u64,
    requesting_origin: u64,
    method_bit: u8,
    header_hashes: &[u64],
) -> CorsResult {
    let mut engine = NET_SEC_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.check_cors(target_origin, requesting_origin, method_bit, header_hashes),
        None => CorsResult::NoPolicyFound,
    }
}

/// Enforce CSP through the global engine
pub fn enforce_csp(directive: CspDirective, source_hash: u64) -> CspResult {
    let mut engine = NET_SEC_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.enforce_csp(directive, source_hash),
        None => CspResult::NoPolicyActive,
    }
}

/// Check mixed content through the global engine
pub fn check_mixed_content(
    page_is_https: bool,
    resource_is_https: bool,
    content_type: MixedContentType,
) -> MixedContentAction {
    let mut engine = NET_SEC_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.check_mixed_content(page_is_https, resource_is_https, content_type),
        None => MixedContentAction::Block,
    }
}

/// Verify a certificate through the global engine
pub fn verify_certificate(
    domain_hash: u64,
    cert_status: CertificateStatus,
    cert_pubkey_hash: u64,
) -> bool {
    let mut engine = NET_SEC_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.verify_certificate(domain_hash, cert_status, cert_pubkey_hash),
        None => false,
    }
}

/// Add an HSTS entry through the global engine
pub fn add_hsts(domain_hash: u64, max_age: u64, include_subdomains: bool, now: u64) {
    let mut engine = NET_SEC_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.add_hsts(domain_hash, max_age, include_subdomains, now);
    }
}

/// Check HSTS through the global engine
pub fn check_hsts(domain_hash: u64, parent_domain_hash: u64, now: u64) -> bool {
    let engine = NET_SEC_ENGINE.lock();
    match engine.as_ref() {
        Some(e) => e.check_hsts(domain_hash, parent_domain_hash, now),
        None => false,
    }
}

/// Pin a certificate through the global engine
pub fn pin_certificate(domain_hash: u64, pin_hash: u64, expires_at: u64, include_subdomains: bool) {
    let mut engine = NET_SEC_ENGINE.lock();
    if let Some(e) = engine.as_mut() {
        e.pin_certificate(domain_hash, pin_hash, expires_at, include_subdomains);
    }
}

/// Check secure context through the global engine
pub fn is_secure_context(is_https: bool, is_localhost: bool, parent_is_secure: bool) -> bool {
    let engine = NET_SEC_ENGINE.lock();
    match engine.as_ref() {
        Some(e) => e.is_secure_context(is_https, is_localhost, parent_is_secure),
        None => false,
    }
}

/// Block tracking request through the global engine
pub fn block_tracking_request(domain_hash: u64) -> bool {
    let mut engine = NET_SEC_ENGINE.lock();
    match engine.as_mut() {
        Some(e) => e.block_tracking_request(domain_hash),
        None => false,
    }
}

/// Get security score through the global engine
pub fn get_security_score(is_https: bool, domain_hash: u64, has_mixed: bool, now: u64) -> i32 {
    let engine = NET_SEC_ENGINE.lock();
    match engine.as_ref() {
        Some(e) => e.get_security_score(is_https, domain_hash, has_mixed, now),
        None => 0,
    }
}

/// Check redirect safety through the global engine
pub fn check_redirect_safety(chain: &[(bool, u64)]) -> RedirectSafety {
    let engine = NET_SEC_ENGINE.lock();
    match engine.as_ref() {
        Some(e) => e.check_redirect_safety(chain),
        None => RedirectSafety::Safe,
    }
}

/// Get network security statistics
pub fn stats() -> Option<NetworkSecStats> {
    let engine = NET_SEC_ENGINE.lock();
    engine.as_ref().map(|e| e.stats())
}
