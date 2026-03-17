/// AI-powered filesystem for Genesis
///
/// Smart search, auto-categorization, content indexing,
/// duplicate detection, predictive prefetch, usage analytics.
///
/// All scoring uses integer math (no floats). Scores are in units
/// of 1/100 (centiscore) for fine-grained ranking.
///
/// Inspired by: macOS Spotlight, Everything Search, Google Drive AI. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// File category from AI classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    Document,
    Spreadsheet,
    Presentation,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Config,
    Database,
    Font,
    Executable,
    Log,
    Backup,
    Unknown,
}

/// Indexed file metadata for AI search
pub struct FileIndex {
    pub path: String,
    pub name: String,
    pub category: FileCategory,
    pub size: u64,
    pub modified: u64,
    pub accessed: u64,
    pub access_count: u32,
    pub keywords: Vec<String>,
    pub content_hash: u64,
    /// Integer embedding (0-255 per component, 8 components)
    pub embedding: [u8; 8],
}

/// Search result with AI relevance scoring (integer, centiscore units)
pub struct SearchResult {
    pub path: String,
    pub name: String,
    /// Relevance score in centiscore units (100 = 1.00 relevance)
    pub relevance: u32,
    pub category: FileCategory,
    pub snippet: String,
}

/// Duplicate group
pub struct DuplicateGroup {
    pub files: Vec<String>,
    pub total_size: u64,
    pub wasted_size: u64,
}

/// AI filesystem engine
pub struct AiFsEngine {
    pub enabled: bool,
    pub index: Vec<FileIndex>,
    pub category_rules: BTreeMap<String, FileCategory>,
    pub access_patterns: Vec<AccessPattern>,
    pub duplicates: Vec<DuplicateGroup>,
    pub total_indexed: u64,
    pub total_searches: u64,
    pub prefetch_enabled: bool,
    pub auto_categorize: bool,
    pub dedup_enabled: bool,
}

pub struct AccessPattern {
    pub path: String,
    pub hour: u8,
    pub frequency: u32,
    pub last_access: u64,
}

impl AiFsEngine {
    const fn new() -> Self {
        AiFsEngine {
            enabled: true,
            index: Vec::new(),
            category_rules: BTreeMap::new(),
            access_patterns: Vec::new(),
            duplicates: Vec::new(),
            total_indexed: 0,
            total_searches: 0,
            prefetch_enabled: true,
            auto_categorize: true,
            dedup_enabled: true,
        }
    }

    /// Classify a file by extension and name
    pub fn categorize_file(&self, filename: &str) -> FileCategory {
        let lower = filename.to_lowercase();
        // Check extension
        if let Some(ext_pos) = lower.rfind('.') {
            let ext = &lower[ext_pos + 1..];
            match ext {
                "txt" | "md" | "doc" | "docx" | "pdf" | "rtf" | "odt" => FileCategory::Document,
                "xls" | "xlsx" | "csv" | "ods" | "tsv" => FileCategory::Spreadsheet,
                "ppt" | "pptx" | "odp" | "key" => FileCategory::Presentation,
                "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tiff" => {
                    FileCategory::Image
                }
                "mp4" | "avi" | "mkv" | "mov" | "wmv" | "flv" | "webm" => FileCategory::Video,
                "mp3" | "wav" | "flac" | "ogg" | "aac" | "m4a" | "wma" => FileCategory::Audio,
                "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => FileCategory::Archive,
                "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "java" | "go" | "rb" | "swift"
                | "kt" => FileCategory::Code,
                "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf" => {
                    FileCategory::Config
                }
                "db" | "sqlite" | "sql" | "mdb" => FileCategory::Database,
                "ttf" | "otf" | "woff" | "woff2" => FileCategory::Font,
                "exe" | "elf" | "bin" | "app" | "msi" | "deb" | "rpm" => FileCategory::Executable,
                "log" => FileCategory::Log,
                "bak" | "backup" | "old" => FileCategory::Backup,
                _ => FileCategory::Unknown,
            }
        } else {
            // No extension -- check name patterns
            if lower == "makefile" || lower == "dockerfile" || lower == "rakefile" {
                FileCategory::Code
            } else if lower == "license" || lower == "readme" || lower == "changelog" {
                FileCategory::Document
            } else {
                FileCategory::Unknown
            }
        }
    }

    /// Index a file for AI search
    pub fn index_file(&mut self, path: &str, name: &str, size: u64, modified: u64) {
        let category = self.categorize_file(name);
        let keywords = self.extract_keywords(name);

        // Simple hash-based embedding (integer, 0-255 per component)
        let mut hash = 0u64;
        for byte in name.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
        }
        let mut embedding = [0u8; 8];
        for i in 0..8 {
            embedding[i] = ((hash >> (i * 8)) & 0xFF) as u8;
        }

        self.index.push(FileIndex {
            path: String::from(path),
            name: String::from(name),
            category,
            size,
            modified,
            accessed: modified,
            access_count: 0,
            keywords,
            content_hash: hash,
            embedding,
        });
        self.total_indexed = self.total_indexed.saturating_add(1);
    }

    fn extract_keywords(&self, name: &str) -> Vec<String> {
        let mut keywords = Vec::new();
        // Split on separators
        for part in name.split(|c: char| c == '_' || c == '-' || c == '.' || c == ' ') {
            if part.len() >= 2 {
                keywords.push(String::from(part).to_lowercase());
            }
        }
        keywords
    }

    /// AI-powered search using integer scoring (centiscores: 100 = 1.00)
    pub fn search(&mut self, query: &str, max_results: usize) -> Vec<SearchResult> {
        self.total_searches = self.total_searches.saturating_add(1);
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();

        // (index, centiscore)
        let mut results: Vec<(usize, u32)> = Vec::new();

        for (idx, file) in self.index.iter().enumerate() {
            let mut score: u32 = 0;
            let name_lower = file.name.to_lowercase();
            let path_lower = file.path.to_lowercase();

            // Exact name match: +1000 centiscores (10.00)
            if name_lower == query_lower {
                score = score.saturating_add(1000);
            }
            // Name contains query: +500 (5.00)
            if name_lower.contains(&query_lower) {
                score = score.saturating_add(500);
            }
            // Path contains query: +200 (2.00)
            if path_lower.contains(&query_lower) {
                score = score.saturating_add(200);
            }
            // Keyword matches: +100 each (1.00)
            for word in &query_words {
                for keyword in &file.keywords {
                    if keyword.contains(word) {
                        score = score.saturating_add(100);
                    }
                }
            }
            // Recency boost
            let age_days = (crate::time::clock::unix_time().saturating_sub(file.accessed)) / 86400;
            if age_days < 1 {
                score = score.saturating_add(200);
            }
            // +2.00
            else if age_days < 7 {
                score = score.saturating_add(100);
            }
            // +1.00
            else if age_days < 30 {
                score = score.saturating_add(50);
            } // +0.50

            // Frequency boost: min(access_count, 5) * 20 centiscores
            let freq_bonus = core::cmp::min(file.access_count, 5) * 20;
            score = score.saturating_add(freq_bonus);

            if score > 0 {
                results.push((idx, score));
            }
        }

        // Sort descending by score
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.truncate(max_results);

        results
            .iter()
            .map(|(idx, score)| {
                let file = &self.index[*idx];
                SearchResult {
                    path: file.path.clone(),
                    name: file.name.clone(),
                    relevance: *score,
                    category: file.category,
                    snippet: alloc::format!("{:?} - {} bytes", file.category, file.size),
                }
            })
            .collect()
    }

    /// Find duplicate files
    pub fn find_duplicates(&mut self) -> usize {
        let mut hash_groups: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (idx, file) in self.index.iter().enumerate() {
            hash_groups
                .entry(file.content_hash)
                .or_insert_with(Vec::new)
                .push(idx);
        }

        self.duplicates.clear();
        for (_, indices) in &hash_groups {
            if indices.len() > 1 {
                let files: Vec<String> = indices
                    .iter()
                    .map(|&i| self.index[i].path.clone())
                    .collect();
                let sizes: Vec<u64> = indices.iter().map(|&i| self.index[i].size).collect();
                let total: u64 = sizes.iter().sum();
                let wasted = total - sizes.iter().copied().min().unwrap_or(0);
                self.duplicates.push(DuplicateGroup {
                    files,
                    total_size: total,
                    wasted_size: wasted,
                });
            }
        }
        self.duplicates.len()
    }

    /// Predict which files user will access next
    pub fn predict_access(&self) -> Vec<String> {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;

        let mut predictions: Vec<(&AccessPattern, u32)> = self
            .access_patterns
            .iter()
            .filter(|p| p.hour == hour)
            .map(|p| (p, p.frequency))
            .collect();

        // Sort descending by frequency
        predictions.sort_by(|a, b| b.1.cmp(&a.1));
        predictions
            .iter()
            .take(5)
            .map(|(p, _)| p.path.clone())
            .collect()
    }

    /// Record file access for learning
    pub fn record_access(&mut self, path: &str) {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;

        if let Some(file) = self.index.iter_mut().find(|f| f.path == path) {
            file.accessed = now;
            file.access_count = file.access_count.saturating_add(1);
        }

        if let Some(pattern) = self
            .access_patterns
            .iter_mut()
            .find(|p| p.path == path && p.hour == hour)
        {
            pattern.frequency = pattern.frequency.saturating_add(1);
            pattern.last_access = now;
        } else {
            self.access_patterns.push(AccessPattern {
                path: String::from(path),
                hour,
                frequency: 1,
                last_access: now,
            });
        }
    }

    pub fn index_count(&self) -> usize {
        self.index.len()
    }
}

static AI_FS: Mutex<AiFsEngine> = Mutex::new(AiFsEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-fs] AI filesystem intelligence initialized (search, categorize, dedup)"
    );
}

pub fn index_file(path: &str, name: &str, size: u64, modified: u64) {
    AI_FS.lock().index_file(path, name, size, modified);
}

pub fn search(query: &str, max: usize) -> Vec<SearchResult> {
    AI_FS.lock().search(query, max)
}

pub fn categorize(filename: &str) -> FileCategory {
    AI_FS.lock().categorize_file(filename)
}

pub fn record_access(path: &str) {
    AI_FS.lock().record_access(path);
}
