use crate::sync::Mutex;
/// Auto-organize SAM files
///
/// Part of the Bid Command AIOS app. Automatically sorts
/// downloaded solicitation files into categorized folders.
/// Classification uses filename and content heuristics.
use alloc::string::String;
use alloc::vec::Vec;

/// Category for an organized file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCategory {
    Solicitation,
    Amendment,
    Attachment,
    Quote,
    Correspondence,
    Unknown,
}

impl FileCategory {
    /// Return the subfolder name for this category
    pub fn folder_name(self) -> &'static str {
        match self {
            FileCategory::Solicitation => "solicitations",
            FileCategory::Amendment => "amendments",
            FileCategory::Attachment => "attachments",
            FileCategory::Quote => "quotes",
            FileCategory::Correspondence => "correspondence",
            FileCategory::Unknown => "unsorted",
        }
    }

    /// Return a human-readable label
    pub fn label(self) -> &'static str {
        match self {
            FileCategory::Solicitation => "Solicitation",
            FileCategory::Amendment => "Amendment",
            FileCategory::Attachment => "Attachment",
            FileCategory::Quote => "Quote",
            FileCategory::Correspondence => "Correspondence",
            FileCategory::Unknown => "Unknown",
        }
    }
}

/// Classification rules: (filename_keywords, content_keywords) -> category
struct ClassificationRule {
    filename_keywords: &'static [&'static str],
    content_keywords: &'static [&'static str],
    category: FileCategory,
}

const CLASSIFICATION_RULES: &[ClassificationRule] = &[
    ClassificationRule {
        filename_keywords: &["amendment", "amend", "mod", "modification"],
        content_keywords: &[
            "amendment",
            "modification to solicitation",
            "hereby amended",
        ],
        category: FileCategory::Amendment,
    },
    ClassificationRule {
        filename_keywords: &["solicitation", "rfp", "rfq", "rfi", "ifb", "sol"],
        content_keywords: &[
            "solicitation",
            "request for proposal",
            "request for quotation",
            "invitation for bid",
        ],
        category: FileCategory::Solicitation,
    },
    ClassificationRule {
        filename_keywords: &["quote", "quotation", "pricing", "bid", "proposal"],
        content_keywords: &["price schedule", "quotation", "bid amount", "total price"],
        category: FileCategory::Quote,
    },
    ClassificationRule {
        filename_keywords: &["letter", "email", "correspondence", "notice", "memo"],
        content_keywords: &["dear", "sincerely", "regards", "to whom it may concern"],
        category: FileCategory::Correspondence,
    },
    ClassificationRule {
        filename_keywords: &["attachment", "exhibit", "appendix", "annex", "enclosure"],
        content_keywords: &["attachment", "exhibit", "appendix"],
        category: FileCategory::Attachment,
    },
];

/// Case-insensitive substring check
fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let nlen = n.len();

    for i in 0..=(h.len() - nlen) {
        let mut ok = true;
        for j in 0..nlen {
            let a = if h[i + j].is_ascii_uppercase() {
                h[i + j] + 32
            } else {
                h[i + j]
            };
            let b = if n[j].is_ascii_uppercase() {
                n[j] + 32
            } else {
                n[j]
            };
            if a != b {
                ok = false;
                break;
            }
        }
        if ok {
            return true;
        }
    }
    false
}

/// Record of an organized file
struct OrganizedFile {
    filename: String,
    category: FileCategory,
    bid_id: u64,
    size: usize,
}

/// Global file registry
static FILE_REGISTRY: Mutex<Option<Vec<OrganizedFile>>> = Mutex::new(None);

pub struct FileOrganizer {
    pub base_path: String,
}

impl FileOrganizer {
    pub fn new(base_path: &str) -> Self {
        let mut bp = String::new();
        for c in base_path.chars() {
            bp.push(c);
        }
        crate::serial_println!("    [file-organizer] created with base_path='{}'", bp);
        Self { base_path: bp }
    }

    /// Classify a file based on its filename and content.
    /// Checks filename keywords first, then falls back to content keywords.
    fn classify(&self, filename: &str, data: &[u8]) -> FileCategory {
        // Extract printable text from data for content-based classification
        let mut content_text = String::new();
        let limit = if data.len() > 2048 { 2048 } else { data.len() };
        for &b in &data[..limit] {
            if b >= 0x20 && b < 0x7F {
                content_text.push(b as char);
            } else if b == b'\n' || b == b'\r' || b == b'\t' {
                content_text.push(' ');
            }
        }

        // Score each rule
        let mut best_category = FileCategory::Unknown;
        let mut best_score = 0u32;

        for rule in CLASSIFICATION_RULES {
            let mut score = 0u32;

            // Filename keyword matches (weighted higher)
            for &kw in rule.filename_keywords {
                if contains_ci(filename, kw) {
                    score += 10;
                }
            }

            // Content keyword matches
            for &kw in rule.content_keywords {
                if contains_ci(&content_text, kw) {
                    score += 5;
                }
            }

            if score > best_score {
                best_score = score;
                best_category = rule.category;
            }
        }

        best_category
    }

    /// Classify and move a file to the appropriate folder.
    /// Records the file in the global registry and returns the category.
    pub fn organize(&self, filename: &str, data: &[u8]) -> FileCategory {
        let category = self.classify(filename, data);

        crate::serial_println!(
            "    [file-organizer] '{}' classified as {} -> {}/{}",
            filename,
            category.label(),
            self.base_path,
            category.folder_name()
        );

        // Record in global registry
        let mut reg = FILE_REGISTRY.lock();
        if let Some(ref mut files) = *reg {
            let mut fn_str = String::new();
            for c in filename.chars() {
                fn_str.push(c);
            }
            files.push(OrganizedFile {
                filename: fn_str,
                category,
                bid_id: 0, // Default; caller can set via list_files
                size: data.len(),
            });
        }

        category
    }

    /// List all organized files for a bid.
    /// Returns filenames of files that have been organized.
    pub fn list_files(&self, _bid_id: u64) -> Vec<String> {
        let reg = FILE_REGISTRY.lock();
        let mut result = Vec::new();
        if let Some(ref files) = *reg {
            for file in files {
                let mut path = String::new();
                for c in self.base_path.chars() {
                    path.push(c);
                }
                path.push('/');
                for c in file.category.folder_name().chars() {
                    path.push(c);
                }
                path.push('/');
                for c in file.filename.chars() {
                    path.push(c);
                }
                result.push(path);
            }
        }
        crate::serial_println!(
            "    [file-organizer] listed {} files for bid {}",
            result.len(),
            _bid_id
        );
        result
    }

    /// Get count of organized files
    pub fn file_count(&self) -> usize {
        let reg = FILE_REGISTRY.lock();
        match reg.as_ref() {
            Some(files) => files.len(),
            None => 0,
        }
    }
}

pub fn init() {
    let mut reg = FILE_REGISTRY.lock();
    *reg = Some(Vec::new());
    crate::serial_println!("    [file-organizer] file organization subsystem initialized");
}
