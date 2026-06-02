pub(super) fn extend_unique<T>(items: &mut Vec<T>, extension: Vec<T>)
where
    T: PartialEq,
{
    for item in extension {
        if !items.contains(&item) {
            items.push(item);
        }
    }
}
