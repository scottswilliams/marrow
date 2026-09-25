//! The typed image rejection: the phase whose invariant the image violated, which fixes
//! the stable `image.*` code, and the kind of violation with its payload. One `Display`
//! renders it; nothing compares rendered text.

use marrow_codes::Code;
use marrow_image::{DurableGraphInputRefusal, Scalar};
use std::fmt;

/// The verifier phase that rejected an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyPhase {
    /// Magic, version, digest, section framing.
    Envelope,
    /// Table decode and grammar.
    Table,
    /// Per-function structure, types, and local initialization.
    Function,
    /// Call closure: cycle rejection.
    Closure,
    /// Transaction and presence flow.
    Flow,
    /// The TEST-ENTRY table and `assert` legality.
    TestEntry,
}

impl VerifyPhase {
    /// The stable code for a rejection in this phase.
    pub fn code(self) -> Code {
        match self {
            VerifyPhase::Envelope => Code::ImageEnvelope,
            VerifyPhase::Table => Code::ImageTable,
            VerifyPhase::Function => Code::ImageFunction,
            VerifyPhase::Closure => Code::ImageClosure,
            VerifyPhase::Flow => Code::ImageFlow,
            VerifyPhase::TestEntry => Code::ImageTestEntry,
        }
    }
}

/// A typed image rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyRejection {
    phase: VerifyPhase,
    kind: RejectionKind,
}

impl VerifyRejection {
    pub(crate) fn new(phase: VerifyPhase, kind: RejectionKind) -> Self {
        Self { phase, kind }
    }

    pub fn phase(&self) -> VerifyPhase {
        self.phase
    }

    /// The stable `image.*` code for the rejecting phase.
    pub fn code(&self) -> Code {
        self.phase.code()
    }

    pub fn kind(&self) -> &RejectionKind {
        &self.kind
    }
}

impl fmt::Display for VerifyRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code().as_str(), self.kind)
    }
}

impl std::error::Error for VerifyRejection {}

/// A region of the container: the framing, one of the ten sections, or a function's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Container,
    Strings,
    Types,
    Durable,
    Consts,
    Functions,
    Exports,
    Spans,
    TestEntries,
    Enums,
    Collections,
    Code,
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Region::Container => "the container",
            Region::Strings => "the string table",
            Region::Types => "the record-type table",
            Region::Durable => "the durable table",
            Region::Consts => "the constant table",
            Region::Functions => "the function table",
            Region::Exports => "the export table",
            Region::Spans => "the span table",
            Region::TestEntries => "the test-entry table",
            Region::Enums => "the enum table",
            Region::Collections => "the collection table",
            Region::Code => "the code",
        })
    }
}

/// What an in-image reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ref {
    String,
    RecordType,
    Enum,
    Collection,
    Root,
    Const,
    Function,
    Local,
    Field,
    Variant,
    PayloadLeaf,
    Site,
    Branch,
    Group,
    Index,
    KeyColumn,
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Ref::String => "string",
            Ref::RecordType => "record type",
            Ref::Enum => "enum",
            Ref::Collection => "collection type",
            Ref::Root => "root",
            Ref::Const => "constant",
            Ref::Function => "function",
            Ref::Local => "local",
            Ref::Field => "field",
            Ref::Variant => "variant",
            Ref::PayloadLeaf => "payload leaf",
            Ref::Site => "site",
            Ref::Branch => "branch",
            Ref::Group => "group",
            Ref::Index => "index",
            Ref::KeyColumn => "key column",
        })
    }
}

/// A bound from [`marrow_image::bounds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    ImageBytes,
    Strings,
    StringBytes,
    RecordTypes,
    Fields,
    Enums,
    Variants,
    PayloadFields,
    Collections,
    Roots,
    KeyColumns,
    Sites,
    SitePathSteps,
    DurableMembers,
    DurableDepth,
    Indexes,
    IndexComponents,
    StructLeaves,
    ValueDepth,
    Consts,
    Functions,
    Params,
    Locals,
    CodeBytes,
    Exports,
    TestEntries,
    StackDepth,
}

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Bound::ImageBytes => "image size",
            Bound::Strings => "string count",
            Bound::StringBytes => "string length",
            Bound::RecordTypes => "record type count",
            Bound::Fields => "field count",
            Bound::Enums => "enum count",
            Bound::Variants => "variant count",
            Bound::PayloadFields => "payload count",
            Bound::Collections => "collection type count",
            Bound::Roots => "root count",
            Bound::KeyColumns => "key column count",
            Bound::Sites => "site count",
            Bound::SitePathSteps => "site path depth",
            Bound::DurableMembers => "durable member count",
            Bound::DurableDepth => "durable member depth",
            Bound::Indexes => "index count",
            Bound::IndexComponents => "index component count",
            Bound::StructLeaves => "struct leaf count",
            Bound::ValueDepth => "value shape depth",
            Bound::Consts => "constant count",
            Bound::Functions => "function count",
            Bound::Params => "parameter count",
            Bound::Locals => "local count",
            Bound::CodeBytes => "code size",
            Bound::Exports => "export count",
            Bound::TestEntries => "test entry count",
            Bound::StackDepth => "operand stack depth",
        })
    }
}

/// What an image spelled twice where it must be unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplicate {
    FieldName,
    VariantName,
    ExportFunction,
    TestEntryFunction,
    Site,
    LedgerId,
    RootPlacement,
    RootName,
}

impl fmt::Display for Duplicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Duplicate::FieldName => "field name in a record",
            Duplicate::VariantName => "variant name in an enum",
            Duplicate::ExportFunction => "export of one function",
            Duplicate::TestEntryFunction => "test entry of one function",
            Duplicate::Site => "durable site",
            Duplicate::LedgerId => "durable ledger id",
            Duplicate::RootPlacement => "durable root occurrence",
            Duplicate::RootName => "durable root name",
        })
    }
}

/// A tag byte outside its closed domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Magic,
    Version,
    Const,
    CollectionKind,
    DurableMember,
    DurableValue,
    DurableLeaf,
    ValueScalar,
    SiteStep,
    SiteTarget,
    IndexComponent,
    Opcode,
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Tag::Magic => "magic",
            Tag::Version => "format version",
            Tag::Const => "constant tag",
            Tag::CollectionKind => "collection kind",
            Tag::DurableMember => "durable member tag",
            Tag::DurableValue => "durable value tag",
            Tag::DurableLeaf => "durable value leaf marker",
            Tag::ValueScalar => "durable value scalar",
            Tag::SiteStep => "site path step kind",
            Tag::SiteTarget => "site target",
            Tag::IndexComponent => "index component kind",
            Tag::Opcode => "opcode",
        })
    }
}

/// A one-byte flag that must be exactly 0 or 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    FieldRequired,
    VariantCategory,
    DurableFieldRequired,
    IndexUnique,
    BoolConst,
    BoolOperand,
}

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Flag::FieldRequired => "field required",
            Flag::VariantCategory => "variant category",
            Flag::DurableFieldRequired => "durable field required",
            Flag::IndexUnique => "index unique",
            Flag::BoolConst => "bool constant",
            Flag::BoolOperand => "bool operand",
        })
    }
}

/// A position the container spells a type reference in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypePosition {
    RecordField,
    EnumPayloadLeaf,
    CollectionLeaf,
    Param,
    Return,
    VacantLoad,
}

impl fmt::Display for TypePosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TypePosition::RecordField => "record field",
            TypePosition::EnumPayloadLeaf => "enum payload leaf",
            TypePosition::CollectionLeaf => "collection leaf",
            TypePosition::Param => "parameter",
            TypePosition::Return => "return",
            TypePosition::VacantLoad => "vacant-load operand",
        })
    }
}

/// What a type reference got wrong for its position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRefFault {
    Truncated,
    Optionality,
    IndexOutOfRange,
    TagNotAdmitted,
}

impl fmt::Display for TypeRefFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TypeRefFault::Truncated => "that is truncated",
            TypeRefFault::Optionality => "with an inadmissible optional flag",
            TypeRefFault::IndexOutOfRange => "index out of range",
            TypeRefFault::TagNotAdmitted => "tag not admitted there",
        })
    }
}

/// The durable member tree a materialized record is tied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieNode {
    Root,
    Group,
    Branch,
}

impl fmt::Display for TieNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TieNode::Root => "root",
            TieNode::Group => "group",
            TieNode::Branch => "branch",
        })
    }
}

/// How a member tree disagrees with the record it is tied to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieFault {
    FieldAfterGroup,
    MoreMembers,
    FewerMembers,
    FieldMismatch,
    SlotNotGroupRecord,
}

impl fmt::Display for TieFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TieFault::FieldAfterGroup => "places a field after a group",
            TieFault::MoreMembers => "has more members than its record has slots",
            TieFault::FewerMembers => "has fewer members than its record has slots",
            TieFault::FieldMismatch => "does not match its record's fields",
            TieFault::SlotNotGroupRecord => "is tied to a slot that is not a bare group record",
        })
    }
}

/// How a managed index's projection is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    FieldNotEligible,
    FieldUnknown,
    KeyUnknown,
    Empty,
    RepeatedComponent,
    MissingIdentitySuffix,
}

impl fmt::Display for Projection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Projection::FieldNotEligible => "over a field that is not index-eligible",
            Projection::FieldUnknown => "over no top-level field of its root",
            Projection::KeyUnknown => "over no identity key of its root",
            Projection::Empty => "that is empty",
            Projection::RepeatedComponent => "that repeats a component",
            Projection::MissingIdentitySuffix => {
                "that does not end with the identity keys in declaration order"
            }
        })
    }
}

/// How an operation site fails to resolve against the reconstructed graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteFault {
    Empty,
    Unresolved,
    TargetKind,
}

impl fmt::Display for SiteFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SiteFault::Empty => "a site path naming no graph node",
            SiteFault::Unresolved => "a site path resolving to no graph node",
            SiteFault::TargetKind => "a site target disagreeing with its graph node's kind",
        })
    }
}

/// The site kind a durable opcode requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKind {
    Entry,
    Field,
    Group,
    Branch,
    Index,
    NotIndex,
}

impl fmt::Display for SiteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SiteKind::Entry => "an operation requiring an entry site",
            SiteKind::Field => "an operation requiring a field site",
            SiteKind::Group => "an operation requiring a group site",
            SiteKind::Branch => "a branch site with an empty branch path",
            SiteKind::Index => "a managed-index opcode over a non-index site",
            SiteKind::NotIndex => "a non-index opcode over a managed-index site",
        })
    }
}

/// What an operand-stack slot had to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    Scalar(Scalar),
    Record,
    Enum,
    List,
    Map,
    Identity,
    Optional,
    Bare,
    Renderable,
    Argument,
    RecordField,
    FieldValue,
    Payload,
    ListElement,
    MapKey,
    MapValue,
    KeyColumn,
    Durable,
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operand::Scalar(scalar) => write!(f, "a bare {}", scalar_name(*scalar)),
            Operand::Record => f.write_str("a bare record"),
            Operand::Enum => f.write_str("a bare enum"),
            Operand::List => f.write_str("a bare list"),
            Operand::Map => f.write_str("a bare map"),
            Operand::Identity => f.write_str("a bare entry identity"),
            Operand::Optional => f.write_str("an optional"),
            Operand::Bare => f.write_str("a bare value"),
            Operand::Renderable => f.write_str("a renderable scalar, enum, or identity"),
            Operand::Argument => f.write_str("the parameter's type"),
            Operand::RecordField => f.write_str("the record field's type"),
            Operand::FieldValue => f.write_str("the field's type"),
            Operand::Payload => f.write_str("the payload leaf's type"),
            Operand::ListElement => f.write_str("the list element type"),
            Operand::MapKey => f.write_str("the map key type"),
            Operand::MapValue => f.write_str("the map value type"),
            Operand::KeyColumn => f.write_str("the root's key column type"),
            Operand::Durable => f.write_str("the durable operand's type"),
        }
    }
}

fn scalar_name(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Int => "int",
        Scalar::Bool => "bool",
        Scalar::Text => "string",
        Scalar::Bytes => "bytes",
        Scalar::Date => "date",
        Scalar::Instant => "instant",
        Scalar::Duration => "duration",
    }
}

/// The invariant an image violated, with what it violated it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionKind {
    Truncated(Region),
    Trailing(Region),
    Unsorted(Region),
    OutOfRange(Ref),
    OverBound(Bound),
    Duplicate(Duplicate),
    Unknown(Tag),
    Flag(Flag),
    TypeRef {
        position: TypePosition,
        fault: TypeRefFault,
    },
    DigestMismatch,
    SectionCount,
    SectionIds,
    InvalidUtf8,
    ConstDomain(Scalar),
    MapKeyNotScalar,
    ValueTypeCycle,
    LocalsBelowParams,
    SpanNotOneBased,
    SpanStart,
    SpanMissing,
    SpanBoundary,
    DurableGraph(DurableGraphInputRefusal),
    KeyNotOrderable,
    RecordTie {
        node: TieNode,
        fault: TieFault,
    },
    IndexProjection(Projection),
    EnumIdentityReused,
    ValueArenaExhausted,
    EmptyLeafName,
    Site(SiteFault),
    IndexReadKind,
    ContractUnidentifiable,
    ContractMismatch,
    RangeGuardEmpty,
    KeySlotArity,
    KeySlotType,
    KeySlotUninit,
    JumpTarget,
    EmptyCode,
    UnreachableInstruction,
    FallsOffEnd,
    StackMerge,
    StackUnderflow,
    LocalUninit,
    LocalRetyped,
    ReturnType,
    ReturnStack,
    MarkerOperandNotText,
    OperandType(Operand),
    EnumMismatch,
    IdentityRootMismatch,
    IdentityArity,
    UnsetRequiredField,
    EraseRequiredField,
    EraseRequiredGroup,
    PresentReadOfSparseField,
    NotList,
    NotMap,
    NotListOfString,
    FrozenListType,
    ParkedSite,
    RootNotExecutable,
    RequiresSite(SiteKind),
    CompositeKeyTraversal,
    TraversalBound,
    IndexRootMismatch,
    ForeignIdentityKey,
    CallCycle,
    OwnerCalled,
    EmptyTransaction,
    MarkerOutsideOwner,
    BeginTwice,
    CommitOutsideRegion,
    ReturnWithoutCommit,
    MutationOutsideRegion,
    OperationAfterCommit,
    TransactionMerge,
    PresenceSite,
    PresenceUnproven,
    AssertOutsideTest,
    TestEntryExported,
    TestEntrySignature,
    TestEntryCalled,
    TestDirectDurable,
    TestCallsUnownedMutation,
}

impl fmt::Display for RejectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sentence = match self {
            Self::Truncated(region) => return write!(f, "truncated {region}"),
            Self::Trailing(region) => return write!(f, "trailing bytes in {region}"),
            Self::Unsorted(region) => return write!(f, "unsorted or repeated rows in {region}"),
            Self::OutOfRange(what) => return write!(f, "a {what} index out of range"),
            Self::OverBound(bound) => return write!(f, "over the {bound} bound"),
            Self::Duplicate(what) => return write!(f, "a duplicate {what}"),
            Self::Unknown(tag) => return write!(f, "an unknown {tag}"),
            Self::Flag(flag) => return write!(f, "a {flag} flag that is not 0 or 1"),
            Self::TypeRef { position, fault } => return write!(f, "a {position} type {fault}"),
            Self::ConstDomain(scalar) => {
                return write!(
                    f,
                    "a {} constant outside its supported range",
                    scalar_name(*scalar)
                );
            }
            Self::DurableGraph(refusal) => graph_refusal(*refusal),
            Self::RecordTie { node, fault } => {
                return write!(f, "a {node} member tree that {fault}");
            }
            Self::IndexProjection(fault) => return write!(f, "a managed-index projection {fault}"),
            Self::Site(fault) => return fault.fmt(f),
            Self::RequiresSite(kind) => return kind.fmt(f),
            Self::OperandType(want) => return write!(f, "an operand that is not {want}"),
            Self::DigestMismatch => "a digest that does not cover the payload",
            Self::SectionCount => "a section count other than 10",
            Self::SectionIds => "section ids other than 1..10 in order",
            Self::InvalidUtf8 => "a string that is not valid UTF-8",
            Self::MapKeyNotScalar => "a map key that is not a bare scalar",
            Self::ValueTypeCycle => "a cycle in the value type graph",
            Self::LocalsBelowParams => "a local count below the parameter count",
            Self::SpanNotOneBased => "a span line or column that is not 1-based",
            Self::SpanStart => "a first span not at instruction offset 0",
            Self::SpanMissing => "code with no span mapping",
            Self::SpanBoundary => "a span offset off an instruction boundary",
            Self::KeyNotOrderable => "a key column that is not an orderable durable-key scalar",
            Self::EnumIdentityReused => {
                "a durable enum identity reused with different members or payload names"
            }
            Self::ValueArenaExhausted => "a durable value shape outside the value arena's domain",
            Self::EmptyLeafName => "a durable struct or payload leaf with an empty name",
            Self::IndexReadKind => "an index read kind disagreeing with the index's unique flag",
            Self::ContractUnidentifiable => "a durable graph too large to identify",
            Self::ContractMismatch => "a durable contract id that does not match the durable graph",
            Self::RangeGuardEmpty => "an empty range-guard interval",
            Self::KeySlotArity => "a present-entry key-path arity that does not match its site",
            Self::KeySlotType => "a present-entry key slot of the wrong type",
            Self::KeySlotUninit => "a present-entry key slot that is uninitialized or out of range",
            Self::JumpTarget => "a jump target off an instruction boundary",
            Self::EmptyCode => "a function with no code",
            Self::UnreachableInstruction => "an unreachable instruction",
            Self::FallsOffEnd => "execution falling off the end of the code",
            Self::StackMerge => "operand stacks that disagree at a merge",
            Self::StackUnderflow => "an operand stack underflow",
            Self::LocalUninit => "a local read before initialization",
            Self::LocalRetyped => "a local slot reused at a different type",
            Self::ReturnType => "a return that does not match the return type",
            Self::ReturnStack => "an operand stack not empty at return",
            Self::MarkerOperandNotText => "a diverging-marker operand that is not a text constant",
            Self::EnumMismatch => "enum operands of different enums",
            Self::IdentityRootMismatch => "identity operands naming different store roots",
            Self::IdentityArity => {
                "an identity column count that does not match the root's key columns"
            }
            Self::UnsetRequiredField => "an unset of a required field",
            Self::EraseRequiredField => "an erase of a required field",
            Self::EraseRequiredGroup => "an erase of a group holding a required leaf",
            Self::PresentReadOfSparseField => "a present field read of a sparse field",
            Self::NotList => "a collection type that is not a list",
            Self::NotMap => "a collection type that is not a map",
            Self::NotListOfString => "a collection type that is not a list of string",
            Self::FrozenListType => "a frozen list type that is not a list of the traversed key",
            Self::ParkedSite => "an operation over a site that is not executable",
            Self::RootNotExecutable => "an operation over a root that is not flat-executable",
            Self::CompositeKeyTraversal => {
                "a traversal over a composite key, which is not yet executable"
            }
            Self::TraversalBound => "a traversal bound that is zero or too large",
            Self::IndexRootMismatch => "an index read site naming an index of a different root",
            Self::ForeignIdentityKey => {
                "an entry identity keying a durable operation on a different store root"
            }
            Self::CallCycle => "a cycle in the call graph",
            Self::OwnerCalled => "a call to a transaction owner",
            Self::EmptyTransaction => "a transaction performing no durable operation",
            Self::MarkerOutsideOwner => "a transaction marker outside its owning export",
            Self::BeginTwice => "a transaction begun more than once",
            Self::CommitOutsideRegion => "a transaction committed outside its region",
            Self::ReturnWithoutCommit => "a path returning without committing the transaction",
            Self::MutationOutsideRegion => "a mutation outside the transaction region",
            Self::OperationAfterCommit => "a durable operation after the transaction's commit",
            Self::TransactionMerge => "transaction states that disagree at a merge",
            Self::PresenceSite => {
                "a present-entry operation over a site that is not a field or group"
            }
            Self::PresenceUnproven => "a present-entry operation not dominated by a presence fact",
            Self::AssertOutsideTest => "an assert outside a test entry",
            Self::TestEntryExported => "a test entry that is also an export",
            Self::TestEntrySignature => "a test entry taking parameters or returning a value",
            Self::TestEntryCalled => "a call to a test entry",
            Self::TestDirectDurable => "a test body performing a direct durable operation",
            Self::TestCallsUnownedMutation => {
                "a test calling a mutating function without its own transaction"
            }
        };
        f.write_str(sentence)
    }
}

fn graph_refusal(refusal: DurableGraphInputRefusal) -> &'static str {
    match refusal {
        DurableGraphInputRefusal::OverPlan | DurableGraphInputRefusal::UnaddressableOccurrence => {
            "more durable roots than admitted"
        }
        DurableGraphInputRefusal::MalformedCommands
        | DurableGraphInputRefusal::UndeclaredProduct => {
            "a durable member graph that is not a well-formed declaration"
        }
        DurableGraphInputRefusal::OverDepth => "a durable member tree past the depth bound",
        DurableGraphInputRefusal::DivergentGraph => {
            "a repeated durable Product declaring a different member graph"
        }
        DurableGraphInputRefusal::DivergentEntryRecord => {
            "a repeated durable Product declaring a different entry record"
        }
    }
}
