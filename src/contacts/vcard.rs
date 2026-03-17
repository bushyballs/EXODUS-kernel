// vCard parser/generator for Genesis contacts
// Supports import/export, fields, photo, multi-value, groups

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;

// ── vCard version ───────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VCardVersion {
    V21,
    V30,
    V40,
}

// ── Phone type for vCard TEL property ───────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VCardPhoneType {
    Cell,
    Home,
    Work,
    Fax,
    Pager,
    Voice,
    Other,
}

// ── Email type for vCard EMAIL property ─────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VCardEmailType {
    Home,
    Work,
    Internet,
    Other,
}

// ── Address type for vCard ADR property ─────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VCardAddrType {
    Home,
    Work,
    Postal,
    Other,
}

// ── Photo encoding ──────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PhotoEncoding {
    Base64,
    Uri,
    None,
}

// ── Parse state machine ─────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParseState {
    Idle,
    InCard,
    ReadingProperty,
    ReadingValue,
    FoldedLine,
    Complete,
    Error,
}

// ── Single phone entry ──────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VCardPhone {
    pub number_hash: u64,
    pub phone_type: VCardPhoneType,
    pub preferred: bool,
}

// ── Single email entry ──────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VCardEmail {
    pub address_hash: u64,
    pub email_type: VCardEmailType,
    pub preferred: bool,
}

// ── Single address entry ────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VCardAddress {
    pub street_hash: u64,
    pub city_hash: u64,
    pub state_hash: u64,
    pub zip_hash: u64,
    pub country_hash: u64,
    pub addr_type: VCardAddrType,
}

// ── Photo attachment ────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VCardPhoto {
    pub data_hash: u64,
    pub encoding: PhotoEncoding,
    pub mime_hash: u64,
    pub size_bytes: u32,
}

impl VCardPhoto {
    pub fn new() -> Self {
        Self {
            data_hash: 0,
            encoding: PhotoEncoding::None,
            mime_hash: 0,
            size_bytes: 0,
        }
    }

    pub fn has_photo(&self) -> bool {
        self.encoding != PhotoEncoding::None && self.data_hash != 0
    }
}

// ── Complete vCard record ───────────────────────────────────────────
#[derive(Clone, Copy)]
pub struct VCard {
    pub id: u32,
    pub version: VCardVersion,
    pub name_hash: u64,
    pub formatted_name_hash: u64,
    pub first_name_hash: u64,
    pub last_name_hash: u64,
    pub middle_name_hash: u64,
    pub prefix_hash: u64,
    pub suffix_hash: u64,
    pub nickname_hash: u64,
    pub org_hash: u64,
    pub title_hash: u64,
    pub role_hash: u64,
    pub note_hash: u64,
    pub url_hash: u64,
    pub birthday_month: u8,
    pub birthday_day: u8,
    pub birthday_year: u16,
    pub anniversary_month: u8,
    pub anniversary_day: u8,
    pub anniversary_year: u16,
    pub phones: [VCardPhone; 8],
    pub phone_count: u8,
    pub emails: [VCardEmail; 8],
    pub email_count: u8,
    pub addresses: [VCardAddress; 4],
    pub address_count: u8,
    pub photo: VCardPhoto,
    pub group_hash: u64,
    pub categories_hash: u64,
    pub uid_hash: u64,
    pub rev_timestamp: u64,
    pub prodid_hash: u64,
}

impl VCard {
    pub fn new(id: u32, version: VCardVersion) -> Self {
        Self {
            id,
            version,
            name_hash: 0,
            formatted_name_hash: 0,
            first_name_hash: 0,
            last_name_hash: 0,
            middle_name_hash: 0,
            prefix_hash: 0,
            suffix_hash: 0,
            nickname_hash: 0,
            org_hash: 0,
            title_hash: 0,
            role_hash: 0,
            note_hash: 0,
            url_hash: 0,
            birthday_month: 0,
            birthday_day: 0,
            birthday_year: 0,
            anniversary_month: 0,
            anniversary_day: 0,
            anniversary_year: 0,
            phones: [VCardPhone { number_hash: 0, phone_type: VCardPhoneType::Other, preferred: false }; 8],
            phone_count: 0,
            emails: [VCardEmail { address_hash: 0, email_type: VCardEmailType::Other, preferred: false }; 8],
            email_count: 0,
            addresses: [VCardAddress {
                street_hash: 0, city_hash: 0, state_hash: 0,
                zip_hash: 0, country_hash: 0, addr_type: VCardAddrType::Other,
            }; 4],
            address_count: 0,
            photo: VCardPhoto::new(),
            group_hash: 0,
            categories_hash: 0,
            uid_hash: 0,
            rev_timestamp: 0,
            prodid_hash: 0,
        }
    }

    pub fn add_phone(&mut self, number_hash: u64, phone_type: VCardPhoneType, preferred: bool) -> bool {
        if self.phone_count < 8 {
            let idx = self.phone_count as usize;
            self.phones[idx] = VCardPhone { number_hash, phone_type, preferred };
            self.phone_count = self.phone_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_email(&mut self, address_hash: u64, email_type: VCardEmailType, preferred: bool) -> bool {
        if self.email_count < 8 {
            let idx = self.email_count as usize;
            self.emails[idx] = VCardEmail { address_hash, email_type, preferred };
            self.email_count = self.email_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn add_address(&mut self, addr: VCardAddress) -> bool {
        if self.address_count < 4 {
            self.addresses[self.address_count as usize] = addr;
            self.address_count = self.address_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    pub fn set_photo(&mut self, data_hash: u64, encoding: PhotoEncoding, mime_hash: u64, size: u32) {
        self.photo = VCardPhoto {
            data_hash,
            encoding,
            mime_hash,
            size_bytes: size,
        };
    }

    pub fn preferred_phone(&self) -> Option<u64> {
        for i in 0..self.phone_count as usize {
            if self.phones[i].preferred {
                return Some(self.phones[i].number_hash);
            }
        }
        if self.phone_count > 0 {
            Some(self.phones[0].number_hash)
        } else {
            None
        }
    }

    pub fn preferred_email(&self) -> Option<u64> {
        for i in 0..self.email_count as usize {
            if self.emails[i].preferred {
                return Some(self.emails[i].address_hash);
            }
        }
        if self.email_count > 0 {
            Some(self.emails[0].address_hash)
        } else {
            None
        }
    }

    pub fn has_birthday(&self) -> bool {
        self.birthday_month > 0 && self.birthday_day > 0
    }

    pub fn has_anniversary(&self) -> bool {
        self.anniversary_month > 0 && self.anniversary_day > 0
    }
}

// ── VCard parser ────────────────────────────────────────────────────
pub struct VCardParser {
    state: ParseState,
    current_property_hash: u64,
    line_count: u32,
    card_count: u32,
    error_count: u32,
}

impl VCardParser {
    pub fn new() -> Self {
        Self {
            state: ParseState::Idle,
            current_property_hash: 0,
            line_count: 0,
            card_count: 0,
            error_count: 0,
        }
    }

    pub fn begin_parse(&mut self) {
        self.state = ParseState::Idle;
        self.line_count = 0;
        self.card_count = 0;
        self.error_count = 0;
    }

    pub fn feed_line(&mut self, line_hash: u64, is_begin: bool, is_end: bool) -> ParseState {
        self.line_count = self.line_count.saturating_add(1);
        match self.state {
            ParseState::Idle => {
                if is_begin {
                    self.state = ParseState::InCard;
                }
            }
            ParseState::InCard | ParseState::ReadingProperty => {
                if is_end {
                    self.card_count = self.card_count.saturating_add(1);
                    self.state = ParseState::Complete;
                } else {
                    self.current_property_hash = line_hash;
                    self.state = ParseState::ReadingValue;
                }
            }
            ParseState::ReadingValue => {
                if is_end {
                    self.card_count = self.card_count.saturating_add(1);
                    self.state = ParseState::Complete;
                } else {
                    self.current_property_hash = line_hash;
                }
            }
            ParseState::FoldedLine => {
                self.state = ParseState::ReadingValue;
            }
            ParseState::Complete => {
                if is_begin {
                    self.state = ParseState::InCard;
                } else {
                    self.state = ParseState::Idle;
                }
            }
            ParseState::Error => {
                self.error_count = self.error_count.saturating_add(1);
            }
        }
        self.state
    }

    pub fn cards_parsed(&self) -> u32 {
        self.card_count
    }

    pub fn errors(&self) -> u32 {
        self.error_count
    }

    pub fn state(&self) -> ParseState {
        self.state
    }
}

// ── VCard store (manages collection of vCards) ──────────────────────
pub struct VCardStore {
    cards: Vec<VCard>,
    next_id: u32,
    parser: VCardParser,
    export_count: u32,
    import_count: u32,
}

impl VCardStore {
    pub fn new() -> Self {
        Self {
            cards: vec![],
            next_id: 1,
            parser: VCardParser::new(),
            export_count: 0,
            import_count: 0,
        }
    }

    pub fn create_card(&mut self, version: VCardVersion) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let card = VCard::new(id, version);
        self.cards.push(card);
        id
    }

    pub fn import_card(&mut self, card: VCard) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut imported = card;
        imported.id = id;
        self.cards.push(imported);
        self.import_count = self.import_count.saturating_add(1);
        id
    }

    pub fn export_card(&mut self, id: u32) -> Option<&VCard> {
        if let Some(card) = self.cards.iter().find(|c| c.id == id) {
            self.export_count = self.export_count.saturating_add(1);
            Some(card)
        } else {
            None
        }
    }

    pub fn remove_card(&mut self, id: u32) -> bool {
        if let Some(pos) = self.cards.iter().position(|c| c.id == id) {
            self.cards.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get_card(&self, id: u32) -> Option<&VCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    pub fn get_card_mut(&mut self, id: u32) -> Option<&mut VCard> {
        self.cards.iter_mut().find(|c| c.id == id)
    }

    pub fn find_by_uid(&self, uid_hash: u64) -> Option<&VCard> {
        self.cards.iter().find(|c| c.uid_hash == uid_hash)
    }

    pub fn find_by_name(&self, name_hash: u64) -> Vec<u32> {
        self.cards.iter()
            .filter(|c| c.name_hash == name_hash || c.formatted_name_hash == name_hash)
            .map(|c| c.id)
            .collect()
    }

    pub fn find_by_group(&self, group_hash: u64) -> Vec<u32> {
        self.cards.iter()
            .filter(|c| c.group_hash == group_hash)
            .map(|c| c.id)
            .collect()
    }

    pub fn find_by_org(&self, org_hash: u64) -> Vec<u32> {
        self.cards.iter()
            .filter(|c| c.org_hash == org_hash)
            .map(|c| c.id)
            .collect()
    }

    pub fn cards_with_photo(&self) -> Vec<u32> {
        self.cards.iter()
            .filter(|c| c.photo.has_photo())
            .map(|c| c.id)
            .collect()
    }

    pub fn cards_with_birthday(&self) -> Vec<u32> {
        self.cards.iter()
            .filter(|c| c.has_birthday())
            .map(|c| c.id)
            .collect()
    }

    pub fn total_cards(&self) -> usize {
        self.cards.len()
    }

    pub fn total_imported(&self) -> u32 {
        self.import_count
    }

    pub fn total_exported(&self) -> u32 {
        self.export_count
    }

    pub fn parser_mut(&mut self) -> &mut VCardParser {
        &mut self.parser
    }
}

static VCARD_STORE: Mutex<Option<VCardStore>> = Mutex::new(None);

pub fn init() {
    let mut store = VCARD_STORE.lock();
    *store = Some(VCardStore::new());
    serial_println!("[CONTACTS] vCard parser/generator initialized");
}

pub fn get_vcard_store() -> &'static Mutex<Option<VCardStore>> {
    &VCARD_STORE
}
