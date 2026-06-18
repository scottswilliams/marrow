# Surface Syntax Lane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement roadmap lane 1 for application surfaces: accepted `surface` source syntax, AST shape, formatter round trip, language docs, and source-local namespace diagnostics, with no runtime behavior or `SurfaceAbi` facts.

**Architecture:** Keep syntax ownership in `marrow-syntax`: `surface` is the only new parser-reserved keyword, while `from`, `fields`, `collection`, `as`, `create`, and `update` remain contextual identifiers parsed only inside a `surface` block. Keep checker ownership narrow in `marrow-check`: source-local namespace collisions use the reserved `check.surface_collision` code, but store/field/index resolution and ABI facts remain deferred to later lanes.

**Tech Stack:** Rust workspace, `marrow-syntax` parser/formatter tests, `marrow-check` project-check tests, Markdown language docs. Every cargo command must include `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax` and `--manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml`.

---

### Task 1: Surface AST And Parser

**Files:**
- Modify: `crates/marrow-syntax/src/token.rs`
- Modify: `crates/marrow-syntax/src/ast.rs`
- Modify: `crates/marrow-syntax/src/lib.rs`
- Modify: `crates/marrow-syntax/src/parse_decl/mod.rs`
- Modify: `crates/marrow-syntax/src/parse_decl/decl.rs`
- Create: `crates/marrow-syntax/src/parse_decl/surface.rs`
- Modify: `crates/marrow-syntax/tests/main.rs`
- Create: `crates/marrow-syntax/tests/cases/parse_surface.rs`
- Modify: `crates/marrow-syntax/tests/cases/lexer.rs`

- [ ] **Step 1: Write the failing parser tests**

Add `crates/marrow-syntax/tests/cases/parse_surface.rs`:

```rust
use marrow_syntax::{
    Declaration, ExpectedSyntax, ParseDiagnosticReason, SurfaceItem, SurfaceTarget, parse_source,
};

fn parse_reason(reason: ParseDiagnosticReason) -> marrow_syntax::DiagnosticReason {
    marrow_syntax::DiagnosticReason::Parser(reason)
}

fn surface_decl(source: &str) -> marrow_syntax::SurfaceDecl {
    let parsed = parse_source(source);
    assert!(
        parsed.diagnostics.is_empty(),
        "expected clean parse, got {:#?}",
        parsed.diagnostics
    );
    parsed
        .file
        .declarations
        .iter()
        .find_map(|decl| match decl {
            Declaration::Surface(surface) => Some(surface.clone()),
            _ => None,
        })
        .expect("surface declaration")
}

#[test]
fn parses_surface_declaration_with_contextual_items() {
    let surface = surface_decl(
        "module app\n\
         surface Books from ^books\n\
         \x20   fields title, author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   create title, author, blurb\n\
         \x20   update title, blurb\n",
    );

    assert_eq!(surface.name, "Books");
    assert_eq!(surface.store.root, "books");
    assert!(surface.store.keys.is_empty());
    assert_eq!(surface.items.len(), 5);
    assert_eq!(
        surface.items[0],
        SurfaceItem::Fields {
            names: vec!["title".into(), "author".into(), "blurb".into()],
            span: surface.items[0].span(),
        }
    );
    assert_eq!(
        surface.items[1],
        SurfaceItem::Collection {
            target: SurfaceTarget::Root { root: "books".into() },
            alias: "list".into(),
            span: surface.items[1].span(),
        }
    );
    assert_eq!(
        surface.items[2],
        SurfaceItem::Collection {
            target: SurfaceTarget::Index {
                root: "books".into(),
                index: "byAuthor".into()
            },
            alias: "byAuthor".into(),
            span: surface.items[2].span(),
        }
    );
    assert_eq!(
        surface.items[3],
        SurfaceItem::Create {
            names: vec!["title".into(), "author".into(), "blurb".into()],
            span: surface.items[3].span(),
        }
    );
    assert_eq!(
        surface.items[4],
        SurfaceItem::Update {
            names: vec!["title".into(), "blurb".into()],
            span: surface.items[4].span(),
        }
    );
}

#[test]
fn surface_contextual_words_remain_identifiers_outside_surface_blocks() {
    let parsed = parse_source(
        "module app\n\
         const from = 1\n\
         fn fields(collection: int)\n\
         \x20   const create = collection\n\
         \x20   return\n",
    );

    assert!(parsed.diagnostics.is_empty(), "{:#?}", parsed.diagnostics);
    assert!(parsed.file.function("fields").is_some());
}

#[test]
fn reports_malformed_surface_header_and_items() {
    let cases = [
        (
            "module app\nsurface Books ^books\n",
            ExpectedSyntax::SurfaceHeader,
        ),
        (
            "module app\nsurface Books from books\n",
            ExpectedSyntax::SurfaceStore,
        ),
        (
            "module app\nsurface Books from ^books\n    fields\n",
            ExpectedSyntax::SurfaceFieldList,
        ),
        (
            "module app\nsurface Books from ^books\n    collection ^books\n",
            ExpectedSyntax::SurfaceCollection,
        ),
        (
            "module app\nsurface Books from ^books\n    bogus title\n",
            ExpectedSyntax::SurfaceItem,
        ),
    ];
    for (source, expected) in cases {
        let parsed = parse_source(source);
        assert!(
            parsed.diagnostics.iter().any(|diagnostic| {
                diagnostic.reason
                    == parse_reason(ParseDiagnosticReason::Expected(expected))
            }),
            "expected {expected:?} for {source:?}: {:#?}",
            parsed.diagnostics
        );
    }
}

#[test]
fn rejects_surface_collection_targets_that_are_not_source_native_root_or_index_paths() {
    let parsed = parse_source(
        "module app\n\
         surface Books from ^books\n\
         \x20   collection books as list\n\
         \x20   collection ^books.byAuthor.extra as bad\n",
    );

    assert!(
        parsed.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason
                == parse_reason(ParseDiagnosticReason::Expected(
                    ExpectedSyntax::SurfaceCollectionTarget
                ))
        }),
        "{:#?}",
        parsed.diagnostics
    );
}
```

Update `crates/marrow-syntax/tests/main.rs`:

```rust
#[path = "cases/parse_surface.rs"]
mod parse_surface;
```

Update `crates/marrow-syntax/tests/cases/lexer.rs` by replacing `lexes_future_surface_reservations_as_keywords` with:

```rust
#[test]
fn lexes_surface_as_the_only_application_surface_keyword() {
    let source = "surface from fields collection as create update";

    assert_eq!(
        kinds(source),
        vec![
            TokenKind::Keyword(Keyword::Surface),
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Identifier,
            TokenKind::Eof,
        ]
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax parse_surface:: --test main
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax lexer::lexes_surface_as_the_only_application_surface_keyword --test main
```

Expected: FAIL to compile because `SurfaceDecl`, `SurfaceItem`, `SurfaceTarget`, `Keyword::Surface`, and the new `ExpectedSyntax` variants do not exist.

- [ ] **Step 3: Add the minimal AST and parser implementation**

In `crates/marrow-syntax/src/token.rs`, add `Surface` to `Keyword` and map only `"surface"` to it. Do not add `from`, `fields`, `collection`, `as`, `create`, or `update` as keywords.

In `crates/marrow-syntax/src/diagnostic.rs`, add:

```rust
SurfaceBody,
SurfaceCollection,
SurfaceCollectionTarget,
SurfaceFieldList,
SurfaceHeader,
SurfaceItem,
SurfaceName,
SurfaceStore,
```

to `ExpectedSyntax`.

In `crates/marrow-syntax/src/ast.rs`, add `Surface` to `Declaration`, add `SourceFile::surface`, and add:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDecl {
    pub name: String,
    pub store: SavedRoot,
    pub items: Vec<SurfaceItem>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceItem {
    Fields { names: Vec<String>, span: SourceSpan },
    Collection {
        target: SurfaceTarget,
        alias: String,
        span: SourceSpan,
    },
    Create { names: Vec<String>, span: SourceSpan },
    Update { names: Vec<String>, span: SourceSpan },
}

impl SurfaceItem {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Fields { span, .. }
            | Self::Collection { span, .. }
            | Self::Create { span, .. }
            | Self::Update { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceTarget {
    Root { root: String },
    Index { root: String, index: String },
}
```

In `crates/marrow-syntax/src/lib.rs`, re-export the new AST types.

In `crates/marrow-syntax/src/parse_decl/mod.rs`, add `mod surface;`.

In `crates/marrow-syntax/src/parse_decl/decl.rs`, import `Keyword::Surface`, dispatch `surface` as a top-level declaration when followed by space, push `Declaration::Surface`, and include it in the unknown-declaration message:

```rust
Some(TokenKind::Keyword(Keyword::Surface)) if self.keyword_introduces_decl() => {
    let trailing_comment = self.peek_header_trailing_comment();
    let surface = self.parse_surface();
    file.declarations.push(Declaration::Surface(surface));
    file.comments.extend(trailing_comment);
}
```

Create `crates/marrow-syntax/src/parse_decl/surface.rs`. Keep it self-contained: parse `surface Name from ^root`, parse an indented body, parse item lead words as contextual identifiers, use shared header/comment helpers, and validate comma-separated name lists with identifier tokens only.

- [ ] **Step 4: Run syntax tests to verify they pass**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax parse_surface:: --test main
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax lexer::lexes_surface_as_the_only_application_surface_keyword --test main
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/marrow-syntax/src/token.rs crates/marrow-syntax/src/diagnostic.rs crates/marrow-syntax/src/ast.rs crates/marrow-syntax/src/lib.rs crates/marrow-syntax/src/parse_decl/mod.rs crates/marrow-syntax/src/parse_decl/decl.rs crates/marrow-syntax/src/parse_decl/surface.rs crates/marrow-syntax/tests/main.rs crates/marrow-syntax/tests/cases/parse_surface.rs crates/marrow-syntax/tests/cases/lexer.rs
git commit -m "Add surface syntax parser"
```

### Task 2: Surface Formatter And Syntax Docs

**Files:**
- Modify: `crates/marrow-syntax/src/format.rs`
- Modify: `crates/marrow-syntax/tests/cases/format.rs`
- Modify: `docs/language/grammar.md`
- Modify: `docs/language/syntax.md`

- [ ] **Step 1: Write the failing formatter tests**

Add to `crates/marrow-syntax/tests/cases/format.rs`:

```rust
#[test]
fn formats_surface_declaration() {
    let source = "module app\n\
         surface Books from ^books\n\
         \x20   fields title,author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   create title,author, blurb\n\
         \x20   update title, blurb\n";
    let expected = "module app\n\n\
         surface Books from ^books\n\
         \x20   fields title, author, blurb\n\
         \x20   collection ^books as list\n\
         \x20   collection ^books.byAuthor as byAuthor\n\
         \x20   create title, author, blurb\n\
         \x20   update title, blurb\n";

    assert_eq!(format_source(source), expected);
}

#[test]
fn surface_format_is_idempotent() {
    let source = "module app\n\
         surface Books from ^books\n\
         \x20   fields title, author\n\
         \x20   collection ^books as list\n";

    let once = format_source(source);
    assert_eq!(format_source(&once), once);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax format::formats_surface_declaration --test main
```

Expected: FAIL because `format_declaration` does not render `Declaration::Surface`.

- [ ] **Step 3: Implement minimal formatter support**

In `crates/marrow-syntax/src/format.rs`, import `SurfaceDecl`, `SurfaceItem`, and `SurfaceTarget`. Add `Declaration::Surface` to `declaration_trailing_comment_line`, `declaration_span`, and `format_declaration`. Implement:

```rust
fn format_surface(decl: &SurfaceDecl) -> String {
    let mut out = format!("surface {} from ^{}", decl.name, decl.store.root);
    let body = format_body_lines(
        &decl.comments,
        decl.items.iter().map(|item| FormattedBodyLine {
            span: item.span(),
            text: format_surface_item(item),
            trailing_comment_line: TrailingCommentLine::Last,
        }),
    );
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn format_surface_item(item: &SurfaceItem) -> String {
    match item {
        SurfaceItem::Fields { names, .. } => format!("{INDENT}fields {}", names.join(", ")),
        SurfaceItem::Collection { target, alias, .. } => {
            format!("{INDENT}collection {} as {alias}", format_surface_target(target))
        }
        SurfaceItem::Create { names, .. } => format!("{INDENT}create {}", names.join(", ")),
        SurfaceItem::Update { names, .. } => format!("{INDENT}update {}", names.join(", ")),
    }
}

fn format_surface_target(target: &SurfaceTarget) -> String {
    match target {
        SurfaceTarget::Root { root } => format!("^{root}"),
        SurfaceTarget::Index { root, index } => format!("^{root}.{index}"),
    }
}
```

- [ ] **Step 4: Update language docs**

In `docs/language/grammar.md`, add `surface_decl` to `top_level_decl` and define:

```ebnf
surface_decl    =
    "surface" identifier "from" saved_root NEWLINE
    INDENT surface_item+ DEDENT ;

surface_item    =
      "fields" identifier_list NEWLINE
    | "collection" surface_collection_target "as" identifier NEWLINE
    | "create" identifier_list NEWLINE
    | "update" identifier_list NEWLINE ;

surface_collection_target =
      saved_root
    | saved_root "." identifier ;

identifier_list = identifier ("," identifier)* ","? ;
```

State that `from`, `fields`, `collection`, `as`, `create`, and `update` are contextual in surface declarations; only `surface` is parser-reserved.

In `docs/language/syntax.md`, add a short `Application Surfaces` section using the canonical example from the design and noting that this parser slice has no runtime behavior.

- [ ] **Step 5: Run tests to verify they pass**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax format::formats_surface_declaration --test main
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax format::surface_format_is_idempotent --test main
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/marrow-syntax/src/format.rs crates/marrow-syntax/tests/cases/format.rs docs/language/grammar.md docs/language/syntax.md
git commit -m "Format and document surface declarations"
```

### Task 3: Surface Namespace Diagnostics

**Files:**
- Modify: `crates/marrow-check/src/diagnostics.rs`
- Modify: `crates/marrow-check/src/lib.rs`
- Modify: `crates/marrow-check/src/driver.rs`
- Modify: `crates/marrow-check/tests/main.rs`
- Create: `crates/marrow-check/tests/cases/project_surfaces.rs`

- [ ] **Step 1: Write the failing checker tests**

Add `crates/marrow-check/tests/cases/project_surfaces.rs`:

```rust
use crate::support::{assert_clean, config, temp_project, with_code, write};
use marrow_check::{DiagnosticPayload, check_project};
use marrow_syntax::SourceSpan;

#[test]
fn surface_declaration_name_shares_the_module_namespace() {
    let root = temp_project("surface-module-collision", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Books\n\
             \x20   title: string\n\
             store ^books(id: int): Books\n\
             surface Books from ^books\n\
             \x20   fields title\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    let diagnostics = with_code(&report, "check.surface_collision");
    assert_eq!(diagnostics.len(), 1, "{:#?}", report.diagnostics);
    assert!(matches!(
        diagnostics[0].payload,
        DiagnosticPayload::SurfaceCollision { .. }
    ));
}

#[test]
fn surface_local_namespace_collisions_use_surface_collision() {
    let root = temp_project("surface-local-collision", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   title: string\n\
             \x20   id: string\n\
             store ^books(id: int): Book\n\
             surface Books from ^books\n\
             \x20   fields title, id\n\
             \x20   collection ^books as title\n\
             \x20   collection ^books as create\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    let diagnostics = with_code(&report, "check.surface_collision");
    assert!(
        diagnostics.len() >= 3,
        "expected id/title/create collisions: {:#?}",
        report.diagnostics
    );
    assert!(diagnostics.iter().all(|diagnostic| matches!(
        diagnostic.payload,
        DiagnosticPayload::SurfaceCollision { .. }
    )));
}

#[test]
fn distinct_surface_namespaces_are_independent() {
    let root = temp_project("surface-independent", |root| {
        write(
            root,
            "src/app.mw",
            "module app\n\
             resource Book\n\
             \x20   title: string\n\
             store ^books(id: int): Book\n\
             surface PublicBooks from ^books\n\
             \x20   fields title\n\
             \x20   collection ^books as list\n\
             surface AdminBooks from ^books\n\
             \x20   fields title\n\
             \x20   collection ^books as list\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}
```

Update `crates/marrow-check/tests/main.rs`:

```rust
#[path = "cases/project_surfaces.rs"]
mod project_surfaces;
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-check project_surfaces:: --test main
```

Expected: FAIL to compile because `CHECK_SURFACE_COLLISION` and `DiagnosticPayload::SurfaceCollision` do not exist, or FAIL behavior because no namespace diagnostics are emitted.

- [ ] **Step 3: Implement minimal source-local checker diagnostics**

In `crates/marrow-check/src/diagnostics.rs`, add:

```rust
pub const CHECK_SURFACE_COLLISION: &str = "check.surface_collision";
```

and payload:

```rust
SurfaceCollision {
    surface: String,
    name: String,
    first_span: SourceSpan,
},
```

In `crates/marrow-check/src/lib.rs`, re-export `CHECK_SURFACE_COLLISION`.

In `crates/marrow-check/src/driver.rs`:

- include surface declarations in the module-level namespace scan, but emit
  `check.surface_collision` instead of `check.duplicate_declaration` whenever
  either side of the collision is a surface declaration;
- add a small `check_surface_local_namespaces(file, source, diagnostics)` called beside duplicate declaration checks;
- for each surface, seed the local namespace with `id`, `get`, `create`, and `update` at the surface span;
- add field names from `fields` and aliases from `collection`;
- report later collisions with `check.surface_collision` and `DiagnosticPayload::SurfaceCollision`;
- do not resolve store targets, field existence, index ownership, create/update support, or ABI facts in this lane.

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-check project_surfaces:: --test main
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/marrow-check/src/diagnostics.rs crates/marrow-check/src/lib.rs crates/marrow-check/src/driver.rs crates/marrow-check/tests/main.rs crates/marrow-check/tests/cases/project_surfaces.rs
git commit -m "Check surface namespace collisions"
```

### Task 4: Lane Gates And Review

**Files:**
- No new feature files unless review requires fixes.

- [ ] **Step 1: Run focused syntax/checker gates**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax --test main parse_surface::
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-syntax --test main format::formats_surface_declaration
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -p marrow-check --test main project_surfaces::
```

Expected: PASS.

- [ ] **Step 2: Run broad gates**

Run:

```bash
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo fmt --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -- --check
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo clippy --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml -- -D warnings
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-surface-syntax cargo test --workspace --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-surface-syntax/Cargo.toml
```

Expected: all PASS.

- [ ] **Step 3: Run absence and sibling scans**

Run:

```bash
rg -n "serve|URI-tree|URI tree|surface.*TODO|SurfaceAbi|surface runtime" crates/marrow-syntax crates/marrow-check docs/language docs/superpowers/plans/2026-06-18-surface-syntax-lane.md
rg -n "Keyword::Surface|Declaration::Surface|SurfaceDecl|SurfaceItem|CHECK_SURFACE_COLLISION|check.surface_collision" crates/marrow-syntax crates/marrow-check docs/language
```

Expected: no `serve`/URI-tree implementation path in syntax/check; `SurfaceAbi` appears only in design docs or future-lane references, not as lane 1 runtime behavior.

- [ ] **Step 4: Request adversarial reviews**

Dispatch two read-only review agents:

- Soundness lens: verify parser recovery, contextual-keyword handling, spans, docs lockstep, no runtime/ABI behavior, and namespace collisions.
- Idiom/spec lens: verify Rust code shape, no broad dispatcher bloat, focused helpers, tests use production parser/checker, and docs are idiomatic.

Fix every blocking finding and rerun the affected focused gates.

- [ ] **Step 5: Integration readiness evidence**

Before integrating, collect:

```bash
git status --short --branch
git log --oneline --max-count=5
git diff --stat main...HEAD
```

Expected: lane branch contains only surface syntax/checker/docs changes plus this plan. No unrelated tool deletions, no `Cargo.lock` churn, no default `target/` output.
