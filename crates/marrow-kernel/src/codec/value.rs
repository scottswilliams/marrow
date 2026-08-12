//! Canonical saved-value encoding.
//!
//! Values are stored in a backend-independent canonical byte form, so backup,
//! diff, equality, and restore are stable. The bytes carry no type tag — the
//! type comes from the schema at read time — and are not order-preserving, since
//! the store orders by key rather than by value.

use marrow_codes::Code;
use marrow_temporal::{
    format_date, format_duration, format_instant, parse_date, parse_duration, parse_instant,
    supported_date_days, supported_instant_nanos,
};

use super::key::KeyScalar;

/// Version of the canonical value encoding, recorded in a store profile so a
/// reopen can refuse data it cannot decode. Advances only on an incompatible
/// byte-format change.
pub const VALUE_CODEC_VERSION: u32 = 0;

/// A decoded scalar value, the runtime representation shared by the VM, kernel,
/// and tooling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeScalar {
    Bool(bool),
    Int(i64),
    Str(String),
    Bytes(Vec<u8>),
    /// A calendar date, held as days since the Unix epoch (1970-01-01).
    Date(i32),
    /// An elapsed span, held as a signed count of nanoseconds.
    Duration(i128),
    /// A UTC instant, held as a signed count of nanoseconds since the epoch.
    Instant(i128),
}

impl RuntimeScalar {
    /// This scalar's order-preserving key projection. The single home for that
    /// mapping; every current scalar type is key-eligible.
    pub fn as_key(&self) -> Result<Option<KeyScalar>, ValueError> {
        let key = match self {
            RuntimeScalar::Int(v) => KeyScalar::Int(*v),
            RuntimeScalar::Bool(v) => KeyScalar::Bool(*v),
            RuntimeScalar::Str(v) => KeyScalar::Str(v.clone()),
            RuntimeScalar::Bytes(v) => KeyScalar::Bytes(v.clone()),
            RuntimeScalar::Date(v) => KeyScalar::Date(*v),
            RuntimeScalar::Duration(v) => KeyScalar::Duration(*v),
            RuntimeScalar::Instant(v) => KeyScalar::Instant(*v),
        };
        validate_scalar_key(&key)?;
        Ok(Some(key))
    }

    /// This scalar's type discriminant.
    pub fn ty(&self) -> ScalarKind {
        match self {
            RuntimeScalar::Bool(_) => ScalarKind::Bool,
            RuntimeScalar::Int(_) => ScalarKind::Int,
            RuntimeScalar::Str(_) => ScalarKind::Str,
            RuntimeScalar::Bytes(_) => ScalarKind::Bytes,
            RuntimeScalar::Date(_) => ScalarKind::Date,
            RuntimeScalar::Duration(_) => ScalarKind::Duration,
            RuntimeScalar::Instant(_) => ScalarKind::Instant,
        }
    }
}

/// A value that cannot be encoded to canonical saved form. A `date`/`instant`
/// outside year 0001-9999 would format to a 5-7 digit year that [`decode_value`]
/// could never read back, so the codec rejects it to keep the round-trip exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    DateOutOfRange {
        days: i32,
    },
    InstantOutOfRange {
        nanos: i128,
    },
    /// A composite value's encoding exceeds a Law-9 size cap (a scalar leaf past
    /// [`MAX_LEAF_BYTES`] or the whole value past [`MAX_DURABLE_VALUE_BYTES`]), refused at
    /// encode before any engine write. Maps to the kernel's `value.range` fault.
    ValueTooLarge,
    /// A composite value carries a shape the storable durable value set excludes (a
    /// collection, an ordered map, unit, or an absent product leaf in a dense struct).
    /// Storable inline values are scalars, dense products, and sums only.
    Unstorable,
    /// A composite value nests past [`MAX_DURABLE_VALUE_DEPTH`], refused before any buffer
    /// is built. The size caps cannot stand in for this bound: nesting contributes no bytes,
    /// so an arbitrarily deep value encodes within any byte cap. This is the encode twin of
    /// the decode guard, and it makes the two sides accept the same set — a cell no reader
    /// could read back is never written. Maps to the kernel's `value.range` fault.
    ValueTooDeep,
}

impl ValueError {
    /// The stable dotted code a tool reports for this error.
    pub fn code(&self) -> &'static str {
        match self {
            Self::DateOutOfRange { .. }
            | Self::InstantOutOfRange { .. }
            | Self::ValueTooLarge
            | Self::Unstorable
            | Self::ValueTooDeep => Code::ValueRange.as_str(),
        }
    }
}

impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DateOutOfRange { days } => {
                write!(f, "date day {days} is outside the year 0001-9999 range")
            }
            Self::InstantOutOfRange { nanos } => {
                write!(f, "instant {nanos}ns is outside the year 0001-9999 range")
            }
            Self::ValueTooLarge => write!(f, "a durable value exceeds its encoded size cap"),
            Self::Unstorable => write!(f, "a value shape is not storable inline in a field"),
            Self::ValueTooDeep => write!(f, "a durable value nests past its shape depth cap"),
        }
    }
}

impl std::error::Error for ValueError {}

/// The type to decode saved bytes as. A typed read knows this from the verified
/// site. Distinct from the compiler's language-level scalar classification: this
/// is the runtime codec's discriminant over the full saved-value domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    Bool,
    Int,
    Str,
    Bytes,
    Date,
    Duration,
    Instant,
}

impl ScalarKind {
    /// The canonical language spelling of this scalar type.
    pub fn name(self) -> &'static str {
        match self {
            ScalarKind::Bool => "bool",
            ScalarKind::Int => "int",
            ScalarKind::Str => "string",
            ScalarKind::Bytes => "bytes",
            ScalarKind::Date => "date",
            ScalarKind::Instant => "instant",
            ScalarKind::Duration => "duration",
        }
    }
}

/// Encodes a value to its canonical saved bytes: `bool` as `0`/`1`, `int` as decimal
/// text, strings as UTF-8, bytes verbatim, dates as `YYYY-MM-DD`, durations as
/// `PT<seconds>S`, instants as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The canonical boundary: it emits only forms [`decode_value`] reads back, so a
/// `date`/`instant` outside year 0001-9999 is a typed [`ValueError`].
pub fn encode_value(value: &RuntimeScalar) -> Result<Vec<u8>, ValueError> {
    // A saved cell holds exactly one present scalar: the only cell discriminant is
    // the scalar type tag, never a null, optional, or tombstone value. Absence is
    // the lack of a cell at the data path, not an encoded marker. The closed
    // `RuntimeScalar` sum is the structural guarantee.
    Ok(match value {
        RuntimeScalar::Bool(value) => vec![if *value { b'1' } else { b'0' }],
        RuntimeScalar::Int(value) => value.to_string().into_bytes(),
        RuntimeScalar::Str(text) => text.as_bytes().to_vec(),
        RuntimeScalar::Bytes(bytes) => bytes.clone(),
        RuntimeScalar::Date(days) => format_date(*days)
            .ok_or(ValueError::DateOutOfRange { days: *days })?
            .into_bytes(),
        RuntimeScalar::Duration(nanos) => format_duration(*nanos).into_bytes(),
        RuntimeScalar::Instant(nanos) => format_instant(*nanos)
            .ok_or(ValueError::InstantOutOfRange { nanos: *nanos })?
            .into_bytes(),
    })
}

/// Decodes canonical saved bytes as `ty`, strictly: non-canonical bytes such as
/// `+1`, `01`, or a non-`0`/`1` boolean are rejected rather than normalized.
pub fn decode_value(bytes: &[u8], ty: ScalarKind) -> Option<RuntimeScalar> {
    match ty {
        ScalarKind::Bool => match bytes {
            b"0" => Some(RuntimeScalar::Bool(false)),
            b"1" => Some(RuntimeScalar::Bool(true)),
            _ => None,
        },
        ScalarKind::Int => Some(RuntimeScalar::Int(parse_canonical_int(bytes)?)),
        ScalarKind::Str => Some(RuntimeScalar::Str(String::from_utf8(bytes.to_vec()).ok()?)),
        ScalarKind::Bytes => Some(RuntimeScalar::Bytes(bytes.to_vec())),
        ScalarKind::Date => Some(RuntimeScalar::Date(parse_date(bytes)?)),
        ScalarKind::Duration => Some(RuntimeScalar::Duration(parse_duration(bytes)?)),
        ScalarKind::Instant => Some(RuntimeScalar::Instant(parse_instant(bytes)?)),
    }
}

/// Parses the canonical int form, rejecting anything that would not round-trip
/// identically (`+1`, `01`, `-0`, whitespace).
fn parse_canonical_int(bytes: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let value: i64 = text.parse().ok()?;
    (value.to_string() == text).then_some(value)
}

pub fn validate_scalar_key(key: &KeyScalar) -> Result<(), ValueError> {
    match key {
        KeyScalar::Date(days) if !supported_date_days(*days) => {
            Err(ValueError::DateOutOfRange { days: *days })
        }
        KeyScalar::Instant(nanos) if !supported_instant_nanos(*nanos) => {
            Err(ValueError::InstantOutOfRange { nanos: *nanos })
        }
        _ => Ok(()),
    }
}

pub fn scalar_key_matches_type(key: &KeyScalar, expected: ScalarKind) -> bool {
    key.scalar_kind() == expected && validate_scalar_key(key).is_ok()
}

// --- Widened composite value codec ---
//
// A durable field value widens from a bare scalar to the closed acyclic storable set:
// scalars, dense products (`struct`/record), and sums (closed `enum`/`Option`/`Result`).
// Collections are never inline field payloads (they are keyed branches); a nominal-typed
// field is not yet admitted. The codec extends the one scalar codec above — a scalar leaf is
// still `encode_value`/`decode_value` — and frames composites within one field-leaf cell:
// a top-level scalar is raw (byte-identical to today); inside a composite each scalar leaf
// is minimal-LEB128 length-prefixed, a sum carries a minimal-LEB128 variant index, and a
// nested composite is schema-delimited. Bytes carry no type tag — the shape comes from the
// schema at read time (`ValueShape`) — so decode is shape-driven and strict: a non-minimal
// length, an out-of-range variant, an over-cap leaf, an over-deep shape, or trailing bytes
// are rejected, never normalized. One value, one encoding.

use super::varint::{decode_len, encode_len};
use crate::equality::ValueDomain;

/// The per-scalar-leaf encoded byte cap (mirrors the VM `run.text_limit`, 64 KiB).
pub const MAX_LEAF_BYTES: usize = 64 * 1024;
/// The whole-value encoded byte cap. Chosen (not inherited); it must stay `<=` the engine
/// `MAX_VALUE_LEN` so a value this codec admits always fits the engine and the codec's own
/// Law-9 fault fires first (see the slice-E design brief §4).
pub const MAX_DURABLE_VALUE_BYTES: usize = 1 << 20;
/// The value-shape nesting depth cap (mirrors `marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH`),
/// bounding decoder recursion before allocation.
pub const MAX_DURABLE_VALUE_DEPTH: usize = 32;

/// The schema-derived shape of a durable field value, driving the tagless decode. A scalar
/// carries its kind; a product carries its type index and per-leaf shapes in declaration
/// order; a sum carries its type index and, per variant in declaration order, that variant's
/// dense payload shapes. `Option`/`Result` are ordinary sums (fixed indices `none=0/some=1`,
/// `ok=0/err=1`).
///
/// The type is opaque over a private recursive node and a checked depth metric. There is no
/// public recursive field, variant, struct literal, or `Vec`-taking constructor: the only
/// way to obtain a composite shape is [`ValueShapeBuilder`], whose flat open/close command
/// stream refuses a composite opened past [`MAX_DURABLE_VALUE_DEPTH`]. That is a
/// construction-time bound, not an entry-time one, and the difference is the whole point: a
/// caller-built recursive shape overflows the stack while it is being built and again while
/// the refused argument is dropped, so no amount of validation at an entry point can make
/// one safe. Because every reachable value is bounded, both the codec's recursion and the
/// implicit recursive `Drop` are bounded by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueShape {
    node: ShapeNode,
    /// The count of composites on this shape's deepest path, including itself. A scalar is
    /// `0`; the top-level composite the codec calls depth 1 is `1`. Never exceeds
    /// [`MAX_DURABLE_VALUE_DEPTH`] — the builder is the sole minter and refuses beyond it.
    depth: u32,
}

/// The private recursive representation. Reachable only through [`ValueShape::view`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ShapeNode {
    Scalar(ScalarKind),
    Product {
        ty: u16,
        fields: Vec<ValueShape>,
    },
    Sum {
        ty: u16,
        variants: Vec<Vec<ValueShape>>,
    },
}

/// A borrowed view of one [`ValueShape`] node. Carries no owned recursive payload, so a
/// consumer can match on the shape's structure without a route to constructing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueShapeRef<'a> {
    Scalar(ScalarKind),
    Product {
        ty: u16,
        fields: &'a [ValueShape],
    },
    Sum {
        ty: u16,
        variants: VariantShapes<'a>,
    },
}

/// The borrowed per-variant payload shapes of a sum, in declaration order. A wrapper rather
/// than a bare slice so no public signature names a recursive owned `Vec<ValueShape>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VariantShapes<'a>(&'a [Vec<ValueShape>]);

impl<'a> VariantShapes<'a> {
    /// The number of declared variants.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the sum declares no variant.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The dense payload shapes of variant `index`, or `None` when the index is out of
    /// range — the decoder's own bound on a forged variant index.
    pub fn get(&self, index: usize) -> Option<&'a [ValueShape]> {
        self.0.get(index).map(Vec::as_slice)
    }

    /// The variants' payload shapes in declaration order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &'a [ValueShape]> {
        self.0.iter().map(Vec::as_slice)
    }
}

impl ValueShape {
    /// A scalar leaf shape. The one composite-free constructor: it nests nothing, so it
    /// carries no depth obligation and cannot be chained into an unbounded tree.
    pub fn scalar(kind: ScalarKind) -> Self {
        Self {
            node: ShapeNode::Scalar(kind),
            depth: 0,
        }
    }

    /// This shape's node, borrowed.
    pub fn view(&self) -> ValueShapeRef<'_> {
        match &self.node {
            ShapeNode::Scalar(kind) => ValueShapeRef::Scalar(*kind),
            ShapeNode::Product { ty, fields } => ValueShapeRef::Product {
                ty: *ty,
                fields: fields.as_slice(),
            },
            ShapeNode::Sum { ty, variants } => ValueShapeRef::Sum {
                ty: *ty,
                variants: VariantShapes(variants.as_slice()),
            },
        }
    }

    /// The scalar kind of a scalar leaf, or `None` for a composite. The common projection
    /// for a consumer that admits only scalar-shaped fields.
    pub fn scalar_kind(&self) -> Option<ScalarKind> {
        match &self.node {
            ShapeNode::Scalar(kind) => Some(*kind),
            ShapeNode::Product { .. } | ShapeNode::Sum { .. } => None,
        }
    }

    /// The count of composites on this shape's deepest path, at most
    /// [`MAX_DURABLE_VALUE_DEPTH`]. A scalar is `0`.
    fn depth(&self) -> usize {
        self.depth as usize
    }
}

/// Why a flat shape command stream did not yield a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeBuildError {
    /// A composite was opened past [`MAX_DURABLE_VALUE_DEPTH`]. Refused at the opening
    /// command, so the builder's own partial tree never grows past the bound either.
    TooDeep,
    /// A command was issued in a position the shape grammar has no place for: a variant
    /// opened outside a sum, a leaf or composite placed directly inside a sum rather than
    /// inside one of its variants, or a close with nothing open.
    Misplaced,
    /// The stream did not describe exactly one shape: it left a composite open, or it
    /// emitted no shape or more than one at the top level.
    NotOneShape,
}

impl std::fmt::Display for ShapeBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooDeep => write!(f, "a value shape nests past its depth cap"),
            Self::Misplaced => write!(f, "a value shape command has no place in the grammar"),
            Self::NotOneShape => write!(f, "a value shape stream did not describe one shape"),
        }
    }
}

impl std::error::Error for ShapeBuildError {}

/// One open composite of a shape under construction.
#[derive(Debug)]
enum ShapeFrame {
    Product {
        ty: u16,
        fields: Vec<ValueShape>,
    },
    Sum {
        ty: u16,
        variants: Vec<Vec<ValueShape>>,
    },
    /// One variant of the enclosing sum. Not a composite: it adds no nesting to the encoded
    /// form, so it does not count against the depth bound.
    Variant {
        payload: Vec<ValueShape>,
    },
}

/// The sole minter of a composite [`ValueShape`]: a flat stream of open/leaf/close commands
/// over an explicit stack, never a recursive value the caller assembles.
///
/// The distinction is the invariant. A caller holding a recursive constructor can build a
/// chain deeper than any machine stack before the callee ever sees it; here the caller holds
/// only a builder, and the builder refuses the command that would open a composite past
/// [`MAX_DURABLE_VALUE_DEPTH`]. A hostile loop issuing a million `open_product` commands
/// costs `O(bound)` memory and returns a typed refusal.
///
/// Commands latch the first refusal rather than returning per call, so a projection can emit
/// its whole stream and read one verdict at [`finish`](Self::finish). Nothing partially built
/// escapes: `finish` consumes the builder and yields a shape only when the stream was whole
/// and within the bound.
#[derive(Debug, Default)]
pub struct ValueShapeBuilder {
    stack: Vec<ShapeFrame>,
    /// Completed top-level shapes. A well-formed stream leaves exactly one.
    finished: Vec<ValueShape>,
    /// Composites refused for depth, still awaiting their matching close so the stream's
    /// open/close balance is read correctly rather than reported as a second, spurious fault.
    suppressed: usize,
    error: Option<ShapeBuildError>,
}

impl ValueShapeBuilder {
    /// An empty builder, awaiting the commands of one shape.
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit a scalar leaf at the current position.
    pub fn scalar(&mut self, kind: ScalarKind) -> &mut Self {
        let shape = ValueShape::scalar(kind);
        self.place(shape)
    }

    /// Place an already-built shape at the current position. Checked against the same
    /// bound: the shape's own depth plus the composites currently open must stay within
    /// [`MAX_DURABLE_VALUE_DEPTH`], so composing built shapes cannot reach a depth the
    /// open/close commands would have refused.
    pub fn shape(&mut self, shape: ValueShape) -> &mut Self {
        if self.suppressed > 0 {
            return self;
        }
        if self.open_composites() + shape.depth() > MAX_DURABLE_VALUE_DEPTH {
            return self.fail(ShapeBuildError::TooDeep);
        }
        self.place(shape)
    }

    /// Open a dense product (`struct`/record) of type index `ty`. Its fields are the shapes
    /// emitted until the matching [`close`](Self::close).
    pub fn open_product(&mut self, ty: u16) -> &mut Self {
        self.open(ShapeFrame::Product {
            ty,
            fields: Vec::new(),
        })
    }

    /// Open a closed sum (`enum`/`Option`/`Result`) of type index `ty`. Its direct children
    /// are variants, opened with [`open_variant`](Self::open_variant).
    pub fn open_sum(&mut self, ty: u16) -> &mut Self {
        self.open(ShapeFrame::Sum {
            ty,
            variants: Vec::new(),
        })
    }

    /// Open the next variant of the enclosing sum. Its dense payload is the shapes emitted
    /// until the matching [`close`](Self::close). A variant adds no nesting.
    pub fn open_variant(&mut self) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed += 1;
            return self;
        }
        if !matches!(self.stack.last(), Some(ShapeFrame::Sum { .. })) {
            return self.fail(ShapeBuildError::Misplaced);
        }
        self.stack.push(ShapeFrame::Variant {
            payload: Vec::new(),
        });
        self
    }

    /// Close the innermost open product, sum, or variant.
    pub fn close(&mut self) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed -= 1;
            return self;
        }
        let Some(frame) = self.stack.pop() else {
            return self.fail(ShapeBuildError::Misplaced);
        };
        match frame {
            ShapeFrame::Product { ty, fields } => {
                let depth = 1 + fields.iter().map(ValueShape::depth).max().unwrap_or(0);
                let shape = ValueShape {
                    node: ShapeNode::Product { ty, fields },
                    depth: depth as u32,
                };
                self.place(shape)
            }
            ShapeFrame::Sum { ty, variants } => {
                let depth = 1 + variants
                    .iter()
                    .flat_map(|payload| payload.iter().map(ValueShape::depth))
                    .max()
                    .unwrap_or(0);
                let shape = ValueShape {
                    node: ShapeNode::Sum { ty, variants },
                    depth: depth as u32,
                };
                self.place(shape)
            }
            ShapeFrame::Variant { payload } => match self.stack.last_mut() {
                Some(ShapeFrame::Sum { variants, .. }) => {
                    variants.push(payload);
                    self
                }
                _ => self.fail(ShapeBuildError::Misplaced),
            },
        }
    }

    /// The refusal this stream has already latched, if any. A projection driving a long
    /// stream reads it to stop early rather than emitting commands into a builder that has
    /// already decided.
    pub fn refusal(&self) -> Option<ShapeBuildError> {
        self.error
    }

    /// Consume the stream and yield the one shape it described, or the first refusal it hit.
    pub fn finish(self) -> Result<ValueShape, ShapeBuildError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if !self.stack.is_empty() || self.suppressed > 0 {
            return Err(ShapeBuildError::NotOneShape);
        }
        let mut finished = self.finished;
        match finished.len() {
            1 => Ok(finished.pop().expect("one finished shape")),
            _ => Err(ShapeBuildError::NotOneShape),
        }
    }

    /// Open a composite frame, refusing one past the depth bound. Depth is the count of
    /// composite frames on the stack; a variant frame is not a composite.
    fn open(&mut self, frame: ShapeFrame) -> &mut Self {
        if self.suppressed > 0 {
            self.suppressed += 1;
            return self;
        }
        if self.open_composites() + 1 > MAX_DURABLE_VALUE_DEPTH {
            self.suppressed = 1;
            return self.fail(ShapeBuildError::TooDeep);
        }
        if matches!(self.stack.last(), Some(ShapeFrame::Sum { .. })) {
            return self.fail(ShapeBuildError::Misplaced);
        }
        self.stack.push(frame);
        self
    }

    /// The number of composites currently open — the nesting a shape placed here would sit
    /// under. A variant frame is not a composite and contributes nothing.
    fn open_composites(&self) -> usize {
        self.stack
            .iter()
            .filter(|frame| !matches!(frame, ShapeFrame::Variant { .. }))
            .count()
    }

    /// Place a completed shape into the innermost open frame, or at the top level.
    fn place(&mut self, shape: ValueShape) -> &mut Self {
        if self.suppressed > 0 {
            return self;
        }
        match self.stack.last_mut() {
            Some(ShapeFrame::Product { fields, .. }) => fields.push(shape),
            Some(ShapeFrame::Variant { payload }) => payload.push(shape),
            // A sum's direct children are variants; a leaf or composite here has no place.
            Some(ShapeFrame::Sum { .. }) => return self.fail(ShapeBuildError::Misplaced),
            None => self.finished.push(shape),
        }
        self
    }

    /// Latch the first refusal. Later commands are accepted and discarded so a projection
    /// need not branch mid-stream; `finish` reports this one verdict.
    fn fail(&mut self, error: ShapeBuildError) -> &mut Self {
        self.error.get_or_insert(error);
        self
    }
}

/// Encode a storable value to its canonical cell bytes. A top-level scalar is the raw scalar
/// codec (byte-identical to `encode_value`); a composite is framed. Refuses a value outside
/// the storable set ([`ValueError::Unstorable`]) or past a size cap
/// ([`ValueError::ValueTooLarge`]) before returning any bytes.
pub fn encode_domain(value: &ValueDomain) -> Result<Vec<u8>, ValueError> {
    let bytes = match value {
        ValueDomain::Scalar(scalar) => encode_value(scalar)?,
        ValueDomain::Product { .. } | ValueDomain::Sum { .. } => {
            let mut out = Vec::new();
            write_composite(value, &mut out, 1)?;
            out
        }
        // An entry identity is not a storable cell value on this slice — the durable
        // codec of a stored identity is a separately reserved decision. Rejecting it here
        // is the encoder half of the no-identity-at-the-store-boundary contract.
        ValueDomain::Unit
        | ValueDomain::List { .. }
        | ValueDomain::Map { .. }
        | ValueDomain::Identity { .. } => {
            return Err(ValueError::Unstorable);
        }
    };
    if bytes.len() > MAX_DURABLE_VALUE_BYTES {
        return Err(ValueError::ValueTooLarge);
    }
    Ok(bytes)
}

/// Write a composite value's leaves. A product writes each field leaf in order; a sum writes
/// its variant index then that variant's dense payload leaves. `depth` bounds nesting before
/// any byte is written, in exact step with [`read_composite`]: the top-level composite is
/// depth 1 and each nested composite is one deeper, so the two sides accept the same set.
fn write_composite(value: &ValueDomain, out: &mut Vec<u8>, depth: usize) -> Result<(), ValueError> {
    if depth > MAX_DURABLE_VALUE_DEPTH {
        return Err(ValueError::ValueTooDeep);
    }
    match value {
        ValueDomain::Product { fields, .. } => {
            for field in fields {
                // A dense durable struct has every leaf present; an absent slot is not a
                // storable inline value (optionality within a struct is an `Option` sum).
                let field = field.as_ref().ok_or(ValueError::Unstorable)?;
                write_member(field, out, depth)?;
            }
            Ok(())
        }
        ValueDomain::Sum {
            variant, payload, ..
        } => {
            encode_len(u64::from(*variant), out);
            for leaf in payload {
                write_member(leaf, out, depth)?;
            }
            Ok(())
        }
        _ => Err(ValueError::Unstorable),
    }
}

/// Write one member (leaf) of a composite: a scalar as a minimal-LEB128 length prefix then
/// its raw scalar bytes (capped per leaf); a nested composite schema-delimited (no prefix),
/// one level deeper — the mirror of [`read_member`].
fn write_member(value: &ValueDomain, out: &mut Vec<u8>, depth: usize) -> Result<(), ValueError> {
    match value {
        ValueDomain::Scalar(scalar) => {
            let bytes = encode_value(scalar)?;
            if bytes.len() > MAX_LEAF_BYTES {
                return Err(ValueError::ValueTooLarge);
            }
            encode_len(bytes.len() as u64, out);
            out.extend_from_slice(&bytes);
            Ok(())
        }
        ValueDomain::Product { .. } | ValueDomain::Sum { .. } => {
            write_composite(value, out, depth + 1)
        }
        _ => Err(ValueError::Unstorable),
    }
}

/// Decode canonical cell bytes as the value of `shape`, strictly. A top-level scalar reads
/// the whole cell; a composite is shape-driven and must consume the whole cell with no
/// trailing bytes. Returns `None` on any malformed or non-canonical input.
pub fn decode_domain(bytes: &[u8], shape: &ValueShape) -> Option<ValueDomain> {
    match shape.view() {
        ValueShapeRef::Scalar(kind) => decode_value(bytes, kind).map(ValueDomain::Scalar),
        ValueShapeRef::Product { .. } | ValueShapeRef::Sum { .. } => {
            let (value, used) = read_composite(bytes, shape, 1)?;
            (used == bytes.len()).then_some(value)
        }
    }
}

/// Read a composite value of `shape` from the front of `bytes`, returning it and the bytes
/// consumed. `depth` bounds nesting before allocation (Law 9).
fn read_composite(bytes: &[u8], shape: &ValueShape, depth: usize) -> Option<(ValueDomain, usize)> {
    if depth > MAX_DURABLE_VALUE_DEPTH {
        return None;
    }
    match shape.view() {
        ValueShapeRef::Product { ty, fields } => {
            let mut used = 0;
            let mut slots = Vec::with_capacity(fields.len());
            for field in fields {
                let (value, n) = read_member(bytes.get(used..)?, field, depth)?;
                slots.push(Some(value));
                used += n;
            }
            Some((ValueDomain::Product { ty, fields: slots }, used))
        }
        ValueShapeRef::Sum { ty, variants } => {
            let (index, mut used) = decode_len(bytes)?;
            let variant = usize::try_from(index).ok()?;
            let payload_shapes = variants.get(variant)?;
            let mut payload = Vec::with_capacity(payload_shapes.len());
            for leaf in payload_shapes {
                let (value, n) = read_member(bytes.get(used..)?, leaf, depth)?;
                payload.push(value);
                used += n;
            }
            Some((
                ValueDomain::Sum {
                    ty,
                    variant: variant as u16,
                    payload,
                },
                used,
            ))
        }
        ValueShapeRef::Scalar(_) => None,
    }
}

/// Read one member (leaf) of a composite of `shape` from the front of `bytes`: a scalar
/// reads its minimal-LEB128 length (capped) then that many raw scalar bytes; a nested
/// composite recurses one deeper.
fn read_member(bytes: &[u8], shape: &ValueShape, depth: usize) -> Option<(ValueDomain, usize)> {
    match shape.view() {
        ValueShapeRef::Scalar(kind) => {
            let (len, prefix) = decode_len(bytes)?;
            let len = usize::try_from(len).ok()?;
            if len > MAX_LEAF_BYTES {
                return None;
            }
            let leaf = bytes.get(prefix..prefix.checked_add(len)?)?;
            let scalar = decode_value(leaf, kind)?;
            Some((ValueDomain::Scalar(scalar), prefix + len))
        }
        ValueShapeRef::Product { .. } | ValueShapeRef::Sum { .. } => {
            read_composite(bytes, shape, depth + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeScalar, decode_value, encode_value};

    /// Every present scalar encodes to bytes that decode back under its own scalar
    /// type tag — the only cell discriminant. There is no null, optional, or
    /// tombstone cell value: absence is the lack of a cell, so the encode boundary
    /// only ever sees a present scalar.
    #[test]
    fn the_only_cell_discriminant_is_the_scalar_type_tag() {
        let values = [
            RuntimeScalar::Bool(true),
            RuntimeScalar::Int(-7),
            RuntimeScalar::Str("hello".into()),
            RuntimeScalar::Bytes(vec![0x00, 0xff]),
            RuntimeScalar::Date(0),
            RuntimeScalar::Duration(1_500_000_000),
            RuntimeScalar::Instant(0),
        ];
        for value in values {
            let bytes = encode_value(&value).expect("a present scalar encodes");
            assert_eq!(decode_value(&bytes, value.ty()), Some(value));
        }
    }
}

#[cfg(test)]
mod composite_codec {
    use super::{
        MAX_DURABLE_VALUE_DEPTH, MAX_LEAF_BYTES, RuntimeScalar, ScalarKind, ShapeBuildError,
        ValueError, ValueShape, ValueShapeBuilder, decode_domain, decode_value, encode_domain,
        encode_value,
    };
    use crate::equality::{ValueDomain, value_equality};

    fn scalar(kind: ScalarKind) -> ValueShape {
        ValueShape::scalar(kind)
    }

    /// A product of the given member shapes, through the sole minter.
    fn product(ty: u16, members: impl IntoIterator<Item = ValueShape>) -> ValueShape {
        let mut builder = ValueShapeBuilder::new();
        builder.open_product(ty);
        for member in members {
            builder.shape(member);
        }
        builder.close();
        builder.finish().expect("a bounded product builds")
    }
    fn di(v: i64) -> ValueDomain {
        ValueDomain::Scalar(RuntimeScalar::Int(v))
    }
    fn ds(s: &str) -> ValueDomain {
        ValueDomain::Scalar(RuntimeScalar::Str(s.into()))
    }
    /// An `Option`-shaped sum: variant 0 = none (empty payload), variant 1 = some(inner).
    fn opt_shape(inner: ValueShape) -> ValueShape {
        let mut builder = ValueShapeBuilder::new();
        builder
            .open_sum(9)
            .open_variant()
            .close()
            .open_variant()
            .shape(inner)
            .close()
            .close();
        builder.finish().expect("a bounded option sum builds")
    }
    fn none() -> ValueDomain {
        ValueDomain::Sum {
            ty: 9,
            variant: 0,
            payload: vec![],
        }
    }
    fn some(inner: ValueDomain) -> ValueDomain {
        ValueDomain::Sum {
            ty: 9,
            variant: 1,
            payload: vec![inner],
        }
    }

    /// A1/byte-identity KAT: a top-level scalar value encodes byte-for-byte as the existing
    /// scalar codec — the oracle-differential-preserving property. No length prefix, no tag.
    #[test]
    fn a_top_level_scalar_is_byte_identical_to_the_scalar_codec() {
        for value in [
            RuntimeScalar::Int(-42),
            RuntimeScalar::Str("hi\u{0}there".into()),
            RuntimeScalar::Bool(true),
            RuntimeScalar::Bytes(vec![0x00, 0xff]),
        ] {
            let raw = encode_value(&value).expect("scalar encodes");
            let domain =
                encode_domain(&ValueDomain::Scalar(value.clone())).expect("domain encodes");
            assert_eq!(
                domain, raw,
                "a top-level scalar carries no composite framing"
            );
            assert_eq!(
                decode_domain(&domain, &scalar(value.ty())),
                Some(ValueDomain::Scalar(value)),
            );
        }
    }

    /// A product (two int leaves) frames each leaf with a minimal length prefix, in schema
    /// order, and round-trips.
    #[test]
    fn a_product_frames_leaves_in_order_and_round_trips() {
        let shape = product(3, [scalar(ScalarKind::Int), scalar(ScalarKind::Str)]);
        let value = ValueDomain::Product {
            ty: 3,
            fields: vec![Some(di(5)), Some(ds("ab"))],
        };
        let bytes = encode_domain(&value).expect("product encodes");
        // len("5")=1, "5", len("ab")=2, "ab".
        assert_eq!(bytes, vec![0x01, b'5', 0x02, b'a', b'b']);
        assert_eq!(decode_domain(&bytes, &shape), Some(value));
    }

    /// A3: nested `Option` is an ordinary sum; `none`, `some(none)`, `some(some(v))` are three
    /// distinct values with three distinct encodings, each round-tripping.
    #[test]
    fn nested_option_is_three_distinct_values() {
        let shape = opt_shape(opt_shape(scalar(ScalarKind::Int)));
        let none_v = none();
        let some_none = some(none());
        let some_some = some(some(di(7)));

        let bs: Vec<_> = [&none_v, &some_none, &some_some]
            .iter()
            .map(|v| encode_domain(v).expect("encodes"))
            .collect();
        // Distinct encodings.
        assert_ne!(bs[0], bs[1]);
        assert_ne!(bs[1], bs[2]);
        assert_ne!(bs[0], bs[2]);
        // Canonical fingerprints: none = variant 0; some(none) = 1 then inner variant 0;
        // some(some(7)) = 1, 1, len("7")=1, "7".
        assert_eq!(bs[0], vec![0x00]);
        assert_eq!(bs[1], vec![0x01, 0x00]);
        assert_eq!(bs[2], vec![0x01, 0x01, 0x01, b'7']);
        for (v, b) in [
            (&none_v, &bs[0]),
            (&some_none, &bs[1]),
            (&some_some, &bs[2]),
        ] {
            assert_eq!(decode_domain(b, &shape).as_ref(), Some(v));
        }
    }

    /// A8: byte equality is value equality — for every pair in a corpus,
    /// `encode(a) == encode(b)` iff `value_equality(a, b)`, tested against the equality owner.
    #[test]
    fn byte_equality_conforms_to_value_domain_equality() {
        let corpus = [
            di(0),
            di(1),
            ds("a"),
            ds("a\u{0}"),
            none(),
            some(di(0)),
            some(di(1)),
            some(none()),
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(di(1)), Some(ds("x"))],
            },
            ValueDomain::Product {
                ty: 3,
                fields: vec![Some(di(1)), Some(ds("y"))],
            },
        ];
        for a in &corpus {
            for b in &corpus {
                let (ea, eb) = (encode_domain(a), encode_domain(b));
                if let (Ok(ea), Ok(eb)) = (ea, eb) {
                    assert_eq!(
                        ea == eb,
                        value_equality(a, b),
                        "byte-equality must match value equality for {a:?} vs {b:?}",
                    );
                }
            }
        }
    }

    /// Forged bytes are rejected, never normalized.
    #[test]
    fn forged_bytes_are_rejected() {
        let prod = product(3, [scalar(ScalarKind::Int), scalar(ScalarKind::Int)]);
        // Truncation: a leaf length says 2 but only 1 byte follows.
        assert_eq!(decode_domain(&[0x02, b'5'], &prod), None);
        // Trailing bytes after a complete value.
        assert_eq!(decode_domain(&[0x01, b'5', 0x01, b'6', 0xff], &prod), None);
        // Non-minimal length prefix (0x80 0x00 = non-minimal zero).
        assert_eq!(decode_domain(&[0x80, 0x00, 0x01, b'6'], &prod), None);
        // A non-canonical scalar leaf ("01" is not a canonical int).
        assert_eq!(decode_domain(&[0x02, b'0', b'1', 0x01, b'6'], &prod), None);

        // Out-of-range sum variant index (only 0/1 declared).
        let opt = opt_shape(scalar(ScalarKind::Int));
        assert_eq!(decode_domain(&[0x02], &opt), None);
    }

    /// Over-cap is a Law-9 refusal at encode; over-depth is refused one step earlier, at
    /// construction, so no over-deep shape exists to hand a decoder.
    ///
    /// This case previously proved the decoder refused a caller-built over-deep shape. That
    /// shape can no longer be built: the refusal moved from the entry point to the sole
    /// minter, which is the stronger property and the only one that closes the class — an
    /// entry point that refuses its argument still has to drop it, and dropping an
    /// unbounded recursive argument overflows the stack. The decoder's own depth guard
    /// stays as defense in depth over a representation defect.
    #[test]
    fn over_cap_is_refused_and_over_depth_is_unconstructible() {
        // An over-`MAX_LEAF_BYTES` scalar leaf inside a product is refused at encode.
        let big = ValueDomain::Product {
            ty: 3,
            fields: vec![Some(ds(&"x".repeat(MAX_LEAF_BYTES + 1)))],
        };
        assert!(encode_domain(&big).is_err());

        // The bound's own depth builds; one composite deeper has no minting route.
        assert!(nest_shape(MAX_DURABLE_VALUE_DEPTH).is_ok());
        assert_eq!(
            nest_shape(MAX_DURABLE_VALUE_DEPTH + 1),
            Err(ShapeBuildError::TooDeep),
        );

        // A hostile stream far past the bound costs O(bound) and returns the same verdict —
        // it neither recurses nor retains the commands it refused.
        assert_eq!(nest_shape(100_000), Err(ShapeBuildError::TooDeep));
    }

    /// A product value nested `composites` deep around one `int` leaf. One composite is the
    /// top level, so `composites == 1` is the shallowest case.
    fn nest_value(composites: usize) -> ValueDomain {
        let mut value = di(7);
        for _ in 0..composites {
            value = ValueDomain::Product {
                ty: 3,
                fields: vec![Some(value)],
            };
        }
        value
    }

    /// The shape that decodes [`nest_value`] of the same depth, or the builder's refusal.
    /// `MAX_DURABLE_VALUE_DEPTH` is the deepest that mints.
    fn nest_shape(composites: usize) -> Result<ValueShape, ShapeBuildError> {
        let mut builder = ValueShapeBuilder::new();
        for _ in 0..composites {
            builder.open_product(3);
        }
        builder.scalar(ScalarKind::Int);
        for _ in 0..composites {
            builder.close();
        }
        builder.finish()
    }

    /// The encode guard is the decode guard's exact twin: the encoder accepts precisely the
    /// nesting the decoder can read back, and refuses deeper before building any buffer.
    ///
    /// Without it the byte cap cannot stand in. Nesting adds no bytes, so an arbitrarily deep
    /// product encodes to the same two bytes its leaf occupies — the cap it was supposed to
    /// backstop can never fire, whatever its value. The encoder therefore both recursed
    /// unbounded on caller-shaped nesting and, below the abort, minted cells at nesting the
    /// decoder refuses: bytes written that no reader can ever read back.
    #[test]
    fn encode_refuses_past_the_shape_depth_bound() {
        // N — the deepest nesting the decoder accepts encodes, and round-trips.
        let value = nest_value(MAX_DURABLE_VALUE_DEPTH);
        let shape = nest_shape(MAX_DURABLE_VALUE_DEPTH).expect("the bound's own depth mints");
        let bytes = encode_domain(&value).expect("the deepest readable nesting encodes");
        assert_eq!(decode_domain(&bytes, &shape).as_ref(), Some(&value));

        // N+1 — one composite deeper is refused at encode, with the same bound the shape
        // minter enforces, so no cell can be written that no reader could read back. The
        // value side is still caller-built (stored durable values are a separate owner), so
        // the encoder's guard is the live refusal there.
        let deeper = nest_value(MAX_DURABLE_VALUE_DEPTH + 1);
        assert_eq!(encode_domain(&deeper), Err(ValueError::ValueTooDeep));
        assert_eq!(
            nest_shape(MAX_DURABLE_VALUE_DEPTH + 1),
            Err(ShapeBuildError::TooDeep),
        );

        // The refusal is a return, not an abort, at nesting far past the bound — and it is
        // reached before the buffer is built, so the byte cap is never consulted.
        let hostile = nest_value(4_096);
        assert_eq!(encode_domain(&hostile), Err(ValueError::ValueTooDeep));
    }

    /// The full round-trip law over a mixed corpus: `decode(encode(v), shape) == v`.
    #[test]
    fn encode_decode_round_trips_the_storable_set() {
        let cases = [
            (di(-1), scalar(ScalarKind::Int)),
            (ds("hi"), scalar(ScalarKind::Str)),
            (some(di(3)), opt_shape(scalar(ScalarKind::Int))),
            (none(), opt_shape(scalar(ScalarKind::Int))),
            (
                ValueDomain::Product {
                    ty: 3,
                    fields: vec![Some(di(1)), Some(some(ds("z")))],
                },
                product(
                    3,
                    [scalar(ScalarKind::Int), opt_shape(scalar(ScalarKind::Str))],
                ),
            ),
        ];
        for (value, shape) in cases {
            let bytes = encode_domain(&value).expect("encodes");
            assert_eq!(decode_domain(&bytes, &shape), Some(value));
        }
    }

    /// A collection, map, or unit value is not storable inline.
    #[test]
    fn non_storable_shapes_are_refused_at_encode() {
        assert!(encode_domain(&ValueDomain::Unit).is_err());
        assert!(
            encode_domain(&ValueDomain::List {
                idx: 0,
                items: vec![]
            })
            .is_err()
        );
        assert!(
            encode_domain(&ValueDomain::Map {
                idx: 0,
                entries: vec![]
            })
            .is_err()
        );
        // An entry identity is not a durable value: the encoder refuses it, the
        // store-boundary half of the no-identity-at-the-encoder contract.
        assert!(
            encode_domain(&ValueDomain::Identity {
                root: crate::equality::RootId(0),
                keys: vec![crate::codec::key::KeyScalar::Int(1)],
            })
            .is_err()
        );
        // Also proves the scalar codec is unchanged for a bare scalar leaf.
        assert_eq!(
            decode_value(b"5", ScalarKind::Int),
            Some(RuntimeScalar::Int(5))
        );
    }
}
