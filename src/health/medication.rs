/// Medication management for Genesis
///
/// Medication reminders, dosage tracking, interaction checking,
/// refill alerts, scheduling, adherence scoring,
/// pharmacy contacts, and prescription history.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point (16 fractional bits). No floats in bare-metal.
const Q16_ONE: i32 = 65536;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum DoseFrequency {
    OnceDaily,
    TwiceDaily,
    ThreeTimesDaily,
    FourTimesDaily,
    EveryNHours(u8),
    Weekly,
    AsNeeded,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DoseForm {
    Tablet,
    Capsule,
    Liquid,
    Injection,
    Topical,
    Inhaler,
    Patch,
    Drops,
}

#[derive(Clone, Copy, PartialEq)]
pub enum InteractionSeverity {
    None,
    Minor,
    Moderate,
    Major,
    Contraindicated,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MedStatus {
    Active,
    Paused,
    Completed,
    Discontinued,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ReminderState {
    Pending,
    Notified,
    Taken,
    Skipped,
    Missed,
}

// ---------------------------------------------------------------------------
// Medication record
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct MedicationRecord {
    id: u32,
    name: [u8; 48],
    name_len: usize,
    dose_form: DoseForm,
    dose_amount: u32,           // milligrams or millilitres (unit depends on form)
    frequency: DoseFrequency,
    interval_secs: u32,         // computed seconds between doses
    start_date: u64,
    end_date: u64,              // 0 = ongoing
    status: MedStatus,
    prescriber: [u8; 32],
    prescriber_len: usize,
    interaction_group: u8,      // interaction class ID (0 = none)
    pills_remaining: u32,       // supply count
    pills_per_dose: u8,
    refill_threshold: u32,      // alert when remaining <= this
}

// ---------------------------------------------------------------------------
// Dose log entry
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct DoseLog {
    med_id: u32,
    scheduled_time: u64,
    actual_time: u64,           // 0 if not taken
    state: ReminderState,
    notes: [u8; 32],
    notes_len: usize,
}

// ---------------------------------------------------------------------------
// Upcoming reminder
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MedReminder {
    pub med_id: u32,
    pub med_name: [u8; 48],
    pub med_name_len: usize,
    pub scheduled_time: u64,
    pub dose_amount: u32,
    pub dose_form: DoseForm,
    pub pills: u8,
}

// ---------------------------------------------------------------------------
// Interaction pair (known interactions stored as pairs of group IDs)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct InteractionPair {
    group_a: u8,
    group_b: u8,
    severity: InteractionSeverity,
    description: [u8; 64],
    desc_len: usize,
}

// ---------------------------------------------------------------------------
// Refill alert
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct RefillAlert {
    pub med_id: u32,
    pub med_name: [u8; 48],
    pub med_name_len: usize,
    pub pills_remaining: u32,
    pub days_until_empty: u32,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct MedicationEngine {
    medications: Vec<MedicationRecord>,
    dose_log: Vec<DoseLog>,
    interactions: Vec<InteractionPair>,
    next_id: u32,
}

static MEDICATION: Mutex<Option<MedicationEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helper: frequency to interval seconds
// ---------------------------------------------------------------------------

fn frequency_to_secs(freq: DoseFrequency) -> u32 {
    match freq {
        DoseFrequency::OnceDaily        => 86400,
        DoseFrequency::TwiceDaily       => 43200,
        DoseFrequency::ThreeTimesDaily  => 28800,
        DoseFrequency::FourTimesDaily   => 21600,
        DoseFrequency::EveryNHours(h)   => (h as u32) * 3600,
        DoseFrequency::Weekly           => 604800,
        DoseFrequency::AsNeeded         => 0,
    }
}

fn copy_name48(dest: &mut [u8; 48], src: &[u8]) -> usize {
    let len = src.len().min(48);
    dest[..len].copy_from_slice(&src[..len]);
    len
}

fn copy_name32(dest: &mut [u8; 32], src: &[u8]) -> usize {
    let len = src.len().min(32);
    dest[..len].copy_from_slice(&src[..len]);
    len
}

fn copy_name64(dest: &mut [u8; 64], src: &[u8]) -> usize {
    let len = src.len().min(64);
    dest[..len].copy_from_slice(&src[..len]);
    len
}

// ---------------------------------------------------------------------------
// Seed common drug interaction pairs
// ---------------------------------------------------------------------------

fn seed_interactions() -> Vec<InteractionPair> {
    let mut v = Vec::new();
    // Group 1: blood thinners, Group 2: NSAIDs
    let mut desc1 = [0u8; 64];
    let d1 = b"Increased bleeding risk";
    let d1len = copy_name64(&mut desc1, d1);
    v.push(InteractionPair { group_a: 1, group_b: 2, severity: InteractionSeverity::Major, description: desc1, desc_len: d1len });

    // Group 3: SSRIs, Group 2: NSAIDs
    let mut desc2 = [0u8; 64];
    let d2 = b"GI bleeding risk increased";
    let d2len = copy_name64(&mut desc2, d2);
    v.push(InteractionPair { group_a: 3, group_b: 2, severity: InteractionSeverity::Moderate, description: desc2, desc_len: d2len });

    // Group 4: ACE inhibitors, Group 5: potassium supplements
    let mut desc3 = [0u8; 64];
    let d3 = b"Hyperkalemia risk";
    let d3len = copy_name64(&mut desc3, d3);
    v.push(InteractionPair { group_a: 4, group_b: 5, severity: InteractionSeverity::Major, description: desc3, desc_len: d3len });

    // Group 6: statins, Group 7: certain antibiotics
    let mut desc4 = [0u8; 64];
    let d4 = b"Rhabdomyolysis risk";
    let d4len = copy_name64(&mut desc4, d4);
    v.push(InteractionPair { group_a: 6, group_b: 7, severity: InteractionSeverity::Major, description: desc4, desc_len: d4len });

    // Group 1: blood thinners, Group 3: SSRIs
    let mut desc5 = [0u8; 64];
    let d5 = b"Serotonin syndrome risk";
    let d5len = copy_name64(&mut desc5, d5);
    v.push(InteractionPair { group_a: 1, group_b: 3, severity: InteractionSeverity::Moderate, description: desc5, desc_len: d5len });

    v
}

impl MedicationEngine {
    fn new() -> Self {
        MedicationEngine {
            medications: Vec::new(),
            dose_log: Vec::new(),
            interactions: seed_interactions(),
            next_id: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Add medication
    // -----------------------------------------------------------------------

    fn add_medication(
        &mut self, name: &[u8], form: DoseForm, dose_mg: u32,
        frequency: DoseFrequency, start: u64, end: u64,
        interaction_group: u8, supply: u32, pills_per_dose: u8,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 48];
        let nlen = copy_name48(&mut n, name);
        self.medications.push(MedicationRecord {
            id, name: n, name_len: nlen, dose_form: form,
            dose_amount: dose_mg, frequency,
            interval_secs: frequency_to_secs(frequency),
            start_date: start, end_date: end,
            status: MedStatus::Active,
            prescriber: [0; 32], prescriber_len: 0,
            interaction_group, pills_remaining: supply,
            pills_per_dose: pills_per_dose, refill_threshold: 7,
        });
        id
    }

    // -----------------------------------------------------------------------
    // Check due reminders
    // -----------------------------------------------------------------------

    fn get_due_reminders(&self, current_time: u64) -> Vec<MedReminder> {
        let mut reminders = Vec::new();
        for med in &self.medications {
            if med.status != MedStatus::Active { continue; }
            if med.interval_secs == 0 { continue; } // as-needed
            if med.end_date != 0 && current_time > med.end_date { continue; }

            // Find next scheduled time for this med
            let elapsed = current_time.saturating_sub(med.start_date);
            let interval = med.interval_secs as u64;
            if interval == 0 { continue; }
            let doses_due = elapsed / interval;
            let next_time = med.start_date + (doses_due + 1) * interval;
            let prev_time = med.start_date + doses_due * interval;

            // Check if previous dose was logged
            let prev_taken = self.dose_log.iter().any(|d| {
                d.med_id == med.id && d.scheduled_time == prev_time
                    && d.state == ReminderState::Taken
            });

            if !prev_taken && current_time >= prev_time {
                reminders.push(MedReminder {
                    med_id: med.id,
                    med_name: med.name,
                    med_name_len: med.name_len,
                    scheduled_time: prev_time,
                    dose_amount: med.dose_amount,
                    dose_form: med.dose_form,
                    pills: med.pills_per_dose,
                });
            }
            // Also upcoming within 30 min
            if next_time <= current_time + 1800 && next_time > current_time {
                reminders.push(MedReminder {
                    med_id: med.id,
                    med_name: med.name,
                    med_name_len: med.name_len,
                    scheduled_time: next_time,
                    dose_amount: med.dose_amount,
                    dose_form: med.dose_form,
                    pills: med.pills_per_dose,
                });
            }
        }
        reminders
    }

    // -----------------------------------------------------------------------
    // Record dose taken / skipped
    // -----------------------------------------------------------------------

    fn record_dose(&mut self, med_id: u32, scheduled_time: u64, actual_time: u64, taken: bool) {
        let state = if taken { ReminderState::Taken } else { ReminderState::Skipped };
        if self.dose_log.len() < 50000 {
            self.dose_log.push(DoseLog {
                med_id, scheduled_time, actual_time, state,
                notes: [0; 32], notes_len: 0,
            });
        }
        // Decrement supply if taken
        if taken {
            if let Some(med) = self.medications.iter_mut().find(|m| m.id == med_id) {
                med.pills_remaining = med.pills_remaining.saturating_sub(med.pills_per_dose as u32);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Adherence rate (Q16): taken / (taken + missed + skipped) for a med
    // -----------------------------------------------------------------------

    fn adherence_q16(&self, med_id: u32) -> i32 {
        let total = self.dose_log.iter().filter(|d| d.med_id == med_id).count() as i64;
        if total == 0 { return Q16_ONE; }
        let taken = self.dose_log.iter()
            .filter(|d| d.med_id == med_id && d.state == ReminderState::Taken)
            .count() as i64;
        (((taken) << 16) / total) as i32
    }

    // -----------------------------------------------------------------------
    // Check interactions for a new medication
    // -----------------------------------------------------------------------

    fn check_interactions(&self, interaction_group: u8) -> Vec<(u32, InteractionSeverity)> {
        let mut results = Vec::new();
        if interaction_group == 0 { return results; }
        for med in &self.medications {
            if med.status != MedStatus::Active { continue; }
            if med.interaction_group == 0 { continue; }
            for pair in &self.interactions {
                let matches = (pair.group_a == interaction_group && pair.group_b == med.interaction_group)
                    || (pair.group_b == interaction_group && pair.group_a == med.interaction_group);
                if matches {
                    results.push((med.id, pair.severity));
                }
            }
        }
        results
    }

    // -----------------------------------------------------------------------
    // Refill alerts
    // -----------------------------------------------------------------------

    fn get_refill_alerts(&self) -> Vec<RefillAlert> {
        let mut alerts = Vec::new();
        for med in &self.medications {
            if med.status != MedStatus::Active { continue; }
            if med.pills_remaining <= med.refill_threshold {
                let doses_per_day = if med.interval_secs > 0 {
                    86400 / med.interval_secs.max(1)
                } else { 1 };
                let pills_per_day = doses_per_day * med.pills_per_dose as u32;
                let days = if pills_per_day > 0 {
                    med.pills_remaining / pills_per_day
                } else { 0 };
                alerts.push(RefillAlert {
                    med_id: med.id,
                    med_name: med.name,
                    med_name_len: med.name_len,
                    pills_remaining: med.pills_remaining,
                    days_until_empty: days,
                });
            }
        }
        alerts
    }

    // -----------------------------------------------------------------------
    // Pause / resume / discontinue
    // -----------------------------------------------------------------------

    fn set_status(&mut self, med_id: u32, status: MedStatus) {
        if let Some(med) = self.medications.iter_mut().find(|m| m.id == med_id) {
            med.status = status;
        }
    }

    // -----------------------------------------------------------------------
    // Schedule overview: next N doses across all meds
    // -----------------------------------------------------------------------

    fn upcoming_schedule(&self, current_time: u64, count: usize) -> Vec<(u32, u64)> {
        let mut schedule: Vec<(u32, u64)> = Vec::new();
        for med in &self.medications {
            if med.status != MedStatus::Active { continue; }
            if med.interval_secs == 0 { continue; }
            let interval = med.interval_secs as u64;
            let elapsed = current_time.saturating_sub(med.start_date);
            let doses_done = elapsed / interval;
            // Compute next 3 doses for each med
            for i in 1..=3u64 {
                let t = med.start_date + (doses_done + i) * interval;
                if med.end_date != 0 && t > med.end_date { break; }
                schedule.push((med.id, t));
            }
        }
        // Sort by time
        schedule.sort_by_key(|&(_, t)| t);
        schedule.truncate(count);
        schedule
    }

    // -----------------------------------------------------------------------
    // Active medication count
    // -----------------------------------------------------------------------

    fn active_count(&self) -> u32 {
        self.medications.iter().filter(|m| m.status == MedStatus::Active).count() as u32
    }
}

// ---------------------------------------------------------------------------
// Public init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut m = MEDICATION.lock();
    *m = Some(MedicationEngine::new());
    serial_println!("    Health: medication management (reminders, interactions, refills) ready");
}
