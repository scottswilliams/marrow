//! The one crate-private frozen limits owner.
//!
//! Every production capture uses [`AdapterLimits::DEFAULT`]; neither the CLI nor a
//! later consumer can select a divergent cap, and there is no configurable
//! cross-crate production API or second cap table. The source-file bounds are the
//! existing pure [`CaptureLimits::DEFAULT`], so the physical adapter and the pure
//! owner enforce one shared source-limit table. The frozen values are pinned by
//! the compile-time assertion below.

use marrow_project::CaptureLimits;

/// The frozen production limits table for physical project capture.
pub(crate) struct AdapterLimits {
    /// Bounded `marrow.toml` bytes, read at limit + 1.
    pub(crate) manifest_bytes: usize,
    /// Bounded `marrow.ids` bytes, read at limit + 1.
    pub(crate) identity_ledger_bytes: usize,
    /// Total directory entries visited below `src`, including ignored entries.
    pub(crate) visited_entries: usize,
    /// Directory edges traversed below `src`.
    pub(crate) traversal_depth: usize,
    /// The shared pure source-file/byte bounds.
    pub(crate) source: CaptureLimits,
    /// Raw borrowed overlay entries.
    pub(crate) overlay_entries: usize,
    /// Bytes in one root-relative overlay key.
    pub(crate) overlay_key_bytes: usize,
    /// Bytes in one overlay replacement body.
    pub(crate) overlay_file_bytes: usize,
    /// Bytes across all overlay replacement bodies.
    pub(crate) overlay_total_bytes: usize,
    /// Simultaneously live platform-native path units the adapter retains.
    pub(crate) max_retained_path_units: usize,
    /// Aggregate platform-native path units the adapter works over.
    pub(crate) max_path_work_units: usize,
}

impl AdapterLimits {
    /// The frozen production defaults.
    pub(crate) const DEFAULT: AdapterLimits = AdapterLimits {
        manifest_bytes: 1 << 20,
        identity_ledger_bytes: marrow_project::MAX_IDS_BYTES,
        visited_entries: 65_536,
        traversal_depth: 64,
        source: CaptureLimits::DEFAULT,
        overlay_entries: 4096,
        overlay_key_bytes: marrow_project::MAX_FILE_IDENTITY_BYTES,
        overlay_file_bytes: 1 << 20,
        overlay_total_bytes: 64 << 20,
        max_retained_path_units: 64 << 20,
        max_path_work_units: 64 << 20,
    };
}

// Freeze every default at compile time. This both pins the production values and
// reads every field, so the frozen table is not dead before its producer lands.
const _: () = {
    let limits = AdapterLimits::DEFAULT;
    assert!(limits.manifest_bytes == 1 << 20);
    assert!(limits.identity_ledger_bytes == marrow_project::MAX_IDS_BYTES);
    assert!(limits.visited_entries == 65_536);
    assert!(limits.traversal_depth == 64);
    assert!(limits.source.max_files() == 4096);
    assert!(limits.source.max_file_bytes() == 1 << 20);
    assert!(limits.source.max_total_bytes() == 64 << 20);
    assert!(limits.overlay_entries == 4096);
    assert!(limits.overlay_key_bytes == marrow_project::MAX_FILE_IDENTITY_BYTES);
    assert!(limits.overlay_file_bytes == 1 << 20);
    assert!(limits.overlay_total_bytes == 64 << 20);
    assert!(limits.max_retained_path_units == 64 << 20);
    assert!(limits.max_path_work_units == 64 << 20);
};
