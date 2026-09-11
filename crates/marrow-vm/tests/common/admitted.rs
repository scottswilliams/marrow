// The armed transaction a fresh savepoint admits over one draft owner.
//
// This helper obtains a fresh savepoint and begins the owner's draft transaction.
// Reached as a `#[path]` module beside `admitted_plan`.

/// The armed transaction a fresh savepoint admits over `owner`.
pub fn admitted(owner: &mut marrow_image::ImageDraft) -> marrow_image::DraftTxn<'_> {
    let savepoint = owner.savepoint();
    owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits")
}
