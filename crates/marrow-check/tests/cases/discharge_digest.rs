use crate::support;
use crate::support_discharge;
use marrow_store::tree::TreeStore;

use support::{temp_project, write};
use support_discharge::*;

/// The shape and evolution digests of a single-file source, computed against the
/// source's own accepted catalog so its evolve default and transform targets bind to
/// real stable ids. The shape digest is the one the store stamps and the fence
/// enforces; the evolution digest is the one the witness records.
fn digests(name: &str, source: &str) -> (String, String) {
    let root = temp_project(name, |root| write(root, "src/books.mw", source));
    let program = commit_then_check(&root).expect("committed fixture");
    let store = TreeStore::memory();
    let witness = witness(&program, &store);
    (witness.source_digest, witness.evolution_digest)
}

/// The store-stamp shape digest.
fn source_digest(name: &str, source: &str) -> String {
    digests(name, source).0
}

/// The witness evolution digest.
fn evolution_digest(name: &str, source: &str) -> String {
    digests(name, source).1
}

/// One single-edit knob over the durable baseline. Each field maps to exactly one
/// durable fact the digest must bind; the default is the baseline source, and a case
/// flips a single field to assert that fact drifts the digest.
struct DurableFixture {
    identity_type: &'static str,
    count_type: &'static str,
    count_required: bool,
    index_unique: bool,
    index_columns: &'static str,
    versions_keys: &'static str,
    tags_keys: &'static str,
    default_value: &'static str,
    transform_body: &'static str,
    func: &'static str,
}

impl Default for DurableFixture {
    fn default() -> Self {
        Self {
            identity_type: "int",
            count_type: "int",
            count_required: true,
            index_unique: true,
            index_columns: "isbn, id",
            versions_keys: "version: int",
            tags_keys: "pos: int",
            default_value: "0",
            transform_body: "return \"x\"",
            func: "pub fn add(isbn: string): Id(^books)\n\
                   \x20   return nextId(^books)",
        }
    }
}

/// Render the durable baseline (or a single-edit variant) as one `.mw` source. The
/// resource carries a scalar member, a keyed group with a required leaf, a top-level
/// keyed leaf, and a unique index; the evolve block defaults the scalar member and
/// transforms `isbn`, so one fixture exercises every digest dimension.
fn durable_fixture(f: DurableFixture) -> String {
    let count_required = if f.count_required { "required " } else { "" };
    let index_unique = if f.index_unique { " unique" } else { "" };
    format!(
        "module books\n\
         resource Book\n\
         \x20   {count_required}count: {count_type}\n\
         \x20   pages: int\n\
         \x20   required isbn: string\n\
         \x20   tags({tags}): string\n\
         \x20   versions({versions})\n\
         \x20       required body: string\n\
         store ^books(id: {identity}): Book\n\
         \x20   index byIsbn({columns}){unique}\n\
         evolve\n\
         \x20   default Book.pages = {default}\n\
         \x20   transform Book.isbn\n\
         \x20       {transform}\n\
         {func}\n",
        identity = f.identity_type,
        count_type = f.count_type,
        tags = f.tags_keys,
        versions = f.versions_keys,
        columns = f.index_columns,
        unique = index_unique,
        default = f.default_value,
        transform = f.transform_body,
        func = f.func,
    )
}

/// The shape digest binds the whole durable shape, with no enumeration gap, so any
/// change to a member type, a required flag, an identity key, an index, a keyed-layer
/// key at any nesting depth, or a top-level keyed-leaf key must drift it, while a pure
/// whitespace reformat of the same declarations must leave it unchanged.
///
/// The evolve decision surface — a default value, a transform body — is *not* shape:
/// editing it leaves the shape digest unchanged but drifts the evolution digest the
/// witness records. The two are asserted together so the boundary is explicit.
///
/// The single baseline carries every dimension once. Each variant edits exactly one
/// fact at the same catalog path, so a digest that still matched the baseline would
/// prove that fact is unbound.
#[test]
fn source_digest_binds_the_durable_shape() {
    let base = durable_fixture(DurableFixture::default());
    let base_digest = source_digest("durable-base", &base);

    let shape_cases: [(&str, DurableFixture, &str); 8] = [
        (
            "member-type",
            DurableFixture {
                count_type: "string",
                ..DurableFixture::default()
            },
            "a member scalar-type change must drift the shape digest",
        ),
        (
            "identity-type",
            DurableFixture {
                identity_type: "string",
                // A string identity has no default allocation policy, so the helper
                // reads rather than allocates. The function is not a durable fact the
                // digest binds, so changing it alongside the identity type keeps the
                // edit single in the durable surface.
                func: "pub fn lookup(id: string): string\n\
                       \x20   return ^books(id).isbn ?? \"\"",
                ..DurableFixture::default()
            },
            "an identity-key scalar-type change must drift the shape digest",
        ),
        (
            "index-unique",
            DurableFixture {
                index_unique: false,
                ..DurableFixture::default()
            },
            "an index uniqueness flip must drift the shape digest",
        ),
        (
            "index-columns",
            DurableFixture {
                index_columns: "count, id",
                ..DurableFixture::default()
            },
            "an index key-columns change must drift the shape digest",
        ),
        (
            "keyed-group-arity",
            DurableFixture {
                versions_keys: "version: int, draft: int",
                ..DurableFixture::default()
            },
            "a keyed-group key arity change must drift the shape digest",
        ),
        (
            "keyed-group-type",
            DurableFixture {
                versions_keys: "version: string",
                ..DurableFixture::default()
            },
            "a keyed-group key scalar-type change must drift the shape digest",
        ),
        (
            "keyed-leaf-type",
            DurableFixture {
                tags_keys: "pos: string",
                ..DurableFixture::default()
            },
            "a top-level keyed-leaf key scalar-type change must drift the shape digest",
        ),
        (
            "optional-toggle",
            DurableFixture {
                count_required: false,
                ..DurableFixture::default()
            },
            "an optional->required toggle must drift the shape digest",
        ),
    ];

    for (name, fixture, message) in shape_cases {
        let digest = source_digest(&format!("durable-{name}"), &durable_fixture(fixture));
        assert_ne!(base_digest, digest, "{message}");
    }

    // The evolve decision surface does not change the shape, so the shape digest is
    // stable, but the evolution digest the witness records must drift.
    let base_evolution = evolution_digest("durable-evolution-base", &base);
    let evolve_cases: [(&str, DurableFixture, &str); 2] = [
        (
            "default-value",
            DurableFixture {
                default_value: "1",
                ..DurableFixture::default()
            },
            "an evolve default value change",
        ),
        (
            "transform-body",
            DurableFixture {
                transform_body: "return \"y\"",
                ..DurableFixture::default()
            },
            "an evolve transform body change",
        ),
    ];
    for (name, fixture, change) in evolve_cases {
        let source = durable_fixture(fixture);
        assert_eq!(
            base_digest,
            source_digest(&format!("durable-shape-{name}"), &source),
            "{change} must not drift the shape digest"
        );
        assert_ne!(
            base_evolution,
            evolution_digest(&format!("durable-evolution-{name}"), &source),
            "{change} must drift the evolution digest"
        );
    }

    // A module const that a transform reads is durable shape, not evolve decision
    // surface, so editing its value drifts the shape digest. The fixture cannot express
    // a module const, so this dimension is asserted directly: two sources that differ
    // only in the const value a transform returns.
    let const_one = "module books\n\
         const Scale = 1\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   transform Book.pages\n\
         \x20       return Scale\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let const_two = const_one.replace("const Scale = 1", "const Scale = 2");
    assert_ne!(
        source_digest("durable-const-one", const_one),
        source_digest("durable-const-two", &const_two),
        "a changed module const a transform reads must drift the shape digest"
    );

    // A pure whitespace and indentation reformat of the same declarations parses to
    // the same schema structure, so the digest is stable.
    let reformatted = marrow_syntax::format_source(&base);
    assert_ne!(reformatted, base, "the reformat must change layout");
    assert_eq!(
        base_digest,
        source_digest("durable-reformatted", &reformatted),
        "a pure reformat must not drift the digest"
    );
}

/// Resource-member order is durable shape: the store restamps under a fresh digest when members
/// are reordered, so the structural digest records each member's ordinal within its siblings and a
/// pure field reorder drifts it — exactly as the store's restamp-on-reorder contract requires, and
/// exactly as retyping or renaming a member does. The digest depends on structure, not formatter
/// text, so the reorder is detected without hashing rendered source.
#[test]
fn resource_member_reorder_drifts_shape_digest() {
    let base = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         \x20   author: string\n\
         store ^books(id: int): Book\n";
    let reordered = "module books\n\
         resource Book\n\
         \x20   author: string\n\
         \x20   pages: int\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n";
    assert_ne!(
        source_digest("member-reorder-base", base),
        source_digest("member-reorder-shuffled", reordered),
        "reordering resource members must drift the shape digest"
    );
}

#[test]
fn source_digest_excludes_surface_declarations() {
    let base = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n";
    let with_surface = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         surface Books from ^books\n\
         \x20   fields title\n\
         \x20   collection ^books as list\n";

    assert_eq!(
        source_digest("surface-digest-base", base),
        source_digest("surface-digest-with-surface", with_surface)
    );
}

/// The shape digest depends on the declared schema structure alone, not on the author's
/// source layout. A layout change — blank lines between and inside declarations, and wider
/// indentation — must leave the digest exactly where it was. This pins the activation fence
/// to the declared shape rather than to incidental whitespace, so reformatting a committed
/// source never reads as schema drift.
///
/// The messy source is hand-written so the input is genuinely non-canonical, exercising a
/// layout the formatter could not emit, yet it parses to the same structure as the baseline.
#[test]
fn source_layout_change_does_not_move_shape_digest() {
    let canonical = "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    // Extra blank lines between and inside declarations, plus eight-space indentation,
    // none of which is durable shape.
    let messy = "module books\n\
         \n\
         \n\
         resource Book\n\
         \n\
         \x20       required title: string\n\
         \n\
         \x20       pages: int\n\
         \n\
         store ^books(id: int): Book\n\
         \n\
         pub fn add(title: string): Id(^books)\n\
         \x20       return nextId(^books)\n";

    assert_eq!(
        source_digest("layout-canonical", canonical),
        source_digest("layout-messy", messy),
        "a formatter-internal layout change must not move the shape digest"
    );
}

/// An enum's members are durable shape. Adding, removing, or reordering a member drifts the shape
/// digest: a reorder mutates no stored data (a stored value is keyed by member identity, not
/// position) but is a tracked shape change the store restamps under a fresh digest, so the digest
/// must move for the restamp to happen. A pure layout reformat is not shape, so it must not move
/// the digest — proving the structural digest ignores formatter text while still tracking member
/// order.
#[test]
fn enum_member_shape_drifts_on_membership_and_reorder_but_not_layout() {
    let base = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         fn s(): bool\n\
         \x20   return true\n";
    let base_digest = source_digest("enum-base", base);

    let added_member = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         \x20   deleted\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-added", added_member),
        "adding an enum member must drift the shape digest"
    );

    let removed_member = "module books\n\
         enum Status\n\
         \x20   active\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-removed", removed_member),
        "removing an enum member must drift the shape digest"
    );

    let reordered = "module books\n\
         enum Status\n\
         \x20   archived\n\
         \x20   active\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-reordered", reordered),
        "reordering enum members is a tracked shape change the store restamps, so it must drift \
         the shape digest"
    );

    // Extra blank lines and eight-space indentation around the same members, none of
    // which is durable shape.
    let messy = "module books\n\
         \n\
         enum Status\n\
         \n\
         \x20       active\n\
         \n\
         \x20       archived\n\
         \n\
         fn s(): bool\n\
         \x20       return true\n";
    assert_eq!(
        base_digest,
        source_digest("enum-messy", messy),
        "an enum-member layout change must not move the shape digest"
    );
}

/// A frozen golden over a fixed canonical shape. The shape digest is stamped into every store
/// and enforced by the activation fence, so the structural encoding it hashes must not move
/// silently: a change to how a member signature, key shape, index shape, or const value is
/// serialized into the digest would move every committed snapshot's digest and read live stores
/// as schema drift. The other digest tests only compare digests within one run, so both sides
/// would shift together and hide such a change. This pins the exact value, so update the golden
/// only alongside an intentional change to the structural digest encoding.
#[test]
fn shape_digest_is_a_frozen_golden() {
    let source = "module books\n\
         const Limit: int = 10\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_eq!(
        source_digest("golden-shape", source),
        "sha256:39611069b28dcdac4f6306c0971844247adbff2db1103d803da89108b5fcb965",
        "the structural digest encoding moved; update the golden only with an intentional \
         encoding change"
    );
}

/// `ErrorCode` is a write-time value refinement, not a stored-shape difference: it shares
/// `string`'s storage, reads decode it as a plain `string`, and the evolution retype fence treats
/// the two as one type. Switching a field between `string` and `ErrorCode` — as a plain field or a
/// sequence element — must therefore leave the shape digest fixed, while a genuine storage-type
/// change still drifts it. This pins the deliberate exclusion (`ErrorCode` is the only scalar
/// spelling that resolves to a storage type another spelling already names), so an accidental
/// future change that encoded the refinement would be caught.
#[test]
fn error_code_refinement_does_not_drift_shape_digest() {
    let string_field = "module books\n\
         resource Entry\n\
         \x20   required code: string\n\
         \x20   note: string\n\
         store ^entries(id: int): Entry\n";
    let error_code_field =
        string_field.replace("required code: string", "required code: ErrorCode");
    assert_ne!(error_code_field, string_field);
    assert_eq!(
        source_digest("errorcode-string", string_field),
        source_digest("errorcode-refined", &error_code_field),
        "string<->ErrorCode shares one storage type and must not drift the shape digest"
    );

    let seq_string = "module books\n\
         resource Entry\n\
         \x20   required codes: sequence[string]\n\
         store ^entries(id: int): Entry\n";
    let seq_error_code = seq_string.replace("sequence[string]", "sequence[ErrorCode]");
    assert_ne!(seq_error_code, seq_string);
    assert_eq!(
        source_digest("errorcode-seq-string", seq_string),
        source_digest("errorcode-seq-refined", &seq_error_code),
        "sequence[string]<->sequence[ErrorCode] must not drift the shape digest"
    );

    // The exclusion is narrow: a real storage-type change still drifts.
    let retyped = string_field.replace("required code: string", "required code: int");
    assert_ne!(
        source_digest("errorcode-string", string_field),
        source_digest("errorcode-retyped", &retyped),
        "a real storage-type change must still drift the shape digest"
    );
}

/// A doc comment is prose, not durable shape: the catalog a stored snapshot must satisfy
/// is unchanged whether a `resource`, `store`, `enum`, `const`, field, member, or `fn`
/// carries documentation or not. So adding, editing, or removing a `;;` doc comment must
/// leave the shape digest exactly where it was, while a real shape edit — a field's type,
/// a const value — still drifts it. A `;` line comment above a declaration is not attached
/// to it and a pure whitespace reformat is layout, so neither moves the digest either.
#[test]
fn doc_comment_edit_does_not_drift_shape_digest() {
    let with_docs = "module books\n\
         ;; The catalog scale factor.\n\
         const Scale: int = 1\n\
         ;; A book's lifecycle status.\n\
         enum Status\n\
         \x20   ;; Currently for sale.\n\
         \x20   active\n\
         \x20   archived\n\
         ;; A book in the catalog.\n\
         resource Book\n\
         \x20   ;; The unique ISBN.\n\
         \x20   required isbn: string\n\
         \x20   pages: int\n\
         ;; Durable books, keyed by id.\n\
         store ^books(id: int): Book\n\
         ;; Allocate a fresh book id.\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let base_digest = source_digest("doc-with", with_docs);

    let without_docs = "module books\n\
         const Scale: int = 1\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Book\n\
         \x20   required isbn: string\n\
         \x20   pages: int\n\
         store ^books(id: int): Book\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    assert_eq!(
        base_digest,
        source_digest("doc-without", without_docs),
        "removing every doc comment must not drift the shape digest"
    );

    let edited_docs = with_docs
        .replace("The catalog scale factor.", "An entirely rewritten note.")
        .replace(
            "Currently for sale.",
            "Still on the shelves and orderable today.",
        )
        .replace("The unique ISBN.", "Reworded ISBN documentation prose.");
    assert_eq!(
        base_digest,
        source_digest("doc-edited", &edited_docs),
        "rewording doc comments must not drift the shape digest"
    );

    // A real shape edit at a documented declaration still drifts the digest.
    let retyped_field = without_docs.replace("pages: int", "pages: string");
    assert_ne!(
        base_digest,
        source_digest("doc-retyped-field", &retyped_field),
        "a field type change must still drift the shape digest"
    );
    let changed_const = without_docs.replace("const Scale: int = 1", "const Scale: int = 2");
    assert_ne!(
        base_digest,
        source_digest("doc-changed-const", &changed_const),
        "a const value change must still drift the shape digest"
    );

    // A `;` line comment above a declaration is not attached to it, and a pure reformat is
    // layout; neither is durable shape.
    let line_commented = without_docs.replace(
        "resource Book\n",
        "; an ordinary line comment\nresource Book\n",
    );
    assert_eq!(
        base_digest,
        source_digest("doc-line-comment", &line_commented),
        "a plain line comment must not drift the shape digest"
    );
    let reformatted = marrow_syntax::format_source(without_docs);
    assert_eq!(
        base_digest,
        source_digest("doc-reformatted", &reformatted),
        "a pure reformat must not drift the shape digest"
    );
}

/// Declaration and member identity is durable shape. Adding a member, and renaming any durable
/// entity — a resource, a store root, an enum, or a member — changes the set of stored identities,
/// so each drifts the shape digest: pre-1.0, stored data is addressed by declared path, so a rename
/// re-stamps rather than silently reads old data under the new name. A key-parameter rename is the
/// one rename that does not: a key parameter is named for readability but stored data is keyed by
/// the key's type, not its parameter name, so renaming it mutates no data and leaves the digest
/// fixed. Each case edits one declaration against a shared baseline, so a digest that still matched
/// the baseline would prove that identity is unbound.
#[test]
fn source_digest_binds_declaration_identity() {
    let base = "module books\n\
         enum Status\n\
         \x20   active\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   note: string\n\
         \x20   shelf(pos: int): string\n\
         store ^books(id: int): Book\n\
         \x20   index byTitle(title, id)\n";
    let base_digest = source_digest("identity-base", base);

    // Each edit keeps the program valid (references renamed alongside their targets), so any
    // digest drift is the identity change alone, not an introduced error.
    let moves: [(&str, String); 6] = [
        (
            "field-added",
            base.replace("note: string\n", "note: string\n    pages: int\n"),
        ),
        (
            "resource-renamed",
            base.replace("resource Book", "resource Tome")
                .replace(": Book", ": Tome"),
        ),
        ("store-root-renamed", base.replace("^books", "^library")),
        ("enum-renamed", base.replace("enum Status", "enum State")),
        (
            "member-renamed",
            base.replace("note: string", "memo: string"),
        ),
        ("index-renamed", base.replace("byTitle(", "byName(")),
    ];
    for (name, edited) in &moves {
        assert_ne!(edited, base, "case `{name}` must actually edit the source");
        assert_ne!(
            base_digest,
            source_digest(&format!("identity-{name}"), edited),
            "`{name}` changes a durable identity and must drift the shape digest"
        );
    }

    // Renaming a key parameter keeps its type, so no stored key changes.
    let key_param_renamed = base.replace("shelf(pos: int)", "shelf(slot: int)");
    assert_ne!(key_param_renamed, base);
    assert_eq!(
        base_digest,
        source_digest("identity-key-param-renamed", &key_param_renamed),
        "renaming a key parameter keeps its type, so it must not drift the shape digest"
    );
}

/// The shape digest hashes the canonical schema structure, never rendered schema text: the
/// formatter's `durable_shape_rendering` is gone, and the digest computation names no
/// declaration-text renderer. This keeps formatter text out of the saved-data trust chain, so a
/// formatter or layout change can never move a committed store's digest and read live data as
/// schema drift. If a later edit reintroduces text hashing into the digest, this fails loudly.
#[test]
fn source_digest_hashes_structure_not_formatter_text() {
    use std::fs;
    use std::path::PathBuf;

    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let format_rs = fs::read_to_string(crate_dir.join("../marrow-syntax/src/format.rs"))
        .expect("read marrow-syntax format.rs");
    assert!(
        !format_rs.contains("fn durable_shape_rendering"),
        "durable_shape_rendering must stay deleted: the durable digest hashes structure, not \
         rendered schema text"
    );

    let digest_rs = fs::read_to_string(crate_dir.join("src/catalog/source_digest.rs"))
        .expect("read source_digest.rs");
    for renderer in [
        "durable_shape_rendering",
        "format_declaration",
        "format_source",
    ] {
        assert!(
            !digest_rs.contains(renderer),
            "the durable digest path must not call `{renderer}`: it hashes schema structure, not \
             rendered declaration text"
        );
    }
}
