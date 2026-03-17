use crate::sync::Mutex;
/// PDF reader and viewer for Genesis OS
///
/// Provides document viewing with page navigation, zoom controls,
/// text search, bookmarking, and outline/table-of-contents support.
/// Documents and pages are referenced by hashes. Zoom level uses
/// Q16 fixed-point for smooth scaling without floating point.
///
/// Inspired by: Evince, Okular, Adobe Reader. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 constants for zoom
// ---------------------------------------------------------------------------

/// 1.0 in Q16 (100% zoom)
const Q16_ONE: i32 = 65536;
/// Minimum zoom: 0.25 (25%)
const ZOOM_MIN: i32 = 16384;
/// Maximum zoom: 5.0 (500%)
const ZOOM_MAX: i32 = 327680;
/// Zoom step: 0.1 (10%)
const ZOOM_STEP: i32 = 6554;
/// Fit-to-width sentinel value
const ZOOM_FIT_WIDTH: i32 = -1;
/// Fit-to-page sentinel value
const ZOOM_FIT_PAGE: i32 = -2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Page layout mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageLayout {
    SinglePage,
    TwoPage,
    Continuous,
    ContinuousTwoPage,
}

/// Rotation angle
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rotation {
    None,
    Clockwise90,
    Clockwise180,
    Clockwise270,
}

/// Result codes for PDF operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PdfResult {
    Success,
    NotFound,
    InvalidPage,
    AlreadyOpen,
    NoDocument,
    SearchNotFound,
    IoError,
    ParseError,
    PasswordRequired,
}

/// A bookmark within a document
#[derive(Debug, Clone)]
pub struct Bookmark {
    pub page_index: u32,
    pub label_hash: u64,
    pub y_position: i32,
    pub created_epoch: u64,
}

/// An outline (table of contents) entry
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub title_hash: u64,
    pub page_index: u32,
    pub level: u8,
    pub children_count: u16,
}

/// Search result hit
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub page_index: u32,
    pub match_hash: u64,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A single rendered page
#[derive(Debug, Clone)]
pub struct PdfPage {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub text_hash: u64,
    pub rendered: bool,
}

/// An open PDF document
#[derive(Debug, Clone)]
pub struct PdfDocument {
    pub page_count: u32,
    pub current_page: u32,
    pub title_hash: u64,
    pub path_hash: u64,
    pub bookmarks: Vec<Bookmark>,
    pub zoom: i32,
}

/// Annotation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AnnotationType {
    Highlight,
    Underline,
    Strikethrough,
    Note,
    FreeText,
}

/// A simple annotation on a page
#[derive(Debug, Clone)]
pub struct Annotation {
    pub id: u64,
    pub page_index: u32,
    pub annotation_type: AnnotationType,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub content_hash: u64,
    pub color: u32,
}

/// Persistent PDF reader state
struct PdfReaderState {
    document: Option<PdfDocument>,
    pages: Vec<PdfPage>,
    outline: Vec<OutlineEntry>,
    annotations: Vec<Annotation>,
    search_results: Vec<SearchHit>,
    search_query_hash: u64,
    search_current_index: usize,
    layout: PageLayout,
    rotation: Rotation,
    scroll_y: i32,
    night_mode: bool,
    next_annotation_id: u64,
    recent_documents: Vec<u64>,
    max_recent: usize,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PDF_READER: Mutex<Option<PdfReaderState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_state() -> PdfReaderState {
    PdfReaderState {
        document: None,
        pages: Vec::new(),
        outline: Vec::new(),
        annotations: Vec::new(),
        search_results: Vec::new(),
        search_query_hash: 0,
        search_current_index: 0,
        layout: PageLayout::SinglePage,
        rotation: Rotation::None,
        scroll_y: 0,
        night_mode: false,
        next_annotation_id: 1,
        recent_documents: Vec::new(),
        max_recent: 20,
    }
}

fn generate_stub_pages(page_count: u32, path_hash: u64) -> Vec<PdfPage> {
    let mut pages = Vec::new();
    for i in 0..page_count {
        pages.push(PdfPage {
            index: i,
            width: 612,  // US Letter width in points
            height: 792, // US Letter height in points
            text_hash: path_hash.wrapping_add(i as u64 * 0xABCD_EF01),
            rendered: false,
        });
    }
    pages
}

fn generate_stub_outline(page_count: u32, title_hash: u64) -> Vec<OutlineEntry> {
    let mut outline = Vec::new();
    // Generate a simple 2-level outline
    let chapter_count = if page_count > 10 { 5 } else { 2 };
    let pages_per_chapter = page_count / chapter_count.max(1);

    for i in 0..chapter_count {
        let chapter_page = i * pages_per_chapter;
        outline.push(OutlineEntry {
            title_hash: title_hash.wrapping_add(i as u64 * 0x1111),
            page_index: chapter_page,
            level: 0,
            children_count: 2,
        });
        // Sub-sections
        for j in 1u32..=2 {
            let sub_page = chapter_page + j;
            if sub_page < page_count {
                outline.push(OutlineEntry {
                    title_hash: title_hash.wrapping_add((i * 10 + j) as u64 * 0x2222),
                    page_index: sub_page,
                    level: 1,
                    children_count: 0,
                });
            }
        }
    }
    outline
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open a PDF document
pub fn open(path_hash: u64, title_hash: u64, page_count: u32) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    if state.document.is_some() {
        return PdfResult::AlreadyOpen;
    }
    if page_count == 0 {
        return PdfResult::ParseError;
    }

    state.pages = generate_stub_pages(page_count, path_hash);
    state.outline = generate_stub_outline(page_count, title_hash);
    state.annotations.clear();
    state.search_results.clear();
    state.search_query_hash = 0;
    state.search_current_index = 0;
    state.scroll_y = 0;

    state.document = Some(PdfDocument {
        page_count,
        current_page: 0,
        title_hash,
        path_hash,
        bookmarks: Vec::new(),
        zoom: Q16_ONE,
    });

    // Add to recent documents
    state.recent_documents.retain(|&h| h != path_hash);
    state.recent_documents.insert(0, path_hash);
    if state.recent_documents.len() > state.max_recent {
        state.recent_documents.truncate(state.max_recent);
    }

    // Mark first page as rendered
    if !state.pages.is_empty() {
        state.pages[0].rendered = true;
    }

    PdfResult::Success
}

/// Close the current document
pub fn close() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    if state.document.is_none() {
        return PdfResult::NoDocument;
    }
    state.document = None;
    state.pages.clear();
    state.outline.clear();
    state.annotations.clear();
    state.search_results.clear();
    state.scroll_y = 0;
    PdfResult::Success
}

/// Navigate to the next page
pub fn next_page() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    if doc.current_page + 1 >= doc.page_count {
        return PdfResult::InvalidPage;
    }
    doc.current_page += 1;
    // Mark the new page as rendered
    let idx = doc.current_page as usize;
    if idx < state.pages.len() {
        state.pages[idx].rendered = true;
    }
    state.scroll_y = 0;
    PdfResult::Success
}

/// Navigate to the previous page
pub fn prev_page() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    if doc.current_page == 0 {
        return PdfResult::InvalidPage;
    }
    doc.current_page -= 1;
    state.scroll_y = 0;
    PdfResult::Success
}

/// Go to a specific page (0-indexed)
pub fn goto_page(page_index: u32) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    if page_index >= doc.page_count {
        return PdfResult::InvalidPage;
    }
    doc.current_page = page_index;
    let idx = page_index as usize;
    if idx < state.pages.len() {
        state.pages[idx].rendered = true;
    }
    state.scroll_y = 0;
    PdfResult::Success
}

/// Zoom in by one step
pub fn zoom_in() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    // If using fit mode, switch to manual zoom first
    if doc.zoom < 0 {
        doc.zoom = Q16_ONE;
    }
    let new_zoom = doc.zoom + ZOOM_STEP;
    doc.zoom = if new_zoom > ZOOM_MAX {
        ZOOM_MAX
    } else {
        new_zoom
    };
    PdfResult::Success
}

/// Zoom out by one step
pub fn zoom_out() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    if doc.zoom < 0 {
        doc.zoom = Q16_ONE;
    }
    let new_zoom = doc.zoom - ZOOM_STEP;
    doc.zoom = if new_zoom < ZOOM_MIN {
        ZOOM_MIN
    } else {
        new_zoom
    };
    PdfResult::Success
}

/// Set zoom to a specific Q16 value, or use ZOOM_FIT_WIDTH / ZOOM_FIT_PAGE
pub fn set_zoom(zoom_q16: i32) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    if zoom_q16 == ZOOM_FIT_WIDTH || zoom_q16 == ZOOM_FIT_PAGE {
        doc.zoom = zoom_q16;
    } else if zoom_q16 >= ZOOM_MIN && zoom_q16 <= ZOOM_MAX {
        doc.zoom = zoom_q16;
    } else {
        return PdfResult::InvalidPage; // reusing as "invalid parameter"
    }
    PdfResult::Success
}

/// Get the current zoom level
pub fn get_zoom() -> i32 {
    let guard = PDF_READER.lock();
    match guard.as_ref() {
        Some(state) => state.document.as_ref().map(|d| d.zoom).unwrap_or(Q16_ONE),
        None => Q16_ONE,
    }
}

/// Search for text in the document
pub fn search_text(query_hash: u64) -> Vec<SearchHit> {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let _doc = match state.document.as_ref() {
        Some(d) => d,
        None => return Vec::new(),
    };

    state.search_query_hash = query_hash;
    state.search_results.clear();
    state.search_current_index = 0;

    // Simulate finding matches across pages
    for page in state.pages.iter() {
        // Use hash proximity to simulate text matching
        let diff = if page.text_hash > query_hash {
            page.text_hash - query_hash
        } else {
            query_hash - page.text_hash
        };
        if diff < 0x0000_FFFF_FFFF {
            let hit_count = ((diff >> 16) % 3) as u32 + 1;
            for h in 0..hit_count {
                state.search_results.push(SearchHit {
                    page_index: page.index,
                    match_hash: query_hash.wrapping_add(h as u64),
                    x: (h as i32 * 50) + 72,
                    y: (h as i32 * 20) + 100,
                    width: 80,
                    height: 14,
                });
            }
        }
    }

    state.search_results.clone()
}

/// Navigate to the next search result
pub fn next_search_result() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    if state.search_results.is_empty() {
        return PdfResult::SearchNotFound;
    }
    state.search_current_index = (state.search_current_index + 1) % state.search_results.len();
    let page = state.search_results[state.search_current_index].page_index;
    if let Some(doc) = state.document.as_mut() {
        doc.current_page = page;
    }
    PdfResult::Success
}

/// Navigate to the previous search result
pub fn prev_search_result() -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    if state.search_results.is_empty() {
        return PdfResult::SearchNotFound;
    }
    if state.search_current_index == 0 {
        state.search_current_index = state.search_results.len() - 1;
    } else {
        state.search_current_index -= 1;
    }
    let page = state.search_results[state.search_current_index].page_index;
    if let Some(doc) = state.document.as_mut() {
        doc.current_page = page;
    }
    PdfResult::Success
}

/// Add a bookmark at the current page
pub fn add_bookmark(label_hash: u64) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    let page_index = doc.current_page;
    doc.bookmarks.push(Bookmark {
        page_index,
        label_hash,
        y_position: state.scroll_y,
        created_epoch: 1_700_000_000,
    });
    PdfResult::Success
}

/// Remove a bookmark by page index
pub fn remove_bookmark(page_index: u32) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let doc = match state.document.as_mut() {
        Some(d) => d,
        None => return PdfResult::NoDocument,
    };
    let before = doc.bookmarks.len();
    doc.bookmarks.retain(|b| b.page_index != page_index);
    if doc.bookmarks.len() < before {
        PdfResult::Success
    } else {
        PdfResult::NotFound
    }
}

/// Get the document outline (table of contents)
pub fn get_outline() -> Vec<OutlineEntry> {
    let guard = PDF_READER.lock();
    match guard.as_ref() {
        Some(state) => state.outline.clone(),
        None => Vec::new(),
    }
}

/// Get the current document info
pub fn get_document() -> Option<PdfDocument> {
    let guard = PDF_READER.lock();
    guard.as_ref().and_then(|s| s.document.clone())
}

/// Get the current page
pub fn get_current_page() -> Option<PdfPage> {
    let guard = PDF_READER.lock();
    let state = guard.as_ref()?;
    let doc = state.document.as_ref()?;
    state.pages.get(doc.current_page as usize).cloned()
}

/// Set page layout mode
pub fn set_layout(layout: PageLayout) {
    let mut guard = PDF_READER.lock();
    if let Some(state) = guard.as_mut() {
        state.layout = layout;
    }
}

/// Rotate the document view
pub fn rotate(rotation: Rotation) {
    let mut guard = PDF_READER.lock();
    if let Some(state) = guard.as_mut() {
        state.rotation = rotation;
    }
}

/// Toggle night mode (inverted colors)
pub fn toggle_night_mode() {
    let mut guard = PDF_READER.lock();
    if let Some(state) = guard.as_mut() {
        state.night_mode = !state.night_mode;
    }
}

/// Get recent document hashes
pub fn get_recent_documents() -> Vec<u64> {
    let guard = PDF_READER.lock();
    match guard.as_ref() {
        Some(state) => state.recent_documents.clone(),
        None => Vec::new(),
    }
}

/// Add an annotation to the current page
pub fn add_annotation(
    annotation_type: AnnotationType,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    content_hash: u64,
    color: u32,
) -> PdfResult {
    let mut guard = PDF_READER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PdfResult::IoError,
    };
    let page_index = match state.document.as_ref() {
        Some(d) => d.current_page,
        None => return PdfResult::NoDocument,
    };
    let id = state.next_annotation_id;
    state.next_annotation_id += 1;
    state.annotations.push(Annotation {
        id,
        page_index,
        annotation_type,
        x,
        y,
        width,
        height,
        content_hash,
        color,
    });
    PdfResult::Success
}

/// Get annotations for the current page
pub fn get_page_annotations() -> Vec<Annotation> {
    let guard = PDF_READER.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let page_index = match state.document.as_ref() {
        Some(d) => d.current_page,
        None => return Vec::new(),
    };
    state
        .annotations
        .iter()
        .filter(|a| a.page_index == page_index)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the PDF reader subsystem
pub fn init() {
    let mut guard = PDF_READER.lock();
    *guard = Some(default_state());
    serial_println!("    PDF reader ready");
}
