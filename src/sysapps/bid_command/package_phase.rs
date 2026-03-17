use crate::sync::Mutex;
/// Document assembly phase
///
/// Part of the Bid Command AIOS app. Assembles the final
/// bid package from analysis, pricing, and vendor data.
/// Validates completeness before finalization.
use alloc::string::String;
use alloc::vec::Vec;

/// Required document types for a complete bid package
const REQUIRED_DOC_TYPES: &[&str] = &["cover_letter", "pricing_schedule", "technical_approach"];

/// A document in the bid package
pub struct PackageDocument {
    pub name: String,
    pub doc_type: String,
    pub data: Vec<u8>,
}

impl PackageDocument {
    /// Get the size of this document in bytes
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

pub struct PackagePhase {
    pub documents: Vec<PackageDocument>,
    pub finalized: bool,
}

impl PackagePhase {
    pub fn new() -> Self {
        crate::serial_println!("    [package-phase] package phase created");
        Self {
            documents: Vec::new(),
            finalized: false,
        }
    }

    /// Add a generated document to the package.
    /// Rejects additions if the package is already finalized.
    pub fn add_document(&mut self, name: &str, doc_type: &str, data: Vec<u8>) {
        if self.finalized {
            crate::serial_println!(
                "    [package-phase] cannot add '{}': package already finalized",
                name
            );
            return;
        }

        let mut n = String::new();
        for c in name.chars() {
            n.push(c);
        }
        let mut dt = String::new();
        for c in doc_type.chars() {
            dt.push(c);
        }

        let size = data.len();

        // Check for duplicate document names, replace if found
        let mut replaced = false;
        for doc in self.documents.iter_mut() {
            if doc.name.as_str() == name {
                doc.doc_type = dt.clone();
                doc.data = data;
                replaced = true;
                crate::serial_println!(
                    "    [package-phase] replaced document '{}' (type: '{}', {} bytes)",
                    name,
                    doc_type,
                    size
                );
                return;
            }
        }

        if !replaced {
            self.documents.push(PackageDocument {
                name: n,
                doc_type: dt,
                data,
            });
            crate::serial_println!(
                "    [package-phase] added document '{}' (type: '{}', {} bytes)",
                name,
                doc_type,
                size
            );
        }
    }

    /// Check whether all required document types are present.
    fn check_completeness(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        for &required in REQUIRED_DOC_TYPES {
            let mut found = false;
            for doc in &self.documents {
                if doc.doc_type.as_str() == required {
                    found = true;
                    break;
                }
            }
            if !found {
                missing.push(required);
            }
        }
        missing
    }

    /// Finalize the package for submission.
    /// Returns Err if required documents are missing or package is empty.
    pub fn finalize(&mut self) -> Result<(), ()> {
        if self.finalized {
            crate::serial_println!("    [package-phase] package already finalized");
            return Ok(());
        }

        if self.documents.is_empty() {
            crate::serial_println!("    [package-phase] cannot finalize: no documents in package");
            return Err(());
        }

        let missing = self.check_completeness();
        if !missing.is_empty() {
            for m in &missing {
                crate::serial_println!(
                    "    [package-phase] missing required document type: '{}'",
                    m
                );
            }
            crate::serial_println!(
                "    [package-phase] cannot finalize: {} required doc types missing",
                missing.len()
            );
            return Err(());
        }

        self.finalized = true;
        let total_size: usize = self.documents.iter().map(|d| d.size()).sum();
        crate::serial_println!(
            "    [package-phase] package finalized: {} documents, {} bytes total",
            self.documents.len(),
            total_size
        );
        Ok(())
    }

    /// Get the total number of documents
    pub fn document_count(&self) -> usize {
        self.documents.len()
    }

    /// Get the total size of all documents in bytes
    pub fn total_size(&self) -> usize {
        self.documents.iter().map(|d| d.size()).sum()
    }

    /// Remove a document by name
    pub fn remove_document(&mut self, name: &str) -> bool {
        if self.finalized {
            crate::serial_println!(
                "    [package-phase] cannot remove '{}': package is finalized",
                name
            );
            return false;
        }
        let initial_len = self.documents.len();
        self.documents.retain(|d| d.name.as_str() != name);
        let removed = self.documents.len() < initial_len;
        if removed {
            crate::serial_println!("    [package-phase] removed document '{}'", name);
        }
        removed
    }
}

/// Global package phase singleton
static PACKAGE_PHASE: Mutex<Option<PackagePhase>> = Mutex::new(None);

pub fn init() {
    let mut pp = PACKAGE_PHASE.lock();
    *pp = Some(PackagePhase::new());
    crate::serial_println!("    [package-phase] document assembly subsystem initialized");
}
