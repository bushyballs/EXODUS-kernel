/// Static file serving for the Genesis webserver
///
/// Serves files from a virtual filesystem with:
///   - MIME type detection by extension
///   - Directory listing (HTML table)
///   - Caching headers (ETag, Last-Modified, Cache-Control)
///   - Range requests (partial content)
///   - Gzip content indication
///   - In-memory file registry (no real FS dependency)
///
/// All code is original. Built for bare-metal Genesis kernel.

use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use alloc::format;
use alloc::collections::BTreeMap;
use crate::sync::Mutex;

use super::http::{HttpRequest, HttpResponse, StatusCode, Method};

// ============================================================================
// MIME type registry
// ============================================================================

/// MIME type entry: extension -> content-type
struct MimeEntry {
    extension: &'static str,
    content_type: &'static str,
}

/// Comprehensive MIME type table
const MIME_TYPES: &[MimeEntry] = &[
    // Text
    MimeEntry { extension: "html", content_type: "text/html; charset=utf-8" },
    MimeEntry { extension: "htm",  content_type: "text/html; charset=utf-8" },
    MimeEntry { extension: "css",  content_type: "text/css; charset=utf-8" },
    MimeEntry { extension: "js",   content_type: "application/javascript; charset=utf-8" },
    MimeEntry { extension: "mjs",  content_type: "application/javascript; charset=utf-8" },
    MimeEntry { extension: "json", content_type: "application/json" },
    MimeEntry { extension: "xml",  content_type: "application/xml" },
    MimeEntry { extension: "txt",  content_type: "text/plain; charset=utf-8" },
    MimeEntry { extension: "csv",  content_type: "text/csv" },
    MimeEntry { extension: "md",   content_type: "text/markdown; charset=utf-8" },
    MimeEntry { extension: "yaml", content_type: "application/yaml" },
    MimeEntry { extension: "yml",  content_type: "application/yaml" },
    MimeEntry { extension: "toml", content_type: "application/toml" },

    // Images
    MimeEntry { extension: "png",  content_type: "image/png" },
    MimeEntry { extension: "jpg",  content_type: "image/jpeg" },
    MimeEntry { extension: "jpeg", content_type: "image/jpeg" },
    MimeEntry { extension: "gif",  content_type: "image/gif" },
    MimeEntry { extension: "svg",  content_type: "image/svg+xml" },
    MimeEntry { extension: "ico",  content_type: "image/x-icon" },
    MimeEntry { extension: "webp", content_type: "image/webp" },
    MimeEntry { extension: "bmp",  content_type: "image/bmp" },

    // Audio
    MimeEntry { extension: "mp3",  content_type: "audio/mpeg" },
    MimeEntry { extension: "wav",  content_type: "audio/wav" },
    MimeEntry { extension: "ogg",  content_type: "audio/ogg" },
    MimeEntry { extension: "flac", content_type: "audio/flac" },
    MimeEntry { extension: "m4a",  content_type: "audio/mp4" },

    // Video
    MimeEntry { extension: "mp4",  content_type: "video/mp4" },
    MimeEntry { extension: "webm", content_type: "video/webm" },
    MimeEntry { extension: "avi",  content_type: "video/x-msvideo" },
    MimeEntry { extension: "mkv",  content_type: "video/x-matroska" },

    // Fonts
    MimeEntry { extension: "woff",  content_type: "font/woff" },
    MimeEntry { extension: "woff2", content_type: "font/woff2" },
    MimeEntry { extension: "ttf",   content_type: "font/ttf" },
    MimeEntry { extension: "otf",   content_type: "font/otf" },
    MimeEntry { extension: "eot",   content_type: "application/vnd.ms-fontobject" },

    // Archives
    MimeEntry { extension: "zip",  content_type: "application/zip" },
    MimeEntry { extension: "gz",   content_type: "application/gzip" },
    MimeEntry { extension: "tar",  content_type: "application/x-tar" },
    MimeEntry { extension: "bz2",  content_type: "application/x-bzip2" },

    // Documents
    MimeEntry { extension: "pdf",  content_type: "application/pdf" },
    MimeEntry { extension: "doc",  content_type: "application/msword" },
    MimeEntry { extension: "docx", content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document" },
    MimeEntry { extension: "xls",  content_type: "application/vnd.ms-excel" },
    MimeEntry { extension: "xlsx", content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" },

    // Binary
    MimeEntry { extension: "wasm", content_type: "application/wasm" },
    MimeEntry { extension: "bin",  content_type: "application/octet-stream" },
    MimeEntry { extension: "exe",  content_type: "application/octet-stream" },
];

/// Look up MIME type for a file extension
pub fn mime_for_extension(ext: &str) -> &'static str {
    let lower = ext.trim_start_matches('.');
    for entry in MIME_TYPES {
        if eq_ignore_case(entry.extension, lower) {
            return entry.content_type;
        }
    }
    "application/octet-stream"
}

/// Extract the file extension from a path
pub fn get_extension(path: &str) -> Option<&str> {
    let filename = path.rsplit('/').next()?;
    let dot_pos = filename.rfind('.')?;
    if dot_pos + 1 < filename.len() {
        Some(&filename[dot_pos + 1..])
    } else {
        None
    }
}

/// Case-insensitive ASCII comparison
fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.len() != b.len() { return false; }
    a.bytes().zip(b.bytes()).all(|(x, y)| {
        let lx = if x >= b'A' && x <= b'Z' { x + 32 } else { x };
        let ly = if y >= b'A' && y <= b'Z' { y + 32 } else { y };
        lx == ly
    })
}

// ============================================================================
// Virtual file registry
// ============================================================================

/// A static file entry stored in the in-memory registry
pub struct StaticFile {
    /// Virtual path (e.g., "/index.html")
    pub path: String,
    /// File content bytes
    pub content: Vec<u8>,
    /// MIME content type
    pub content_type: String,
    /// Size in bytes
    pub size: u32,
    /// Last-modified timestamp (kernel ticks)
    pub last_modified: u64,
    /// ETag (simple hash string for caching)
    pub etag: String,
    /// Whether the content is pre-compressed (gzip)
    pub is_compressed: bool,
    /// Cache-Control max-age in seconds (Q16 fixed-point for precision)
    pub cache_max_age_q16: i32,
}

/// The virtual file system for static content
static FILE_REGISTRY: Mutex<Vec<StaticFile>> = Mutex::new(Vec::new());

/// Directory entry for listing
pub struct DirEntry {
    pub name: String,
    pub is_directory: bool,
    pub size: u32,
    pub content_type: String,
}

/// Register a static file
pub fn register_file(path: &str, content: Vec<u8>, content_type: &str) {
    let size = content.len() as u32;
    let etag = compute_etag(&content);
    let cache_max_age_q16 = 3600 << 16;  // 1 hour default in Q16

    let file = StaticFile {
        path: String::from(path),
        content,
        content_type: String::from(content_type),
        size,
        last_modified: 0,
        etag,
        is_compressed: false,
        cache_max_age_q16,
    };

    FILE_REGISTRY.lock().push(file);
    serial_println!("  [static] registered: {} ({} bytes, {})", path, size, content_type);
}

/// Register a file with automatic MIME detection from extension
pub fn register_auto(path: &str, content: Vec<u8>) {
    let ct = match get_extension(path) {
        Some(ext) => mime_for_extension(ext),
        None => "application/octet-stream",
    };
    register_file(path, content, ct);
}

/// Unregister a file by path
pub fn unregister_file(path: &str) -> bool {
    let mut registry = FILE_REGISTRY.lock();
    let before = registry.len();
    registry.retain(|f| f.path.as_str() != path);
    registry.len() < before
}

/// Compute a simple ETag from content bytes using FNV-1a hash
fn compute_etag(data: &[u8]) -> String {
    // FNV-1a 64-bit hash
    let mut hash: u64 = 0xCBF29CE484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001B3);
    }
    format!("\"{}\"", format_hex_u64(hash))
}

/// Format a u64 as hex string
fn format_hex_u64(val: u64) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut buf = [0u8; 16];
    for i in 0..16 {
        let nibble = ((val >> (60 - i * 4)) & 0x0F) as usize;
        buf[i] = HEX[nibble];
    }
    String::from_utf8(buf.to_vec()).unwrap_or_else(|_| String::from("0000000000000000"))
}

// ============================================================================
// File serving handler
// ============================================================================

/// Serve a static file request. Called by the router for paths under the
/// static root.
pub fn serve_file(req: &HttpRequest) -> HttpResponse {
    // Only GET and HEAD allowed for static files
    if req.method != Method::Get && req.method != Method::Head {
        return HttpResponse::text(StatusCode::MethodNotAllowed, "Method Not Allowed");
    }

    let path = normalize_path(&req.path);

    // Look up file in registry
    let registry = FILE_REGISTRY.lock();

    // Try exact path
    if let Some(file) = registry.iter().find(|f| f.path == path) {
        return build_file_response(req, file);
    }

    // Try path with /index.html appended
    let index_path = if path.ends_with('/') {
        format!("{}index.html", path)
    } else {
        format!("{}/index.html", path)
    };

    if let Some(file) = registry.iter().find(|f| f.path == index_path) {
        return build_file_response(req, file);
    }

    // Check if this is a directory (any files start with this prefix)
    let dir_prefix = if path.ends_with('/') {
        path.clone()
    } else {
        format!("{}/", path)
    };

    let has_children = registry.iter().any(|f| f.path.starts_with(dir_prefix.as_str()));
    drop(registry);

    if has_children {
        return serve_directory_listing(&path);
    }

    HttpResponse::text(StatusCode::NotFound, "File not found")
}

/// Build the response for a matched static file, handling caching and ranges
fn build_file_response(req: &HttpRequest, file: &StaticFile) -> HttpResponse {
    // Check If-None-Match (ETag-based caching)
    if let Some(inm) = req.headers.get("if-none-match") {
        if inm == file.etag.as_str() {
            return HttpResponse::new(StatusCode::NotModified);
        }
    }

    // Check for Range request
    if let Some(range_header) = req.headers.get("range") {
        if let Some((start, end)) = parse_range(range_header, file.size as u64) {
            return build_range_response(file, start, end);
        }
    }

    // Build normal response
    let cache_secs = file.cache_max_age_q16 >> 16;
    let cache_control = format!("public, max-age={}", cache_secs);

    let mut resp = if req.method == Method::Head {
        HttpResponse::new(StatusCode::Ok)
    } else {
        HttpResponse::binary(StatusCode::Ok, &file.content_type, file.content.clone())
    };

    resp.headers.set("etag", &file.etag);
    resp.headers.set("cache-control", &cache_control);
    resp.headers.set("accept-ranges", "bytes");
    resp.headers.set("content-type", &file.content_type);

    if file.is_compressed {
        resp.headers.set("content-encoding", "gzip");
    }

    // X-Content-Type-Options to prevent MIME sniffing
    resp.headers.set("x-content-type-options", "nosniff");

    resp
}

/// Build a partial content (206) response for a Range request
fn build_range_response(file: &StaticFile, start: u64, end: u64) -> HttpResponse {
    let start_usize = start as usize;
    let end_usize = (end as usize).min(file.content.len());

    if start_usize >= file.content.len() {
        let mut resp = HttpResponse::text(StatusCode::RangeNotSatisfiable, "Range Not Satisfiable");
        resp.headers.set("content-range", &format!("bytes */{}", file.size));
        return resp;
    }

    let slice = file.content[start_usize..end_usize].to_vec();
    let content_range = format!("bytes {}-{}/{}", start, end_usize - 1, file.size);

    let mut resp = HttpResponse::binary(StatusCode::PartialContent, &file.content_type, slice);
    resp.headers.set("content-range", &content_range);
    resp.headers.set("accept-ranges", "bytes");

    resp
}

/// Parse a Range header: "bytes=START-END" or "bytes=START-"
fn parse_range(header: &str, total_size: u64) -> Option<(u64, u64)> {
    let trimmed = header.trim();
    if !trimmed.starts_with("bytes=") { return None; }

    let range_spec = &trimmed[6..];
    let dash = range_spec.find('-')?;
    let start_str = range_spec[..dash].trim();
    let end_str = range_spec[dash + 1..].trim();

    let start: u64 = if start_str.is_empty() {
        0
    } else {
        parse_u64(start_str)?
    };

    let end: u64 = if end_str.is_empty() {
        total_size
    } else {
        parse_u64(end_str)?.min(total_size) + 1  // Range is inclusive, our slice is exclusive
    };

    if start >= end || start >= total_size {
        return None;
    }

    Some((start, end))
}

/// Parse u64 from decimal string
fn parse_u64(s: &str) -> Option<u64> {
    let mut result: u64 = 0;
    for c in s.chars() {
        if c < '0' || c > '9' { return None; }
        result = result.checked_mul(10)?;
        result = result.checked_add((c as u8 - b'0') as u64)?;
    }
    Some(result)
}

// ============================================================================
// Directory listing
// ============================================================================

/// Generate an HTML directory listing for a path
fn serve_directory_listing(dir_path: &str) -> HttpResponse {
    let registry = FILE_REGISTRY.lock();

    let dir_prefix = if dir_path.ends_with('/') {
        String::from(dir_path)
    } else {
        format!("{}/", dir_path)
    };

    // Collect entries at this directory level
    let mut entries: Vec<DirEntry> = Vec::new();
    let mut seen_dirs: Vec<String> = Vec::new();

    for file in registry.iter() {
        if !file.path.starts_with(dir_prefix.as_str()) { continue; }

        let relative = &file.path[dir_prefix.len()..];
        if relative.is_empty() { continue; }

        // Check if this is a direct child or a subdirectory
        if let Some(slash) = relative.find('/') {
            let subdir = String::from(&relative[..slash]);
            if !seen_dirs.contains(&subdir) {
                seen_dirs.push(subdir.clone());
                entries.push(DirEntry {
                    name: format!("{}/", subdir),
                    is_directory: true,
                    size: 0,
                    content_type: String::from("directory"),
                });
            }
        } else {
            entries.push(DirEntry {
                name: String::from(relative),
                is_directory: false,
                size: file.size,
                content_type: file.content_type.clone(),
            });
        }
    }

    drop(registry);

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        if a.is_directory != b.is_directory {
            if a.is_directory { core::cmp::Ordering::Less } else { core::cmp::Ordering::Greater }
        } else {
            a.name.as_str().cmp(b.name.as_str())
        }
    });

    // Build HTML
    let mut html = String::from("<!DOCTYPE html><html><head>");
    html.push_str("<title>Index of ");
    html.push_str(dir_path);
    html.push_str("</title>");
    html.push_str("<style>");
    html.push_str("body{font-family:monospace;margin:20px;}");
    html.push_str("table{border-collapse:collapse;width:100%;}");
    html.push_str("th,td{text-align:left;padding:4px 12px;border-bottom:1px solid #ddd;}");
    html.push_str("a{color:#0066cc;text-decoration:none;}");
    html.push_str("a:hover{text-decoration:underline;}");
    html.push_str(".dir{font-weight:bold;}");
    html.push_str("</style></head><body>");
    html.push_str("<h1>Index of ");
    html.push_str(dir_path);
    html.push_str("</h1>");

    // Parent directory link
    if dir_path != "/" {
        html.push_str("<p><a href=\"..\">.. (parent directory)</a></p>");
    }

    html.push_str("<table><tr><th>Name</th><th>Size</th><th>Type</th></tr>");

    for entry in &entries {
        html.push_str("<tr><td>");
        if entry.is_directory {
            html.push_str("<a class=\"dir\" href=\"");
        } else {
            html.push_str("<a href=\"");
        }
        html.push_str(&entry.name);
        html.push_str("\">");
        html.push_str(&entry.name);
        html.push_str("</a></td><td>");
        if entry.is_directory {
            html.push_str("-");
        } else {
            html.push_str(&format_file_size(entry.size));
        }
        html.push_str("</td><td>");
        html.push_str(&entry.content_type);
        html.push_str("</td></tr>");
    }

    html.push_str("</table>");
    html.push_str("<hr><p>Genesis/1.0 - Static File Server</p>");
    html.push_str("</body></html>");

    HttpResponse::html(StatusCode::Ok, &html)
}

/// Format a file size in human-readable form using Q16 fixed-point division
fn format_file_size(bytes: u32) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }

    // Use Q16 for fractional KB/MB/GB display
    let bytes_q16 = (bytes as i64) << 16;

    if bytes < 1_048_576 {
        // KB: bytes / 1024, in Q16
        let kb_q16 = (((bytes_q16) << 16) / (1024_i64 << 16)) as i32;
        let whole = kb_q16 >> 16;
        let frac = ((((kb_q16 & 0xFFFF) as i64) * 10) >> 16) as i32;
        format!("{}.{} KB", whole, frac)
    } else if bytes < 1_073_741_824 {
        // MB: bytes / (1024*1024), in Q16
        let mb_q16 = (((bytes_q16) << 16) / (1_048_576_i64 << 16)) as i32;
        let whole = mb_q16 >> 16;
        let frac = ((((mb_q16 & 0xFFFF) as i64) * 10) >> 16) as i32;
        format!("{}.{} MB", whole, frac)
    } else {
        // GB
        let gb_q16 = (((bytes_q16) << 16) / (1_073_741_824_i64 << 16)) as i32;
        let whole = gb_q16 >> 16;
        let frac = ((((gb_q16 & 0xFFFF) as i64) * 10) >> 16) as i32;
        format!("{}.{} GB", whole, frac)
    }
}

/// Normalize a file path (remove double slashes, resolve . and ..)
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => { segments.pop(); }
            _ => segments.push(part),
        }
    }

    let mut result = String::from("/");
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 { result.push('/'); }
        result.push_str(seg);
    }
    result
}

/// Get the number of registered files
pub fn file_count() -> usize {
    FILE_REGISTRY.lock().len()
}

/// Get total size of all registered files in bytes
pub fn total_size() -> u64 {
    FILE_REGISTRY.lock().iter().map(|f| f.size as u64).sum()
}

/// Initialize the static file server with default content
pub fn init() {
    // Register a default favicon (1x1 pixel PNG)
    let favicon_data: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,  // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,  // IHDR chunk
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,  // 1x1 pixels
        0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
        0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41,  // IDAT chunk
        0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC,
        0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,  // IEND chunk
        0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    register_file("/favicon.ico", favicon_data, "image/x-icon");

    // Register default index page
    let index_html = "<html><head><title>Genesis Webserver</title></head>\
        <body><h1>Genesis OS Webserver</h1>\
        <p>Serving static files from kernel memory.</p>\
        <p><a href=\"/_routes\">View routes</a> | <a href=\"/_stats\">Server stats</a></p>\
        </body></html>";
    register_file("/index.html", index_html.as_bytes().to_vec(), "text/html; charset=utf-8");

    serial_println!("  [static] Static file server initialized");
    serial_println!("  [static] {} MIME types, {} files registered", MIME_TYPES.len(), file_count());
}
