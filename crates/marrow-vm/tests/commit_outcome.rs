use marrow_image::{
    DurableMemberDef, DurableValueShape, ExportId, FieldDef, FunctionDef, ImageDraft, ImageType,
    Instr, KeyColumn, LedgerIdBytes, RecordTypeDef, RootDef, RootIdentity, Scalar, SemanticPath,
    SemanticStep, SemanticStepKind, SiteDef, SpanEntry,
};
use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::durable::{
    AuthorizedSite, BoundedKeys, BoundedLimit, CommitResult, CreateOutcome, DemandCoverage,
    Durable, DurableCommitState, EntryValue, EraseOutcome, InvocationGrant, KernelFault, Presence,
    ReplaceOutcome,
};
use marrow_kernel::equality::ValueDomain;
use marrow_verify::{VerifiedImage, verify};
use marrow_vm::{
    DurableExecutionFault, DurableRun, Ephemeral, Value, mint_ephemeral, run_durable, run_export,
};

const APPLICATION_ID: [u8; 16] = [0x81; 16];
const ROOT_PLACEMENT_ID: [u8; 16] = [0x82; 16];
const ROOT_PRODUCT_ID: [u8; 16] = [0x83; 16];
const ROOT_KEY_ID: [u8; 16] = [0x84; 16];
const VALUE_FIELD_ID: [u8; 16] = [0x85; 16];

fn root_path() -> SemanticPath {
    SemanticPath::from_steps(vec![
        SemanticStep::new(
            SemanticStepKind::Application,
            LedgerIdBytes::from_bytes(APPLICATION_ID),
        ),
        SemanticStep::new(
            SemanticStepKind::Placement,
            LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
        ),
    ])
}

fn spans(code: &[Instr]) -> Vec<SpanEntry> {
    code.iter()
        .enumerate()
        .map(|(index, _)| SpanEntry {
            instr_index: index as u32,
            line: 10 + index as u32,
            column: 3,
        })
        .collect()
}

#[derive(Clone, Copy)]
enum PostCommitFault {
    None,
    Direct,
    Helper,
}

fn commit_image(post_commit_fault: PostCommitFault, mutating: bool) -> VerifiedImage {
    let mut draft = ImageDraft::new();
    let record_name = draft.intern_string("Counter");
    let field_name = draft.intern_string("value");
    let record = draft.add_record_type(RecordTypeDef {
        name: record_name,
        fields: vec![FieldDef {
            name: field_name,
            ty: ImageType::scalar(Scalar::Int),
            required: true,
        }],
    });
    let root_name = draft.intern_string("counters");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    draft.add_root(RootDef {
        name: root_name,
        keys: vec![KeyColumn {
            scalar: Scalar::Int,
            id: LedgerIdBytes::from_bytes(ROOT_KEY_ID),
        }],
        record,
        identity: RootIdentity {
            placement: LedgerIdBytes::from_bytes(ROOT_PLACEMENT_ID),
            product: LedgerIdBytes::from_bytes(ROOT_PRODUCT_ID),
            indexes: Vec::new(),
            members: vec![DurableMemberDef::Field {
                id: LedgerIdBytes::from_bytes(VALUE_FIELD_ID),
                required: true,
                value: DurableValueShape::Scalar(Scalar::Int),
            }],
        },
    });
    let site = draft.add_site(SiteDef::whole_payload(root_path())).index();
    let key = draft.intern_int(1);
    let value = draft.intern_int(7);
    let one = draft.intern_int(1);
    let zero = draft.intern_int(0);
    let helper = if matches!(post_commit_fault, PostCommitFault::Helper) {
        let helper_name = draft.intern_string("faultingHelper");
        let source = draft.intern_string("src/main.mw");
        let helper_code = vec![
            Instr::ConstLoad(one.index()),
            Instr::ConstLoad(zero.index()),
            Instr::IntDiv,
            Instr::Pop,
            Instr::Return,
        ];
        Some(draft.add_function(FunctionDef {
            name: helper_name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            spans: spans(&helper_code),
            code: helper_code,
        }))
    } else {
        None
    };
    let mut code = if mutating {
        vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key.index()),
            Instr::ConstLoad(value.index()),
            Instr::RecordNew(record.index()),
            Instr::DurCreateEntry(site),
            Instr::TxnCommit,
        ]
    } else {
        vec![
            Instr::TxnBegin,
            Instr::ConstLoad(key.index()),
            Instr::DurReadEntry(site),
            Instr::Pop,
            Instr::TxnCommit,
        ]
    };
    match post_commit_fault {
        PostCommitFault::None => {}
        PostCommitFault::Direct => code.extend([
            Instr::ConstLoad(one.index()),
            Instr::ConstLoad(zero.index()),
            Instr::IntDiv,
            Instr::Pop,
        ]),
        PostCommitFault::Helper => {
            code.push(Instr::Call(helper.expect("helper was built").index()));
        }
    }
    code.push(Instr::Return);
    let export_name = if mutating { "write" } else { "read" };
    let name = draft.intern_string(export_name);
    let source = draft.intern_string("src/main.mw");
    let function = draft.add_function(FunctionDef {
        name,
        source,
        params: Vec::new(),
        ret: ImageType::Unit,
        local_count: 0,
        spans: spans(&code),
        code,
    });
    draft.add_export(ExportId::of_local("", export_name), function);
    verify(&draft.encode().expect("encode").bytes).expect("verify")
}

enum CommitMode {
    Delegate,
    Abort,
}

/// Override only the commit verdict while all durable operations run through a
/// real kernel transaction session. Returning `Aborted` leaves the inner
/// transaction to roll back on drop.
struct CommitOverride<D> {
    inner: D,
    mode: CommitMode,
}

impl<D: Durable> Durable for CommitOverride<D> {
    fn site(&self, index: u16) -> AuthorizedSite {
        self.inner.site(index)
    }

    fn presence(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        self.inner.presence(site, keys)
    }

    fn read_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<ValueDomain>, KernelFault> {
        self.inner.read_field(site, keys)
    }

    fn read_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        self.inner.read_entry(site, keys)
    }

    fn read_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<Option<EntryValue>, KernelFault> {
        self.inner.read_group(site, keys)
    }

    fn replace_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        self.inner.replace_group(site, keys, value)
    }

    fn erase_group(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        self.inner.erase_group(site, keys)
    }

    fn iterate_bounded(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        self.inner.iterate_bounded(site, ancestor_keys, from, limit)
    }

    fn index_scan(
        &mut self,
        site: &AuthorizedSite,
        prefix: &[KeyScalar],
        from: Option<KeyScalar>,
        limit: BoundedLimit,
    ) -> Result<BoundedKeys, KernelFault> {
        self.inner.index_scan(site, prefix, from, limit)
    }

    fn index_lookup(
        &mut self,
        site: &AuthorizedSite,
        key: &[KeyScalar],
    ) -> Result<Option<Vec<KeyScalar>>, KernelFault> {
        self.inner.index_lookup(site, key)
    }

    fn family_populated(
        &mut self,
        site: &AuthorizedSite,
        ancestor_keys: &[KeyScalar],
    ) -> Result<Presence, KernelFault> {
        self.inner.family_populated(site, ancestor_keys)
    }

    fn set_required(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: ValueDomain,
    ) -> Result<(), KernelFault> {
        self.inner.set_required(site, keys, value)
    }

    fn set_sparse(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        self.inner.set_sparse(site, keys, value)
    }

    fn set_sparse_present(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        value: Option<ValueDomain>,
    ) -> Result<(), KernelFault> {
        self.inner.set_sparse_present(site, keys, value)
    }

    fn create_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<CreateOutcome, KernelFault> {
        self.inner.create_entry(site, keys, entry)
    }

    fn replace_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
        entry: EntryValue,
    ) -> Result<ReplaceOutcome, KernelFault> {
        self.inner.replace_entry(site, keys, entry)
    }

    fn erase_field(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        self.inner.erase_field(site, keys)
    }

    fn erase_entry(
        &mut self,
        site: &AuthorizedSite,
        keys: &[KeyScalar],
    ) -> Result<EraseOutcome, KernelFault> {
        self.inner.erase_entry(site, keys)
    }

    fn commit(&mut self) -> CommitResult {
        match self.mode {
            CommitMode::Delegate => self.inner.commit(),
            CommitMode::Abort => CommitResult::Aborted,
        }
    }
}

fn run_with_mode(
    image: &VerifiedImage,
    mode: CommitMode,
) -> Result<Option<Value>, DurableExecutionFault> {
    let mut attachment = match mint_ephemeral(image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("fixture must be executable"),
        Ephemeral::Failed(code) => panic!("attachment failed: {code}"),
    };
    let export = image
        .export_by_id(ExportId::of_local("", "write"))
        .expect("export");
    let session = attachment
        .txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: export.demand().reads(),
                write: export.demand().writes(),
            },
        )
        .expect("transaction session");
    let mut session = CommitOverride {
        inner: session,
        mode,
    };
    run_durable(image, export.function(), Vec::new(), &mut session)
}

#[test]
fn aborted_commit_is_incomplete_known_old() {
    let image = commit_image(PostCommitFault::None, true);
    let fault = run_with_mode(&image, CommitMode::Abort).expect_err("abort is not a return");
    let DurableExecutionFault::Incomplete(incomplete) = fault else {
        panic!("aborted commit was flattened to an ordinary runtime fault");
    };
    assert_eq!(incomplete.runtime_fault().code(), "run.commit");
    assert_eq!(
        incomplete.durable_state(),
        Some(DurableCommitState::KnownOld)
    );
}

#[test]
fn confirmed_commit_followed_by_pure_fault_is_incomplete_known_new() {
    let image = commit_image(PostCommitFault::Direct, true);
    let fault =
        run_with_mode(&image, CommitMode::Delegate).expect_err("post-commit divide must fault");
    let DurableExecutionFault::Incomplete(incomplete) = fault else {
        panic!("post-commit fault was flattened to an ordinary runtime fault");
    };
    assert_eq!(incomplete.runtime_fault().code(), "run.divide_by_zero");
    assert_eq!(incomplete.runtime_fault().line(), 18);
    assert_eq!(
        incomplete.durable_state(),
        Some(DurableCommitState::KnownNew)
    );
}

#[test]
fn confirmed_commit_followed_by_helper_fault_is_incomplete_known_new() {
    let image = commit_image(PostCommitFault::Helper, true);
    let fault =
        run_with_mode(&image, CommitMode::Delegate).expect_err("post-commit helper must fault");
    let DurableExecutionFault::Incomplete(incomplete) = fault else {
        panic!("post-commit helper fault was flattened to an ordinary runtime fault");
    };
    assert_eq!(incomplete.runtime_fault().code(), "run.divide_by_zero");
    assert_eq!(
        incomplete.runtime_fault().line(),
        12,
        "the incomplete outcome retains the helper instruction's source span",
    );
    assert_eq!(
        incomplete.durable_state(),
        Some(DurableCommitState::KnownNew)
    );
}

#[test]
fn read_only_region_followed_by_pure_fault_is_an_ordinary_runtime_fault() {
    let image = commit_image(PostCommitFault::Direct, false);
    let mut attachment = match mint_ephemeral(&image) {
        Ephemeral::Ready(attachment) => *attachment,
        Ephemeral::Parked => panic!("fixture must be executable"),
        Ephemeral::Failed(code) => panic!("attachment failed: {code}"),
    };
    let export = image
        .export_by_id(ExportId::of_local("", "read"))
        .expect("read export");
    let DurableRun::Ran(result) = run_export(&image, &mut attachment, export, Vec::new()) else {
        panic!("read-only fixture must run")
    };
    let fault = result.expect_err("post-region divide must fault");
    let DurableExecutionFault::Runtime(fault) = fault else {
        panic!("a read-only region was misreported as a confirmed durable write")
    };
    assert_eq!(fault.code(), "run.divide_by_zero");
}
