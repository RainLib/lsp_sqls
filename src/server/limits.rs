//! Limits for the SQL language server.
pub(super) const CURRENT_SQL_DOCUMENT_URI: &str = "file:///current.sql";

pub(super) const CURRENT_JSON_DOCUMENT_URI: &str = "file:///current.json";

pub(super) const PROJECT_SQL_INDEX_MAX_BYTES: usize = 512 * 1024;

pub(super) const PROJECT_SQL_INDEX_MAX_DOCUMENTS: usize = 256;

pub(super) const PROJECT_SQL_INDEX_MAX_OCCURRENCES: usize = 10_000;

pub(super) const PROJECT_SQL_INDEX_MAX_RESULTS: usize = 2_000;

pub(super) const SIGNATURE_HELP_MAX_SCAN_BYTES: usize = 64 * 1024;

pub(super) const SIGNATURE_HELP_MAX_OVERLOADS: usize = 50;

pub(super) const BUILTIN_FUNCTION_COMPLETION_MAX_ITEMS: usize = 100;

pub(super) const COMPLETION_RESOLVE_CACHE_MAX_ENTRIES: usize = 8_192;

pub(super) const COMPLETION_RESOLVE_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) const COMPLETION_RESOLVE_DOCUMENTATION_MAX_BYTES: usize = 256 * 1024;

pub(super) const INLAY_HINT_STATEMENT_SCAN_BYTES: usize = 32 * 1024;

pub(super) const INLAY_HINT_REQUEST_MAX_BYTES: usize = 64 * 1024;

pub(super) const INLAY_HINT_MAX_ITEMS: usize = 500;

pub(super) const COMPLETION_RESOLVE_ID_FIELD: &str = "oxideSqlLspCompletionResolveId";

pub(super) const COMPLETION_RESOLVE_ORIGINAL_DATA_FIELD: &str = "originalData";
