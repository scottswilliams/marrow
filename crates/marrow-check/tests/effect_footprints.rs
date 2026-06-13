mod support;

use std::path::Path;

use marrow_catalog::CatalogMetadata;
use marrow_check::{
    CheckedFunctionRef, EntryStoreOpenMode, WorkShapeClass, check_project,
    check_project_with_catalog,
};
use marrow_schema::ReturnPresence;

use support::{assert_clean, config, temp_project, write};

fn baseline_catalog(root: &Path) -> CatalogMetadata {
    let (report, program) = check_project(root, &config()).expect("baseline check");
    assert_clean(&report);
    program.catalog.proposal.expect("baseline catalog proposal")
}

fn function_ref(
    program: &marrow_check::CheckedProgram,
    module: &str,
    function: &str,
) -> CheckedFunctionRef {
    let module_id = program.facts.module_id(module).expect("module");
    let function_id = program
        .facts
        .function_id(module_id, function)
        .expect("function");
    let fact = program.facts.function(function_id);
    CheckedFunctionRef {
        module: fact.module.0,
        function: fact.source_index,
        presence: ReturnPresence::Always,
    }
}

#[test]
fn public_entry_closure_reaches_helper_saved_write() {
    let root = temp_project("effect-closure-helper-write", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title, id)\n\
             \x20   index byShelf(shelf, id)\n\
             fn writeShelf(id: int, shelf: string)\n\
             \x20   ^books(id).shelf = shelf\n\
             pub fn save(id: int, shelf: string)\n\
             \x20   writeShelf(id, shelf)\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let module = program.facts.module_id("books").expect("books module");
    let store = program
        .facts
        .store_id(module, "books")
        .expect("books store");
    let by_shelf = program
        .facts
        .store_indexes()
        .iter()
        .find(|index| index.name == "byShelf")
        .expect("byShelf index")
        .id;

    let save = function_ref(&program, "books", "save");
    let closure = program
        .effect_closure(save)
        .expect("save closure is available");
    assert!(closure.write_effects_reachable);
    assert_eq!(closure.stores_written, vec![store]);
    assert_eq!(closure.indexes_touched, vec![by_shelf]);

    let footprint = program
        .entry_footprints()
        .into_iter()
        .find(|footprint| footprint.entry == "books::save")
        .expect("public save entry footprint");
    assert!(footprint.write_effects_reachable);
    assert_eq!(footprint.stores_written, vec![store]);
    assert_eq!(footprint.work_shape, WorkShapeClass::WritesSavedData);
    assert_eq!(
        program.entry_store_open_mode(save),
        Some(EntryStoreOpenMode::WriteCapable)
    );
}

#[test]
fn read_only_public_entry_reports_static_footprint() {
    let root = temp_project("effect-closure-read-only", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn title(id: int): string\n\
             \x20   return ^books(id).title ?? \"\"\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let module = program.facts.module_id("books").expect("books module");
    let store = program
        .facts
        .store_id(module, "books")
        .expect("books store");
    let title = function_ref(&program, "books", "title");
    let closure = program
        .effect_closure(title)
        .expect("title closure is available");
    assert!(!closure.write_effects_reachable);
    assert_eq!(closure.stores_read, vec![store]);
    assert!(closure.stores_written.is_empty());

    let footprint = program
        .entry_footprints()
        .into_iter()
        .find(|footprint| footprint.entry == "books::title")
        .expect("public title entry footprint");
    assert!(!footprint.write_effects_reachable);
    assert_eq!(footprint.stores_read, vec![store]);
    assert!(footprint.stores_written.is_empty());
    assert_eq!(footprint.work_shape, WorkShapeClass::ReadOnly);
    assert_eq!(
        program.entry_store_open_mode(title),
        Some(EntryStoreOpenMode::WriteCapable),
        "a first-run program with no frozen catalog identity still needs a write-capable open"
    );
}

#[test]
fn entry_cost_shape_reports_counted_index_branch_as_one_range_scan() {
    let root = temp_project("entry-cost-shape-index-count", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   shelf: string\n\
             store ^books(id: int): Book\n\
             \x20   index byShelf(shelf, id)\n\
             pub fn shelfCount(shelf: string): int\n\
             \x20   return count(^books.byShelf(shelf))\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let shape = program
        .entry_cost_shapes()
        .into_iter()
        .find(|shape| shape.entry == "books::shelfCount")
        .expect("public shelfCount cost shape");
    assert_eq!(shape.work_shape, WorkShapeClass::ReadOnly);
    assert_eq!(shape.point_reads, 0);
    assert_eq!(shape.range_scans, 1);
    assert_eq!(shape.writes, 0);
    assert_eq!(shape.index_entry_touches, 0);
    assert_eq!(shape.commit_points, 0);
}

#[test]
fn entry_cost_shape_counts_distinct_static_shapes_not_expression_multiplicity() {
    let root = temp_project("entry-cost-shape-distinct-static-shapes", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn repeated(id: int): string\n\
             \x20   const a = ^books(id).title ?? \"\"\n\
             \x20   const b = ^books(id).title ?? \"\"\n\
             \x20   return a + b\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);

    let shape = program
        .entry_cost_shapes()
        .into_iter()
        .find(|shape| shape.entry == "books::repeated")
        .expect("public repeated cost shape");
    assert_eq!(shape.work_shape, WorkShapeClass::ReadOnly);
    assert_eq!(
        shape.point_reads, 1,
        "cost shape records one distinct saved member read, not two dynamic reads"
    );
    assert_eq!(shape.range_scans, 0);
    assert_eq!(shape.writes, 0);
    assert_eq!(shape.index_entry_touches, 0);
    assert_eq!(shape.commit_points, 0);
}

#[test]
fn transaction_wrapped_read_entry_requires_write_capable_open_after_identity_freeze() {
    let root = temp_project("effect-closure-transaction-read-open-mode", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn title(id: int): string\n\
             \x20   transaction\n\
             \x20       return ^books(id).title ?? \"\"\n",
        );
    });
    let baseline = baseline_catalog(&root);

    let (report, program) =
        check_project_with_catalog(&root, &config(), Some(&baseline)).expect("accepted check");
    assert_clean(&report);
    assert_eq!(program.catalog.accepted_epoch, Some(baseline.epoch));
    assert!(program.catalog.proposal.is_none());

    let title = function_ref(&program, "books", "title");
    let closure = program
        .effect_closure(title)
        .expect("title closure is available");
    assert!(!closure.write_effects_reachable);
    assert!(closure.transactions);
    assert_eq!(
        program.entry_store_open_mode(title),
        Some(EntryStoreOpenMode::WriteCapable),
        "runtime transaction evaluation begins a store transaction, which read-only handles reject"
    );
}

#[test]
fn pending_catalog_proposal_keeps_read_only_entry_write_capable() {
    let root = temp_project("effect-closure-pending-proposal-open-mode", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             pub fn title(id: int): string\n\
             \x20   return ^books(id).title ?? \"\"\n",
        );
    });
    let baseline = baseline_catalog(&root);
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\
         pub fn title(id: int): string\n\
         \x20   return ^books(id).title ?? \"\"\n",
    );

    let (report, program) =
        check_project_with_catalog(&root, &config(), Some(&baseline)).expect("accepted check");
    assert_clean(&report);
    assert_eq!(program.catalog.accepted_epoch, Some(baseline.epoch));
    assert!(
        program.catalog.proposal.is_some(),
        "source change should leave a pending proposal"
    );

    let title = function_ref(&program, "books", "title");
    let closure = program
        .effect_closure(title)
        .expect("title closure is available");
    assert!(!closure.write_effects_reachable);
    assert!(!closure.transactions);
    assert_eq!(
        program.entry_store_open_mode(title),
        Some(EntryStoreOpenMode::WriteCapable),
        "pending catalog activation can still require writes even for read-only closures"
    );
}

#[test]
fn closure_uses_source_index_specific_function_facts() {
    let root = temp_project("effect-closure-duplicate-source-index", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn dup(id: Id(^books)): string\n\
             \x20   return ^books(id).title ?? \"\"\n\
             fn dup(id: Id(^books), title: string)\n\
             \x20   ^books(id).title = title\n",
        );
    });
    let (report, program) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == marrow_check::CHECK_DUPLICATE_DECLARATION),
        "{:#?}",
        report.diagnostics
    );

    let writer = CheckedFunctionRef {
        module: 0,
        function: 1,
        presence: ReturnPresence::Always,
    };
    let closure = program
        .effect_closure(writer)
        .expect("second duplicate function has a fact");
    assert!(
        closure.write_effects_reachable,
        "closure must read direct effects through module+source_index, not the first fact by name"
    );
}
