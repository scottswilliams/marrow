//! The count-and-bytes ceiling machine every bounded owner in this crate is built on.
//!
//! One owner for the whole rule: a retained payload is charged against a count ceiling
//! and a byte ceiling at the push that produces it; crossing either discards the whole
//! payload — the incoming contribution and every already-retained one — because a
//! crossing refuses the whole product and there is no partial publication to unwind.
//! Count takes precedence over bytes at a simultaneous crossing, and a bytes limit
//! strengthens to count once the composed count crosses. Count never weakens.

/// The ceilings one bounded owner is charged against, and the typed limit it reports.
pub(crate) trait Ceiling {
    /// What the owner retains until a ceiling is crossed.
    type Payload: Default;
    /// The typed limit a crossing reports.
    type Limit: Copy;

    const MAX_COUNT: u64;
    const MAX_BYTES: u64;

    fn count_limit() -> Self::Limit;
    fn bytes_limit() -> Self::Limit;
    /// Whether `limit` is the bytes arm, which a later count crossing strengthens.
    fn is_bytes(limit: Self::Limit) -> bool;
}

/// A bounded owner's exact state.
///
/// `count` and `bytes` are this owner's own contribution, not a composed total: a
/// staging owner charges over a settled ledger's totals through the `base` argument of
/// [`Bounded::admit`] while releasing only what it added. A `Limited` owner's totals
/// saturate at ceiling plus one, so later input composes without unbounded growth.
pub(crate) enum Bounded<C: Ceiling> {
    Retaining {
        count: u64,
        bytes: u64,
        payload: C::Payload,
    },
    Limited {
        count: u64,
        bytes: u64,
        limit: C::Limit,
    },
}

impl<C: Ceiling> std::fmt::Debug for Bounded<C>
where
    C::Payload: std::fmt::Debug,
    C::Limit: std::fmt::Debug,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retaining {
                count,
                bytes,
                payload,
            } => formatter
                .debug_struct("Retaining")
                .field("count", count)
                .field("bytes", bytes)
                .field("payload", payload)
                .finish(),
            Self::Limited {
                count,
                bytes,
                limit,
            } => formatter
                .debug_struct("Limited")
                .field("count", count)
                .field("bytes", bytes)
                .field("limit", limit)
                .finish(),
        }
    }
}

/// Classify one composed total against both ceilings, count taking precedence.
///
/// The sole owner of the ceiling comparison, so a staging owner cannot reach a
/// different verdict for a contribution than the ledger reaches when it settles.
pub(crate) fn crossed<C: Ceiling>(count: u64, bytes: u64) -> Option<C::Limit> {
    if count > C::MAX_COUNT {
        Some(C::count_limit())
    } else if bytes > C::MAX_BYTES {
        Some(C::bytes_limit())
    } else {
        None
    }
}

impl<C: Ceiling> Bounded<C> {
    pub(crate) fn new() -> Self {
        Self::Retaining {
            count: 0,
            bytes: 0,
            payload: C::Payload::default(),
        }
    }

    /// This owner's own running totals, in either state.
    pub(crate) fn totals(&self) -> (u64, u64) {
        match *self {
            Self::Retaining { count, bytes, .. } | Self::Limited { count, bytes, .. } => {
                (count, bytes)
            }
        }
    }

    pub(crate) fn is_limited(&self) -> bool {
        matches!(self, Self::Limited { .. })
    }

    /// Logical emptiness. A `Limited` owner retains nothing but is never empty: its
    /// limit displaced at least one contribution. Production code reads emptiness from
    /// a finished terminal, never from a live owner.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, Self::Retaining { count: 0, .. })
    }

    #[cfg(test)]
    pub(crate) fn limit(&self) -> Option<C::Limit> {
        match self {
            Self::Retaining { .. } => None,
            Self::Limited { limit, .. } => Some(*limit),
        }
    }

    /// Admit one contribution before `retain` may allocate for it, composing over
    /// `base` — a settled ledger's totals for a staging owner, and `(0, 0)` for a
    /// ledger charging itself.
    pub(crate) fn admit(
        &mut self,
        base: (u64, u64),
        added_count: u64,
        added_bytes: u64,
        retain: impl FnOnce(&mut C::Payload),
    ) {
        match self {
            Self::Retaining {
                count,
                bytes,
                payload,
            } => {
                let new_count = count.saturating_add(added_count);
                let new_bytes = bytes.saturating_add(added_bytes);
                let composed_count = base.0.saturating_add(new_count);
                let composed_bytes = base.1.saturating_add(new_bytes);
                match crossed::<C>(composed_count, composed_bytes) {
                    Some(limit) => *self = Self::limited(new_count, new_bytes, limit),
                    None => {
                        *count = new_count;
                        *bytes = new_bytes;
                        retain(payload);
                    }
                }
            }
            Self::Limited {
                count,
                bytes,
                limit,
            } => {
                *count = count.saturating_add(added_count).min(C::MAX_COUNT + 1);
                *bytes = bytes.saturating_add(added_bytes).min(C::MAX_BYTES + 1);
                if C::is_bytes(*limit) && base.0.saturating_add(*count) > C::MAX_COUNT {
                    *limit = C::count_limit();
                }
            }
        }
    }

    /// Compose a contribution whose own payload a crossing already destroyed: this owner
    /// becomes (or stays) `Limited` unconditionally, even when the composed totals sit
    /// under both ceilings, because that payload never re-materializes. A composed
    /// crossing selects its own kind, count first; otherwise `inherited` stands.
    pub(crate) fn absorb_limited(
        &mut self,
        added_count: u64,
        added_bytes: u64,
        inherited: C::Limit,
    ) {
        match self {
            Self::Retaining { count, bytes, .. } => {
                let new_count = count.saturating_add(added_count);
                let new_bytes = bytes.saturating_add(added_bytes);
                let limit = crossed::<C>(new_count, new_bytes).unwrap_or(inherited);
                *self = Self::limited(new_count, new_bytes, limit);
            }
            Self::Limited { .. } => self.admit((0, 0), added_count, added_bytes, |_| {}),
        }
    }

    /// The saturated `Limited` state. The whole retained payload is dropped here.
    fn limited(count: u64, bytes: u64, limit: C::Limit) -> Self {
        Self::Limited {
            count: count.min(C::MAX_COUNT + 1),
            bytes: bytes.min(C::MAX_BYTES + 1),
            limit,
        }
    }
}
