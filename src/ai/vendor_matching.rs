use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
/// AI-powered vendor discovery using embeddings
///
/// Part of the Hoags AI subsystem. Matches vendors to contract
/// requirements via weighted criteria scoring, capability matching,
/// normalization, and threshold-based filtering.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Vendor match result with relevance score
pub struct VendorMatch {
    pub vendor_id: u64,
    pub relevance: f32,
    pub reason: String,
}

/// A vendor profile
#[derive(Clone)]
pub struct VendorProfile {
    pub id: u64,
    pub name: String,
    pub description: String,
    /// Capabilities this vendor offers (capability_name -> proficiency 0.0-1.0)
    pub capabilities: BTreeMap<String, f32>,
    /// Certifications (e.g., ISO9001, SOC2)
    pub certifications: Vec<String>,
    /// Geographic regions served
    pub regions: Vec<String>,
    /// Company size category (1=small, 2=medium, 3=large)
    pub size_category: u8,
    /// Past performance score (0.0 to 1.0)
    pub performance_score: f32,
    /// Hourly rate / cost factor
    pub cost_factor: f32,
    /// Active status
    pub active: bool,
    /// Tags for text-based matching
    pub tags: Vec<String>,
}

impl VendorProfile {
    pub fn new(id: u64, name: &str, description: &str) -> Self {
        VendorProfile {
            id,
            name: String::from(name),
            description: String::from(description),
            capabilities: BTreeMap::new(),
            certifications: Vec::new(),
            regions: Vec::new(),
            size_category: 2,
            performance_score: 0.5,
            cost_factor: 1.0,
            active: true,
            tags: Vec::new(),
        }
    }

    fn with_capabilities(mut self, caps: &[(&str, f32)]) -> Self {
        for &(name, prof) in caps {
            self.capabilities
                .insert(String::from(name), prof.max(0.0).min(1.0));
        }
        self
    }

    fn with_certs(mut self, certs: &[&str]) -> Self {
        for c in certs {
            self.certifications.push(String::from(*c));
        }
        self
    }

    fn with_regions(mut self, regions: &[&str]) -> Self {
        for r in regions {
            self.regions.push(String::from(*r));
        }
        self
    }

    fn with_performance(mut self, score: f32) -> Self {
        self.performance_score = score.max(0.0).min(1.0);
        self
    }

    fn with_cost(mut self, cost: f32) -> Self {
        self.cost_factor = cost;
        self
    }

    fn with_size(mut self, size: u8) -> Self {
        self.size_category = size;
        self
    }

    fn with_tags(mut self, tags: &[&str]) -> Self {
        for t in tags {
            self.tags.push(String::from(*t));
        }
        self
    }
}

/// Requirements specification for vendor matching
#[derive(Clone)]
pub struct Requirements {
    /// Required capabilities with minimum proficiency
    pub required_capabilities: BTreeMap<String, f32>,
    /// Desired capabilities (bonus if present, not required)
    pub desired_capabilities: BTreeMap<String, f32>,
    /// Required certifications
    pub required_certifications: Vec<String>,
    /// Required regions
    pub required_regions: Vec<String>,
    /// Preferred size category (0 = no preference)
    pub preferred_size: u8,
    /// Maximum cost factor
    pub max_cost: Option<f32>,
    /// Minimum performance score
    pub min_performance: f32,
    /// Weight for capability matching (0.0 to 1.0)
    pub weight_capability: f32,
    /// Weight for certification matching
    pub weight_certification: f32,
    /// Weight for region matching
    pub weight_region: f32,
    /// Weight for performance score
    pub weight_performance: f32,
    /// Weight for cost
    pub weight_cost: f32,
    /// Keyword text for text-based matching
    pub keywords: Vec<String>,
    /// Weight for keyword/text matching
    pub weight_text: f32,
}

impl Requirements {
    pub fn new() -> Self {
        Requirements {
            required_capabilities: BTreeMap::new(),
            desired_capabilities: BTreeMap::new(),
            required_certifications: Vec::new(),
            required_regions: Vec::new(),
            preferred_size: 0,
            max_cost: None,
            min_performance: 0.0,
            weight_capability: 0.35,
            weight_certification: 0.15,
            weight_region: 0.1,
            weight_performance: 0.2,
            weight_cost: 0.1,
            keywords: Vec::new(),
            weight_text: 0.1,
        }
    }

    pub fn require_capability(mut self, name: &str, min_proficiency: f32) -> Self {
        self.required_capabilities
            .insert(String::from(name), min_proficiency);
        self
    }

    pub fn desire_capability(mut self, name: &str, weight: f32) -> Self {
        self.desired_capabilities.insert(String::from(name), weight);
        self
    }

    pub fn require_certification(mut self, cert: &str) -> Self {
        self.required_certifications.push(String::from(cert));
        self
    }

    pub fn require_region(mut self, region: &str) -> Self {
        self.required_regions.push(String::from(region));
        self
    }

    pub fn max_cost(mut self, max: f32) -> Self {
        self.max_cost = Some(max);
        self
    }

    pub fn min_performance(mut self, min: f32) -> Self {
        self.min_performance = min;
        self
    }

    pub fn keywords(mut self, kws: &[&str]) -> Self {
        for kw in kws {
            self.keywords.push(String::from(*kw));
        }
        self
    }
}

pub struct VendorMatcher {
    pub embeddings: Vec<Vec<f32>>,
    /// Vendor profiles
    vendors: Vec<VendorProfile>,
    /// Minimum match score threshold (0.0 to 1.0)
    threshold: f32,
    /// Next vendor ID
    next_id: u64,
    /// Total matches performed
    total_matches: u64,
}

impl VendorMatcher {
    pub fn new() -> Self {
        VendorMatcher {
            embeddings: Vec::new(),
            vendors: Vec::new(),
            threshold: 0.2,
            next_id: 1,
            total_matches: 0,
        }
    }

    /// Add a vendor to the registry
    pub fn add_vendor(&mut self, vendor: VendorProfile) {
        if vendor.id >= self.next_id {
            self.next_id = vendor.id + 1;
        }
        // Build a simple embedding from capabilities
        let embedding = self.build_embedding(&vendor);
        self.embeddings.push(embedding);
        self.vendors.push(vendor);
    }

    /// Set the minimum match threshold
    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold.max(0.0).min(1.0);
    }

    /// Find top-N vendors matching the given requirement text
    pub fn find_matches(&self, query: &str, top_n: usize) -> Vec<VendorMatch> {
        if self.vendors.is_empty() {
            return Vec::new();
        }

        // Build requirements from query text
        let reqs = self.requirements_from_query(query);
        self.match_requirements(&reqs, top_n)
    }

    /// Match vendors against structured requirements
    pub fn match_requirements(&self, reqs: &Requirements, top_n: usize) -> Vec<VendorMatch> {
        let mut scored: Vec<(u64, f32, String)> = Vec::new();

        for vendor in &self.vendors {
            if !vendor.active {
                continue;
            }

            // Hard filter: required capabilities
            let mut meets_requirements = true;
            for (cap_name, &min_prof) in &reqs.required_capabilities {
                let vendor_prof = vendor.capabilities.get(cap_name).copied().unwrap_or(0.0);
                if vendor_prof < min_prof {
                    meets_requirements = false;
                    break;
                }
            }
            if !meets_requirements {
                continue;
            }

            // Hard filter: required certifications
            if !reqs.required_certifications.is_empty() {
                let has_all_certs = reqs
                    .required_certifications
                    .iter()
                    .all(|cert| vendor.certifications.iter().any(|vc| vc == cert));
                if !has_all_certs {
                    continue;
                }
            }

            // Hard filter: required regions
            if !reqs.required_regions.is_empty() {
                let serves_region = reqs
                    .required_regions
                    .iter()
                    .any(|region| vendor.regions.iter().any(|vr| vr == region));
                if !serves_region {
                    continue;
                }
            }

            // Hard filter: max cost
            if let Some(max_cost) = reqs.max_cost {
                if vendor.cost_factor > max_cost {
                    continue;
                }
            }

            // Hard filter: min performance
            if vendor.performance_score < reqs.min_performance {
                continue;
            }

            // Score: capability matching
            let cap_score = self.score_capabilities(vendor, reqs);

            // Score: certification bonus
            let cert_score = if reqs.required_certifications.is_empty() {
                // Bonus for having any certifications
                (vendor.certifications.len() as f32 * 0.2).min(1.0)
            } else {
                1.0 // Already passed hard filter
            };

            // Score: region match
            let region_score = if reqs.required_regions.is_empty() {
                (vendor.regions.len() as f32 * 0.15).min(1.0)
            } else {
                let matching = reqs
                    .required_regions
                    .iter()
                    .filter(|r| vendor.regions.iter().any(|vr| vr == *r))
                    .count();
                matching as f32 / reqs.required_regions.len() as f32
            };

            // Score: performance
            let perf_score = vendor.performance_score;

            // Score: cost (inverse - lower cost is better)
            let max_cost_factor = self
                .vendors
                .iter()
                .map(|v| v.cost_factor)
                .fold(1.0f32, |a, b| if b > a { b } else { a });
            let cost_score = 1.0 - (vendor.cost_factor / max_cost_factor.max(1.0));

            // Score: text/keyword matching
            let text_score = self.score_text_match(vendor, reqs);

            // Weighted composite score
            let total = reqs.weight_capability * cap_score
                + reqs.weight_certification * cert_score
                + reqs.weight_region * region_score
                + reqs.weight_performance * perf_score
                + reqs.weight_cost * cost_score
                + reqs.weight_text * text_score;

            if total >= self.threshold {
                let reason = self
                    .generate_match_reason(vendor, cap_score, cert_score, perf_score, text_score);
                scored.push((vendor.id, total, reason));
            }
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

        // Take top-N
        scored
            .into_iter()
            .take(top_n)
            .map(|(id, score, reason)| VendorMatch {
                vendor_id: id,
                relevance: score,
                reason,
            })
            .collect()
    }

    /// Score capability matching between vendor and requirements
    fn score_capabilities(&self, vendor: &VendorProfile, reqs: &Requirements) -> f32 {
        let mut total_score = 0.0f32;
        let mut total_weight = 0.0f32;

        // Required capabilities
        for (cap, &min_prof) in &reqs.required_capabilities {
            let vendor_prof = vendor.capabilities.get(cap).copied().unwrap_or(0.0);
            let match_quality = if vendor_prof >= min_prof {
                // Bonus for exceeding requirement
                1.0 + (vendor_prof - min_prof) * 0.5
            } else {
                vendor_prof / min_prof.max(0.01)
            };
            total_score += match_quality;
            total_weight += 1.0;
        }

        // Desired capabilities (lower weight)
        for (cap, &weight) in &reqs.desired_capabilities {
            let vendor_prof = vendor.capabilities.get(cap).copied().unwrap_or(0.0);
            total_score += vendor_prof * weight * 0.5;
            total_weight += 0.5;
        }

        if total_weight == 0.0 {
            // No specific capabilities required; score based on breadth
            return (vendor.capabilities.len() as f32 * 0.1).min(1.0);
        }

        (total_score / total_weight).min(1.0)
    }

    /// Score text matching between vendor profile and requirement keywords
    fn score_text_match(&self, vendor: &VendorProfile, reqs: &Requirements) -> f32 {
        if reqs.keywords.is_empty() {
            return 0.0;
        }

        let vendor_text = format!(
            "{} {} {}",
            vendor.name,
            vendor.description,
            vendor.tags.join(" ")
        );
        let vendor_lower = vendor_text.to_lowercase();

        let matched = reqs
            .keywords
            .iter()
            .filter(|kw| vendor_lower.contains(kw.as_str()))
            .count();

        matched as f32 / reqs.keywords.len() as f32
    }

    /// Build requirements from a natural language query
    fn requirements_from_query(&self, query: &str) -> Requirements {
        let mut reqs = Requirements::new();
        let lower = query.to_lowercase();

        // Extract capability keywords from query
        let all_capabilities: Vec<String> = self
            .vendors
            .iter()
            .flat_map(|v| v.capabilities.keys().cloned())
            .collect();

        for cap in &all_capabilities {
            if lower.contains(cap.as_str()) {
                reqs.required_capabilities.insert(cap.clone(), 0.3);
            }
        }

        // Extract certification keywords
        let all_certs: Vec<String> = self
            .vendors
            .iter()
            .flat_map(|v| v.certifications.iter().cloned())
            .collect();
        for cert in &all_certs {
            if lower.contains(&cert.to_lowercase()) {
                reqs.required_certifications.push(cert.clone());
            }
        }

        // Extract region keywords
        let all_regions: Vec<String> = self
            .vendors
            .iter()
            .flat_map(|v| v.regions.iter().cloned())
            .collect();
        for region in &all_regions {
            if lower.contains(&region.to_lowercase()) {
                reqs.required_regions.push(region.clone());
            }
        }

        // Extract general keywords
        for word in lower.split(|c: char| !c.is_alphanumeric()) {
            if word.len() >= 3 {
                reqs.keywords.push(String::from(word));
            }
        }

        // Cost sensitivity
        if lower.contains("cheap") || lower.contains("budget") || lower.contains("affordable") {
            reqs.weight_cost = 0.3;
        }
        if lower.contains("quality") || lower.contains("best") || lower.contains("premium") {
            reqs.weight_performance = 0.35;
        }

        reqs
    }

    /// Build a simple embedding vector from vendor capabilities
    fn build_embedding(&self, vendor: &VendorProfile) -> Vec<f32> {
        // Use a fixed set of dimensions based on common capability names
        let dimensions = [
            "software",
            "hardware",
            "consulting",
            "design",
            "manufacturing",
            "engineering",
            "construction",
            "logistics",
            "security",
            "training",
            "support",
            "maintenance",
            "research",
            "analytics",
            "cloud",
            "networking",
            "database",
            "mobile",
            "web",
            "ai",
        ];

        dimensions
            .iter()
            .map(|dim| vendor.capabilities.get(*dim).copied().unwrap_or(0.0))
            .collect()
    }

    /// Generate a human-readable match reason
    fn generate_match_reason(
        &self,
        vendor: &VendorProfile,
        cap_score: f32,
        cert_score: f32,
        perf_score: f32,
        _text_score: f32,
    ) -> String {
        let mut reasons = Vec::new();

        if cap_score > 0.7 {
            let top_caps: Vec<&String> = vendor
                .capabilities
                .iter()
                .filter(|(_, &v)| v > 0.5)
                .map(|(k, _)| k)
                .take(3)
                .collect();
            if !top_caps.is_empty() {
                reasons.push(format!(
                    "Strong capabilities: {}",
                    top_caps
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        if cert_score > 0.5 && !vendor.certifications.is_empty() {
            reasons.push(format!("Certified: {}", vendor.certifications.join(", ")));
        }

        if perf_score > 0.7 {
            reasons.push(format!("High performance: {:.0}%", perf_score * 100.0));
        }

        if vendor.cost_factor < 1.0 {
            reasons.push(String::from("Competitive pricing"));
        }

        if reasons.is_empty() {
            format!("Vendor: {}", vendor.name)
        } else {
            reasons.join("; ")
        }
    }

    /// Get a vendor by ID
    pub fn get_vendor(&self, id: u64) -> Option<&VendorProfile> {
        self.vendors.iter().find(|v| v.id == id)
    }

    /// Number of vendors in registry
    pub fn vendor_count(&self) -> usize {
        self.vendors.len()
    }

    /// Total matching operations performed
    pub fn total_matches(&self) -> u64 {
        self.total_matches
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MATCHER: Mutex<Option<VendorMatcher>> = Mutex::new(None);

pub fn init() {
    let mut matcher = VendorMatcher::new();

    // Register sample vendors
    matcher.add_vendor(
        VendorProfile::new(
            1,
            "TechCorp Systems",
            "Enterprise software development and cloud solutions",
        )
        .with_capabilities(&[
            ("software", 0.9),
            ("cloud", 0.8),
            ("web", 0.7),
            ("ai", 0.6),
            ("database", 0.7),
        ])
        .with_certs(&["ISO27001", "SOC2"])
        .with_regions(&["US", "EU"])
        .with_performance(0.85)
        .with_cost(1.2)
        .with_size(3)
        .with_tags(&["enterprise", "software", "cloud", "saas", "devops"]),
    );

    matcher.add_vendor(
        VendorProfile::new(
            2,
            "BuildRight Construction",
            "Commercial construction and facility management",
        )
        .with_capabilities(&[
            ("construction", 0.95),
            ("engineering", 0.7),
            ("design", 0.6),
            ("maintenance", 0.8),
        ])
        .with_certs(&["ISO9001", "OSHA"])
        .with_regions(&["US"])
        .with_performance(0.8)
        .with_cost(1.0)
        .with_size(3)
        .with_tags(&["construction", "building", "facility", "commercial"]),
    );

    matcher.add_vendor(
        VendorProfile::new(
            3,
            "SecureNet Solutions",
            "Cybersecurity consulting and managed security services",
        )
        .with_capabilities(&[
            ("security", 0.95),
            ("consulting", 0.8),
            ("networking", 0.7),
            ("training", 0.6),
        ])
        .with_certs(&["ISO27001", "SOC2", "CISSP"])
        .with_regions(&["US", "EU", "APAC"])
        .with_performance(0.9)
        .with_cost(1.5)
        .with_size(2)
        .with_tags(&[
            "security",
            "cybersecurity",
            "penetration",
            "audit",
            "compliance",
        ]),
    );

    matcher.add_vendor(
        VendorProfile::new(
            4,
            "DataDriven Analytics",
            "Data science and business intelligence",
        )
        .with_capabilities(&[
            ("analytics", 0.9),
            ("ai", 0.85),
            ("database", 0.8),
            ("cloud", 0.6),
            ("research", 0.7),
        ])
        .with_certs(&["SOC2"])
        .with_regions(&["US", "EU"])
        .with_performance(0.82)
        .with_cost(1.3)
        .with_size(2)
        .with_tags(&[
            "data",
            "analytics",
            "ml",
            "ai",
            "visualization",
            "reporting",
        ]),
    );

    matcher.add_vendor(
        VendorProfile::new(
            5,
            "QuickFix IT Support",
            "IT support and hardware maintenance",
        )
        .with_capabilities(&[
            ("support", 0.9),
            ("hardware", 0.8),
            ("networking", 0.7),
            ("maintenance", 0.85),
        ])
        .with_certs(&["CompTIA"])
        .with_regions(&["US"])
        .with_performance(0.75)
        .with_cost(0.7)
        .with_size(1)
        .with_tags(&[
            "it",
            "support",
            "helpdesk",
            "hardware",
            "repair",
            "maintenance",
        ]),
    );

    matcher.add_vendor(
        VendorProfile::new(
            6,
            "DesignWorks Studio",
            "UI/UX design and digital marketing",
        )
        .with_capabilities(&[
            ("design", 0.95),
            ("web", 0.8),
            ("mobile", 0.7),
            ("consulting", 0.5),
        ])
        .with_certs(&[])
        .with_regions(&["US", "EU"])
        .with_performance(0.88)
        .with_cost(1.1)
        .with_size(1)
        .with_tags(&["design", "ui", "ux", "branding", "marketing", "creative"]),
    );

    matcher.next_id = 7;
    let count = matcher.vendor_count();
    *MATCHER.lock() = Some(matcher);
    crate::serial_println!(
        "    [vendor_matching] Vendor matcher ready ({} vendors, weighted criteria scoring)",
        count
    );
}

/// Find vendors matching a query
pub fn find_matches(query: &str, top_n: usize) -> Vec<VendorMatch> {
    MATCHER
        .lock()
        .as_ref()
        .map(|m| m.find_matches(query, top_n))
        .unwrap_or_else(Vec::new)
}
