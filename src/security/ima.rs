/// Integrity Measurement Architecture for Genesis
///
/// Runtime file integrity measurement and enforcement:
///   - Measures (hashes) files before execution or access
///   - Maintains an ordered measurement log (IMA log)
///   - Extends TPM PCRs with each measurement for attestation
///   - Policy engine: measure, appraise, audit, enforce
///   - Template formats for measurement entries
///   - Violation detection with configurable response
///
/// Reference: Linux IMA/EVM subsystem design.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static IMA: Mutex<Option<ImaInner>> = Mutex::new(None);

/// Maximum measurement log entries
const MAX_LOG_ENTRIES: usize = 8192;

/// Default PCR index for IMA measurements
const IMA_PCR: u8 = 10;

/// IMA policy action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImaAction {
    /// Measure the file (add to log, extend PCR)
    Measure,
    /// Appraise the file (verify against stored hash)
    Appraise,
    /// Audit the file access (log but don't block)
    Audit,
    /// Don't measure or appraise
    DontMeasure,
}

/// IMA policy hook point
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImaHook {
    /// Binary execution (execve)
    BprmCheck,
    /// File open
    FileCheck,
    /// mmap with PROT_EXEC
    MmapCheck,
    /// Module loading
    ModuleCheck,
    /// Firmware loading
    FirmwareCheck,
    /// Policy loading
    PolicyCheck,
    /// Kexec image
    KexecCheck,
}

/// File type for policy matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Executable,
    SharedLib,
    KernelModule,
    Firmware,
    Script,
    Config,
}

/// IMA policy rule
#[derive(Clone)]
pub struct ImaPolicyRule {
    pub action: ImaAction,
    pub hook: ImaHook,
    pub file_type: Option<FileType>,
    pub uid_match: Option<u32>,
    pub path_prefix: Option<String>,
    pub enabled: bool,
}

/// Measurement template data
#[derive(Clone)]
pub struct MeasurementTemplate {
    pub digest: [u8; 32],
    pub filename: String,
    pub file_type: FileType,
}

/// IMA measurement entry (stored in the measurement log)
#[derive(Clone)]
pub struct Measurement {
    pub pcr: u8,
    pub digest: [u8; 32],
    pub template_digest: [u8; 32],
    pub template: MeasurementTemplate,
    pub sequence: u64,
}

/// IMA appraisal status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppraisalResult {
    /// Hash matches stored reference
    Pass,
    /// Hash does not match
    Fail,
    /// No reference hash available
    Unknown,
    /// File was not appraised (policy says don't)
    Skipped,
}

/// IMA violation record
#[derive(Clone)]
struct ImaViolation {
    filename: String,
    expected: [u8; 32],
    actual: [u8; 32],
    sequence: u64,
}

/// Inner IMA state
struct ImaInner {
    /// Ordered measurement log
    log: Vec<Measurement>,
    /// Policy rules (evaluated in order, first match wins)
    policy: Vec<ImaPolicyRule>,
    /// Known-good hashes for appraisal (filename -> expected hash)
    reference_hashes: Vec<(String, [u8; 32])>,
    /// Violation log
    violations: Vec<ImaViolation>,
    /// Running aggregate of all measurements (for TPM attestation)
    aggregate: [u8; 32],
    /// Sequence counter
    sequence: u64,
    /// Statistics
    total_measurements: u64,
    total_appraisals: u64,
    appraisal_failures: u64,
    /// Whether to enforce appraisal (block on failure) or just audit
    enforce_appraisal: bool,
    /// PCR index to extend
    pcr_index: u8,
}

impl ImaInner {
    fn new() -> Self {
        ImaInner {
            log: Vec::with_capacity(256),
            policy: Vec::new(),
            reference_hashes: Vec::new(),
            violations: Vec::new(),
            aggregate: [0u8; 32],
            sequence: 0,
            total_measurements: 0,
            total_appraisals: 0,
            appraisal_failures: 0,
            enforce_appraisal: false,
            pcr_index: IMA_PCR,
        }
    }

    /// Load default policy rules
    fn load_default_policy(&mut self) {
        // Measure all executed binaries
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Measure,
            hook: ImaHook::BprmCheck,
            file_type: Some(FileType::Executable),
            uid_match: None,
            path_prefix: None,
            enabled: true,
        });

        // Measure shared libraries on mmap
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Measure,
            hook: ImaHook::MmapCheck,
            file_type: Some(FileType::SharedLib),
            uid_match: None,
            path_prefix: None,
            enabled: true,
        });

        // Measure kernel modules
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Measure,
            hook: ImaHook::ModuleCheck,
            file_type: Some(FileType::KernelModule),
            uid_match: None,
            path_prefix: None,
            enabled: true,
        });

        // Measure firmware
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Measure,
            hook: ImaHook::FirmwareCheck,
            file_type: Some(FileType::Firmware),
            uid_match: None,
            path_prefix: None,
            enabled: true,
        });

        // Audit all file opens by root
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Audit,
            hook: ImaHook::FileCheck,
            file_type: None,
            uid_match: Some(0),
            path_prefix: None,
            enabled: true,
        });

        // Appraise executables in /bin, /sbin, /usr
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Appraise,
            hook: ImaHook::BprmCheck,
            file_type: Some(FileType::Executable),
            uid_match: None,
            path_prefix: Some(String::from("/bin")),
            enabled: true,
        });
        self.policy.push(ImaPolicyRule {
            action: ImaAction::Appraise,
            hook: ImaHook::BprmCheck,
            file_type: Some(FileType::Executable),
            uid_match: None,
            path_prefix: Some(String::from("/sbin")),
            enabled: true,
        });
    }

    /// Evaluate policy for a given file and hook
    fn evaluate_policy(
        &self,
        filename: &str,
        hook: ImaHook,
        file_type: FileType,
        uid: u32,
    ) -> ImaAction {
        for rule in &self.policy {
            if !rule.enabled {
                continue;
            }

            // Check hook match
            if rule.hook != hook {
                continue;
            }

            // Check file type match
            if let Some(ref ft) = rule.file_type {
                if *ft != file_type {
                    continue;
                }
            }

            // Check UID match
            if let Some(uid_match) = rule.uid_match {
                if uid != uid_match {
                    continue;
                }
            }

            // Check path prefix match
            if let Some(ref prefix) = rule.path_prefix {
                if !filename.starts_with(prefix.as_str()) {
                    continue;
                }
            }

            // All conditions matched, return this rule's action
            return rule.action;
        }

        // Default: don't measure
        ImaAction::DontMeasure
    }

    /// Compute the SHA-256 hash of file data
    fn compute_hash(&self, data: &[u8]) -> [u8; 32] {
        crate::crypto::sha256::hash(data)
    }

    /// Create a measurement entry and add to the log
    fn measure(&mut self, filename: &str, data: &[u8], file_type: FileType) -> Measurement {
        let digest = self.compute_hash(data);

        let template = MeasurementTemplate {
            digest,
            filename: String::from(filename),
            file_type,
        };

        // Compute template digest (hash of the template data itself)
        let template_digest = crate::crypto::sha256::hash_multi(&[&digest, filename.as_bytes()]);

        let entry = Measurement {
            pcr: self.pcr_index,
            digest,
            template_digest,
            template,
            sequence: self.sequence,
        };

        // Update running aggregate: aggregate = SHA256(aggregate || template_digest)
        self.aggregate = crate::crypto::sha256::hash_multi(&[&self.aggregate, &template_digest]);

        // Extend TPM PCR
        crate::security::tpm::pcr_extend(self.pcr_index, &template_digest);

        // Add to log (with eviction of oldest if full)
        if self.log.len() >= MAX_LOG_ENTRIES {
            self.log.remove(0);
        }
        self.log.push(entry.clone());

        self.sequence += 1;
        self.total_measurements = self.total_measurements.saturating_add(1);

        entry
    }

    /// Appraise a file against reference hashes
    fn appraise(&mut self, filename: &str, digest: &[u8; 32]) -> AppraisalResult {
        self.total_appraisals = self.total_appraisals.saturating_add(1);

        // Look up reference hash
        let reference = self
            .reference_hashes
            .iter()
            .find(|(name, _)| name == filename);

        match reference {
            Some((_, expected)) => {
                if expected == digest {
                    AppraisalResult::Pass
                } else {
                    self.appraisal_failures = self.appraisal_failures.saturating_add(1);
                    self.violations.push(ImaViolation {
                        filename: String::from(filename),
                        expected: *expected,
                        actual: *digest,
                        sequence: self.sequence,
                    });

                    serial_println!("    [ima] APPRAISAL FAIL: {} (hash mismatch)", filename);
                    crate::security::audit::log(
                        crate::security::audit::AuditEvent::FileAccess,
                        crate::security::audit::AuditResult::Deny,
                        0,
                        0,
                        &format!("IMA appraisal failed: {}", filename),
                    );

                    AppraisalResult::Fail
                }
            }
            None => AppraisalResult::Unknown,
        }
    }

    /// Full measurement + appraisal pipeline for a file
    fn process_file(
        &mut self,
        filename: &str,
        data: &[u8],
        hook: ImaHook,
        file_type: FileType,
        uid: u32,
    ) -> Result<Measurement, ()> {
        let action = self.evaluate_policy(filename, hook, file_type, uid);

        match action {
            ImaAction::DontMeasure => {
                // Create a minimal measurement for the return value
                let digest = self.compute_hash(data);
                Ok(Measurement {
                    pcr: 0,
                    digest,
                    template_digest: [0u8; 32],
                    template: MeasurementTemplate {
                        digest,
                        filename: String::from(filename),
                        file_type,
                    },
                    sequence: 0,
                })
            }
            ImaAction::Measure => {
                let entry = self.measure(filename, data, file_type);
                Ok(entry)
            }
            ImaAction::Appraise => {
                let entry = self.measure(filename, data, file_type);
                let result = self.appraise(filename, &entry.digest);
                if result == AppraisalResult::Fail && self.enforce_appraisal {
                    return Err(());
                }
                Ok(entry)
            }
            ImaAction::Audit => {
                let entry = self.measure(filename, data, file_type);
                serial_println!(
                    "    [ima] AUDIT: {} (hook={:?} uid={})",
                    filename,
                    hook,
                    uid
                );
                crate::security::audit::log(
                    crate::security::audit::AuditEvent::FileAccess,
                    crate::security::audit::AuditResult::Info,
                    0,
                    uid,
                    &format!("IMA audit: {} ({:?})", filename, hook),
                );
                Ok(entry)
            }
        }
    }

    /// Verify the entire measurement log against the aggregate
    fn verify_log(&self) -> bool {
        let mut computed_aggregate = [0u8; 32];
        for entry in &self.log {
            computed_aggregate =
                crate::crypto::sha256::hash_multi(&[&computed_aggregate, &entry.template_digest]);
        }
        computed_aggregate == self.aggregate
    }

    /// Add a reference hash for appraisal
    fn add_reference(&mut self, filename: &str, hash: [u8; 32]) {
        // Remove existing entry if any
        self.reference_hashes.retain(|(name, _)| name != filename);
        self.reference_hashes.push((String::from(filename), hash));
    }
}

/// IMA measurement log (public API for backward compatibility)
pub struct ImaLog;

impl ImaLog {
    pub fn new() -> Self {
        ImaLog
    }

    pub fn measure_file(&mut self, path: &str) -> Result<Measurement, ()> {
        // For backward compatibility, create a measurement with empty data
        // Real usage should call the module-level measure_file()
        if let Some(ref mut inner) = *IMA.lock() {
            let empty_data = [];
            return inner.process_file(path, &empty_data, ImaHook::FileCheck, FileType::Regular, 0);
        }
        Err(())
    }

    pub fn verify(&self) -> bool {
        if let Some(ref inner) = *IMA.lock() {
            return inner.verify_log();
        }
        false
    }
}

/// Measure a file before execution
pub fn measure_file(
    filename: &str,
    data: &[u8],
    hook: ImaHook,
    uid: u32,
) -> Result<Measurement, ()> {
    let file_type = classify_file(filename);
    if let Some(ref mut inner) = *IMA.lock() {
        return inner.process_file(filename, data, hook, file_type, uid);
    }
    Err(())
}

/// Add a reference hash for file appraisal
pub fn add_reference(filename: &str, hash: [u8; 32]) {
    if let Some(ref mut inner) = *IMA.lock() {
        inner.add_reference(filename, hash);
    }
}

/// Add a policy rule
pub fn add_policy_rule(rule: ImaPolicyRule) {
    if let Some(ref mut inner) = *IMA.lock() {
        inner.policy.push(rule);
    }
}

/// Set enforcement mode
pub fn set_enforce(enforce: bool) {
    if let Some(ref mut inner) = *IMA.lock() {
        inner.enforce_appraisal = enforce;
        serial_println!(
            "    [ima] Enforcement mode: {}",
            if enforce { "enforce" } else { "audit" }
        );
    }
}

/// Verify the measurement log integrity
pub fn verify_log() -> bool {
    if let Some(ref inner) = *IMA.lock() {
        return inner.verify_log();
    }
    false
}

/// Get the current aggregate measurement
pub fn get_aggregate() -> [u8; 32] {
    if let Some(ref inner) = *IMA.lock() {
        return inner.aggregate;
    }
    [0u8; 32]
}

/// Get measurement count
pub fn measurement_count() -> u64 {
    if let Some(ref inner) = *IMA.lock() {
        return inner.total_measurements;
    }
    0
}

/// Get violation count
pub fn violation_count() -> u64 {
    if let Some(ref inner) = *IMA.lock() {
        return inner.appraisal_failures;
    }
    0
}

/// Classify a file by its path/extension
fn classify_file(filename: &str) -> FileType {
    if filename.ends_with(".ko") || filename.ends_with(".mod") {
        FileType::KernelModule
    } else if filename.ends_with(".so") || filename.contains(".so.") {
        FileType::SharedLib
    } else if filename.ends_with(".fw") || filename.ends_with(".bin") {
        FileType::Firmware
    } else if filename.ends_with(".sh") || filename.ends_with(".py") {
        FileType::Script
    } else if filename.ends_with(".conf") || filename.ends_with(".cfg") {
        FileType::Config
    } else if filename.starts_with("/bin/")
        || filename.starts_with("/sbin/")
        || filename.starts_with("/usr/bin/")
        || filename.starts_with("/usr/sbin/")
    {
        FileType::Executable
    } else {
        FileType::Regular
    }
}

/// Initialize the IMA subsystem
pub fn init() {
    let mut inner = ImaInner::new();
    inner.load_default_policy();

    let rule_count = inner.policy.len();
    *IMA.lock() = Some(inner);

    serial_println!("    [ima] Integrity Measurement Architecture initialized");
    serial_println!(
        "    [ima] Default policy loaded ({} rules), PCR={}",
        rule_count,
        IMA_PCR
    );
    serial_println!("    [ima] Max log entries: {}", MAX_LOG_ENTRIES);
}
