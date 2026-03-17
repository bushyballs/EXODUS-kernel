use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Node categories for marketplace organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeCategory {
    Productivity,
    Entertainment,
    Health,
    Communication,
    Security,
    Developer,
    System,
    Creative,
    Education,
    Custom,
}

impl NodeCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeCategory::Productivity => "Productivity",
            NodeCategory::Entertainment => "Entertainment",
            NodeCategory::Health => "Health",
            NodeCategory::Communication => "Communication",
            NodeCategory::Security => "Security",
            NodeCategory::Developer => "Developer",
            NodeCategory::System => "System",
            NodeCategory::Creative => "Creative",
            NodeCategory::Education => "Education",
            NodeCategory::Custom => "Custom",
        }
    }
}

/// Marketplace listing for an available AI node.
#[derive(Debug, Clone)]
pub struct MarketListing {
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: u32, // Major.Minor.Patch stored as (major << 16) | (minor << 8) | patch
    pub category: NodeCategory,
    pub capabilities: Vec<NodeCapability>,
    pub trust_score: Q16,
    pub installs: u32,
    pub rating: Q16, // 0.0 to 5.0 in Q16
}

impl MarketListing {
    pub fn new(
        name: &str,
        description: &str,
        author: &str,
        version: u32,
        category: NodeCategory,
        capabilities: Vec<NodeCapability>,
    ) -> Self {
        MarketListing {
            name: String::from(name),
            description: String::from(description),
            author: String::from(author),
            version,
            category,
            capabilities,
            trust_score: Q16_HALF, // Start at 0.5 trust
            installs: 0,
            rating: Q16_ZERO,
        }
    }

    pub fn version_str(&self) -> [u8; 16] {
        let major = (self.version >> 16) & 0xFF;
        let minor = (self.version >> 8) & 0xFF;
        let patch = self.version & 0xFF;
        let mut buf = [0u8; 16];
        let s = alloc::format!("{}.{}.{}", major, minor, patch);
        let bytes = s.as_bytes();
        let len = bytes.len().min(15);
        buf[..len].copy_from_slice(&bytes[..len]);
        buf
    }
}

/// Installed node instance with runtime state.
#[derive(Debug)]
pub struct InstalledNode {
    pub listing: MarketListing,
    pub node_id: u16,
    pub enabled: bool,
    pub signal_filter: Vec<SignalKind>,
    pub local_data_bytes: usize,
    pub crash_count: u32,
    pub stability_score: Q16,
}

impl InstalledNode {
    pub fn new(listing: MarketListing, node_id: u16) -> Self {
        InstalledNode {
            listing,
            node_id,
            enabled: true,
            signal_filter: Vec::new(),
            local_data_bytes: 0,
            crash_count: 0,
            stability_score: Q16_HALF,
        }
    }

    pub fn should_auto_disable(&self) -> bool {
        self.listing.trust_score < Q16_TENTH
    }

    pub fn record_crash(&mut self) {
        self.crash_count = self.crash_count.saturating_add(1);
        // Trust penalty: each crash reduces trust by ~0.05
        let penalty = Q16_ONE / 20;
        self.listing.trust_score = self.listing.trust_score.saturating_sub(penalty);
        if self.should_auto_disable() {
            self.enabled = false;
        }
    }

    pub fn record_success(&mut self) {
        // Successful operation increases trust slightly
        let boost = Q16_ONE / 100; // +0.01 per success
        self.listing.trust_score = self.listing.trust_score.saturating_add(boost);
        if self.listing.trust_score > Q16_ONE {
            self.listing.trust_score = Q16_ONE;
        }
    }
}

/// The neural node marketplace — manages app plugins and custom AI nodes.
pub struct NeuralMarket {
    available: Vec<MarketListing>,
    installed: BTreeMap<String, InstalledNode>,
    trust_threshold: Q16,
    max_installed: usize,
    total_installs: u64,
    next_node_id: u16,
}

impl NeuralMarket {
    pub fn new() -> Self {
        NeuralMarket {
            available: Vec::new(),
            installed: BTreeMap::new(),
            trust_threshold: Q16_HALF, // 0.5 required to auto-install
            max_installed: 32,
            total_installs: 0,
            next_node_id: 1000,
        }
    }

    /// Initialize the marketplace with built-in catalog.
    pub fn init_catalog(&mut self) {
        // Weather Predictor Node
        let weather = MarketListing::new(
            "Weather Predictor",
            "Predicts local weather using neural patterns",
            "HoagsOS",
            0x010000, // 1.0.0
            NodeCategory::Productivity,
            alloc::vec![
                NodeCapability::SignalProcessing,
                NodeCapability::DataIntegration,
            ],
        );
        self.available.push(weather);

        // Music Mood Node
        let music = MarketListing::new(
            "Music Mood",
            "Recommends music based on mood signals",
            "HoagsOS",
            0x010100, // 1.1.0
            NodeCategory::Entertainment,
            alloc::vec![NodeCapability::SignalProcessing],
        );
        self.available.push(music);

        // Fitness Tracker Node
        let fitness = MarketListing::new(
            "Fitness Tracker",
            "Analyzes and predicts fitness patterns",
            "HoagsOS",
            0x010200, // 1.2.0
            NodeCategory::Health,
            alloc::vec![
                NodeCapability::DataIntegration,
                NodeCapability::SignalProcessing,
            ],
        );
        self.available.push(fitness);

        // Code Assistant Node
        let assistant = MarketListing::new(
            "Code Assistant",
            "Helps with code completion and debugging",
            "HoagsOS",
            0x020000, // 2.0.0
            NodeCategory::Developer,
            alloc::vec![
                NodeCapability::SignalProcessing,
                NodeCapability::ContextRetention,
            ],
        );
        self.available.push(assistant);

        // Smart Home Controller
        let smart_home = MarketListing::new(
            "Smart Home",
            "Coordinates smart home automation",
            "HoagsOS",
            0x010300, // 1.3.0
            NodeCategory::System,
            alloc::vec![
                NodeCapability::SignalProcessing,
                NodeCapability::DataIntegration,
            ],
        );
        self.available.push(smart_home);

        // Battery Optimizer
        let battery = MarketListing::new(
            "Battery Optimizer",
            "Optimizes power consumption patterns",
            "HoagsOS",
            0x010000, // 1.0.0
            NodeCategory::System,
            alloc::vec![NodeCapability::SignalProcessing],
        );
        self.available.push(battery);
    }

    /// Install a node from the marketplace.
    pub fn install_node(&mut self, listing_name: &str) -> Result<u16, &'static str> {
        // Verify not already installed
        if self.installed.contains_key(listing_name) {
            return Err("Node already installed");
        }

        // Check capacity
        if self.installed.len() >= self.max_installed {
            return Err("Market capacity exceeded");
        }

        // Find listing
        let listing = self
            .available
            .iter()
            .find(|m| m.name == listing_name)
            .ok_or("Listing not found")?
            .clone();

        // Check trust
        if listing.trust_score < self.trust_threshold && listing.category == NodeCategory::Custom {
            return Err("Trust threshold not met");
        }

        // Create node instance
        let node_id = self.next_node_id;
        self.next_node_id = self.next_node_id.saturating_add(1);

        let mut installed = InstalledNode::new(listing, node_id);
        installed
            .signal_filter
            .push(SignalKind::EnvironmentContext);
        installed
            .signal_filter
            .push(SignalKind::UserIntent);

        self.installed.insert(String::from(listing_name), installed);
        self.total_installs = self.total_installs.saturating_add(1);

        Ok(node_id)
    }

    /// Uninstall a node.
    pub fn uninstall_node(&mut self, name: &str) -> Result<(), &'static str> {
        self.installed.remove(name).ok_or("Node not found")?;
        Ok(())
    }

    /// Enable a node.
    pub fn enable_node(&mut self, name: &str) -> Result<(), &'static str> {
        let node = self
            .installed
            .get_mut(name)
            .ok_or("Node not found")?;
        node.enabled = true;
        Ok(())
    }

    /// Disable a node.
    pub fn disable_node(&mut self, name: &str) -> Result<(), &'static str> {
        let node = self
            .installed
            .get_mut(name)
            .ok_or("Node not found")?;
        node.enabled = false;
        Ok(())
    }

    /// Update trust score for a node.
    pub fn update_trust(&mut self, name: &str, delta: Q16) -> Result<(), &'static str> {
        let node = self
            .installed
            .get_mut(name)
            .ok_or("Node not found")?;

        if delta >= Q16_ZERO {
            node.listing.trust_score = node.listing.trust_score.saturating_add(delta);
            if node.listing.trust_score > Q16_ONE {
                node.listing.trust_score = Q16_ONE;
            }
        } else {
            node.listing.trust_score = node.listing.trust_score.saturating_sub(delta.abs());
        }

        if node.should_auto_disable() {
            node.enabled = false;
        }

        Ok(())
    }

    /// Get list of available nodes.
    pub fn list_available(&self) -> &[MarketListing] {
        &self.available
    }

    /// Get list of installed nodes.
    pub fn list_installed(&self) -> Vec<(String, u16, bool, Q16)> {
        self.installed
            .iter()
            .map(|(name, node)| {
                (
                    name.clone(),
                    node.node_id,
                    node.enabled,
                    node.listing.trust_score,
                )
            })
            .collect()
    }

    /// Get installed node by name.
    pub fn get_installed(&self, name: &str) -> Option<&InstalledNode> {
        self.installed.get(name)
    }

    /// Get mutable reference to installed node.
    pub fn get_installed_mut(&mut self, name: &str) -> Option<&mut InstalledNode> {
        self.installed.get_mut(name)
    }

    /// Count installed nodes.
    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    /// Get total install count across all nodes.
    pub fn total_installs_count(&self) -> u64 {
        self.total_installs
    }

    /// Set trust threshold for auto-install.
    pub fn set_trust_threshold(&mut self, threshold: Q16) {
        self.trust_threshold = threshold;
    }

    /// Set maximum installed nodes.
    pub fn set_max_installed(&mut self, max: usize) {
        self.max_installed = max;
    }

    /// Query nodes by category.
    pub fn query_by_category(&self, category: NodeCategory) -> Vec<&MarketListing> {
        self.available
            .iter()
            .filter(|m| m.category == category)
            .collect()
    }

    /// Get recommendation score for a node (combines trust and rating).
    pub fn recommendation_score(&self, listing_name: &str) -> Option<Q16> {
        self.available
            .iter()
            .find(|m| m.name == listing_name)
            .map(|listing| {
                let trust_weight = 7; // 70%
                let rating_weight = 3; // 30% (normalized 0-1)
                let normalized_rating = listing.rating / Q16::from_int(5);
                (listing.trust_score.saturating_mul(Q16::from_int(trust_weight))
                    + normalized_rating.saturating_mul(Q16::from_int(rating_weight)))
                    / Q16::from_int(10)
            })
    }

    /// Perform health check on installed nodes.
    pub fn health_check(&mut self) -> usize {
        let mut disabled_count = 0;
        for node in self.installed.values_mut() {
            if node.should_auto_disable() && node.enabled {
                node.enabled = false;
                disabled_count += 1;
            }
        }
        disabled_count
    }
}

/// Global market instance with Mutex protection.
static MARKET: Mutex<Option<NeuralMarket>> = Mutex::new(None);

/// Initialize the marketplace.
pub fn init() -> Result<(), &'static str> {
    let mut market_opt = MARKET.lock();
    if market_opt.is_some() {
        return Err("Market already initialized");
    }

    let mut market = NeuralMarket::new();
    market.init_catalog();
    *market_opt = Some(market);

    serial_println!("[Neural Market] Initialized with {} listings", 6);
    Ok(())
}

/// Install a node globally.
pub fn install(listing_name: &str) -> Result<u16, &'static str> {
    let mut market_opt = MARKET.lock();
    let market = market_opt.as_mut().ok_or("Market not initialized")?;
    let node_id = market.install_node(listing_name)?;
    serial_println!("[Market] Installed {} as node_id {}", listing_name, node_id);
    Ok(node_id)
}

/// Uninstall a node globally.
pub fn uninstall(name: &str) -> Result<(), &'static str> {
    let mut market_opt = MARKET.lock();
    let market = market_opt.as_mut().ok_or("Market not initialized")?;
    market.uninstall_node(name)?;
    serial_println!("[Market] Uninstalled {}", name);
    Ok(())
}

/// Enable a node globally.
pub fn enable(name: &str) -> Result<(), &'static str> {
    let mut market_opt = MARKET.lock();
    let market = market_opt.as_mut().ok_or("Market not initialized")?;
    market.enable_node(name)?;
    serial_println!("[Market] Enabled {}", name);
    Ok(())
}

/// Disable a node globally.
pub fn disable(name: &str) -> Result<(), &'static str> {
    let mut market_opt = MARKET.lock();
    let market = market_opt.as_mut().ok_or("Market not initialized")?;
    market.disable_node(name)?;
    serial_println!("[Market] Disabled {}", name);
    Ok(())
}

/// List available nodes.
pub fn list_available() -> Vec<(String, NodeCategory, Q16)> {
    let market_opt = MARKET.lock();
    match market_opt.as_ref() {
        Some(market) => {
            market
                .list_available()
                .iter()
                .map(|listing| {
                    (
                        listing.name.clone(),
                        listing.category,
                        listing.trust_score,
                    )
                })
                .collect()
        }
        None => Vec::new(),
    }
}

/// List installed nodes.
pub fn list_installed() -> Vec<(String, u16, bool, Q16)> {
    let market_opt = MARKET.lock();
    match market_opt.as_ref() {
        Some(market) => market.list_installed(),
        None => Vec::new(),
    }
}

/// Update trust for a node.
pub fn update_trust(name: &str, delta: Q16) -> Result<(), &'static str> {
    let mut market_opt = MARKET.lock();
    let market = market_opt.as_mut().ok_or("Market not initialized")?;
    market.update_trust(name, delta)?;
    serial_println!(
        "[Market] Updated trust for {} by {:?}",
        name,
        delta.to_f32()
    );
    Ok(())
}

/// Run health check on all installed nodes.
pub fn perform_health_check() -> usize {
    let mut market_opt = MARKET.lock();
    match market_opt.as_mut() {
        Some(market) => {
            let disabled = market.health_check();
            if disabled > 0 {
                serial_println!("[Market] Health check disabled {} nodes", disabled);
            }
            disabled
        }
        None => 0,
    }
}

/// Get installed node count.
pub fn installed_count() -> usize {
    let market_opt = MARKET.lock();
    match market_opt.as_ref() {
        Some(market) => market.installed_count(),
        None => 0,
    }
}

/// Get total install count.
pub fn total_installs() -> u64 {
    let market_opt = MARKET.lock();
    match market_opt.as_ref() {
        Some(market) => market.total_installs_count(),
        None => 0,
    }
}
