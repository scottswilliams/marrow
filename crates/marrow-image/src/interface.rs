//! The host-neutral wire interface descriptor and the `InterfaceId` identity (G00a).
//!
//! A program's **wire interface** is the set of its concrete root-package exports,
//! each described host-neutrally by a [`FunctionDescriptor`]: the export's stable
//! [`ExportId`], its parameter and return types projected into the closed **transfer
//! graph**, and its verifier-reconstructed durable demand identity
//! ([`DemandSetId`]). Both real callers — the terminal and the generated TypeScript
//! client — consume this one descriptor set; neither reparses source, and no
//! host-specific value model crosses the boundary.
//!
//! Like the [`DemandSetId`](crate::DemandSetId), the interface is a **fact
//! reconstructed from the verified image, never serialized into it**. Every input —
//! the export ids, the function parameter/return types, the record and enum tables,
//! and each export's `DemandSetId` — is already present in a `VerifiedImage`, so the
//! [`InterfaceId`] derives from verified facts (like `DemandSetId` derives at
//! verify) rather than trusting a compiler-written interface section. A body edit
//! that changes no signature and no demand leaves the `InterfaceId` unchanged while
//! changing the [`ImageId`](crate::ImageId); any signature or demand change moves it.
//!
//! The **transfer graph** is the closed set of value types that may cross the wire
//! at G00a: `unit`, the seven scalars (a nominal type erases to its scalar, so it is
//! already covered), a product (`struct`/record), and a sum (a user `enum`, or a
//! built-in `Option`/`Result`, both represented as image enums). Finite collections
//! (`List`/`Map`) are deliberately excluded until the earned transfer extension
//! (G00b); a signature that reaches one is rejected with a typed
//! [`InterfaceError::TransferTypeExcluded`]. Because a record field or enum payload
//! may itself be a record or enum, a signature is expanded structurally under a node
//! budget ([`bounds::MAX_INTERFACE_TRANSFER_NODES`](crate::bounds::MAX_INTERFACE_TRANSFER_NODES)),
//! so a verified-but-adversarial diamond of many-fielded records cannot drive an
//! exponential expansion.
//!
//! ```text
//! InterfaceId = SHA-256( KIND ‖ u64_be(len(payload)) ‖ payload )
//!   KIND       = b"marrow.interf.v0"
//!   payload    = LP(lineage) ‖ u32_be(export_count) ‖ export*   (ascending by ExportId bytes)
//!   export     = LP(export_body)
//!   export_body= id32 ‖ demand_id32 ‖ u16_be(param_count) ‖ ttype* ‖ ttype   (params then the return)
//!   ttype      = 0x00                                   unit
//!              | 0x01 ‖ u8(scalar_tag)                  scalar
//!              | 0x02 ‖ ttype                           optional wrapper
//!              | 0x03 ‖ u16_be(n) ‖ field*              product (record)
//!              | 0x04 ‖ u16_be(n) ‖ variant*            sum (enum / Option / Result)
//!   field      = LP(name) ‖ u8(required) ‖ ttype
//!   variant    = LP(name) ‖ u8(category) ‖ u16_be(m) ‖ ttype*
//!   LP(b)      = u64_be(b.len()) ‖ b
//!   lineage    = the local project root's single tag byte 0x00; a dependency
//!                package is 0x01 ‖ <32-byte package id> at a later phase, mirroring
//!                the `ExportId`/`DemandSetId` lineage seam.
//! ```
//!
//! The export's `ExportId` already carries its declaration-path (name) identity, so
//! the interface identity need not re-encode the name; it composes the export ids,
//! their transfer signatures, and their demand ids. Field and variant names *are*
//! encoded, because a rename is an observable signature change a caller must track.

use sha2::{Digest, Sha256};

use crate::bounds::MAX_INTERFACE_TRANSFER_NODES;
use crate::demand::DemandSetId;
use crate::export_id::ExportId;
use crate::ty::{ImageType, Scalar};

/// The domain-separation tag for the wire-interface identity. Distinct from every
/// other Marrow identity's `kind`, so an `InterfaceId` can never collide with an
/// `ImageId`, `ExportId`, `DemandSetId`, or `DurableContractId` over the same bytes.
pub const INTERFACE_ID_KIND: &[u8; 16] = b"marrow.interf.v0";

/// The lineage of an interface computed in the local project root: the single tag
/// byte `0x00`. A dependency package's lineage begins with `0x01` at a later phase.
const LOCAL_ROOT_LINEAGE: &[u8] = &[0x00];

/// A resolved transfer type: the structural shape of one value that may cross the
/// wire, expanded from an [`ImageType`] against the record and enum tables. The
/// closed set at G00a; a finite collection is not a transfer type and is rejected
/// before a descriptor is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferType {
    Unit,
    Scalar(Scalar),
    /// A `T?` optional wrapper around a bare transfer type.
    Optional(Box<TransferType>),
    /// A product (`struct`/record): its fields in declaration order.
    Product(Vec<TransferField>),
    /// A sum (a user `enum`, or a built-in `Option`/`Result`): its variants in
    /// declaration order.
    Sum(Vec<TransferVariant>),
}

/// One field of a product transfer type: its declared name, whether it is required
/// (a sparse field is optional), and its transfer type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferField {
    pub name: String,
    pub required: bool,
    pub ty: TransferType,
}

/// One variant of a sum transfer type: its member name, the reserved `category`
/// flag (always a leaf on the current flat line), and its dense payload in
/// declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferVariant {
    pub name: String,
    pub category: bool,
    pub payload: Vec<TransferType>,
}

impl TransferType {
    /// Append this transfer type's canonical bytes (the `ttype` grammar above).
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            TransferType::Unit => out.push(0x00),
            TransferType::Scalar(scalar) => {
                out.push(0x01);
                out.push(scalar.tag());
            }
            TransferType::Optional(inner) => {
                out.push(0x02);
                inner.encode(out);
            }
            TransferType::Product(fields) => {
                out.push(0x03);
                out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
                for field in fields {
                    push_lp(out, field.name.as_bytes());
                    out.push(u8::from(field.required));
                    field.ty.encode(out);
                }
            }
            TransferType::Sum(variants) => {
                out.push(0x04);
                out.extend_from_slice(&(variants.len() as u16).to_be_bytes());
                for variant in variants {
                    push_lp(out, variant.name.as_bytes());
                    out.push(u8::from(variant.category));
                    out.extend_from_slice(&(variant.payload.len() as u16).to_be_bytes());
                    for leaf in &variant.payload {
                        leaf.encode(out);
                    }
                }
            }
        }
    }
}

/// A record's structural shape for interface projection: its fields in declaration
/// order. A caller populates this from a verified image's record table (each field's
/// name, bare type, and required flag).
#[derive(Debug, Clone)]
pub struct RecordShape {
    pub fields: Vec<FieldShape>,
}

/// One field of a [`RecordShape`]: its name, bare [`ImageType`], and required flag.
#[derive(Debug, Clone)]
pub struct FieldShape {
    pub name: String,
    pub ty: ImageType,
    pub required: bool,
}

/// An enum's structural shape for interface projection: its variants in declaration
/// order. A caller populates this from a verified image's enum table.
#[derive(Debug, Clone)]
pub struct EnumShape {
    pub variants: Vec<VariantShape>,
}

/// One variant of an [`EnumShape`]: its name, `category` flag, and dense payload of
/// bare [`ImageType`]s.
#[derive(Debug, Clone)]
pub struct VariantShape {
    pub name: String,
    pub category: bool,
    pub payload: Vec<ImageType>,
}

/// One export's wire signature as a caller supplies it: the export's id, its
/// parameter and return [`ImageType`]s (the return mapped from the function's
/// `RetShape`), and its verifier-reconstructed [`DemandSetId`].
#[derive(Debug, Clone)]
pub struct ExportSignature {
    pub id: ExportId,
    pub params: Vec<ImageType>,
    pub ret: ImageType,
    pub demand_id: DemandSetId,
}

/// The host-neutral descriptor of one export: its stable id, its parameters and
/// return projected into the transfer graph, and its demand identity. Shared by the
/// terminal and the generated TypeScript client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDescriptor {
    id: ExportId,
    params: Vec<TransferType>,
    ret: TransferType,
    demand_id: DemandSetId,
}

impl FunctionDescriptor {
    pub fn id(&self) -> ExportId {
        self.id
    }
    pub fn params(&self) -> &[TransferType] {
        &self.params
    }
    pub fn ret(&self) -> &TransferType {
        &self.ret
    }
    pub fn demand_id(&self) -> DemandSetId {
        self.demand_id
    }

    /// Append this descriptor's canonical `export_body` bytes.
    fn encode_body(&self) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(self.id.bytes());
        body.extend_from_slice(self.demand_id.bytes());
        body.extend_from_slice(&(self.params.len() as u16).to_be_bytes());
        for param in &self.params {
            param.encode(&mut body);
        }
        self.ret.encode(&mut body);
        body
    }
}

/// The reconstructed wire interface: one [`FunctionDescriptor`] per export, sorted
/// ascending by [`ExportId`]. Built only by [`Interface::build`], so the canonical
/// order is established once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    descriptors: Vec<FunctionDescriptor>,
}

/// Where in an export's signature a rejected type appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignaturePosition {
    /// The zero-based parameter position.
    Param(u16),
    /// The return type.
    Return,
}

/// A value kind that is not in the G00a transfer graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExcludedKind {
    /// A finite `List`/`Map`. Collections join the transfer graph at G00b.
    Collection,
    /// An entry identity `Id(^root)`. An identity references a store root and is a
    /// runtime/lookup value, not a self-describing transferable value on this line.
    Identity,
}

/// Why an interface could not be reconstructed from a signature set. Each is a typed
/// fact about one export, not rendered prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterfaceError {
    /// An export signature reaches a value type outside the transfer graph.
    TransferTypeExcluded {
        export: ExportId,
        position: SignaturePosition,
        kind: ExcludedKind,
    },
    /// An export signature expands to more transfer nodes than the budget admits.
    SignatureTooComplex { export: ExportId },
    /// A record or enum reference in a signature names no table row. Unreachable from
    /// a verified image; a defense for a caller-assembled signature set.
    TypeIndexOutOfRange { export: ExportId },
}

impl std::fmt::Display for InterfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterfaceError::TransferTypeExcluded {
                position,
                kind: ExcludedKind::Collection,
                ..
            } => write!(
                f,
                "{} uses a collection, which cannot cross the wire yet",
                position_word(*position)
            ),
            InterfaceError::TransferTypeExcluded {
                position,
                kind: ExcludedKind::Identity,
                ..
            } => write!(
                f,
                "{} uses an entry identity, which cannot cross the wire yet",
                position_word(*position)
            ),
            InterfaceError::SignatureTooComplex { .. } => {
                write!(f, "an export signature is too complex to describe")
            }
            InterfaceError::TypeIndexOutOfRange { .. } => {
                write!(f, "an export signature names an unknown type")
            }
        }
    }
}

impl std::error::Error for InterfaceError {}

fn position_word(position: SignaturePosition) -> String {
    match position {
        SignaturePosition::Param(index) => format!("parameter {index}"),
        SignaturePosition::Return => "the return type".to_string(),
    }
}

impl Interface {
    /// Reconstruct the interface from the export signatures and the record/enum
    /// tables. Each signature is projected into the transfer graph (rejecting a
    /// collection or an over-budget expansion), then descriptors are sorted ascending
    /// by [`ExportId`].
    ///
    /// `records[i]` and `enums[i]` are the shapes at image record/enum index `i`,
    /// exactly the tables a `VerifiedImage` exposes.
    pub fn build(
        exports: impl IntoIterator<Item = ExportSignature>,
        records: &[RecordShape],
        enums: &[EnumShape],
    ) -> Result<Self, InterfaceError> {
        let mut descriptors = Vec::new();
        for export in exports {
            let mut budget = MAX_INTERFACE_TRANSFER_NODES;
            let mut params = Vec::with_capacity(export.params.len());
            for (index, param) in export.params.iter().enumerate() {
                let position = SignaturePosition::Param(index as u16);
                params.push(resolve(
                    *param,
                    records,
                    enums,
                    export.id,
                    position,
                    &mut budget,
                )?);
            }
            let ret = resolve(
                export.ret,
                records,
                enums,
                export.id,
                SignaturePosition::Return,
                &mut budget,
            )?;
            descriptors.push(FunctionDescriptor {
                id: export.id,
                params,
                ret,
                demand_id: export.demand_id,
            });
        }
        descriptors.sort_by(|a, b| a.id.bytes().cmp(b.id.bytes()));
        Ok(Self { descriptors })
    }

    /// The reconstructed descriptors, ascending by [`ExportId`].
    pub fn descriptors(&self) -> &[FunctionDescriptor] {
        &self.descriptors
    }

    /// The stable identity of this interface: a domain-separated SHA-256 over the
    /// length-delimited canonical payload of the sorted descriptor set.
    pub fn interface_id(&self) -> InterfaceId {
        let mut payload: Vec<u8> = Vec::new();
        push_lp(&mut payload, LOCAL_ROOT_LINEAGE);
        payload.extend_from_slice(&(self.descriptors.len() as u32).to_be_bytes());
        for descriptor in &self.descriptors {
            push_lp(&mut payload, &descriptor.encode_body());
        }
        let mut hasher = Sha256::new();
        hasher.update(INTERFACE_ID_KIND);
        hasher.update((payload.len() as u64).to_be_bytes());
        hasher.update(&payload);
        InterfaceId(hasher.finalize().into())
    }
}

/// Project one [`ImageType`] into the transfer graph, resolving record/enum
/// references against the tables and rejecting a collection or an over-budget
/// expansion. `budget` is decremented once per produced [`TransferType`] node.
fn resolve(
    ty: ImageType,
    records: &[RecordShape],
    enums: &[EnumShape],
    export: ExportId,
    position: SignaturePosition,
    budget: &mut usize,
) -> Result<TransferType, InterfaceError> {
    if *budget == 0 {
        return Err(InterfaceError::SignatureTooComplex { export });
    }
    *budget -= 1;
    match ty {
        ImageType::Unit => Ok(TransferType::Unit),
        ImageType::Scalar { scalar, optional } => Ok(wrap(optional, TransferType::Scalar(scalar))),
        ImageType::Record { idx, optional } => {
            let record = records
                .get(idx as usize)
                .ok_or(InterfaceError::TypeIndexOutOfRange { export })?;
            let mut fields = Vec::with_capacity(record.fields.len());
            for field in &record.fields {
                fields.push(TransferField {
                    name: field.name.clone(),
                    required: field.required,
                    ty: resolve(field.ty, records, enums, export, position, budget)?,
                });
            }
            Ok(wrap(optional, TransferType::Product(fields)))
        }
        ImageType::Enum { idx, optional } => {
            let enum_shape = enums
                .get(idx as usize)
                .ok_or(InterfaceError::TypeIndexOutOfRange { export })?;
            let mut variants = Vec::with_capacity(enum_shape.variants.len());
            for variant in &enum_shape.variants {
                let mut payload = Vec::with_capacity(variant.payload.len());
                for leaf in &variant.payload {
                    payload.push(resolve(*leaf, records, enums, export, position, budget)?);
                }
                variants.push(TransferVariant {
                    name: variant.name.clone(),
                    category: variant.category,
                    payload,
                });
            }
            Ok(wrap(optional, TransferType::Sum(variants)))
        }
        ImageType::Collection { .. } => Err(InterfaceError::TransferTypeExcluded {
            export,
            position,
            kind: ExcludedKind::Collection,
        }),
        ImageType::Identity { .. } => Err(InterfaceError::TransferTypeExcluded {
            export,
            position,
            kind: ExcludedKind::Identity,
        }),
    }
}

/// Wrap `inner` in an [`TransferType::Optional`] when `optional`.
fn wrap(optional: bool, inner: TransferType) -> TransferType {
    if optional {
        TransferType::Optional(Box::new(inner))
    } else {
        inner
    }
}

/// The stable 32-byte identity of a program's wire interface. Separate from every
/// export's [`ExportId`], each export's [`DemandSetId`], and the image's
/// [`ImageId`](crate::ImageId): a body edit that preserves every signature and demand
/// preserves this id while changing the `ImageId`; any signature or demand change
/// moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InterfaceId([u8; 32]);

impl InterfaceId {
    /// Reconstruct an id from its 32 raw bytes. There is no trusted serialized
    /// interface id — the interface is always recomputed from verified facts — so
    /// this exists only for a downstream fact consumer that stores an id alongside
    /// its own data (for example a generated client pinning the interface it targets).
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The 32 identity bytes.
    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
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

/// Append `u64_be(len) ‖ bytes`.
fn push_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::{
        EnumShape, ExcludedKind, ExportSignature, FieldShape, INTERFACE_ID_KIND, Interface,
        InterfaceError, LOCAL_ROOT_LINEAGE, RecordShape, SignaturePosition, TransferType,
        VariantShape, push_lp,
    };
    use crate::bounds::MAX_INTERFACE_TRANSFER_NODES;
    use crate::demand::{DemandAtom, ExportDemand, OperationClass};
    use crate::durable_id::LedgerIdBytes;
    use crate::export_id::ExportId;
    use crate::semantic::{SemanticPath, SemanticStep, SemanticStepKind};
    use crate::ty::{ImageType, Scalar};
    use sha2::{Digest, Sha256};

    fn ledger(byte: u8) -> LedgerIdBytes {
        LedgerIdBytes::from_bytes([byte; 16])
    }

    fn demand_a() -> crate::DemandSetId {
        ExportDemand::from_atoms([DemandAtom::new(
            SemanticPath::from_steps(vec![
                SemanticStep::new(SemanticStepKind::Application, ledger(0x0a)),
                SemanticStep::new(SemanticStepKind::Placement, ledger(0x0b)),
            ]),
            OperationClass::Read,
        )])
        .demand_set_id()
    }

    fn empty_demand() -> crate::DemandSetId {
        ExportDemand::from_atoms([]).demand_set_id()
    }

    /// A two-export storeless interface: `add(int, int) -> int` (empty demand) and
    /// `lookup(int) -> Point?` where `Point` is a two-`int`-field record (a read
    /// demand). Fixed ids so the KAT is reproducible.
    fn sample() -> Result<Interface, InterfaceError> {
        let point = RecordShape {
            fields: vec![
                FieldShape {
                    name: "x".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
                FieldShape {
                    name: "y".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
            ],
        };
        let add = ExportSignature {
            id: ExportId::of_local("main", "add"),
            params: vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Int),
            ],
            ret: ImageType::scalar(Scalar::Int),
            demand_id: empty_demand(),
        };
        let lookup = ExportSignature {
            id: ExportId::of_local("main", "lookup"),
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Record {
                idx: 0,
                optional: true,
            },
            demand_id: demand_a(),
        };
        Interface::build([add, lookup], &[point], &[])
    }

    #[test]
    fn kind_is_sixteen_bytes_and_distinct_from_the_other_kinds() {
        assert_eq!(INTERFACE_ID_KIND.len(), 16);
        for other in [
            crate::export_id::EXPORT_ID_KIND.as_slice(),
            crate::demand::DEMAND_SET_KIND.as_slice(),
            crate::durable_id::DURABLE_CONTRACT_KIND.as_slice(),
            crate::digest::IMAGE_DIGEST_KIND.as_slice(),
        ] {
            assert_ne!(INTERFACE_ID_KIND.as_slice(), other);
        }
    }

    /// Known-answer test for the frozen canonical payload of the sample interface.
    /// Freezing this hex pins the domain separation, length delimiting, export sort,
    /// transfer-type encoding, and demand-id composition so a later reader can
    /// reconstruct it independently. If this value must change, the interface
    /// identity contract has changed.
    #[test]
    fn interface_id_known_answer() {
        let iface = sample().expect("sample interface builds");
        assert_eq!(iface.interface_id().to_hex(), independent_id(&iface));
        assert_eq!(
            iface.interface_id().to_hex(),
            "0e1f515400c8ac8c7c9e4ea677af5ab668fb157eb73e0d02be08cb1efdbc6bc4",
        );
    }

    /// Independent-decoder reconstruction: a second, hand-written implementation of
    /// the construction reproduces the same 32 bytes, sharing no code with
    /// [`Interface::interface_id`].
    fn independent_id(iface: &Interface) -> String {
        fn enc_ttype(ty: &TransferType, out: &mut Vec<u8>) {
            match ty {
                TransferType::Unit => out.push(0x00),
                TransferType::Scalar(scalar) => {
                    out.push(0x01);
                    out.push(scalar.tag());
                }
                TransferType::Optional(inner) => {
                    out.push(0x02);
                    enc_ttype(inner, out);
                }
                TransferType::Product(fields) => {
                    out.push(0x03);
                    out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
                    for field in fields {
                        out.extend_from_slice(&(field.name.len() as u64).to_be_bytes());
                        out.extend_from_slice(field.name.as_bytes());
                        out.push(u8::from(field.required));
                        enc_ttype(&field.ty, out);
                    }
                }
                TransferType::Sum(variants) => {
                    out.push(0x04);
                    out.extend_from_slice(&(variants.len() as u16).to_be_bytes());
                    for variant in variants {
                        out.extend_from_slice(&(variant.name.len() as u64).to_be_bytes());
                        out.extend_from_slice(variant.name.as_bytes());
                        out.push(u8::from(variant.category));
                        out.extend_from_slice(&(variant.payload.len() as u16).to_be_bytes());
                        for leaf in &variant.payload {
                            enc_ttype(leaf, out);
                        }
                    }
                }
            }
        }

        // Descriptors must already be sorted by the builder; verify that here.
        let mut sorted = iface.descriptors().to_vec();
        sorted.sort_by(|a, b| a.id().bytes().cmp(b.id().bytes()));
        assert_eq!(sorted, iface.descriptors());

        let mut payload: Vec<u8> = Vec::new();
        payload.extend_from_slice(&(LOCAL_ROOT_LINEAGE.len() as u64).to_be_bytes());
        payload.extend_from_slice(LOCAL_ROOT_LINEAGE);
        payload.extend_from_slice(&(iface.descriptors().len() as u32).to_be_bytes());
        for descriptor in iface.descriptors() {
            let mut body = Vec::new();
            body.extend_from_slice(descriptor.id().bytes());
            body.extend_from_slice(descriptor.demand_id().bytes());
            body.extend_from_slice(&(descriptor.params().len() as u16).to_be_bytes());
            for param in descriptor.params() {
                enc_ttype(param, &mut body);
            }
            enc_ttype(descriptor.ret(), &mut body);
            payload.extend_from_slice(&(body.len() as u64).to_be_bytes());
            payload.extend_from_slice(&body);
        }
        let mut framed: Vec<u8> = Vec::new();
        framed.extend_from_slice(INTERFACE_ID_KIND);
        framed.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        framed.extend_from_slice(&payload);
        let bytes: [u8; 32] = Sha256::digest(&framed).into();
        let mut hex = String::with_capacity(64);
        for byte in bytes {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex nibble"));
            hex.push(char::from_digit(u32::from(byte & 0xf), 16).expect("hex nibble"));
        }
        hex
    }

    /// The interface id is over the export *set*: discovery order does not change it.
    #[test]
    fn interface_id_is_order_independent() {
        let iface = sample().expect("builds");
        // Rebuild with the exports supplied in the reverse order.
        let point = RecordShape {
            fields: vec![
                FieldShape {
                    name: "x".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
                FieldShape {
                    name: "y".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
            ],
        };
        let add = ExportSignature {
            id: ExportId::of_local("main", "add"),
            params: vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Int),
            ],
            ret: ImageType::scalar(Scalar::Int),
            demand_id: empty_demand(),
        };
        let lookup = ExportSignature {
            id: ExportId::of_local("main", "lookup"),
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Record {
                idx: 0,
                optional: true,
            },
            demand_id: demand_a(),
        };
        let reversed = Interface::build([lookup, add], &[point], &[]).expect("builds");
        assert_eq!(iface.interface_id(), reversed.interface_id());
    }

    /// A body-only edit — no signature and no demand change — leaves the id fixed,
    /// while any signature or demand change moves it.
    #[test]
    fn signature_and_demand_changes_move_the_id_body_does_not() {
        let base = sample().expect("builds").interface_id();

        // Same signatures and demands (a body edit changes neither): same id.
        assert_eq!(base, sample().expect("builds").interface_id());

        // A changed parameter type moves the id.
        let point = RecordShape {
            fields: vec![
                FieldShape {
                    name: "x".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
                FieldShape {
                    name: "y".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
            ],
        };
        let add_wider = ExportSignature {
            id: ExportId::of_local("main", "add"),
            params: vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Text),
            ],
            ret: ImageType::scalar(Scalar::Int),
            demand_id: empty_demand(),
        };
        let lookup = ExportSignature {
            id: ExportId::of_local("main", "lookup"),
            params: vec![ImageType::scalar(Scalar::Int)],
            ret: ImageType::Record {
                idx: 0,
                optional: true,
            },
            demand_id: demand_a(),
        };
        let retyped = Interface::build(
            [add_wider, lookup.clone()],
            std::slice::from_ref(&point),
            &[],
        )
        .expect("builds");
        assert_ne!(base, retyped.interface_id());

        // A changed field name (a record signature change) moves the id.
        let point_renamed = RecordShape {
            fields: vec![
                FieldShape {
                    name: "x".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
                FieldShape {
                    name: "z".to_string(),
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                },
            ],
        };
        let add = ExportSignature {
            id: ExportId::of_local("main", "add"),
            params: vec![
                ImageType::scalar(Scalar::Int),
                ImageType::scalar(Scalar::Int),
            ],
            ret: ImageType::scalar(Scalar::Int),
            demand_id: empty_demand(),
        };
        let renamed_field =
            Interface::build([add.clone(), lookup.clone()], &[point_renamed], &[]).expect("builds");
        assert_ne!(base, renamed_field.interface_id());

        // A changed demand (same signatures) moves the id.
        let lookup_more_demand = ExportSignature {
            demand_id: {
                ExportDemand::from_atoms([
                    DemandAtom::new(
                        SemanticPath::from_steps(vec![
                            SemanticStep::new(SemanticStepKind::Application, ledger(0x0a)),
                            SemanticStep::new(SemanticStepKind::Placement, ledger(0x0b)),
                        ]),
                        OperationClass::Read,
                    ),
                    DemandAtom::new(
                        SemanticPath::from_steps(vec![
                            SemanticStep::new(SemanticStepKind::Application, ledger(0x0a)),
                            SemanticStep::new(SemanticStepKind::Placement, ledger(0x0b)),
                        ]),
                        OperationClass::Write,
                    ),
                ])
                .demand_set_id()
            },
            ..lookup
        };
        let redemanded =
            Interface::build([add, lookup_more_demand], &[point], &[]).expect("builds");
        assert_ne!(base, redemanded.interface_id());
    }

    /// A signature reaching a collection is rejected with a typed, positioned error.
    #[test]
    fn a_collection_in_a_signature_is_rejected() {
        let id = ExportId::of_local("main", "listy");
        let err = Interface::build(
            [ExportSignature {
                id,
                params: vec![ImageType::scalar(Scalar::Int)],
                ret: ImageType::Collection {
                    idx: 0,
                    optional: false,
                },
                demand_id: empty_demand(),
            }],
            &[],
            &[],
        )
        .expect_err("a collection return is not a transfer type");
        assert_eq!(
            err,
            InterfaceError::TransferTypeExcluded {
                export: id,
                position: SignaturePosition::Return,
                kind: ExcludedKind::Collection,
            }
        );

        // A collection nested inside a record field is rejected just as a top-level
        // collection is — reachability, not only the outermost type, is checked.
        let holder = RecordShape {
            fields: vec![FieldShape {
                name: "items".to_string(),
                ty: ImageType::Collection {
                    idx: 0,
                    optional: false,
                },
                required: true,
            }],
        };
        let id2 = ExportId::of_local("main", "holds");
        let err2 = Interface::build(
            [ExportSignature {
                id: id2,
                params: vec![ImageType::Record {
                    idx: 0,
                    optional: false,
                }],
                ret: ImageType::Unit,
                demand_id: empty_demand(),
            }],
            &[holder],
            &[],
        )
        .expect_err("a collection reached through a record is excluded");
        assert!(matches!(
            err2,
            InterfaceError::TransferTypeExcluded {
                position: SignaturePosition::Param(0),
                kind: ExcludedKind::Collection,
                ..
            }
        ));
    }

    /// A signature whose structural expansion exceeds the node budget is rejected,
    /// rather than materializing an exponential tree. A chain of many-fielded records
    /// (each field the next record) is acyclic but expands geometrically.
    #[test]
    fn an_over_budget_signature_is_rejected() {
        // Build records r0..r_{n-1}: each r_i has `width` fields all of type r_{i+1};
        // the last record is a single scalar field. The expansion of r0 is
        // width^(n-1) leaves — far past the budget for modest width/n.
        let width = 8usize;
        let depth = 8usize;
        let mut records: Vec<RecordShape> = Vec::new();
        for level in 0..depth {
            let field_ty = if level + 1 < depth {
                ImageType::Record {
                    idx: (level + 1) as u16,
                    optional: false,
                }
            } else {
                ImageType::scalar(Scalar::Int)
            };
            records.push(RecordShape {
                fields: (0..width)
                    .map(|f| FieldShape {
                        name: format!("f{f}"),
                        ty: field_ty,
                        required: true,
                    })
                    .collect(),
            });
        }
        let id = ExportId::of_local("main", "deep");
        let err = Interface::build(
            [ExportSignature {
                id,
                params: vec![ImageType::Record {
                    idx: 0,
                    optional: false,
                }],
                ret: ImageType::Unit,
                demand_id: empty_demand(),
            }],
            &records,
            &[],
        )
        .expect_err("the expansion exceeds the node budget");
        assert_eq!(err, InterfaceError::SignatureTooComplex { export: id });
        // Sanity: the budget is the reason, and a shallow interface stays well under it.
        assert!(width.pow((depth - 1) as u32) > MAX_INTERFACE_TRANSFER_NODES);
    }

    /// A sum transfer type round-trips through the encoding: `Option[int]` as a
    /// two-variant enum contributes its variant names and payloads to the id.
    #[test]
    fn a_sum_signature_builds_and_is_stable() {
        let option_int = EnumShape {
            variants: vec![
                VariantShape {
                    name: "none".to_string(),
                    category: false,
                    payload: vec![],
                },
                VariantShape {
                    name: "some".to_string(),
                    category: false,
                    payload: vec![ImageType::scalar(Scalar::Int)],
                },
            ],
        };
        let id = ExportId::of_local("main", "maybe");
        let build = |shape: &EnumShape| {
            Interface::build(
                [ExportSignature {
                    id,
                    params: vec![],
                    ret: ImageType::Enum {
                        idx: 0,
                        optional: false,
                    },
                    demand_id: empty_demand(),
                }],
                &[],
                std::slice::from_ref(shape),
            )
            .expect("builds")
            .interface_id()
        };
        let base = build(&option_int);
        assert_eq!(base, build(&option_int));

        // Renaming a variant moves the id.
        let renamed = EnumShape {
            variants: vec![
                VariantShape {
                    name: "nothing".to_string(),
                    category: false,
                    payload: vec![],
                },
                VariantShape {
                    name: "some".to_string(),
                    category: false,
                    payload: vec![ImageType::scalar(Scalar::Int)],
                },
            ],
        };
        assert_ne!(base, build(&renamed));
    }

    /// The `LP` framing keeps the payload self-delimiting: a helper sanity check that
    /// `push_lp` prefixes the big-endian length.
    #[test]
    fn lp_frames_length_then_bytes() {
        let mut out = Vec::new();
        push_lp(&mut out, b"hi");
        assert_eq!(out, [0, 0, 0, 0, 0, 0, 0, 2, b'h', b'i']);
    }

    /// A deterministic xorshift64 PRNG for the property test — no external crate.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }

        fn scalar(&mut self) -> Scalar {
            match self.below(7) {
                0 => Scalar::Int,
                1 => Scalar::Bool,
                2 => Scalar::Text,
                3 => Scalar::Bytes,
                4 => Scalar::Date,
                5 => Scalar::Instant,
                _ => Scalar::Duration,
            }
        }
    }

    /// A random bare type referencing only records with index `> after_record` and
    /// enums with index `> after_enum`, so the value graph is acyclic and the
    /// structural expansion always terminates. `allow_collection` occasionally emits a
    /// collection (a top-level signature type may, a nested leaf may not) to exercise
    /// the rejection path.
    fn random_type(
        rng: &mut Rng,
        record_count: usize,
        enum_count: usize,
        after_record: usize,
        after_enum: usize,
        allow_collection: bool,
    ) -> ImageType {
        let record_room = record_count.saturating_sub(after_record + 1);
        let enum_room = enum_count.saturating_sub(after_enum + 1);
        let optional = rng.next() & 1 == 0;
        // Weighted choice among the reachable constructors.
        let mut choices: Vec<u8> = vec![0]; // scalar always available
        if record_room > 0 {
            choices.push(1);
        }
        if enum_room > 0 {
            choices.push(2);
        }
        if allow_collection {
            choices.push(3);
        }
        match choices[rng.below(choices.len())] {
            0 => ImageType::Scalar {
                scalar: rng.scalar(),
                optional,
            },
            1 => ImageType::Record {
                idx: (after_record + 1 + rng.below(record_room)) as u16,
                optional,
            },
            2 => ImageType::Enum {
                idx: (after_enum + 1 + rng.below(enum_room)) as u16,
                optional,
            },
            _ => ImageType::Collection {
                idx: 0,
                optional: false,
            },
        }
    }

    /// Build random acyclic record and enum tables plus a small export set, then
    /// assert the interface invariants: `build` never panics, its result is
    /// deterministic, its id agrees with an independent recomputation, and the id is
    /// invariant under export reordering.
    #[test]
    fn interface_id_is_deterministic_order_free_and_independently_reproducible() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for _ in 0..600 {
            let record_count = rng.below(4);
            let enum_count = rng.below(4);
            let records: Vec<RecordShape> = (0..record_count)
                .map(|i| RecordShape {
                    fields: (0..rng.below(4))
                        .map(|f| FieldShape {
                            name: format!("f{f}"),
                            ty: random_type(
                                &mut rng,
                                record_count,
                                enum_count,
                                i,
                                enum_count,
                                false,
                            ),
                            required: rng.next() & 1 == 0,
                        })
                        .collect(),
                })
                .collect();
            let enums: Vec<EnumShape> = (0..enum_count)
                .map(|j| EnumShape {
                    variants: (0..1 + rng.below(3))
                        .map(|v| VariantShape {
                            name: format!("v{v}"),
                            category: false,
                            payload: (0..rng.below(3))
                                .map(|_| {
                                    random_type(
                                        &mut rng,
                                        record_count,
                                        enum_count,
                                        record_count,
                                        j,
                                        false,
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect();
            let export_count = 1 + rng.below(4);
            let signatures: Vec<ExportSignature> = (0..export_count)
                .map(|k| ExportSignature {
                    id: ExportId::of_local("m", &format!("f{k}")),
                    params: (0..rng.below(4))
                        .map(|_| {
                            random_type(
                                &mut rng,
                                record_count,
                                enum_count,
                                usize::MAX - 1,
                                usize::MAX - 1,
                                true,
                            )
                        })
                        .map(|ty| clamp_refs(ty, record_count, enum_count))
                        .collect(),
                    ret: clamp_refs(
                        random_type(
                            &mut rng,
                            record_count,
                            enum_count,
                            usize::MAX - 1,
                            usize::MAX - 1,
                            true,
                        ),
                        record_count,
                        enum_count,
                    ),
                    demand_id: if rng.next() & 1 == 0 {
                        empty_demand()
                    } else {
                        demand_a()
                    },
                })
                .collect();

            let first = Interface::build(signatures.clone(), &records, &enums);
            let second = Interface::build(signatures.clone(), &records, &enums);
            match (&first, &second) {
                (Ok(a), Ok(b)) => {
                    // Deterministic id.
                    assert_eq!(a.interface_id(), b.interface_id());
                    // Independent recomputation agrees with the builder.
                    assert_eq!(a.interface_id().to_hex(), independent_id(a));
                    // Reordering the supplied exports does not change the id.
                    let mut reversed = signatures.clone();
                    reversed.reverse();
                    let reordered =
                        Interface::build(reversed, &records, &enums).expect("reorder builds");
                    assert_eq!(a.interface_id(), reordered.interface_id());
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("build determinism violated: {first:?} vs {second:?}"),
            }
        }
    }

    /// A top-level signature type produced with `after_* = usize::MAX - 1` names no
    /// record/enum room, so `random_type` returns a scalar or a collection there. To
    /// let signatures also reference real records/enums, remap a top-level scalar to a
    /// random in-range record/enum reference some of the time. Keeps the fuzz input
    /// rich without risking an out-of-range index.
    fn clamp_refs(ty: ImageType, record_count: usize, enum_count: usize) -> ImageType {
        match ty {
            ImageType::Scalar { scalar, optional } if record_count + enum_count > 0 => {
                // Deterministically fold the scalar tag into an in-range reference.
                let span = record_count + enum_count;
                let pick = (scalar.tag() as usize) % (span + 1);
                if pick == span {
                    ImageType::Scalar { scalar, optional }
                } else if pick < record_count {
                    ImageType::Record {
                        idx: pick as u16,
                        optional,
                    }
                } else {
                    ImageType::Enum {
                        idx: (pick - record_count) as u16,
                        optional,
                    }
                }
            }
            other => other,
        }
    }
}
