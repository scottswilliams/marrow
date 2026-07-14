//! The `ExportId` durable export identity (kernel identity rule).
//!
//! An [`ExportId`] is the stable 32-byte identity of one public (`pub fn`) export.
//! It crosses the compiler → image → verifier → VM boundary, so it is a distinct
//! typed 32-byte domain-separated SHA-256 over a length-delimited canonical payload,
//! exactly as the kernel identity rule requires: one owning phase (C00), one frozen
//! `kind`, one canonical payload, one known-answer test, and one independent-decoder
//! reconstruction test.
//!
//! The payload is the export's *declaration path* and nothing else — not its body,
//! parameters, or return type. Editing a body or changing a signature therefore
//! leaves the id unchanged (the id is *material-stable*), while renaming the
//! function or moving it to another module changes it (a different declaration
//! path). Downstream trust comes from verification: anyone can mint a valid id, so
//! the VM accepts an id only from a verified image, and never dispatches on a
//! source name.
//!
//! ```text
//! ExportId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND    = b"marrow.export.v0"
//!   payload = LP(lineage) ‖ LP(module) ‖ LP(item)
//!   LP(b)   = u64_be(b.len()) ‖ b
//!   lineage = the export's package lineage. The local project root is the single
//!             tag byte 0x00; a dependency package is 0x01 ‖ <32-byte package id>
//!             at a later phase. The tag byte keeps the two disjoint, so a local
//!             export id can never alias a package export id and stays byte-stable
//!             when packages arrive.
//!   module  = the dotted module path, e.g. "a.b"
//!   item    = the export's function name, e.g. "add"
//! ```
//!
//! The construction mirrors the image digest ([`crate::digest`]): `KIND`, then the
//! big-endian length of the whole payload, then the payload. Every module segment
//! and the item are ASCII identifiers (non-empty, no `.`), so the dotted `module`
//! join is injective over segments and the id is collision-free across declaration
//! paths. Three defenses keep that true: the compiler validates every module
//! segment and the item against the identifier domain immediately before minting
//! an id (its `valid_export_path` guard); project capture derives each module
//! name from a unique canonical source path, so no two declarations share a
//! payload; and the verifier rejects an EXPORTS table whose ids are not strictly
//! ascending and unique.
//!
//! Identity is not compatibility. Because signatures are excluded, a later
//! cross-boundary *binding* that stores an `ExportId` must pair it with a separate
//! typed signature fingerprint (its own identity when built) checked at bind time;
//! `ExportId` itself is never widened to carry the signature.

use sha2::{Digest, Sha256};

/// The domain-separation tag for the export identity. Distinct from every other
/// Marrow identity's `kind`, so an `ExportId` can never collide with an `ImageId`
/// or a later identity computed over the same bytes.
pub const EXPORT_ID_KIND: &[u8; 16] = b"marrow.export.v0";

/// The lineage of an export declared in the local project root: the single tag
/// byte `0x00`. A dependency package's lineage begins with `0x01` at a later phase,
/// so the tag byte alone keeps local and package lineages disjoint.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// The stable 32-byte identity of a public export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExportId(pub(crate) [u8; 32]);

impl ExportId {
    /// Compute the identity of the export named `item` declared in module `module`
    /// (the dotted module path) of the local project root. `module` segments and
    /// `item` are expected to be ASCII identifiers; the compiler enforces this
    /// before minting so the dotted join stays injective.
    pub fn of_local(module: &str, item: &str) -> Self {
        Self::compute(LOCAL_ROOT_LINEAGE, module.as_bytes(), item.as_bytes())
    }

    /// Reconstruct an id from its 32 raw bytes. The verifier decodes the id from an
    /// untrusted image with this; it does not recompute the hash, because the image
    /// carries the id and the compiler that minted it is not trusted.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 identity bytes, as carried in the image EXPORTS table.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The domain-separated, length-delimited hash construction. Kept private so the
    /// one canonical payload has a single owner; `of_local` is the only minting
    /// entry point today.
    fn compute(lineage: &[u8], module: &[u8], item: &[u8]) -> Self {
        let mut payload: Vec<u8> = Vec::new();
        for component in [lineage, module, item] {
            payload.extend_from_slice(&(component.len() as u64).to_be_bytes());
            payload.extend_from_slice(component);
        }
        let mut hasher = Sha256::new();
        hasher.update(EXPORT_ID_KIND);
        hasher.update((payload.len() as u64).to_be_bytes());
        hasher.update(&payload);
        ExportId(hasher.finalize().into())
    }

    /// The lowercase hex spelling of the identity, for diagnostics and tests.
    pub fn to_hex(self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in self.0 {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }
}

#[cfg(test)]
mod tests {
    use super::{EXPORT_ID_KIND, ExportId, LOCAL_ROOT_LINEAGE};
    use sha2::{Digest, Sha256};

    #[test]
    fn kind_is_sixteen_bytes_and_distinct_from_the_image_kind() {
        assert_eq!(EXPORT_ID_KIND.len(), 16);
        assert_ne!(
            EXPORT_ID_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
            "the export identity must be domain-separated from the image digest"
        );
    }

    /// Known-answer test for the frozen canonical payload. Freezing this hex pins the
    /// domain-separation and length-delimiting layout so a later reader can
    /// reconstruct it independently. If this value must change, the export identity
    /// contract has changed and every stored/derived id changes with it.
    #[test]
    fn export_id_known_answer() {
        assert_eq!(
            ExportId::of_local("a.b", "add").to_hex(),
            "c7c1a798c2aae5f5c64335ed90dfe97b3272742a6c254851d8da3033e1ca8e34"
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes. It shares no code with
    /// `ExportId::compute`, so a change to the owner that silently altered the layout
    /// would diverge here.
    #[test]
    fn independent_reconstruction_matches() {
        let module = "a.b";
        let item = "add";

        let mut payload: Vec<u8> = Vec::new();
        for component in [LOCAL_ROOT_LINEAGE, module.as_bytes(), item.as_bytes()] {
            let len = component.len() as u64;
            payload.extend_from_slice(&len.to_be_bytes());
            payload.extend_from_slice(component);
        }
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(EXPORT_ID_KIND);
        framed.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        framed.extend_from_slice(&payload);
        let expected: [u8; 32] = Sha256::digest(&framed).into();

        assert_eq!(ExportId::of_local(module, item).bytes(), &expected);
    }

    #[test]
    fn rename_and_move_change_the_id_body_does_not() {
        let base = ExportId::of_local("a.b", "add");
        // A different item name (rename) changes the id.
        assert_ne!(base, ExportId::of_local("a.b", "remove"));
        // A different module (move) changes the id.
        assert_ne!(base, ExportId::of_local("a.c", "add"));
        // The same declaration path is stable regardless of anything else.
        assert_eq!(base, ExportId::of_local("a.b", "add"));
    }

    /// The length-delimiting makes the (module, item) split unambiguous: module
    /// `a.b` + item `c` must not collide with module `a` + item `b.c`, even though
    /// their dotted concatenations coincide.
    #[test]
    fn component_boundaries_do_not_collide() {
        assert_ne!(
            ExportId::of_local("a.b", "c"),
            ExportId::of_local("a", "b.c")
        );
    }
}
