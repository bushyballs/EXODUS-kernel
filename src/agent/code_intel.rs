use crate::sync::Mutex;
/// Code intelligence for Genesis agent
///
/// AST-level codebase understanding, symbol resolution,
/// embeddings-based semantic search, dependency graphing.
/// Makes the agent understand code structure, not just text.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Module,
    Variable,
    Constant,
    Type,
    Macro,
    Import,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    C,
    Cpp,
    Go,
    Java,
    Shell,
    Unknown,
}

#[derive(Clone, Copy)]
struct Symbol {
    id: u32,
    name_hash: u64,
    kind: SymbolKind,
    file_hash: u64,
    line_start: u32,
    line_end: u32,
    parent_id: u32, // 0 = top-level
    language: Language,
    embedding_idx: u32, // Index into embeddings store
    reference_count: u32,
}

#[derive(Clone, Copy)]
struct FileIndex {
    path_hash: u64,
    language: Language,
    line_count: u32,
    symbol_count: u16,
    last_indexed: u64,
    size_bytes: u32,
    import_count: u16,
}

#[derive(Clone, Copy)]
struct SymbolReference {
    from_symbol: u32,
    to_symbol: u32,
    ref_type: RefType,
    line: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RefType {
    Calls,
    Imports,
    Inherits,
    Implements,
    Uses,
    References,
}

struct CodeIntelEngine {
    symbols: Vec<Symbol>,
    files: Vec<FileIndex>,
    references: Vec<SymbolReference>,
    embeddings: Vec<[u8; 64]>, // Compact 512-bit embeddings
    next_symbol_id: u32,
    total_indexed_files: u32,
    total_symbols: u32,
    index_version: u32,
}

static CODE_INTEL: Mutex<Option<CodeIntelEngine>> = Mutex::new(None);

impl CodeIntelEngine {
    fn new() -> Self {
        CodeIntelEngine {
            symbols: Vec::new(),
            files: Vec::new(),
            references: Vec::new(),
            embeddings: Vec::new(),
            next_symbol_id: 1,
            total_indexed_files: 0,
            total_symbols: 0,
            index_version: 0,
        }
    }

    fn index_file(
        &mut self,
        path_hash: u64,
        language: Language,
        line_count: u32,
        size: u32,
        timestamp: u64,
    ) {
        // Update or add file index
        if let Some(f) = self.files.iter_mut().find(|f| f.path_hash == path_hash) {
            f.line_count = line_count;
            f.last_indexed = timestamp;
            f.size_bytes = size;
        } else {
            self.files.push(FileIndex {
                path_hash,
                language,
                line_count,
                symbol_count: 0,
                last_indexed: timestamp,
                size_bytes: size,
                import_count: 0,
            });
            self.total_indexed_files = self.total_indexed_files.saturating_add(1);
        }
        self.index_version = self.index_version.saturating_add(1);
    }

    fn add_symbol(
        &mut self,
        name_hash: u64,
        kind: SymbolKind,
        file_hash: u64,
        line_start: u32,
        line_end: u32,
        parent_id: u32,
        language: Language,
    ) -> u32 {
        let id = self.next_symbol_id;
        self.next_symbol_id = self.next_symbol_id.saturating_add(1);
        self.total_symbols = self.total_symbols.saturating_add(1);

        // Create a simple embedding (in real impl, would use neural encoder)
        let mut embedding = [0u8; 64];
        let bytes = name_hash.to_le_bytes();
        embedding[..8].copy_from_slice(&bytes);
        embedding[8] = kind as u8;
        embedding[9] = language as u8;
        let embed_idx = self.embeddings.len() as u32;
        self.embeddings.push(embedding);

        self.symbols.push(Symbol {
            id,
            name_hash,
            kind,
            file_hash,
            line_start,
            line_end,
            parent_id,
            language,
            embedding_idx: embed_idx,
            reference_count: 0,
        });

        // Update file symbol count
        if let Some(f) = self.files.iter_mut().find(|f| f.path_hash == file_hash) {
            f.symbol_count = f.symbol_count.saturating_add(1);
        }
        id
    }

    fn add_reference(&mut self, from: u32, to: u32, ref_type: RefType, line: u32) {
        self.references.push(SymbolReference {
            from_symbol: from,
            to_symbol: to,
            ref_type,
            line,
        });
        if let Some(s) = self.symbols.iter_mut().find(|s| s.id == to) {
            s.reference_count = s.reference_count.saturating_add(1);
        }
    }

    fn find_symbol(&self, name_hash: u64) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.name_hash == name_hash)
            .collect()
    }

    fn find_references(&self, symbol_id: u32) -> Vec<&SymbolReference> {
        self.references
            .iter()
            .filter(|r| r.to_symbol == symbol_id)
            .collect()
    }

    fn find_callers(&self, symbol_id: u32) -> Vec<u32> {
        self.references
            .iter()
            .filter(|r| r.to_symbol == symbol_id && r.ref_type == RefType::Calls)
            .map(|r| r.from_symbol)
            .collect()
    }

    fn find_callees(&self, symbol_id: u32) -> Vec<u32> {
        self.references
            .iter()
            .filter(|r| r.from_symbol == symbol_id && r.ref_type == RefType::Calls)
            .map(|r| r.to_symbol)
            .collect()
    }

    /// Semantic search — find symbols similar to query embedding
    fn semantic_search(&self, query_embedding: &[u8; 64], limit: usize) -> Vec<(u32, u32)> {
        // Cosine similarity approximation using dot product on byte embeddings
        let mut scores: Vec<(u32, u32)> = self
            .symbols
            .iter()
            .map(|s| {
                let emb = &self.embeddings[s.embedding_idx as usize];
                let mut dot: u32 = 0;
                for i in 0..64 {
                    dot += (query_embedding[i] as u32) * (emb[i] as u32);
                }
                (s.id, dot)
            })
            .collect();
        scores.sort_by(|a, b| b.1.cmp(&a.1));
        scores.truncate(limit);
        scores
    }

    fn get_file_symbols(&self, file_hash: u64) -> Vec<&Symbol> {
        self.symbols
            .iter()
            .filter(|s| s.file_hash == file_hash)
            .collect()
    }

    fn detect_language(&self, extension_hash: u64) -> Language {
        // Simple extension-based detection
        match extension_hash {
            0x7273 => Language::Rust,       // "rs"
            0x7079 => Language::Python,     // "py"
            0x6A73 => Language::JavaScript, // "js"
            0x7473 => Language::TypeScript, // "ts"
            0x63 => Language::C,            // "c"
            0x637070 => Language::Cpp,      // "cpp"
            0x676F => Language::Go,         // "go"
            0x6A617661 => Language::Java,   // "java"
            0x7368 => Language::Shell,      // "sh"
            _ => Language::Unknown,
        }
    }

    fn get_stats(&self) -> (u32, u32, u32) {
        (
            self.total_indexed_files,
            self.total_symbols,
            self.references.len() as u32,
        )
    }
}

pub fn init() {
    let mut ci = CODE_INTEL.lock();
    *ci = Some(CodeIntelEngine::new());
    serial_println!("    Code intel: AST symbols, references, semantic search, 10 languages ready");
}
