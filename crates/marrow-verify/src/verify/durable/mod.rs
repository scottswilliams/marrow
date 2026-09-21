//! Phase 2 durable table: the roots and their member graphs, the operation sites resolved
//! against the reconstructed graph, and the recomputed contract id.

mod members;
mod project;
mod sites;

pub(crate) use project::{
    is_flat_executable_root, member_flat_at_root, seal_branches, seal_groups, seal_root_indexes,
};

use super::model::DecodedRoot;
use super::reject;
use crate::reader::Reader;
use crate::reject::{
    Bound, Duplicate, Ref, Region, RejectionKind as Kind, VerifyPhase, VerifyRejection,
};
use crate::sealed::{SealedEnumType, SealedRecordType, SealedSite};
use marrow_image::{
    AdmittedGraphInputPlan, DurableContractGraph, DurableContractId, DurableGraphInputRefusal,
    DurableIndexShape, KeyColumn, RootOccurrenceDef, SemanticNode, SemanticPath, StrId, TypeId,
};
use members::{
    LedgerScope, MemberBudget, MemberClaim, claim_distinct, decode_indexes, decode_key_tuple,
    decode_members, read_id, take_distinct_id, tie_root_record, validate_branch_records,
};
use sites::decode_sites;
use std::rc::Rc;

/// The decoded durable graph: the roots, the sealed operation sites, each site's resolved
/// graph-node path (parallel to the sites), the recomputed contract id, and the graph's
/// node set.
///
/// The graph survives sealing for old/new store admission. Decoded roots and the graph
/// share Product rows and index arrays; retaining it does not reconstruct those facts.
pub(super) struct DecodedDurable {
    pub(super) graph: Rc<DurableContractGraph>,
    pub(super) roots: Vec<DecodedRoot>,
    pub(super) sites: Vec<SealedSite>,
    /// Each site's resolved graph-node path, parallel to `sites` by index.
    pub(super) site_paths: Vec<SemanticPath>,
    pub(super) contract: DurableContractId,
    pub(super) nodes: Vec<SemanticNode>,
}

/// The already-decoded tables a durable root resolves its surface references against: the
/// interned string pool, the record types, and the enum types.
///
/// They travel together because a root's decode reads all three at once — a name index
/// against the pool, an entry record index against the types, and a field's value shape
/// against both types and enums — and because no durable decode step is meaningful with a
/// subset of them.
#[derive(Clone, Copy)]
struct DecodedTables<'a> {
    strings: &'a [Rc<str>],
    types: &'a [SealedRecordType],
    enums: &'a [SealedEnumType],
}

/// The construction plan a section claiming `root_count` roots is decoded under.
///
/// The one structural count the section states is read and bounded by the caller, before
/// any graph exists and before a row is allocated, so a hostile root count is answered by
/// that one bound rather than by the allocator. The admitted counts follow from it: each
/// root declares at most one Product, and a declaration's command vector is admitted at
/// the image's own one-past-the-bound declaration width.
///
/// The mint is total, and nothing saturates in it: an admitted `root_count` is already
/// within `MAX_ROOTS`, which sits below every admitted-intake ceiling.
fn structural_plan(root_count: usize) -> AdmittedGraphInputPlan {
    AdmittedGraphInputPlan::admit(
        root_count,
        root_count,
        marrow_image::bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
}

/// A refused durable-graph command is a hostile image, carried as the graph owner's own
/// typed refusal.
fn reject_graph_input(refusal: DurableGraphInputRefusal) -> VerifyRejection {
    reject(VerifyPhase::Table, Kind::DurableGraph(refusal))
}

/// Decode the DURABLE table (section 0x03): up to `MAX_ROOTS` roots — preceded,
/// when any root is present, by the application's 16-byte ledger id — then the operation
/// sites, then the 32-byte durable-contract id closing the section. Each root
/// carries its ledger identity block (placement, product, and key ids plus one id
/// per record field). Every site is revalidated against the roots and record
/// types, every declaration ledger id in the section must be pairwise distinct
/// (a durable enum's sum and member ids are one per-declaration identity that
/// later fields of that enum reference rather than reclaim), and the
/// contract id is independently recomputed from the decoded graph and checked
/// against the carried bytes.
///
/// The section's three runs each have their own decoder below. This owns only what they
/// share: the one structural count, the plan minted from it, and the one contract graph
/// every run reads or writes.
pub(super) fn decode_durable(
    body: &[u8],
    strings: &[Rc<str>],
    types: &[SealedRecordType],
    enums: &[SealedEnumType],
) -> Result<DecodedDurable, VerifyRejection> {
    let tables = DecodedTables {
        strings,
        types,
        enums,
    };
    let mut reader = Reader::new(body);
    let root_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, Kind::OverBound(Bound::Roots)));
    }
    // The construction plan is minted before the graph exists, from the one structural
    // count just bounded: no plan, no graph.
    let plan = structural_plan(root_count);
    // This image's one durable contract graph: every declaration, occurrence, and field
    // value shape is decoded into it, so a repeated Product costs one declaration row and
    // the decoder never holds a second representation of a member graph or a value.
    let mut graph = DurableContractGraph::new();
    let roots = decode_roots(&mut reader, root_count, &plan, &mut graph, tables)?;

    // Reconstruct the durable graph's node set now, from the same view the contract id is
    // computed over, so every operation site resolves against this verifier's own
    // derivation of the graph rather than a compiler-side summary.
    let nodes = graph.contract_view().semantic_nodes();
    let (sites, site_paths) = decode_sites(&mut reader, &nodes, &roots)?;
    let contract = close_contract(&mut reader, &graph)?;
    Ok(DecodedDurable {
        graph: Rc::new(graph),
        roots,
        sites,
        site_paths,
        contract,
        nodes,
    })
}

/// Decode the section's root run: the application ledger identity that precedes a
/// non-empty run, then `root_count` roots, then the cross-root name check.
///
/// The ledger scope lives exactly as long as this run, because that is the run every
/// declaration id in the section is read during: the sites carry references to ids already
/// claimed here, and the contract-id tail carries none.
fn decode_roots(
    reader: &mut Reader<'_>,
    root_count: usize,
    plan: &AdmittedGraphInputPlan,
    graph: &mut DurableContractGraph,
    tables: DecodedTables<'_>,
) -> Result<Vec<DecodedRoot>, VerifyRejection> {
    let mut scope = LedgerScope::default();
    if root_count > 0 {
        let application = take_distinct_id(reader, &mut scope)?;
        graph.set_application_identity(application);
    }
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        roots.push(decode_root(reader, plan, graph, tables, &mut scope)?);
    }

    // A root's name keys its physical cell family, so two roots that resolve to the same
    // name would share one family — a later write to one silently overwriting the other.
    // The escape encoding is injective, so distinct name strings never collide physically;
    // reject only an image whose roots resolve to the same name string. Placement/product/
    // key ledger ids are already distinct across the table (`take_distinct_id`), so this
    // closes the one remaining cross-root physical-collision axis.
    for (i, root) in roots.iter().enumerate() {
        for other in &roots[..i] {
            if tables.strings[root.name as usize] == tables.strings[other.name as usize] {
                return Err(reject(
                    VerifyPhase::Table,
                    Kind::Duplicate(Duplicate::RootName),
                ));
            }
        }
    }
    Ok(roots)
}

/// Decode one durable root: its surface name, key tuple, entry record, placement and
/// Product ledger identities, its Product's member tree, and its managed indexes — then
/// tie the decoded declaration to the record tables and admit the occurrence.
fn decode_root(
    reader: &mut Reader<'_>,
    plan: &AdmittedGraphInputPlan,
    graph: &mut DurableContractGraph,
    tables: DecodedTables<'_>,
    scope: &mut LedgerScope,
) -> Result<DecodedRoot, VerifyRejection> {
    let name = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
    if name as usize >= tables.strings.len() {
        return Err(reject(VerifyPhase::Table, Kind::OutOfRange(Ref::String)));
    }
    // The key tuple: a count, then each column's scalar type and distinct
    // ledger id. Zero columns is a singleton root; the closed orderable
    // durable-key scalar set admits int, string, bool, bytes, date, and
    // instant per column (`duration` is a span, not an identity).
    let key_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        as usize;
    if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OverBound(Bound::KeyColumns),
        ));
    }
    let keys = decode_key_tuple(reader, key_count, scope, MemberClaim::Declaration)?;
    let record = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?;
    if record as usize >= tables.types.len() {
        return Err(reject(
            VerifyPhase::Table,
            Kind::OutOfRange(Ref::RecordType),
        ));
    }
    // The root placement is an occurrence identity: every root occupies its own, so
    // a repeated placement is a duplicate root occurrence — two rows that would
    // address one durable node — rather than a generic identity collision.
    let placement = read_id(reader)?;
    if !scope.placements.insert(placement) {
        return Err(reject(
            VerifyPhase::Table,
            Kind::Duplicate(Duplicate::RootPlacement),
        ));
    }
    claim_distinct(scope, placement)?;
    // The Product is a declaration identity. Its first occurrence claims it together
    // with its whole member/value graph; a later root carrying it is a reference that
    // claims nothing and must match the accepted declaration exactly.
    let product = read_id(reader)?;
    // A Product is a declaration and a root is an occurrence of it. The first
    // occurrence claims the declaration's ledger ids; a later one is a reference that
    // reclaims nothing and must state the identical graph and entry record, which the
    // declaration table itself decides — there is no second comparison here to drift
    // from the one the graph performs.
    let claim = if scope.products.insert(product) {
        claim_distinct(scope, product)?;
        MemberClaim::Declaration
    } else {
        MemberClaim::Reference
    };
    // The resource's durable member tree: top-level fields interleaved with
    // static `group` namespaces and keyed `branch` placements. A field's stored
    // value is drawn from the closed acyclic durable value set (a bare scalar, a
    // dense struct, or a closed enum with sum/member ids).
    let commands = decode_members(
        reader,
        MemberBudget::whole_declaration(),
        scope,
        graph,
        claim,
    )?;
    let members = graph
        .declare_product(plan, product, TypeId::from_index(record), commands)
        .map_err(reject_graph_input)?;
    // The member tree's top-level fields and groups are exactly the materialized
    // record's stored field slots followed by its trailing group slots, in order and
    // value shape: this ties the durable identity to the executable record so a
    // hostile image cannot claim one identity while executing over a different field
    // or group shape. A field slot's value-shape match recurses through the record and
    // enum tables, so a widened field (a nominal, struct, or enum) is checked as
    // thoroughly as a plain scalar; each group slot is a group record whose own fields
    // tie to its `Group` member's direct fields one level down.
    let record_fields = &tables.types[record as usize].fields;
    let values = graph.value_shapes();
    tie_root_record(record_fields, &members, tables.types, tables.enums, values)?;
    // Every keyed `branch` nested in the tree ties its own materialized record to
    // its direct field members the same way, one level down, so a hostile image
    // cannot claim a branch identity while executing over a different record shape.
    validate_branch_records(
        members.iter(),
        tables.types,
        tables.enums,
        tables.strings.len(),
        values,
    )?;
    // The root's managed indexes follow its member tree. Each index's `Index`
    // ledger id is a distinct id across the whole table; each projected component
    // must reference a real top-level field or identity key of this same root, so a
    // hostile image cannot forge a projection over a leaf that does not exist.
    //
    // The list becomes a shared owner here and is never copied again: the occurrence row
    // the graph holds and the decoded root the sealing phase reads are two handles on this
    // one allocation.
    let indexes: Rc<[DurableIndexShape]> =
        decode_indexes(reader, &keys, &members, scope, values)?.into();
    graph
        .add_root_occurrence(
            plan,
            product,
            RootOccurrenceDef {
                name: StrId::from_index(name),
                keys: keys
                    .iter()
                    .map(|(scalar, id)| KeyColumn {
                        scalar: *scalar,
                        id: *id,
                    })
                    .collect(),
                placement,
                indexes: Rc::clone(&indexes),
            },
        )
        .map_err(reject_graph_input)?;
    Ok(DecodedRoot {
        name,
        keys,
        record,
        placement,
        members,
        indexes,
    })
}

/// Close the section: read the carried 32-byte durable-contract id, refuse trailing bytes,
/// and check the id against one independently recomputed from the decoded graph.
///
/// The carried bytes are never trusted, so a hostile image that mutates a root or field
/// shape without re-minting the contract is refused here.
fn close_contract(
    reader: &mut Reader<'_>,
    graph: &DurableContractGraph,
) -> Result<DurableContractId, VerifyRejection> {
    let carried: [u8; 32] = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Table, Kind::Truncated(Region::Durable)))?
        .try_into()
        .expect("take(32) yields 32 bytes");
    if !reader.is_empty() {
        return Err(reject(VerifyPhase::Table, Kind::Trailing(Region::Durable)));
    }
    // A graph decoded from an image is inside the identity owner's payload ceiling by
    // construction — the payload is the same walk as the section, spelling a ledger
    // reference in 25 bytes where the section spelled 16 — so the refusal is a bound this
    // decode cannot reach. It is answered rather than assumed: an image whose graph
    // somehow priced its own identity out of reach carries an identity nothing can
    // recompute, which is exactly a contract that does not match its graph.
    let recomputed = graph
        .contract_view()
        .contract_id()
        .map_err(|_| reject(VerifyPhase::Table, Kind::ContractUnidentifiable))?;
    if recomputed.bytes() != &carried {
        return Err(reject(VerifyPhase::Table, Kind::ContractMismatch));
    }
    Ok(recomputed)
}
