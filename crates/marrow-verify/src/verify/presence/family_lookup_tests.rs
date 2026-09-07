use super::{EntryFamilies, entry_family};
use crate::sealed::{SealedSite, SealedSiteTarget};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, DraftTxn, ExportId, FieldDef, FuncId,
    FunctionDef, ImageDraft, ImageType, Instr, KeyColumn, LedgerIdBytes, RecordTypeDef,
    RootOccurrenceDef, Scalar, SemanticPath, SemanticTarget, SpanEntry,
};
use std::cell::Cell;

#[path = "../../../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
#[path = "../../../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;
use admitted_plan::admitted_plan;
use site_seam::site;

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    builds: usize,
    entry_rows: usize,
    capacity: usize,
    lookups: usize,
    comparisons: usize,
}

thread_local! {
    static COUNTS: Cell<Option<Counts>> = const { Cell::new(None) };
}

pub(super) fn record_build(entry_rows: usize, capacity: usize) {
    COUNTS.with(|slot| {
        if let Some(mut counts) = slot.get() {
            counts.builds += 1;
            counts.entry_rows += entry_rows;
            counts.capacity = counts.capacity.max(capacity);
            slot.set(Some(counts));
        }
    });
}

pub(super) fn record_lookup() {
    COUNTS.with(|slot| {
        if let Some(mut counts) = slot.get() {
            counts.lookups += 1;
            slot.set(Some(counts));
        }
    });
}

pub(super) fn record_comparison() {
    COUNTS.with(|slot| {
        if let Some(mut counts) = slot.get() {
            counts.comparisons += 1;
            slot.set(Some(counts));
        }
    });
}

fn observe<T>(run: impl FnOnce() -> T) -> (T, Counts) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            COUNTS.with(|slot| slot.set(None));
        }
    }
    COUNTS.with(|slot| {
        assert!(slot.get().is_none(), "observations do not nest");
        slot.set(Some(Counts::default()));
    });
    let _reset = Reset;
    let result = run();
    let counts = COUNTS.with(|slot| slot.get().expect("observation is active"));
    (result, counts)
}

fn id(kind: u8, index: usize) -> LedgerIdBytes {
    let mut bytes = [0; 16];
    bytes[0] = kind;
    bytes[8..].copy_from_slice(&(index as u64).to_be_bytes());
    LedgerIdBytes::from_bytes(bytes)
}

fn path(index: usize) -> SemanticPath {
    SemanticPath::root(id(1, 0), id(7, index))
}

fn comparison_ceiling(entries: usize) -> usize {
    // Leave room for standard-library binary-search implementation differences.
    2 * (usize::BITS - entries.leading_zeros()) as usize + 1
}

#[test]
fn sorted_entry_lookup_bounds_comparisons_at_small_and_wide_sizes() {
    for entries in [1, 65, 257] {
        let sites: Vec<_> = (0..entries)
            .rev()
            .map(|index| SealedSite::Flat {
                root: index as u16,
                target: SealedSiteTarget::BranchEntry(vec![3, 7].into_boxed_slice()),
            })
            .collect();
        let paths: Vec<_> = (0..entries).rev().map(path).collect();
        let (_, counts) = observe(|| {
            let families = EntryFamilies::new(&sites, &paths);
            for index in 0..entries {
                assert_eq!(
                    families.get(&path(index)),
                    Some((index as u16, &[3, 7][..]))
                );
            }
        });
        assert_eq!(counts.builds, 1);
        assert_eq!(counts.entry_rows, entries);
        assert!(counts.capacity >= entries);
        assert_eq!(counts.lookups, entries);
        eprintln!("entry lookup, E={entries}: {counts:?}");
        assert!(
            counts.comparisons <= entries * comparison_ceiling(entries),
            "entry lookup exceeds logarithmic comparisons: {counts:?}",
        );
    }
}

#[test]
fn entry_lookup_borrows_branch_storage_and_excludes_non_entries() {
    let sites = [
        SealedSite::Flat {
            root: 0,
            target: SealedSiteTarget::WholePayload,
        },
        SealedSite::Flat {
            root: 3,
            target: SealedSiteTarget::BranchEntry(vec![2, 7].into_boxed_slice()),
        },
        SealedSite::Flat {
            root: 4,
            target: SealedSiteTarget::BranchEntry(vec![2, 7].into_boxed_slice()),
        },
        SealedSite::Flat {
            root: 3,
            target: SealedSiteTarget::BranchEntry(vec![2, 8].into_boxed_slice()),
        },
        SealedSite::Flat {
            root: 0,
            target: SealedSiteTarget::FieldLeaf(0),
        },
        SealedSite::Flat {
            root: 3,
            target: SealedSiteTarget::BranchField {
                branch: vec![2, 7].into_boxed_slice(),
                field: 0,
            },
        },
        SealedSite::Flat {
            root: 0,
            target: SealedSiteTarget::GroupEntry(0),
        },
        SealedSite::Flat {
            root: 0,
            target: SealedSiteTarget::IndexScan(0),
        },
        SealedSite::Flat {
            root: 0,
            target: SealedSiteTarget::IndexLookup(0),
        },
        SealedSite::Parked {
            path: path(9),
            target: SemanticTarget::WholePayload,
        },
    ];
    let paths: Vec<_> = (0..sites.len()).map(path).collect();
    let families = EntryFamilies::new(&sites, &paths);
    assert_eq!(families.rows.len(), 4);
    assert_eq!(families.get(&paths[0]), Some((0, &[][..])));
    assert_eq!(entry_family(&sites[0]), Some((0, &[][..])));
    for index in 1..4 {
        let SealedSite::Flat {
            root,
            target: SealedSiteTarget::BranchEntry(original),
        } = &sites[index]
        else {
            panic!("branch fixture");
        };
        let (found_root, borrowed) = families.get(&paths[index]).expect("entry family");
        assert_eq!(found_root, *root);
        assert_eq!(borrowed, original.as_ref());
        assert_eq!(borrowed.as_ptr(), original.as_ptr());
        assert_eq!(entry_family(&sites[index]), Some((*root, borrowed)));
    }
    for index in 4..sites.len() {
        assert_eq!(families.get(&paths[index]), None);
        assert_eq!(entry_family(&sites[index]), None);
    }
    assert_eq!(families.get(&path(sites.len())), None);
    for (borrowed_path, _, _) in &families.rows {
        assert!(
            paths
                .iter()
                .any(|original| std::ptr::eq(*borrowed_path, original))
        );
    }
}

fn add_function(
    draft: &mut DraftTxn<'_>,
    name: &str,
    params: Vec<ImageType>,
    code: Vec<Instr>,
) -> FuncId {
    let name = draft.intern_string(name).expect("function name");
    let source = draft.intern_string("src/main.mw").expect("source name");
    draft
        .add_function(FunctionDef {
            name,
            source,
            local_count: params.len() as u16,
            params,
            ret: ImageType::Unit,
            spans: (0..code.len())
                .map(|index| SpanEntry {
                    instr_index: index as u32,
                    line: 1,
                    column: 1,
                })
                .collect(),
            code,
        })
        .expect("function operands are live")
}

const CALLS_PER_CALLER: usize = 8;

fn repeated_call_image(entries: usize) -> Vec<u8> {
    assert!(entries >= 2);
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh savepoint");
    draft.set_application_identity(id(1, 0));
    let record_name = draft.intern_string("Counter").expect("record name");
    let value_name = draft.intern_string("value").expect("field name");
    let branch_name = draft.intern_string("children").expect("branch name");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: vec![FieldDef {
                name: value_name,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        })
        .expect("one required integer field");
    let scalar = draft.value_scalar(Scalar::Int).expect("integer shape");
    draft
        .declare_product(
            &admitted_plan(),
            id(2, 0),
            record,
            vec![
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: id(3, 0),
                        required: true,
                        value: scalar,
                    },
                },
                DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Branch {
                        placement: id(4, 0),
                        name: branch_name,
                        record,
                        keys: vec![KeyColumn {
                            scalar: Scalar::Int,
                            id: id(5, 0),
                        }],
                    },
                },
                DeclarationMemberDef {
                    parent: Some(1),
                    shape: DeclarationMemberShape::Field {
                        id: id(6, 0),
                        required: true,
                        value: scalar,
                    },
                },
            ],
        )
        .expect("one field and one keyed branch");
    let members = draft.product_members(id(2, 0)).expect("declared members");
    let key = draft.intern_int(1).expect("key constant");
    let value = draft.intern_int(7).expect("field constant");
    let mut protected = None;
    let mut protected_field = None;
    let mut erased = None;
    let mut reads = Vec::new();
    // One whole-root site plus one branch-entry site per occurrence gives E rows.
    // Field sites are retained too, and must not enlarge the entry index.
    for index in 0..entries - 1 {
        let name = draft
            .intern_string(&format!("root{index:03}"))
            .expect("root name");
        let root = draft
            .add_root_occurrence(
                &admitted_plan(),
                id(2, 0),
                RootOccurrenceDef {
                    name,
                    keys: vec![KeyColumn {
                        scalar: Scalar::Int,
                        id: id(8, index),
                    }],
                    placement: id(7, index),
                    indexes: Vec::new().into(),
                },
            )
            .expect("independent root occurrence");
        let field = site(
            &mut draft,
            root.occurrence(),
            members[0].path(),
            SemanticTarget::FieldLeaf,
        );
        let branch = site(
            &mut draft,
            root.occurrence(),
            members[1].path(),
            SemanticTarget::WholePayload,
        );
        reads.extend([
            Instr::ConstLoad(key),
            Instr::ConstLoad(key),
            Instr::DurExists(branch.clone()),
            Instr::Pop,
            Instr::ConstLoad(key),
            Instr::DurReadField(field.clone()),
            Instr::Pop,
        ]);
        if index == 0 {
            protected = Some(site(
                &mut draft,
                root.occurrence(),
                root.placement_path(),
                SemanticTarget::WholePayload,
            ));
            protected_field = Some(field);
        }
        erased = Some(branch);
    }
    reads.push(Instr::Return);
    add_function(&mut draft, "readUnrelated", Vec::new(), reads);
    let protected = protected.expect("protected root");
    let protected_field = protected_field.expect("protected field");
    // Read and replacement atoms on the protected family must not be looked up
    // as erasures. The sole Erase atom names the last, different branch family.
    let helper = add_function(
        &mut draft,
        "eraseOther",
        Vec::new(),
        vec![
            Instr::ConstLoad(key),
            Instr::DurExists(protected.clone()),
            Instr::Pop,
            Instr::ConstLoad(key),
            Instr::ConstLoad(value),
            Instr::RecordNew(record),
            Instr::DurReplaceEntry(protected.clone()),
            Instr::ConstLoad(key),
            Instr::ConstLoad(key),
            Instr::DurEraseEntry(erased.expect("erased branch")),
            Instr::Return,
        ],
    );
    for name in ["left", "right"] {
        let mut code = vec![
            Instr::TxnBegin,
            Instr::LocalGet(0),
            Instr::DurExists(protected.clone()),
            Instr::JumpIfFalse(0),
        ];
        code.extend((0..CALLS_PER_CALLER).map(|_| Instr::Call(helper.index())));
        code.extend([
            Instr::ConstLoad(value),
            Instr::DurSetField {
                site: protected_field.clone(),
                key_slots: vec![0],
            },
        ]);
        code[3] = Instr::JumpIfFalse(code.len() as u32);
        code.extend([Instr::TxnCommit, Instr::Return]);
        let caller = add_function(&mut draft, name, vec![ImageType::scalar(Scalar::Int)], code);
        draft.add_export(ExportId::of_local("", name), caller);
    }
    draft.encode().expect("bounded durable image").bytes
}

#[test]
fn verification_builds_one_borrowed_index_for_repeated_calls_across_functions() {
    // A protected entry and a different erased entry require at least two rows.
    // The owner-only test above separately covers E = 1.
    for entries in [2, 65, 257] {
        let bytes = repeated_call_image(entries);
        let (verified, counts) = observe(|| crate::verify::verify(&bytes));
        let verified = verified.expect("other-family erasure preserves the root proof");
        assert_eq!(verified.sites().len(), 2 * entries - 1);
        assert_eq!(counts.builds, 1, "the index is shared by all functions");
        assert_eq!(counts.entry_rows, entries);
        assert!(counts.capacity >= entries);
        assert_eq!(counts.lookups, 2 * CALLS_PER_CALLER);
        eprintln!("verified presence calls, E={entries}: {counts:?}");
        assert!(
            counts.comparisons <= counts.lookups * comparison_ceiling(entries),
            "repeated calls exceed logarithmic entry lookup: {counts:?}",
        );
    }
}
