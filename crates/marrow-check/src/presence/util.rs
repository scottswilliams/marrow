pub(super) fn extend_unique<T>(items: &mut Vec<T>, extension: Vec<T>)
where
    T: PartialEq,
{
    for item in extension {
        push_unique(items, item);
    }
}

pub(super) fn push_unique<T>(items: &mut Vec<T>, item: T)
where
    T: PartialEq,
{
    if !items.contains(&item) {
        items.push(item);
    }
}
