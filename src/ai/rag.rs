/// Retrieval-Augmented Generation for Genesis
///
/// Index documents, semantic search, context injection,
/// and grounded responses — all on-device.
///
/// Inspired by: LangChain, LlamaIndex. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// A document chunk for RAG indexing
pub struct DocumentChunk {
    pub id: u32,
    pub source: String, // file path or URI
    pub content: String,
    pub chunk_index: u32,
    pub embedding: Vec<f32>,
    pub metadata: Vec<(String, String)>,
}

/// RAG search result
pub struct RagResult {
    pub chunk_id: u32,
    pub source: String,
    pub content: String,
    pub relevance: f32,
    pub chunk_index: u32,
}

/// Chunking strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStrategy {
    FixedSize { tokens: usize },
    Paragraph,
    Sentence,
    Sliding { window: usize, overlap: usize },
}

/// RAG index
pub struct RagIndex {
    pub chunks: Vec<DocumentChunk>,
    pub next_id: u32,
    pub chunk_strategy: ChunkStrategy,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub max_chunks: usize,
    pub total_documents: u32,
}

impl RagIndex {
    const fn new() -> Self {
        RagIndex {
            chunks: Vec::new(),
            next_id: 1,
            chunk_strategy: ChunkStrategy::FixedSize { tokens: 256 },
            chunk_size: 512,   // chars per chunk
            chunk_overlap: 50, // overlap chars
            max_chunks: 50000,
            total_documents: 0,
        }
    }

    /// Index a document by splitting into chunks and embedding each
    pub fn index_document(&mut self, source: &str, content: &str) {
        let chunks = self.split_into_chunks(content);
        for (i, chunk_text) in chunks.iter().enumerate() {
            if self.chunks.len() >= self.max_chunks {
                break;
            }

            // Generate embedding for this chunk
            let embedding = super::embeddings::embed(chunk_text);

            let id = self.next_id;
            self.next_id = self.next_id.saturating_add(1);
            self.chunks.push(DocumentChunk {
                id,
                source: String::from(source),
                content: String::from(*chunk_text),
                chunk_index: i as u32,
                embedding: embedding.vector,
                metadata: Vec::new(),
            });
        }
        self.total_documents = self.total_documents.saturating_add(1);
    }

    fn split_into_chunks<'a>(&self, text: &'a str) -> Vec<&'a str> {
        let mut chunks = Vec::new();
        let len = text.len();
        if len == 0 {
            return chunks;
        }

        let mut start = 0;
        while start < len {
            let end = (start + self.chunk_size).min(len);
            // Try to break at a word boundary
            let actual_end = if end < len {
                text[start..end]
                    .rfind(' ')
                    .map(|p| start + p)
                    .unwrap_or(end)
            } else {
                end
            };
            if actual_end > start {
                chunks.push(&text[start..actual_end]);
            }
            start = if actual_end > self.chunk_overlap {
                actual_end - self.chunk_overlap
            } else {
                actual_end
            };
            if start >= len {
                break;
            }
        }
        chunks
    }

    /// Search the index for the most relevant chunks
    pub fn search(&self, query: &str, top_k: usize) -> Vec<RagResult> {
        let query_emb = super::embeddings::embed(query);
        let mut results: Vec<RagResult> = self
            .chunks
            .iter()
            .map(|chunk| {
                let similarity = cosine_similarity(&query_emb.vector, &chunk.embedding);
                RagResult {
                    chunk_id: chunk.id,
                    source: chunk.source.clone(),
                    content: chunk.content.clone(),
                    relevance: similarity,
                    chunk_index: chunk.chunk_index,
                }
            })
            .collect();

        results.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        results.truncate(top_k);
        results
    }

    /// Build an augmented prompt with retrieved context
    pub fn augmented_prompt(&self, query: &str, top_k: usize) -> String {
        let results = self.search(query, top_k);
        let mut prompt =
            String::from("Use the following context to answer the question.\n\nContext:\n");
        for (i, result) in results.iter().enumerate() {
            prompt.push_str(&format!(
                "[{}] (from {}): {}\n\n",
                i + 1,
                result.source,
                result.content
            ));
        }
        prompt.push_str(&format!("\nQuestion: {}\nAnswer:", query));
        prompt
    }

    /// Remove all chunks from a specific source
    pub fn remove_source(&mut self, source: &str) {
        self.chunks.retain(|c| c.source != source);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    pub fn document_count(&self) -> u32 {
        self.total_documents
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = sqrt_f32(norm_a) * sqrt_f32(norm_b);
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x / 2.0;
    for _ in 0..10 {
        guess = (guess + x / guess) * 0.5;
    }
    guess
}

static RAG: Mutex<RagIndex> = Mutex::new(RagIndex::new());

pub fn init() {
    crate::serial_println!("    [rag] Retrieval-Augmented Generation initialized");
}

pub fn index_document(source: &str, content: &str) {
    RAG.lock().index_document(source, content);
}

pub fn search(query: &str, top_k: usize) -> Vec<RagResult> {
    RAG.lock().search(query, top_k)
}

pub fn augmented_prompt(query: &str) -> String {
    RAG.lock().augmented_prompt(query, 3)
}
