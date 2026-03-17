//! Audio codec implementations
//!
//! This module provides encoder and decoder implementations for all supported codecs.
//! Uses static dispatch (no Box/dyn) to remain heap-free.

pub mod aac;
pub mod alac;
pub mod flac;
pub mod ldac;
pub mod mp3;
pub mod opus;
pub mod pcm;
pub mod vorbis;

use crate::audio::error::*;
use crate::audio::types::*;

/// Codec encoder trait
pub trait Encoder {
    /// Initialize the encoder with given parameters
    fn init(&mut self, config: &AudioConfig) -> Result<()>;

    /// Encode an audio frame
    fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize>;

    /// Flush any buffered data
    fn flush(&mut self, output: &mut [u8]) -> Result<usize>;

    /// Get codec capabilities
    fn capabilities(&self) -> CodecCapabilities;
}

/// Codec decoder trait
pub trait Decoder {
    /// Initialize the decoder with given parameters
    fn init(&mut self, config: &AudioConfig) -> Result<()>;

    /// Decode an audio packet
    fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize>;

    /// Reset decoder state
    fn reset(&mut self);

    /// Get codec capabilities
    fn capabilities(&self) -> CodecCapabilities;
}

/// Static codec encoder union — holds one concrete encoder at a time (no heap).
pub enum StaticEncoder {
    Aac(aac::AacEncoder),
    Mp3(mp3::Mp3Encoder),
    Opus(opus::OpusEncoder),
    Flac(flac::FlacEncoder),
    Vorbis(vorbis::VorbisEncoder),
    Alac(alac::AlacEncoder),
    Ldac(ldac::LdacEncoder),
    Pcm(pcm::PcmEncoder),
}

/// Static codec decoder union — holds one concrete decoder at a time (no heap).
pub enum StaticDecoder {
    Aac(aac::AacDecoder),
    Mp3(mp3::Mp3Decoder),
    Opus(opus::OpusDecoder),
    Flac(flac::FlacDecoder),
    Vorbis(vorbis::VorbisDecoder),
    Alac(alac::AlacDecoder),
    Ldac(ldac::LdacDecoder),
    Pcm(pcm::PcmDecoder),
}

impl StaticEncoder {
    pub fn init(&mut self, config: &AudioConfig) -> Result<()> {
        match self {
            Self::Aac(e) => e.init(config),
            Self::Mp3(e) => e.init(config),
            Self::Opus(e) => e.init(config),
            Self::Flac(e) => e.init(config),
            Self::Vorbis(e) => e.init(config),
            Self::Alac(e) => e.init(config),
            Self::Ldac(e) => e.init(config),
            Self::Pcm(e) => e.init(config),
        }
    }

    pub fn encode(&mut self, frame: &AudioFrame, output: &mut [u8]) -> Result<usize> {
        match self {
            Self::Aac(e) => e.encode(frame, output),
            Self::Mp3(e) => e.encode(frame, output),
            Self::Opus(e) => e.encode(frame, output),
            Self::Flac(e) => e.encode(frame, output),
            Self::Vorbis(e) => e.encode(frame, output),
            Self::Alac(e) => e.encode(frame, output),
            Self::Ldac(e) => e.encode(frame, output),
            Self::Pcm(e) => e.encode(frame, output),
        }
    }

    pub fn flush(&mut self, output: &mut [u8]) -> Result<usize> {
        match self {
            Self::Aac(e) => e.flush(output),
            Self::Mp3(e) => e.flush(output),
            Self::Opus(e) => e.flush(output),
            Self::Flac(e) => e.flush(output),
            Self::Vorbis(e) => e.flush(output),
            Self::Alac(e) => e.flush(output),
            Self::Ldac(e) => e.flush(output),
            Self::Pcm(e) => e.flush(output),
        }
    }

    pub fn capabilities(&self) -> CodecCapabilities {
        match self {
            Self::Aac(e) => e.capabilities(),
            Self::Mp3(e) => e.capabilities(),
            Self::Opus(e) => e.capabilities(),
            Self::Flac(e) => e.capabilities(),
            Self::Vorbis(e) => e.capabilities(),
            Self::Alac(e) => e.capabilities(),
            Self::Ldac(e) => e.capabilities(),
            Self::Pcm(e) => e.capabilities(),
        }
    }
}

impl StaticDecoder {
    pub fn init(&mut self, config: &AudioConfig) -> Result<()> {
        match self {
            Self::Aac(d) => d.init(config),
            Self::Mp3(d) => d.init(config),
            Self::Opus(d) => d.init(config),
            Self::Flac(d) => d.init(config),
            Self::Vorbis(d) => d.init(config),
            Self::Alac(d) => d.init(config),
            Self::Ldac(d) => d.init(config),
            Self::Pcm(d) => d.init(config),
        }
    }

    pub fn decode(&mut self, packet: &AudioPacket, output: &mut [u8]) -> Result<usize> {
        match self {
            Self::Aac(d) => d.decode(packet, output),
            Self::Mp3(d) => d.decode(packet, output),
            Self::Opus(d) => d.decode(packet, output),
            Self::Flac(d) => d.decode(packet, output),
            Self::Vorbis(d) => d.decode(packet, output),
            Self::Alac(d) => d.decode(packet, output),
            Self::Ldac(d) => d.decode(packet, output),
            Self::Pcm(d) => d.decode(packet, output),
        }
    }

    pub fn reset(&mut self) {
        match self {
            Self::Aac(d) => d.reset(),
            Self::Mp3(d) => d.reset(),
            Self::Opus(d) => d.reset(),
            Self::Flac(d) => d.reset(),
            Self::Vorbis(d) => d.reset(),
            Self::Alac(d) => d.reset(),
            Self::Ldac(d) => d.reset(),
            Self::Pcm(d) => d.reset(),
        }
    }

    pub fn capabilities(&self) -> CodecCapabilities {
        match self {
            Self::Aac(d) => d.capabilities(),
            Self::Mp3(d) => d.capabilities(),
            Self::Opus(d) => d.capabilities(),
            Self::Flac(d) => d.capabilities(),
            Self::Vorbis(d) => d.capabilities(),
            Self::Alac(d) => d.capabilities(),
            Self::Ldac(d) => d.capabilities(),
            Self::Pcm(d) => d.capabilities(),
        }
    }
}

/// Create a static encoder for the given codec.
pub fn get_encoder(codec: CodecId) -> Result<StaticEncoder> {
    match codec {
        CodecId::AAC => Ok(StaticEncoder::Aac(aac::AacEncoder::new())),
        CodecId::MP3 => Ok(StaticEncoder::Mp3(mp3::Mp3Encoder::new())),
        CodecId::Opus => Ok(StaticEncoder::Opus(opus::OpusEncoder::new())),
        CodecId::FLAC => Ok(StaticEncoder::Flac(flac::FlacEncoder::new())),
        CodecId::Vorbis => Ok(StaticEncoder::Vorbis(vorbis::VorbisEncoder::new())),
        CodecId::ALAC => Ok(StaticEncoder::Alac(alac::AlacEncoder::new())),
        CodecId::LDAC => Ok(StaticEncoder::Ldac(ldac::LdacEncoder::new())),
        CodecId::PCM => Ok(StaticEncoder::Pcm(pcm::PcmEncoder::new())),
    }
}

/// Create a static decoder for the given codec.
pub fn get_decoder(codec: CodecId) -> Result<StaticDecoder> {
    match codec {
        CodecId::AAC => Ok(StaticDecoder::Aac(aac::AacDecoder::new())),
        CodecId::MP3 => Ok(StaticDecoder::Mp3(mp3::Mp3Decoder::new())),
        CodecId::Opus => Ok(StaticDecoder::Opus(opus::OpusDecoder::new())),
        CodecId::FLAC => Ok(StaticDecoder::Flac(flac::FlacDecoder::new())),
        CodecId::Vorbis => Ok(StaticDecoder::Vorbis(vorbis::VorbisDecoder::new())),
        CodecId::ALAC => Ok(StaticDecoder::Alac(alac::AlacDecoder::new())),
        CodecId::LDAC => Ok(StaticDecoder::Ldac(ldac::LdacDecoder::new())),
        CodecId::PCM => Ok(StaticDecoder::Pcm(pcm::PcmDecoder::new())),
    }
}
