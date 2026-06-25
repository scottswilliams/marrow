use crate::support;
use crate::support_discharge;
use marrow_catalog::CatalogEntryKind;
use marrow_check::evolution::{RepairReason, Verdict, preview};
use marrow_store::tree::TreeStore;
use marrow_store::value::Scalar;

use support::catalog::write_catalog;
use support::{temp_project, write};
use support_discharge::*;

/// A pure enum rename (`Status` -> `State`) is not a retype. The member keeps referencing
/// the same enum stable identity, so the identity-aware leaf token is unchanged across the
/// rename and a populated record discharges as a clean `DataProof`, never a false
/// `TypeChangeRequiresTransform`.
#[test]
fn enum_rename_is_not_a_retype() -> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let draft_member = hex_id(8);
    let shipped_member = hex_id(9);
    let root = temp_project("discharge-enum-rename", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum State\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book\n\
             \x20   required value: State\n\
             store ^books(id: int): Book\n\
             evolve\n\
             \x20   rename Status -> State\n\
             pub fn add(value: State): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        // The accepted catalog records the enum under the OLD spelling `Status` with the
        // stable id the rename preserves, and the member's accepted leaf token references
        // that same enum identity.
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &draft_member,
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::shipped",
                    &shipped_member,
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{enum_stable}"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    // The rename preserves the enum's stable id, so the bound enum id matches the accepted.
    assert_eq!(
        enum_catalog_id(&program, "State")?,
        enum_stable,
        "rename preserves the enum stable id"
    );
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Seed the stored `draft` value written under the prior member identity. The decisive
    // check is the leaf token: the enum's stable id is preserved across the rename, so this
    // is not a retype and the populated record proves cleanly.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&enum_stable, &draft_member));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let value_id = member_catalog_id(&place, "value")?;
    assert!(
        matches!(verdict_for(&result, &value_id), Verdict::DataProof),
        "a pure enum rename must not read as a retype: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// Redefining an enum under the same source spelling fails closed when a stored
/// member is no longer a member of the current enum. The enum keeps its stable
/// identity, so the leaf token is unchanged and this is not a retype, but the
/// stored value cannot be reread as the current enum.
#[test]
fn enum_member_removed_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let root = temp_project("discharge-enum-redefine", |root| {
        // Current source declares `Status` with only `draft`: `shipped` was removed.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             resource Book\n\
             \x20   required value: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(value: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{enum_stable}"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Seed a stored value whose member id is NOT a member of the current `Status`: a
    // removed `shipped`. A made-up member catalog id stands in for the retired member.
    let removed_member = hex_id(9);
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &value_id,
        enum_value_bytes(&enum_stable, &removed_member),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let value_id = member_catalog_id(&place, "value")?;
    assert_fails_closed(
        &result,
        &diagnostics,
        &value_id,
        RepairReason::InvalidStoredValue,
    );

    // The repair must be actionable: it names the affected record, says the stored value is no
    // longer accepted, and points at the way forward (an evolve transform and `marrow data get`),
    // not a bare `repair before activating`.
    let message = &diagnostics
        .iter()
        .find(|diagnostic| diagnostic.catalog_id.as_str() == value_id)
        .expect("a repair diagnostic for the drifted member")
        .message;
    assert!(
        message.contains("record(s) 1")
            && message.contains("evolve rename")
            && message.contains("evolve transform")
            && message.contains("marrow data get")
            && !message.contains("repair before activating"),
        "the enum-drift repair must name a record-preserving `evolve rename` and the transform fallback: {message}"
    );
    // The `data get` example is concrete and copy-pasteable: a real drifted record's saved path,
    // never the `<saved-path>` placeholder.
    assert!(
        message.contains("marrow data get <projectdir> ^books(1).value")
            && !message.contains("<saved-path>"),
        "the repair must name a concrete drifted saved path, not a placeholder: {message}"
    );

    Ok(())
}

/// A required enum leaf is presence- and decode-scanned exactly like a required scalar: a
/// record missing its enum cell fails closed.
#[test]
fn required_enum_leaf_missing_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-required-enum-missing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required state: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string, state: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so the enum and member ids are accepted, then exercise an old
    // snapshot that predates the required enum member.
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The record carries `title` but no `state` cell: the required enum is missing.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let state_id = member_catalog_id(&place, "state")?;
    assert_fails_closed(
        &result,
        &diagnostics,
        &state_id,
        RepairReason::MissingRequiredMember,
    );

    Ok(())
}

/// A REQUIRED identity leaf is presence- and decode-scanned like a required
/// scalar: a record missing its identity cell fails closed.
#[test]
fn required_identity_leaf_missing_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-required-identity-missing", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Author\n\
             \x20   required name: string\n\
             store ^authors(id: int): Author\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   required author: Id(^authors)\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string, author: Id(^authors)): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // The record carries `title` but no `author` cell: the required identity is missing.
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let author_id = member_catalog_id(&place, "author")?;
    assert_fails_closed(
        &result,
        &diagnostics,
        &author_id,
        RepairReason::MissingRequiredMember,
    );

    Ok(())
}

/// A present, valid required enum leaf discharges as a clean `DataProof`: the total scan
/// proves the cell present and decodable, and the stored member is a member of the current
/// enum.
#[test]
fn required_enum_leaf_present_proves_data() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-required-enum-present", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book\n\
             \x20   required state: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(state: Status): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    let state_id = member_catalog_id(&place, "state")?;
    let enum_id = enum_catalog_id(&program, "Status")?;
    let draft = enum_member_catalog_id(&program, "Status", "draft")?;
    seed.record(1);
    seed.member_bytes_by_id(1, &state_id, enum_value_bytes(&enum_id, &draft));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        matches!(verdict_for(&result, &state_id), Verdict::DataProof),
        "a present valid required enum leaf proves cleanly: {:#?}",
        result.verdicts
    );
    assert!(result.is_activatable(), "{result:#?}");
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// A retype from one enum to a DIFFERENT enum (`Status` -> `Kind`) is a real retype: the
/// identity-aware token differs (each names a distinct enum stable id), so a populated
/// record is steered to a transform. Identity awareness must not over-collapse: distinct
/// enums are distinct leaf types.
#[test]
fn retype_enum_a_to_enum_b_is_transform_required() -> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let status_stable = hex_id(7);
    let root = temp_project("discharge-retype-enum-enum", |root| {
        // Source now types `value: Kind`; the accepted catalog had it as enum `Status`.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Kind\n\
             \x20   alpha\n\
             \x20   beta\n\
             resource Book\n\
             \x20   required value: Kind\n\
             store ^books(id: int): Book\n\
             pub fn add(value: Kind): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            None,
            vec![
                entry(CatalogEntryKind::Enum, "books::Status", &status_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                member_entry(
                    "books::Book::value",
                    &value_id,
                    &format!("enum:{status_stable}"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // Seed a stored value of the OLD enum `Status`; its bytes must not be reread as `Kind`.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&status_stable, &hex_id(8)));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    let value_id = member_catalog_id(&place, "value")?;
    assert_retype_steered(&value_id, &result, &diagnostics);

    Ok(())
}

/// A stored enum value that names a member which has become a `category` (gained children,
/// so it is no longer selectable) fails closed: a category is unselectable, so a value naming
/// it is not a valid value of the current enum. The enum-member validity check must admit
/// only SELECTABLE members, not every catalog member, or a stored value of a now-grouping
/// member would be silently accepted.
#[test]
fn stored_enum_value_naming_now_category_member_fails_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-enum-now-category", |root| {
        // `tiger` was a selectable leaf when the value was written; source now gives it
        // children, making it a category and unselectable. A stored `tiger` is invalid.
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Cat\n\
             \x20   category tiger\n\
             \x20       bengal\n\
             \x20       siberian\n\
             \x20   housecat\n\
             resource Pet\n\
             \x20   required kind: Cat\n\
             store ^pets(id: int): Pet\n\
             pub fn add(): Id(^pets)\n\
             \x20   return nextId(^pets)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "pets")?;
    let kind_id = member_catalog_id(&place, "kind")?;
    let cat_enum_id = enum_catalog_id(&program, "Cat")?;
    let tiger_member_id = enum_member_catalog_id(&program, "Cat", "tiger")?;

    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // A record whose `kind` was stored as `Cat::tiger`, now a category.
    seed.record(1);
    seed.member_bytes_by_id(
        1,
        &kind_id,
        enum_value_bytes(&cat_enum_id, &tiger_member_id),
    );

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &kind_id,
        RepairReason::InvalidStoredValue,
    );

    Ok(())
}

/// An optional enum leaf whose enum dropped a selectable member fails closed when a stored
/// value names the removed member. An optional enum leaf is normally scanned only on a
/// retype, but here the enum keeps its stable identity (not a retype) while its
/// selectable-member set shrank this cycle, so every leaf referencing it must still be
/// presence- and validity-scanned.
#[test]
fn optional_enum_leaf_with_dropped_member_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let value_id = hex_id(3);
    let enum_stable = hex_id(7);
    let root = temp_project("discharge-optional-enum-dropped", |root| {
        // `Status` keeps its identity but drops selectable `shipped`; `state` is OPTIONAL.
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             resource Book\n\
             \x20   state: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
        let accepted = accepted_catalog(
            4,
            "books::Book",
            "books::^books",
            Some("int"),
            vec![
                entry(CatalogEntryKind::Enum, "books::Status", &enum_stable),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::draft",
                    &hex_id(8),
                ),
                entry(
                    CatalogEntryKind::EnumMember,
                    "books::Status::shipped",
                    &hex_id(9),
                ),
                member_entry(
                    "books::Book::state",
                    &value_id,
                    &format!("enum:{enum_stable}"),
                ),
            ],
        );
        write_catalog(root, &accepted);
    });
    let program = checked(&root).expect("checked fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    // A record whose optional `state` was stored as the now-removed `shipped`.
    seed.record(1);
    seed.member_bytes_by_id(1, &value_id, enum_value_bytes(&enum_stable, &hex_id(9)));

    let value_id = member_catalog_id(&place, "state")?;
    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &value_id,
        RepairReason::InvalidStoredValue,
    );

    Ok(())
}

/// Re-parenting a populated enum leaf under a fresh `category` (with no `evolve rename`)
/// is pending evolution work, not an auto-applicable change. The leaf's saved identity is
/// keyed on its full ancestor path, so `Pet::dog` -> `Pet::mammal::dog` mints a new member
/// identity and orphans the stored `Pet::dog`. The discharge must fail closed so a plain
/// `run` fences to `evolve apply` rather than silently orphaning the value.
#[test]
fn reparenting_a_populated_enum_leaf_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-enum-reparent", |root| {
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Pet\n\
             \x20   dog\n\
             \x20   cat\n\
             resource Animal\n\
             \x20   required kind: Pet\n\
             store ^animals(id: int): Animal\n\
             pub fn add(): Id(^animals)\n\
             \x20   return nextId(^animals)\n",
        );
    });
    // Commit the flat enum, seed a record holding `Pet::dog`, then re-parent `dog`/`cat`
    // under a new `category mammal` with no rename.
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "animals")?;
    let kind_id = member_catalog_id(&place, "kind")?;
    let pet_enum_id = enum_catalog_id(&program, "Pet")?;
    let dog_member_id = enum_member_catalog_id(&program, "Pet", "dog")?;

    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_bytes_by_id(1, &kind_id, enum_value_bytes(&pet_enum_id, &dog_member_id));

    write(
        &root,
        "src/zoo.mw",
        "module zoo\n\
         enum Pet\n\
         \x20   category mammal\n\
         \x20       dog\n\
         \x20       cat\n\
         resource Animal\n\
         \x20   required kind: Pet\n\
         store ^animals(id: int): Animal\n\
         pub fn add(): Id(^animals)\n\
         \x20   return nextId(^animals)\n",
    );
    let program = checked(&root).expect("checked fixture");

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert_fails_closed(
        &result,
        &diagnostics,
        &kind_id,
        RepairReason::InvalidStoredValue,
    );

    Ok(())
}

/// A populated re-parent WITH an explicit `evolve rename` carrying the leaf's identity forward
/// is how a developer activates a re-parent: the rename preserves the member's saved identity, so
/// the stored value stays decodable and the change proves cleanly. This is the path `evolve apply`
/// drives, the activation a plain `run` fences toward.
#[test]
fn reparenting_a_populated_enum_leaf_with_a_rename_proves() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_project("discharge-enum-reparent-rename", |root| {
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Pet\n\
             \x20   dog\n\
             \x20   cat\n\
             resource Animal\n\
             \x20   required kind: Pet\n\
             store ^animals(id: int): Animal\n\
             pub fn add(): Id(^animals)\n\
             \x20   return nextId(^animals)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "animals")?;
    let kind_id = member_catalog_id(&place, "kind")?;
    let pet_enum_id = enum_catalog_id(&program, "Pet")?;
    let dog_member_id = enum_member_catalog_id(&program, "Pet", "dog")?;

    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_bytes_by_id(1, &kind_id, enum_value_bytes(&pet_enum_id, &dog_member_id));

    // Re-parent both members under `mammal`, naming the carry-forward explicitly.
    write(
        &root,
        "src/zoo.mw",
        "module zoo\n\
         enum Pet\n\
         \x20   category mammal\n\
         \x20       dog\n\
         \x20       cat\n\
         resource Animal\n\
         \x20   required kind: Pet\n\
         store ^animals(id: int): Animal\n\
         evolve\n\
         \x20   rename Pet::dog -> Pet::mammal::dog\n\
         \x20   rename Pet::cat -> Pet::mammal::cat\n\
         pub fn add(): Id(^animals)\n\
         \x20   return nextId(^animals)\n",
    );
    let program = checked(&root).expect("checked fixture");

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        result.is_activatable(),
        "a re-parent with an identity-preserving rename must activate: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// Re-parenting an enum leaf over an EMPTY store is a free schema move: no stored value names
/// the old identity, so there is nothing to orphan and the change stays activatable (a plain
/// `run` auto-applies it). Only a populated re-parent is fenced.
#[test]
fn reparenting_an_enum_leaf_over_an_empty_store_proves() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-enum-reparent-empty", |root| {
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Pet\n\
             \x20   dog\n\
             \x20   cat\n\
             resource Animal\n\
             \x20   required kind: Pet\n\
             store ^animals(id: int): Animal\n\
             pub fn add(): Id(^animals)\n\
             \x20   return nextId(^animals)\n",
        );
    });
    let _program = commit_then_check(&root).expect("committed fixture");
    write(
        &root,
        "src/zoo.mw",
        "module zoo\n\
         enum Pet\n\
         \x20   category mammal\n\
         \x20       dog\n\
         \x20       cat\n\
         resource Animal\n\
         \x20   required kind: Pet\n\
         store ^animals(id: int): Animal\n\
         pub fn add(): Id(^animals)\n\
         \x20   return nextId(^animals)\n",
    );
    let program = checked(&root).expect("checked fixture");
    // An empty store: no record holds the old `Pet::dog` identity.
    let store = TreeStore::memory();

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        result.is_activatable(),
        "an empty-store re-parent must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// Reordering enum members keeps every member's full ancestor path, so each member's saved
/// identity is unchanged and a stored value stays decodable. A populated reorder proves
/// cleanly and a plain `run` auto-applies it.
#[test]
fn reordering_enum_members_over_populated_data_proves() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-enum-reorder", |root| {
        write(
            root,
            "src/zoo.mw",
            "module zoo\n\
             enum Pet\n\
             \x20   dog\n\
             \x20   cat\n\
             resource Animal\n\
             \x20   required kind: Pet\n\
             store ^animals(id: int): Animal\n\
             pub fn add(): Id(^animals)\n\
             \x20   return nextId(^animals)\n",
        );
    });
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "animals")?;
    let kind_id = member_catalog_id(&place, "kind")?;
    let pet_enum_id = enum_catalog_id(&program, "Pet")?;
    let dog_member_id = enum_member_catalog_id(&program, "Pet", "dog")?;

    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    seed.record(1);
    seed.member_bytes_by_id(1, &kind_id, enum_value_bytes(&pet_enum_id, &dog_member_id));

    // Swap the member order; each member keeps its `Pet::<name>` path and stable identity.
    write(
        &root,
        "src/zoo.mw",
        "module zoo\n\
         enum Pet\n\
         \x20   cat\n\
         \x20   dog\n\
         resource Animal\n\
         \x20   required kind: Pet\n\
         store ^animals(id: int): Animal\n\
         pub fn add(): Id(^animals)\n\
         \x20   return nextId(^animals)\n",
    );
    let program = checked(&root).expect("checked fixture");

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        result.is_activatable(),
        "an enum reorder over populated data must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}

/// An optional enum leaf whose enum is UNCHANGED proves cleanly over a stored value: the
/// shrank-enum trigger must not over-fire and force a scan (or a block) when no selectable
/// member was dropped. This pins that an honest optional enum stays a no-op.
#[test]
fn optional_enum_leaf_with_unchanged_enum_proves() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_project("discharge-optional-enum-unchanged", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             enum Status\n\
             \x20   draft\n\
             \x20   shipped\n\
             resource Book\n\
             \x20   state: Status\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the baseline so the enum and member ids are accepted, then re-preview the
    // unchanged enum over a populated optional leaf.
    let program = commit_then_check(&root).expect("committed fixture");
    let place = root_place(&program, "books")?;
    let store = TreeStore::memory();
    let seed = Seed::new(&store, &place);
    let state_id = member_catalog_id(&place, "state")?;
    let enum_id = enum_catalog_id(&program, "Status")?;
    let shipped = enum_member_catalog_id(&program, "Status", "shipped")?;
    seed.record(1);
    seed.member_bytes_by_id(1, &state_id, enum_value_bytes(&enum_id, &shipped));

    let (result, diagnostics) = preview(&program, &store).expect("preview");

    assert!(
        result.is_activatable(),
        "an unchanged optional enum must stay activatable: {:#?}",
        result.verdicts
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    Ok(())
}
