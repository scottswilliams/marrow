//! A stored key must retain the type established by the verified traversal.

use super::{
    APPLICATION_ID, BorrowedEngine, ROOT_KEY_ID, ROOT_PLACEMENT_ID, ROOT_PRODUCT_ID, admitted_plan,
    vm_spans,
};
use marrow_image::{
    CollectionTypeDef, ExportId, FunctionDef, ImageDraft, ImageType, Instr, KeyColumn,
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget,
};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::durable::{
    CommitResult, DemandCoverage, Durable, DurableStore, EntryValue, InvocationGrant, SiteTarget,
    StoreProjection, StoreSchemaBuilder, number_store,
};
use marrow_lifecycle::{PreparedImage, prepare};
use marrow_store::{ByteEngine, MemoryEngine, ReadView};
use marrow_verify::{SealedInstr, verify};

use crate::fault::DurableExecutionFault;
use crate::run::run_durable;
use crate::value::Value;

fn integer_traversal() -> PreparedImage {
    let mut owner = ImageDraft::new();
    let savepoint = owner.savepoint();
    let mut draft = owner.begin_transaction(savepoint).expect("fresh draft");
    let record_name = draft.intern_string("Item").expect("record name");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: record_name,
            fields: Vec::new(),
        })
        .expect("empty complete record");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let product = LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID);
    draft
        .declare_product(&admitted_plan(), product, record, Vec::new())
        .expect("empty declaration");
    let root_name = draft.intern_string("items").expect("root name");
    let root = draft
        .add_root_occurrence(
            &admitted_plan(),
            product,
            RootOccurrenceDef {
                name: root_name,
                keys: vec![KeyColumn {
                    scalar: Scalar::Int,
                    id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
                }],
                placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
                indexes: Vec::new().into(),
            },
        )
        .expect("integer root");
    let handle = draft
        .bind_occurrence_site(
            root.occurrence(),
            root.placement_path(),
            SemanticTarget::WholePayload,
        )
        .expect("root path");
    let site = draft.request_site(&handle).expect("live site");
    let list = draft
        .add_collection_type(CollectionTypeDef::List {
            elem: ImageType::scalar(Scalar::Int),
        })
        .expect("integer list");
    let code = vec![
        Instr::DurIterateBounded {
            site,
            limit: 1,
            from: false,
            list_ty: list,
        },
        Instr::Pop,
        Instr::Return,
    ];
    let name = draft.intern_string("keys").expect("export name");
    let source = draft.intern_string("src/main.mw").expect("source name");
    let function = draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Collection {
                idx: list,
                optional: false,
            },
            local_count: 0,
            spans: vm_spans(&code),
            code,
        })
        .expect("live traversal");
    draft.add_export(ExportId::of_local("", "keys"), function);
    let bytes = draft.encode().expect("encode traversal").bytes;
    prepare(verify(&bytes).expect("independently verify integer traversal"))
}

fn seed(engine: &mut MemoryEngine, projection: &StoreProjection, key: KeyScalar) {
    // The alternate trusted projection writes a real marker in the same numbered
    // family. Reopening under the verified projection exposes a wrong-kind cell
    // without copying the kernel's physical grammar into this VM test.
    let schema = StoreSchemaBuilder::root("items", vec![key.scalar_kind()])
        .finish()
        .expect("empty complete schema");
    let mut seed_projection = StoreProjection::builder();
    seed_projection.root(schema);
    seed_projection.site(0, SiteTarget::whole_payload());
    let seed_projection = seed_projection.finish().expect("seed projection");
    assert_eq!(number_store(&seed_projection), number_store(projection));
    let mut store = DurableStore::from_engine(BorrowedEngine(engine), seed_projection);
    let mut session = store
        .txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: true,
            },
        )
        .expect("seed session");
    session
        .create_entry(
            &session.site(0),
            &[key],
            EntryValue {
                fields: Vec::new(),
                groups: Vec::new(),
            },
        )
        .expect("real complete entry");
    assert!(matches!(session.commit(), CommitResult::Committed));
}

#[test]
fn a_wrong_kind_stored_key_faults_at_the_verified_traversal() {
    let prepared = integer_traversal();
    let projection = prepared.projection().expect("executable projection");
    let image = prepared.image();
    let export = image
        .export_by_id(ExportId::of_local("", "keys"))
        .expect("export");
    let function = image
        .function(export.function())
        .expect("verified function");
    let tape = function.body().instrs();
    assert!(matches!(tape[0], SealedInstr::DurIterateBounded { .. }));

    for key in [KeyScalar::Int(7), KeyScalar::Str("wrong".into())] {
        let valid = matches!(key, KeyScalar::Int(_));
        let mut engine = MemoryEngine::new();
        seed(&mut engine, projection, key);
        let before = engine
            .read_view()
            .expect("seeded view")
            .scan_after(&[], &[])
            .expect("cells");
        assert_eq!(before.len(), 2, "one entry marker and the commit witness");
        {
            let mut store =
                DurableStore::from_engine(BorrowedEngine(&mut engine), projection.clone());
            let mut session = store
                .read_session(
                    InvocationGrant::full_store(),
                    DemandCoverage {
                        read: export.demand().reads(),
                        write: export.demand().writes(),
                    },
                )
                .expect("verified read session");
            let result = run_durable(function, Vec::new(), &mut session);
            if valid {
                let Some(Value::List(_, _, items)) = result.expect("valid key traversal") else {
                    panic!("the verified traversal returns a list");
                };
                assert_eq!(items.as_slice(), &[Value::Int(7)]);
            } else {
                let DurableExecutionFault::Runtime(fault) =
                    result.expect_err("wrong-kind stored key")
                else {
                    panic!("a key decoding fault is an ordinary runtime fault");
                };
                assert_eq!(fault.code(), marrow_codes::Code::RunCorruption.as_str());
                assert_eq!((fault.line(), fault.column()), (20, 4));
            }
        }
        assert_eq!(
            engine
                .read_view()
                .expect("after view")
                .scan_after(&[], &[])
                .expect("after cells"),
            before,
            "read-only execution preserves the exact seeded bytes",
        );
    }
}
