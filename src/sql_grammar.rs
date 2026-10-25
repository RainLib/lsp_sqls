//! Private binding to the checked-in MIT SQL grammar. See grammars/sql/LICENSE
//! and OXIDE_PATCHES.md for its published source, patch and generation command.
use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_sql() -> *const ();
}

pub(crate) fn language() -> tree_sitter::Language {
    // SAFETY: build.rs links this generated C function. It returns the static
    // TSLanguage for ABI 14, supported by our locked Tree-sitter runtime.
    tree_sitter::Language::new(unsafe { LanguageFn::from_raw(tree_sitter_sql) })
}
