# Elanu design decisions

Elanu is being designed from semantics outward. Important language behavior must therefore be documented as a design decision, not left to be inferred from the current compiler implementation.

This file is a decision index, not a historical changelog and not a compiler-architecture inventory. It records the durable semantic choices a future maintainer should be able to answer with “why does Elanu work this way?”

For the current compiler-accepted source surface, see
[`LANGUAGE_REFERENCE.md`](LANGUAGE_REFERENCE.md).

## What belongs here

A significant design decision should record, where relevant:

1. **Decision** — the behavior or rule being adopted.
2. **Rationale** — why the rule fits Elanu's goals and semantic model.
3. **Alternatives considered** — especially conventional alternatives.
4. **Tradeoffs** — what the decision improves and what it makes harder.
5. **Boundaries / failure modes** — cases where the rule does not provide the desired guarantee.
6. **Revisit triggers** — future features or evidence that should cause the decision to be reviewed.

Do not promote bootstrap implementation details into language law merely because the current compiler happens to encode them that way.

---

# Core semantic decisions

## Language identity: Elanu

Decision:

> Elanu is the language formerly developed under the name Meld. The rename is terminological only and does not alter established language semantics.

The pre-1.0 public language, compiler/package/binary identity, current documentation, and source-file convention use **Elanu**. Historical experiment, milestone, pressure, prior-art, and architecture documents may retain **Meld** where that is historically accurate.

The canonical source-file extension is `.elnu`. The current compiler does not enforce a filename extension; this is a project/tooling convention rather than a parsing or runtime semantic rule.

Generated `__meld_*` names and similar private encodings remain bootstrap implementation representation. This naming decision does not rename them or promote them into language semantics.

## `state`, `derived`, and `action` are distinct semantic roles

Decision:

- `state` is mutable application state with stable semantic identity;
- `derived` is read-only computed state whose dependencies are tracked dynamically;
- `action` is the synchronous atomic mutation boundary for Elanu-managed state.

These roles are intentionally separate. A `derived` value is not hidden mutable state, and an `action` is not a general async/effect/workflow construct.

Revisit when external effects, async work, recovery, concurrency, or persistence create a concrete need to split or extend these boundaries.

## State reads yield values, not implicit writable aliases

Ordinary reads of state produce values. They do not silently grant mutation authority.

The writable-authority experiment showed that reusable mutation of caller-selected state can be expressed without turning ordinary reads into references.

The resulting distinction is:

```text
ordinary state read
    -> value

explicit writable grant
    -> authority to mutate one exact state identity
```

This decision has since survived modeled state, live designation, sequence membership, reduction, filter, and owner-relative creation composition.

Revisit the authority model only when concrete pressure requires semantics beyond current exact writable grants—for example escaping/captured authority, concurrency, external resources, or mutation patterns that cannot be expressed as authority to an existing state identity.

Do not generalize ordinary state reads into a reference system by default.

## Nested actions join the outer transaction

Nested action invocation composes inside one shared atomic transaction. A nested action does not create an implicit commit boundary.

This preserves:

- transaction-local staged reads;
- atomic invariant-preserving composition;
- rollback across nested calls;
- coherent derived reads inside the same transition.

This law has survived multiple modeled-state owners, live designation, structural sequence replacement, indirect mutation, reduction, filter, and fresh modeled-state creation.

Revisit before recovery/savepoints, concurrency/isolation, package/plugin mutation boundaries, or irreversible external effects.

## `fail` aborts the current shared transaction

`fail <String expression>` means the current action transition cannot complete and must roll back.

It is intentionally narrower than a general exception model. Current Elanu does not infer `throw`, `catch`, exception hierarchies, partial commits, or savepoints from `fail`.

Revisit when a realistic partial-success workflow demonstrates that all-or-nothing action composition is insufficient.

## Program equality and state invalidation equivalence are different relations

Source-level equality answers ordinary program-logic questions. State equivalence answers whether a write is observably different enough to invalidate dependents.

These relations are intentionally not identical. In particular, repeated NaN may be state-equivalent even though ordinary `NaN == NaN` is false.

The same distinction extends naturally to structural state: a state-equivalent whole-sequence replacement suppresses invalidation even though source-level sequence equality is not part of the current minimum surface.

Revisit when newly supported operations make an existing equivalence rule observably unsound.

## `derived` is lazy dynamically tracked cached computation

A committed `derived` value records the dependencies actually read during evaluation. When an active dependency meaningfully changes, the cached result is no longer trusted. The runtime need not recompute immediately; the next read establishes a current valid result.

The programmer-visible rule is coherence, not evaluation count.

Therefore:

```text
stale cached implementation data
    !=
program-visible derived value
```

A stale cache entry may physically remain in runtime memory, but ordinary Elanu code does not read it as the current derived value.

Transaction-local derived evaluation uses the current staged transaction world and temporary dependency/cache state. Rollback discards transaction-local work.

Revisit before ordinary functions/closures, background or asynchronous computation, package boundaries, or language-visible observation/effects create new dependency or purity pressure.

## Short-circuit Boolean control flow determines dynamic dependencies

For `and` and `or`, Elanu evaluates only the operands required to determine the current Boolean result. An operand skipped by short-circuit control flow is not read and therefore is not an active dependency merely because it appears in source text.

Decision:

```text
A and B
    A false -> B is not evaluated
    A true  -> B is evaluated

A or B
    A true  -> B is not evaluated
    A false -> B is evaluated
```

`not` evaluates its one Bool operand once and adds no dependency beyond facts read by that operand.

This is a semantic consequence of Elanu's existing dynamic-dependency law, not a performance hint. A dependency graph describes facts actually read during the current evaluation. When state changes alter short-circuit reachability, reevaluation may replace the dependency set accordingly. Transaction-local staged state participates in the same rule, and rollback does not publish temporary dependency edges.

The language therefore must not implement Boolean composition by eagerly evaluating both operands and merely discarding one result afterward. That would produce correct Boolean values but incorrect Elanu dependency semantics.

Revisit only if future effectful expression forms, concurrency, or other evaluation-visible behavior requires a broader expression-effect rule. Do not infer a general effects system from short-circuiting alone.

## Same-value derived stabilization is an optimization permission, not a new language law

If an invalidated pure cached `derived` recomputes to a value equivalent to its previous observable value, the runtime may safely suppress unnecessary downstream recomputation provided existing observable semantics are preserved.

This does **not** create source-visible states such as `dirty`, `stable`, `changed`, generations, versions, or evaluation counters.

Evaluation count remains implementation detail.

A runtime may conservatively fall back to ordinary recomputation whenever proving stabilization is inconvenient.

## Historical derived values are ordinary application history when they matter

Elanu does not currently define a special `previous(derived)` or stale-cache history mechanism.

If an old derived result matters to application behavior, store it explicitly as `state` at the application-defined transition point.

Conceptually:

```elanu
state previousTotal = 0.0

derived total = ...
derived difference = total - previousTotal

action update {
    previousTotal = total
    // mutate state that changes total
}
```

This keeps historical retention, timing, rollback, and resource cost under application control rather than turning incidental cache contents into language semantics.

Revisit only if repeated realistic programs demonstrate substantial boilerplate or correctness problems that explicit state cannot address cleanly.


## String containment keeps exact and caseless policy explicit

The workstation/search-view composition pressure established two deliberately distinct text predicates:

```elanu
text contains search
text contains search ignoring case
```

Decision:

- both operands are `String` and both forms produce `Bool`;
- plain `contains` remains exact case-sensitive substring containment;
- `contains ... ignoring case` applies locale-independent Unicode default **full case folding** to both operands and then performs exact substring containment on the folded text;
- the caseless form is not defined as lowercasing either operand;
- neither form performs Unicode normalization;
- the caseless form performs no locale-specific/Turkic tailoring;
- the empty String is contained in every String;
- ordinary dynamic-dependency rules apply to the operands actually evaluated;
- surrounding short-circuit Boolean composition may prevent the containment expression from being evaluated, in which case its operands are not read and do not become dependencies.

Keeping exact and caseless containment separate prevents a user-facing search convenience from silently changing the meaning of existing exact source. Full Unicode case folding was selected over ASCII-only comparison or naive lowercase conversion because ordinary application text is Unicode and full folding handles case equivalences that lowercasing does not, including multi-character folds and sigma variants.

Unicode normalization, locale-sensitive collation/tailoring, tokenization, regex, fuzzy matching, and stemming remain separate capabilities. Case folding alone intentionally does not make canonically equivalent but differently normalized strings equal. Those mechanisms require their own composition evidence rather than being smuggled into `ignoring case`.

The current implementation uses a focused Unicode case-folding dependency and folds both strings before substring comparison. The dependency, its internal tables, and the compiler's `ContainsIgnoringCase` representation are implementation choices, not language law. The durable rule is locale-independent Unicode default full case folding without normalization.

Revisit only when realistic application composition demonstrates a need for normalization-aware matching, locale-sensitive behavior, or a broader text abstraction. Do not infer a general String API from these two predicates.

---

# Modeled state, designation, and authority

## `state model` defines a reusable state-capable shape

The composition work promoted into v0.8.0 established `state model` as the minimum reusable shape for repeated modeled state.

Distinct live modeled-state variables of the same model have distinct constituent member identities while sharing one model definition.

This does not create per-model transaction or invalidation domains. Existing exact state identity remains sufficient.

## Every concrete modeled-state identity has its own declared model-local state

Dynamic owner composition established that a state model is not merely a template for statically declared roots. Every actual live identity of model `T`, including an identity created at runtime, is an instance of the model's declared state structure.

Decision:

- each concrete modeled-state identity has its own declared stored member state;
- this includes model-local `[live U]` structural state;
- model-local derived values and structural views are evaluated relative to that exact identity;
- a compatible `live T` designation may identify the exact owner whose members are being reached;
- changing the designation changes which existing modeled-state identity later member access resolves;
- structural membership remains non-owning and does not become lifetime provenance;
- designation remains non-owning identity and does not become writable authority;
- writable authority remains explicit;
- root provenance remains the separate fact governing supported child lifetime termination.

Therefore a source expression such as:

```elanu
selectedProject.tasks[0]
```

means the current `tasks` structure belonging to the exact `Project` identity designated by `selectedProject`. The application does not need to reconstruct that fact using a project key, mirrored task table, copied owner state, or positional owner lookup.

This decision does not introduce general references, object ownership, borrowing, general collection APIs, whole-model writable projection, or designation-valued ordinary expressions. Current private runtime-member nodes, generated carriers, and lowered designation names are compiler representation rather than language law.

Revisit only if future composition requires a modeled-state member relationship that cannot be expressed as state local to one exact modeled identity, or if whole-model value/authority projection must coherently include structural members.

## Ordinary modeled-state values remain detached values

Reading modeled state as an ordinary value does not produce a hidden live alias to the original state world.

This preserves Elanu's value-oriented data flow and prevents ordinary value passing from silently acquiring live identity or writable authority.

## `live T` is a non-owning designation of an existing live target

A `live T` value answers:

> Which existing live `T` state identity is meant?

It does not itself answer:

> May this code mutate that target?

The resulting conceptual split is:

```text
ordinary T value
    -> detached value flow

live T designation
    -> identifies an existing live target
    -> no writable authority

writable authority
    -> permission to mutate one exact live state identity
```

Do not generalize `live T` into a conventional pointer/reference type without new evidence.

## Persistent `live T` state stores target identity, not selecting position

Workstation composition established a distinct application fact that positional `Int`
state cannot preserve by itself: "keep this exact child selected even if other rows
enter or leave before it."

Decision:

```elanu
state selectedTask: live Task = live initialTask

selectedTask = workstation.visibleTasks[selectedIndex]
```

For supported owner-relative `[live T]` membership and identity-preserving filtered
views:

- indexing as a whole designation evaluates the current structure and zero-based `Int`
  selector in transaction-visible state;
- assigning that result to compatible scalar `live T` state stores the exact selected
  child identity at assignment time;
- later selector changes, membership edits, predicate changes, and view-position
  changes do not retarget that stored designation;
- reselection is ordinary transactional state mutation and rolls back on action
  failure;
- removing a structural membership occurrence or making the child leave a filter does
  not itself invalidate the stored designation, because membership/view participation
  is not child lifetime;
- duplicate occurrences that designate the same child still designate the same child
  identity; storing one does not invent stable occurrence identity;
- storing a designation remains non-owning and grants no writable authority;
- member reads may follow the stored designation, while mutation remains explicit with
  `through`.

Literal and runtime `Int` selectors have the same designation semantics. A compiler
representation that gives `view[0]` and `view[indexExpr]` different identity behavior
would therefore be an implementation bug, not two language rules.

Rationale:

A structural position answers "where is something now?" A persistent designation
answers "which existing live child is this?" The workstation pressure requires the
second fact when rows can move around a still-selected child. Preserving that identity
inside Elanu lets the compiler carry a fact it already owns instead of requiring UI or
framework code to reconstruct identity from position or an invented business key.

Boundaries:

- plain `live T` remains exactly-one-target state; absence is represented separately by
  `maybe live T`;
- plain `live T` itself does not grant destruction authority; the current owner-relative
  `destroy` surface rejects plain-live operands, and any remaining plain `live T` designation
  blocks termination so the exactly-one-live-target invariant cannot become dangling;
- it does not select stable structural occurrence identity when the same child appears
  more than once;
- it does not create general references, ownership/borrowing, designation action
  parameters, or a general collection lookup API;
- creation-scoped fresh designations remain a separately bounded source mechanism.

Current private target tags and structured runtime designation AST nodes are compiler
representation, not language law.

Designation-directed structural removal now provides one narrow operation that can
consume the stored child identity and resolve one unique current structural occurrence
without exposing public designation equality or lookup. The narrow owner-relative `destroy`
surface now consumes only persistent `maybe live T` state and preserves plain-live
non-dangling guarantees. Revisit persistent designation state when realistic composition
requires stronger designation flow, explicit duplicate-occurrence choice, or a destruction
form outside that bounded surface.

## `maybe live T` represents target absence without general nullability

The selection-absence pressure established that a persistent designation sometimes
needs to represent one exact live child **or no child at all**. Encoding that fact as a
parallel `Bool` plus a real sentinel `live T` was transactionally workable but left the
payload fully usable while the Boolean claimed there was no selection.

Decision:

```elanu
state selectedTask: maybe live Task = none
```

- `live T` continues to mean exactly one existing live target;
- `maybe live T` means either one exact compatible live target or no target;
- contextual `none` is valid for `maybe live T` initialization/assignment and is not a
  general null literal;
- assigning a live designation preserves exact child identity;
- clearing to `none` changes only the designation state; it does not destroy the child
  or edit membership;
- structural membership removal does not implicitly clear the designation;
- absent member read, `through` mutation, `state through` authority grant, and
  designation-directed structural removal fail the current shared action transaction;
- absent-use failure rolls back earlier staged work under the ordinary action law;
- the designation remains non-owning whether present or absent.

Rationale:

Absence is a real application fact for runtime-sized selection. Representing it directly
lets the compiler know that there is no target to follow, avoiding a dummy modeled-state
identity and preventing accidental reads/mutations of an arbitrary sentinel. Keeping it
separate from plain `live T` preserves the useful stronger invariant that ordinary
`live T` always designates one target.

This does **not** select a generalized `Option` type, nullable ordinary values, null
propagation, pattern matching, designation equality, or reference ownership semantics.
The current private empty carrier used by the bootstrap runtime is representation only.

Presence is observable through the separate narrow `is present` predicate. The current
owner-relative child-destruction law clears every persistent `maybe live T` designation
that targets the terminated child as one compiler-owned transactional consequence, so this
optional designation form does not become dangling under that operation. Revisit `maybe live T`
when another lifetime-ending form cannot preserve that guarantee or when repeated non-live
optional-state pressure earns a broader sum/option mechanism.

## Root provenance governs explicit dynamic-child lifetime termination

Decision:

> Explicit child lifetime termination is authorized by the child's recorded rooting owner,
> not by a designation or by any structural membership that happens to contain the child.

The selected source form is currently:

```elanu
destroy selected in workspace
```

where `selected` is persistent `maybe live T` state. The owner operand must resolve to the
exact modeled identity recorded as that committed child's rooting owner. It may name a static
modeled-state root directly or carry a dynamic owner identity through persistent `live T` /
present `maybe live T` designation state. The current operation is transactional and limited
to leaf children.

Successful termination has compiler-owned consequences because Elanu already owns the relevant
identity relations: all structural occurrences of the child are removed, all persistent
`maybe live T` designations naming it become absent, its modeled state ceases to be live, and
its root relation ends. A later action failure rolls the whole transition back.

Plain `live T` designations of the child being destroyed block termination rather than becoming
dangling. A designation used in the owner slot is different: it contributes exact owner identity
for comparison with recorded root provenance, but the designation itself remains non-owning and
does not create lifetime authority. A different root or dynamic child that merely contains or
designates the target has no such authority. A child that still roots another live dynamic child
is rejected rather than cascading implicitly.

Rationale:

Creation already establishes one exact root-provenance fact, while designation and membership
are deliberately non-owning. Using root provenance therefore expresses application intent
without promoting `live T` into an owning reference or turning structural containers into
lifetime owners. Compiler-owned cleanup also avoids requiring application/framework code to
find every foreign membership and optional designation that names the same child.

Alternatives rejected by the supporting pressure include membership-owner authority,
designation-as-owner authority, application-maintained tombstones/manual global cleanup, and
GC/reference-counting semantics. None represents the explicit application fact that this rooted
child is permanently leaving the live application world as directly as the owner-relative
transition.

Boundaries:

- the current source surface is limited to committed dynamic leaf children selected through
  persistent `maybe live T` state;
- fresh transaction-local child cancellation is not selected;
- statically declared modeled-state roots are not destroyable through this form;
- cascade, reparenting, ownership transfer, scoped/indexed/derived destruction operands, and
  general resource lifetime remain separate questions.

Revisit only when realistic composition demonstrates one of those stronger facts is needed.

## Persistent designation membership is a read-only child-identity relation

The current-view reconciliation pressure established a second fact that is distinct
from designation presence:

```elanu
selectedTask is in workstation.visibleTasks
```

Decision:

```text
designation is in structure
    -> read the current transaction-visible designation
    -> read the current supported [live T] membership/direct filtered view
    -> require compatible child model
    -> true if at least one current occurrence designates that exact child
    -> false otherwise
```

For `maybe live T`, an absent designation is simply not in the structure and therefore
produces `false`. A present designation can also produce `false` when its child remains
live but no longer occurs in the specified membership or filtered view.

Durable conclusions:

- the relation is based on exact child identity, not current numeric position;
- earlier rows may enter or leave a filtered view without changing the result while the
  same child remains represented;
- staged membership and predicate-driving writes affect the relation in the same action;
- rollback restores both structural/predicate state and the observed relation;
- duplicate occurrences are existential: one or more matching occurrences produce
  `true` rather than ambiguity;
- existential membership does not select, expose, or stabilize an occurrence;
- the relation is read-only and grants no writable authority;
- model compatibility is checked statically;
- designation presence, structural membership, filtered-view participation, child
  lifetime, and writable authority remain separate semantic axes.

Rationale:

Elanu already owns both facts required to answer the application question: one persisted
live child identity and an identity-preserving structural value. Requiring application
code to copy a business key or reconstruct a numeric position would discard semantic
information the compiler already has. A narrow Boolean identity-membership relation
therefore advances the compiler-ownership thesis without exposing general designation
equality or a conventional collection lookup API.

This decision does **not** add `find`/`indexOf`, a position-returning lookup, iterators,
stable occurrence identity, designation comparison as an ordinary value operation, or
a general query/collection framework. Those mechanisms require separate composition
pressure.

Current compiler-only `IsIn` transport and private designation/sequence encodings are
implementation representation, not language law.

## Relative navigation resolves one persistent child identity against current ordered structure

Decision:

```elanu
target = next anchor in source
target = previous anchor in source
```

Relative navigation is a narrow contextual designation-producing relation. `anchor` is a
persistent compatible `live T` or `maybe live T` designation. `source` is supported stored
`[live T]` membership or a direct identity-preserving filtered view. `target` is a
compatible persistent designation state. The operation does not make live designations
general ordinary values.

The durable semantic law is:

```text
current persistent child designation
+ current ordered structure/view containing live T designations
+ previous | next
    -> exact neighboring child designation
```

Resolution is against the transaction-visible ordered structure at the moment of the
operation. The designated child must have exactly one current occurrence in that source.
No occurrence fails; multiple current occurrences are ambiguous and fail. An absent
`maybe live T` anchor fails. A missing neighbor at the requested boundary currently fails.
All failures join the surrounding action transaction.

A successful operation persists only the neighboring child identity. Any numeric
occurrence position used by an implementation is transient and is not language identity.
The same persistent child may therefore have different neighbors in different structures
or views, and staged membership, filter-predicate, or anchor changes may change the result
inside one action.

Rationale:

Elanu already owns the two application facts frameworks normally have to reconnect: exact
persistent child identity and current ordered structural identity. Requiring applications
to copy a business key, reconstruct an index, or persist a cursor would throw away those
facts and recreate framework bookkeeping. Unique-current-occurrence navigation lets the
compiler answer the intended relation directly without inventing stable occurrence
identity.

Alternatives rejected by the supporting pressure include position reconstruction,
collection-method syntax that prematurely implies a general collection API, and
designation-method syntax that incorrectly suggests the child itself owns traversal
state.

This decision does not add designation equality, `find`/`indexOf`, first/last duplicate
selection policy, stable occurrence handles, wraparound/clamping, cursors/iterators, or a
general collection API. Revisit only if realistic composition requires one of those
stronger facts rather than merely making it familiar or convenient.

## Lexical designation binding carries identity without creating state, authority, or lifetime

Decision:

> Elanu may give one exact live child identity an immutable lexical designation name for a
> bounded block without turning that identity into persistent application state, writable
> authority, or child ownership.

The durable semantic law is:

```text
supported designation-producing expression
    -> evaluate once in the current transaction-visible world
    -> require one present exact live child identity
    -> bind that identity immutably for one lexical block

scope exit
    -> lexical name ends
    -> child identity/lifetime and structural membership are unchanged
```

The captured name is identity flow, not storage. Later persistent reselection or structural
reorder/filter changes do not retarget it. Operations that consume the captured designation
still apply their own current-time structural or authority rules.

The binding itself grants no writable authority. `through` and `state through` remain the
explicit crossings into mutation/authority, and exact authority resolved through a
captured designation follows the same grant-time identity law as other designations.

The block is not a transaction boundary. Nested actions continue to join the surrounding
action transaction, and failure rolls back ordinary state/structural effects. Lexical scope
is therefore distinct from transaction scope and child lifetime.

Rationale:

Two independent composition pressures now require the same semantic capability: scoped
creation needs a short-lived name for a fresh child identity, and workstation
selection/removal needs a short-lived name for an already-existing child identity. In both
cases the compiler already owns the exact identity; forcing applications to allocate
persistent scratch state solely to carry it between statements recreates framework-style
bookkeeping.

This decision remains deliberately narrower than general local bindings. It does not add
`let`/`var`, mutable locals, arbitrary ordinary-value locals, closures/escaping capture,
action returns, or general first-class designation values. Revisit broader lexical
bindings only when non-designation composition independently earns them.

The current `with <designation> as <name> { ... }` spelling and generated helper-action /
private-String transport are implementation/source-surface details, not the durable law.

## `through` crosses a designation explicitly

`through` follows a designation-valued expression to the designated live target for the allowed read/write/authority operation.

`state through ...` grants writable authority to the exact target resolved at grant time.

Later sequence reorder or designation reselection does not move authority that was already granted.

This preserves the rule that designation and mutation authority are separate semantic axes.

## Rooted locality governs model-local `derived`

A model-local `derived` may directly use that model's own state/derived members and may follow explicit live designations obtained through that model's stored topology.

It does not silently capture arbitrary ambient live state.

This makes ownership/topology visible in source and keeps reusable modeled-state computation from acquiring hidden global dependencies.

Revisit only if realistic composition shows rooted locality is too restrictive or creates disproportionate ceremony.

## Owner-relative `create` establishes fresh identity without structural membership

Inside an action, owner-relative creation creates one fresh modeled-state identity
of model `T` rooted in the named owner/state world.

The current accepted base spelling is:

```elanu
create T in owner
```

The durable semantic law is:

- the new identity is distinct from every existing or separately created identity;
- model state defaults initialize the new identity;
- model-local derived evaluation is rooted at that fresh identity;
- the fresh identity and its owner relationship participate in the current shared
  action transaction;
- successful action commit preserves them;
- action failure discards them;
- creation does not by itself add a designation to any structural sequence;
- creation does not by itself grant writable authority or define destruction
  semantics.

The scoped-designation experiment additionally established that exposing the fresh
identity lexically does not require collapsing designation, authority, lifetime, or
transaction scope.

The current provisional spelling is:

```elanu
create T in owner as item {
    // item designates exactly the fresh T identity
}
```

The durable semantic conclusions from that experiment are:

- the scoped name is a non-owning designation of the exact fresh identity;
- lexical name scope does not define target lifetime;
- leaving the lexical block does not itself destroy the created child;
- designation does not grant writable authority;
- mutation through the designation still requires explicit `through`;
- the block joins the surrounding action transaction rather than creating a nested
  commit boundary;
- failure in the block rolls back the fresh identity and the rest of the shared
  transaction together;
- the exact fresh identity may cross from the lexical creation scope into compatible persistent
  `live T` / `maybe live T` designation state by explicit assignment;
- that persistence copies identity only: it does not create ownership, authority, membership,
  or a second child, and it rolls back with the surrounding action.

The exact scoped spelling remains provisional. The compiler currently supports
member-level reads, derived reads, explicit indirect mutation, structural insertion, contextual
root-owner use, and direct persistence into compatible designation state. This does **not**
treat the whole scoped designation as an ordinary scalar value. The create-and-select pressure
specifically earned one lexical-to-persistent identity crossing because otherwise application
code would have to recover an identity the compiler already knows through a numeric position or
business key.

Composition now establishes that one live modeled child may itself be the explicit rooting
owner of another dynamic child. In the scoped creation form, the exact owner identity may come
from an enclosing fresh scoped designation or from persistent `live T` / present `maybe live T`
state. This does not make designation owning: the designation carries identity only in the
explicit rooting position, and the resulting owner relation is recorded on the newly created
child.

Current compiler restrictions—such as transporting scoped creation through private
marker/builtin actions, generating a private body action, or encoding runtime identities with
generated names—are bootstrap behavior, not durable language law. The unscoped creation path
has not been generalized by this composition and remains narrower in the current compiler.

Structural insertion has shown that both fresh and persistent designations can flow
into compatible membership without broader scalar `live T` flow. That membership may
belong to another modeled-state root without changing the child's creation owner. Do
not infer a general constructor, allocation primitive, local-variable system,
reference model, or ownership-transfer mechanism from the create/insertion experiments.

---

# Structural sequence decisions

## Sequence indexing is zero-based

The first structural position is index `0`.

```text
0    first occurrence
1    second occurrence
```

This is a language semantic choice, not merely a bootstrap/runtime-array detail.
Zero-based indexing keeps source structural position aligned with offset semantics
while preserving the more important Elanu distinction: **position is not identity**.

Changing which designation occupies position `0` changes what a later indexed
operation resolves to; it does not change either child's identity.

Higher-level intent-oriented selection such as `first` or `last` may be explored
later if composition earns it. Such conveniences would not change the zero-based
meaning of `sequence[index]`.

## Runtime sequence positions may be selected by `Int` expressions

For supported owner-relative runtime-sized indexed operations, the index may be an
`Int` expression rather than only a literal.

Decision:

```text
sequence[indexExpr]
    -> evaluate indexExpr in current transaction-visible state
    -> require Int
    -> interpret result as zero-based current structural position
    -> resolve the designation currently stored there
```

For `state through sequence[indexExpr].member`, expression evaluation and structural
resolution occur at grant time. The resulting writable authority targets that exact
concrete member state and does not retarget if the selector or membership later
changes.

Negative and out-of-bounds results are runtime operation failures because their
validity depends on current structure. They participate in ordinary action rollback.

Rationale:

Application state often determines which current structural occurrence is being
viewed or edited. Making the compiler understand that selection directly lets the
same transaction, dependency, structure, and authority laws compose without
introducing a reference, iterator, local alias, or framework-level reconstruction.
The selector chooses a **position**; it does not become child identity or writable
authority.

This position law also survives selection from an owner-relative identity-preserving
derived `filter` view. The selector addresses the current zero-based position in the
current filtered structural value; it does not become an alias, child identity, or
stable occurrence identity.

The view is evaluated in transaction-visible state before each selection, so staged
membership or predicate-driving writes may change which child occupies a later
filtered position. Source order and duplicate multiplicity remain part of the view's
structure.

Filtered selection now composes with reads, direct `through`, and `state through`.
The view itself remains read-only derived structure: mutation or writable authority
resolves to the exact selected child member rather than making the view mutable.

Owner-relative positional `remove` also composes with ordinary `Int` selectors.

Revisit if richer selection requires stable occurrence identity, designation-based
selection, or a broader mechanism that the current expression/structure model cannot
represent cleanly.

## Filtered-view selection resolves exact child state at the relevant semantic time

An identity-preserving derived `filter` view may be indexed with the same zero-based
`Int` positional law used by supported stored membership operations. Read-only member
selection, direct `through` mutation, and `state through` writable-authority grants
share the same current-view selection law while differing in **when** exact state is
resolved.

Decision:

```text
filteredView[indexExpr].member
    -> evaluate the current derived view in transaction-visible state
    -> evaluate indexExpr as Int
    -> select the current zero-based occurrence in that view
    -> read the existing designated child member

through filteredView[indexExpr].storedMember = value
    -> resolve that exact child and stored member at operation time
    -> mutate that exact member identity
    -> do not retarget the write if the mutation changes the later view

state through filteredView[indexExpr].storedMember
    -> evaluate the current view and index at grant time
    -> resolve that exact child and stored member state
    -> grant authority to that exact state identity
    -> do not retarget already-granted authority after later view changes
```

Durable conclusions:

- the filter result remains read-only derived structure; mutation/authority does not
  mutate the view itself;
- indexing does not materialize a new child or copy its state world;
- retained designation identity, source order, and duplicate multiplicity survive
  selection;
- staged membership and predicate-driving writes earlier in the same action affect
  the view used for operation-time or grant-time resolution;
- a direct mutation may change the predicate that selected the child; the already
  resolved write remains attached to that exact child member, while later operations
  re-evaluate the current view;
- a `state through` grant remains attached to the exact resolved member state even if
  the callee mutation makes that child leave the view or later selector/membership
  changes alter the structural position;
- writable authority may be forwarded through writable-state parameters without
  rebinding to the filtered position;
- duplicate occurrences designating the same child resolve the same underlying member
  state rather than occurrence-specific authority;
- model-local derived child members remain readable but non-writable and non-grantable;
- negative and out-of-bounds selection fails transactionally and participates in
  ordinary rollback;
- the selector remains a value rather than an alias or stable occurrence identity;
- read dependencies include the derived view and exact selected-child facts actually
  read; unrelated child facts need not become active dependencies.

Rationale:

`filter` already produces an ordered identity-preserving `[live T]` structural value.
The existing `through` law resolves an exact target at the operation, while the
existing `state through` law resolves exact writable authority at grant time. The
filtered-view experiments show those laws compose without inventing a new reference
or authority category: the current view chooses a child, then ordinary exact-state
semantics take over.

This preserves compiler ownership of structural intent without inventing a reference,
iterator, local alias, stable occurrence identity, or general collection abstraction.
Compiler-private filter bindings, static-index carriers, and runtime-index grant
carriers are implementation details rather than language law.

Revisit only if a future derived structural operation cannot preserve the distinction
between current structural selection and exact child/member identity. Filtered
selection itself no longer requires a separate writable-authority rule.

## Child-directed structural movement is an identity relation over current ordered structure

Decision:

```elanu
move moving before anchor in structure
move moving after anchor in structure
```

Structural movement consumes two compatible live child designations plus one chosen current
ordered structure. It does not consume or expose persistent numeric positions.

The durable rule is:

```text
moving child identity
+ anchor child identity
+ current transaction-visible stored [live T] membership or direct filtered view
+ before | after
    -> reordered backing membership
```

Both designated children must occur exactly once in the chosen structure. Zero current
occurrences fail; duplicate current occurrences of either designated child are ambiguous and
fail. Unrelated duplicate children remain ordinary multiplicity. Moving one unique child
relative to itself is a no-op.

A filtered view is not writable state by itself. It supplies the current ordered occurrence
relation; movement is mapped back to its mutable backing membership. Hidden children preserve
their relative order to one another while visible movement is realized in backing order.
The operation observes transaction-local designation, membership, and predicate changes and
participates in the ordinary shared action transaction and rollback law.

The source form is deliberately statement-oriented. Movement is a structural mutation, not
a pure sequence transformation that happens to be assigned back. Persistent or scoped
selection supplies identity but not writable authority. The compiler owns the structural
operation and may use private helper authority internally; this does not establish general
`[live T]` writable action parameters or make designation an authority value.

Rationale:

Application-side reorder code would otherwise reconstruct child position from indices,
business keys, or whole-sequence transformations even though the compiler already owns exact
child identity and current structural membership. Keeping movement identity-directed lets
Elanu preserve that intent without inventing stable occurrence identity or a general
collection library.

Boundaries:

- the current source surface accepts designation names rather than arbitrary
  designation-producing expressions as move operands;
- only stored membership and direct identity-preserving filtered views are supported;
- movement does not create child lifetime/destruction semantics;
- movement exposes no stable occurrence handle, numeric result, cursor, iterator,
  `find` / `indexOf`, general sequence equality, or general collection API;
- private generated action names, sequence carriers, and transient occurrence indices are
  implementation representation only.

Revisit only when concrete composition requires choosing among duplicate occurrences,
moving across unrelated owners/structures, or expressing a structural transformation that
cannot preserve the existing identity/membership/authority separation.

## Structural insertion stores designation membership without changing child lifetime

Insertion is a structural membership operation over an existing live child identity. The
membership owner and the child's lifetime/rooting owner are separate facts.

Representative provisional source shapes are:

```elanu
create LineItem in invoice as line {
    insert line into otherInvoice.lines
}

insert selectedLine into otherInvoice.lines
```

Durable semantic conclusions:

- insertion stores the existing designation; it does not create another child;
- the inserted occurrence designates the same live child identity;
- insertion changes sequence membership, not child lifetime/rooting;
- compatible membership may belong to a different modeled-state owner from the child;
- cross-owner insertion does not reparent the child, create a second lifetime owner, or
  alter the owner established by creation;
- later reads, filters/reductions, persistent designation, direct `through`, and
  `state through` through the foreign membership continue to resolve the same child;
- insertion grants no writable authority;
- insertion participates in the current shared action transaction;
- rollback removes the staged occurrence together with other staged writes;
- repeated insertion preserves ordered structural occurrence;
- duplicate designation occurrences remain valid sequence structure.

The cross-owner composition pressure established this by keeping a child rooted in owner A,
inserting the same identity into owner B's compatible `[live T]` membership, then exercising
selection, membership observation, filter/reduce, direct mutation, explicit authority,
structural removal, and rollback through B. The child could still be inserted into A's own
membership as the same identity, demonstrating that membership had not silently changed
owner provenance.

This is **non-owning cross-owner membership**, not ownership transfer. Actual lifetime/
rooting transfer or reparenting remains a separate unresolved semantic operation and must
not be inferred from insertion.

The experiment also showed that this composition does **not** earn general first-class
designation values, a general collection API, positional insertion, stable occurrence
identity, ownership/borrowing, or a general reference graph. The source spelling and
compiler-private identity/helper transport remain provisional implementation/surface choices.

Revisit if concrete pressure requires actual owner/rooting transfer, child destruction,
stronger topology constraints on cross-owner membership, or structural relationships that
cannot remain coherent under the current non-owning designation law.

## Positional removal edits one current occurrence, not child identity

The removal experiment established a narrow occurrence-oriented structural edit:

```elanu
remove invoice.lines[0]
```

Durable semantic conclusions:

- an index addresses the occurrence currently at that structural position;
- position is not child identity;
- position is not a permanent occurrence identity;
- removal deletes one occurrence, not every occurrence designating the same child;
- duplicate occurrences remain meaningful and independently removable;
- removal does not destroy the designated child or alter its owner-relative
  modeled-state lifetime;
- removal grants no child writable authority;
- removal is an ordinary transactional structural state change;
- later operations in the same action observe the updated structure;
- rollback restores the prior membership structure.

For `[A, A, B]`, removing position `0` produces `[A, B]`. The remaining
`A` still designates the same child.

The removal index may be an ordinary `Int` expression. It is evaluated in the
current transaction-visible world at the removal operation, then interpreted as a
zero-based current structural position. Staged selector or membership writes earlier
in the same action therefore affect which occurrence is removed.

Negative and out-of-bounds positions are runtime failures because validity depends on
the current runtime-sized structure. They participate in ordinary action rollback.

The selector expression does not become an alias, reference, or stable occurrence
identity. A later removal evaluates its selector again against the then-current
structure.

The same occurrence law now composes through a direct identity-preserving filtered
view. `remove filteredView[indexExpr]` does not mutate the derived view as stored state.
Instead, Elanu evaluates the current transaction-visible view, identifies the exact
backing source occurrence that produced the selected view occurrence, and removes that
one occurrence from the mutable source membership.

This mapping is occurrence-sensitive rather than identity-only. If the backing source
is `[A, B, A]` and all three occurrences are retained, removing filtered position `2`
removes the third backing occurrence and leaves `[A, B]`; the first `A` is not selected
merely because it designates the same child. Non-retained backing occurrences are
skipped while preserving source order and multiplicity.

Staged source-membership and predicate changes affect the current mapping, repeated
removals reevaluate it, and ordinary action rollback restores backing membership after
failure. The operation does not create stable occurrence identity or grant authority to
derived view storage.

The current implementation can recover the backing position transiently from the
current source and current identity-preserving filtered subsequence. That recovery
strategy is implementation detail, but it demonstrates that this pressure does not earn
a permanent occurrence-identity category.

Designation-directed removal is now a separate narrow operation for child-identity
intent. Revisit positional removal if broader editing requires explicit
duplicate-occurrence choice, chained or transformed mutable views, occurrence-local
predicates, or another case where the current source/view relation cannot identify one
backing occurrence coherently. Do not infer child destruction from removal.

## Designation-directed removal uses child identity only when it identifies one current occurrence

Persistent `live T` state can express "this exact child" even after filtered-view
positions move. Structural editing may now consume that identity directly with the
narrow provisional form:

```elanu
remove selectedTask from workstation.tasks
remove selectedTask from workstation.visibleTasks
```

Decision:

```text
at the removal operation:
    read the current transaction-visible persistent live designation
    evaluate the current stored membership or supported direct filtered view
    find current occurrences designating that exact child identity

    exactly one match -> remove that occurrence
    zero matches       -> fail the action
    multiple matches   -> fail the action as structurally ambiguous
```

For a direct identity-preserving filtered view, the unique matching view occurrence is
then mapped to the exact mutable source occurrence that produced it, using the existing
filtered structural-removal law. The view itself remains read-only derived structure.

Durable conclusions:

- staged designation reselection is visible before removal resolution;
- staged membership/reorder and predicate-driving writes are likewise visible;
- child identity is sufficient to select a structural occurrence only when exactly one
  current occurrence in the target designates that child;
- duplicate occurrences of the same child do not acquire hidden occurrence identity;
- ambiguity does not silently choose first match, all matches, or another arbitrary
  policy;
- zero-match and ambiguous-match failures participate in ordinary action rollback;
- removing membership does not clear, destroy, or retarget the persisted designation;
  membership and child lifetime remain separate;
- model compatibility is checked before the operation;
- a later action failure restores the structural edit together with other staged
  changes.

Rationale:

The compiler already owns both facts needed by the application: the persisted live
child identity and the current structural membership/view. Forcing source to copy a
business key and reconstruct a numeric position would make application/framework code
recreate identity information Elanu already possesses. Consuming that identity in one
narrow structural operation therefore advances Elanu's application-intent thesis without
exposing a general lookup or reference mechanism.

The duplicate rule is equally important. A live designation answers "which child?"; it
does not answer "which occurrence of that child?". Failing when the answer is
structurally ambiguous preserves that distinction instead of smuggling stable
occurrence identity into the language.

Current private helper parameters, hidden designation carriers, and runtime identity
encodings are implementation representation rather than language law.

Revisit if application pressure requires intentionally choosing among duplicate
occurrences, no-op-on-missing behavior, or structural editing through transformed/chained
views. Those pressures must not be assumed to imply designation equality, a general
collection API, or stable occurrence identity.

## Maybe-live presence is a read-only fact of designation state

A `maybe live T` designation already stores whether it currently has a target. Elanu now
exposes that compiler-owned fact directly:

```elanu
selectedTask is present
```

Decision:

```text
maybeLiveDesignation is present
    -> read the current transaction-visible designation state
    -> true when it contains one target
    -> false when it contains no target
    -> produce Bool only
```

Durable conclusions:

- presence is valid only for `maybe live T`; plain `live T` already guarantees one
  target and ordinary values do not acquire optionality;
- presence is read-only and grants no writable authority;
- it does not expose or compare child identity;
- it performs no structural membership or collection lookup;
- staged selection and clearing are visible immediately in the current transaction;
- rollback restores the prior presence state;
- presence composes with existing `if`, `and`, `or`, and `not` Boolean control flow;
- short-circuit reachability continues to determine actual dynamic dependencies, so an
  absent guarded member read is neither evaluated nor registered as a dependency;
- no pattern matching, flow-sensitive refinement, generalized `Option`, null
  propagation, or designation equality is implied.

Rationale:

Once `maybe live T` stores absence directly, requiring a parallel Boolean makes
application code duplicate a fact the compiler already owns and permits inconsistent
split state. A narrow Bool observation removes that reconstruction while leaving target
identity, authority, lifetime, and structural membership as separate semantic facts.

The compiler currently lowers presence through its private designation carrier. That
encoding and the compiler-only operator used during lowering are representation, not
language law.

Revisit when application policy must ask a different question: whether the designated
child currently occurs in a particular stored or derived structural view. That is a
membership/reconciliation fact, not designation presence.

## A sequence is one structural value

The minimum sequence type is `[T]`.

A sequence value carries ordered element values. When stored in one `state`, the containing sequence binding has one state identity.

Sequence positions are structural locations, not independent logical state identities.

Therefore reorder changes structure without changing the identities designated by `live T` elements.

## `[live T]` stores designation values, not child ownership

A `[live T]` sequence is an ordered structural value whose elements designate existing live targets.

Copying, reordering, removing, duplicating, or cross-owner inserting those elements manipulates designation values and membership structure. It does not by itself copy, destroy, transfer, or reparent the designated child state worlds.

Duplicates are permitted by the structural sequence abstraction unless a future narrower construct explicitly forbids them.

## Sequence membership and child lifetime are separate semantic axes

Owner-relative creation establishes fresh modeled-state identity and one lifetime/rooting owner independently of structural membership.

A child may therefore exist in its owner/state world without appearing in a particular `[live T]` sequence, may appear in compatible memberships belonging to other modeled-state roots, and may be removed from any such membership without changing its lifetime/rooting owner.

Cross-owner membership is consequently not ownership transfer. Do not collapse lifetime into membership merely because insertion or designation-flow syntax creates ergonomic pressure.

## Whole-sequence replacement participates in ordinary transaction semantics

Replacing a sequence-valued `state` is an ordinary staged state write.

Later reads in the same action observe the staged structural value. Indirect reads/writes through designation-valued elements therefore follow the staged sequence.

Failure rolls back the sequence replacement and any related target writes together.

State-equivalent replacement suppresses invalidation under the existing state-equivalence law.

## `through` may follow designation-valued sequence expressions

A designation-valued expression such as an indexed `[live T]` element may be followed by `through` / `state through`.

For owner-relative runtime-sized sequences, direct indexed reads and direct
`through` mutation now resolve against the current structural value at operation
time. Dynamically inserted identities therefore participate in later indexed
member access without static whole-sequence enumeration.

The durable semantic law remains:

- index position selects the designation currently stored there;
- position is not child identity;
- `through sequence[index].member` targets that exact designated child's member;
- `state through sequence[index].member` grants authority to the exact member
  state resolved at grant time;
- authority already granted does not remain attached to a structural position and
  does not retarget after later reorder/removal.

The runtime-index experiment earned narrow structured compiler representation for
direct indexed reads/writes. That representation is not a general reference value
or new source-level identity category.

The compiler now carries the same runtime-selected fact through the action-argument
authority path for dynamically inserted identities. The grant resolves the current
designation at bind time and the callee receives the exact concrete member state;
later membership edits do not retarget that authority.

The compiler-private carrier used to preserve this fact through the bootstrap
pipeline is representation only. It does not introduce a new source-level
reference or authority category.

---

# Traversal and derived structural views

## `reduce` is the current read-only aggregation primitive

Owner-relative reduction traverses the current structural sequence and computes one result without granting writable authority to elements.

Dynamic dependencies include the source structural value plus the exact child/owner facts actually read during evaluation.

The runtime may execute over a genuine runtime-sized sequence; static whole-sequence enumeration is not semantic law.

## Reduction result type is anchored to the initial expression

The static type of the initial reduction expression is the accumulator type and the result type of the whole reduction.

The accumulator binding has that type at every step. Each step must produce a value assignable to that type under ordinary Elanu assignment compatibility.

Examples:

```text
from 0   + Int step   -> Int
from 0.0 + Float step -> Float
from 0.0 + Int step   -> Float via widening
from 0   + Float step -> invalid
```

The rule is checked even for an empty runtime sequence.

## `filter` is a dedicated identity-preserving derived structural-view surface

The base source form remains:

```elanu
derived activeLines =
    filter lines as line {
        line.quantity > 0
    }
```

Its base meaning is retention/omission:

- source is `[live T]`;
- the element binding is a read-only `live T` designation;
- predicate must be `Bool`;
- result is `[live T]`;
- retained designations preserve identity and duplicate multiplicity;
- without an ordering clause, source order is preserved;
- no writable authority or child lifetime ownership is introduced.

The view's dynamic dependencies are the source sequence plus the exact owner/child facts
actually read by the predicate. A downstream reduction may consume the derived filter
result as a runtime-sized structural sequence.

## Derived order is a view fact, not stored membership order

Composition pressure from a priority-driven dispatch queue established a second structural
fact: an application may need the order of a read-only view to follow current child state
without maintaining a mirrored mutable sequence.

The accepted narrow surface is:

```elanu
derived visibleTickets =
    filter tickets as ticket {
        ticket.open
    } order by ticket.priority descending
```

Current durable law:

- the ordering key is one current child `Int` member;
- direction is explicit ascending or descending;
- filtering determines membership first;
- ordering changes only the derived view order, never backing membership order;
- equal keys preserve source occurrence order, giving deterministic stable ties without
  inventing another key or stable occurrence identity;
- the same child identities and duplicate multiplicity survive ordering;
- ordering-key reads are ordinary dynamic dependencies of the derived computation;
- staged key changes participate in the existing transaction law and rollback;
- plain filters retain their established source-order semantics;
- an ordered derived view is read-only structure and is not a structural `remove`/`move`
  selector in the current language.

This distinction is intentional:

```text
stored membership order
    mutable application-owned structural state

derived order
    read-only computation from current application facts
```

The compiler owns the dependency between the child key facts and derived view order. Requiring
application code to issue a matching structural `move` whenever an ordering fact changes would
duplicate an invariant the compiler can already observe and maintain as derived computation.

Stable source-order ties were selected because the application pressure did not earn another
tie-break fact or occurrence identity. Structural editing through an ordered view is rejected
rather than silently defining how an automatically reordered occurrence maps to a backing edit.
That boundary should be revisited only under concrete mutation pressure.

This decision does **not** select a general query language, comprehension syntax, higher-order
collection API, arbitrary comparator, multi-key ordering, locale/string collation, Float/NaN
ordering, general `sort`, group, distinct, or incremental-maintenance contract.

`filter` and its current ordering clause remain narrow source surfaces and may be revisited if
broader composition shows that a stronger application-semantic abstraction is required.

---

# Failure, history, effects, and other axes must remain separate

Future design should continue to distinguish:

- mutation authority;
- transaction scope;
- concurrency isolation;
- external observation/effects;
- recovery scope;
- child lifetime/ownership;
- structural membership;
- history policy such as undo/replay/persistence.

Current `action` intentionally couples mutation of Elanu-managed state with one synchronous atomic transition. It does not promise rollback of arbitrary external effects, suspension across async work, savepoints, or concurrency isolation.

Likewise, `live` designation identifies a target but does not own it, and sequence membership structures application relationships but does not automatically own target lifetime.

New features may connect these axes only through explicit evidence and a new design decision.

---

# Deferred / not-yet-selected decisions

The following remain open and must not be inferred from conventional languages or bootstrap implementation techniques:

- how to represent an intentionally absent/cleared `live T` designation, if current composition proves one is needed;
- whether a non-mutating designation-presence/equality operation is ever needed beyond current narrow structural consumption;
- whether broader scalar `live T` flow is required beyond persistent state and specialized designation contexts;
- exact initialization syntax beyond model defaults for a newly created identity;
- positional insertion and move/reorder surface beyond current append/remove;
- immutable action-local bindings such as `let`;
- mutable untracked locals;
- ordinary functions and closures;
- action return values;
- general collection APIs, maps, or sets;
- general reference/pointer/borrowing systems;
- individual child destruction or ownership transfer/reparenting;
- recoverable errors/savepoints/partial commits;
- async/concurrency semantics;
- external effects and effectful reads;
- time/scheduling semantics;
- persistence, undo, replay, synchronization, and identity across process boundaries;
- a permanent memory-management model;
- a general compiler IR architecture.

Active composition pressure, historical sequencing, and exploratory evidence are maintained separately from this durable decision index.

---

# Compiler architecture is evidence, not language law

The bootstrap compiler has used direct checked-AST interpretation, preprocessing, hidden generated bindings, specialized lowering, typed sidecars, and compiler-private transports at different stages.

Those mechanisms are implementation choices unless a semantic decision explicitly depends on them.

The post-filter review currently earns a cleaner resolved/typed semantic boundary: later compiler phases should receive resolved bindings and established semantic/type facts without repeatedly reconstructing them where practical.

That direction does **not** itself select a wholesale HIR/MIR/SSA stack, bytecode VM, JIT, or universal IR. A new representation layer should earn itself because a concrete compiler phase requires information the existing representation cannot carry cleanly.

---

# Status and revisit discipline

A documented decision is current law only to the extent explicitly stated. Elanu remains experimental.

Distinguish:

```text
established Elanu semantics
    -> current programmer-visible law

bootstrap implementation behavior
    -> current compiler/runtime mechanism, replaceable if behavior is preserved

provisional design direction
    -> strongest current candidate under active pressure

speculative future idea
    -> not selected
```

A future semantic domain that activates a documented revisit trigger must pressure-test the old decision rather than treating prior implementation as proof that composition will work.

If a decision survives, record the additional evidence. If it fails, update the semantic model, compiler/runtime behavior, tests, language reference, roadmap/composition documents, and rationale together.