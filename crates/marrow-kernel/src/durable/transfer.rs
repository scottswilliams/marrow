//! Private construction of an empty body from complete logical transfer input.
//! No ordinary session or native service owner exposes raw cell writes.

use marrow_store::{
    ByteEngine, Cell, CommitOutcome, ReadView, SCAN_MAX_RECORDS, StoreError, WriteTxn,
    batch_is_full, cell_within_limits,
};

use super::audit::{self, AuditReport, ContentDigest, Tables};

/// Construction failed; no usable store owner is returned.
#[derive(Debug)]
pub enum RestoreError<R> {
    Input(R),
    Store(StoreError),
    NotEmpty,
    CellLimit,
    Unordered,
    OutsideNamespace,
    Aborted,
    Indeterminate,
    Invalid(AuditReport),
}

impl<R> From<StoreError> for RestoreError<R> {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Input completion must mean validated completion, never an unframed EOF.
/// Tables are reused for admission and final validation; data-sized state is
/// limited to one batch, one pending cell and the preceding key.
pub(super) fn populate<E: ByteEngine, R>(
    engine: &mut E,
    tables: &Tables<'_>,
    mut next: impl FnMut() -> Result<Option<Cell>, R>,
    digest: &mut dyn ContentDigest,
) -> Result<AuditReport, RestoreError<R>> {
    {
        let view = engine.read_view()?;
        if view.get(&[])?.is_some() || !view.scan_after(&[], &[])?.is_empty() {
            return Err(RestoreError::NotEmpty);
        }
    }
    let mut previous: Option<Vec<u8>> = None;
    let mut family_cursor = 0;
    let mut index_cursor = 0;
    let mut batch = Vec::with_capacity(SCAN_MAX_RECORDS);
    let mut bytes = 0;
    loop {
        let Some((key, value)) = next().map_err(RestoreError::Input)? else {
            break;
        };
        if !cell_within_limits(&key, &value) {
            return Err(RestoreError::CellLimit);
        }
        if previous.as_ref().is_some_and(|before| before >= &key) {
            return Err(RestoreError::Unordered);
        }
        if tables
            .namespace(&key, &mut family_cursor, &mut index_cursor)
            .is_none()
        {
            return Err(RestoreError::OutsideNamespace);
        }
        let size = key.len() + value.len();
        if batch_is_full(batch.len(), bytes, size) {
            commit(engine, &mut batch)?;
            bytes = 0;
        }
        previous = Some(key.clone());
        batch.push((key, value));
        bytes += size;
    }
    if !batch.is_empty() {
        commit(engine, &mut batch)?;
    }
    engine.audit_integrity()?;
    let report = audit::inspect(&engine.read_view()?, tables, digest)?;
    if !report.is_clean() {
        return Err(RestoreError::Invalid(report));
    }
    Ok(report)
}

fn commit<E: ByteEngine, R>(engine: &mut E, batch: &mut Vec<Cell>) -> Result<(), RestoreError<R>> {
    let mut txn = engine.begin()?;
    for (key, value) in batch.drain(..) {
        txn.put(&key, value)?;
    }
    match txn.commit() {
        CommitOutcome::Confirmed => Ok(()),
        CommitOutcome::Aborted => Err(RestoreError::Aborted),
        CommitOutcome::Indeterminate => Err(RestoreError::Indeterminate),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{key::KeyScalar, value::ScalarKind};
    use crate::durable::{StoreProjection, StoreSchemaBuilder, number_store, physical};
    use marrow_store::{MAX_VALUE_LEN, MemoryEngine};

    struct Digest;
    impl ContentDigest for Digest {
        fn absorb(&mut self, _: &[u8], _: &[u8]) {}
    }

    fn projection() -> StoreProjection {
        let schema = StoreSchemaBuilder::root("items", vec![ScalarKind::Int])
            .finish()
            .unwrap();
        let mut builder = StoreProjection::builder();
        builder.root(schema);
        builder.finish().unwrap()
    }

    fn cells(projection: &StoreProjection, count: i64) -> Vec<Cell> {
        let number = number_store(projection)[0].root();
        (0..count)
            .map(|key| {
                (
                    physical::marker_key(number, &[KeyScalar::Int(key)]),
                    physical::MARKER_VALUE.to_vec(),
                )
            })
            .collect()
    }

    #[test]
    fn complete_input_spans_batches_and_finishes_with_clean_audit() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        let mut input = cells(&projection, 130).into_iter();
        let report = populate(
            &mut MemoryEngine::new(),
            &tables,
            || Ok::<_, ()>(input.next()),
            &mut Digest,
        )
        .unwrap();
        assert_eq!(report.summary.entries, 130);
        assert!(report.is_clean());
    }

    #[test]
    fn native_late_input_failure_keeps_exact_confirmed_prefix_under_same_owner() {
        use marrow_store::{NativeEngineOwner, NativeOpenAccess};
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "marrow-transfer-prefix-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        eprintln!("preserved native transfer prefix: {}", directory.display());
        NativeEngineOwner::provision(&directory).unwrap();
        let mut owner = NativeEngineOwner::acquire_existing(&directory)
            .unwrap()
            .bind_and_open_existing(NativeOpenAccess::ReadWrite, [0x73; 16], || {
                Ok::<_, std::convert::Infallible>(())
            })
            .unwrap();
        let projection = projection();
        let tables = Tables::new(&projection, &number_store(&projection));
        let input = cells(&projection, 65);
        let mut remaining = input.clone().into_iter();
        let error = populate(
            &mut owner,
            &tables,
            || remaining.next().map(Some).ok_or("late input"),
            &mut Digest,
        )
        .unwrap_err();
        assert!(matches!(error, RestoreError::Input("late input")));
        // The 65th valid cell triggers the preceding 64-cell commit. Observe
        // the same still-open native owner; no failed-body reopen is involved.
        let view = owner.read_view().unwrap();
        let mut actual = Vec::new();
        let mut after = Vec::new();
        loop {
            let page = view.scan_after(&[], &after).unwrap();
            if page.is_empty() {
                break;
            }
            after = page.last().unwrap().0.clone();
            actual.extend(page);
        }
        assert_eq!(actual, input[..64]);
        assert_eq!(view.get(&input[64].0).unwrap(), None);
        drop(view);
        drop(owner);
    }

    #[test]
    fn valid_witness_is_rejected_after_prior_batch_committed() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        let mut input = cells(&projection, 65);
        let key = physical::meta_key(crate::durable::store::WITNESS);
        let mut witness = vec![1];
        witness.extend_from_slice(&0u128.to_be_bytes());
        input.push((key.clone(), witness));
        let mut input = input.into_iter();
        let mut engine = MemoryEngine::new();
        assert!(matches!(
            populate(
                &mut engine,
                &tables,
                || Ok::<_, ()>(input.next()),
                &mut Digest
            ),
            Err(RestoreError::OutsideNamespace)
        ));
        let view = engine.read_view().unwrap();
        assert_eq!(view.scan_after(&[], &[]).unwrap().len(), 64);
        assert_eq!(view.get(&key).unwrap(), None);
    }

    #[test]
    fn populated_body_is_refused_before_reading_input() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        let mut engine = MemoryEngine::new();
        let mut txn = engine.begin().unwrap();
        txn.put(&[], vec![7]).unwrap();
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
        assert!(matches!(
            populate(
                &mut engine,
                &tables,
                || -> Result<Option<Cell>, ()> { panic!("input must not be consumed") },
                &mut Digest
            ),
            Err(RestoreError::NotEmpty)
        ));
        assert_eq!(engine.read_view().unwrap().get(&[]).unwrap(), Some(vec![7]));
    }

    #[test]
    fn late_input_and_order_failures_preserve_only_prior_confirmed_batch() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        for duplicate in [false, true] {
            let mut input = cells(&projection, 65);
            if duplicate {
                input.push(input.last().unwrap().clone());
            }
            let mut input = input.into_iter();
            let mut engine = MemoryEngine::new();
            let result = populate(
                &mut engine,
                &tables,
                || input.next().map(Some).ok_or("input failed"),
                &mut Digest,
            );
            if duplicate {
                assert!(matches!(result, Err(RestoreError::Unordered)));
            } else {
                assert!(matches!(result, Err(RestoreError::Input("input failed"))));
            }
            let view = engine.read_view().unwrap();
            assert_eq!(view.scan_after(&[], &[]).unwrap().len(), 64);
            assert_eq!(view.get(&cells(&projection, 65)[64].0).unwrap(), None);
        }
    }

    #[derive(Clone, Copy)]
    enum Failure {
        Put,
        Commit(CommitOutcome),
    }

    struct Probe {
        memory: MemoryEngine,
        failure: Failure,
        begins: usize,
        audits: usize,
    }
    struct ProbeTxn<'a> {
        inner: <MemoryEngine as ByteEngine>::Txn<'a>,
        failure: Option<Failure>,
    }
    impl ReadView for ProbeTxn<'_> {
        fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
            self.inner.get(key)
        }
        fn scan_after(&self, prefix: &[u8], cursor: &[u8]) -> Result<Vec<Cell>, StoreError> {
            self.inner.scan_after(prefix, cursor)
        }
    }
    impl WriteTxn for ProbeTxn<'_> {
        fn put(&mut self, key: &[u8], value: Vec<u8>) -> Result<(), StoreError> {
            if matches!(self.failure, Some(Failure::Put)) {
                return Err(StoreError::RecoveryRequired);
            }
            self.inner.put(key, value)
        }
        fn remove(&mut self, key: &[u8]) -> Result<(), StoreError> {
            self.inner.remove(key)
        }
        fn commit(self) -> CommitOutcome {
            match self.failure {
                Some(Failure::Commit(outcome)) => outcome,
                _ => self.inner.commit(),
            }
        }
    }
    impl ByteEngine for Probe {
        type View<'a> = <MemoryEngine as ByteEngine>::View<'a>;
        type Txn<'a> = ProbeTxn<'a>;
        fn read_view(&self) -> Result<Self::View<'_>, StoreError> {
            self.memory.read_view()
        }
        fn begin(&mut self) -> Result<Self::Txn<'_>, StoreError> {
            self.begins += 1;
            Ok(ProbeTxn {
                inner: self.memory.begin()?,
                failure: (self.begins == 2).then_some(self.failure),
            })
        }
        fn require_write_access(&self, op: marrow_store::StoreOp) -> Result<(), StoreError> {
            self.memory.require_write_access(op)
        }
        fn audit_integrity(&mut self) -> Result<(), StoreError> {
            self.audits += 1;
            self.memory.audit_integrity()
        }
    }

    #[test]
    fn failed_second_batch_never_retries_reads_more_input_or_audits() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        for failure in [
            Failure::Put,
            Failure::Commit(CommitOutcome::Aborted),
            Failure::Commit(CommitOutcome::Indeterminate),
        ] {
            let mut engine = Probe {
                memory: MemoryEngine::new(),
                failure,
                begins: 0,
                audits: 0,
            };
            let mut input = cells(&projection, 130).into_iter();
            let mut calls = 0;
            let result = populate(
                &mut engine,
                &tables,
                || {
                    calls += 1;
                    Ok::<_, ()>(input.next())
                },
                &mut Digest,
            );
            match failure {
                Failure::Put => assert!(matches!(
                    result,
                    Err(RestoreError::Store(StoreError::RecoveryRequired))
                )),
                Failure::Commit(CommitOutcome::Aborted) => {
                    assert!(matches!(result, Err(RestoreError::Aborted)))
                }
                Failure::Commit(CommitOutcome::Indeterminate) => {
                    assert!(matches!(result, Err(RestoreError::Indeterminate)))
                }
                _ => unreachable!(),
            }
            assert_eq!(engine.begins, 2);
            assert_eq!(engine.audits, 0);
            assert_eq!(calls, 129);
            assert_eq!(
                engine
                    .memory
                    .read_view()
                    .unwrap()
                    .scan_after(&[], &[])
                    .unwrap()
                    .len(),
                64
            );
        }
    }

    #[test]
    fn byte_budget_commits_a_single_oversized_record_before_the_next_one() {
        let projection = projection();
        let numbers = number_store(&projection);
        let tables = Tables::new(&projection, &numbers);
        let mut input = cells(&projection, 3);
        input[0].1 = vec![1; MAX_VALUE_LEN];
        let mut input = input.into_iter();
        let mut engine = Probe {
            memory: MemoryEngine::new(),
            failure: Failure::Commit(CommitOutcome::Aborted),
            begins: 0,
            audits: 0,
        };
        assert!(matches!(
            populate(
                &mut engine,
                &tables,
                || Ok::<_, ()>(input.next()),
                &mut Digest
            ),
            Err(RestoreError::Aborted)
        ));
        assert_eq!(engine.begins, 2);
        assert_eq!(
            engine
                .memory
                .read_view()
                .unwrap()
                .scan_after(&[], &[])
                .unwrap()
                .len(),
            1
        );
    }
}
