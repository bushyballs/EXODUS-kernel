/// Developer console for Genesis store
///
/// App submission workflow, review status tracking,
/// crash report aggregation, developer analytics,
/// A/B testing framework, release management.
///
/// Original implementation for Hoags OS.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers (i32 with 16 fractional bits, NO floats)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    ((a as i64 * Q16_ONE as i64) / b as i64) as i32
}

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum SubmissionStatus {
    Draft,
    Submitted,
    InReview,
    Approved,
    Rejected,
    Published,
    Suspended,
    Withdrawn,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RejectionReason {
    PolicyViolation,
    MalwareDetected,
    InsufficientMetadata,
    CrashOnStartup,
    PrivacyViolation,
    CopyrightIssue,
    InappropriateContent,
    PerformanceIssue,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CrashSeverity {
    Critical,
    Major,
    Minor,
    Warning,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TestVariant {
    Control,
    VariantA,
    VariantB,
    VariantC,
}

struct AppSubmission {
    id: u32,
    developer_hash: u64,
    listing_id: u32,
    version_major: u8,
    version_minor: u8,
    version_patch: u16,
    status: SubmissionStatus,
    submitted_at: u64,
    reviewed_at: u64,
    reviewer_hash: u64,
    rejection: Option<RejectionReason>,
    rejection_notes_hash: u64,
    binary_hash: u64,
    binary_size: u64,
    min_os_version: u32,
    changelog_hash: u64,
    resubmit_count: u8,
}

struct CrashReport {
    id: u32,
    listing_id: u32,
    version_major: u8,
    version_minor: u8,
    version_patch: u16,
    severity: CrashSeverity,
    exception_hash: u64,
    stack_hash: u64,
    device_hash: u64,
    os_version: u32,
    timestamp: u64,
    occurrence_count: u32,
    resolved: bool,
}

struct DeveloperAnalytics {
    listing_id: u32,
    daily_installs: u32,
    daily_uninstalls: u32,
    daily_active_users: u32,
    total_installs: u64,
    total_revenue_cents: u64,
    avg_session_secs_q16: i32,   // Q16 average session duration
    retention_day1_q16: i32,     // Q16 percentage
    retention_day7_q16: i32,
    retention_day30_q16: i32,
    crash_rate_q16: i32,         // Q16 crashes per 1000 users
    rating_avg_q16: i32,
    snapshot_time: u64,
}

struct ABTest {
    id: u32,
    listing_id: u32,
    name_hash: u64,
    active: bool,
    start_time: u64,
    end_time: u64,
    control_users: u32,
    variant_a_users: u32,
    variant_b_users: u32,
    variant_c_users: u32,
    control_conversions: u32,
    variant_a_conversions: u32,
    variant_b_conversions: u32,
    variant_c_conversions: u32,
    winner: Option<TestVariant>,
}

struct ReleaseTrack {
    listing_id: u32,
    track_type: ReleaseTrackType,
    version_major: u8,
    version_minor: u8,
    version_patch: u16,
    rollout_pct: u8,            // 0-100
    created_at: u64,
    user_count: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ReleaseTrackType {
    Internal,
    Alpha,
    Beta,
    Production,
}

struct DeveloperProfile {
    developer_hash: u64,
    verified: bool,
    app_count: u32,
    total_downloads: u64,
    total_revenue_cents: u64,
    joined_at: u64,
    suspended: bool,
    api_key_hash: u64,
}

struct DevConsole {
    submissions: Vec<AppSubmission>,
    crash_reports: Vec<CrashReport>,
    analytics: Vec<DeveloperAnalytics>,
    ab_tests: Vec<ABTest>,
    release_tracks: Vec<ReleaseTrack>,
    developers: Vec<DeveloperProfile>,
    next_submission_id: u32,
    next_crash_id: u32,
    next_test_id: u32,
    pending_reviews: u32,
}

static DEV_CONSOLE: Mutex<Option<DevConsole>> = Mutex::new(None);

impl DevConsole {
    fn new() -> Self {
        DevConsole {
            submissions: Vec::new(),
            crash_reports: Vec::new(),
            analytics: Vec::new(),
            ab_tests: Vec::new(),
            release_tracks: Vec::new(),
            developers: Vec::new(),
            next_submission_id: 1,
            next_crash_id: 1,
            next_test_id: 1,
            pending_reviews: 0,
        }
    }

    fn register_developer(&mut self, dev_hash: u64, timestamp: u64, api_key_hash: u64) -> bool {
        if self.developers.iter().any(|d| d.developer_hash == dev_hash) {
            return false; // already registered
        }
        self.developers.push(DeveloperProfile {
            developer_hash: dev_hash,
            verified: false,
            app_count: 0,
            total_downloads: 0,
            total_revenue_cents: 0,
            joined_at: timestamp,
            suspended: false,
            api_key_hash,
        });
        true
    }

    fn verify_developer(&mut self, dev_hash: u64) -> bool {
        if let Some(dev) = self.developers.iter_mut().find(|d| d.developer_hash == dev_hash) {
            dev.verified = true;
            return true;
        }
        false
    }

    fn submit_app(
        &mut self,
        dev_hash: u64,
        listing_id: u32,
        ver_major: u8,
        ver_minor: u8,
        ver_patch: u16,
        binary_hash: u64,
        binary_size: u64,
        min_os: u32,
        changelog_hash: u64,
        timestamp: u64,
    ) -> u32 {
        // Verify developer exists and is not suspended
        let dev_ok = self.developers.iter().any(|d| {
            d.developer_hash == dev_hash && !d.suspended
        });
        if !dev_ok { return 0; }

        let id = self.next_submission_id;
        self.next_submission_id = self.next_submission_id.saturating_add(1);

        self.submissions.push(AppSubmission {
            id,
            developer_hash: dev_hash,
            listing_id,
            version_major: ver_major,
            version_minor: ver_minor,
            version_patch: ver_patch,
            status: SubmissionStatus::Submitted,
            submitted_at: timestamp,
            reviewed_at: 0,
            reviewer_hash: 0,
            rejection: None,
            rejection_notes_hash: 0,
            binary_hash,
            binary_size,
            min_os_version: min_os,
            changelog_hash,
            resubmit_count: 0,
        });

        self.pending_reviews = self.pending_reviews.saturating_add(1);
        id
    }

    fn approve_submission(&mut self, submission_id: u32, reviewer_hash: u64, timestamp: u64) -> bool {
        if let Some(sub) = self.submissions.iter_mut().find(|s| s.id == submission_id) {
            if sub.status != SubmissionStatus::Submitted && sub.status != SubmissionStatus::InReview {
                return false;
            }
            sub.status = SubmissionStatus::Approved;
            sub.reviewed_at = timestamp;
            sub.reviewer_hash = reviewer_hash;
            if self.pending_reviews > 0 {
                self.pending_reviews -= 1;
            }
            return true;
        }
        false
    }

    fn reject_submission(
        &mut self,
        submission_id: u32,
        reviewer_hash: u64,
        reason: RejectionReason,
        notes_hash: u64,
        timestamp: u64,
    ) -> bool {
        if let Some(sub) = self.submissions.iter_mut().find(|s| s.id == submission_id) {
            if sub.status != SubmissionStatus::Submitted && sub.status != SubmissionStatus::InReview {
                return false;
            }
            sub.status = SubmissionStatus::Rejected;
            sub.reviewed_at = timestamp;
            sub.reviewer_hash = reviewer_hash;
            sub.rejection = Some(reason);
            sub.rejection_notes_hash = notes_hash;
            if self.pending_reviews > 0 {
                self.pending_reviews -= 1;
            }
            return true;
        }
        false
    }

    fn publish_submission(&mut self, submission_id: u32, timestamp: u64) -> bool {
        if let Some(sub) = self.submissions.iter_mut().find(|s| s.id == submission_id) {
            if sub.status != SubmissionStatus::Approved { return false; }
            sub.status = SubmissionStatus::Published;
            return true;
        }
        false
    }

    fn withdraw_submission(&mut self, submission_id: u32, dev_hash: u64) -> bool {
        if let Some(sub) = self.submissions.iter_mut().find(|s| {
            s.id == submission_id && s.developer_hash == dev_hash
        }) {
            if sub.status == SubmissionStatus::Published { return false; }
            sub.status = SubmissionStatus::Withdrawn;
            if self.pending_reviews > 0 {
                self.pending_reviews -= 1;
            }
            return true;
        }
        false
    }

    fn report_crash(
        &mut self,
        listing_id: u32,
        ver_major: u8,
        ver_minor: u8,
        ver_patch: u16,
        severity: CrashSeverity,
        exception_hash: u64,
        stack_hash: u64,
        device_hash: u64,
        os_version: u32,
        timestamp: u64,
    ) -> u32 {
        // Check if same crash already reported (deduplicate by stack hash)
        if let Some(existing) = self.crash_reports.iter_mut().find(|c| {
            c.listing_id == listing_id && c.stack_hash == stack_hash && !c.resolved
        }) {
            existing.occurrence_count = existing.occurrence_count.saturating_add(1);
            return existing.id;
        }

        let id = self.next_crash_id;
        self.next_crash_id = self.next_crash_id.saturating_add(1);

        self.crash_reports.push(CrashReport {
            id,
            listing_id,
            version_major: ver_major,
            version_minor: ver_minor,
            version_patch: ver_patch,
            severity,
            exception_hash,
            stack_hash,
            device_hash,
            os_version,
            timestamp,
            occurrence_count: 1,
            resolved: false,
        });
        id
    }

    fn resolve_crash(&mut self, crash_id: u32) -> bool {
        if let Some(cr) = self.crash_reports.iter_mut().find(|c| c.id == crash_id) {
            cr.resolved = true;
            return true;
        }
        false
    }

    fn crashes_for_app(&self, listing_id: u32, unresolved_only: bool) -> Vec<u32> {
        self.crash_reports.iter()
            .filter(|c| c.listing_id == listing_id && (!unresolved_only || !c.resolved))
            .map(|c| c.id)
            .collect()
    }

    fn crash_rate_q16(&self, listing_id: u32, active_users: u32) -> i32 {
        if active_users == 0 { return 0; }
        let total_crashes: u32 = self.crash_reports.iter()
            .filter(|c| c.listing_id == listing_id && !c.resolved)
            .map(|c| c.occurrence_count)
            .sum();
        // Crashes per 1000 users in Q16
        let per_1000 = (total_crashes as i64 * 1000) / active_users as i64;
        q16_from_int(per_1000 as i32)
    }

    fn update_analytics(
        &mut self,
        listing_id: u32,
        installs: u32,
        uninstalls: u32,
        dau: u32,
        revenue_cents: u64,
        session_secs_q16: i32,
        timestamp: u64,
    ) {
        if let Some(a) = self.analytics.iter_mut().find(|a| a.listing_id == listing_id) {
            a.daily_installs = installs;
            a.daily_uninstalls = uninstalls;
            a.daily_active_users = dau;
            a.total_installs = a.total_installs.saturating_add(installs as u64);
            a.total_revenue_cents = a.total_revenue_cents.saturating_add(revenue_cents);
            a.avg_session_secs_q16 = session_secs_q16;
            a.snapshot_time = timestamp;
        } else {
            self.analytics.push(DeveloperAnalytics {
                listing_id,
                daily_installs: installs,
                daily_uninstalls: uninstalls,
                daily_active_users: dau,
                total_installs: installs as u64,
                total_revenue_cents: revenue_cents,
                avg_session_secs_q16: session_secs_q16,
                retention_day1_q16: 0,
                retention_day7_q16: 0,
                retention_day30_q16: 0,
                crash_rate_q16: 0,
                rating_avg_q16: 0,
                snapshot_time: timestamp,
            });
        }
    }

    fn set_retention(
        &mut self,
        listing_id: u32,
        day1_pct: i32,
        day7_pct: i32,
        day30_pct: i32,
    ) {
        if let Some(a) = self.analytics.iter_mut().find(|a| a.listing_id == listing_id) {
            a.retention_day1_q16 = q16_div(day1_pct, 100);
            a.retention_day7_q16 = q16_div(day7_pct, 100);
            a.retention_day30_q16 = q16_div(day30_pct, 100);
        }
    }

    fn create_ab_test(
        &mut self,
        listing_id: u32,
        name_hash: u64,
        start_time: u64,
        end_time: u64,
    ) -> u32 {
        let id = self.next_test_id;
        self.next_test_id = self.next_test_id.saturating_add(1);

        self.ab_tests.push(ABTest {
            id,
            listing_id,
            name_hash,
            active: true,
            start_time,
            end_time,
            control_users: 0,
            variant_a_users: 0,
            variant_b_users: 0,
            variant_c_users: 0,
            control_conversions: 0,
            variant_a_conversions: 0,
            variant_b_conversions: 0,
            variant_c_conversions: 0,
            winner: None,
        });
        id
    }

    fn record_ab_event(&mut self, test_id: u32, variant: TestVariant, converted: bool) {
        if let Some(test) = self.ab_tests.iter_mut().find(|t| t.id == test_id && t.active) {
            match variant {
                TestVariant::Control => {
                    test.control_users = test.control_users.saturating_add(1);
                    if converted { test.control_conversions = test.control_conversions.saturating_add(1); }
                }
                TestVariant::VariantA => {
                    test.variant_a_users = test.variant_a_users.saturating_add(1);
                    if converted { test.variant_a_conversions = test.variant_a_conversions.saturating_add(1); }
                }
                TestVariant::VariantB => {
                    test.variant_b_users = test.variant_b_users.saturating_add(1);
                    if converted { test.variant_b_conversions = test.variant_b_conversions.saturating_add(1); }
                }
                TestVariant::VariantC => {
                    test.variant_c_users = test.variant_c_users.saturating_add(1);
                    if converted { test.variant_c_conversions = test.variant_c_conversions.saturating_add(1); }
                }
            }
        }
    }

    fn conclude_ab_test(&mut self, test_id: u32) -> Option<TestVariant> {
        if let Some(test) = self.ab_tests.iter_mut().find(|t| t.id == test_id) {
            test.active = false;

            // Compute conversion rates in Q16
            let rates = [
                (TestVariant::Control,  if test.control_users > 0 { q16_div(test.control_conversions as i32, test.control_users as i32) } else { 0 }),
                (TestVariant::VariantA, if test.variant_a_users > 0 { q16_div(test.variant_a_conversions as i32, test.variant_a_users as i32) } else { 0 }),
                (TestVariant::VariantB, if test.variant_b_users > 0 { q16_div(test.variant_b_conversions as i32, test.variant_b_users as i32) } else { 0 }),
                (TestVariant::VariantC, if test.variant_c_users > 0 { q16_div(test.variant_c_conversions as i32, test.variant_c_users as i32) } else { 0 }),
            ];

            let best = rates.iter().max_by_key(|(_, r)| *r);
            if let Some((variant, _)) = best {
                test.winner = Some(*variant);
                return Some(*variant);
            }
        }
        None
    }

    fn set_release_track(
        &mut self,
        listing_id: u32,
        track: ReleaseTrackType,
        ver_major: u8,
        ver_minor: u8,
        ver_patch: u16,
        rollout_pct: u8,
        timestamp: u64,
    ) {
        if let Some(rt) = self.release_tracks.iter_mut().find(|r| {
            r.listing_id == listing_id && r.track_type == track
        }) {
            rt.version_major = ver_major;
            rt.version_minor = ver_minor;
            rt.version_patch = ver_patch;
            rt.rollout_pct = rollout_pct.min(100);
            rt.created_at = timestamp;
        } else {
            self.release_tracks.push(ReleaseTrack {
                listing_id,
                track_type: track,
                version_major: ver_major,
                version_minor: ver_minor,
                version_patch: ver_patch,
                rollout_pct: rollout_pct.min(100),
                created_at: timestamp,
                user_count: 0,
            });
        }
    }

    fn submission_history(&self, dev_hash: u64) -> Vec<u32> {
        self.submissions.iter()
            .filter(|s| s.developer_hash == dev_hash)
            .map(|s| s.id)
            .collect()
    }
}

pub fn init() {
    let mut console = DEV_CONSOLE.lock();
    *console = Some(DevConsole::new());
    serial_println!("    App store: developer console (submissions, crashes, analytics, A/B) ready");
}
