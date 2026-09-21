//! Managed-index reads at the source level.
//!
//! A nonunique index is scanned with a bounded `for` head that binds the source-root
//! `Id(^root)`; a unique index is looked up with a bracket access that yields the
//! optional `Id(^root)`. Both drive the whole production path — capture -> compile ->
//! verify -> attach -> VM — and compose with the entry-identity dereference: the bound
//! identity reads its entry through `^root[id]`.

use marrow_codes::Code;
use marrow_kernel::codec::key::KeyScalar;
use marrow_syntax::SourceSpan;
use marrow_verify::{LedgerIdBytes, RootId, SealedInstr, SealedSite, SealedSiteTarget};
use marrow_vm::Value;

use crate::common::{CallOutcome, Diagnostics, Project, Session};

// `^books[id: int]: Book` with a nonunique `byShelf[shelf, id]` and a unique
// `byIsbn[isbn]`. The index anchors live at `books.<index name>`.
const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.shelf 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id field Book.isbn 2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index books.byShelf 3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b\n\
     id index books.byIsbn 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Book {
    required title: string
    required shelf: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byShelf[shelf, id]
    index byIsbn[isbn] unique
}

pub fn shelve(id: int, title: string, shelf: string, isbn: string) {
    transaction {
        ^books[id] = Book(title: title, shelf: shelf, isbn: isbn)
    }
}

pub fn countOnShelf(shelf: string): int {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 100 {
        if const b = ^books[bookId] {
            count += 1
        }
    } on more {
        count = -1
    }
    return count
}

pub fn countOnShelfBounded(shelf: string): int {
    var count = 0
    for bookId in ^books.byShelf[shelf] at most 2 {
        if const b = ^books[bookId] {
            count += 1
        }
    } on more {
        return -1
    }
    return count
}

pub fn isbnPresent(isbn: string): bool {
    if const found = ^books.byIsbn[isbn] {
        return true
    }
    return false
}

pub fn titleByIsbn(isbn: string): string? {
    if const found = ^books.byIsbn[isbn] {
        return ^books[found].title
    }
    return absent
}

pub fn hasIsbn(isbn: string): bool {
    return exists(^books.byIsbn[isbn])
}
"#;

/// An ephemeral session over `source` against the ledger `ids`.
fn open(source: &str, ids: &str) -> Session {
    Project::single(source).ids(ids).session()
}

/// The diagnostics reported for the shared fixture extended by `body`.
fn compile_errors(body: &str) -> Diagnostics {
    source_errors(&format!("{SOURCE}\n{body}"), IDS)
}

/// The diagnostics a source the checker must reject reports.
fn source_errors(source: &str, ids: &str) -> Diagnostics {
    let Err(diagnostics) = Project::single(source).ids(ids).try_image() else {
        panic!("the checker must reject this program");
    };
    diagnostics
}

fn has_type_error(diagnostics: &Diagnostics) -> bool {
    diagnostics.has_code("check.type") || diagnostics.has_code("check.unsupported")
}

fn s(v: &str) -> Value {
    Value::Text(v.into())
}

fn seed(session: &mut Session) {
    // Two books on shelf "A", one on "B"; distinct isbns.
    session.call("shelve", vec![Value::Int(1), s("dune"), s("A"), s("i1")]);
    session.call(
        "shelve",
        vec![Value::Int(2), s("hyperion"), s("A"), s("i2")],
    );
    session.call(
        "shelve",
        vec![Value::Int(3), s("neuromancer"), s("B"), s("i3")],
    );
}

#[test]
fn a_nonunique_scan_binds_the_identity_and_dereferences_it() {
    let mut session = open(SOURCE, IDS);
    seed(&mut session);

    assert_eq!(
        session.call("countOnShelf", vec![s("A")]),
        Some(Value::Int(2))
    );
    assert_eq!(
        session.call("countOnShelf", vec![s("B")]),
        Some(Value::Int(1))
    );
    assert_eq!(
        session.call("countOnShelf", vec![s("Z")]),
        Some(Value::Int(0))
    );
}

#[test]
fn a_bounded_scan_is_exact_and_fan_out_independent() {
    // The precise O(distinct + 1) seek cost is owned by the kernel `index_read` fixtures;
    // these assert the source-observable consequences the compiler's lowering delivers: a
    // bounded scan freezes exactly `at most N` identities and reports `on more`, and one
    // shelf's scan is isolated from another shelf's fan-out.
    let mut session = open(SOURCE, IDS);
    // Shelf "A" holds three books, shelf "B" one — `at most 2` on "A" hits the bound and
    // runs `on more`; on "B" it does not.
    session.call("shelve", vec![Value::Int(1), s("a"), s("A"), s("i1")]);
    session.call("shelve", vec![Value::Int(2), s("b"), s("A"), s("i2")]);
    session.call("shelve", vec![Value::Int(3), s("c"), s("A"), s("i3")]);
    session.call("shelve", vec![Value::Int(4), s("d"), s("B"), s("i4")]);

    assert_eq!(
        session.call("countOnShelfBounded", vec![s("A")]),
        Some(Value::Int(-1))
    );
    assert_eq!(
        session.call("countOnShelfBounded", vec![s("B")]),
        Some(Value::Int(1))
    );
    // Isolation: the full (unbounded) scan of each shelf sees only that shelf's rows,
    // independent of the other shelf's fan-out.
    assert_eq!(
        session.call("countOnShelf", vec![s("A")]),
        Some(Value::Int(3))
    );
    assert_eq!(
        session.call("countOnShelf", vec![s("B")]),
        Some(Value::Int(1))
    );
}

#[test]
fn a_unique_lookup_is_present_or_absent() {
    let mut session = open(SOURCE, IDS);
    seed(&mut session);

    assert_eq!(
        session.call("isbnPresent", vec![s("i2")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("isbnPresent", vec![s("missing")]),
        Some(Value::Bool(false))
    );
}

#[test]
fn a_unique_lookup_dereferences_the_found_identity() {
    let mut session = open(SOURCE, IDS);
    seed(&mut session);

    assert_eq!(
        session.call("titleByIsbn", vec![s("i2")]),
        Some(Value::Optional(Some(Box::new(s("hyperion"))))),
    );
    assert_eq!(
        session.call("titleByIsbn", vec![s("missing")]),
        Some(Value::Optional(None)),
    );
}

#[test]
fn an_exists_over_a_unique_index_is_present_or_absent() {
    // `exists(^root.uidx[keys])` completes the presence family over a unique index: the
    // same complete-key probe as the `if const` lookup, yielding a bare bool without
    // materializing the found identity.
    let mut session = open(SOURCE, IDS);
    seed(&mut session);

    assert_eq!(
        session.call("hasIsbn", vec![s("i2")]),
        Some(Value::Bool(true))
    );
    assert_eq!(
        session.call("hasIsbn", vec![s("missing")]),
        Some(Value::Bool(false))
    );
}

// --- Adversarial rejections: unsupported index-read forms. ---

#[test]
fn an_exists_over_a_nonunique_index_is_rejected() {
    // `exists` over a nonunique index has no complete-key probe — a nonunique index is
    // scan-only — so naming one as an `exists` argument is refused, exactly as reading
    // it as a value is.
    let diagnostics = compile_errors(
        "pub fn bad(shelf: string): bool {\n    return exists(^books.byShelf[shelf])\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

#[test]
fn scanning_a_unique_index_is_rejected() {
    let diagnostics = compile_errors(
        "pub fn bad(isbn: string): int {\n    var n = 0\n    for x in ^books.byIsbn[isbn] at most 10 {\n        n += 1\n    } on more {\n        n = -1\n    }\n    return n\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

#[test]
fn reading_a_nonunique_index_as_a_value_is_rejected() {
    let diagnostics = compile_errors(
        "pub fn bad(shelf: string): bool {\n    if const found = ^books.byShelf[shelf] {\n        return true\n    }\n    return false\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

#[test]
fn a_unique_lookup_with_the_wrong_arity_is_rejected() {
    let diagnostics = compile_errors(
        "pub fn bad(a: string, b: string): bool {\n    if const found = ^books.byIsbn[a, b] {\n        return true\n    }\n    return false\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

#[test]
fn a_from_cursor_on_a_scan_is_rejected() {
    let diagnostics = compile_errors(
        "pub fn bad(shelf: string): int {\n    var n = 0\n    for x in ^books.byShelf[shelf] at most 10 from 1 {\n        n += 1\n    } on more {\n        n = -1\n    }\n    return n\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

#[test]
fn a_two_binding_index_scan_is_rejected() {
    let diagnostics = compile_errors(
        "pub fn bad(shelf: string): int {\n    var n = 0\n    for x, p in ^books.byShelf[shelf] at most 10 {\n        n += 1\n    } on more {\n        n = -1\n    }\n    return n\n}\n",
    );
    assert!(has_type_error(&diagnostics));
}

const KEY_ONLY_SOURCE: &str = r#"resource Item {
    note: string
}

store ^items[id: int]: Item {
    index byId[id] unique
    index all[id]
}

pub fn put(id: int) {
    transaction {
        ^items[id] = Item()
    }
}

pub fn erase(id: int) {
    transaction {
        delete ^items[id]
    }
}

pub fn present(id: int): bool {
    return exists(^items[id])
}

pub fn find(id: int): Id(^items)? {
    return ^items.byId[id]
}

pub fn indexed(id: int): bool {
    return exists(^items.byId[id])
}

"#;

const KEY_ONLY_IDS: &str = "marrow ids v0\n\
machine-written by marrow; do not edit\n\
id application . 01010101010101010101010101010101\n\
id product Item 10101010101010101010101010101010\n\
id field Item.note 11111111111111111111111111111111\n\
id root items 20202020202020202020202020202020\n\
id key items.id 21212121212121212121212121212121\n\
id index items.byId 22222222222222222222222222222222\n\
id index items.all 23232323232323232323232323232323\n\
high-water 0\n\
end\n";

fn key_only_lifetime(source: &str, ids: &str) {
    let mut session = open(source, ids);
    let identity = Value::Optional(Some(Box::new(Value::Id(0, [KeyScalar::Int(7)].into()))));
    let absent = (
        Some(Value::Bool(false)),
        Some(Value::Optional(None)),
        Some(Value::Bool(false)),
    );
    let present = (
        Some(Value::Bool(true)),
        Some(identity),
        Some(Value::Bool(true)),
    );
    let observe = |session: &mut Session| {
        (
            session.call("present", vec![Value::Int(7)]),
            session.call("find", vec![Value::Int(7)]),
            session.call("indexed", vec![Value::Int(7)]),
        )
    };
    assert_eq!(observe(&mut session), absent);
    session.call("put", vec![Value::Int(7)]);
    assert_eq!(observe(&mut session), present);
    session.call("put", vec![Value::Int(7)]);
    assert_eq!(observe(&mut session), present);
    session.call("erase", vec![Value::Int(7)]);
    assert_eq!(observe(&mut session), absent);
    session.call("put", vec![Value::Int(7)]);
    assert_eq!(observe(&mut session), present);
}

#[test]
fn key_only_indexes_follow_all_sparse_entry_lifetime() {
    key_only_lifetime(KEY_ONLY_SOURCE, KEY_ONLY_IDS);
}

#[test]
fn key_only_indexes_follow_empty_entry_lifetime() {
    let source = KEY_ONLY_SOURCE.replace("    note: string\n", "");
    let ids = KEY_ONLY_IDS.replace("id field Item.note 11111111111111111111111111111111\n", "");
    key_only_lifetime(&source, &ids);
}

const KEY_ONLY_SCAN: &str = r#"pub fn scanOrder(): int {
    var order = 0
    for itemId in ^items.all at most 2 {
        if const entry = ^items[itemId] {
            if itemId == Id(^items, 1) {
                order = order * 10 + 1
            } else if itemId == Id(^items, 2) {
                order = order * 10 + 2
            } else if itemId == Id(^items, 3) {
                order = order * 10 + 3
            } else {
                unreachable("unexpected identity")
            }
        } else {
            unreachable("missing entry")
        }
    } on more {
        order += 1000
    }
    return order
}
"#;

fn key_only_scan(source: &str, ids: &str) {
    let mut session = open(&format!("{source}{KEY_ONLY_SCAN}"), ids);
    let image = session.image();
    let instrs = image
        .exports()
        .iter()
        .find_map(|export| {
            let function = image
                .function(export.function())
                .expect("verified function");
            (function.body().name() == "scanOrder").then_some(function)
        })
        .expect("the scanOrder export")
        .body()
        .instrs();
    let mut scans = instrs.iter().filter_map(|instr| match instr {
        SealedInstr::DurIndexScan {
            site, limit, from, ..
        } => Some((*site, *limit, *from)),
        _ => None,
    });
    let (site, limit, from) = scans.next().expect("scanOrder executes an index scan");
    assert!(scans.next().is_none(), "one frozen scan in scanOrder");
    assert_eq!((limit, from), (2, false));
    assert!(
        !instrs
            .iter()
            .any(|instr| matches!(instr, SealedInstr::DurIterateBounded { .. }))
    );
    let SealedSite::Flat {
        root,
        target: SealedSiteTarget::IndexScan(index),
    } = &image.sites()[usize::from(site)]
    else {
        panic!("the executed scan must name an executable index site");
    };
    assert_eq!(*root, RootId::from_index(0));
    let index = &image.indexes()[usize::from(*index)];
    assert_eq!(index.id(), LedgerIdBytes::from_bytes([0x23; 16]));
    assert_eq!(index.root(), *root);
    assert!(!index.unique());

    assert_eq!(session.call("scanOrder", vec![]), Some(Value::Int(0)));
    for id in [2, 1] {
        session.call("put", vec![Value::Int(id)]);
    }
    assert_eq!(session.call("scanOrder", vec![]), Some(Value::Int(12)));
    session.call("put", vec![Value::Int(3)]);
    assert_eq!(session.call("scanOrder", vec![]), Some(Value::Int(1012)));
    session.call("erase", vec![Value::Int(1)]);
    assert_eq!(session.call("scanOrder", vec![]), Some(Value::Int(23)));
    for id in [2, 3] {
        session.call("erase", vec![Value::Int(id)]);
    }
    assert_eq!(session.call("scanOrder", vec![]), Some(Value::Int(0)));
}

#[test]
fn bare_index_scan_orders_and_bounds_all_sparse_entries() {
    key_only_scan(KEY_ONLY_SOURCE, KEY_ONLY_IDS);
}

#[test]
fn bare_index_scan_orders_and_bounds_empty_entries() {
    let source = KEY_ONLY_SOURCE.replace("    note: string\n", "");
    let ids = KEY_ONLY_IDS.replace("id field Item.note 11111111111111111111111111111111\n", "");
    key_only_scan(&source, &ids);
}

fn diagnostic_span(source: &str, body: &str, marked: &str, width: usize) -> SourceSpan {
    let start = body
        .find(marked)
        .expect("the case marks its offending construct");
    let before = format!("{source}\n{}", &body[..start]);
    SourceSpan {
        start_byte: before.len(),
        end_byte: before.len() + width,
        line: u32::try_from(before.bytes().filter(|byte| *byte == b'\n').count() + 1)
            .expect("small source fixture"),
        column: u32::try_from(before.rsplit('\n').next().expect("source line").len() + 1)
            .expect("small source fixture"),
    }
}

#[test]
fn bare_index_read_diagnostics_preserve_each_consumer_boundary() {
    let controls = format!(
        r#"{SOURCE}
pub fn ordinaryField(): string? {{
    return ^books[1].title
}}

pub fn localField(): string {{
    const book = Book(title: "local", shelf: "L", isbn: "local")
    return book.title
}}
"#
    );
    let mut session = open(&controls, IDS);
    assert_eq!(
        session.call("ordinaryField", vec![]),
        Some(Value::Optional(None))
    );
    assert_eq!(session.call("localField", vec![]), Some(s("local")));
    seed(&mut session);
    assert_eq!(
        session.call("ordinaryField", vec![]),
        Some(Value::Optional(Some(Box::new(s("dune")))))
    );

    let cases = [
        (
            "missing mixed prefix",
            "pub fn bad() { for x in ^books.byShelf at most 2 {} on more {} }\n",
            Code::CheckType,
            "for x in ^books.byShelf at most 2 {} on more {}",
            "for x in ^books.byShelf at most 2 {} on more {}".len(),
        ),
        (
            "wrong mixed prefix scalar",
            "pub fn bad() { for x in ^books.byShelf[1] at most 2 {} on more {} }\n",
            Code::CheckType,
            "1",
            1,
        ),
        (
            "bare unique value",
            "pub fn bad(): Id(^books)? { return ^books.byIsbn }\n",
            Code::CheckType,
            "^books.byIsbn",
            "^books.byIsbn".len(),
        ),
        (
            "bare unique exists",
            "pub fn bad(): bool { return exists(^books.byIsbn) }\n",
            Code::CheckType,
            "^books.byIsbn",
            "^books.byIsbn".len(),
        ),
        (
            "bare unique for",
            "pub fn bad() { for x in ^books.byIsbn at most 2 {} on more {} }\n",
            Code::CheckType,
            "for x in ^books.byIsbn at most 2 {} on more {}",
            "for x in ^books.byIsbn at most 2 {} on more {}".len(),
        ),
        (
            "bare nonunique value",
            "pub fn bad(): Id(^books)? { return ^books.byShelf }\n",
            Code::CheckType,
            "^books.byShelf",
            "^books.byShelf".len(),
        ),
        (
            "bare nonunique exists",
            "pub fn bad(): bool { return exists(^books.byShelf) }\n",
            Code::CheckType,
            "^books.byShelf",
            "^books.byShelf".len(),
        ),
        (
            "unknown root member",
            "pub fn bad() { for x in ^books.missing at most 2 {} on more {} }\n",
            Code::CheckType,
            "for x in ^books.missing at most 2 {} on more {}",
            "for x in ^books.missing at most 2 {} on more {}".len(),
        ),
        (
            "unkeyed field is not an index",
            "pub fn bad() { for x in ^books.title at most 2 {} on more {} }\n",
            Code::CheckType,
            "for x in ^books.title at most 2 {} on more {}",
            "for x in ^books.title at most 2 {} on more {}".len(),
        ),
    ];
    let actual: Vec<_> = cases
        .iter()
        .map(|(name, body, ..)| {
            let diagnostics = compile_errors(body)
                .iter()
                .map(|diagnostic| (diagnostic.code().as_str().to_string(), diagnostic.span()))
                .collect::<Vec<_>>();
            (*name, diagnostics)
        })
        .collect();
    let expected: Vec<_> = cases
        .iter()
        .map(|(name, body, code, marked, width)| {
            (
                *name,
                vec![(
                    code.as_str().to_string(),
                    diagnostic_span(SOURCE, body, marked, *width),
                )],
            )
        })
        .collect();
    assert_eq!(actual, expected);

    let empty = "pub fn bad() { const x = ^books.byShelf[] }\n";
    let mut insertion = diagnostic_span(SOURCE, empty, "]", 0);
    // The parser reports the insertion byte at `]` and the opening bracket's column.
    insertion.column -= 1;
    let diagnostics = compile_errors(empty);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = diagnostics.only("parse.syntax");
    assert_eq!(diagnostic.code(), Code::ParseSyntax);
    assert_eq!(diagnostic.span(), insertion);

    let source = SUBSET_SOURCE.replace(
        "index byTenant[tenant] unique",
        "index byTenant[tenant] unique\n    index all[tenant, slot]",
    );
    let ids = SUBSET_IDS.replace(
        "high-water 0",
        "id index slots.all 34343434343434343434343434343434\nhigh-water 0",
    );
    let scan = "for x in ^slots.all at most 2 {} on more {}";
    let body = format!("pub fn bad() {{ {scan} }}\n");
    let diagnostics = source_errors(&format!("{source}\n{body}"), &ids);
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = diagnostics.only("check.unsupported");
    assert_eq!(diagnostic.code(), Code::CheckUnsupported);
    assert_eq!(
        diagnostic.span(),
        diagnostic_span(&source, &body, scan, scan.len())
    );
}

const SUBSET_SOURCE: &str = r#"resource Item {
    note: string
}

resource Slot {
    note: string
}

store ^items[id: int]: Item {
    index byId[id] unique
}

store ^slots[tenant: string, slot: int]: Slot {
    index byTenant[tenant] unique
}

pub fn putItem(id: int) {
    transaction {
        ^items[id] = Item(note: "present")
    }
}

pub fn findItem(id: int): Id(^items)? {
    return ^items.byId[id]
}

pub fn itemPresent(id: int): bool {
    return exists(^items[id])
}

pub fn collide(): int {
    transaction {
        ^items[99] = Item(note: "rollback witness")
        ^slots["tenant", 1] = Slot()
        ^slots["tenant", 2] = Slot()
    }
    return 1
}

pub fn slotPresent(slot: int): bool {
    return exists(^slots["tenant", slot])
}

pub fn findTenant(): Id(^slots)? {
    return ^slots.byTenant["tenant"]
}
"#;

const SUBSET_IDS: &str = "marrow ids v0\n\
machine-written by marrow; do not edit\n\
id application . 01010101010101010101010101010101\n\
id product Item 10101010101010101010101010101010\n\
id field Item.note 11111111111111111111111111111111\n\
id product Slot 12121212121212121212121212121212\n\
id field Slot.note 13131313131313131313131313131313\n\
id root items 20202020202020202020202020202020\n\
id key items.id 21212121212121212121212121212121\n\
id index items.byId 22222222222222222222222222222222\n\
id root slots 30303030303030303030303030303030\n\
id key slots.tenant 31313131313131313131313131313131\n\
id key slots.slot 32323232323232323232323232323232\n\
id index slots.byTenant 33333333333333333333333333333333\n\
high-water 0\n\
end\n";

#[test]
fn key_only_unique_subset_collision_rolls_back_the_complete_transaction() {
    let mut session = open(SUBSET_SOURCE, SUBSET_IDS);
    // The fault names the colliding write, not the export or the transaction boundary.
    assert_eq!(
        session.try_call("collide", vec![]),
        CallOutcome::Fault {
            code: Code::RunUniqueIndex,
            line: 35,
            column: 9,
        },
    );
    assert_eq!(
        session.call("itemPresent", vec![Value::Int(99)]),
        Some(Value::Bool(false))
    );
    assert_eq!(
        session.call("findItem", vec![Value::Int(99)]),
        Some(Value::Optional(None))
    );
    for slot in [1, 2] {
        assert_eq!(
            session.call("slotPresent", vec![Value::Int(slot)]),
            Some(Value::Bool(false))
        );
    }
    assert_eq!(
        session.call("findTenant", vec![]),
        Some(Value::Optional(None))
    );
    session.call("putItem", vec![Value::Int(7)]);
    assert_eq!(
        session.call("findItem", vec![Value::Int(7)]),
        Some(Value::Optional(Some(Box::new(Value::Id(
            0,
            [KeyScalar::Int(7)].into()
        )))))
    );
}
