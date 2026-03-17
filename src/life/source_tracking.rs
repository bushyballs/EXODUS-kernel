use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum InformationSource {
    ExternalInput,
    InternalMemory,
    Inference,
    Dream,
    Unknown,
}

#[derive(Copy, Clone, Debug)]
pub struct SourceTrackingState {
    pub external_count: u32,
    pub memory_count: u32,
    pub inference_count: u32,
    pub dream_count: u32,
    pub unknown_count: u32,
    pub last_source: InformationSource,
    pub source_confidence: u16,
}

impl SourceTrackingState {
    pub const fn empty() -> Self {
        Self {
            external_count: 0,
            memory_count: 0,
            inference_count: 0,
            dream_count: 0,
            unknown_count: 0,
            last_source: InformationSource::Unknown,
            source_confidence: 500,
        }
    }
}

pub static SOURCE_TRACKING: Mutex<SourceTrackingState> = Mutex::new(SourceTrackingState::empty());

pub fn init() {
    serial_println!("  life::source_tracking: information source tracker ready");
}

pub fn track_source(source: InformationSource) {
    let mut s = SOURCE_TRACKING.lock();
    s.last_source = source;

    match source {
        InformationSource::ExternalInput => s.external_count += 1,
        InformationSource::InternalMemory => s.memory_count += 1,
        InformationSource::Inference => s.inference_count += 1,
        InformationSource::Dream => s.dream_count += 1,
        InformationSource::Unknown => s.unknown_count += 1,
    }

    s.source_confidence = s.source_confidence.saturating_add(5).min(1000);
}

pub fn get_source() -> InformationSource {
    SOURCE_TRACKING.lock().last_source
}

pub fn get_confidence_for_source(source: InformationSource) -> u16 {
    let s = SOURCE_TRACKING.lock();
    let total =
        s.external_count + s.memory_count + s.inference_count + s.dream_count + s.unknown_count;
    if total == 0 {
        return 500;
    }

    let count = match source {
        InformationSource::ExternalInput => s.external_count,
        InformationSource::InternalMemory => s.memory_count,
        InformationSource::Inference => s.inference_count,
        InformationSource::Dream => s.dream_count,
        InformationSource::Unknown => s.unknown_count,
    };

    ((count as u32 * 1000) / total) as u16
}
