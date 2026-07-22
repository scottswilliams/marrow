# CORE-F1 MAP — recovery nodes, position classes, candidate sources

Non-shipping lane design note (delete on integration). Authority: H00c-DESIGN §3 (position-class
law), §4 (bounds), §5 (file reservation). Base: origin/beta `285c9392`. Grounded read of
`crates/marrow-syntax/src/{ast.rs,parse_expr.rs}`, `crates/marrow-compile/src/{analysis.rs,
compile.rs,lower/**}`.

## Scoping correction (measured against the tree)

The packet names five incomplete forms. Two already have their surface today and need **no new
recovery node** — only classification (Stage 3):

- **Incomplete call tail** `f(a, ` — already recovers. `postfix_expr` (parse_expr.rs 675–697) emits
  an `Expression::Call` with the arguments parsed so far when the `)` is missing, carrying its
  `expected `,`/`)`` syntax diagnostic. A position inside that arg list is an ordinary **expression
  position** → `ExpressionName`. CORE-F2 owns the active-call fact over this same recovered node.
- **Partial identifier** `getO` — not a parse error at all; it parses as a complete
  `Expression::Name` that merely fails to resolve. A position over an unresolved `Name` in
  expression position is `ExpressionName`.

Three forms genuinely collapse to `Expression::Error` today and lose the base, so they get **typed
recovery nodes**:

| Form | Source | Today (parse_expr.rs) | Lost | Class |
|---|---|---|---|---|
| Member `expr.` / `expr?.` | after `.`/`?.`, no field ident | `field_segment` → `Error` (754–762) | the base receiver | `Member` |
| Path `Enum::` | after `::`, no segment | `name_expr` → `Error` (1189–1210) | segments-so-far | `EnumPath` |
| Type annotation `x: ` | type position, no type | TypeExpr parse → error (parse_decl) | annotation site | `TypeAnnotation` |

## Recovery-node representation (fork RESOLVED — locked)

**Locked:** extend the one existing placeholder rather than add three top-level variants.
`Expression::Error { span }` → `Expression::Error { span, recovery: Option<Recovery> }` where

```
enum Recovery {
    Member         { base: Box<Expression> },   // expr.
    OptionalMember { base: Box<Expression> },   // expr?.
    Path           { base: Box<Expression> },   // Enum::  (base is the Name so far)
}
```
and, in the type grammar, `TypeExpr` gains one inert `Incomplete { span }` leaf.

**Discriminate `.` from `?.` by variant, not a bool** (both reviews; workspace rule "a boolean that
changes semantics usually deserves a typed state"). The surrounding AST already splits exactly this
distinction into two variants — `Expression::Field` vs `Expression::OptionalField` — so `Recovery`
mirrors that split (`Member` / `OptionalMember`) for local consistency rather than carrying
`optional: bool`.

Rationale: every semantic match site (builtins/diagnostics/durable/exprs/stmts/format/lib/decl/stmt)
already handles `Error`; a struct-field addition keeps them compiling and keeps the node **inert and
fail-closed** in the compile path exactly like `Error` today. `recovery` is `None` for every error
the parser emits today, so no existing tree changes shape. Only the three incomplete forms populate
it. The alternative (three new `Expression` variants) ripples every match site and is the fallback
if review rejects widening `Error`.

**Byte-identity law.** The recovery node always travels with its existing `parse.syntax` Error
diagnostic (M3FIX01 truncation law — a broken file stays honestly broken; `has_errors()` stays
true). Semantic processing is gated on `!has_errors`, so the compile path never lowers a recovery
node: reject behavior is unchanged for programs *with* the incomplete form, and programs *without*
it never construct one. The agreement-gate fixture compiles a corpus with no incomplete form and
asserts image bytes + diagnostic set are identical pre/post. `format.rs` must **refuse** a tree
containing a recovery node (round-trip is meaningless for broken source) — revalidate the
`check_format` refusal over recovery nodes (reservation includes format.rs).

## Position-class derivation (checker is the only authority — purely positional, no token scan)

Classification runs over the parsed AST at a byte offset; it inspects **nodes**, never the trigger
char, `CompletionContext`, or raw text (H00c §3). The owner is the checker's one resolution model
(`FnLowerer`, lower/mod.rs 238+), which already holds every namespace a class needs:

- **`ExpressionName`** — offset over an `Expression::Name` (resolved or not) in expression position,
  or inside a recovered call's arg list. Namespace: `self.locals` (in scope before the offset) +
  `FunctionRegistry` (module fns) + `ConstRegistry` + builtins + imported module names + enum type
  names (`registry.rs`, `builtins.rs`).
- **`Member`** — offset in a `Recovery::Member` / `Recovery::OptionalMember` node (or a `Field`/
  `OptionalField` name span). Type the base through the **fail-soft type probe** (see Collection
  path): a struct type → declared fields (`TypeRegistry`, `resolve_product_field` exprs.rs 3054); any
  partial, unresolvable, or invalid base → `Absent` (never a lowerer failure arm).
- **`EnumPath`** — offset in a `Recovery::Path` node whose base resolves to an enum type/path.
  Immediate child members from the enum decl; `category` members marked non-selectable
  (`EnumMember`, ast.rs).
- **`TypeAnnotation`** — offset in a `TypeExpr::Incomplete` (or a type-position name). Namespace:
  named types + aliases + generic templates (`GenericRegistry`) + builtin type names + in-scope
  type parameters (`type_env`, lower/mod.rs 265).
- Comments/strings/literals/whitespace outside any recovered node → `Absent`.

## Candidate fact shape (H00c §1 CORE-F1, §4 bounds)

```
AnalysisSnapshot::completions(file, byte_offset) -> Result<Fact<Completions>, QueryError>
Completions { class: PositionClass, candidates: Vec<Candidate> }
PositionClass  = ExpressionName | Member | EnumPath | TypeAnnotation   // closed
Candidate { label: String /*declared spelling*/, kind: CandidateKind, detail: String /*ISP01*/ }
CandidateKind  = Function | Builtin | Local | Param | Const | Field
               | EnumMember { selectable: bool } | Type | TypeParam | Module   // closed
```
Complete in-scope namespace for the class — never prefix-filtered, ranked, or truncated. Over-cap is
a typed refusal, never a truncated prefix: `MAX_COMPLETION_CANDIDATES = 512`,
`MAX_COMPLETION_RENDER_BYTES = 256 KiB` per query, mapped to the `AnalysisResourceLimit` family
(new arms `CompletionCandidateCount`/`CompletionRenderBytes`), server → recoverable `-32803`.
Computed **per query** over retained checker facts; candidate sets are never retained per position
(unlike hover/symbol facts). `Unavailable(Syntax)` only when the *whole* file has no recoverable
structure at the position; a broken file with a recovered node at the offset still classifies.
`QueryError::{UnknownFile,OffsetOutOfRange}` reused verbatim. Revision echoed by the snapshot.

## Collection path (SPR01 — analysis only, zero compile-path collection; forks RESOLVED)

Hover facts are collected during lowering into `Lowered.hover_facts` (lower/mod.rs 183,
`record_hover` 411) and threaded through `compile.rs` `drive`/`analyze_project` (620–804).

**Per-query re-resolution only — no retained per-position resolution env (locked; both reviews).**
The "re-runnable handle to the resolution environment" option is **deleted**: a retained handle is
checker-internal state held on `AnalysisSnapshot` across queries, which is a lifetime/aliasing
hazard and directly contradicts §96/§4's own law that *candidate sets are never retained per
position* (and would silently reintroduce the O(N²)/retention pressure SPR01/CORE-F4 fight).
`completions` computes candidates **per query** over facts the snapshot **already** retains: the
parsed AST plus the name / type / const / generic registries. The snapshot gains **no** new
per-position state.

**Read-only traversal that never re-enters the compile-path lowerer/resolver (locked; soundness
F2).** The compile-path lowerer (`FnLowerer`, `lower/exprs.rs`) assumes post-`has_errors` input:
its unmatched arm is `other => self.fail(unsupported(...))` (exprs.rs:240) and its resolution paths
can raise `ResolveError::Invariant` (exprs.rs:1645+); compilation only avoids these on a broken file
*because* `has_errors()` gates it (compile.rs 711/733/752/774). Completions deliberately runs on
broken files, so it must **not** drive that lowerer. Instead it is a distinct read-only pass that:
1. locates the AST node at the byte offset (purely positional);
2. enumerates the class namespace directly from the retained registries (Function / Const / Type /
   Generic) plus locals in scope before the offset plus builtins — no lowering;
3. for `Member` / `EnumPath`, types the base through a **fail-soft type probe** that returns
   `Absent` on any partial / unresolvable / invalid base instead of entering a `fail`/`Invariant`
   arm.
It emits **zero** diagnostics into the snapshot: a partial or malformed base yields an empty/`Absent`
classification, never an `unsupported`/`Invariant` leak. Gated only by the presence of a
recovered/unresolved node at the offset, never by `has_errors`.

## Stage sequence & red-first tests (per form: observe-today → node → byte-identity)

1. `parse_expr` tests: `member_dot_eof_recovers_base` (→ `Recovery::Member`),
   `member_optional_dot_eof_recovers_base` (→ `Recovery::OptionalMember`),
   `path_colon_colon_eof_retains_segments` (→ `Recovery::Path`), type-annotation incomplete test
   (parse_decl, → `TypeExpr::Incomplete`) — each first asserts today's `Error`, then the recovered
   node.
2. Byte-identity fixture under `fixtures/v01/**`: no-incomplete-form corpus, image+diagnostic bytes
   identical pre/post (agreement-gate style).
3. `format.rs` refusal test over a recovery node.
4. analysis.rs: `completions_expression_name`, `_member_fields`, `_enum_members`,
   `_type_annotation`, `completions_over_cap_refuses`, `completions_broken_file_still_classifies`,
   `completions_unknown_file`/`_offset_out_of_range`, `completions_absent_in_literal`.
5. Production red (H00c §6, owned by the H00c LSP lane, not this lane's files):
   `crates/marrow/tests/lsp_stdio.rs` completion at the `Role::` position in Graph Report currently
   `-32601`.

## STOPs honored (H00c §7)

STOP if recovery cannot live in the one parser with byte-identical compile behavior; STOP if the
position class needs a second resolution model; STOP if the LSP would need to read text/parse. None
hit at MAP time — the `Error`-widening fork keeps recovery inside the one parser, and every class
derives from the existing `FnLowerer` namespaces (read-only, without re-entering its failing arms).

## Review folds (2026-07-22 dual review of the MAP head)

Both independent reviews returned **NOT INTEGRABLE / NOT CLEAN — no implementation** (the lane head
changes only this non-shipping note; none of CORE-F1's code deliverables exist). Their design
findings against this "fork of record" are folded above and now locked:

- **F (idiom) — Recovery bool → variant split.** Folded: `Recovery::Member` / `OptionalMember`
  mirror `Field` / `OptionalField`; no `optional: bool`.
- **F (soundness+idiom) — collection-path either/or.** Folded: the retained resolution-env handle is
  deleted; per-query re-resolution over the already-retained AST + registries only; no new
  per-position snapshot state.
- **F (soundness) — broken-file re-entry.** Folded: completions is a read-only traversal that never
  drives the compile-path lowerer/resolver; a fail-soft type probe returns `Absent` on any partial
  base; zero diagnostics leak into the snapshot.

**Lane disposition:** this MAP is a non-shipping design note. CORE-F1's code deliverable is
undelivered and remains a build stage to be executed in a successor packet against this
fork-resolved spec. The board row must not be marked shipped on the strength of this note.
