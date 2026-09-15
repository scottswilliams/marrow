// The four construction steps every durable fixture repeats: an entry record, a Product
// declaration, a root occurrence over it, and a unit function.
//
// Shared because each step carries an admission contract — a declaration spends the
// plan's Product term, an occurrence spends its root term, a function append validates
// every site operand — and a per-file copy is a per-file opportunity to spell one of
// those contracts differently. Written as ordinary comments rather than inner doc
// comments so the one owner can be reached both as a `#[path]` module and, where a
// nested module has no directory to point at, by `include!`.
#![allow(dead_code)]

use marrow_image::{
    AdmittedGraphInputPlan, AdmittedRoot, DeclarationMember, DeclarationMemberDef, DraftTxn,
    DurableIndexShape, FuncId, FunctionDef, ImageType, Instr, KeyColumn, LedgerIdBytes,
    RecordTypeDef, RootOccurrenceDef, TypeId,
};

/// A record type of `name` with no fields: the entry record a Product declares when the
/// record's own shape is not what the test varies.
pub fn empty_record(draft: &mut DraftTxn<'_>, name: &str) -> TypeId {
    let name = draft.intern_string(name).expect("a within-domain mint");
    draft
        .add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        })
        .expect("a within-domain mint")
}

/// Declare `product` over entry record `record` with `members`, returning its direct
/// members in declaration order.
pub fn declare_product(
    draft: &mut DraftTxn<'_>,
    plan: &AdmittedGraphInputPlan,
    product: LedgerIdBytes,
    record: TypeId,
    members: Vec<DeclarationMemberDef>,
) -> Vec<DeclarationMember> {
    draft
        .declare_product(plan, product, record, members)
        .expect("a well-formed declaration")
}

/// Append one root occurrence of the declared `product`, spelled `name` and placed at
/// `placement`.
pub fn admit_root(
    draft: &mut DraftTxn<'_>,
    plan: &AdmittedGraphInputPlan,
    product: LedgerIdBytes,
    name: &str,
    placement: LedgerIdBytes,
    keys: Vec<KeyColumn>,
    indexes: Vec<DurableIndexShape>,
) -> AdmittedRoot {
    let name = draft.intern_string(name).expect("a within-domain mint");
    draft
        .add_root_occurrence(
            plan,
            product,
            RootOccurrenceDef {
                name,
                keys,
                placement,
                indexes: indexes.into(),
            },
        )
        .expect("the Product is declared")
}

/// A zero-argument unit function of `code`, sourced at `src/main.mw`.
pub fn unit_function(draft: &mut DraftTxn<'_>, name: &str, code: Vec<Instr>) -> FuncId {
    let name = draft.intern_string(name).expect("a within-domain mint");
    let source = draft
        .intern_string("src/main.mw")
        .expect("a within-domain mint");
    draft
        .add_function(FunctionDef {
            name,
            source,
            params: Vec::new(),
            ret: ImageType::Unit,
            local_count: 0,
            code,
            spans: Vec::new(),
        })
        .expect("every site operand is live")
}
