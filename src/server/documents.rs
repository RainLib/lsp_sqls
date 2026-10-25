//! Atomic document text/version snapshots and syntax caches.
use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

#[derive(Clone)]
pub(super) struct DocumentSnapshot {
    pub(super) text: String,
    pub(super) version: i32,
    pub(super) revision: u64,
}

#[derive(Clone)]
pub(super) struct DocumentManager {
    documents: Arc<DashMap<String, DocumentSnapshot>>,
    next_revision: Arc<AtomicU64>,
}

#[derive(Clone)]
pub(super) struct CachedSqlAnalysis {
    pub(super) source: String,
    pub(super) tree: tree_sitter::Tree,
}

impl DocumentManager {
    pub(super) fn new() -> Self {
        Self {
            documents: Arc::new(DashMap::new()),
            next_revision: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(super) fn update(&self, uri: String, text: String, version: i32) {
        // Generate the revision while holding this document's entry lock.
        // Reopening version 1 must not reuse a closed document's revision.
        let mut entry = self
            .documents
            .entry(uri)
            .or_insert_with(|| DocumentSnapshot {
                text: String::new(),
                version,
                revision: 0,
            });
        *entry = DocumentSnapshot {
            text,
            version,
            revision: self.next_revision.fetch_add(1, Ordering::Relaxed),
        };
    }

    pub(super) fn snapshot(&self, uri: &str) -> Option<DocumentSnapshot> {
        self.documents.get(uri).map(|entry| entry.value().clone())
    }

    pub(super) fn get(&self, uri: &str) -> Option<String> {
        self.documents.get(uri).map(|entry| entry.text.clone())
    }

    pub(super) fn entries(&self) -> Vec<(String, String)> {
        self.documents
            .iter()
            .map(|entry| (entry.key().clone(), entry.text.clone()))
            .collect()
    }

    pub(super) fn uris(&self) -> Vec<String> {
        self.documents
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    pub(super) fn revision(&self, uri: &str) -> Option<u64> {
        self.documents.get(uri).map(|entry| entry.revision)
    }

    pub(super) fn remove(&self, uri: &str) {
        self.documents.remove(uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_document_snapshots_keep_text_and_version_together() {
        let manager = DocumentManager::new();
        manager.update("doc".into(), "0".into(), 0);
        let other = manager.clone();
        let writer = std::thread::spawn(move || {
            for version in 1..5000 {
                other.update("doc".into(), version.to_string(), version);
            }
        });
        for _ in 0..5000 {
            let snapshot = manager.snapshot("doc").unwrap();
            assert_eq!(snapshot.text, snapshot.version.to_string());
            assert!(snapshot.revision > 0);
        }
        writer.join().unwrap();
    }

    #[test]
    fn reopening_same_document_version_has_a_distinct_revision() {
        let manager = DocumentManager::new();
        manager.update("doc".into(), "same".into(), 1);
        let before = manager.snapshot("doc").unwrap();
        manager.remove("doc");
        manager.update("other".into(), "other".into(), 1);
        manager.update("doc".into(), "same".into(), 1);
        let after = manager.snapshot("doc").unwrap();
        assert_eq!(before.version, after.version);
        assert_eq!(before.text, after.text);
        assert_ne!(before.revision, after.revision);
    }
}
