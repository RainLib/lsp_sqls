//! Documents for the SQL language server.
use dashmap::DashMap;
use std::sync::Arc;
/// 文档管理器，用于存储和管理打开的文档内容
#[derive(Clone)]
pub(super) struct DocumentManager {
    pub(super) documents: Arc<DashMap<String, String>>,
    pub(super) revisions: Arc<DashMap<String, u64>>,
    pub(super) versions: Arc<DashMap<String, i32>>,
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
            revisions: Arc::new(DashMap::new()),
            versions: Arc::new(DashMap::new()),
        }
    }

    pub(super) fn update(&self, uri: String, text: String, version: i32) {
        self.documents.insert(uri.clone(), text);
        self.versions.insert(uri.clone(), version);
        self.revisions
            .entry(uri)
            .and_modify(|revision| *revision = revision.saturating_add(1))
            .or_insert(1);
    }

    pub(super) fn get(&self, uri: &str) -> Option<String> {
        self.documents.get(uri).map(|v| v.clone())
    }

    pub(super) fn entries(&self) -> Vec<(String, String)> {
        self.documents
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    pub(super) fn revision(&self, uri: &str) -> Option<u64> {
        self.revisions.get(uri).map(|revision| *revision)
    }

    pub(super) fn version(&self, uri: &str) -> Option<i32> {
        self.versions.get(uri).map(|version| *version)
    }

    pub(super) fn remove(&self, uri: &str) {
        self.documents.remove(uri);
        self.revisions.remove(uri);
        self.versions.remove(uri);
    }
}
