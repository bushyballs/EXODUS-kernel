/// Disk quota management for Genesis
///
/// Implements per-user and per-group disk usage quotas with:
///   - Soft limits (warning) and hard limits (enforced)
///   - Grace periods for soft limit violations
///   - Real-time usage tracking (blocks and inodes)
///   - Warning notifications via serial console
///   - Quota reporting and administration
///
/// Quota enforcement is checked on every block/inode allocation.
/// Grace periods allow temporary soft-limit overages — once expired,
/// the soft limit becomes a hard limit.
///
/// Inspired by: Linux disk quotas (quota.h), XFS project quotas. All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Quota types
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QuotaType {
    User,
    Group,
}

/// Quota limit values (0 = unlimited)
#[derive(Debug, Clone, Copy)]
pub struct QuotaLimits {
    /// Soft limit on disk blocks (in KB)
    pub block_soft: u64,
    /// Hard limit on disk blocks (in KB)
    pub block_hard: u64,
    /// Soft limit on inodes (file count)
    pub inode_soft: u64,
    /// Hard limit on inodes (file count)
    pub inode_hard: u64,
}

impl QuotaLimits {
    pub const fn unlimited() -> Self {
        QuotaLimits {
            block_soft: 0,
            block_hard: 0,
            inode_soft: 0,
            inode_hard: 0,
        }
    }

    pub fn new(block_soft: u64, block_hard: u64, inode_soft: u64, inode_hard: u64) -> Self {
        QuotaLimits {
            block_soft,
            block_hard,
            inode_soft,
            inode_hard,
        }
    }
}

/// Current usage for a quota subject
#[derive(Debug, Clone, Copy)]
pub struct QuotaUsage {
    /// Current disk blocks used (in KB)
    pub blocks_used: u64,
    /// Current inodes used
    pub inodes_used: u64,
}

impl QuotaUsage {
    pub const fn zero() -> Self {
        QuotaUsage {
            blocks_used: 0,
            inodes_used: 0,
        }
    }
}

/// Grace period state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraceState {
    /// Within limits, no grace period active
    Normal,
    /// Over soft limit, grace period running (timestamp when grace expires)
    GracePeriod { expires: u64 },
    /// Grace period expired, soft limit now enforced as hard limit
    Expired,
}

/// Complete quota entry for a user or group
#[derive(Debug, Clone, Copy)]
pub struct QuotaEntry {
    pub quota_type: QuotaType,
    pub id: u32,
    pub limits: QuotaLimits,
    pub usage: QuotaUsage,
    pub block_grace: GraceState,
    pub inode_grace: GraceState,
    /// Number of warnings issued
    pub block_warnings_issued: u32,
    pub inode_warnings_issued: u32,
    /// Maximum warnings before silent enforcement
    pub max_warnings: u32,
}

impl QuotaEntry {
    pub fn new(quota_type: QuotaType, id: u32, limits: QuotaLimits) -> Self {
        QuotaEntry {
            quota_type,
            id,
            limits,
            usage: QuotaUsage::zero(),
            block_grace: GraceState::Normal,
            inode_grace: GraceState::Normal,
            block_warnings_issued: 0,
            inode_warnings_issued: 0,
            max_warnings: 7,
        }
    }
}

/// Result of a quota check
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaCheckResult {
    /// Allocation allowed, within limits
    Allowed,
    /// Allocation allowed, but over soft limit (warning)
    Warning,
    /// Allocation denied, hard limit reached
    Denied,
    /// Allocation denied, grace period expired
    GraceExpired,
}

/// Default grace period: 7 days in seconds (using kernel uptime ticks)
const DEFAULT_BLOCK_GRACE_SECONDS: u64 = 7 * 24 * 3600;
const DEFAULT_INODE_GRACE_SECONDS: u64 = 7 * 24 * 3600;

/// Quota key: (QuotaType, id)
type QuotaKey = (QuotaType, u32);

/// Per-filesystem quota state
pub struct QuotaState {
    /// Active quotas keyed by (type, id)
    entries: BTreeMap<QuotaKey, QuotaEntry>,
    /// Whether quotas are enabled
    pub enabled_user: bool,
    pub enabled_group: bool,
    /// Block grace period duration (seconds)
    pub block_grace_period: u64,
    /// Inode grace period duration (seconds)
    pub inode_grace_period: u64,
    /// Filesystem mount point this applies to
    pub mount_point: String,
}

impl QuotaState {
    pub const fn new() -> Self {
        QuotaState {
            entries: BTreeMap::new(),
            enabled_user: false,
            enabled_group: false,
            block_grace_period: DEFAULT_BLOCK_GRACE_SECONDS,
            inode_grace_period: DEFAULT_INODE_GRACE_SECONDS,
            mount_point: String::new(),
        }
    }

    /// Enable quotas for the given filesystem
    pub fn enable(&mut self, mount_point: &str, user: bool, group: bool) {
        self.mount_point = String::from(mount_point);
        self.enabled_user = user;
        self.enabled_group = group;
        serial_println!(
            "  quota: enabled for {} (user={}, group={})",
            mount_point,
            user,
            group
        );
    }

    /// Disable all quotas
    pub fn disable(&mut self) {
        self.enabled_user = false;
        self.enabled_group = false;
        serial_println!("  quota: disabled for {}", self.mount_point);
    }

    /// Set quota limits for a user or group
    pub fn set_limits(&mut self, quota_type: QuotaType, id: u32, limits: QuotaLimits) {
        let key = (quota_type, id);
        match self.entries.get_mut(&key) {
            Some(entry) => {
                entry.limits = limits;
            }
            None => {
                self.entries
                    .insert(key, QuotaEntry::new(quota_type, id, limits));
            }
        }
    }

    /// Remove quota for a user or group
    pub fn remove_quota(&mut self, quota_type: QuotaType, id: u32) {
        self.entries.remove(&(quota_type, id));
    }

    /// Get current timestamp (simplified: use a counter or kernel uptime)
    fn current_time() -> u64 {
        // In a real kernel, this would read from the RTC or kernel tick counter.
        // Placeholder: return 0 (grace periods won't expire until time source is wired up)
        0
    }

    /// Check if a block allocation is allowed
    pub fn check_block_alloc(
        &mut self,
        quota_type: QuotaType,
        id: u32,
        blocks_to_add: u64,
    ) -> QuotaCheckResult {
        let enabled = match quota_type {
            QuotaType::User => self.enabled_user,
            QuotaType::Group => self.enabled_group,
        };
        if !enabled {
            return QuotaCheckResult::Allowed;
        }

        let key = (quota_type, id);
        let entry = match self.entries.get_mut(&key) {
            Some(e) => e,
            None => return QuotaCheckResult::Allowed, // No quota set = unlimited
        };

        let new_usage = entry.usage.blocks_used + blocks_to_add;

        // Check hard limit
        if entry.limits.block_hard > 0 && new_usage > entry.limits.block_hard {
            return QuotaCheckResult::Denied;
        }

        // Check soft limit
        if entry.limits.block_soft > 0 && new_usage > entry.limits.block_soft {
            let now = Self::current_time();
            match entry.block_grace {
                GraceState::Normal => {
                    // First time exceeding soft limit — start grace period
                    let expires = now + self.block_grace_period;
                    entry.block_grace = GraceState::GracePeriod { expires };
                    Self::issue_block_warning(entry);
                    return QuotaCheckResult::Warning;
                }
                GraceState::GracePeriod { expires } => {
                    if now > expires && expires > 0 {
                        entry.block_grace = GraceState::Expired;
                        return QuotaCheckResult::GraceExpired;
                    }
                    return QuotaCheckResult::Warning;
                }
                GraceState::Expired => {
                    return QuotaCheckResult::GraceExpired;
                }
            }
        } else {
            // Under soft limit — reset grace if it was active
            entry.block_grace = GraceState::Normal;
        }

        QuotaCheckResult::Allowed
    }

    /// Check if an inode allocation is allowed
    pub fn check_inode_alloc(&mut self, quota_type: QuotaType, id: u32) -> QuotaCheckResult {
        let enabled = match quota_type {
            QuotaType::User => self.enabled_user,
            QuotaType::Group => self.enabled_group,
        };
        if !enabled {
            return QuotaCheckResult::Allowed;
        }

        let key = (quota_type, id);
        let entry = match self.entries.get_mut(&key) {
            Some(e) => e,
            None => return QuotaCheckResult::Allowed,
        };

        let new_usage = entry.usage.inodes_used + 1;

        // Check hard limit
        if entry.limits.inode_hard > 0 && new_usage > entry.limits.inode_hard {
            return QuotaCheckResult::Denied;
        }

        // Check soft limit
        if entry.limits.inode_soft > 0 && new_usage > entry.limits.inode_soft {
            let now = Self::current_time();
            match entry.inode_grace {
                GraceState::Normal => {
                    let expires = now + self.inode_grace_period;
                    entry.inode_grace = GraceState::GracePeriod { expires };
                    Self::issue_inode_warning(entry);
                    return QuotaCheckResult::Warning;
                }
                GraceState::GracePeriod { expires } => {
                    if now > expires && expires > 0 {
                        entry.inode_grace = GraceState::Expired;
                        return QuotaCheckResult::GraceExpired;
                    }
                    return QuotaCheckResult::Warning;
                }
                GraceState::Expired => {
                    return QuotaCheckResult::GraceExpired;
                }
            }
        } else {
            entry.inode_grace = GraceState::Normal;
        }

        QuotaCheckResult::Allowed
    }

    /// Record a block allocation (after check passed)
    pub fn charge_blocks(&mut self, quota_type: QuotaType, id: u32, blocks: u64) {
        let key = (quota_type, id);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.usage.blocks_used += blocks;
        }
    }

    /// Record block deallocation
    pub fn release_blocks(&mut self, quota_type: QuotaType, id: u32, blocks: u64) {
        let key = (quota_type, id);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.usage.blocks_used = entry.usage.blocks_used.saturating_sub(blocks);
            // Check if we're back under soft limit
            if entry.limits.block_soft > 0 && entry.usage.blocks_used <= entry.limits.block_soft {
                entry.block_grace = GraceState::Normal;
                entry.block_warnings_issued = 0;
            }
        }
    }

    /// Record an inode allocation
    pub fn charge_inode(&mut self, quota_type: QuotaType, id: u32) {
        let key = (quota_type, id);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.usage.inodes_used = entry.usage.inodes_used.saturating_add(1);
        }
    }

    /// Record inode deallocation
    pub fn release_inode(&mut self, quota_type: QuotaType, id: u32) {
        let key = (quota_type, id);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.usage.inodes_used = entry.usage.inodes_used.saturating_sub(1);
            if entry.limits.inode_soft > 0 && entry.usage.inodes_used <= entry.limits.inode_soft {
                entry.inode_grace = GraceState::Normal;
                entry.inode_warnings_issued = 0;
            }
        }
    }

    /// Issue a block limit warning
    fn issue_block_warning(entry: &mut QuotaEntry) {
        if entry.block_warnings_issued < entry.max_warnings {
            let type_str = match entry.quota_type {
                QuotaType::User => "user",
                QuotaType::Group => "group",
            };
            serial_println!(
                "  quota: WARNING: {} {} exceeded block soft limit ({} KB / {} KB)",
                type_str,
                entry.id,
                entry.usage.blocks_used,
                entry.limits.block_soft
            );
            entry.block_warnings_issued = entry.block_warnings_issued.saturating_add(1);
        }
    }

    /// Issue an inode limit warning
    fn issue_inode_warning(entry: &mut QuotaEntry) {
        if entry.inode_warnings_issued < entry.max_warnings {
            let type_str = match entry.quota_type {
                QuotaType::User => "user",
                QuotaType::Group => "group",
            };
            serial_println!(
                "  quota: WARNING: {} {} exceeded inode soft limit ({} / {})",
                type_str,
                entry.id,
                entry.usage.inodes_used,
                entry.limits.inode_soft
            );
            entry.inode_warnings_issued = entry.inode_warnings_issued.saturating_add(1);
        }
    }

    /// Get quota info for a user or group
    pub fn get_quota(&self, quota_type: QuotaType, id: u32) -> Option<&QuotaEntry> {
        self.entries.get(&(quota_type, id))
    }

    /// Get all active quota entries
    pub fn list_quotas(&self) -> Vec<&QuotaEntry> {
        self.entries.values().collect()
    }

    /// Generate a quota report
    pub fn report(&self) -> Vec<QuotaReport> {
        let mut reports = Vec::new();

        for ((_, _), entry) in &self.entries {
            let block_pct = if entry.limits.block_hard > 0 {
                // Q16 fixed-point percentage: (used * 100 * 65536) / hard
                let numerator = entry.usage.blocks_used as i64 * 100 * 65536;
                let denominator = entry.limits.block_hard as i64;
                if denominator > 0 {
                    (numerator / denominator) as i32
                } else {
                    0i32
                }
            } else {
                0i32
            };

            let inode_pct = if entry.limits.inode_hard > 0 {
                let numerator = entry.usage.inodes_used as i64 * 100 * 65536;
                let denominator = entry.limits.inode_hard as i64;
                if denominator > 0 {
                    (numerator / denominator) as i32
                } else {
                    0i32
                }
            } else {
                0i32
            };

            reports.push(QuotaReport {
                quota_type: entry.quota_type,
                id: entry.id,
                blocks_used: entry.usage.blocks_used,
                block_soft: entry.limits.block_soft,
                block_hard: entry.limits.block_hard,
                block_pct_q16: block_pct,
                block_grace: entry.block_grace,
                inodes_used: entry.usage.inodes_used,
                inode_soft: entry.limits.inode_soft,
                inode_hard: entry.limits.inode_hard,
                inode_pct_q16: inode_pct,
                inode_grace: entry.inode_grace,
            });
        }

        reports
    }
}

/// Quota report entry for display
#[derive(Debug, Clone)]
pub struct QuotaReport {
    pub quota_type: QuotaType,
    pub id: u32,
    pub blocks_used: u64,
    pub block_soft: u64,
    pub block_hard: u64,
    /// Block usage percentage in Q16 fixed-point (65536 = 100%)
    pub block_pct_q16: i32,
    pub block_grace: GraceState,
    pub inodes_used: u64,
    pub inode_soft: u64,
    pub inode_hard: u64,
    /// Inode usage percentage in Q16 fixed-point
    pub inode_pct_q16: i32,
    pub inode_grace: GraceState,
}

/// Global quota state
static QUOTA_STATE: Mutex<QuotaState> = Mutex::new(QuotaState::new());

/// Enable quotas on a filesystem
pub fn enable(mount_point: &str, user: bool, group: bool) {
    QUOTA_STATE.lock().enable(mount_point, user, group);
}

/// Disable quotas
pub fn disable() {
    QUOTA_STATE.lock().disable();
}

/// Set quota limits
pub fn set_limits(quota_type: QuotaType, id: u32, limits: QuotaLimits) {
    QUOTA_STATE.lock().set_limits(quota_type, id, limits);
}

/// Check and charge a block allocation
pub fn alloc_blocks(uid: u32, gid: u32, blocks: u64) -> QuotaCheckResult {
    let mut state = QUOTA_STATE.lock();

    let user_result = state.check_block_alloc(QuotaType::User, uid, blocks);
    if user_result == QuotaCheckResult::Denied || user_result == QuotaCheckResult::GraceExpired {
        return user_result;
    }

    let group_result = state.check_block_alloc(QuotaType::Group, gid, blocks);
    if group_result == QuotaCheckResult::Denied || group_result == QuotaCheckResult::GraceExpired {
        return group_result;
    }

    // Both passed — charge both
    state.charge_blocks(QuotaType::User, uid, blocks);
    state.charge_blocks(QuotaType::Group, gid, blocks);

    // Return the worst result
    if user_result == QuotaCheckResult::Warning || group_result == QuotaCheckResult::Warning {
        QuotaCheckResult::Warning
    } else {
        QuotaCheckResult::Allowed
    }
}

/// Release blocks from quota
pub fn free_blocks(uid: u32, gid: u32, blocks: u64) {
    let mut state = QUOTA_STATE.lock();
    state.release_blocks(QuotaType::User, uid, blocks);
    state.release_blocks(QuotaType::Group, gid, blocks);
}

/// Check and charge an inode allocation
pub fn alloc_inode(uid: u32, gid: u32) -> QuotaCheckResult {
    let mut state = QUOTA_STATE.lock();

    let user_result = state.check_inode_alloc(QuotaType::User, uid);
    if user_result == QuotaCheckResult::Denied || user_result == QuotaCheckResult::GraceExpired {
        return user_result;
    }

    let group_result = state.check_inode_alloc(QuotaType::Group, gid);
    if group_result == QuotaCheckResult::Denied || group_result == QuotaCheckResult::GraceExpired {
        return group_result;
    }

    state.charge_inode(QuotaType::User, uid);
    state.charge_inode(QuotaType::Group, gid);

    if user_result == QuotaCheckResult::Warning || group_result == QuotaCheckResult::Warning {
        QuotaCheckResult::Warning
    } else {
        QuotaCheckResult::Allowed
    }
}

/// Release an inode from quota
pub fn free_inode(uid: u32, gid: u32) {
    let mut state = QUOTA_STATE.lock();
    state.release_inode(QuotaType::User, uid);
    state.release_inode(QuotaType::Group, gid);
}

/// Get quota report
pub fn report() -> Vec<QuotaReport> {
    QUOTA_STATE.lock().report()
}

/// Initialize the quota subsystem
pub fn init() {
    serial_println!("  quota: disk quota subsystem initialized (user/group, grace periods)");
}
