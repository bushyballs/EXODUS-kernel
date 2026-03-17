/// NFC (Near Field Communication) for Genesis
///
/// Tag reading/writing, peer-to-peer, card emulation,
/// NDEF records, and contactless payment support.
///
/// Inspired by: Android NFC, libnfc. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// NFC tag type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagType {
    Type1, // Topaz
    Type2, // NTAG/Mifare UL
    Type3, // FeliCa
    Type4, // ISO-DEP
    MifareClassic,
    IsoDep,
}

/// NDEF record type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdefType {
    Text,
    Uri,
    SmartPoster,
    MimeMedia,
    ExternalType,
    Unknown,
}

/// An NDEF record
pub struct NdefRecord {
    pub ndef_type: NdefType,
    pub payload: Vec<u8>,
    pub id: Vec<u8>,
    pub language: String,
}

/// NFC mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NfcMode {
    ReaderWriter,
    PeerToPeer,
    CardEmulation,
}

/// NFC tag
pub struct NfcTag {
    pub uid: Vec<u8>,
    pub tag_type: TagType,
    pub records: Vec<NdefRecord>,
    pub writable: bool,
    pub max_size: usize,
}

/// NFC controller
pub struct NfcController {
    pub enabled: bool,
    pub mode: NfcMode,
    pub discovered_tag: Option<NfcTag>,
    pub beam_enabled: bool,
    pub card_emulation_aid: Vec<Vec<u8>>,
    pub tap_pay_enabled: bool,
}

impl NfcController {
    const fn new() -> Self {
        NfcController {
            enabled: false,
            mode: NfcMode::ReaderWriter,
            discovered_tag: None,
            beam_enabled: false,
            card_emulation_aid: Vec::new(),
            tap_pay_enabled: false,
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }
    pub fn disable(&mut self) {
        self.enabled = false;
    }

    pub fn set_mode(&mut self, mode: NfcMode) {
        self.mode = mode;
    }

    pub fn on_tag_discovered(&mut self, tag: NfcTag) {
        if !self.enabled {
            return;
        }
        crate::serial_println!(
            "  [nfc] Tag discovered: UID={} bytes, {} records",
            tag.uid.len(),
            tag.records.len()
        );
        self.discovered_tag = Some(tag);
    }

    pub fn write_ndef(&mut self, record: NdefRecord) -> bool {
        if let Some(ref mut tag) = self.discovered_tag {
            if !tag.writable {
                return false;
            }
            tag.records.push(record);
            true
        } else {
            false
        }
    }

    pub fn register_aid(&mut self, aid: &[u8]) {
        self.card_emulation_aid.push(aid.to_vec());
    }

    pub fn create_text_record(text: &str) -> NdefRecord {
        NdefRecord {
            ndef_type: NdefType::Text,
            payload: text.as_bytes().to_vec(),
            id: Vec::new(),
            language: String::from("en"),
        }
    }

    pub fn create_uri_record(uri: &str) -> NdefRecord {
        NdefRecord {
            ndef_type: NdefType::Uri,
            payload: uri.as_bytes().to_vec(),
            id: Vec::new(),
            language: String::new(),
        }
    }
}

static NFC: Mutex<NfcController> = Mutex::new(NfcController::new());

pub fn init() {
    crate::serial_println!("  [connectivity] NFC controller initialized");
}

pub fn enable() {
    NFC.lock().enable();
}
pub fn disable() {
    NFC.lock().disable();
}
