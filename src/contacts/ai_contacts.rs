use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SuggestReason {
    FrequentContact,
    RecentConversation,
    SameCompany,
    TimeOfDay,
    LocationBased,
}

#[derive(Clone, Copy)]
pub struct ContactSuggestion {
    pub contact_id: u32,
    pub score: u32,
    pub reason: SuggestReason,
}

impl ContactSuggestion {
    pub fn new(contact_id: u32, score: u32, reason: SuggestReason) -> Self {
        Self {
            contact_id,
            score,
            reason,
        }
    }
}

pub struct AiContactEngine {
    suggestions: Vec<ContactSuggestion>,
    contact_patterns: Vec<(u32, u8, u32)>, // (contact_id, hour_of_day, frequency)
    duplicate_candidates: Vec<(u32, u32, u8)>, // (contact_id_1, contact_id_2, similarity_score)
}

impl AiContactEngine {
    pub fn new() -> Self {
        Self {
            suggestions: vec![],
            contact_patterns: vec![],
            duplicate_candidates: vec![],
        }
    }

    pub fn suggest_contacts(
        &mut self,
        current_time: u64,
        contact_frequencies: &[(u32, u32, u64)], // (id, frequency, last_contacted)
    ) -> Vec<u32> {
        self.suggestions.clear();

        // Extract hour of day from timestamp (simplified - assumes seconds since epoch)
        let hour_of_day = ((current_time / 3600) % 24) as u8;

        for &(contact_id, frequency, last_contacted) in contact_frequencies {
            let mut score = frequency;

            // Boost score based on time-of-day patterns
            if let Some(&(_, pattern_hour, pattern_freq)) = self
                .contact_patterns
                .iter()
                .find(|&&(id, _, _)| id == contact_id)
            {
                // If contact is usually contacted at this hour, boost score
                if pattern_hour == hour_of_day {
                    score += pattern_freq * 2;
                }
            }

            // Boost score for recent contacts
            let time_since_contact = current_time.saturating_sub(last_contacted);
            if time_since_contact < 3600 {
                // Less than 1 hour
                score += 100;
            } else if time_since_contact < 86400 {
                // Less than 1 day
                score += 50;
            }

            let reason = if frequency > 100 {
                SuggestReason::FrequentContact
            } else if time_since_contact < 3600 {
                SuggestReason::RecentConversation
            } else {
                SuggestReason::TimeOfDay
            };

            self.suggestions
                .push(ContactSuggestion::new(contact_id, score, reason));
        }

        // Sort by score
        self.suggestions.sort_by(|a, b| b.score.cmp(&a.score));

        self.suggestions.iter().map(|s| s.contact_id).collect()
    }

    pub fn detect_duplicates(
        &mut self,
        contacts: &[(u32, u64, &[u64])], // (id, name_hash, phone_hashes)
    ) -> Vec<(u32, u32)> {
        self.duplicate_candidates.clear();

        for i in 0..contacts.len() {
            for j in (i + 1)..contacts.len() {
                let (id1, name1, phones1) = contacts[i];
                let (id2, name2, phones2) = contacts[j];

                let mut similarity = 0u8;

                // Check name similarity
                if name1 == name2 {
                    similarity += 100;
                }

                // Check phone number overlap
                for &phone1 in phones1.iter() {
                    if phone1 != 0 && phones2.contains(&phone1) {
                        similarity = similarity.saturating_add(50);
                    }
                }

                // If similarity is high enough, mark as duplicate candidate
                if similarity >= 100 {
                    self.duplicate_candidates.push((id1, id2, similarity));
                }
            }
        }

        // Sort by similarity score
        self.duplicate_candidates.sort_by(|a, b| b.2.cmp(&a.2));

        self.duplicate_candidates
            .iter()
            .map(|&(id1, id2, _)| (id1, id2))
            .collect()
    }

    pub fn predict_best_contact_time(&self, contact_id: u32) -> Option<u8> {
        // Find the most common hour for this contact
        let patterns: Vec<&(u32, u8, u32)> = self
            .contact_patterns
            .iter()
            .filter(|&&(id, _, _)| id == contact_id)
            .collect();

        if patterns.is_empty() {
            return None;
        }

        // Return the hour with highest frequency
        patterns
            .iter()
            .max_by_key(|&&(_, _, freq)| freq)
            .map(|&&(_, hour, _)| hour)
    }

    pub fn auto_categorize_contacts(
        &self,
        contacts: &[(u32, u64, u32)], // (id, company_hash, frequency)
    ) -> Vec<(u32, &'static str)> {
        let mut categories = vec![];

        for &(contact_id, company_hash, frequency) in contacts {
            let category = if frequency > 100 {
                "frequent"
            } else if frequency > 50 {
                "regular"
            } else if company_hash != 0 {
                "work"
            } else {
                "occasional"
            };

            categories.push((contact_id, category));
        }

        categories
    }

    pub fn record_contact_pattern(&mut self, contact_id: u32, hour_of_day: u8) {
        // Find existing pattern for this contact and hour
        if let Some(pattern) = self
            .contact_patterns
            .iter_mut()
            .find(|&&mut (id, hour, _)| id == contact_id && hour == hour_of_day)
        {
            pattern.2 = pattern.2.saturating_add(1); // Increment frequency
        } else {
            self.contact_patterns.push((contact_id, hour_of_day, 1));
        }
    }

    pub fn get_suggestions(&self) -> &[ContactSuggestion] {
        &self.suggestions
    }

    pub fn get_duplicate_candidates(&self) -> &[(u32, u32, u8)] {
        &self.duplicate_candidates
    }

    pub fn total_patterns(&self) -> usize {
        self.contact_patterns.len()
    }

    pub fn total_duplicates(&self) -> usize {
        self.duplicate_candidates.len()
    }
}

static AI_CONTACTS: Mutex<Option<AiContactEngine>> = Mutex::new(None);

pub fn init() {
    let mut ai_contacts = AI_CONTACTS.lock();
    *ai_contacts = Some(AiContactEngine::new());
    serial_println!("[CONTACTS] AI contact engine initialized");
}

pub fn get_ai_engine() -> &'static Mutex<Option<AiContactEngine>> {
    &AI_CONTACTS
}
