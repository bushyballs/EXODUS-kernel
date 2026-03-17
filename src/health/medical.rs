use crate::sync::Mutex;
/// Medical records for Genesis
///
/// Health records storage, medication tracking,
/// vaccination records, allergy management,
/// doctor appointments, health sharing.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum RecordType {
    LabResult,
    Prescription,
    Vaccination,
    Allergy,
    Condition,
    Procedure,
    Visit,
    Imaging,
}

struct HealthRecord {
    id: u32,
    record_type: RecordType,
    title: [u8; 64],
    title_len: usize,
    date: u64,
    provider: [u8; 32],
    provider_len: usize,
    data_hash: u64,
}

struct Medication {
    name: [u8; 48],
    name_len: usize,
    dosage: [u8; 16],
    dosage_len: usize,
    frequency_hours: u8,
    start_date: u64,
    end_date: Option<u64>,
    next_dose_time: u64,
    taken_count: u32,
    missed_count: u32,
}

struct Vaccination {
    name: [u8; 48],
    name_len: usize,
    date_administered: u64,
    dose_number: u8,
    total_doses: u8,
    next_dose_date: Option<u64>,
    lot_number: [u8; 16],
    lot_len: usize,
}

struct MedicalEngine {
    records: Vec<HealthRecord>,
    medications: Vec<Medication>,
    vaccinations: Vec<Vaccination>,
    allergies: Vec<[u8; 32]>,
    next_id: u32,
    emergency_sharing_enabled: bool,
}

static MEDICAL: Mutex<Option<MedicalEngine>> = Mutex::new(None);

impl MedicalEngine {
    fn new() -> Self {
        MedicalEngine {
            records: Vec::new(),
            medications: Vec::new(),
            vaccinations: Vec::new(),
            allergies: Vec::new(),
            next_id: 1,
            emergency_sharing_enabled: false,
        }
    }

    fn add_record(&mut self, record_type: RecordType, title: &[u8], date: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut t = [0u8; 64];
        let tlen = title.len().min(64);
        t[..tlen].copy_from_slice(&title[..tlen]);
        self.records.push(HealthRecord {
            id,
            record_type,
            title: t,
            title_len: tlen,
            date,
            provider: [0; 32],
            provider_len: 0,
            data_hash: 0,
        });
        id
    }

    fn add_medication(&mut self, name: &[u8], dosage: &[u8], freq_hours: u8, start: u64) {
        let mut n = [0u8; 48];
        let nlen = name.len().min(48);
        n[..nlen].copy_from_slice(&name[..nlen]);
        let mut d = [0u8; 16];
        let dlen = dosage.len().min(16);
        d[..dlen].copy_from_slice(&dosage[..dlen]);
        self.medications.push(Medication {
            name: n,
            name_len: nlen,
            dosage: d,
            dosage_len: dlen,
            frequency_hours: freq_hours,
            start_date: start,
            end_date: None,
            next_dose_time: start + freq_hours as u64 * 3600,
            taken_count: 0,
            missed_count: 0,
        });
    }

    fn check_medication_due(&self, current_time: u64) -> Vec<usize> {
        let mut due = Vec::new();
        for (i, med) in self.medications.iter().enumerate() {
            if med.end_date.map_or(true, |end| current_time < end) {
                if current_time >= med.next_dose_time {
                    due.push(i);
                }
            }
        }
        due
    }

    fn record_dose_taken(&mut self, med_idx: usize) {
        if let Some(med) = self.medications.get_mut(med_idx) {
            med.taken_count = med.taken_count.saturating_add(1);
            med.next_dose_time += med.frequency_hours as u64 * 3600;
        }
    }

    fn adherence_rate(&self, med_idx: usize) -> u32 {
        if let Some(med) = self.medications.get(med_idx) {
            let total = med.taken_count + med.missed_count;
            if total == 0 {
                return 100;
            }
            (med.taken_count * 100) / total
        } else {
            0
        }
    }
}

pub fn init() {
    let mut m = MEDICAL.lock();
    *m = Some(MedicalEngine::new());
    serial_println!("    Health: medical records (meds, vaccines, allergies) ready");
}
