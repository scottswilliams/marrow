# Authorization

A designed surface with a normative contract that is not implemented yet.
Authorization has no counterpart in `docs/language/` today; when it ships, this
page moves there. The surface below states the intended contract in the same
neutral terms as the rest of the reference.

## Model and scope

Authority in Marrow is a typed `principal` shape plus allow-only `policy for`
blocks attached to the surfaces of the one tree. The checker evaluates every
policy against the navigation graph and derives an access matrix: one verdict
per operation and principal shape, drawn from three values — **static-allow**
(the check is erased and costs nothing at runtime), **dead** (the operation does
not exist for that principal, absent from its route set and its generated
client), and **filtered** (a data-dependent `where` residual, compiled to exact
tests over already-read values plus at most one keyed point probe per conjunct,
evaluated at the operation's own snapshot). Denial renders as absence; there is
no denial code. Identity is the only thing that crosses a trust boundary, and
the verdict is always recomputed at the enforcing node against its own accepted
image.

The organizing rule: **identity travels, verdicts do not.** The only authority
object that crosses a trust boundary is an authenticated claim of a principal
and its typed attribute values. Permissions are never serialized into tokens,
headers, or wire payloads; the verdict is recomputed at the enforcing node's own
seam, against its own image's policy, at the operation's own snapshot.

What is in scope: authorization of surface operations — which principal may read,
create, update, delete, or invoke which operation, and under which data
condition. What is out of scope:

- **Authentication.** How a credential, session, or token is established and
  mapped to a principal is the application's concern at its own boundary. This
  page begins once a principal and its attribute values are bound.
- **Developer and agent capability over the store itself** — who may run, serve,
  evolve, back up, restore, or recover a store. That is a deployment and
  operations concern, homed at the process or connection admission boundary that
  binds a credential to a capability set, not an in-language `policy` construct.
  It is not specified here.

The precise strength and limits of the model are stated in
[Guarantees and limits](#guarantees-and-limits). In short: each surface
operation is proven to obey its own arms over its own footprint against its own
surface's policy. Cross-surface, cross-store, and per-path consistency are
enforced by a separate information-flow gate ([Enforcement](#the-cross-surface-information-flow-gate)),
not by that per-operation proof alone.

## When authorization applies

Authorization is opt-in, and its cost is proportional to need. A program that
declares no `principal` has no policy and no matrix: every operation runs under a
single implicit trusted principal with full access. A local tool, a script, a
prototype, or a single-user application writes no `principal` and no `policy for`
and pays nothing — no declarations, no runtime residual, no ceremony.

Declaring the first `principal` introduces the access matrix for the program.
From that point mixed state fails closed: a declared principal is dead on any
surface whose policy does not name it, every surface reachable by a declared
principal must carry an allow arm or it is dead, and the cross-surface
information-flow gate is enforced. The model becomes available exactly when it is
asked for, and not before.

### The open boundary

Exposure, not the presence of a policy, decides what the checker demands. Running
a store locally (`marrow run`) or serving it on the loopback interface never
requires a policy: the operator holds the store through the filesystem and is the
trusted principal. Serving a store to the network (`marrow serve --remote`) does.
The checker refuses to expose a surface that carries neither a `policy for` block
nor an explicit `public` marker.

```mw
surface Directory from ^practitioners public   ; intentionally world-open, acknowledged
    fields family, given, specialty
```

An unmarked, unpoliced surface reached remotely is a checker error naming the
surface, with the two resolutions: write a `policy for` block, or mark the
surface `public`. A `public` surface is readable by the anonymous principal; the
marker is explicit source — greppable and reviewable — so an open remote surface
is always a written decision, never a default an author falls into.

A project may forbid the open marker so that every surface must carry a policy:

```mw
forbid public          ; `public` becomes a checker error; every surface needs a policy
```

Under `forbid public` an intentionally open endpoint is still expressible — as a
`policy for` block with an anonymous `allow read` arm — so every exposure is a
reviewed policy rather than a marker. The default permits `public`; a deployment
that requires a policy on every surface opts into forbidding it.

## Language surface

### `principal`

A principal is a typed shape, never a stored row. It declares the attribute
values that bind at authentication.

```mw
principal Visitor anonymous          ; explicit anonymous shape; binds no attributes

principal Member
    memberId: Id(^members)

principal Staff
    branch: string
```

- Attributes use the field indentation idiom and the identity, scalar, and enum
  type vocabulary. `Id(^members)` is the identity type; no new type syntax is
  introduced.
- `anonymous` is an explicit trailing modifier. An attribute-less principal is
  not implicitly anonymous; an anonymous principal cannot declare attributes,
  and a policy arm that references `principal.x` on an anonymous principal is
  reported as `check.policy_anonymous_attr`.
- **Principal identity attributes are required and non-absent by
  construction.** A principal shape cannot declare a maybe-present identity
  attribute, and the checker rejects a residual equality between two
  maybe-present operands (`check.policy_absent_equality`). This keeps an
  identity comparison from silently matching two absent values.
- The application maps its own rows to attribute values at its authentication
  boundary; the store holds no principal rows and no policy state outside the
  image. Users remain application data.

### `policy for` — allow-only arms

A `policy for <Surface>` block lists allow arms per principal, co-located with
the surface it governs.

```mw
surface Library from ^books
    fields title, author, shelf, ownerId
    collection ^books.byShelf as byShelf
    action checkout::run as checkout

policy for Library
    Visitor
        allow read                   ; verb class: every read operation in the surface
    Member
        allow read
        allow create
        allow update where ownerId == principal.memberId
        allow checkout               ; a named operation, by its surface alias
```

- **Location.** One surface has at most one `policy for` block, written after the
  surface in the same module. Adjacency is a convention the formatter enforces,
  not grammar.
- **Operation selector.** `read`, `create`, `update`, and `delete` are the four
  verb classes; a declared action or computed read is named by its alias
  (`allow checkout`). Both selectors resolve in the one collision-checked surface
  operation namespace. Per-operation naming is the recommended spelling for exact
  matrix diffs; verb classes are the coarse shorthand.
- **Allow arms only.** There is no `deny` keyword. Deny-by-default is structural:
  anything not matched by an allow arm is dead. There is no rule ordering,
  shadowing, or override precedence to reason about.

### The `where` residual

A `where` clause is the one data-dependent position. It is the derivation
fragment used elsewhere in the language plus exactly one atom, `principal.x`,
typed by the declared shape. It is effect-free: no user calls, no clock, no
quantifiers. Its data-dependent forms are exactly three.

- **Already-read exact test.** `ownerId == principal.memberId` tests values the
  operation already reads, so it adds no operations. This is the ownership rule,
  and it costs nothing.

  ```mw
  allow update where ownerId == principal.memberId   ; zero extra probes
  ```

- **Keyed existence probe.** Following a typed link is a point read. A keyed
  existence test is one point probe on one named unique index. A stored identity
  field is itself the relationship — `ownerId: Id(^members)` already records that
  a book belongs to a member — and the residual navigates it.

  ```mw
  allow read where ^shares.byDocMember(doc.id, principal.memberId) exists   ; one probe
  ```

- **Monotone union.** `a exists || b exists` — every arm probes regardless of
  earlier outcomes, so the probe multiset is a static function of the operation
  and principal shape. The fragment is union-only: no negation and no
  intersection-with-negation. Monotone structure is what keeps a defaulted-field
  hazard from turning a denial into a grant for the identity and existence forms
  (see [defaulted fields](#defaulted-fields-and-over-grant)).

`via <indexName>` disambiguates when two indexes could back a keyed probe:

```mw
allow read where ^folders(doc.folder).team has principal.memberId via byTeamMember
```

When exactly one index qualifies, or the conjunct tests already-read values,
`via` is omitted and the compiler's choice is forced, not heuristic. Two
candidate indexes report `check.policy_ambiguous` (fix: add `via`). No backing
index reports `check.policy_unindexed`, with the index declaration and its
write-cost delta as the attached fix. There is no scan fallback.

### Create-time `where`

At `create` there is no stored record. A `create` arm's `where` binds only the
incoming record's already-read values and any parent the create path names. A
conjunct that would require a stored read of the not-yet-created record fails
closed with `check.policy_create_stored_read`.

```mw
policy for Loans
    Member
        allow create where ^books(book).ownerId == principal.memberId   ; parent probe: allowed
```

### Granting access by writing a row

Because a residual navigates a link, granting one user access is often an
ordinary data write rather than a policy edit. Sharing a document, adding a team
member, or checking out a book writes a link row, and authority follows in the
same snapshot under the same serial writer — no policy transition, no deploy, no
cache invalidation.

```mw
store ^shares(id: int): Share
    index byDocMember(doc, member) unique

policy for Docs
    Member
        allow read where ^shares.byDocMember(doc.id, principal.memberId) exists
```

The split mirrors Marrow's schema-versus-data split applied to authority: the
policy text declares which shapes of relationship confer access — stable,
reviewable, an image transition when it changes; the tree holds who stands in
those relationships — fluid and application-managed, with no policy transition.

**Who may write the granting row is itself an authorization question, and the
model does not make the unguarded spelling unrepresentable.** An unconditional
`allow create` on a record that confers authority elsewhere is self-service
escalation: any principal writes a grant row naming any resource and admits
itself in the same snapshot. The safe form gates the create with an authority
the grantee cannot self-confer:

```mw
policy for Shares
    Member
        allow create where ^docs(doc).ownerId == principal.memberId   ; only owners may share
```

This closes the case where an owner link pre-exists. It does **not** close the
case where the grantee is the subject and no prior authority exists — care-team
membership, group membership, moderator assignment. There, "you must already be
a member to add a member" is a bootstrap impossibility, and the natural fallback
is an unconditional `allow create` that lets any principal self-appoint. The
checker reports `check.policy_unguarded_grant` when a surface whose records are
keys of a residual probe elsewhere carries an unconditional `allow create`,
because such a record confers authority in some policy. The diagnostic is the
signal; the model does not otherwise prevent the spelling, and the grant chain's
trust root is unenforced (see [operational concerns](#admin-bootstrap-and-the-trust-root)).

### Reifying authority-conferring facts

A fact like "published posts are world-readable" has no link to navigate, but
publication is a relationship with the world and the tree can say so. Model it as
a record rather than a boolean on a content field.

```mw
store ^publishedPosts(id: int): PublishedPost
    index byPost(post) unique

policy for Blog
    Visitor
        allow read where ^publishedPosts.byPost(post.id) exists   ; existence, no principal attribute
```

A principal-free existence probe binds every principal shape, anonymous ones
included. The publish action writes the row; unpublish deletes it; authority
follows the write in the same snapshot. A reified fact is more auditable than a
predicate over a content field, and it avoids the defaulted-field hazard below.
It is the only spelling for a time or scalar window in the first form of the
language, with the honesty caveat in [time windows](#time-windows-run-outside-the-check).

## Enforcement

### The access matrix

The checker computes, for each operation and principal shape, one of three
verdicts. One classifier produces both the rendered matrix cell and the runtime
admission decision; there is no second policy classifier in checker, runtime,
surface, or tests. The matrix is derived, never stored.

- **Static-allow** erases the per-operation predicate. An arm with no data
  condition covers the operation's whole footprint for every attribute valuation
  of the principal, so no authorization predicate runs. The principal-scoped
  admission still resolves the route against the matrix at runtime — the
  reachable-or-dead decision is a server lookup, not client trust.
- **Dead** is genuine absence. A dead operation is not checked and refused; it is
  absent from the principal's route set and its generated client, and a
  hand-crafted request for it resolves to `surface.absent`, byte-identical to an
  unknown route name.
- **Filtered** carries a `where` residual. It is the only runtime policy
  evaluation, and it runs inside the operation's own admission at its snapshot.
  Probe reads join the operation's footprint, so a concurrent revocation
  conflicts correctly instead of racing. Evaluation is total: every conjunct's
  probes execute regardless of earlier outcomes.

Consistency needs no extra apparatus: the serial writer orders policy-relevant
data writes against content writes, so there is no policy-staleness window and no
consistency token. This is what makes granting access by writing a row sound
within one store.

Denial renders as absence throughout. A filter-denied point read is
byte-identical to a truly absent record; a statically-dead route is
byte-identical to an unknown name. There is no authenticated-but-denied code, and
its absence is a gated artifact over the generated code registry. Conflict and
validation errors never echo a stored value the principal cannot read.

### The cross-surface information-flow gate

A per-operation proof does not compose into a per-path guarantee on its own. A
surface exposes computed reads — read-only functions whose footprint can navigate
typed links into other records and other stores. Without a further rule, a field
denied on its canonical surface can be read through a second surface's computed
read under a static-allow arm, invisible in the per-operation matrix.

```mw
surface PatientChart from ^patients
    fields name                              ; SSN deliberately not exposed
policy for PatientChart
    Nurse
        allow read where ^careTeam.byPatientProvider(patient.id, principal.nurseId) exists

surface ClinicalObs from ^observations
    fields code, value
    read observationSummary as summary       ; navigates ^patients(patient).ssn
policy for ClinicalObs
    Nurse
        allow read                           ; static-allow would erase the check
```

A nurse not on a patient's care team is filtered-denied on `PatientChart`, yet a
bare `allow read` on `ClinicalObs` would expose the same patient's SSN through
`summary`. The per-surface matrix shows only a green cell and cannot see the
divergence.

The gate closes it: **a projected or navigated read path's verdict must be no
weaker than the strictest verdict any surface assigns that path for that
principal.** A computed read that navigates a path a stricter surface gates more
tightly for the same principal is a compile error (`check.policy_projection_authority`).
Protecting a field by giving its store a strict surface is only sound with this
gate; the checker enforces the consistency the matrix cannot show.

### Footprint containment, stated honestly

Static-allow erasure rests on two containments: the operation's compiled
footprint over-approximates every actual access, and a static-allow arm's allowed
region covers that whole footprint. So `allowed ⊇ footprint ⊇ actual`, and the
allow decision is a sound compile-time set-containment.

The footprint over-approximation is enforced by a mutation-verified containment
oracle over the conformance corpus — strong and empirical on executed paths, not
a formal proof over all programs. Narrowing an arm, adding a `where`, or widening
the footprint flips a verdict off static-allow, so an erased check that should not
have been erased does not pass the suite. The erasure of the allow decision is
sound given that containment contract; it is not advertised as a proof over every
program.

### Defaulted fields and over-grant

A field that carries a read-default — a value substituted for records that never
populated it, as an additive `evolve default` produces — reads as a live value.
An exact-equality residual against such a field is satisfied by the default for
every record that never set it:

```mw
policy for Blog
    Visitor
        allow read where status == Visibility::published   ; over-grants defaulted records
```

Records created before `status` existed, or never assigned it, read
`Visibility::published` by default and become world-readable though no author
published them. Monotonicity does not help here: the default value itself
satisfies the positive test. The over-deny-only property of the monotone fragment
holds for existence probes and identity equality against an absent default, and
**not** for a defaulted scalar or enum field. An exact-equality of a principal
attribute or constant against a defaulted scalar or enum field is reported as
`check.policy_defaulted_probe`; use a required field or a reified row instead. A
residual over a defaulted field is exact-test-only and never rewritten to an
index walk, which would not see the unpopulated records.

### Checker diagnostics

| Code | Reported for | Fix |
|---|---|---|
| `check.policy_anonymous_attr` | `principal.x` on an anonymous principal | drop the attribute or declare a shape |
| `check.policy_absent_equality` | residual equality between two maybe-present operands | compare non-absent operands |
| `check.policy_ambiguous` | two indexes could back a keyed probe | add `via` |
| `check.policy_unindexed` | a keyed probe with no backing index | declare the index (write-cost delta attached) |
| `check.policy_create_stored_read` | a create arm reads the not-yet-created record | probe a parent or the incoming values only |
| `check.policy_unguarded_grant` | unconditional `allow create` on an authority-conferring record | gate the create arm |
| `check.policy_defaulted_probe` | exact-equality against a defaulted scalar or enum field | use a required field or a reified row |
| `check.policy_projection_authority` | a computed read navigates a path a stricter surface gates | tighten the arm or the projection |

Each diagnostic carries a typed payload; the registry gate catches an
unregistered code.

## Authentication boundary and topologies

The three deployment topologies vary exactly one thing: which boundary constructs
the execution context and what proof of identity it demands. They never vary the
policy source, the verdict computation, the enforcement point, or the rendering
rules. The same project served under all three produces the same matrix and the
same admission decisions per operation and principal shape.

| | Direct | Server-client | Distributed |
|---|---|---|---|
| Identity established at | connection admission, `run`, or test setup | serve request admission | receiving node's request admission |
| Trust boundary | OS process or loopback | the wire (authentication only) | inter-node wire plus configured trust roots |
| Proof of identity | ambient OS or a named local credential | a bearer credential, or a credential plus an asserted principal | a signed principal assertion under a trusted key |
| Authority travels as | nothing (connection-pinned) | a bearer credential or per-request assertion | a signed identity assertion |
| Enforcement | in-process at the seam, at the snapshot | same | same, per node, against that node's image |

### Direct

Several terminals or processes against one live serve, or a `run` invocation.
Identity is one principal per connection, pinned at admission. A named local
credential resolves to a declared principal shape; different tools connect with
different credentials and receive different matrix columns.

`marrow run` and raw-store access construct the legacy default principal,
ceilinged by invocation mode: **policy is bypassed.** For a system where the data
model is the API, filesystem access to the store file is the trust boundary — the
operating system already authenticated the owner, and `run` executes as the
owner. This is stated plainly rather than described as authorization that
happens to be skipped: filesystem access to the store is equivalent to root. An
operator who wants `run` itself policy-gated runs it against a serve connection
under a declared principal.

### Server-client

A remote serve over the wire; browsers and services call it, possibly through a
trusted middle tier holding one connection for many end users. Identity is
established at request admission. Direct callers hold named bearer credentials
that serve configuration resolves to a principal and its attribute values.
Credential-to-principal binding is configuration, not source — secrets never
enter the image, the lock, or the ledger.

A middle tier may present its own credential plus a typed principal assertion. Its
configuration names the set of principal shapes it may assert:

```toml
# serve credential configuration — never source, never in the image
[credential.web-tier]
source  = { env = "WEB_TIER_TOKEN" }
asserts = ["Member", "Visitor"]        # may act as these shapes, never Staff

[credential.reporting]
source    = { file = "/etc/marrow/reporting.token" }
principal = "Auditor"
```

Per request, the tier presents its token plus an assertion of one principal shape
and its attributes; the constructor verifies the credential, checks membership in
the `asserts` set, typechecks the attributes, and constructs the context. One
identity per request.

**The `asserts` set constrains which shapes, not which attribute values.**
`asserts = ["Member"]` means the tier may act as any member, not a specific one.
The tier is the trust root that binds `memberId` from authenticated end-user
identity, and Marrow does not close the per-attribute confused-deputy question: a
tier holding `asserts = ["Member"]` can act as every member, and its honesty in
binding attributes from authenticated input is a step Marrow does not own. The
`asserts` set is the one authorization-adjacent element that lives in
configuration rather than the matrix; it names only shapes, never operations or
predicates.

A generated principal-scoped client emits only the operations that principal can
reach — dead routes are absent by failing to typecheck — and its identity folds
the policy section, so a policy-only narrowing flips its freshness.

### Distributed

Multiple nodes, or Marrow behind other services, where identity crosses a machine
boundary. No multi-node Marrow exists today; this topology is a forward
constraint. Authority crosses as a signed principal assertion — identity, not
permissions:

```
PrincipalAssertion {
  principal,                     # rename-stable within one image lineage
  attrs,                         # per the declared shape: scalar, enum, identity
  issuer, expiry, nonce          # ordinary signed-assertion hygiene
}   signed under a key the receiving node's configuration trusts
```

The receiving node resolves the assertion at its one credential-to-principal
boundary and recomputes the verdict against its own accepted image at its own
snapshot. The assertion does not pin an image version — the current policy
governs.

A principal identity is rename-stable only within one image lineage. Two
independent stores mint different identities for the same principal shape, so an
assertion is store-scoped by construction and cannot be replayed into a foreign
store's policy — a replay-safety property, not a defect. Replay within the
`expiry`/`nonce` window against the same store is a documented residual.

**Caveat-style attenuation — macaroon or biscuit tokens, and authority-carrying
tokens generally — is rejected.** A caveat is a predicate evaluated at the
verifier, outside the compiled matrix: a second policy language the checker
cannot see, rejected on the same grounds as configuration-file policy.
Attenuation is instead a narrower declared principal shape with its own matrix
column, and cross-boundary attenuation is the issuer's `asserts` set naming only
the narrow shape. A signed attenuation chain resolved against the receiver's own
policy is specifically rejected, because a token minted as narrow silently gains
authority if the receiver later widens what its shapes admit; "the current policy
governs" has no such failure. If bearer delegation is ever needed, the only
admissible form is a matrix-meet that drops rows and columns over the existing
operation-selector vocabulary — monotone narrowing, never a predicate.

Consistency is per node. A revoked membership row on one node does not instantly
kill another node's verdicts; cross-node revocation is assertion expiry and
reissue. A relationship row must be replicated into the enforcing node's tree to
grant there; the graph does not travel implicitly.

### Relationship to OAuth2

OAuth2 and OIDC are authentication and token mechanics at the door. Their claims
map onto principal attribute binding — `sub` becomes `memberId`, for example — at
the application's authentication boundary. Token scopes must never become a
second policy vocabulary: a scope may, at most, meet the matrix by dropping
routes, never add semantics the policy does not already grant. Session mechanics
such as cookies and redirects are the application's business, upstream of the
principal this page begins with.

## Guarantees and limits

What holds:

- **Deny-by-default is structural.** An operation with no allow arm is a dead
  route, absent from the principal's surface and generated client. The
  forgot-a-rule and precedence-ordering families are unrepresentable.
- **Dead routes are server-enforced absence.** A hand-crafted client hitting a
  route dead for its principal receives `surface.absent`, indistinguishable from
  an unknown name; aliases resolve to the same operation and the same verdict.
- **Static-allow erasure of the allow decision** is a sound compile-time
  set-containment, given the footprint containment contract above.
- **Read-path leak families close.** Existence, value echo, route enumeration,
  verdict-inference through error codes, and probe count under total evaluation
  are closed on read paths. Denial renders as absence.
- **Single-store grant and revoke are consistent at the snapshot.** Probes carry
  footprint atoms and the serial writer orders writes, so a concurrent revocation
  conflicts rather than races.
- **The cross-surface information-flow gate** makes a projected read path's
  verdict consistent with the strictest surface over it.

The named limits and residuals, stated rather than hidden:

- **Per-path confidentiality is not proven by the per-operation proof alone**; it
  is the cross-surface gate that supplies it. Each surface operation is proven to
  obey its own arms over its own footprint against its own surface's policy.
- **Middle-tier attribute binding** is outside Marrow's proof: the tier that
  binds attribute values from authenticated input is a trust root the model does
  not own.
- **Timing.** Marrow does not claim constant-time authorization; a residual
  probe's timing is an honest channel.
- **Unique-conflict write probing.** A create against a unique index reveals
  existence at zero read access; naming the conflict is the residual, echoing the
  conflicting value is forbidden.
- **Time-window rules run outside the check** — see below.
- **Cross-tenant isolation is a discipline, not a primitive** — see below.
- **The trust root of every grant chain is unenforced** — see
  [operational concerns](#admin-bootstrap-and-the-trust-root).

### Time windows run outside the check

The `where` fragment has no clock, so a time or scalar window — an edit window, an
active-status window, a booking window — is expressed by reifying a fact and
letting background code retire it. The check over that fact is race-free: the
retirement is a serial-writer commit, and the probe joins the footprint, so at
any snapshot the row is present or absent with no read-write race.

The time **bound**, however, lives in the background sweep, which the checker
never sees. A crashed, late, or incorrect sweep silently extends the window, and
sweep granularity is a fail-open over-permit window up to one sweep interval.
Time-window rules are therefore not compiler-enforced: the reified fact makes the
check sound, and the bound is only as sound as the sweep. The sweep-lag window sits
beside the timing and unique-conflict channels as an honest residual, and a
background-sweep facility is a required companion for any time-window rule.

### Update writes are operation-granular

An update arm authorizes an operation, not a field. An owner-update arm lets the
owner write every field the surface exposes, including a privilege-conferring one:

```mw
surface Members from ^members
    fields memberId, role
policy for Members
    Member
        allow read
        allow update where memberId == principal.memberId
```

`Members.update(memberId: alice, role: admin)` admits — the record is owned — and
Alice's next authentication maps the new `role` to a wider principal shape. The
residual fragment cannot express "the new value must not change this field," so
"may edit email but not role" is expressible only by splitting the writable
fields onto a separate surface. Field-granular write arms, with an
incoming-versus-stored comparison, are a later extension.

### Cross-tenant isolation is a discipline

There is no tenant or subtree primitive in the first form of the language.
Multi-tenant isolation is hand-written as a per-arm conjunct on every arm of
every surface:

```mw
policy for Docs
    Member
        allow read where tenantId == principal.tenantId
```

Omitting the conjunct is a silent cross-tenant breach: `Member { allow read }`
reads every tenant's data, erased at compile time, and the matrix shows
`static-allow` — indistinguishable from intentionally public. **The static
verdicts never apply to a multi-tenant root**; every operation over one is a
filtered residual. A lint against an unconditional arm on a marked tenant root
arrives with the subtree-authority extension; until then the per-arm discipline
is the whole mechanism, and it is stated so it is not mistaken for an enforced
guarantee.

## Operational concerns

### Admin bootstrap and the trust root

Once any principal is declared, serve refuses to start with a credential that
does not resolve to a declared principal. The zero-principals state is preserved
exactly: with no principals and no policy, a token-only or loopback serve behaves
as it does today, a single authenticated principal bounded by serve mode. Policy
narrows within that ceiling; it never widens it.

The first-principal transition is therefore explicit: bind the first
administrative credential in configuration, declare the first policy in source,
link, and restart. Getting the order wrong is a lockout or a fail-open window, so
the sequence is a deliberate operational step.

Every grant chain bottoms out at a seed write by a privileged principal or the
filesystem-owner path, and **that trust root is unenforced.** Self-registration —
a provider creating their own record before they have an identity to gate on, a
passenger authenticated only by a booking reference — is where no prior authority
exists, and the model delegates it to the application's authentication boundary.
The grant-by-writing-a-row and self-registration patterns do not imply the seed
is safe; the seeding step is named as unenforced for each topology.

### Break-glass

Emergency access is a distinct, wider principal — `principal EmergencyClinician`
with its own matrix column — activated by an emergency credential and audited,
not an override of an existing principal's arms. Modeling it as its own column
keeps the elevated access in the matrix where it is reviewable.

### Audit

Every mutation is audited once a principal-stamped, append-only, per-transition
ledger lands; that ledger is a planned addition, not an inherited fact, and until
it lands commit records carry no actor. Reads are not ledger events, so
"who read this record under break-glass" is not auditable in the first form.
Attribution is shape-granular — the principal shape, not the individual — unless
the identity-bearing attribute values are persisted as audit facts in the commit
record. Whether to persist them is a deliberate choice: they are audit facts, not
policy state, and reconcile with "the store holds no principal state" only when
recorded as the former.

### Revocation

Revoking a data grant is instant: delete the link row, and the next snapshot no
longer admits. A leaked bearer credential is revoked only by a configuration edit
and reload — there is no revocation list, which is acceptable for a single node
and named as such. An issued signed assertion cannot be revoked before its expiry;
cross-node revocation is expiry and reissue.

### Key rotation

For the distributed topology, the issuer key is rotated with an overlap window —
ordinary signed-assertion hygiene that the assertion's `issuer`, `expiry`, and
`nonce` fields support. It is a forward obligation of that topology, named so it
is not overlooked, and low-ranked while no multi-node Marrow exists.

### Data erasure

Erasing a subject's row interacts with authorization. Reified grant rows keyed on
that identity become inert rather than re-grantable, because allocator identities
are not reused; the subject can no longer authenticate, which is the natural
revocation. The audit ledger must retain entries citing the erased identity, so
erasure and audit retention are in tension and the retention side wins for
audited mutations.

### Rate limiting

Rate limiting is not authorization. A filtered read runs a bounded probe per
request, but there is no per-principal cost ceiling until request budgets exist,
so an authenticated principal can drive probe load. This is a cost concern, named
so it is not mistaken for an authorization promise.

## Staging

The surface ships in stages so that each stage lands with its enforcement
artifacts.

- **The static matrix ships first**: `principal` declarations, `policy for` blocks
  with static verdicts only, the access matrix as a checked fact and a
  pull-request diff, the refuse-to-start gate for unbound credentials once any
  principal exists, and the documented operator-principal posture. No runtime
  policy evaluation, no cryptography, and no new value type exist at this stage.
- **Data-dependent residuals follow**: the `where` residual with total evaluation
  and deterministic compilation, the three navigation forms, create-time binding,
  `via` disambiguation, the defaulted-field rule, the cross-surface information-flow
  gate, per-principal cost in the matrix, and principal-scoped client emission.
- **Later extensions**, each additive on the matrix: a declared transitive-closure
  store for nested groups and folders (a runtime recursive graph walk is refused);
  field-granular read projection with the computed-field information-flow rule; a
  first-class subtree or tenant-scoped authority primitive with its omission lint;
  and field-granular write arms with an incoming-versus-stored comparison. A
  bearer delegation form, if demanded, is admitted only as a matrix-meet that
  drops rows and columns, never as a predicate.
