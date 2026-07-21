//! The sealed, verified image the VM accepts (design §A/§E phase 6).
//!
//! [`VerifiedImage`] has all-private fields and a single constructor,
//! [`crate::verify`]. Sealing produces a typed instruction tape per function:
//! [`SealedInstr`] values with jumps resolved to instruction indices and operands
//! as bounds-checked typed handles, so the VM never sees raw opcode bytes and a
//! verifier/VM width or discriminant disagreement is unrepresentable.

use std::rc::Rc;

use marrow_image::{
    DemandSetId, DurableContractDescriptor, DurableContractId, DurableIndexComponent, ExportDemand,
    ExportId, ImageId, ImageType, LedgerIdBytes, OperationClass, Scalar, SemanticNode,
    SemanticPath, SemanticTarget,
};

/// A function's position in a [`VerifiedImage`]'s function table. A typed handle so a
/// function index cannot be confused with the many other `u16` indices a sealed image
/// carries (local slots, const indices, field/site/root/group handles): the VM's run
/// entry points and the image's function accessors take a `FunctionIndex`, and the
/// only ways to obtain one are [`SealedExport::function`], [`SealedTestEntry::func`],
/// or an explicit [`FunctionIndex::new`] over a value the caller vouches is a function
/// index. It is meaningful only within its own image's [`ImageId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionIndex(u16);

impl FunctionIndex {
    /// Wrap a raw function-table position. The caller asserts `raw` indexes the
    /// image's function table; the accessors that return a `FunctionIndex` are the
    /// safe sources.
    pub fn new(raw: u16) -> Self {
        FunctionIndex(raw)
    }

    /// The raw function-table position.
    pub fn get(self) -> u16 {
        self.0
    }

    /// The position as a table subscript.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A resolved constant value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedConst {
    Int(i64),
    Bool(bool),
    Text(Rc<str>),
    /// A temporal scalar: a `date` (days since the epoch), an `instant` (signed
    /// nanoseconds since the epoch), or a `duration` (signed nanoseconds).
    Date(i32),
    Instant(i128),
    Duration(i128),
}

/// A verified instruction with typed, bounds-checked operands. Jump targets are
/// instruction indices into the owning function's tape. This enum grows one slice
/// at a time; an opcode whose vertical has not landed is rejected at verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedInstr {
    /// Push constant `consts[idx]`.
    ConstLoad(u16),
    /// Push local slot `l`.
    LocalGet(u16),
    /// Pop into local slot `l`.
    LocalSet(u16),
    /// Discard the top of the stack.
    Pop,
    /// Return the frame's value (or nothing for a Unit return).
    Return,
    /// Unconditional jump to the instruction at this tape index.
    Jump(usize),
    /// Pop a bool; if false, jump to this tape index, else fall through.
    JumpIfFalse(usize),
    IntAdd,
    IntSub,
    IntMul,
    IntRem,
    IntDiv,
    IntNeg,
    BoolNot,
    IntLt,
    IntLe,
    IntGt,
    IntGe,
    EqInt,
    EqBool,
    EqText,
    TextConcat,
    TextLt,
    TextLe,
    TextGt,
    TextGe,
    EqBytes,
    BytesLt,
    BytesLe,
    BytesGt,
    BytesGe,
    /// `value → string`: render an interpolable value (scalar, enum, or identity) to
    /// its canonical text. `string → bytes` (UTF-8) is the reverse byte conversion.
    ConvString,
    ConvBytesText,
    /// The closed pure text floor: `string → bool`, `string, string → bool`,
    /// `string → string`.
    TextIsEmpty,
    TextContains,
    TextTrim,
    /// The collection-returning text floor. `split`/`lines` produce a `List[string]`
    /// of COLLTYPES index `_0` (which the verifier proves names a `List[string]`),
    /// faulting `run.collection_limit` on a bound excess; `join` concatenates a
    /// `List[string]` with a separator into a string, faulting `run.text_limit` on a
    /// concatenation-ceiling excess.
    TextSplit(u16),
    TextLines(u16),
    TextJoin,
    /// Temporal equality and order over two bare temporals of the same type,
    /// producing a bool. The order agrees with the kernel key-codec byte order.
    EqDate,
    DateLt,
    DateLe,
    DateGt,
    DateGe,
    EqInstant,
    InstantLt,
    InstantLe,
    InstantGt,
    InstantGe,
    EqDuration,
    DurationLt,
    DurationLe,
    DurationGt,
    DurationGe,
    /// The closed temporal arithmetic floor. Each faults `run.temporal_overflow`
    /// when its result would leave the supported day/nanosecond domain.
    DateAddDays,
    DateDaysBetween,
    DurationAdd,
    DurationSub,
    InstantAddDuration,
    InstantSubDuration,
    /// Checked arithmetic: `_0` is the fault-handler tape index. On overflow the op
    /// transfers there (carrying the post-pop stack) instead of faulting; otherwise
    /// it pushes the int result and falls through.
    IntAddChecked(usize),
    IntSubChecked(usize),
    IntMulChecked(usize),
    IntNegChecked(usize),
    IntDivChecked(usize),
    IntRemChecked(usize),
    /// Peek the int on top of the stack; fault `run.range` when it lies outside
    /// the inclusive `[lo, hi]` interval, else fall through with no stack
    /// effect. Decode rejects an empty interval (`lo > hi`).
    RangeGuard {
        lo: i64,
        hi: i64,
    },
    /// Construct a record of type index `_0` from its field values popped in
    /// reverse (f0 pushed first). The field count and per-field required flag come
    /// from the sealed record type.
    RecordNew(u16),
    /// Read field `_0` of the record on the stack; required-ness comes from the
    /// record value's sealed type (bare value vs optional).
    FieldGet(u16),
    /// `[record, value] → [record]`: store the bare value into field `_0`'s slot
    /// present, returning the updated record. Local product assignment.
    FieldSet(u16),
    /// `[record] → [record]`: clear field `_0`'s slot to vacant, returning the
    /// updated record. Only a sparse field is unset (the verifier proves it).
    FieldUnset(u16),
    /// Coerce a bare value into an optional (`Some`).
    SomeWrap,
    /// Push a vacant optional of the given optional type (a scalar or an enum).
    /// The runtime pushes only the vacant marker; the operand records the
    /// verifier-checked type — an optional scalar, or an optional enum for a
    /// defaulted sparse enum field.
    VacantLoad(ImageType),
    /// Construct enum `enum_idx`'s variant `variant` from its dense scalar payload
    /// popped in reverse (p0 pushed first).
    EnumConstruct {
        enum_idx: u16,
        variant: u16,
    },
    /// Pop an enum value and push its variant index as a bare int.
    EnumTag,
    /// Read payload leaf `field` of `variant` from the enum value on the stack,
    /// pushing its bare scalar. The variant operand types the leaf; the VM faults
    /// if the runtime value carries a different variant.
    EnumPayloadGet {
        variant: u16,
        field: u16,
    },
    /// `E, E → bool`: exact equality of two values of the same enum.
    EqEnum,
    /// `Id, Id → bool`: equality of two entry identities of the same store root.
    EqId,
    /// `[k0, …, k(cols-1)] → Id`: construct store root `root`'s entry identity from
    /// `cols` bare key scalars popped in reverse (k0 pushed first, key-column order).
    MakeIdentity {
        root: u16,
        cols: u16,
    },
    /// `Id → [k0, …, k(cols-1)]`: spread an identity into its `cols` key scalars,
    /// root-first, so a `^root[id]` dereference reuses the ordinary keyed entry read.
    IdentityKeyPath(u16),
    /// Pop an optional; if present, push its bare value and fall through, else jump
    /// to this tape index. The only way to obtain a bare value from an optional.
    BranchPresent(usize),
    /// Fault with `run.unreachable`, carrying the static text at const index `_0`.
    /// Terminates the frame; it never falls through.
    Unreachable(u16),
    /// Fault with `run.todo`, carrying the static text at const index `_0`. A deferred
    /// path; like `Unreachable` it terminates the frame and never falls through.
    Todo(u16),
    /// Pop a bool; on false fault with `run.assert` at this instruction's span, else
    /// fall through. The verifier admits it only in a test-entry function.
    Assert,
    /// Call function `_0` directly: pop its arguments (a0 pushed first, lands in
    /// callee local 0) and push its return value (nothing for a Unit return).
    Call(u16),
    /// `K → bool`: whether the cell site `_0` addresses is present.
    DurExists(u16),
    /// `ancestor-keys → bool`: whether the family the whole-entry site `_0` names has at
    /// least one payload-bearing immediate child. Pops the ancestor key-path (none for a
    /// root site, `[root_key]` for a single-level branch site); names no child key.
    DurFamilyExists(u16),
    /// `K → T?`: read field site `_0`.
    DurReadField(u16),
    /// `K → Rec?`: read the whole entry at site `_0`.
    DurReadEntry(u16),
    /// `K, T →`: set the required field site `_0` (transaction-region only).
    DurSetRequired(u16),
    /// `K, T? →`: set (present) or clear (vacant) the sparse field site `_0`.
    DurSetSparse(u16),
    /// `T? →`: set (present) or clear (vacant) the sparse field `site`, reading the
    /// containing entry's key-path from local slots `key_slots` (root-first) and
    /// asserting that entry is present. The strict form: emitted only for a sparse set
    /// through a `place` a presence fact dominates, so the verifier's place-slot
    /// presence lattice proves the `key_slots` tuple's entry is present on every path
    /// here, and the kernel faults `run.corruption` if the marker is absent (defense in
    /// depth).
    DurSetSparsePresent {
        site: u16,
        key_slots: Vec<u16>,
    },
    /// `K, Rec →`: create the entry at site `_0` (algebra `create`).
    DurCreateEntry(u16),
    /// `K, Rec →`: replace the entry at site `_0` (algebra `replace`).
    DurReplaceEntry(u16),
    /// `K →`: erase the sparse field site `_0` (no-op on absent).
    DurEraseField(u16),
    /// `K →`: erase the entry at site `_0` (no-op on absent).
    DurEraseEntry(u16),
    /// `K → Rec?`: read the whole materialized value of the group the `GroupEntry` site
    /// `_0` names.
    DurReadGroup(u16),
    /// `K, Rec →`: replace the group the `GroupEntry` site `_0` names, group-scoped
    /// payload-only (transaction-region only).
    DurReplaceGroup(u16),
    /// `K →`: erase the group the `GroupEntry` site `_0` names — clears only that
    /// group's leaves (no-op on an absent entry).
    DurEraseGroup(u16),
    /// The bounded nested traversal `for … at most N … on more`. Freeze the first
    /// `limit` immediate keys of the layer the whole-entry site `_ .site` belongs to —
    /// the root's entry family (a root site) or a keyed branch family under a fixed
    /// parent (a branch site) — then push the frozen key list and whether a further key
    /// existed. Stack effect `[ancestor-keys, from?] → List[K], Bool`: pop the layer's
    /// ancestor key-path (a root site pops none; a single-level branch site pops
    /// `[root_key]`), then the inclusive `from` key of the traversed key type `K` when
    /// `from` is set, and push `List[K]` then `Bool`. `limit` is the positive
    /// compile-time `N`, bounded by `MAX_TRAVERSAL_BOUND`; `list_ty` is the COLLTYPES
    /// index the verifier proved names exactly `List[K]`, the frozen keys' materialized
    /// list value (obeying the one collection aggregate-byte ceiling).
    DurIterateBounded {
        site: u16,
        limit: u32,
        from: bool,
        list_ty: u16,
    },
    /// The bounded progressive scan of a nonunique managed index (a `DurIterateBounded`
    /// mirror). Stack effect `[prefix-keys, from?] → List[K], Bool`: pop the index's
    /// leading field components (in projection order) then the inclusive `from` key of the
    /// scanned component when set; push the frozen `List[K]` of scanned identity keys then
    /// `Bool`. `site` names the index scan site; the projection is read from the sealed
    /// index the site names.
    DurIndexScan {
        site: u16,
        limit: u32,
        from: bool,
        list_ty: u16,
    },
    /// The exact complete-key lookup of a unique managed index. Stack effect
    /// `[projection-keys] → Id(^root)?`: pop the whole projection (one key per component,
    /// projection order) and push the optional source identity.
    DurIndexLookup(u16),
    /// The presence half of [`DurIndexLookup`] — the unique-index arm of `exists`. Stack
    /// effect `[projection-keys] → bool`: pop the whole projection (one key per component,
    /// projection order) and push whether a matching entry exists, without materializing its
    /// identity. `site` names the same unique-index lookup site.
    DurIndexExists(u16),
    /// Open the export's single transaction region.
    TxnBegin,
    /// Close the export's single transaction region.
    TxnCommit,
    /// Push an empty `List` of COLLTYPES index `_0`.
    ListNew(u16),
    /// `[list, value] → [list]`: append the bare element, faulting
    /// `run.collection_limit` on a bound excess.
    ListAppend,
    /// `[list] → [int]`: the element count.
    ListLen,
    /// `[list, int] → [element]`: the bare element at the 0-based index.
    ListGet,
    /// `[list, int] → [element?]`: the 1-based local bracket read `xs[i]`, absent
    /// outside `1..=length`. No out-of-bounds fault class.
    ListIndex,
    /// Push an empty `Map` of COLLTYPES index `_0`.
    MapNew(u16),
    /// `[map, key, value] → [map]`: insert or replace by key in key order.
    MapInsert,
    /// `[map, key] → [map]`: remove the key if present, idempotent if absent.
    MapRemove,
    /// `[map, key] → [value?]`: the value at `key`, or absent.
    MapGet,
    /// `[map] → [int]`: the entry count.
    MapLen,
    /// `[map, int] → [key]`: the bare key at the 0-based position in key order.
    MapKeyAt,
    /// `[map, int] → [value]`: the bare value at the 0-based position in key order.
    MapValueAt,
}

/// The resolved physical form of a durable operation site's closed
/// [`marrow_image::SemanticTarget`], as the verifier re-derives it from the site's
/// semantic path: `WholePayload` over a keyed placement, or `FieldLeaf` carrying the
/// resolved index of the field within its root's record. The index is a verifier
/// derivation from the resolved graph node, never a value trusted from the image.
///
/// A branch target names its node by a *branch path*: the per-level branch indices from
/// the root down to the addressed branch node, each an index into that level's
/// declaration-ordered branch list ([`SealedRoot::branches`], then each
/// [`SealedBranch::branches`]). A single-element path names a direct branch of the root;
/// a longer path names a branch nested one level deeper per element. A branch node's
/// key-path is the root key followed by one key per path element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedSiteTarget {
    WholePayload,
    FieldLeaf(u16),
    /// The whole payload of a keyed branch entry, named by its branch path (per-level
    /// branch indices into the declaration-ordered branch list at each level). Its
    /// operations address the `(1 + path.len())`-element key-path
    /// `[root_key, branch_key, …]`.
    BranchEntry(Box<[u16]>),
    /// One field leaf of a keyed branch entry: the branch node's branch path and the
    /// field's index within that branch's materialized record. Its field-exact
    /// operations address the `(1 + path.len())`-element key-path
    /// `[root_key, branch_key, …]`, one or more levels below the root.
    BranchField {
        branch: Box<[u16]>,
        field: u16,
    },
    /// The whole materialized value of one unkeyed `group` node, named by the group's
    /// index into its root's declaration-ordered [`SealedRoot::groups`]. Its operations
    /// address the root's key-path (a group is a value unit of the root entry, not a
    /// keyed child), and read/replace/erase scope to the group's own field set under the
    /// group-scoped payload-only law.
    GroupEntry(u16),
    /// A progressive scan of a nonunique managed index, by the index's position in the
    /// image-wide index table. Its `DurIndexScan` holds the index's leading field
    /// components as a stack prefix and yields the trailing identity component.
    IndexScan(u16),
    /// An exact lookup of a unique managed index, by the index's position in the
    /// image-wide index table. Its `DurIndexLookup` pops the whole projection and yields
    /// the optional source identity.
    IndexLookup(u16),
}

/// A verified durable operation site. The verifier reconstructs it by resolving the
/// image's site path against its own derived node set and re-deriving the executable
/// coordinates — it trusts no compiler-side site summary.
///
/// A site is [`SealedSite::Flat`] exactly when the kernel can execute over it: a
/// whole-payload, keyed-branch-entry, or field-leaf site on a flat-executable keyed
/// root — a root with one or more key columns whose members are all fields or simple
/// keyed branches (no group at any level). Widened (record/enum) field values,
/// composite key tuples, and keyed branches nested to any depth all execute. Every
/// other resolved site — a singleton (keyless) root, a group-bearing root (a group at
/// any level, or a branch enclosing one), or a managed-index read — is
/// [`SealedSite::Parked`]: its identity is complete and its path and target agree with
/// the reconstructed graph, but physical execution stays parked (index traversal lands
/// at E05; groups at their lane). A durable opcode may reference only a `Flat` site; a
/// reference to a `Parked` site is refused in phase 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealedSite {
    /// Executable on the flat keyed root: the root index it resolved to
    /// and the whole-payload or resolved-field-index target.
    Flat { root: u16, target: SealedSiteTarget },
    /// A sealed but not-yet-executable site over the wider durable graph. It carries
    /// the resolved node path and target so the widened kernel derives its physical
    /// coordinates at E01 without re-parsing the image.
    Parked {
        path: SemanticPath,
        target: SemanticTarget,
    },
}

/// A verified durable root: one placement of a record type over its ordered key
/// tuple. The tuple is empty for a singleton root and holds one or more
/// orderable durable-key scalars for a keyed root. The executable subset served
/// by the single-root kernel is the *flat* keyed root — one or more
/// key columns and no groups; a wider key arity or a resource declaring a
/// group or branch (`has_extras`) carries identity but rejects at its operation
/// sites (rechecked during flow validation).
#[derive(Debug, Clone)]
pub struct SealedRoot {
    pub(crate) name: Rc<str>,
    pub(crate) keys: Vec<Scalar>,
    pub(crate) record: u16,
    /// Whether the root's member tree holds a shape the flat kernel
    /// cannot execute: a static `group` namespace, a nested or composite-key branch,
    /// or a widened (non-scalar) field. A scalar-field keyed branch of
    /// scalar fields does *not* count — it is executable (E03), so a root of scalar
    /// fields and such branches is flat-executable.
    pub(crate) has_extras: bool,
    /// The root's scalar-field keyed branches, in declaration order, each
    /// carrying its own nested branches recursively. Populated only for a flat-executable
    /// root; empty otherwise, so a [`SealedSiteTarget::BranchEntry`] branch path into this
    /// tree is meaningful exactly when a branch site sealed executable.
    pub(crate) branches: Vec<SealedBranch>,
    /// The root's unkeyed `group` nodes, in declaration order, each carrying its own
    /// materialized record type. Populated only for a flat-executable root; empty
    /// otherwise, so a [`SealedSiteTarget::GroupEntry`] group index into this list is
    /// meaningful exactly when a group site sealed executable.
    pub(crate) groups: Vec<SealedGroup>,
}

/// A verified unkeyed `group` of a flat-executable root: its physical name and its
/// materialized record type index. Unlike a branch, a group is not a keyed child node:
/// it is a value unit of the containing entry, addressed by the entry's key-path, and
/// its record is the trailing group slot the root's own record ties to the group member.
#[derive(Debug, Clone)]
pub struct SealedGroup {
    pub(crate) name: Rc<str>,
    pub(crate) record: u16,
}

impl SealedGroup {
    /// The group's simple name, which the physical layer scopes its leaves by.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The group's materialized record type index.
    pub fn record(&self) -> u16 {
        self.record
    }
}

/// A verified keyed branch of a flat-executable root: its physical name, its ordered key
/// columns (one or more), its materialized record type index, and its own nested branches
/// in declaration order. The branch entry is a distinct durable node one
/// level below its parent, reusing the parent's marker/field topology; its whole-payload
/// operations address the parent's key-path extended with the branch key. The list is
/// recursive — a branch may declare keyed branches of its own — so a
/// [`SealedSiteTarget::BranchEntry`] branch path indexes this tree level by level.
#[derive(Debug, Clone)]
pub struct SealedBranch {
    pub(crate) name: Rc<str>,
    pub(crate) keys: Vec<Scalar>,
    pub(crate) record: u16,
    pub(crate) branches: Vec<SealedBranch>,
}

impl SealedBranch {
    /// The branch's simple name, which the physical layer keys its family by.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The branch's ordered key columns (one or more), the whole composite branch key.
    pub fn keys(&self) -> &[Scalar] {
        &self.keys
    }
    /// The branch entry's materialized record type index.
    pub fn record(&self) -> u16 {
        self.record
    }
    /// The branch's own nested branches, in declaration order.
    pub fn branches(&self) -> &[SealedBranch] {
        &self.branches
    }
}

impl SealedRoot {
    pub fn name(&self) -> &str {
        &self.name
    }
    /// The ordered key tuple: empty for a singleton root, one scalar per column
    /// otherwise.
    pub fn keys(&self) -> &[Scalar] {
        &self.keys
    }
    pub fn record(&self) -> u16 {
        self.record
    }
    /// Whether the resource declares a member shape the flat kernel cannot execute (a
    /// group, a nested/composite branch, or a widened field). A single-level
    /// scalar-field keyed branch is executable and does not set this.
    pub fn has_extras(&self) -> bool {
        self.has_extras
    }

    /// The root's executable single-level branches, in declaration order. Empty unless
    /// the root is flat-executable.
    pub fn branches(&self) -> &[SealedBranch] {
        &self.branches
    }

    /// The root's unkeyed groups, in declaration order. Empty unless the root is
    /// flat-executable.
    pub fn groups(&self) -> &[SealedGroup] {
        &self.groups
    }
}

/// One resolved component of a managed index's ordered projection: an identity `key`
/// column or a top-level `field` of the root, each named by its position rather than its
/// ledger id. The position is what the path kernel needs to build a physical index cell
/// key — a `Field` indexes the root's materialized record (matching
/// [`SealedRecordType::fields`] order), and a `Key` indexes the root's key tuple. The
/// verifier resolves it from the index's ledger-id [`DurableIndexComponent`] against the
/// decoded root at seal, so the kernel never re-resolves a ledger id. The
/// identity-projection view of the same component is [`SealedIndex::components`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedIndexComponent {
    /// An identity key column, by its index into the root's key tuple.
    Key(u16),
    /// A top-level field, by its index into the root's materialized record.
    Field(u16),
}

/// A verified managed index of a durable root: its stable `Index` ledger id, the
/// index of the root it belongs to, its `unique` flag, its ordered projection of
/// leaf references (each a top-level `field` or identity `key` of the same root), and the
/// same projection resolved to record/key positions the kernel maintains. The verifier
/// reconstructs it by re-resolving every projected leaf against the decoded root, so a
/// projection over a non-existent leaf never seals. An index has no application write
/// opcode; maintenance is kernel-owned at the write path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedIndex {
    pub(crate) id: LedgerIdBytes,
    pub(crate) root: u16,
    pub(crate) unique: bool,
    pub(crate) components: Vec<DurableIndexComponent>,
    pub(crate) projection: Vec<SealedIndexComponent>,
}

impl SealedIndex {
    /// The index's stable `Index` ledger identity — the sole `IndexId` a durable
    /// operation algebra `unique_index_collision` outcome reveals.
    pub fn id(&self) -> LedgerIdBytes {
        self.id
    }

    /// The index of the durable root this index belongs to.
    pub fn root(&self) -> u16 {
        self.root
    }

    /// Whether this is a unique index (a complete-key exact lookup yielding at most
    /// one source key) rather than a nonunique ordered index.
    pub fn unique(&self) -> bool {
        self.unique
    }

    /// The ordered projection as leaf identity references (each a top-level `field` or
    /// identity `key` of the root, by ledger id). This is the identity view; the path
    /// kernel consumes [`Self::projection`] instead.
    pub fn components(&self) -> &[DurableIndexComponent] {
        &self.components
    }

    /// The ordered projection resolved to record/key positions — the form the path kernel
    /// maintains, in the same order as [`Self::components`].
    pub fn projection(&self) -> &[SealedIndexComponent] {
        &self.projection
    }
}

/// A sealed record field: its name, bare value type, and whether it is required.
/// The type is a scalar for a durable-storable field or a closed enum for a
/// local-only value field. The name is carried so the path kernel can key
/// physical field leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedField {
    pub name: Rc<str>,
    pub ty: ImageType,
    pub required: bool,
}

/// A sealed record type: an ordered field list in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedRecordType {
    pub(crate) fields: Vec<SealedField>,
}

impl SealedRecordType {
    pub fn fields(&self) -> &[SealedField] {
        &self.fields
    }
}

/// One sealed enum variant: its member name, `category` flag, and dense payload in
/// declaration order. Each payload leaf is a bare (non-optional) [`ImageType`]: a
/// user `enum` member carries only bare scalars, while a built-in `Option`/`Result`
/// instantiation carries whatever concrete type its argument monomorphized to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedVariant {
    pub name: Rc<str>,
    pub category: bool,
    pub payload: Vec<ImageType>,
}

/// A sealed collection value type: a finite `List<T>` or ordered `Map<K, V>`. The
/// element/key/value types are bare [`ImageType`]s (possibly `Collection` tags into
/// an earlier row); the verifier proved every referenced index in range and that a
/// `Map` key is a bare scalar key type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealedCollectionType {
    List { elem: ImageType },
    Map { key: ImageType, value: ImageType },
}

/// A sealed enum type: an ordered variant list in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedEnumType {
    pub(crate) name: Rc<str>,
    pub(crate) variants: Vec<SealedVariant>,
}

impl SealedEnumType {
    /// The enum's declared name, used to render an enum value.
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn variants(&self) -> &[SealedVariant] {
        &self.variants
    }
}

/// A function's return shape, used to check `Return` and to render the result. A
/// record return names a sealed record type by index (a dense `struct` value); the
/// verifier proved the index in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetShape {
    Unit,
    Scalar { scalar: Scalar, optional: bool },
    Record { idx: u16, optional: bool },
    Enum { idx: u16, optional: bool },
    Collection { idx: u16, optional: bool },
    Identity { root: u16, optional: bool },
}

/// A source-position row: the instruction it maps and its 1-based line/column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanRow {
    pub instr_index: usize,
    pub line: u32,
    pub column: u32,
}

/// A sealed function body.
#[derive(Debug, Clone)]
pub struct SealedFunction {
    pub(crate) name: Rc<str>,
    pub(crate) source: Rc<str>,
    pub(crate) params: Vec<ImageType>,
    pub(crate) ret: RetShape,
    pub(crate) local_count: u16,
    pub(crate) instrs: Vec<SealedInstr>,
    pub(crate) spans: Vec<SpanRow>,
    pub(crate) max_stack: usize,
    pub(crate) mutating: bool,
}

impl SealedFunction {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn params(&self) -> &[ImageType] {
        &self.params
    }
    pub fn ret(&self) -> RetShape {
        self.ret
    }
    pub fn local_count(&self) -> u16 {
        self.local_count
    }
    pub fn instrs(&self) -> &[SealedInstr] {
        &self.instrs
    }
    pub fn max_stack(&self) -> usize {
        self.max_stack
    }
    pub fn is_mutating(&self) -> bool {
        self.mutating
    }

    /// The source line/column for the instruction at `pc`, using the greatest span
    /// offset ≤ `pc` (design §C SPANS lookup rule). Every function with code has a
    /// span at instruction 0, so a mapping always exists.
    pub fn span_at(&self, pc: usize) -> Option<(u32, u32)> {
        self.spans
            .iter()
            .rev()
            .find(|row| row.instr_index <= pc)
            .map(|row| (row.line, row.column))
    }
}

/// A public export: a stable [`ExportId`] bound to a function, with its
/// verifier-reconstructed durable demand and the image-local set of operation sites
/// its call closure can reach. The image carries no export name, so an export is
/// addressed only by its verified id.
#[derive(Debug, Clone)]
pub struct SealedExport {
    pub(crate) id: ExportId,
    pub(crate) func: u16,
    pub(crate) mutating: bool,
    /// The stable atom set the verifier reconstructed from the sealed sites the
    /// export's call closure references. The single owner of this export's demand.
    pub(crate) demand: ExportDemand,
    /// The export's [`DemandSetId`], cached from `demand`. Stable across a body edit
    /// that preserves the atom set; changes when the atom set changes.
    pub(crate) demand_id: DemandSetId,
    /// The image-local indices of the operation sites the export's call closure can
    /// reach, ascending. Meaningful only within this image's [`ImageId`] — site
    /// indices are not a stable boundary identity and never enter the `DemandSetId`.
    pub(crate) reachable_sites: Vec<u16>,
}

impl SealedExport {
    /// The stable identity this export is addressed by.
    pub fn id(&self) -> ExportId {
        self.id
    }
    pub fn function(&self) -> FunctionIndex {
        FunctionIndex(self.func)
    }
    pub fn is_mutating(&self) -> bool {
        self.mutating
    }

    /// The verifier-reconstructed durable demand of this export: its stable atom set
    /// over semantic paths and operation classes. An input to the authority check,
    /// never a grant.
    pub fn demand(&self) -> &ExportDemand {
        &self.demand
    }

    /// The stable identity of this export's demand set. Separate from
    /// [`Self::id`] and the image id.
    pub fn demand_id(&self) -> DemandSetId {
        self.demand_id
    }

    /// The image-local operation sites this export's call closure can reach, in
    /// ascending index order. This is not stable demand — it is bound to this exact
    /// image and is never part of any identity.
    pub fn reachable_sites(&self) -> &[u16] {
        &self.reachable_sites
    }
}

/// A verified test entry: a report name bound to the zero-argument function
/// `marrow test` runs, plus the demand the verifier reconstructed from its call
/// closure. Unlike an export the name is a human report label, never an interface or
/// durable identity, and a test entry is never dispatched as an export. Its demand
/// is recorded in a table parallel to — and separate from — the export demand table
/// so an E01 ephemeral test attachment can bound the test's authority by the
/// test-image demand union.
/// How a test body reaches durable data, deciding the runtime that drives it. The
/// three kinds are disjoint: a body that performs a direct durable op and also
/// drives a transaction owner is refused by the verifier before this classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    /// No durable demand: runs with no attachment.
    Storeless,
    /// Performs durable operations directly in its body: runs against one harness
    /// session over a fresh attachment.
    DirectDurable,
    /// Reaches durable data only through calls: runs as a driver, where each call is
    /// its own invocation boundary against a fresh persistent attachment.
    Driver,
}

#[derive(Debug, Clone)]
pub struct SealedTestEntry {
    pub(crate) name: Rc<str>,
    pub(crate) func: u16,
    pub(crate) demand: ExportDemand,
    pub(crate) kind: TestKind,
}

impl SealedTestEntry {
    /// The report name (the `test "..."` title).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The image function index this test runs.
    pub fn func(&self) -> FunctionIndex {
        FunctionIndex(self.func)
    }

    /// The verifier-reconstructed durable demand of this test entry's call closure.
    /// Empty for a storeless test; nonempty for a durable test whose attachment E01
    /// bounds by the test-image union.
    pub fn demand(&self) -> &ExportDemand {
        &self.demand
    }

    /// How this test reaches durable data, deciding which runtime drives it.
    pub fn kind(&self) -> TestKind {
        self.kind
    }
}

/// The verified, sealed program image.
#[derive(Debug, Clone)]
pub struct VerifiedImage {
    pub(crate) image_id: ImageId,
    pub(crate) types: Vec<SealedRecordType>,
    pub(crate) enums: Vec<SealedEnumType>,
    pub(crate) collections: Vec<SealedCollectionType>,
    pub(crate) roots: Vec<SealedRoot>,
    pub(crate) indexes: Vec<SealedIndex>,
    pub(crate) sites: Vec<SealedSite>,
    pub(crate) durable_contract: DurableContractId,
    pub(crate) durable_descriptor: DurableContractDescriptor,
    pub(crate) consts: Vec<SealedConst>,
    pub(crate) functions: Vec<SealedFunction>,
    pub(crate) exports: Vec<SealedExport>,
    pub(crate) test_entries: Vec<SealedTestEntry>,
    /// Per-function reconstructed durable demand over its whole call closure,
    /// indexed by image function index. The same `Effects` owner that builds each
    /// export's demand builds this, so a test-body driver can open the session one
    /// export call requires without a second demand model. Derived, never
    /// serialized in the image.
    pub(crate) function_demands: Vec<ExportDemand>,
}

impl VerifiedImage {
    pub fn image_id(&self) -> ImageId {
        self.image_id
    }

    pub fn record_type(&self, index: u16) -> &SealedRecordType {
        &self.types[index as usize]
    }

    /// The sealed record types, indexed by image record-type index. Consumed by the
    /// CLI to render a returned record value's field names.
    pub fn record_types(&self) -> &[SealedRecordType] {
        &self.types
    }

    /// The sealed enum types, indexed by image enum index. Consumed by the CLI to
    /// render an enum value's declared and variant names.
    pub fn enums(&self) -> &[SealedEnumType] {
        &self.enums
    }

    /// The sealed collection value types, indexed by image COLLTYPES index. Consumed
    /// by the VM to type collection operands and by the CLI to render a value.
    pub fn collections(&self) -> &[SealedCollectionType] {
        &self.collections
    }

    /// The sealed collection type at `index`. The verifier proved every operand and
    /// return index in range.
    pub fn collection_type(&self, index: u16) -> SealedCollectionType {
        self.collections[index as usize]
    }

    /// The durable roots, in declaration order (up to `MAX_ROOTS`).
    pub fn roots(&self) -> &[SealedRoot] {
        &self.roots
    }

    /// The durable-contract identity of this image's durable graph, independently
    /// recomputed by the verifier and proven to match the bytes the image carried.
    /// A later store-admission phase binds an activated store to this id.
    pub fn durable_contract(&self) -> DurableContractId {
        self.durable_contract
    }

    /// Every durable graph node paired with its derived [`SemanticPath`]
    /// ([`marrow_image::SemanticPath`]), reconstructed from the decoded graph. The
    /// verifier rebuilds the same descriptor it recomputes the contract id from, so
    /// these paths are the verifier's independent derivation — identical to the
    /// compiler's for a graph that verifies — not a trusted transfer of compiler
    /// output. A rename that only moves ledger anchors leaves every path unchanged.
    pub fn semantic_nodes(&self) -> Vec<SemanticNode> {
        self.durable_descriptor.semantic_nodes()
    }

    /// The durable operation sites, indexed by image site index.
    pub fn sites(&self) -> &[SealedSite] {
        &self.sites
    }

    /// The verified managed indexes, in image declaration order. Each is a narrow
    /// compiler-maintained ordered projection of a durable root; the verifier
    /// reconstructed its projection against the decoded graph.
    pub fn indexes(&self) -> &[SealedIndex] {
        &self.indexes
    }

    /// The verifier-derived `FieldId → [IndexId]` incidence: the stable ids of every
    /// managed index whose projection includes the stored field `field`. This is the
    /// maintenance consequence of mutating that field — the set of indexes a later
    /// exact-field write must keep coherent. Identity-key projection components
    /// are excluded: a key is immutable, so it triggers no field maintenance. Derived
    /// from the sealed indexes, never trusted from the image.
    pub fn field_incidence(&self, field: LedgerIdBytes) -> Vec<LedgerIdBytes> {
        self.indexes
            .iter()
            .filter(|index| {
                index
                    .components
                    .contains(&DurableIndexComponent::Field(field))
            })
            .map(|index| index.id)
            .collect()
    }

    /// The verifier-derived `RootId → [IndexId]` incidence: the stable ids of every
    /// managed index of the root at index `root`. This is the maintenance consequence
    /// of a whole-entry create/replace/erase on that root — every index must be kept
    /// coherent. Derived from the sealed indexes, never trusted from the image.
    pub fn root_incidence(&self, root: u16) -> Vec<LedgerIdBytes> {
        self.indexes
            .iter()
            .filter(|index| index.root == root)
            .map(|index| index.id)
            .collect()
    }

    /// The verifier-derived legal `unique_index_collision` outcome layout for a
    /// `create`/`replace` on the root at index `root`: the stable ids of its unique
    /// managed indexes, each of which a create/replace may collide on. A durable
    /// operation algebra collision reveals exactly one of these `IndexId`s and nothing
    /// else — no colliding key, entry, or sibling. A root with no unique index admits
    /// no collision outcome. Derived from the sealed indexes, never trusted from the
    /// image.
    pub fn unique_collision_outcomes(&self, root: u16) -> Vec<LedgerIdBytes> {
        self.indexes
            .iter()
            .filter(|index| index.root == root && index.unique)
            .map(|index| index.id)
            .collect()
    }

    pub fn consts(&self) -> &[SealedConst] {
        &self.consts
    }

    pub fn function(&self, index: FunctionIndex) -> &SealedFunction {
        &self.functions[index.index()]
    }

    /// The reconstructed durable demand of the function at `index` over its whole
    /// call closure. A test-body driver consults this to open the read or write
    /// session one export call requires; empty demand needs no session.
    pub fn function_demand(&self, index: FunctionIndex) -> &ExportDemand {
        &self.function_demands[index.index()]
    }

    pub fn functions(&self) -> &[SealedFunction] {
        &self.functions
    }

    pub fn exports(&self) -> &[SealedExport] {
        &self.exports
    }

    /// The program-wide durable demand union over every export: the canonical demand
    /// admission checks a store against. One invocation then checks its own export's
    /// named demand; this union is the ceiling-admission side. Derived from the
    /// exports' reconstructed demands, never serialized in the image.
    pub fn demand_union(&self) -> ExportDemand {
        ExportDemand::union(self.exports.iter().map(SealedExport::demand))
    }

    /// The durable demand union over every test entry: the ceiling an E01 ephemeral
    /// test attachment bounds a durable source test by. Empty unless the test-profile
    /// image carries a durable test. Derived, never serialized.
    pub fn test_demand_union(&self) -> ExportDemand {
        ExportDemand::union(self.test_entries.iter().map(SealedTestEntry::demand))
    }

    /// The reverse index of export demand: one row per durable graph node any export
    /// demands, in ascending path order, listing which exports touch it and with
    /// which operation class. This is the verifier's derivation of durable
    /// classification from the call closure — which places are read, written, erased,
    /// probed, or traversed, and by whom. Nothing here is serialized in the image;
    /// it is rebuilt from the exports' reconstructed demand.
    pub fn demand_incidence(&self) -> Vec<NodeIncidence> {
        use std::collections::BTreeMap;
        let mut by_path: BTreeMap<SemanticPath, Vec<AtomIncidence>> = BTreeMap::new();
        for export in &self.exports {
            for atom in export.demand.atoms() {
                by_path
                    .entry(atom.path().clone())
                    .or_default()
                    .push(AtomIncidence {
                        export: export.id,
                        class: atom.class(),
                    });
            }
        }
        by_path
            .into_iter()
            .map(|(path, touched_by)| NodeIncidence { path, touched_by })
            .collect()
    }

    /// The test entries, in ascending report-name order. `marrow test` runs each
    /// storeless; a test entry is never dispatched as an export.
    pub fn test_entries(&self) -> &[SealedTestEntry] {
        &self.test_entries
    }

    /// The export bound to `id`, if any. The VM and CLI dispatch only through this
    /// verified id — a source name is resolved to an id outside the image (through
    /// the compiler's export directory) and never crosses this boundary.
    pub fn export_by_id(&self, id: ExportId) -> Option<&SealedExport> {
        self.exports.iter().find(|export| export.id == id)
    }
}

/// One row of the export-demand reverse index ([`VerifiedImage::demand_incidence`]):
/// a durable graph node and every `(export, class)` incidence upon it. The path is
/// the stable ledger-id chain; the incidences are in export-discovery order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIncidence {
    pub path: SemanticPath,
    pub touched_by: Vec<AtomIncidence>,
}

/// One export's access to a durable graph node: which export, and the operation
/// class it makes there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomIncidence {
    pub export: ExportId,
    pub class: OperationClass,
}
