/// Whether a value-returning operation always yields a value or can be absent at
/// the read site. Maybe-present results must be resolved with the same language
/// forms as maybe-present saved reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnPresence {
    Always,
    MaybePresent,
}
