/// Runtime integrity monitoring for Genesis
///
/// Continuously verifies kernel and system integrity:
///   - Kernel text hash verification (detect rootkits)
///   - IDT/GDT integrity (detect hooking)
///   - Syscall table integrity
///   - Critical data structure checksums
///   - IMA-like file measurement (hash before execute)
///
/// Inspired by: Linux IMA, Windows PatchGuard, macOS KEXT signing.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

static INTEGRITY: Mutex<Option<IntegrityMonitor>> = Mutex::new(None);

/// What kind of measurement this is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementType {
    /// Kernel text section
    KernelText,
    /// Interrupt Descriptor Table
    Idt,
    /// Global Descriptor Table
    Gdt,
    /// Syscall dispatch table
    SyscallTable,
    /// Critical kernel data structure
    KernelData,
    /// Executable file (before exec)
    FileExec,
    /// Shared library (before load)
    SharedLib,
    /// Kernel module
    KernelModule,
    /// Configuration file
    ConfigFile,
}

/// A baseline measurement
#[derive(Debug, Clone)]
pub struct Measurement {
    pub mtype: MeasurementType,
    pub name: String,
    pub hash: [u8; 32],
    pub size: usize,
    pub address: u64,
    pub timestamp: u64,
}

/// Integrity violation
#[derive(Debug, Clone)]
pub struct Violation {
    pub mtype: MeasurementType,
    pub name: String,
    pub expected_hash: [u8; 32],
    pub actual_hash: [u8; 32],
    pub timestamp: u64,
}

/// Integrity check result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityResult {
    /// All measurements match baseline
    Ok,
    /// One or more measurements don't match
    Tampered,
    /// No baseline exists for this measurement
    Unknown,
}

/// Policy for what to do on integrity violation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationPolicy {
    /// Log and continue
    LogOnly,
    /// Log and alert (serial + display warning)
    Alert,
    /// Log and panic (halt the system)
    Panic,
}

/// Integrity monitor
pub struct IntegrityMonitor {
    /// Baseline measurements
    pub baselines: BTreeMap<String, Measurement>,
    /// Detected violations
    pub violations: Vec<Violation>,
    /// What to do on violation
    pub policy: ViolationPolicy,
    /// IMA measurement log (file execution log)
    pub ima_log: Vec<Measurement>,
    /// Maximum IMA log entries
    pub ima_max_entries: usize,
    /// Whether periodic checks are enabled
    pub periodic_check: bool,
    /// Check interval in ticks
    pub check_interval: u64,
    /// Last check tick
    pub last_check: u64,
}

impl IntegrityMonitor {
    pub fn new(policy: ViolationPolicy) -> Self {
        IntegrityMonitor {
            baselines: BTreeMap::new(),
            violations: Vec::new(),
            policy,
            ima_log: Vec::new(),
            ima_max_entries: 8192,
            periodic_check: true,
            check_interval: 10_000, // Every 10K ticks
            last_check: 0,
        }
    }

    /// Take a baseline measurement of a memory region
    pub fn baseline(&mut self, mtype: MeasurementType, name: &str, addr: u64, size: usize) {
        let data = unsafe { core::slice::from_raw_parts(addr as *const u8, size) };
        let hash = crate::crypto::sha256::hash(data);

        let measurement = Measurement {
            mtype,
            name: String::from(name),
            hash,
            size,
            address: addr,
            timestamp: 0,
        };

        self.baselines.insert(String::from(name), measurement);
    }

    /// Verify a measurement against its baseline
    pub fn verify(&mut self, name: &str) -> IntegrityResult {
        let baseline = match self.baselines.get(name) {
            Some(b) => b.clone(),
            None => return IntegrityResult::Unknown,
        };

        let data =
            unsafe { core::slice::from_raw_parts(baseline.address as *const u8, baseline.size) };
        let current_hash = crate::crypto::sha256::hash(data);

        if current_hash == baseline.hash {
            IntegrityResult::Ok
        } else {
            let violation = Violation {
                mtype: baseline.mtype,
                name: String::from(name),
                expected_hash: baseline.hash,
                actual_hash: current_hash,
                timestamp: 0,
            };

            self.handle_violation(&violation);
            self.violations.push(violation);
            IntegrityResult::Tampered
        }
    }

    /// Verify all baselines
    pub fn verify_all(&mut self) -> Vec<String> {
        let names: Vec<String> = self.baselines.keys().cloned().collect();
        let mut failed = Vec::new();

        for name in &names {
            if self.verify(name) != IntegrityResult::Ok {
                failed.push(name.clone());
            }
        }

        failed
    }

    /// IMA: measure a file before execution
    pub fn measure_file(&mut self, name: &str, data: &[u8]) -> [u8; 32] {
        let hash = crate::crypto::sha256::hash(data);

        let measurement = Measurement {
            mtype: MeasurementType::FileExec,
            name: String::from(name),
            hash,
            size: data.len(),
            address: 0,
            timestamp: 0,
        };

        // Add to IMA log
        if self.ima_log.len() >= self.ima_max_entries {
            self.ima_log.remove(0);
        }
        self.ima_log.push(measurement);

        hash
    }

    /// Handle an integrity violation
    fn handle_violation(&self, violation: &Violation) {
        serial_println!(
            "  [integrity] VIOLATION: {} ({:?})",
            violation.name,
            violation.mtype
        );
        serial_println!("    Expected: {:02x?}", &violation.expected_hash[..8]);
        serial_println!("    Actual:   {:02x?}", &violation.actual_hash[..8]);

        crate::security::audit::log(
            crate::security::audit::AuditEvent::PolicyChange,
            crate::security::audit::AuditResult::Deny,
            0,
            0,
            &alloc::format!("integrity violation: {}", violation.name),
        );

        match self.policy {
            ViolationPolicy::LogOnly => {}
            ViolationPolicy::Alert => {
                serial_println!("  [integrity] !!! ALERT: System integrity compromised !!!");
            }
            ViolationPolicy::Panic => {
                panic!(
                    "INTEGRITY VIOLATION: {} has been tampered with",
                    violation.name
                );
            }
        }
    }

    /// Periodic check (called from timer interrupt)
    pub fn tick(&mut self, current_tick: u64) {
        if !self.periodic_check {
            return;
        }
        if current_tick - self.last_check >= self.check_interval {
            self.last_check = current_tick;
            self.verify_all();
        }
    }
}

/// Initialize the integrity monitor
pub fn init(policy: ViolationPolicy) {
    let monitor = IntegrityMonitor::new(policy);
    *INTEGRITY.lock() = Some(monitor);
    serial_println!(
        "  [integrity] Runtime integrity monitor initialized (policy: {:?})",
        policy
    );
}

/// Take a baseline measurement
pub fn baseline(mtype: MeasurementType, name: &str, addr: u64, size: usize) {
    if let Some(ref mut mon) = *INTEGRITY.lock() {
        mon.baseline(mtype, name, addr, size);
    }
}

/// Verify all baselines
pub fn verify_all() -> Vec<String> {
    INTEGRITY
        .lock()
        .as_mut()
        .map(|mon| mon.verify_all())
        .unwrap_or_default()
}

/// Measure a file (IMA)
pub fn measure_file(name: &str, data: &[u8]) -> [u8; 32] {
    INTEGRITY
        .lock()
        .as_mut()
        .map(|mon| mon.measure_file(name, data))
        .unwrap_or([0u8; 32])
}

/// Periodic tick
pub fn tick(current_tick: u64) {
    if let Some(ref mut mon) = *INTEGRITY.lock() {
        mon.tick(current_tick);
    }
}
