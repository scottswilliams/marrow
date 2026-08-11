//! Phase 2 durable graph: root/branch/group/index decoding, sealing, and shape descriptors.

use super::model::{DecodedEnum, DecodedField, DecodedRecordType, DecodedRoot};
use super::reject;
use super::tables::decode_bare_scalar;
use crate::reader::Reader;
use crate::reject::{VerifyPhase, VerifyRejection};
use crate::sealed::{
    SealedBranch, SealedGroup, SealedIndex, SealedIndexComponent, SealedRecordType, SealedSite,
    SealedSiteTarget,
};
use marrow_image::{
    AdmittedGraphInputPlan, CanonicalValueShapeDag, DeclarationMemberDef, DeclarationMemberShape,
    DurableContractGraph, DurableContractId, DurableGraphInputRefusal, DurableIndexComponent,
    DurableIndexShape, DurableMemberView, DurableMemberViewKind, DurableMemberViews,
    DurableProductGraph, ImageType, KeyColumn, LedgerIdBytes, RootOccurrenceDef, Scalar,
    SemanticNode, SemanticNodeKind, SemanticPath, SemanticStep, SemanticStepKind, SemanticTarget,
    StrId, TypeId, ValueShapeNodeId, ValueShapeView,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// The verifier-side maximum-live durable graph, in bytes.
///
/// The verifier cannot state its populations the way the compiler's admission owner can:
/// it holds received bytes, and what they decode to is the sender's choice. Its extremum is
/// therefore the received bytes themselves — bounded by the container ceiling before this
/// phase is reached — times the widest live-bytes-per-wire-byte rate `marrow-image`
/// publishes for the representation it decodes into, times the growth-and-copy factor a
/// table still growing one row at a time is live at. The received bytes are charged again
/// on top, because the decoder borrows the whole section throughout.
///
/// One accepted graph is charged, not two: acceptance consumes the transient decode state
/// into the graph rather than copying it, and a refusal returns before the section is fully
/// decoded, so hostile refusal state is strictly under the accepted case.
const MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES: u64 = marrow_image::bounds::GROWTH_AND_COPY
    * marrow_image::bounds::DURABLE_LIVE_BYTES_PER_WIRE_BYTE
    * marrow_image::bounds::MAX_IMAGE_BYTES as u64
    + marrow_image::bounds::MAX_IMAGE_BYTES as u64
    + marrow_image::bounds::DURABLE_GRAPH_FIXED_BYTES;

/// The declared ceiling for what one hostile image may make the verifier hold live.
///
/// Declared, not derived from the sum. The verifier is the boundary an untrusted image
/// meets, so its ceiling is stated against what the host must survive rather than against
/// what the current representation happens to cost: a widening that raised both sides
/// equally would prove nothing.
const H_VERIFIED_DURABLE_GRAPH_BYTES: u64 = 256 * 1024 * 1024;

/// Prove the verifier-side equation closes at compile time. A representation or ceiling
/// change that let a container-admitted image drive more live bytes than the host accepts
/// fails the build here rather than at some later measurement.
const _: () = assert!(
    MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES <= H_VERIFIED_DURABLE_GRAPH_BYTES,
    "a container-admitted image can drive the verifier past its declared live ceiling",
);

/// Decode the DURABLE table (design §C 0x03): up to `MAX_ROOTS` roots — preceded,
/// when any root is present, by the application's 16-byte ledger id — then the operation
/// sites, then the 32-byte durable-contract id closing the section. Each root
/// carries its ledger identity block (placement, product, and key ids plus one id
/// per record field). Every site is revalidated against the roots and record
/// types, every declaration ledger id in the section must be pairwise distinct
/// (a durable enum's sum and member ids are one per-declaration identity that
/// later fields of that enum reference rather than reclaim), and the
/// contract id is independently recomputed from the decoded graph and checked
/// against the carried bytes.
/// The decoded durable graph: the one contract graph the section decoded into, the roots,
/// the sealed operation sites, each site's resolved graph-node path (parallel to the
/// sites), the recomputed contract id, and the graph's node set.
///
/// The contract graph owns the value-shape arena, and it is returned rather than dropped:
/// a decoded member row references its field's shape by node id, so the owner of those
/// nodes outlives every row that addresses one.
type DecodedDurable = (
    DurableContractGraph,
    Vec<DecodedRoot>,
    Vec<SealedSite>,
    Vec<SemanticPath>,
    DurableContractId,
    Vec<SemanticNode>,
);

/// Mint the construction plan this section is decoded under, from an allocation-free
/// structural preflight over bytes the image container ceiling already bounds.
///
/// The preflight reads the one structural count the section states before any graph exists
/// — how many roots it claims — and refuses root N+1 **there**, before a plan is minted or
/// a row is allocated, so a hostile root count is answered by the bound rather than by the
/// allocator. The admitted counts follow from it: each root declares at most one Product,
/// and a declaration's command vector is admitted at the image's own one-past-the-bound
/// declaration width, which the per-Product member budget then spends exactly.
///
/// It consumes nothing: the returned reader position is the caller's own, so the root
/// count is read once by the decode that follows.
fn admit_structural_plan(body: &[u8]) -> Result<AdmittedGraphInputPlan, VerifyRejection> {
    let mut preflight = Reader::new(body);
    let root_count = preflight
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))? as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, "too many durable roots"));
    }
    AdmittedGraphInputPlan::admit(
        root_count,
        root_count,
        marrow_image::bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
    )
    .ok_or(reject(VerifyPhase::Table, "too many durable roots"))
}

/// Project one refused flat durable-graph command into this verifier's own typed
/// hostile-image detail. A hostile image states the graph; the refusal names what it
/// stated wrongly, in the same `&'static str` form every other rejection in this decode
/// carries.
fn reject_graph_input(refusal: DurableGraphInputRefusal) -> VerifyRejection {
    reject(
        VerifyPhase::Table,
        match refusal {
            DurableGraphInputRefusal::DivergentGraph => {
                "a repeated durable Product declares a different member graph"
            }
            DurableGraphInputRefusal::DivergentEntryRecord => {
                "a repeated durable Product declares a different entry record"
            }
            DurableGraphInputRefusal::OverPlan
            | DurableGraphInputRefusal::UnaddressableOccurrence => "too many durable roots",
            DurableGraphInputRefusal::MalformedCommands
            | DurableGraphInputRefusal::UndeclaredProduct => {
                "durable member graph is not a well-formed declaration"
            }
        },
    )
}

pub(super) fn decode_durable(
    body: &[u8],
    strings: &[Rc<str>],
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> Result<DecodedDurable, VerifyRejection> {
    let string_count = strings.len();
    // The construction plan is minted before the graph exists, from a structural preflight
    // over the received bytes: no plan, no graph.
    let plan = admit_structural_plan(body)?;
    let mut reader = Reader::new(body);
    let root_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short root count"))? as usize;
    if root_count > marrow_image::bounds::MAX_ROOTS {
        return Err(reject(VerifyPhase::Table, "too many durable roots"));
    }
    let mut scope = LedgerScope::default();
    // This image's one durable contract graph. Every declaration, occurrence, and field
    // value shape is decoded into it and referenced from there, so a repeated Product
    // costs one declaration row and the decoder never holds a second representation of a
    // member graph or of a value.
    let mut graph = DurableContractGraph::new();
    if root_count > 0 {
        let application = take_distinct_id(&mut reader, &mut scope, "short application identity")?;
        graph.set_application_identity(application);
    }
    let mut roots = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        let name = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root name"))?;
        if name as usize >= string_count {
            return Err(reject(VerifyPhase::Table, "root name index out of range"));
        }
        // The key tuple: a count, then each column's scalar type and distinct
        // ledger id. Zero columns is a singleton root; the closed orderable
        // durable-key scalar set (frozen at C04) admits int, string, bool, bytes,
        // date, and instant per column (`duration` is a span, not an identity).
        let key_count = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root key count"))?
            as usize;
        if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
            return Err(reject(VerifyPhase::Table, "too many root key columns"));
        }
        let keys = decode_key_tuple(&mut reader, key_count, &mut scope, MemberClaim::Declaration)?;
        let record = reader
            .u16()
            .ok_or(reject(VerifyPhase::Table, "short root record"))?;
        if record as usize >= types.len() {
            return Err(reject(
                VerifyPhase::Table,
                "root record type index out of range",
            ));
        }
        // The root placement is an occurrence identity: every root occupies its own, so
        // a repeated placement is a duplicate root occurrence — two rows that would
        // address one durable node — rather than a generic identity collision.
        let placement = read_id(&mut reader, "short placement identity")?;
        if !scope.placements.insert(placement) {
            return Err(reject(
                VerifyPhase::Table,
                "duplicate durable root occurrence",
            ));
        }
        claim_distinct(&mut scope, placement)?;
        // The Product is a declaration identity. Its first occurrence claims it together
        // with its whole member/value graph; a later root carrying it is a reference that
        // claims nothing and must match the accepted declaration exactly.
        let product = read_id(&mut reader, "short product identity")?;
        // The resource's durable member tree: top-level fields interleaved with
        // static `group` namespaces and keyed `branch` placements. A field's stored
        // value is drawn from the closed acyclic durable value set (a bare scalar, a
        // dense struct, or a closed enum with sum/member ids).
        let mut member_budget = marrow_image::bounds::MAX_DURABLE_MEMBERS;
        // A Product is a declaration and a root is an occurrence of it. The first
        // occurrence claims the declaration's ledger ids; a later one is a reference that
        // reclaims nothing and must state the identical graph and entry record, which the
        // declaration table itself decides — there is no second comparison here to drift
        // from the one the graph performs.
        let claim = if scope.products.insert(product) {
            claim_distinct(&mut scope, product)?;
            MemberClaim::Declaration
        } else {
            MemberClaim::Reference
        };
        let commands = decode_members(
            &mut reader,
            &mut member_budget,
            &mut scope,
            &mut graph,
            claim,
        )?;
        let members = graph
            .declare_product(&plan, product, TypeId::from_index(record), commands)
            .map_err(reject_graph_input)?;
        // The member tree's top-level fields and groups are exactly the materialized
        // record's stored field slots followed by its trailing group slots, in order and
        // value shape: this ties the durable identity to the executable record so a
        // hostile image cannot claim one identity while executing over a different field
        // or group shape. A field slot's value-shape match recurses through the record and
        // enum tables, so a widened field (a nominal, struct, or enum) is checked as
        // thoroughly as a plain scalar; each group slot is a group record whose own fields
        // tie to its `Group` member's direct fields one level down.
        let record_fields = &types[record as usize].fields;
        let values = graph.value_shapes();
        tie_root_record(record_fields, &members, types, enums, values)?;
        // Every keyed `branch` nested in the tree ties its own materialized record to
        // its direct field members the same way, one level down, so a hostile image
        // cannot claim a branch identity while executing over a different record shape.
        validate_branch_records(members.iter(), types, enums, string_count, values)?;
        // The root's managed indexes follow its member tree. Each index's `Index`
        // ledger id is a distinct id across the whole table; each projected component
        // must reference a real top-level field or identity key of this same root, so a
        // hostile image cannot forge a projection over a leaf that does not exist.
        let indexes = decode_indexes(&mut reader, &keys, &members, &mut scope, values)?;
        graph
            .add_root_occurrence(
                &plan,
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
                    indexes: indexes.clone(),
                },
            )
            .map_err(reject_graph_input)?;
        roots.push(DecodedRoot {
            name,
            keys,
            record,
            placement,
            members,
            indexes,
        });
    }

    // A root's name keys its physical cell family, so two roots that resolve to the same
    // name would share one family — a later write to one silently overwriting the other.
    // The escape encoding is injective, so distinct name strings never collide physically;
    // reject only an image whose roots resolve to the same name string. Placement/product/
    // key ledger ids are already distinct across the table (`take_distinct_id`), so this
    // closes the one remaining cross-root physical-collision axis.
    for (i, root) in roots.iter().enumerate() {
        for other in &roots[..i] {
            if strings[root.name as usize] == strings[other.name as usize] {
                return Err(reject(VerifyPhase::Table, "two durable roots share a name"));
            }
        }
    }

    // Reconstruct the durable graph's node set now, from the same view the contract id is
    // computed over, so every operation site resolves against this verifier's own
    // derivation of the graph rather than a compiler-side summary.
    let nodes = graph.contract_view().semantic_nodes();
    // Project that node set once into its keyed form: each node's executable
    // coordinates, keyed by its path. A site then resolves in one lookup.
    let projection = project_graph(&nodes, &roots);

    let site_count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short site count"))? as usize;
    if site_count > marrow_image::bounds::MAX_SITES {
        return Err(reject(VerifyPhase::Table, "too many durable sites"));
    }
    let mut sites: Vec<SealedSite> = Vec::with_capacity(site_count);
    // Each site's resolved graph-node path, parallel to `sites` by index. The demand
    // reconstruction maps a durable opcode's site index to the semantic path of the
    // node it addresses; a flat site drops the path from its executable form, so it
    // is retained here rather than re-derived.
    let mut site_paths: Vec<SemanticPath> = Vec::with_capacity(site_count);
    // The resolved identities already claimed, keyed. Sites are unique by that identity:
    // a flat site by (root, target), a parked site by (path, target), and a flat and a
    // parked site can never collide. Comparing each fresh site against every retained one
    // instead was a deep structural comparison of whole path chains and branch paths, so
    // the claim cost grew with the square of the site count while the answer — and the
    // wire ordinal the first duplicate is refused at — is exactly this one.
    let mut claimed: HashSet<SealedSite> = HashSet::with_capacity(site_count);
    for _ in 0..site_count {
        let (site, path) = decode_site(&mut reader, &projection)?;
        if !claimed.insert(site.clone()) {
            return Err(reject(VerifyPhase::Table, "duplicate durable site"));
        }
        sites.push(site);
        site_paths.push(path);
    }

    // The section closes with the 32-byte durable-contract id. Recompute it
    // independently from the decoded graph — never trust the carried bytes — and
    // reject a mismatch, so a hostile image that mutates a root or field shape
    // without re-minting the contract is refused here.
    let carried: [u8; 32] = reader
        .take(32)
        .ok_or(reject(VerifyPhase::Table, "short durable contract id"))?
        .try_into()
        .expect("take(32) yields 32 bytes");
    if !reader.is_empty() {
        return Err(reject(
            VerifyPhase::Table,
            "trailing bytes in durable table",
        ));
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
        .map_err(|_| reject(VerifyPhase::Table, "durable graph is too large to identify"))?;
    if recomputed.bytes() != &carried {
        return Err(reject(
            VerifyPhase::Table,
            "durable contract id does not match the durable graph",
        ));
    }
    // The graph is the one owner: the roots reference its declaration rows and its arena,
    // and its two derivations — the recomputed id and the node set — are the only
    // projections of it that leave this decode. No caller retains a second one.
    Ok((graph, roots, sites, site_paths, recomputed, nodes))
}

/// Decode one operation site — its semantic path then its target-kind byte — and
/// resolve it against the reconstructed node set. The path is `u8(step_count) ‖
/// [u8(ledger_kind) ‖ 16 id bytes]*`; the target byte is `0x00` whole-payload or
/// `0x01` field-leaf. Nothing here is trusted: the path is resolved to a node and
/// its kind cross-checked, and the executable physical facts are re-derived, so a
/// forged path, a flipped target byte, or a mutated ledger id is refused.
fn decode_site(
    reader: &mut Reader<'_>,
    graph: &GraphProjection<'_>,
) -> Result<(SealedSite, SemanticPath), VerifyRejection> {
    let step_count = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site path length"))? as usize;
    if step_count < marrow_image::bounds::MIN_SITE_PATH_STEPS {
        return Err(reject(
            VerifyPhase::Table,
            "durable site path names no graph node",
        ));
    }
    if step_count > marrow_image::bounds::MAX_SITE_PATH_STEPS {
        return Err(reject(VerifyPhase::Table, "durable site path too deep"));
    }
    let mut steps = Vec::with_capacity(step_count);
    for _ in 0..step_count {
        let kind_byte = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short site path step kind"))?;
        let kind = SemanticStepKind::from_ledger_kind(kind_byte)
            .ok_or(reject(VerifyPhase::Table, "unknown site path step kind"))?;
        let id_bytes: [u8; 16] = reader
            .take(16)
            .ok_or(reject(VerifyPhase::Table, "short site path step id"))?
            .try_into()
            .expect("take(16) yields 16 bytes");
        steps.push(SemanticStep::new(kind, LedgerIdBytes::from_bytes(id_bytes)));
    }
    let path = SemanticPath::try_from_steps(steps)
        .map_err(|_| reject(VerifyPhase::Table, "durable site path names no graph node"))?;
    let target = match reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short site target"))?
    {
        0x00 => SemanticTarget::WholePayload,
        0x01 => SemanticTarget::FieldLeaf,
        0x02 => SemanticTarget::IndexScan,
        0x03 => SemanticTarget::IndexLookup,
        0x04 => SemanticTarget::GroupEntry,
        _ => return Err(reject(VerifyPhase::Table, "unknown site target tag")),
    };
    let site = resolve_site(&path, target, graph)?;
    // The site's node path is the chain it resolved against — retained parallel to
    // the sealed site so demand reconstruction can name the node a flat site
    // addresses without re-deriving it from the executable form.
    Ok((site, path))
}

/// Resolve a decoded site path plus target kind to a [`SealedSite`]. A path that
/// names no reconstructed node, or a target whose kind disagrees with the resolved
/// node's kind, is refused. A whole-payload, keyed-branch-entry, or field-leaf site on
/// a flat-executable keyed root seals as [`SealedSite::Flat`] with its re-derived root
/// index and (for a field leaf) resolved field index — widened field values, composite
/// keys, and keyed branches nested to any depth all execute. A managed-index read also
/// seals [`SealedSite::Flat`] with its re-derived index position and read-kind target. A
/// site on a non-flat root — a singleton (keyless) root, or a group at any level — seals
/// as [`SealedSite::Parked`], carrying the resolved path and target. Both forms re-derive
/// everything from the reconstructed graph, never trusting the image.
fn resolve_site(
    path: &SemanticPath,
    target: SemanticTarget,
    graph: &GraphProjection<'_>,
) -> Result<SealedSite, VerifyRejection> {
    let node = graph.nodes.get(path.steps()).ok_or(reject(
        VerifyPhase::Table,
        "durable site path does not resolve to a graph node",
    ))?;
    // The target kind must agree with the resolved node's kind: a whole-payload
    // target names a keyed placement, a field-leaf target names a stored field, and an
    // index scan/lookup target names a managed index node.
    match (target, node.kind) {
        (SemanticTarget::WholePayload, SemanticNodeKind::Root | SemanticNodeKind::Branch) => {}
        (SemanticTarget::FieldLeaf, SemanticNodeKind::Field) => {}
        (SemanticTarget::GroupEntry, SemanticNodeKind::Group) => {}
        (SemanticTarget::IndexScan | SemanticTarget::IndexLookup, SemanticNodeKind::Index) => {}
        _ => {
            return Err(reject(
                VerifyPhase::Table,
                "durable site target kind does not match its resolved graph node",
            ));
        }
    }
    // An index read site resolves to its managed index and seals flat-executable, carrying
    // the index's global position and read-kind target for the VM's bounded scan/lookup. The
    // read kind must agree with the index's `unique` flag: a nonunique index admits
    // only a progressive-prefix `IndexScan`, and a unique index admits only a
    // complete-key `IndexLookup`. This is where a site that claims to *traverse* a
    // unique index — or to exact-lookup a nonunique one — is refused, so source can
    // never observe siblings through a unique index.
    if let NodeCoordinates::Index { global, unique } = node.coordinates {
        let agrees = match target {
            SemanticTarget::IndexScan => !unique,
            SemanticTarget::IndexLookup => unique,
            _ => unreachable!("only an index node admits an index read target"),
        };
        if !agrees {
            return Err(reject(
                VerifyPhase::Table,
                "durable index site read kind disagrees with the index's unique flag",
            ));
        }
        let sealed_target = match target {
            SemanticTarget::IndexScan => SealedSiteTarget::IndexScan(global),
            SemanticTarget::IndexLookup => SealedSiteTarget::IndexLookup(global),
            _ => unreachable!("only an index node admits an index read target"),
        };
        return Ok(SealedSite::Flat {
            root: node.root,
            target: sealed_target,
        });
    }
    // A flat-executable keyed root — keyed, with every member a field or a simple keyed
    // branch (no group at any level) — is kernel-executable: a whole-payload or
    // keyed-branch-entry site, or a field-leaf site (scalar or widened value), at any
    // branch depth. A site on a non-flat root — a singleton, or a group at any level —
    // seals as parked (identity complete, execution deferred), as does any node the flat
    // kernel cannot address: a node reached through a `group` namespace, or a group
    // nested below the root's own members.
    let parked = || SealedSite::Parked {
        path: path.clone(),
        target,
    };
    if !graph.flat_roots[node.root as usize] {
        return Ok(parked());
    }
    let sealed = match &node.coordinates {
        NodeCoordinates::Root => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::WholePayload,
        },
        NodeCoordinates::Branch(branch) => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::BranchEntry(branch.clone()),
        },
        NodeCoordinates::Field { branch, field } if branch.is_empty() => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::FieldLeaf(*field),
        },
        NodeCoordinates::Field { branch, field } => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::BranchField {
                branch: branch.clone(),
                field: *field,
            },
        },
        NodeCoordinates::Group(group) => SealedSite::Flat {
            root: node.root,
            target: SealedSiteTarget::GroupEntry(*group),
        },
        NodeCoordinates::Unaddressable => parked(),
        NodeCoordinates::Index { .. } => {
            unreachable!("an index node sealed and returned before this point")
        }
    };
    Ok(sealed)
}

/// The keyed projection of one durable table's reconstructed graph: every node's kind,
/// enclosing root occurrence, and executable coordinates, keyed by the node's path.
///
/// It is derived in one pass from the same [`SemanticNode`] set the contract id is
/// computed over — this verifier's own reconstruction — so it introduces no second graph,
/// path, or identity owner. Resolving a site is then one keyed lookup: previously each
/// site scanned the whole node set for its path and then re-walked the root's member tree
/// to recover its branch/field/group ordinals, so the cost of sealing a table grew with
/// the product of its site count and its graph size.
struct GraphProjection<'a> {
    nodes: HashMap<&'a [SemanticStep], ProjectedNode>,
    /// Whether the root occurrence at each DURABLE-table position is the flat keyed root
    /// the kernel executes, by table position.
    flat_roots: Vec<bool>,
}

/// One reconstructed graph node's resolved facts.
struct ProjectedNode {
    kind: SemanticNodeKind,
    /// The DURABLE-table position of the root occurrence this node hangs under. Every
    /// node's path carries that root's placement as its second step.
    root: u16,
    coordinates: NodeCoordinates,
}

/// A graph node's executable coordinates under its root occurrence — the positions the
/// path kernel addresses it by, re-derived from the graph rather than trusted from the
/// image.
enum NodeCoordinates {
    /// The root occurrence's own entry.
    Root,
    /// A keyed branch, by its per-level branch index from the root down.
    Branch(Box<[u16]>),
    /// A stored field, by its containing node's branch path and its index among that
    /// node's direct fields.
    Field { branch: Box<[u16]>, field: u16 },
    /// A root-level unkeyed group, by its index among the root's direct groups.
    Group(u16),
    /// A managed index, by its position in the image-wide index table and its `unique`
    /// flag.
    Index { global: u16, unique: bool },
    /// A node the flat kernel cannot address whatever its root: one reached through a
    /// `group` namespace, or a group below the root's own members. It parks.
    Unaddressable,
}

/// The per-parent ordinals a node takes among its same-kind siblings, in declaration
/// order. A field's ordinal is its index into its container's materialized record (their
/// orders are tied during decode), a branch's is its index into the sealed branch list at
/// its level, and a group's is its index into its root's sealed group list.
#[derive(Default)]
struct SiblingOrdinals {
    fields: u16,
    groups: u16,
    branches: u16,
}

/// The fixed prefix every managed-index row consumes, in the order [`decode_indexes`]
/// reads it: a 16-byte ledger id, a 1-byte `unique` flag, and a 2-byte component count.
/// Its components follow and only add to the cost, so this is the row's lower bound.
const MIN_DECODED_INDEX_ROW_BYTES: usize = 16 + 1 + 2;

/// An image-wide managed-index position fits the `u16` [`project_graph`] spells it with.
///
/// The per-root structural bounds do **not** establish this: `MAX_ROOTS * MAX_INDEXES`
/// is 4096 × 32 = 131,072, twice what a `u16` carries. The bound is the container
/// ceiling instead. Every index row is decoded from a reader over bytes already limited
/// to `MAX_IMAGE_BYTES` by `decode_container`, and each row consumes at least
/// [`MIN_DECODED_INDEX_ROW_BYTES`], so an image carries at most
/// `MAX_IMAGE_BYTES / MIN_DECODED_INDEX_ROW_BYTES` = 27,594 of them — comfortably
/// inside `u16::MAX`. An image stating more rows than that runs out of bytes and is
/// rejected before this projection is reached.
const _: () = assert!(
    marrow_image::bounds::MAX_IMAGE_BYTES / MIN_DECODED_INDEX_ROW_BYTES < u16::MAX as usize,
    "an image-wide managed-index position must fit the u16 the graph projection spells it with",
);

/// Project a reconstructed node set into its keyed form.
///
/// The node set is in pre-order — a node precedes its descendants — so each node's
/// coordinates are its parent's extended by its own ordinal among its same-kind siblings,
/// and both are already known when it is reached. Nothing is re-derived per site.
fn project_graph<'a>(nodes: &'a [SemanticNode], roots: &[DecodedRoot]) -> GraphProjection<'a> {
    let mut root_positions: HashMap<LedgerIdBytes, u16> = HashMap::with_capacity(roots.len());
    for (position, root) in roots.iter().enumerate() {
        root_positions.insert(root.placement, position as u16);
    }
    // Each managed index by its ledger id: its position in the image-wide index table,
    // assembled by iterating the roots in order and each root's indexes in order — the
    // same order the sealed index list is built in, so the position indexes that list
    // directly — and its `unique` flag. The narrowing to `u16` cannot truncate: the
    // count is bounded by the container ceiling, not by the per-root structural bounds
    // (see the const assert above).
    let mut index_positions: HashMap<LedgerIdBytes, (u16, bool)> = HashMap::new();
    let mut global: usize = 0;
    for root in roots {
        for index in &root.indexes {
            index_positions.insert(index.id, (global as u16, index.unique));
            global += 1;
        }
    }

    let mut ordinals: HashMap<&'a [SemanticStep], SiblingOrdinals> = HashMap::new();
    let mut projected: HashMap<&'a [SemanticStep], ProjectedNode> =
        HashMap::with_capacity(nodes.len());
    for node in nodes {
        let steps = node.path.steps();
        let (_, parent) = steps
            .split_last()
            .expect("a semantic path carries at least one step");
        // Every node hangs under a root occurrence whose placement is its second step:
        // a root's own path is `[application, placement]` and every descendant extends it.
        let root = *root_positions
            .get(&steps[marrow_image::bounds::MIN_SITE_PATH_STEPS - 1].id)
            .expect("a reconstructed node hangs under a decoded root");
        let sibling = ordinals.entry(parent).or_default();
        let coordinates = match node.kind {
            SemanticNodeKind::Root => NodeCoordinates::Root,
            SemanticNodeKind::Field => {
                let field = sibling.fields;
                sibling.fields += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Field {
                        branch: Box::default(),
                        field,
                    },
                    Some(NodeCoordinates::Branch(branch)) => NodeCoordinates::Field {
                        branch: branch.clone(),
                        field,
                    },
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Group => {
                let group = sibling.groups;
                sibling.groups += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Group(group),
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Branch => {
                let branch = sibling.branches;
                sibling.branches += 1;
                match projected.get(parent).map(|node| &node.coordinates) {
                    Some(NodeCoordinates::Root) => NodeCoordinates::Branch(Box::new([branch])),
                    Some(NodeCoordinates::Branch(above)) => {
                        let mut path = above.to_vec();
                        path.push(branch);
                        NodeCoordinates::Branch(path.into())
                    }
                    _ => NodeCoordinates::Unaddressable,
                }
            }
            SemanticNodeKind::Index => match index_positions.get(&node.path.node_id()) {
                Some(&(global, unique)) => NodeCoordinates::Index { global, unique },
                None => NodeCoordinates::Unaddressable,
            },
        };
        projected.insert(
            steps,
            ProjectedNode {
                kind: node.kind,
                root,
                coordinates,
            },
        );
    }

    GraphProjection {
        nodes: projected,
        flat_roots: roots.iter().map(is_flat_executable_root).collect(),
    }
}

/// Whether a decoded root is the flat keyed root the kernel executes: at least one key
/// column and a member tree of top-level storable-value fields (scalar or widened) and
/// keyed branches of the same shape (no group). The key may be single-column or a composite
/// tuple, at the root and at every branch. Re-derived from the decoded graph, so the
/// flat/parked classification never trusts a compiler summary.
pub(super) fn is_flat_executable_root(root: &DecodedRoot) -> bool {
    !root.keys.is_empty() && root.members.iter().all(member_flat_at_root)
}

/// Whether one member is a field-only keyed branch, at every level below it — the branch
/// shape the kernel executes at any depth. Its key is one or more columns and every direct
/// member itself keeps flat: a field (scalar or widened composite), or a nested branch that
/// is itself a simple branch. A static `group` breaks it. The rule admits an arbitrarily
/// deep chain of field-only branches with composite keys, which the recursive physical
/// layout and profile serve.
///
/// The descent is an explicit stack of member runs, not recursion: nesting depth is a
/// property of the rows the image stated, and this predicate's own stack use must not be.
fn is_simple_branch(member: DurableMemberView<'_>) -> bool {
    let DurableMemberViewKind::Branch(branch) = member.kind() else {
        return false;
    };
    if branch.keys().is_empty() {
        return false;
    }
    let mut stack = vec![member.members()];
    while let Some(run) = stack.last_mut() {
        let Some(inner) = run.next() else {
            stack.pop();
            continue;
        };
        match inner.kind() {
            DurableMemberViewKind::Field(_) => {}
            DurableMemberViewKind::Group(_) => return false,
            DurableMemberViewKind::Branch(nested) => {
                if nested.keys().is_empty() {
                    return false;
                }
                stack.push(inner.members());
            }
        }
    }
    true
}

/// Whether a root's *direct* member keeps the root flat-executable. It admits one more
/// shape than [`is_simple_branch`]'s inner rule: a root-level unkeyed `group` whose own
/// members are all storable-value fields (a scalar or widened composite). A group is a
/// value unit of the root entry, executable at the root level, but a group nested in a
/// branch or in another group still parks — the branch rule keeps `Group => false`, so a
/// group below the root's direct members never makes its enclosing branch flat.
pub(super) fn member_flat_at_root(member: DurableMemberView<'_>) -> bool {
    match member.kind() {
        DurableMemberViewKind::Field(_) => true,
        DurableMemberViewKind::Group(_) => member
            .members()
            .all(|inner| matches!(inner.kind(), DurableMemberViewKind::Field(_))),
        DurableMemberViewKind::Branch(_) => is_simple_branch(member),
    }
}

/// Seal a member tree's keyed branches into the recursive [`SealedBranch`] tree, in
/// declaration order, so a [`SealedSiteTarget::BranchEntry`] branch path indexes it level
/// by level. Called only for a flat-executable root, so every branch is a scalar-field
/// keyed branch (its `keys` are its ordered key columns) and its own members recurse
/// through the same rule.
pub(super) fn seal_branches(
    members: &DurableProductGraph,
    strings: &[Rc<str>],
) -> Vec<SealedBranch> {
    seal_branch_run(members.iter(), strings)
}

/// Seal one run of member rows into its branch list, descending through each branch's own
/// members. Nesting is bounded by the decoded member depth the table phase already
/// enforced.
fn seal_branch_run(members: DurableMemberViews<'_>, strings: &[Rc<str>]) -> Vec<SealedBranch> {
    members
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Branch(branch) => Some(SealedBranch {
                name: strings[usize::from(branch.name().index())].clone(),
                keys: branch.keys().iter().map(|key| key.scalar).collect(),
                record: branch.record().index(),
                branches: seal_branch_run(member.members(), strings),
            }),
            _ => None,
        })
        .collect()
}

/// Seal a flat-executable root's root-level unkeyed groups into [`SealedGroup`]s, in
/// declaration order, so a [`SealedSiteTarget::GroupEntry`] group index selects one.
/// Each group's name and materialized record come from the root's own record: the
/// verifier's record↔member tie (validated in the table phase) places one trailing
/// group slot per `Group` member, after the leading scalar/widened field slots, in
/// declaration order — so the group slot at `field_count + ordinal` is exactly this
/// group's slot. Called only for a flat-executable root, whose groups are all
/// storable-value-field groups.
pub(super) fn seal_groups(root: &DecodedRoot, types: &[SealedRecordType]) -> Vec<SealedGroup> {
    let record = &types[root.record as usize];
    let field_count = root
        .members
        .iter()
        .filter(|member| matches!(member.kind(), DurableMemberViewKind::Field(_)))
        .count();
    root.members
        .iter()
        .filter(|member| matches!(member.kind(), DurableMemberViewKind::Group(_)))
        .enumerate()
        .map(|(ordinal, _group)| {
            let slot = &record.fields[field_count + ordinal];
            let record = match slot.ty {
                ImageType::Record { idx, .. } => idx,
                _ => unreachable!("the record↔member tie places a Record slot per group member"),
            };
            SealedGroup {
                name: slot.name.clone(),
                record,
            }
        })
        .collect()
}

/// Seal every managed index of the root occurrence at DURABLE-table index
/// `root_index`, resolving each ledger-id projection to the record/key positions the
/// path kernel maintains.
///
/// A managed index is declared by one root occurrence, and its projection is resolved
/// against that occurrence and no other: a field component names its position in the
/// Product declaration's member order (tied to the materialized record), and a key
/// component names its column in *this occurrence's* key tuple, which two roots over one
/// Product may spell differently. Sealing the whole set here rather than resolving one
/// projection at a time is what makes that pairing structural — the sealed row's root and
/// the graph its components resolved against come from the same occurrence by
/// construction, so no caller can pair an index with a neighbouring root.
///
/// Every component already resolved to a real leaf during decode, so a miss here is an
/// internal inconsistency the verifier refuses rather than mis-addressing a maintained
/// index cell.
pub(super) fn seal_root_indexes(
    root_index: u16,
    root: &DecodedRoot,
) -> Result<Vec<SealedIndex>, VerifyRejection> {
    // The occurrence's two leaf position tables, built once for the whole set rather than
    // rescanned per component: a top-level field's index into the materialized record
    // (their orders are tied during root decode), and a key column's position in this
    // occurrence's own key tuple.
    let mut fields: HashMap<LedgerIdBytes, u16> = HashMap::new();
    for (position, field) in root
        .members
        .iter()
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Field(field) => Some(field),
            _ => None,
        })
        .enumerate()
    {
        fields.entry(field.id()).or_insert(position as u16);
    }
    let mut columns: HashMap<LedgerIdBytes, u16> = HashMap::with_capacity(root.keys.len());
    for (column, (_, id)) in root.keys.iter().enumerate() {
        columns.entry(*id).or_insert(column as u16);
    }
    root.indexes
        .iter()
        .map(|index| {
            let projection = index
                .components
                .iter()
                .map(|component| match component {
                    DurableIndexComponent::Field(id) => fields
                        .get(id)
                        .copied()
                        .map(SealedIndexComponent::Field)
                        .ok_or(reject(
                            VerifyPhase::Table,
                            "durable index field component resolves to no record position",
                        )),
                    DurableIndexComponent::Key(id) => columns
                        .get(id)
                        .copied()
                        .map(SealedIndexComponent::Key)
                        .ok_or(reject(
                            VerifyPhase::Table,
                            "durable index key component resolves to no key column",
                        )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SealedIndex {
                id: index.id,
                root: root_index,
                unique: index.unique,
                components: index.components.clone(),
                projection,
            })
        })
        .collect()
}

/// The ledger-id accounting for one durable table. `seen` holds every *declaration*
/// id — the application, each root's placement/product/key ids, each member's
/// field/group/branch id and branch key ids, each managed index id, and each durable
/// enum's sum and member ids on the enum's first durable occurrence — which must be
/// pairwise distinct because entropy-minted ids are distinct by construction. `enums`
/// records each durable enum identity by its sum id: the ordered member ids claimed at
/// its first occurrence. A later value shape carrying an already-recorded sum id — the
/// shape a second durable field of that enum emits — is a *reference* to that one
/// per-declaration identity, so it reclaims nothing and must carry the identical member
/// ids in order.
///
/// `products` records which durable Product identities a root has already declared. A
/// later root carrying an already-recorded Product id is a *reference* to that one
/// declaration, so it reclaims none of the declaration's ids; whether it states the same
/// graph and entry record is decided by the declaration table, which holds them.
/// `placements` records the root placement ids already occupied, so a repeated root
/// occurrence is refused as itself rather than as a generic duplicate id.
#[derive(Default)]
struct LedgerScope {
    seen: BTreeSet<LedgerIdBytes>,
    enums: BTreeMap<LedgerIdBytes, Vec<LedgerIdBytes>>,
    products: BTreeSet<LedgerIdBytes>,
    placements: BTreeSet<LedgerIdBytes>,
}

/// Whether a member tree is being decoded as a Product declaration's first occurrence,
/// which claims each declaration id it reads, or as a later reference to an already
/// accepted declaration, which claims nothing and is compared against it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberClaim {
    Declaration,
    Reference,
}

impl MemberClaim {
    /// Read one declaration id, claiming it as fresh when this occurrence declares it.
    fn read(
        self,
        reader: &mut Reader<'_>,
        scope: &mut LedgerScope,
        what: &'static str,
    ) -> Result<LedgerIdBytes, VerifyRejection> {
        match self {
            Self::Declaration => take_distinct_id(reader, scope, what),
            Self::Reference => read_id(reader, what),
        }
    }
}

/// Read one 16-byte ledger id from the reader without claiming it.
fn read_id(reader: &mut Reader<'_>, what: &'static str) -> Result<LedgerIdBytes, VerifyRejection> {
    let bytes: [u8; 16] = reader
        .take(16)
        .ok_or(reject(VerifyPhase::Table, what))?
        .try_into()
        .expect("take(16) yields 16 bytes");
    Ok(LedgerIdBytes::from_bytes(bytes))
}

/// Claim `id` as a fresh declaration id, rejecting a duplicate against those already
/// seen in this durable table. Two equal declaration ids are a forged or corrupted
/// identity block.
fn claim_distinct(scope: &mut LedgerScope, id: LedgerIdBytes) -> Result<(), VerifyRejection> {
    if !scope.seen.insert(id) {
        return Err(reject(VerifyPhase::Table, "duplicate durable ledger id"));
    }
    Ok(())
}

/// Read one 16-byte ledger id and claim it as a fresh, pairwise-distinct declaration id.
fn take_distinct_id(
    reader: &mut Reader<'_>,
    scope: &mut LedgerScope,
    what: &'static str,
) -> Result<LedgerIdBytes, VerifyRejection> {
    let id = read_id(reader, what)?;
    claim_distinct(scope, id)?;
    Ok(id)
}

/// Decode a placement key tuple: `count` columns, each a bare orderable durable-key
/// scalar and a distinct ledger id. Shared by roots and branches; the caller has
/// already validated `count` against `MAX_KEY_COLUMNS`.
fn decode_key_tuple(
    reader: &mut Reader<'_>,
    count: usize,
    scope: &mut LedgerScope,
    claim: MemberClaim,
) -> Result<Vec<(Scalar, LedgerIdBytes)>, VerifyRejection> {
    let mut keys = Vec::with_capacity(count);
    for _ in 0..count {
        let key_tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short key type"))?;
        let scalar = match decode_bare_scalar(key_tag) {
            Some(
                scalar @ (Scalar::Int
                | Scalar::Text
                | Scalar::Bool
                | Scalar::Bytes
                | Scalar::Date
                | Scalar::Instant),
            ) => scalar,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "key type must be an orderable durable-key scalar",
                ));
            }
        };
        let key_id = claim.read(reader, scope, "short key identity")?;
        keys.push((scalar, key_id));
    }
    Ok(keys)
}

/// Tie a root's group-inclusive materialized record to its durable member tree. The
/// record's slots run in the member tree's own top-level order with keyed branches
/// dropped: each `Field` member matches the next slot by value shape and required flag,
/// and each `Group` member matches the next slot — a bare group record — by tying its own
/// fields to the group's direct fields one level down. A slot count that disagrees, a
/// group slot that is not a group record, or any field mismatch is refused, so a hostile
/// image cannot claim one identity while executing over a different field or group shape.
///
/// Field slots precede group slots: a `Field` member after any `Group` member is refused,
/// so the record's leading scalar/widened field slots and its trailing group slots occupy
/// disjoint contiguous ranges. Sealing relies on this — a group's slot is `field_count +
/// ordinal` — so the fields-first invariant is verifier-enforced here rather than trusted
/// from the compiler.
fn tie_root_record(
    record_fields: &[DecodedField],
    members: &DurableProductGraph,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    let mut slots = record_fields.iter();
    let mut seen_group = false;
    for member in members.iter() {
        match member.kind() {
            DurableMemberViewKind::Field(field) => {
                let (value, required) = (field.value(), field.required());
                if seen_group {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree places a field after a group",
                    ));
                }
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                if required != slot.required
                    || !value_shape_matches(values, value, slot.ty, types, enums)
                {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree fields do not match the record fields",
                    ));
                }
            }
            DurableMemberViewKind::Group(_) => {
                seen_group = true;
                let Some(slot) = slots.next() else {
                    return Err(reject(
                        VerifyPhase::Table,
                        "root member tree has more top-level members than the record",
                    ));
                };
                tie_group_slot(slot, member.members(), types, enums, values)?;
            }
            // A keyed branch is a distinct durable node, not a materialized record slot.
            DurableMemberViewKind::Branch(_) => {}
        }
    }
    if slots.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "root member tree has fewer top-level members than the record",
        ));
    }
    Ok(())
}

/// Tie one trailing group slot of a root record to its `Group` member: the slot is a
/// bare group record whose fields match the member's direct `Field` members by value
/// shape and required flag, one level down — the same field tie the root and a branch
/// apply. A group holds only leaf fields on the executable line, so a non-record slot,
/// an optional record slot, an out-of-range record index, or a field/member mismatch is
/// refused.
fn tie_group_slot(
    slot: &DecodedField,
    group_members: DurableMemberViews<'_>,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    let ImageType::Record { idx, optional } = slot.ty else {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot is not a group record",
        ));
    };
    if optional {
        return Err(reject(
            VerifyPhase::Table,
            "a root group slot must be a bare group record",
        ));
    }
    if idx as usize >= types.len() {
        return Err(reject(
            VerifyPhase::Table,
            "root group slot record index out of range",
        ));
    }
    let group_fields = &types[idx as usize].fields;
    let mut direct_fields = group_members.filter_map(|member| match member.kind() {
        DurableMemberViewKind::Field(field) => Some((field.value(), field.required())),
        _ => None,
    });
    for field in group_fields {
        match direct_fields.next() {
            Some((value, member_required))
                if member_required == field.required
                    && value_shape_matches(values, value, field.ty, types, enums) => {}
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "group member tree fields do not match its record fields",
                ));
            }
        }
    }
    if direct_fields.next().is_some() {
        return Err(reject(
            VerifyPhase::Table,
            "group member tree has more direct fields than its record",
        ));
    }
    Ok(())
}

/// Validate every keyed `branch` in a decoded member tree: its surface name and
/// materialized record type indices are in range, and its record's fields match its
/// own direct scalar field members in order, value shape, and required flag — the
/// same tie the root's record has to its member tree, one level down. Recurses
/// through groups and branches. The name and record are surface (not identity), so
/// this is the only place they are checked; a hostile image that names a branch
/// record disagreeing with the branch's field shapes is refused here.
fn validate_branch_records(
    members: DurableMemberViews<'_>,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
    string_count: usize,
    values: &CanonicalValueShapeDag,
) -> Result<(), VerifyRejection> {
    // One explicit stack of member runs, in the same pre-order the recursive walk had, so
    // the first violation is refused at the same member as before while the walk's own
    // stack use stays independent of how deep the image nested its rows.
    let mut stack = vec![members];
    while let Some(run) = stack.last_mut() {
        let Some(member) = run.next() else {
            stack.pop();
            continue;
        };
        match member.kind() {
            DurableMemberViewKind::Field(_) => {}
            DurableMemberViewKind::Group(_) => stack.push(member.members()),
            DurableMemberViewKind::Branch(branch) => {
                if usize::from(branch.name().index()) >= string_count {
                    return Err(reject(VerifyPhase::Table, "branch name index out of range"));
                }
                if usize::from(branch.record().index()) >= types.len() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch record type index out of range",
                    ));
                }
                let record_fields = &types[usize::from(branch.record().index())].fields;
                let mut direct_fields = member.members().filter_map(|inner| match inner.kind() {
                    DurableMemberViewKind::Field(field) => Some((field.value(), field.required())),
                    _ => None,
                });
                for field in record_fields {
                    match direct_fields.next() {
                        Some((value, member_required))
                            if member_required == field.required
                                && value_shape_matches(values, value, field.ty, types, enums) => {}
                        _ => {
                            return Err(reject(
                                VerifyPhase::Table,
                                "branch member tree fields do not match its record fields",
                            ));
                        }
                    }
                }
                if direct_fields.next().is_some() {
                    return Err(reject(
                        VerifyPhase::Table,
                        "branch member tree has more direct fields than its record",
                    ));
                }
                stack.push(member.members());
            }
        }
    }
    Ok(())
}

/// Decode a durable member tree: `u16(count) ‖ member*`. A field is tag `0x00`; a
/// group is tag `0x01`; a branch is tag `0x02`. `budget` bounds the total member
/// records across the whole tree and `depth` bounds nesting, so a hostile image
/// cannot drive unbounded recursion or allocation before the bounds are rechecked
/// (§ law 9). Every declaration ledger id is distinct across the table; a durable
/// enum's sum and member ids are the exception — one per-declaration identity a
/// later field of that enum references rather than reclaims.
fn decode_members(
    reader: &mut Reader<'_>,
    budget: &mut usize,
    scope: &mut LedgerScope,
    graph: &mut DurableContractGraph,
    claim: MemberClaim,
) -> Result<Vec<DeclarationMemberDef>, VerifyRejection> {
    let top = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable member count"))? as usize;
    let mut commands: Vec<DeclarationMemberDef> = Vec::with_capacity(top.min(*budget));
    // One explicit stack of `(parent command, members still to read at that level)`. The
    // wire order is unchanged — a nested run is read where the tree wrote it — but nothing
    // recurses and nothing owns a child vector: each command names its parent by an
    // earlier command's index, which is the only shape the graph's construction accepts.
    let mut stack: Vec<(Option<u16>, usize)> = vec![(None, top)];
    while let Some((parent, remaining)) = stack.last_mut() {
        if *remaining == 0 {
            stack.pop();
            continue;
        }
        *remaining -= 1;
        let parent = *parent;
        if *budget == 0 {
            return Err(reject(VerifyPhase::Table, "too many durable members"));
        }
        *budget -= 1;
        let index = u16::try_from(commands.len())
            .map_err(|_| reject(VerifyPhase::Table, "too many durable members"))?;
        let tag = reader
            .u8()
            .ok_or(reject(VerifyPhase::Table, "short durable member tag"))?;
        let shape = match tag {
            0x00 => {
                let id = claim.read(reader, scope, "short durable field identity")?;
                let required = match reader.u8().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable field required flag",
                ))? {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable field required flag must be 0 or 1",
                        ));
                    }
                };
                let value = decode_value_shape(reader, 1, scope, graph.value_shapes_mut())?;
                DeclarationMemberShape::Field {
                    id,
                    required,
                    value,
                }
            }
            0x01 => {
                let id = claim.read(reader, scope, "short durable group identity")?;
                descend(&mut stack, reader, index)?;
                DeclarationMemberShape::Group { id }
            }
            0x02 => {
                let placement = claim.read(reader, scope, "short durable branch identity")?;
                // The branch's surface name and materialized record type index follow
                // the placement. Their ranges (against the string and type tables) and
                // the record/member-field alignment are checked in
                // `validate_branch_records`, where the type and enum tables are in scope.
                let name = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch name"))?;
                let record = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short durable branch record"))?;
                let key_count = reader
                    .u16()
                    .ok_or(reject(VerifyPhase::Table, "short branch key count"))?
                    as usize;
                if key_count > marrow_image::bounds::MAX_KEY_COLUMNS {
                    return Err(reject(VerifyPhase::Table, "too many branch key columns"));
                }
                let keys = decode_key_tuple(reader, key_count, scope, claim)?;
                descend(&mut stack, reader, index)?;
                DeclarationMemberShape::Branch {
                    placement,
                    name: StrId::from_index(name),
                    record: TypeId::from_index(record),
                    keys: keys
                        .into_iter()
                        .map(|(scalar, id)| KeyColumn { scalar, id })
                        .collect(),
                }
            }
            _ => return Err(reject(VerifyPhase::Table, "unknown durable member tag")),
        };
        commands.push(DeclarationMemberDef { parent, shape });
    }
    Ok(commands)
}

/// Enter the nested member run a `group` or `branch` command opened: check the nesting
/// bound the level below would occupy, then read that level's count.
///
/// The bound is checked before the count is read, exactly where the recursive decode
/// checked it on entry, so an over-deep image is refused at the same byte.
fn descend(
    stack: &mut Vec<(Option<u16>, usize)>,
    reader: &mut Reader<'_>,
    parent: u16,
) -> Result<(), VerifyRejection> {
    if stack.len() + 1 > marrow_image::bounds::MAX_DURABLE_DEPTH {
        return Err(reject(VerifyPhase::Table, "durable member tree too deep"));
    }
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable member count"))? as usize;
    stack.push((Some(parent), count));
    Ok(())
}

/// Decode a root's managed indexes: `u16(count) ‖ index*`. Each index is its distinct
/// `Index` ledger id, a `unique` flag byte, a `u16(component_count)`, and per component
/// a one-byte leaf kind (`0x02` field, `0x04` key) and the leaf's 16-byte ledger id.
/// Every component id is re-resolved against this root's own top-level field ids
/// (kind `0x02`) or identity key ids (kind `0x04`), so a projection over a leaf that
/// does not exist on the root is refused. The index id is distinct across the whole
/// durable table (via `seen`); component ids are references to already-seen leaf ids
/// and so are not added to `seen`.
fn decode_indexes(
    reader: &mut Reader<'_>,
    keys: &[(Scalar, LedgerIdBytes)],
    members: &DurableProductGraph,
    scope: &mut LedgerScope,
    values: &CanonicalValueShapeDag,
) -> Result<Vec<DurableIndexShape>, VerifyRejection> {
    let field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Field(field) => Some(field.id()),
            _ => None,
        })
        .collect();
    // A managed-index field component must project one of the compiler's closed set of
    // orderable durable-key scalar shapes. Field executability is independent: Duration
    // and widened values can be stored but are not index-eligible.
    let index_eligible_field_ids: Vec<LedgerIdBytes> = members
        .iter()
        .filter_map(|member| match member.kind() {
            DurableMemberViewKind::Field(field) => match values.view(field.value()) {
                ValueShapeView::Scalar(
                    Scalar::Int
                    | Scalar::Text
                    | Scalar::Bool
                    | Scalar::Bytes
                    | Scalar::Date
                    | Scalar::Instant,
                ) => Some(field.id()),
                ValueShapeView::Scalar(Scalar::Duration)
                | ValueShapeView::Struct(_)
                | ValueShapeView::Enum { .. } => None,
            },
            DurableMemberViewKind::Group(_) | DurableMemberViewKind::Branch(_) => None,
        })
        .collect();
    let count = reader
        .u16()
        .ok_or(reject(VerifyPhase::Table, "short durable index count"))? as usize;
    if count > marrow_image::bounds::MAX_INDEXES {
        return Err(reject(VerifyPhase::Table, "too many durable indexes"));
    }
    let mut indexes = Vec::with_capacity(count);
    for _ in 0..count {
        let id = take_distinct_id(reader, scope, "short durable index identity")?;
        let unique = match reader.u8().ok_or(reject(
            VerifyPhase::Table,
            "short durable index unique flag",
        ))? {
            0 => false,
            1 => true,
            _ => {
                return Err(reject(
                    VerifyPhase::Table,
                    "durable index unique flag must be 0 or 1",
                ));
            }
        };
        let component_count = reader.u16().ok_or(reject(
            VerifyPhase::Table,
            "short durable index component count",
        ))? as usize;
        if component_count > marrow_image::bounds::MAX_INDEX_COMPONENTS {
            return Err(reject(
                VerifyPhase::Table,
                "too many durable index components",
            ));
        }
        let mut components = Vec::with_capacity(component_count);
        for _ in 0..component_count {
            let kind = reader.u8().ok_or(reject(
                VerifyPhase::Table,
                "short durable index component kind",
            ))?;
            let leaf: [u8; 16] = reader
                .take(16)
                .ok_or(reject(
                    VerifyPhase::Table,
                    "short durable index component identity",
                ))?
                .try_into()
                .expect("take(16) yields 16 bytes");
            let leaf = LedgerIdBytes::from_bytes(leaf);
            let component = match kind {
                0x02 => {
                    if !index_eligible_field_ids.contains(&leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            if field_ids.contains(&leaf) {
                                "durable index field component names a field that is not \
                                 index-eligible"
                            } else {
                                "durable index field component names no top-level field of its root"
                            },
                        ));
                    }
                    DurableIndexComponent::Field(leaf)
                }
                0x04 => {
                    if !keys.iter().any(|(_, key_id)| *key_id == leaf) {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable index key component names no identity key of its root",
                        ));
                    }
                    DurableIndexComponent::Key(leaf)
                }
                _ => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "unknown durable index component kind",
                    ));
                }
            };
            components.push(component);
        }
        // Re-enforce projection well-formedness the compiler owns: a reference-valid but
        // malformed projection (an empty projection, a repeated component, or a
        // non-unique index whose identity suffix is missing, misordered, or preceded by a
        // key) must never reach the sealed index model the runtime trusts to order rows.
        if let Err(detail) = validate_index_projection(unique, &components, keys) {
            return Err(reject(VerifyPhase::Table, detail));
        }
        indexes.push(DurableIndexShape {
            id,
            unique,
            components,
        });
    }
    Ok(indexes)
}

/// Re-check one decoded index's projection against the closed well-formedness rules the
/// compiler owns, so a hostile image cannot smuggle a malformed projection past the
/// verifier. Every component id is already re-resolved to a real scalar field or identity
/// key of the root (the orderable-key predicate); this owns the ordering and cardinality
/// rules: the projection is non-empty, no component repeats, and a non-unique index ends
/// with exactly the identity keys in declaration order — the row-distinguishing suffix. A
/// unique index carries no suffix obligation. Returns a static detail describing the first
/// violation.
///
/// The no-leading-key rule (a non-unique index carries no identity key before its suffix)
/// needs no separate branch: distinctness forbids any component from repeating, and the
/// suffix must already hold every identity key, so a leading identity key would duplicate
/// a suffix key and is rejected by the distinctness check.
fn validate_index_projection(
    unique: bool,
    components: &[DurableIndexComponent],
    keys: &[(Scalar, LedgerIdBytes)],
) -> Result<(), &'static str> {
    if components.is_empty() {
        return Err("durable index has an empty projection");
    }
    for (position, component) in components.iter().enumerate() {
        if components[..position]
            .iter()
            .any(|earlier| earlier.id() == component.id())
        {
            return Err("durable index repeats a projection component");
        }
    }
    if !unique {
        // The trailing `keys.len()` components must be exactly the identity keys in
        // declaration order.
        if components.len() < keys.len() {
            return Err("non-unique durable index does not end with the identity suffix");
        }
        let suffix_start = components.len() - keys.len();
        for (offset, (_, key_id)) in keys.iter().enumerate() {
            match components[suffix_start + offset] {
                DurableIndexComponent::Key(id) if id == *key_id => {}
                _ => {
                    return Err(
                        "non-unique durable index does not end with the identity keys in \
                         declaration order",
                    );
                }
            }
        }
    }
    Ok(())
}

/// Decode a durable field's stored value shape into `values`, returning a reference to
/// the node it minted: `u8(value_tag) ‖ body`. A scalar is tag `0x00` (a bare scalar); a
/// dense struct is tag `0x01` (`u16(count) ‖ value*`); a closed enum is tag `0x02`
/// (`sum id ‖ u16(count) ‖ [member id ‖ u16(payload) ‖ value*]*`). Leaves are minted
/// before the shape that references them, so the wire tree is consumed without ever
/// being materialized as one — a shape the image spells twice is decoded into the one
/// node the arena already holds. An enum's sum and member ids are the identity of the
/// enum *declaration*. The first occurrence of a given sum id claims it and its member
/// ids as fresh pairwise-distinct ids; a later occurrence carrying an already-claimed sum
/// id — the shape a second durable field of that enum emits — is a reference that
/// reclaims nothing and must carry the identical member ids in order. `depth` bounds
/// nesting so a hostile image cannot drive unbounded recursion before the value shape is
/// rechecked (§ law 9).
fn decode_value_shape(
    reader: &mut Reader<'_>,
    depth: usize,
    scope: &mut LedgerScope,
    values: &mut CanonicalValueShapeDag,
) -> Result<ValueShapeNodeId, VerifyRejection> {
    if depth > marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH {
        return Err(reject(
            VerifyPhase::Table,
            "durable field value shape too deep",
        ));
    }
    let tag = reader
        .u8()
        .ok_or(reject(VerifyPhase::Table, "short durable value tag"))?;
    match tag {
        0x00 => {
            let scalar_tag = reader
                .u8()
                .ok_or(reject(VerifyPhase::Table, "short durable value scalar"))?;
            let scalar = decode_bare_scalar(scalar_tag).ok_or(reject(
                VerifyPhase::Table,
                "durable value scalar must be a bare scalar",
            ))?;
            Ok(values.scalar(scalar))
        }
        0x01 => {
            let count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable struct leaf count",
            ))? as usize;
            if count > marrow_image::bounds::MAX_STRUCT_LEAVES {
                return Err(reject(VerifyPhase::Table, "too many durable struct leaves"));
            }
            let mut leaves = Vec::with_capacity(count);
            for _ in 0..count {
                leaves.push(decode_value_shape(reader, depth + 1, scope, values)?);
            }
            Ok(values.struct_shape(leaves))
        }
        0x02 => {
            let sum = read_id(reader, "short durable enum sum identity")?;
            let member_count = reader.u16().ok_or(reject(
                VerifyPhase::Table,
                "short durable enum member count",
            ))? as usize;
            if member_count > marrow_image::bounds::MAX_VARIANTS {
                return Err(reject(VerifyPhase::Table, "too many durable enum members"));
            }
            // An enum reached before (by its sum id) is a reference to that one
            // per-declaration identity: it reclaims neither the sum nor any member id,
            // and must present the identical member ids in the identical order. The
            // recorded identity is the ordered member ids only; each occurrence's payload
            // shapes are tied to the field's own enum-table entry by
            // `value_shape_matches`, so a payload divergence is caught there rather than
            // against the first occurrence.
            let recorded = scope.enums.get(&sum).cloned();
            match &recorded {
                Some(recorded_ids) if recorded_ids.len() != member_count => {
                    return Err(reject(
                        VerifyPhase::Table,
                        "durable enum identity reused with a different member set",
                    ));
                }
                Some(_) => {}
                None => claim_distinct(scope, sum)?,
            }
            let mut members = Vec::with_capacity(member_count);
            for index in 0..member_count {
                let id = read_id(reader, "short durable enum member identity")?;
                match &recorded {
                    Some(recorded_ids) if recorded_ids[index] != id => {
                        return Err(reject(
                            VerifyPhase::Table,
                            "durable enum identity reused with a different member set",
                        ));
                    }
                    Some(_) => {}
                    None => claim_distinct(scope, id)?,
                }
                let payload_count = reader.u16().ok_or(reject(
                    VerifyPhase::Table,
                    "short durable enum member payload count",
                ))? as usize;
                if payload_count > marrow_image::bounds::MAX_PAYLOAD_FIELDS {
                    return Err(reject(
                        VerifyPhase::Table,
                        "too many durable enum member payload leaves",
                    ));
                }
                let mut payload = Vec::with_capacity(payload_count);
                for _ in 0..payload_count {
                    payload.push(decode_value_shape(reader, depth + 1, scope, values)?);
                }
                members.push((id, payload));
            }
            if recorded.is_none() {
                scope
                    .enums
                    .insert(sum, members.iter().map(|(id, _)| *id).collect());
            }
            Ok(values.enum_shape(sum, members))
        }
        _ => Err(reject(VerifyPhase::Table, "unknown durable value tag")),
    }
}

/// Whether a decoded durable field value shape structurally matches the materialized
/// record field type it claims, recursing through the record and enum tables. The
/// ledger ids a value shape carries (a struct records none; an enum a sum and per-
/// member id) are durable identity, verified by pairwise distinctness and the
/// contract-id recomputation — this match ties the *structure* to the executable
/// record so a hostile image cannot claim one durable identity while its record
/// carries a different value shape. A nominal field erases to its base scalar, so it
/// matches a bare scalar exactly like a plain scalar field.
fn value_shape_matches(
    values: &CanonicalValueShapeDag,
    shape: ValueShapeNodeId,
    ty: ImageType,
    types: &[DecodedRecordType],
    enums: &[DecodedEnum],
) -> bool {
    match (values.view(shape), ty) {
        (
            ValueShapeView::Scalar(shape_scalar),
            ImageType::Scalar {
                scalar,
                optional: false,
            },
        ) => shape_scalar == scalar,
        (
            ValueShapeView::Struct(leaves),
            ImageType::Record {
                idx,
                optional: false,
            },
        ) => {
            let Some(record) = types.get(idx as usize) else {
                return false;
            };
            // A durable struct value is dense: every leaf is a required bare field,
            // matched positionally.
            record.fields.len() == leaves.len()
                && record.fields.iter().zip(leaves).all(|(field, leaf)| {
                    field.required && value_shape_matches(values, *leaf, field.ty, types, enums)
                })
        }
        (
            ValueShapeView::Enum { members, .. },
            ImageType::Enum {
                idx,
                optional: false,
            },
        ) => {
            let Some(enum_def) = enums.get(idx as usize) else {
                return false;
            };
            enum_def.variants.len() == members.len()
                && enum_def
                    .variants
                    .iter()
                    .zip(members)
                    .all(|(variant, member)| {
                        variant.payload.len() == member.payload().len()
                            && variant.payload.iter().zip(member.payload()).all(
                                |(leaf_ty, leaf)| {
                                    value_shape_matches(values, *leaf, *leaf_ty, types, enums)
                                },
                            )
                    })
        }
        _ => false,
    }
}

#[cfg(test)]
mod capacity_tests {
    use super::{H_VERIFIED_DURABLE_GRAPH_BYTES, MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES};

    /// The exact accounted figure, published in the implementation map beside the
    /// compiler-side one.
    ///
    /// Asserted rather than only bounded because it is a term a later capacity join
    /// consumes: a change to it is an observable-contract change, and the map and this pin
    /// move together.
    #[test]
    fn the_verifier_side_maximum_live_graph_holds_its_accounted_figure() {
        assert_eq!(
            MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES, 145_228_008,
            "the accounted verifier-side live graph moved; re-derive the exported term and \
             update the implementation map with this pin"
        );
    }

    /// The ceiling is not trivially satisfied: a bound met by an empty accounting would
    /// pass whatever the representation did.
    #[test]
    fn the_verifier_ceiling_is_a_meaningful_fraction() {
        assert!(MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES <= H_VERIFIED_DURABLE_GRAPH_BYTES);
        assert!(MAX_LIVE_VERIFIED_DURABLE_GRAPH_BYTES > H_VERIFIED_DURABLE_GRAPH_BYTES / 4);
    }
}
