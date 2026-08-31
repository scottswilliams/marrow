// The armed transaction a fresh savepoint admits over one draft owner.
//
// The one owner of this fixture protocol per crate: every test that opens a
// transaction admits it the same way, so admission-law drift in fixtures is
// impossible. Reached as a `#[path]` module beside `admitted_plan`.

/// The armed transaction a fresh savepoint admits over `owner`.
pub fn admitted(owner: &mut marrow_image::ImageDraft) -> marrow_image::DraftTxn<'_> {
    let savepoint = owner.savepoint();
    owner
        .begin_transaction(savepoint)
        .expect("a fresh savepoint admits")
}
