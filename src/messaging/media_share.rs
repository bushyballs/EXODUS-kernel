use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
/// Media attachment sharing subsystem for Genesis OS
///
/// Provides:
///   - Typed media attachments (Image, Video, Audio, Document, Contact, Location)
///   - Attach / download / compress / thumbnail generation stubs
///   - In-memory media store with size tracking
///   - Quota enforcement per user
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum single attachment size in bytes (16 MiB).
const MAX_ATTACHMENT_SIZE: usize = 16 * 1024 * 1024;

/// Maximum total stored media bytes (128 MiB).
const MAX_TOTAL_MEDIA_BYTES: usize = 128 * 1024 * 1024;

/// Thumbnail target side length in pixels (Q16 fixed-point not needed here
/// because we are dealing with whole pixels).
const THUMBNAIL_SIZE: u32 = 128;

/// JPEG quality level for compressed images (0-100 integer scale).
const COMPRESS_QUALITY: u32 = 75;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The kind of media carried by an attachment.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Image,
    Video,
    Audio,
    Document,
    Contact,
    Location,
}

/// Transfer state of an attachment.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    Uploading,
    Available,
    Downloading,
    Failed,
}

/// A single media attachment.
#[derive(Clone)]
pub struct MediaAttachment {
    pub id: u64,
    pub media_type: MediaType,
    pub data_hash: u64,
    pub size_bytes: usize,
    pub thumbnail_hash: u64,
    pub caption_hash: u64,
    pub owner_hash: u64,
    pub timestamp: u64,
    pub status: TransferStatus,
    /// Raw data blob (in a real system this would be an on-disk reference).
    pub data: Vec<u8>,
    /// Generated thumbnail data.
    pub thumbnail: Vec<u8>,
}

/// Manager for all media attachments.
pub struct MediaManager {
    attachments: Vec<MediaAttachment>,
    next_id: u64,
    total_bytes: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MEDIA_MANAGER: Mutex<Option<MediaManager>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Hashing utility
// ---------------------------------------------------------------------------

fn fnv_hash(data: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    h
}

// ---------------------------------------------------------------------------
// Image processing stubs (no f32/f64 -- integer only)
// ---------------------------------------------------------------------------

/// Simulate image compression by stripping every Nth byte.
///
/// Real compression would use a DCT / quantisation pipeline.  Here we simply
/// drop bytes at regular intervals to reduce size, simulating a quality
/// reduction.  `quality` is in 0..=100 -- higher keeps more data.
fn compress_image_stub(data: &[u8], quality: u32) -> Vec<u8> {
    if data.is_empty() || quality >= 100 {
        return data.into();
    }
    // keep_ratio = quality / 100 (integer division => 0..1 mapped to 0..100).
    // We keep every `step`-th byte where step = 100 / (100 - quality).
    let drop_pct = 100u32.saturating_sub(quality);
    if drop_pct == 0 {
        return data.into();
    }
    // step = how many bytes between each dropped byte
    let step = if drop_pct > 0 { 100 / drop_pct } else { 0 } as usize;
    if step == 0 {
        // drop everything -- degenerate
        return vec![];
    }
    let mut out = Vec::with_capacity(data.len());
    for (i, &b) in data.iter().enumerate() {
        if i % (step + 1) != 0 {
            out.push(b);
        }
    }
    out
}

/// Generate a thumbnail by taking every Nth sample from the data.
///
/// For a real image this would down-scale to THUMBNAIL_SIZE x THUMBNAIL_SIZE.
/// Here we simply decimate the byte stream to approximate a smaller
/// representation.
fn generate_thumbnail_stub(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return vec![];
    }
    // Target thumbnail byte count: THUMBNAIL_SIZE * THUMBNAIL_SIZE bytes.
    let target = (THUMBNAIL_SIZE * THUMBNAIL_SIZE) as usize;
    if data.len() <= target {
        return data.into();
    }
    let step = data.len() / target;
    let step = if step == 0 { 1 } else { step };
    let mut thumb = Vec::with_capacity(target);
    let mut i: usize = 0;
    while i < data.len() && thumb.len() < target {
        thumb.push(data[i]);
        i += step;
    }
    thumb
}

// ---------------------------------------------------------------------------
// MediaManager implementation
// ---------------------------------------------------------------------------

impl MediaManager {
    pub fn new() -> Self {
        Self {
            attachments: vec![],
            next_id: 1,
            total_bytes: 0,
        }
    }

    /// Attach a new media item.  Returns the attachment id on success.
    pub fn attach_media(
        &mut self,
        media_type: MediaType,
        raw_data: &[u8],
        caption_hash: u64,
        owner_hash: u64,
        timestamp: u64,
    ) -> Option<u64> {
        if raw_data.len() > MAX_ATTACHMENT_SIZE {
            serial_println!(
                "[media_share] attachment too large: {} bytes (max {})",
                raw_data.len(),
                MAX_ATTACHMENT_SIZE
            );
            return None;
        }
        if self.total_bytes + raw_data.len() > MAX_TOTAL_MEDIA_BYTES {
            serial_println!("[media_share] media store quota exceeded");
            return None;
        }

        // For images, auto-compress and generate a thumbnail.
        let (stored_data, thumbnail) = if media_type == MediaType::Image {
            let compressed = compress_image_stub(raw_data, COMPRESS_QUALITY);
            let thumb = generate_thumbnail_stub(&compressed);
            (compressed, thumb)
        } else {
            let thumb = generate_thumbnail_stub(raw_data);
            (raw_data.into(), thumb)
        };

        let data_hash = fnv_hash(&stored_data);
        let thumbnail_hash = fnv_hash(&thumbnail);
        let size_bytes = stored_data.len();

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let attachment = MediaAttachment {
            id,
            media_type,
            data_hash,
            size_bytes,
            thumbnail_hash,
            caption_hash,
            owner_hash,
            timestamp,
            status: TransferStatus::Available,
            data: stored_data,
            thumbnail,
        };

        self.total_bytes += size_bytes;
        self.attachments.push(attachment);

        serial_println!(
            "[media_share] attached media id={} type={} size={}",
            id,
            media_type_name(media_type),
            size_bytes
        );
        Some(id)
    }

    /// Compress an already-stored image attachment in-place.
    pub fn compress_image(&mut self, attachment_id: u64, quality: u32) -> bool {
        if let Some(att) = self.attachments.iter_mut().find(|a| a.id == attachment_id) {
            if att.media_type != MediaType::Image {
                return false;
            }
            let old_size = att.data.len();
            att.data = compress_image_stub(&att.data, quality);
            att.size_bytes = att.data.len();
            att.data_hash = fnv_hash(&att.data);
            // Update total tracked bytes.
            self.total_bytes = self.total_bytes.saturating_sub(old_size) + att.size_bytes;
            serial_println!(
                "[media_share] compressed id={} from {} to {} bytes",
                attachment_id,
                old_size,
                att.size_bytes
            );
            true
        } else {
            false
        }
    }

    /// Regenerate the thumbnail for an attachment.
    pub fn generate_thumbnail(&mut self, attachment_id: u64) -> bool {
        if let Some(att) = self.attachments.iter_mut().find(|a| a.id == attachment_id) {
            att.thumbnail = generate_thumbnail_stub(&att.data);
            att.thumbnail_hash = fnv_hash(&att.thumbnail);
            serial_println!(
                "[media_share] regenerated thumbnail for id={} ({} bytes)",
                attachment_id,
                att.thumbnail.len()
            );
            true
        } else {
            false
        }
    }

    /// "Download" (retrieve) an attachment's data.
    pub fn download_attachment(&mut self, attachment_id: u64) -> Option<Vec<u8>> {
        if let Some(att) = self.attachments.iter_mut().find(|a| a.id == attachment_id) {
            att.status = TransferStatus::Downloading;
            let data = att.data.clone();
            att.status = TransferStatus::Available;
            serial_println!(
                "[media_share] downloaded id={} ({} bytes)",
                attachment_id,
                data.len()
            );
            Some(data)
        } else {
            None
        }
    }

    /// Delete an attachment by id.
    pub fn delete_attachment(&mut self, attachment_id: u64) -> bool {
        if let Some(pos) = self.attachments.iter().position(|a| a.id == attachment_id) {
            let removed = self.attachments.remove(pos);
            self.total_bytes = self.total_bytes.saturating_sub(removed.size_bytes);
            serial_println!("[media_share] deleted id={}", attachment_id);
            true
        } else {
            false
        }
    }

    /// List attachment ids owned by a user.
    pub fn attachments_for_user(&self, owner_hash: u64) -> Vec<u64> {
        let mut result = vec![];
        for att in &self.attachments {
            if att.owner_hash == owner_hash {
                result.push(att.id);
            }
        }
        result
    }

    /// Total bytes currently stored.
    pub fn total_stored_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Total attachment count.
    pub fn attachment_count(&self) -> usize {
        self.attachments.len()
    }

    /// Get attachment metadata (without cloning the full data blob).
    pub fn get_attachment_info(
        &self,
        attachment_id: u64,
    ) -> Option<(MediaType, usize, u64, TransferStatus)> {
        self.attachments
            .iter()
            .find(|a| a.id == attachment_id)
            .map(|a| (a.media_type, a.size_bytes, a.data_hash, a.status))
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn media_type_name(mt: MediaType) -> &'static str {
    match mt {
        MediaType::Image => "Image",
        MediaType::Video => "Video",
        MediaType::Audio => "Audio",
        MediaType::Document => "Document",
        MediaType::Contact => "Contact",
        MediaType::Location => "Location",
    }
}

// ---------------------------------------------------------------------------
// Public API (through the global mutex)
// ---------------------------------------------------------------------------

pub fn attach_media(
    media_type: MediaType,
    raw_data: &[u8],
    caption_hash: u64,
    owner_hash: u64,
    timestamp: u64,
) -> Option<u64> {
    let mut guard = MEDIA_MANAGER.lock();
    let mgr = guard.as_mut()?;
    mgr.attach_media(media_type, raw_data, caption_hash, owner_hash, timestamp)
}

pub fn compress_image(attachment_id: u64, quality: u32) -> bool {
    let mut guard = MEDIA_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.compress_image(attachment_id, quality)
    } else {
        false
    }
}

pub fn generate_thumbnail(attachment_id: u64) -> bool {
    let mut guard = MEDIA_MANAGER.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.generate_thumbnail(attachment_id)
    } else {
        false
    }
}

pub fn download_attachment(attachment_id: u64) -> Option<Vec<u8>> {
    let mut guard = MEDIA_MANAGER.lock();
    let mgr = guard.as_mut()?;
    mgr.download_attachment(attachment_id)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut guard = MEDIA_MANAGER.lock();
    *guard = Some(MediaManager::new());
    serial_println!("[media_share] initialised");
}
