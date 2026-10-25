# Oxide SQL grammar patch

This is a local source copy of the published `tree-sitter-sequel` 0.3.11 crate.
The original MIT license is in LICENSE; the upstream crate archive SHA-256 and
its reported VCS metadata are in oxide-upstream.json. The published artifact
reports a dirty VCS tree, so its crate checksum is the authoritative base.

oxide-grammar.patch contains the grammar changes:

- REGEXP/RLIKE/GLOB and their NOT forms retain binary operator nodes and
  complete source ranges. Each profile's semantic owner determines which
  operators have modeled results; generic syntax does not enable extensions.
- Custom type arguments also retain nested types, including Nullable(String),
  LowCardinality(Nullable(String)), Array(UInt8) and Map(String, UInt8).

- Custom type names retain one or two numeric modifiers, including ClickHouse
  Decimal64(2) and qualified custom types. The modifiers belong to the type,
  so CAST and postfix casts preserve their complete source and array dimensions.

- SELECT owns WHERE/GROUP/WINDOW/ORDER/LIMIT after its optional FROM. Those
  clauses no longer require a physical row source, and their nodes retain their
  original names and source coordinates.
- Set operands can be a parenthesized set operation. Grouping is represented
  in the generated tree instead of flattening or deleting parentheses.

- MySQL null-safe equality `<=>` has a binary comparison node and preserves
  the operator as one token.

- Array type dimensions bind to their type before expression subscripting.
  Casts retain complete array_size_definition nodes, including repeated
  dimensions; parenthesized cast results can still be subscripted normally.
- Postfix :: casts bind to their immediate expression operand before binary
  arithmetic, and chained casts retain left associativity.

- Arrays also retain real nodes for bare bracket constructors (ClickHouse)
  and ARRAY(value, ...) constructors (Hive). PostgreSQL-specific constructor
  restrictions are diagnosed by its parser profile.

- Invocation arguments retain an explicit VARIADIC keyword before their real
  parameter term. PostgreSQL-only/final-argument restrictions stay in the
  profile diagnostics; parameter ranges retain their original AST coordinates.

- Parenthesized CALL statements own their actual invocation node instead of
  falling through error recovery. Keyword and argument coordinates survive.

- Named invocation arguments retain a named_argument node with name, operator
  (=> or :=) and actual value fields. The MySQL/SQL Server profiles reject this
  syntax; live PostgreSQL binding owns semantic name/default/ordering checks.

The scanner and Rust binding originate from the same crate. Cargo.upstream.toml preserves the original published crate manifest as provenance.
The root LSP's build.rs compiles parser.c/scanner.c, and src/sql_grammar.rs owns
the private LanguageFn binding. There is no nested Cargo.toml or path dependency:
Cargo source packages retain the grammar and cannot fall back to the unpatched
registry crate. The already locked cc and tree-sitter-language versions are used
directly; tree-sitter-sequel is removed from the dependency graph. Registry caches
and the app bundle are not edited.

## Reproduce generated source

Use Node and the pinned Tree-sitter CLI from this directory:

```sh
npm exec --yes --package=tree-sitter-cli@0.26.3 -- tree-sitter generate --abi 14
```

ABI 14 matches the original grammar and is supported by the locked Tree-sitter
0.26.10 Rust runtime. package.json supplies CommonJS scope for upstream grammar.js;
it does not install application dependencies. Ordinary Cargo builds compile the
checked-in C source and do not download or run the CLI.

From the repository root, run `node scripts/verify-sql-grammar-source.mjs` to
verify the patch, source fingerprints, ABI and Cargo build integration. It is
also included in the SQL parser boundary check. Source hashes normalize CRLF
to LF so Windows and Linux checkouts verify the same text.

The generated-source snapshot records what was regenerated; semantic and
coordinate regressions are covered by the Rust parser and native LSP tests.
A valid parse does not certify every database's semantic or session rules.

## T-SQL modules and batches

Separate program branches retain procedure/function definitions and GO nodes;
the SQL Server profile validates module-first batches, parameter identities,
duplicate/2100-parameter limits, options and standalone positive GO counts.
Procedure parameters, scalar/inline-table function parameters and type/default
fields remain real nodes. Module blocks contain query/DML and supported table
DDL statements, nested blocks, RETURN, SET assignments/NOCOUNT, IF/ELSE and WHILE.
Optional module-body terminators are isolated from the ordinary program rule.
Explicit GLR conflicts preserve PostgreSQL CREATE FUNCTION and optional EXEC
continuations; full parser and protocol regressions cover both profiles.
CLR/native compilation, multi-statement table-return shapes, full T-SQL control
flow and database execution constraints still require separate implementation
and engine verification. These nodes do not certify engine-level validity.

## Parameterized datetime2

The datetime2 node retains optional fractional precision, matching time and
datetimeoffset. Inference compares the collected declaration spelling with
the real AST type node before propagating a scalar type into query outputs.

## Joined table inputs and implicit conditions

FROM retains comma-separated joined inputs, including a comma after a complete
JOIN. A named parenthesized_join node owns grouped and nested relation inputs.
NATURAL JOIN owns its implicit condition and does not require ON/USING; ordinary
JOIN still requires an explicit condition. Hidden inline join types preserve
direct keyword nodes for scope consumers without introducing broad GLR rules.
The checked patch is regenerated from the pinned published base with the same
tree-sitter-cli 0.26.3 and ABI 14. An independent generation reproduces all six
generated files byte for byte.

Request-owned JoinExpr layouts feed one catalog/local row binder for completion,
hover, output inference and ambiguity checks. Group aliases and USING/NATURAL
keys retain ordered output columns; unknown local inputs, invalid key shapes
and exhausted layout budgets do not publish partial stars or bind same-named
physical tables. Source traversal is bounded to 256 input/group nodes and 32
construction levels; output rows are bounded to 4096 columns. SQLite retains
its separately tested grouped-name and inner-ON visibility behavior.

An unfinished member immediately before FROM can swallow the input as a call
such as g.FROM(...). Completion alone hides that dangling dot in a request-local
tree with identical byte coordinates, within a 64 KiB suffix budget. It never
stores this recovery tree as canonical syntax or validates rewritten SQL.
SQL Server/Hive reject USING, and SQL Server/Hive/ClickHouse reject NATURAL in
their provider validation. This is not comprehensive engine validation: full
coercion/collation/session settings, uncommon grouped alias forms, every lateral
edge and engine-specific join families remain separate coverage work.

## LATERAL/APPLY correlation after joined inputs

The source model now supplies the preceding join-tree inputs for LATERAL/APPLY
correlation, retaining closed group aliases, USING output keys and nested left
operands. Anonymous subqueries retain an unknown local shape; no empty catalog
name is resolved. Empty eligible inputs and exhausted layouts are distinct.
Source discovery treats query parentheses as isolation boundaries while joined
input parentheses stay in the same query block, including a WITH token at the
query opening. OUTER APPLY is recognized from its original token range rather
than the adapted CROSS JOIN text. This step changes consumers only: grammar.js,
scanner and all six generated grammar outputs remain unchanged.

## ClickHouse QUALIFY

The grammar now retains a `qualify` predicate between WINDOW and ORDER BY.
It remains a shared syntax superset. LSP document validation rejects QUALIFY
for the configured unsupported SQL profiles and requires a window function in
the same ClickHouse SELECT, ignoring windows hidden in child queries/CTEs.
Parser recovery preserves errors in unfinished predicates; the existing policy
for suppressing some trailing incomplete diagnostics is unchanged.

The request-local QUALIFY binding uses the existing query masks to select its
own SELECT/set branch and verifies the actual keyword in the syntax tree.
It reuses projection inference, preserving CTE/derived metadata and routine
identity, and never installs output aliases as physical relation columns.
ClickHouse ranking functions, denseRank and count retain UInt64 output types.
Independent query blocks, qualified source members and ambiguous output names
do not acquire these output bindings. Conflicting input/alias types remain
unknown when the session's name-preference setting is unavailable.

This path has a 512 KiB source / 32768 clause-token budget. It merges existing
items by name in one pass, emits at most 256 matching output candidates and
reports isIncomplete when more remain; later exact names are retained before
the cap. No persistent candidate cache is introduced. Direct QUALIFY-alias
hover/navigation, complete ClickHouse type/session/version semantics, other
QUALIFY dialects and visible suggestion-widget interaction remain separate
work. The pinned 0.26.3 generator and ABI 14 are unchanged; an isolated second
generation reproduces all six generated files byte for byte.

## Hive LATERAL VIEW row sources

Relations now retain ordered LATERAL VIEW declarations, including OUTER and
optional AS column names. A narrow self-conflict preserves comma-separated
column lists without broadly enabling ambiguous SQL rules. These views are
folded into the same JoinExpr row layout as ordinary sources; arguments can
see preceding inputs but not their own output or later views. They never bind
to a same-named physical catalog table as a fallback.

Hive-only local row inference supports array/map explode, array posexplode,
inline over arrays of structs, json_tuple, parse_url_tuple and literal-row-count
stack. It reads nested metadata types with quoted struct names and decimal
commas, and reuses existing expression/UDF bindings plus conservative Hive
array/map/struct constructors, scalar casts and same-type CASE/set outputs.
Known row/container structure survives unknown element types; unsupported
coercions and arbitrary UDTF returns are not invented. Default field names and
OUTER nullability follow the documented Hive factories.

Input work is bounded to 512 KiB, 32768 syntax nodes, 64 views, 256 arguments or
fields, and 16 KiB / 32 nesting levels for complex metadata types. An exact
immutable source can share its original tree with projection inference. A
request-local completion tree also handles a dangling member before FROM;
canonical validation retains the original text. The shared grammar remains a
syntax superset, with unsupported-profile LATERAL VIEW errors emitted by LSP
document validation. Full vendor/version validation, arbitrary UDTF metadata,
all conversion and constructor forms, LATERAL TABLE and complete session rules
remain separate work.

The new grammar can recover a missing-ON JOIN tail as a sibling ERROR. Query
ownership now includes adjacent error nodes only within the same lexical
statement, preserving ambiguity warnings without crossing statement boundaries.
The pinned 0.26.3 generator and ABI 14 are retained; independent regeneration
and cumulative patch fingerprints are checked with the runtime tests.

Hive completion also offers LATERAL VIEW, VIEW/OUTER and six built-in UDTF
names while typing the introducer. This bounded, query/clause-aware entry
recognizer defers to normal field completion as soon as invocation arguments
start; quoted text/comments never become an empty function slot. It does not
classify arbitrary metadata routines as table-generating functions.

## Pattern ESCAPE operands

ESCAPE uses the existing binary-in precedence. A pattern's right operand can
therefore contain a binary ESCAPE expression without duplicating LIKE, ILIKE
or SIMILAR TO grammar branches. Original syntax ranges remain authoritative.
The shared grammar is a syntax superset: semantic inference separately checks
the actual operator, product and known server version. ClickHouse LIKE/ILIKE
ESCAPE requires 26.6; SIMILAR TO requires 26.10. Unknown versions, Hive ESCAPE,
DuckDB SIMILAR TO ESCAPE and non-pattern ESCAPE receive no invented type.

The existing expression owner handles pattern/escape operand types and NULL
propagation. SQLite permits Unicode characters and numeric coercion; the
tested DuckDB 1.5.5 escape character must be ASCII. Empty escapes are not
suggested because validity differs by product and MySQL SQL mode. Arbitrary
runtime expressions, SQL modes and every vendor extension are not validated.

Completion uses the existing bounded, request-local AST and snapshot identity.
The optional valueRole "pattern-escape" is bound to the same text type,
document version, profile, configuration and UTF-16 cursor. It selects plain
one-character literals or NULL instead of generic text/Boolean templates;
ordinary field and function expressions retain their identities. No additional
parser invocation, metadata request or persistent candidate cache is added.
The pinned generator is tree-sitter 0.26.3 and the generated ABI remains 14.

## Completed pattern continuations

Logical completion probes append a bounded AND operand rather than equality.
AND binds below LIKE/ILIKE and other matching operators, so the inferred left
operand retains the complete predicate, including ESCAPE. A closed string
literal no longer makes the lexical precheck treat the preceding operator as
an incomplete RHS. This uses the existing grammar and does not regenerate it.

The same request-local projection tree supplies a bounded trailing-pattern
inspection. It reuses operand, product, version and escape validation to offer
optional ESCAPE only at a directly attachable pattern, including the trailing
pattern in AND/OR expressions. Parenthesized whole predicates, unsupported
operators and already escaped predicates do not get another ESCAPE. SQL Server
predicate continuation is separate from projected scalar Boolean typing.
Standard completion and both snapshot routes share exact UTF-16 keyword edits.
No additional parse, metadata RPC or persistent candidate cache is introduced;
the new AST inspection is bounded to 32 levels, with existing document and
replacement-word limits retained. End-to-end speedup is not claimed.

The logical precheck also accepts the parser's existing completed-predicate
context. Dynamic and NULL escape operands can already have a valid predicate
type while the lexical classifier uses that more specific context; they must
not lose positive logical knowledge before reaching the shared type owner.

## Versioned pattern keyword capability

Both completion snapshots expose optional `patternEscape` Boolean knowledge
alongside a positive `logicalContinuation`. True authorizes a directly
attachable ESCAPE, false explicitly denies it, and omission preserves unknown
server-version knowledge. This differs from the `pattern-escape` value role,
which describes the character operand after an accepted keyword. The shared
pattern owner supplies both routes; document-only analysis cannot inherit a
warm physical catalog's version. All knowledge shares document, version,
dialect, configuration and UTF-16 cursor identity.

WHERE continuation now uses the existing bound snapshot RPC and structurally
checks the local pattern before applying that capability. It rejects malformed
or conflicting contracts atomically and withdraws stale version grants during
recovery. No additional parse, metadata read or persistent cache is added;
the bound snapshot replaces the legacy items-only completion request.
