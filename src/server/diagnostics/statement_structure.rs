//! Syntax-owned mutation clauses and error-tolerant, query-level JOIN spans.
use crate::parser::SqlParser;
use std::ops::Range;
use tree_sitter::Node;
#[cfg(test)]
thread_local! {pub(in crate::server) static FALLBACK_PARSE_COUNT:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
#[derive(Default)]
pub(super) struct Structure {
    pub mutations: Vec<Mutation>,
    pub joins: Vec<MissingJoin>,
    scopes: Vec<Scope>,
}
pub(in crate::server) struct Mutation {
    pub keyword: usize,
    pub range: Range<usize>,
    pub owner: Range<usize>,
    pub root: bool,
}
pub(in crate::server) struct MissingJoin {
    pub keyword: usize,
    pub insert: Option<usize>,
    pub scope_start: usize,
}
struct Scope {
    range: Range<usize>,
    parent: Option<usize>,
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Word,
    Open,
    Close,
    Semi,
    Opaque,
    Other,
}
struct Token {
    start: usize,
    end: usize,
    kind: Kind,
    depth: usize,
    mate: Option<usize>,
    scope: usize,
    join_keyword: bool,
}
impl Token {
    fn word<'a>(&self, source: &'a str) -> &'a str {
        if self.kind == Kind::Word {
            &source[self.start..self.end]
        } else {
            ""
        }
    }
}
fn child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).find(|n| n.kind() == kind);
    found
}
fn own_where(node: Node) -> bool {
    child(node, "where").is_some()
        || child(node, "from").is_some_and(|from| child(from, "where").is_some())
}
fn scope_start(node: Node) -> usize {
    let mut parent = node.parent();
    while let Some(n) = parent {
        if matches!(n.kind(), "statement" | "subquery") {
            return n.start_byte();
        }
        parent = n.parent();
    }
    0
}
impl Structure {
    pub fn new(
        source: &str,
        normalized: &str,
        has_directives: bool,
        tree: &tree_sitter::Tree,
    ) -> Self {
        let mut result = Self::default();
        let want_joins = super::contains_sql_keyword(normalized, "JOIN");
        let mut tokens = Vec::new();
        let mut cursor = tree.walk();
        loop {
            let n = cursor.node();
            if has_directives && matches!(n.kind(), "statement" | "subquery" | "cte") {
                result.scopes.push(Scope {
                    range: n.byte_range(),
                    parent: None,
                });
            }
            if matches!(n.kind(), "update" | "delete") {
                let name = if n.kind() == "update" {
                    "keyword_update"
                } else {
                    "keyword_delete"
                };
                if let Some(keyword) = child(n, name).filter(|k| {
                    !k.is_missing()
                        && normalized[k.byte_range()]
                            .trim()
                            .eq_ignore_ascii_case(&name[8..])
                }) {
                    let owner = n.parent().filter(|p| p.kind() == "statement").unwrap_or(n);
                    // EXPLAIN without ANALYZE describes a plan without executing it.
                    let mut ancestor = Some(owner);
                    let mut plan_only = false;
                    while let Some(scope) = ancestor {
                        if scope.kind() == "statement" && child(scope, "keyword_explain").is_some()
                        {
                            plan_only = child(scope, "keyword_analyze").is_none();
                            break;
                        }
                        ancestor = scope.parent();
                    }
                    if !plan_only && !own_where(n) && !own_where(owner) {
                        result.mutations.push(Mutation {
                            keyword: keyword.start_byte(),
                            range: keyword.start_byte()..owner.end_byte(),
                            owner: owner.byte_range(),
                            root: owner.parent().is_some_and(|p| p.kind() == "program"),
                        });
                    }
                }
            }
            let opaque = n.kind() == "identifier"
                && normalized[n.byte_range()].trim().is_empty()
                && !source[n.byte_range()].trim().is_empty();
            let leaf = n.child_count() == 0;
            if want_joins && !n.is_missing() && (!n.is_extra() || n.is_error()) && (opaque || leaf)
            {
                if opaque {
                    tokens.push(Token {
                        start: n.start_byte(),
                        end: n.end_byte(),
                        kind: Kind::Opaque,
                        depth: 0,
                        mate: None,
                        scope: 0,
                        join_keyword: false,
                    });
                } else {
                    scan(
                        &normalized[n.byte_range()],
                        n.start_byte(),
                        if n.kind() == "keyword_join" {
                            scope_start(n)
                        } else {
                            0
                        },
                        n.kind() == "keyword_join",
                        &mut tokens,
                    );
                }
            }
            // Tree-sitter marks recovery ERROR nodes as extra too. Their
            // children still own real keywords and recoverable JOIN targets.
            if !opaque && (!n.is_extra() || n.is_error()) && cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    result.finish(tokens, normalized);
                    return result;
                }
            }
        }
    }
    pub fn comment_matches(&self, at: usize, comment: &Range<usize>) -> bool {
        let end = self
            .scopes
            .partition_point(|s| s.range.start <= comment.start);
        let mut index = end.checked_sub(1);
        while let Some(i) = index {
            let s = &self.scopes[i];
            if s.range.end >= comment.end {
                return s.range.contains(&at);
            }
            index = s.parent;
        }
        true
    }
    fn finish(&mut self, mut tokens: Vec<Token>, source: &str) {
        let mut parents = Vec::<usize>::new();
        for i in 0..self.scopes.len() {
            while parents.last().is_some_and(|p| {
                self.scopes[*p].range.end < self.scopes[i].range.end
                    || self.scopes[*p].range.end <= self.scopes[i].range.start
            }) {
                parents.pop();
            }
            self.scopes[i].parent = parents.last().copied();
            parents.push(i);
        }
        let mut stack = Vec::<usize>::new();
        for i in 0..tokens.len() {
            if tokens[i].kind == Kind::Close {
                if let Some(open) = stack.pop() {
                    tokens[open].mate = Some(i);
                    tokens[i].mate = Some(open);
                }
            }
            tokens[i].depth = stack.len();
            if tokens[i].kind == Kind::Open {
                stack.push(i);
            }
        }
        for i in 0..tokens.len() {
            if !tokens[i].join_keyword || tokens[i].word(source) != "JOIN" {
                continue;
            }
            let depth = tokens[i].depth;
            let mut before = i;
            let mut exempt = false;
            while before > 0 && tokens[before - 1].depth == depth {
                let word = tokens[before - 1].word(source);
                if matches!(word, "CROSS" | "NATURAL") {
                    exempt = true;
                    break;
                }
                if !matches!(word, "LEFT" | "RIGHT" | "INNER" | "FULL" | "OUTER") {
                    break;
                }
                before -= 1;
            }
            if exempt {
                continue;
            }
            let mut j = i + 1;
            let mut last = None;
            let mut conditioned = false;
            let mut complete = true;
            while j < tokens.len() {
                let t = &tokens[j];
                if t.depth < depth || t.kind == Kind::Semi {
                    break;
                }
                let word = t.word(source);
                if t.depth == depth && matches!(word, "ON" | "USING") {
                    conditioned = true;
                    break;
                }
                if t.depth == depth
                    && matches!(
                        word,
                        "JOIN"
                            | "LEFT"
                            | "RIGHT"
                            | "INNER"
                            | "FULL"
                            | "OUTER"
                            | "CROSS"
                            | "NATURAL"
                            | "WHERE"
                            | "GROUP"
                            | "HAVING"
                            | "ORDER"
                            | "LIMIT"
                            | "UNION"
                            | "EXCEPT"
                            | "INTERSECT"
                            | "RETURNING"
                            | "QUALIFY"
                            | "WINDOW"
                            | "SET"
                            | "FOR"
                            | "OFFSET"
                            | "FETCH"
                    )
                {
                    break;
                }
                if t.kind == Kind::Open {
                    if let Some(close) = t.mate {
                        last = Some(tokens[close].end);
                        j = close + 1;
                        continue;
                    }
                    complete = false;
                    break;
                }
                last = Some(t.end);
                j += 1;
            }
            if !conditioned {
                self.joins.push(MissingJoin {
                    keyword: tokens[i].start,
                    insert: last.filter(|_| complete),
                    scope_start: tokens[i].scope,
                });
            }
        }
        self.mutations.sort_by_key(|m| m.keyword);
    }
}
fn scan(source: &str, offset: usize, scope: usize, join_keyword: bool, tokens: &mut Vec<Token>) {
    let mut at = 0;
    while at < source.len() {
        let ch = source[at..].chars().next().unwrap();
        if ch.is_whitespace() {
            at += ch.len_utf8();
            continue;
        }
        let start = at;
        at += ch.len_utf8();
        let kind = match ch {
            '(' => Kind::Open,
            ')' => Kind::Close,
            ';' => Kind::Semi,
            c if SqlParser::is_identifier_char(c) => {
                while at < source.len()
                    && source[at..]
                        .chars()
                        .next()
                        .is_some_and(SqlParser::is_identifier_char)
                {
                    at += source[at..].chars().next().unwrap().len_utf8();
                }
                Kind::Word
            }
            _ => Kind::Other,
        };
        tokens.push(Token {
            start: offset + start,
            end: offset + at,
            kind,
            depth: 0,
            mate: None,
            scope,
            join_keyword,
        });
    }
}
