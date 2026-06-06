mod support;
mod support_discharge;

use marrow_store::tree::TreeStore;

use support::{temp_project, write};
use support_discharge::*;

/// The shape and evolution digests of a single-file source, computed against the
/// source's own accepted catalog so its evolve default and transform targets bind to
/// real stable ids. The shape digest is the one the store stamps and the fence
/// enforces; the evolution digest is the one the witness records.
fn digests(name: &str, source: &str) -> (String, String) {
    let root = temp_project(name, |root| write(root, "src/books.mw", source));
    let program = commit_then_check(&root);
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
/// keyed-leaf map, and a unique index; the evolve block defaults the scalar member and
/// transforms `isbn`, so one fixture exercises every digest dimension.
fn durable_fixture(f: DurableFixture) -> String {
    let count_required = if f.count_required { "required " } else { "" };
    let index_unique = if f.index_unique { " unique" } else { "" };
    format!(
        "module books\n\
         resource Book at ^books(id: {identity})\n\
         \x20   {count_required}count: {count_type}\n\
         \x20   pages: int\n\
         \x20   required isbn: string\n\
         \x20   tags({tags}): string\n\
         \x20   versions({versions})\n\
         \x20       required body: string\n\
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

/// The store-stamp shape digest binds the durable shape, not the transient evolve
/// block: editing only the evolve decision surface (a default value, a transform body)
/// leaves the shape digest unchanged, so a consumed block is deletable without reading
/// as schema drift. A change that touches the shape — a module const a transform reads,
/// an optional/required toggle — drifts it, because the store must satisfy that shape.
#[test]
fn shape_digest_binds_shape_and_not_the_evolve_block() {
    let base = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_default = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 1\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_transform = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"y\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let required_pages = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   default Book.pages = 0\n\
         \x20   transform Book.title\n\
         \x20       return \"x\"\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";

    let base_digest = source_digest("shape-base", base);
    assert_eq!(
        base_digest,
        source_digest("shape-default", changed_default),
        "a changed evolve default value must not drift the shape digest"
    );
    assert_eq!(
        base_digest,
        source_digest("shape-transform", changed_transform),
        "a changed transform body must not drift the shape digest"
    );
    let const_transform = "module books\n\
         const Scale = 1\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   transform Book.pages\n\
         \x20       return Scale\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    let changed_const = "module books\n\
         const Scale = 2\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   required pages: int\n\
         evolve\n\
         \x20   transform Book.pages\n\
         \x20       return Scale\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    assert_ne!(
        source_digest("shape-const-base", const_transform),
        source_digest("shape-const", changed_const),
        "a changed module const is part of the shape and must drift the shape digest"
    );
    assert_ne!(
        base_digest,
        source_digest("shape-required", required_pages),
        "an optional->required toggle must drift the shape digest"
    );

    // The witness evolution digest, in contrast, binds the evolve decision surface, so a
    // changed default and a changed transform body each drift it. This is what keeps
    // apply fencing a transform-body edit between preview and apply.
    let base_evolution = evolution_digest("evolution-base", base);
    assert_ne!(
        base_evolution,
        evolution_digest("evolution-default", changed_default),
        "a changed default value must drift the evolution digest"
    );
    assert_ne!(
        base_evolution,
        evolution_digest("evolution-transform", changed_transform),
        "a changed transform body must drift the evolution digest"
    );
}

/// The shape digest binds the whole durable shape, with no enumeration gap. It is
/// computed from the canonical normalized rendering of every shape declaration, so any
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
                       \x20   return ^books(id).isbn",
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

    // A pure whitespace and indentation reformat of the same declarations parses to
    // the same syntax tree, so the normalized rendering — and the digest — is stable.
    let reformatted = marrow_syntax::format_source(&base);
    assert_ne!(reformatted, base, "the reformat must change layout");
    assert_eq!(
        base_digest,
        source_digest("durable-reformatted", &reformatted),
        "a pure reformat must not drift the digest"
    );
}

/// The shape digest is derived by re-formatting each declaration through the frozen
/// normalized formatter, so it must depend on the declared shape alone, not on the
/// author's source layout. A formatter-internal layout change — blank lines between and
/// inside declarations, and wider indentation, all of which the normalized formatter
/// collapses — must leave the digest exactly where it was. This pins the activation
/// fence to the declared shape rather than to incidental whitespace, so reformatting a
/// committed source never reads as schema drift.
///
/// The messy source is hand-written rather than produced by the formatter so the input
/// is genuinely non-canonical: the formatter could not emit it, and only the normalized
/// rendering brings it back to the canonical baseline.
#[test]
fn formatter_internal_layout_change_does_not_move_shape_digest() {
    let canonical = "module books\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n";
    // Extra blank lines between and inside declarations, plus eight-space indentation,
    // none of which the normalized formatter preserves.
    let messy = "module books\n\
         \n\
         \n\
         resource Book at ^books(id: int)\n\
         \n\
         \x20       required title: string\n\
         \n\
         \x20       pages: int\n\
         \n\
         \n\
         pub fn add(title: string): Id(^books)\n\
         \x20       return nextId(^books)\n";

    assert_eq!(
        source_digest("layout-canonical", canonical),
        source_digest("layout-messy", messy),
        "a formatter-internal layout change must not move the shape digest"
    );
}

/// An enum's members are durable shape: each is a catalog entry a stored snapshot binds.
/// Adding, removing, or reordering a member drifts the shape digest, because the stored
/// shape no longer matches. A pure layout reformat of the same members — blank lines
/// between them and wider indentation, all of which the normalized formatter collapses —
/// must leave the digest exactly where it was. This proves the frozen-anchor claim for
/// enum members directly, not just for resource declarations.
#[test]
fn enum_member_shape_drifts_digest_but_layout_does_not() {
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

    let reordered = "module books\n\
         enum Status\n\
         \x20   archived\n\
         \x20   active\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_ne!(
        base_digest,
        source_digest("enum-reordered", reordered),
        "reordering enum members must drift the shape digest"
    );

    // Extra blank lines and eight-space indentation around the same members, none of
    // which the normalized formatter preserves.
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

/// A frozen golden over a fixed canonical shape. The shape digest is stamped into every
/// store and enforced by the activation fence, so the canonical rendering it hashes must
/// not move silently: a formatter change that altered the normalized text of an unchanged
/// shape — different indentation, blank-line policy, or token spacing — would move every
/// committed snapshot's digest and read live stores as schema drift. The other digest
/// tests only compare digests within one run, so both sides would shift together and hide
/// such a change. This pins the exact value, so update the golden only alongside
/// an intentional change to the durable rendering.
#[test]
fn shape_digest_is_a_frozen_golden() {
    let source = "module books\n\
         enum Status\n\
         \x20   active\n\
         \x20   archived\n\
         resource Book at ^books(id: int)\n\
         \x20   required title: string\n\
         \x20   pages: int\n\
         fn s(): bool\n\
         \x20   return true\n";
    assert_eq!(
        source_digest("golden-shape", source),
        "sha256:531be928b3fe8d46135633888c6ec346e4cb219928a57777cb60bc16d9d88eb9",
        "the canonical shape rendering moved; update the golden only with an intentional \
         durable-rendering change"
    );
}
