# Elanu language reference

This document describes the **current compiler-accepted Elanu source surface**.

Elanu remains experimental. The latest tagged release is **v0.9.0**. This reference describes the current compiler-accepted language surface built on that baseline. Features explicitly marked provisional remain subject to refinement even when included in the current compiler.

Status labels used here:

- **Released** — part of the tagged v0.9.0 language baseline.
- **Selected/implemented** — accepted by the current compiler/runtime and supported by composition evidence.
- **Provisional** — current source spelling or boundary may still change even though the compiler accepts it.

Do not infer features from conventional languages merely because they would be familiar.

For the durable semantic rationale behind the current language, see
[`DESIGN_DECISIONS.md`](DESIGN_DECISIONS.md).

---

# 1. Core program structure

Top-level declarations include:

```elanu
state
derived
action
state model
```

`state`, `derived`, `action`, and `state model` are part of the current compiler-accepted language surface.

Example:

```elanu
state price = 10
state quantity = 2

derived total = price * quantity

action purchase {
    quantity -= 1
}
```

Top-level declarations may be separated by newlines or semicolons.

Single-line comments begin with `//`. Block comments are not currently supported.

---

# 2. Primitive values and types

Released primitive types are:

```text
Int
Float
Bool
String
```

Integer literals use decimal digits. Floating literals currently require digits on both sides of the decimal point, for example `10.0` or `0.08`.

Boolean literals are:

```elanu
true
false
```

Strings use double quotes. Supported escapes include:

```text
\n
\r
\t
\"
\\
```

String interpolation is not currently supported.

Numeric assignment compatibility allows `Int -> Float` widening where an ordinary value is required. Writable state authority does **not** widen because authority must target an exact state slot type.

---

# 3. `state` — released

`state` declares mutable application state with stable state identity.

```elanu
state count = 0
state price: Float = 10
```

A state declaration requires an initializer. If no type annotation is supplied, the type is inferred from the initializer.

State may be mutated only inside an `action`.

```elanu
state count = 0

action increment {
    count += 1
}
```

Writes are staged inside the current action transaction. Later reads in the same transaction observe earlier staged writes. Successful top-level actions commit atomically; failure discards staged writes.

Ordinary reads of state produce values. They do not create writable aliases.

---

# 4. `derived` — released kernel, extended post-release composition

`derived` declares a read-only computed value.

```elanu
state price = 10
state quantity = 2

derived total = price * quantity
```

A derived value:

- has no independently mutable storage;
- cannot be assigned;
- records the state/derived identities actually read during evaluation;
- is cached;
- is invalidated when an active dependency meaningfully changes;
- observes transaction-local staged state when read inside an action;
- may change its dependency set between evaluations.

Derived evaluation is lazy. An invalid cached result is not program-visible as the value of the derived binding; the next read must establish a value valid for the current visible state.

The runtime may safely suppress downstream work when an invalidated pure derived recomputes to an equivalent observable value. That is an implementation optimization, not a source-visible stale/current/history mechanism.

If historical values matter to application logic, preserve them explicitly as `state`.

---

# 5. Expressions and operators — released core, extended post-release composition

Elanu currently supports primitive literals, names, arithmetic, comparisons, equality, and expression `if`.

Arithmetic operators:

```text
+
-
*
/
```

`+` also supports `String + String` concatenation.

Comparison/equality operators:

```text
==
!=
<
<=
>
>=
```

Ordering comparisons are numeric. Equality/inequality support the current primitive equality domains accepted by the compiler.

Selected/implemented String containment is:

```elanu
text contains search
text contains search ignoring case
```

Both forms require `String` operands and produce `Bool`.

`text contains search` performs exact case-sensitive substring containment. It does not perform case folding, locale-sensitive comparison, or Unicode normalization.

`text contains search ignoring case` performs locale-independent Unicode default full case folding on both operands and then tests exact substring containment of the folded text. It does not perform Unicode normalization or locale-specific tailoring. Consequently, case-fold equivalents such as `Straße` / `STRASSE` and Greek sigma forms match, while canonically equivalent text with different normalization forms is not made equivalent automatically.

The empty String is contained in every String in both forms. Both forms read exactly their two operands under the ordinary expression/dependency rules. Surrounding Boolean short-circuiting may prevent the containment expression from being evaluated, in which case neither containment operand is read.

`contains`, `ignoring`, and `case` are contextual words in this surface rather than globally reserved identifiers. These are narrow text predicates, not a general String method/library, locale/collation, or normalization surface.

Selected/implemented Boolean operators are:

```text
not
and
or
```

Their operands must statically type as `Bool`, and their result type is `Bool`. `and` and `or` short-circuit:

```text
A and B
    evaluate A
    if A is false, return false without evaluating B
    otherwise evaluate and return B

A or B
    evaluate A
    if A is true, return true without evaluating B
    otherwise evaluate and return B

not A
    evaluate A once and return its Boolean negation
```

Short-circuiting is semantic, not merely an optimization. An operand that is not reached is not read and therefore does not become an active dynamic dependency of a `derived` evaluation. If later state makes that operand reachable, its actually-read facts become dependencies on that evaluation. Transaction-local evaluation uses staged state to decide reachability, and rollback discards temporary dependency changes.

Current precedence from tighter to looser is arithmetic, comparison, containment (including `ignoring case`), equality, `not`, `and`, then `or`; parentheses may be used explicitly. `and`, `or`, and `not` are currently parsed contextually rather than being globally reserved identifiers.

Expression `if` requires `else` because it produces a value:

```elanu
derived label = if enabled {
    "Enabled"
} else {
    "Disabled"
}
```

The condition must be `Bool`. Compatible `Int` / `Float` result branches produce static type `Float`.

---

# 6. `action` — released

An `action` is a synchronous atomic transition over Elanu-managed state.

Zero-parameter form:

```elanu
action refresh {
}
```

Calls use parentheses:

```elanu
refresh()
```

Typed parameters use:

```text
name: Type
state name: Type
```

Example:

```elanu
action setQuantity(state target: Int, newValue: Int) {
    target = newValue
}
```

Ordinary parameters are immutable values.

`state` parameters carry explicit writable authority to one exact existing state slot.

A caller grants authority explicitly:

```elanu
state quantity = 1

action increment(state target: Int) {
    target += 1
}

action demo {
    increment(state quantity)
}
```

The distinction is:

```text
quantity        -> ordinary value read
state quantity  -> explicit writable authority grant
```

Writable authority may be forwarded explicitly through another writable state parameter.

Nested actions join the current transaction. They do not create implicit independent commit boundaries.

Action calls are statement-only and currently return no value.

---

# 7. Assignment — released

Assignments are action statements.

Supported forms include:

```text
=
+=
-=
*=
/=
```

Targets must denote writable state available to the action, including writable `state` parameters and supported indirect `through` targets described below.

Ordinary value parameters and `derived` bindings are not assignable.

---

# 8. Statement `if` and `fail` — released

Statement `if` is available inside actions:

```elanu
action update {
    if enabled {
        count += 1
    } else {
        count = 0
    }
}
```

`else` is optional for statement conditionals.

`fail` aborts the current shared action transaction:

```elanu
fail "Out of stock"
```

The failure expression must have type `String`.

Elanu does not currently provide `try`, `catch`, `throw`, or independent nested transaction semantics.

---

# 9. `state model` — v0.8.0

A state model defines one reusable state-capable shape.

```elanu
state model LineItem {
    state quantity = 1
    state unitPrice = 10.0

    derived lineTotal = quantity * unitPrice
}
```

A modeled state variable creates a distinct live modeled-state world:

```elanu
state lineA: LineItem
state lineB: LineItem
```

Distinct modeled state variables of the same model have distinct constituent member identities.

Every concrete modeled-state identity, whether statically declared or dynamically created, has its own instance of the model's declared state. This includes model-local `[live T]` structural state and derived structural views. Reaching such a member through a compatible live designation resolves the member belonging to that exact modeled-state identity; it does not reconstruct ownership from a key, position, or copied owner value.

Reading modeled state ordinarily remains value-oriented; it does not silently create a writable alias to the live modeled-state world.

Model-local `derived` uses **rooted locality**: it may directly name that model's own members and may follow explicit live designations obtained through the model's stored topology. It may not silently capture unrelated ambient live state.

This distinction is semantic. Current hidden-name/lowering details are bootstrap implementation behavior.

---

# 10. `live T` designation — v0.8.0

`live T` identifies which existing live modeled-state target is meant without granting writable authority.

Conceptually:

```elanu
state active: live Invoice = live invoiceA
```

The key categories are:

```text
ordinary T value
    detached value

live T
    non-owning designation of an existing live target

writable authority
    permission to mutate one exact target/state identity
```

A designation is not a general pointer/reference and does not own the target lifetime.

A scalar `state` may persist one `live T` designation and may be reassigned from a
compatible whole-designation selection from supported `[live T]` structure:

```elanu
state selectedTask: live Task = live initialTask

action select {
    selectedTask = workstation.visibleTasks[selectedIndex]
}
```

That assignment evaluates the current sequence/view and index in transaction-visible
state and stores the exact selected child identity. Later selector, membership, or
filter changes do not retarget the stored designation. Reselection is ordinary state
mutation and rolls back with the surrounding action.

Membership and filtered-view participation remain separate from child lifetime. A
stored designation therefore continues to designate the same live child if its
membership occurrence is removed or if it later stops satisfying the filter, provided
the child itself remains live. Explicit owner-relative `destroy` may end the lifetime of
a supported committed dynamic leaf child. That operation clears persistent `maybe live T`
designations targeting the terminated child transactionally, while any remaining plain
`live T` designation blocks termination so its exactly-one-live-target invariant cannot
become dangling.

Member reads may follow a live designation.

Indirect mutation crosses the designation explicitly with `through`. Persisting a
`live T` designation does not itself grant writable authority. A compatible persistent
scalar designation may also be consumed by the narrow designation-directed structural
removal form documented below; that use does not turn the designation into writable
authority, equality, or a general collection lookup value.

A scalar `live T` state always designates exactly one live target. Absence is a
separate, selected/implemented live-designation state form:

```elanu
state selectedTask: maybe live Task = none
```

`maybe live T` means either one exact compatible live `T` target or no target. It may
receive supported whole-designation selection from `[live T]` structure just as plain
`live T` state can, and it may be cleared with contextual `none`:

```elanu
selectedTask = workstation.visibleTasks[selectedIndex]
selectedTask = none
```

Plain `live T` does not accept `none`; its stronger exactly-one-target guarantee is
preserved. `maybe` in this type position and `none` in this designation context do not
create general nullable values or a general `Option` type.

Clearing a `maybe live T` designation does not destroy its former child and does not
edit structural membership. Likewise, removing a membership occurrence does not
automatically clear a present designation. Membership, child lifetime, and designation
presence remain separate semantic axes.

Using an absent `maybe live T` as though it had a target is a runtime action failure.
This includes member read, explicit `through` mutation, `state through` authority grant,
and designation-directed structural removal. The failure participates in the ordinary
shared action transaction, so earlier staged writes roll back.

Presence is observed directly with the read-only predicate:

```elanu
selectedTask is present
```

`is present` is valid only for `maybe live T` designation state and produces `Bool`.
It reads whether that designation currently contains a target; it does not project the
target, expose identity, grant writable authority, search a collection, or ask whether
the target occurs in a particular structural view.

Presence observes current transaction-visible designation state. A selection staged
earlier in the same action makes the predicate true; assigning `none` makes it false.
Rollback restores the prior presence state with the rest of the action transaction.

Presence composes with ordinary short-circuit Boolean control flow. For example:

```elanu
selectedTask is present and selectedTask.title contains search
```

When `selectedTask` is absent, `and` does not evaluate the member-read operand. The
existing actual-read dependency law therefore tracks the designation presence fact but
does not invent a dependency on `selectedTask.title` until that operand is actually
reached. `is` and `present` are contextual in this predicate rather than globally
reserved words.

A persistent live designation may also be tested for current child-level membership in
supported stored `[live T]` membership or a direct identity-preserving filtered view:

```elanu
selectedTask is in workstation.tasks
selectedTask is in workstation.visibleTasks
```

`is in` produces `Bool`. The left operand must be a compatible persistent `live T` or
`maybe live T` designation and the right operand must be a supported `[live T]`
structural value with the same element model. The predicate asks whether the exact
designated child occurs at least once in the current transaction-visible structure.
It does not return a position or occurrence.

For `maybe live T`, absence produces `false`. A present designation may remain present
while `is in` becomes false because membership and filter participation are separate
from designation presence and child lifetime. Staged membership and predicate-driving
writes are visible to the predicate, and ordinary action rollback restores the prior
relation.

Duplicate occurrences are existential for this read-only predicate: one or more current
occurrences designating the same child produce `true`. This does not create stable
occurrence identity or choose among duplicate occurrences. `is in` does not expose
general designation equality, `find`/`indexOf`, iteration, writable authority, or a
general collection query API. `is` and `in` are contextual in this predicate rather
than globally reserved words.

### Relative navigation — selected/implemented

A persistent `live T` or `maybe live T` designation may be reassigned relative to its
unique current occurrence in supported ordered structure:

```elanu
selectedTask = next selectedTask in workstation.tasks
selectedTask = previous selectedTask in workstation.visibleTasks
```

`next` / `previous` are designation-producing relations, not general collection
operations. The anchor must be a compatible persistent designation and the source must be
a supported stored `[live T]` membership or direct identity-preserving filtered view of
the same `T`. The form is accepted only where the compiler already expects a compatible
`live T` or `maybe live T` designation result; it does not make whole designations ordinary
first-class values.

Execution reads both anchor and source from the current transaction-visible world. The
anchor child identity must identify exactly one current occurrence in the source. Zero
current occurrences fail the action. Multiple occurrences of the anchor child are
ambiguous and fail the action. At the first/last boundary, requesting a missing previous
or next neighbor currently fails the action. An absent `maybe live T` anchor also fails.
All such failures participate in the ordinary shared action transaction and roll back
prior staged writes.

On success, only the exact neighboring child designation is produced and persisted. The
transient occurrence used to resolve adjacency is not source-visible identity and is not
retained. Staged membership changes, staged predicate-driving changes to a filtered view,
and staged reselection of the anchor are all observed before navigation. A later failure
rolls the successful reselection back with the rest of the action.

This surface does not add designation equality, `find` / `indexOf`, stable occurrence
identity, cursors, iterators, wraparound/clamping, collection methods, or general
first-class designation values. `next`, `previous`, and `in` are contextual in this form
rather than globally reserved words.

### Child-directed structural movement — selected/implemented

Inside an `action`, Elanu can move one currently designated child before or after another
within supported ordered structure:

```elanu
move moving before anchor in workstation.tasks
move moving after anchor in workstation.visibleTasks
```

`moving` and `anchor` must name compatible live designations for the element model. The
current narrow surface accepts persistent `live T` / `maybe live T` designation state and
scoped immutable designation names; it does not make arbitrary designation-producing
expressions into general structural operands.

The chosen `in` structure must be mutable stored `[live T]` membership or a direct
identity-preserving filtered view of such membership. The statement mutates structural
membership, not child state or child lifetime. The designation names contribute exact child
identity only; they do not themselves grant writable authority.

Both the moving child and anchor child must each have exactly one current occurrence in the
chosen transaction-visible structure. Zero occurrences fail the action. Multiple occurrences
of either child are ambiguous and fail rather than choosing one occurrence. An absent
`maybe live T` therefore fails when used as either operand. Unrelated duplicate children do
not make the operation ambiguous.

For a direct filtered view, occurrence ordering is resolved in the current view and the
compiler/runtime maps that movement back to the mutable backing membership. Rows hidden from
the view retain their relative order with respect to one another; they do not occupy fixed
slots between visible rows. Transaction-local membership, designation reselection, and
filter-predicate changes are observed before movement, and ordinary action rollback restores
the prior order if the move or any later work fails.

Moving a uniquely occurring child relative to itself is a no-op. The operation returns no
position, occurrence handle, cursor, or sequence value and does not introduce general
`moveAt`, `insertAt`, `find` / `indexOf`, iterator, or collection-method APIs.

Current generated structural helper actions, private sequence carriers, and occurrence
indices are compiler representation rather than source-visible authority or identity.

### Scoped immutable designation binding — selected/implemented

Elanu can capture one present live designation once and give that exact child identity an
immutable lexical name for a bounded block. The current accepted spelling is provisional:

```elanu
with selectedTask as removalTarget {
    selectedTask = next selectedTask in workstation.visibleTasks
    remove removalTarget from workstation.visibleTasks
}
```

The expression before `as` must be a supported designation-producing expression with one
compatible live model. Current supported examples include a persistent `live T` or
present `maybe live T` designation, indexed selection from supported `[live T]` structure,
and contextual `next` / `previous` navigation. The expression is evaluated exactly once
at block entry in the current transaction-visible world.

The bound name designates that exact captured child for the life of the lexical block. It
is immutable: it cannot be assigned or reselected. Later changes to persistent selection,
sequence membership/order, or filter participation do not retarget the captured identity.
Those later structural changes may still affect whether a subsequent structural operation
on the captured child succeeds—for example, designation-directed removal still requires a
unique current occurrence at the time of removal.

A present `maybe live T` may be captured. Capturing an absent maybe-live designation is a
runtime action failure and rolls back prior staged work in the surrounding action.

Inside the block, the captured designation may participate in the same narrow operations
that already consume compatible designations, including member reads, explicit `through`
mutation, `state through` writable-authority grants, membership relations, relative
navigation, and designation-directed structural removal. The binding itself does not grant
writable authority; authority remains explicit and resolves to exact state identity under
the existing rules.

The block is not a transaction boundary. Nested actions join the same surrounding action
transaction, and later failure rolls back writes and structural edits normally. Leaving
the block ends only the lexical name's scope; it does not destroy the child, remove
membership, clear persistent designations, or otherwise affect child lifetime.

This feature is specifically **not** a general local-variable system. It does not add
`let` / `var`, mutable locals, arbitrary primitive/model/sequence locals, closures or
escaping capture, action returns, or general first-class designation values. The current
`with ... as ... { ... }` spelling may still be refined without changing the scoped
identity-flow law.

---

# 11. `through` and `state through` — v0.8.0

`through` explicitly follows a designation-valued expression for mutation/read-through behavior supported by the current surface.

Representative indirect mutation:

```elanu
through active.quantity = 5
```

Writable authority may be granted through a designation at an action-call boundary:

```elanu
updateLine(state through active)
increment(state through active.quantity)
```

Authority resolves to the exact designated identity at grant time. Later reselection or sequence reorder does not move already granted authority.

`through` does not make the designation itself writable authority.

---

# 12. Ordered structural sequences `[T]` — v0.8.0

Sequence type syntax:

```text
[T]
```

Examples:

```elanu
[Int]
[String]
[live LineItem]
```

A sequence is one ordered structural value.

Important laws:

- position is not live state identity;
- order is observable;
- duplicate elements/designations are allowed;
- copying a `[live T]` sequence copies designation values, not the target state worlds;
- reordering designations changes structure without changing designated target identities;
- when stored in one `state`, the sequence contributes one containing state identity rather than independent state per position.

Sequence literals use bracket syntax:

```elanu
[]
[1, 2, 3]
[live invoiceA, live invoiceB]
```

An empty literal needs an expected sequence type in the current surface, for example:

```elanu
state lines: [live LineItem] = []
```

Whole-sequence replacement inside actions is supported by the current sequence surface.

The current language does **not** imply a conventional list API merely because sequence values exist.

---

# 13. Sequence indexing — selected/implemented runtime `Int` expressions

Indexing uses:

```elanu
sequence[indexExpr]
```

Elanu sequence indexing is **zero-based**:

```text
sequence[0]    first structural position
sequence[1]    second structural position
```

Position remains structural and is not live target identity.

For the implemented owner-relative runtime-sized path, `indexExpr` may be any
currently supported expression whose static type is `Int`:

```elanu
state selectedLine = 0

observed = invoice.lines[selectedLine].quantity
through invoice.lines[selectedLine + 0].quantity += 1
increment(state through invoice.lines[selectedLine].quantity)
```

Indexed member access also composes with an owner-relative derived `filter` view:

```elanu
observed = invoice.activeLines[selectedLine].quantity
through invoice.activeLines[selectedLine].quantity += 1
increment(state through invoice.activeLines[selectedLine].quantity)
```

Whole-designation selection may be assigned to compatible scalar `live T` or
`maybe live T` state from both stored owner-relative membership and a direct
owner-relative filtered view:

```elanu
selectedLine = invoice.lines[indexExpr]
selectedLine = invoice.activeLines[indexExpr]
```

Literal and runtime `Int` selectors have the same identity meaning for these supported
owner-relative paths.

Current contract:

- the index expression must have static type `Int`;
- it is evaluated in current transaction-visible state at the indexed operation;
- the resulting integer is interpreted as a zero-based current structural position;
- member reads follow the designation currently stored at that position;
- when the indexed expression is used as a compatible `live T`/`maybe live T`
  assignment value, the operation yields the exact designation currently stored at
  that position;
- the receiving scalar designation state stores that child identity rather than the
  structural position that selected it;
- later selector, membership, or filter changes do not retarget an already stored
  designation;
- stored and model-local derived members may be read;
- direct `through` mutation resolves to the exact current designated child's stored
  member;
- `state through` evaluates the index and resolves the exact concrete member state
  at grant time;
- authority already granted does not retarget after later selector or membership
  changes;
- later indexed operations re-evaluate the expression and current structure;
- staged selector and membership writes earlier in the same action are visible;
- a negative or out-of-bounds indexed read/write/grant fails the action at runtime;
- failure rolls indexed writes and selector/membership changes back with other action
  state changes;
- writable authority preserves exact state type and does not use numeric widening;
- position remains a structural address and never becomes child identity or a
  permanent occurrence identity.

For indexing of an owner-relative derived `filter` view specifically:

- the current filtered view is evaluated in transaction-visible state;
- source order and duplicate designation multiplicity are preserved;
- the `Int` selector addresses the current zero-based position in that view;
- retained designations preserve the existing child identity;
- stored and model-local derived members of the selected child may be read;
- direct `through filteredView[indexExpr].storedMember` mutation is accepted;
- direct mutation resolves the exact selected child and stored member at operation
  time, then applies the write to that exact member identity;
- `state through filteredView[indexExpr].storedMember` writable-authority forwarding
  is accepted;
- a `state through` grant evaluates the current filtered view and selector at grant
  time, then resolves one exact stored member state on the selected child;
- granted authority remains attached to that exact member state if later selector,
  source membership, or predicate changes alter the filtered view;
- writable authority may be forwarded through later writable-state parameters without
  rebinding to the view position;
- if a direct write or granted mutation changes filter membership, the already
  resolved operation/authority does not retarget, while later filtered-view operations
  observe the newly current view;
- staged membership changes and staged predicate-driving state earlier in the same
  action affect the current view before selection or grant resolution;
- negative and out-of-bounds positions fail transactionally;
- dependency tracking for reads follows the filtered view plus the exact selected
  child facts actually read;
- model-local derived child members may be read but cannot receive direct mutation or
  writable-state authority;
- indexing the view does not create stable occurrence identity.

This runtime-sized behavior supersedes the older bootstrap restriction that source
indices had to be non-negative integer literals for the owner-relative read/direct
`through`/`state through` path.

Legacy top-level sequence indexing still uses older bootstrap machinery. The
owner-relative structural-removal surface now accepts the same ordinary `Int`
selector expressions described below. Remaining implementation/surface differences
are not semantic claims that positions have different identity meaning.

Whole-designation selection is currently accepted narrowly as compatible `live T`
state assignment on the supported owner-relative stored/filtered paths. Unconstrained
ordinary whole-designation value projection, slicing, iterators, and general
collection APIs remain outside the currently accepted surface.

---

# 14. State-model sequence members — v0.8.0

A state model may store owner-relative structural membership:

```elanu
state model Invoice {
    state lines: [live LineItem] = []
}
```

Each concrete `Invoice` root has a distinct `lines` state identity.

The sequence stores non-owning designations. Membership does not own child lifetime. A
membership may designate a compatible live child rooted in another modeled-state owner;
that does not change the child's owner/rooting relation.

Removing a designation from membership is therefore not, by itself, child destruction.

Current compiler lowering externalizes some of this storage through hidden implementation bindings. Those hidden names are not Elanu source semantics.

---

# 15. Owner-relative `create` — provisional/implemented

The current compiler accepts owner-relative creation as an action statement:

```elanu
action addLine {
    create LineItem in invoice
}
```

It also accepts a provisional creation-scoped designation form:

```elanu
action addConfiguredLine {
    create LineItem in invoice as line {
        through line.quantity = 3
        observedTotal = line.lineTotal
    }
}
```

Current semantic contract:

- `LineItem` names a `state model`;
- creation allocates one fresh live modeled-state identity of that model;
- the fresh identity records one exact rooting owner identity;
- the existing unscoped `create T in owner` path still accepts a statically declared modeled-state root;
- the scoped form additionally accepts an exact current live modeled identity as owner when that identity is carried by a persistent `live T` / present `maybe live T` designation or by an enclosing fresh scoped creation binding;
- using a live designation as the owner operand does not make the designation owning, grant writable authority, or transfer/reparent any existing child;
- model state members are initialized from their declared defaults;
- model-local derived members evaluate against the fresh identity;
- creation participates in the current action transaction;
- successful commit preserves the fresh identity and exact owner relationship;
- `fail` or another action failure rolls the creation back;
- an absent `maybe live T` owner fails transactionally rather than inventing an owner identity;
- repeated creation produces distinct identities;
- creation alone does **not** insert the fresh designation into any `[live T]`
  sequence;
- creation does not itself grant writable authority or define child destruction.

For the scoped form:

- `line` is a lexical, non-owning `live LineItem` designation of exactly the
  fresh identity created by that statement;
- `line.member` may read stored members and model-local derived members;
- reads observe earlier staged writes in the same action transaction;
- mutation through the designation requires explicit `through line.member`;
- writable authority remains explicit and separate from designation;
- the scoped body shares the surrounding action transaction rather than creating a
  nested commit boundary;
- failure inside the body rolls back the fresh identity and all other staged writes
  in that action;
- the exact fresh designation may be assigned directly to compatible persistent `live T` or
  `maybe live T` state, persisting that same child identity without structural lookup;
- such persistence is ordinary transactional state assignment: later action failure restores
  the prior persistent designation together with the fresh child and other staged effects;
- the assignment grants no writable authority and does not change rooting, lifetime, or
  structural membership;
- the lexical name does not escape the block;
- leaving the block does not itself destroy the fresh child.

The unscoped statement remains useful when the source does not need to refer to the
fresh designation afterward; it discards the source-level designation.

The scoped fresh-child designation is **not an ordinary freely flowing scalar value**
in the current compiler. Source can operate through its members, may use the narrow
structural insertion form documented below, may use an enclosing fresh scoped identity as the
explicit rooting owner of another scoped creation, and may explicitly persist the exact fresh
identity into compatible persistent designation state:

```elanu
state selectedLine: maybe live LineItem = none

action addAndSelect {
    create LineItem in invoice as line {
        insert line into invoice.lines
        selectedLine = line
    }
}
```

That assignment copies child identity into designation state; it does not copy child state,
create another child, grant authority, or make the scoped designation an ordinary `String` or
general first-class value. Other bare whole-designation uses remain rejected.

For example, a dynamic project may root a fresh task without an application-maintained
business-key or mirrored owner field:

```elanu
create Project in workspace as project {
    create Task in project as task {
        insert task into workspace.tasks
    }
}
```

A committed dynamic owner may likewise be named by persistent designation state:

```elanu
state selectedProject: maybe live Project = none

action addTask {
    create Task in selectedProject as task {
        insert task into workspace.tasks
    }
}
```

Current compiler restrictions should not be mistaken for broader language law. Scoped
creation is transported through compiler-private lowering/generated actions, and the owner
identity is preserved only in the explicit owner position. Generated names and private
builtin machinery are not Elanu source semantics.

Neither form of `create T in owner` is a general constructor, general allocation
primitive, general local-binding facility, or reference operation.

---

# 16. Structural insertion — provisional/implemented

Elanu accepts append-style insertion of an exact live child designation into compatible
mutable `[live T]` structural membership. The membership owner need not be the child's
lifetime/rooting owner.

A fresh creation-scoped designation may be inserted directly:

```elanu
create LineItem in invoice as line {
    insert line into otherInvoice.lines
}
```

An already-existing persistent designation may also be inserted after creation scope has
ended:

```elanu
state selectedLine: maybe live LineItem = none

action share {
    insert selectedLine into otherInvoice.lines
}
```

Current contract:

- the source must designate one exact live child of the destination element model;
- fresh scoped insertion uses the current designation from the surrounding `create`;
- existing-designation insertion currently accepts persistent `live T` / `maybe live T`
  designation state;
- the destination must be mutable owner-relative `[live T]` membership with the same
  element model as the designation;
- an absent `maybe live T` source fails the action transactionally;
- insertion appends one occurrence of that exact designation to the destination sequence;
- insertion preserves child identity and does not create a second child;
- structural membership is non-owning, so insertion may cross modeled-state owner
  boundaries without transferring, replacing, or changing the child's established
  lifetime/rooting owner;
- the same child may therefore participate in compatible memberships owned by different
  roots while retaining one unchanged live identity and owner/rooting relation;
- insertion does not grant writable authority;
- insertion participates in the current shared action transaction;
- failure rolls back the membership change with other staged writes;
- repeated insertion preserves order and duplicate designation occurrences remain legal.

Insertion composes with designation-directed removal, persistent selection, filtered views,
reductions, direct `through` mutation, and `state through` authority using the same child
identity laws. Cross-owner membership does not make the destination owner a second lifetime
owner and does not reparent the child. Reads and explicit authority through that membership
resolve the existing designated child state.

This remains deliberately narrower than a general collection or ownership API. Lifetime/
rooting transfer or reparenting, positional insertion, arbitrary designation expressions,
and child destruction remain unresolved or unsupported. A structural insertion is not an
ownership operation merely because the destination membership belongs to another modeled-
state root.

The spelling `insert ... into ...` remains provisional. Compiler-private String identity
transport and generated helper actions are implementation representation, not source law.

---

# 17. Structural occurrence removal — provisional/implemented

The current compiler accepts two narrow owner-relative structural-removal forms.

Positional removal uses an `Int` expression:

```elanu
state selectedLine = 0

remove invoice.lines[selectedLine]
remove invoice.lines[selectedLine + 1]
```

Designation-directed removal uses compatible persistent scalar `live T` state:

```elanu
state selectedLine: live LineItem = live initialLine

remove selectedLine from invoice.lines
remove selectedLine from invoice.activeLines
```

For both forms, the target may be a mutable owner-relative `[live T]` state-model
member or a direct owner-relative identity-preserving `filter` view whose source is
such mutable membership. Removal edits membership structure; it does not destroy the
designated child or grant writable authority.

For positional removal:

- the index expression must statically type as `Int`;
- the index expression is evaluated in current transaction-visible state when the
  removal executes;
- the resulting integer is interpreted as a zero-based current structural position;
- staged selector and membership writes earlier in the same action are visible;
- removal removes exactly the one occurrence currently stored at that position;
- duplicate occurrences of the same designation remain independent structural
  occurrences and may be selected independently by position;
- a negative or out-of-bounds position fails the action at runtime;
- a later positional removal reevaluates its selector against the then-current
  structure.

For designation-directed removal:

- the selector is currently a persistent scalar `state name: live T` designation, not
  an arbitrary designation expression, model member, or general reference;
- the designation model must match the target sequence/view element model;
- the designation is read in current transaction-visible state at the removal
  operation, so staged reselection earlier in the same action changes the target child;
- the current target structure/view is evaluated in the same transaction-visible
  world;
- removal succeeds only when that exact child identity appears **exactly once** in the
  current target;
- zero matching current occurrences fail the action transactionally rather than
  silently becoming a no-op;
- multiple current occurrences designating the same child are ambiguous and fail
  transactionally rather than choosing first/all or inventing stable occurrence
  identity;
- removing membership does not clear, destroy, or retarget the persistent designation;
  it continues to designate the same live child under current lifetime semantics;
- incompatible `live` models are rejected statically.

When either form targets a direct owner-relative filtered view:

- the current transaction-visible view is evaluated at the removal operation;
- the selected view occurrence maps to the exact occurrence in the mutable source
  membership that produced it;
- removal edits that backing source occurrence; it does not make derived view storage
  writable;
- non-retained source occurrences are skipped when mapping a positional selection;
- source order and duplicate multiplicity remain structural facts;
- for designation-directed removal, the child identity must identify exactly one
  current view occurrence before backing-occurrence mapping occurs;
- staged source-membership and predicate-driving writes earlier in the same action
  affect the current view/mapping;
- later filtered removals reevaluate the then-current view and source structure;
- no stable occurrence identity is created by either operation.

All successful removals participate in the surrounding shared action transaction.
Later reads, indexing, `filter`, and `reduce` evaluation in the same action observe the
updated structure. Failure of the removal or a later statement rolls the structural
edit and other staged changes back together.

The two forms preserve an important distinction:

```text
remove sequence[indexExpr]
    -> edit the occurrence at one current structural position

remove designation from sequence-or-filter
    -> edit by child identity only when that identity identifies one unique current
       occurrence in the target
```

Child identity is not structural occurrence identity. If the same child appears twice,
a designation alone cannot say which occurrence is intended, so the designation form
fails rather than silently selecting one. Positional removal remains available when
the application actually means one particular current position.

Designation-directed removal is a narrow structural operation, not public designation
equality, `find`/`indexOf`, remove-all, a mutable-view abstraction, iterator mutation,
child destruction, or a general collection API. Filtered removal remains limited to a
direct identity-preserving owner-relative filter over one mutable membership source.

---

# 18. Owner-relative child `destroy` — provisional/implemented

Elanu accepts one narrow explicit lifetime-ending operation for a committed dynamic leaf child:

```elanu
state selected: maybe live Document = none

action permanentlyDeleteSelected {
    destroy selected in workspace
}
```

`destroy` is distinct from structural `remove`. `remove` edits one membership occurrence;
`destroy` states that the designated rooted child itself must cease to be live.

Current contract:

- the operand must be persistent `maybe live T` state; plain `live T` is rejected because
  successful destruction would violate its exactly-one-live-target invariant;
- the designation must be present when the statement executes;
- the owner operand may be a statically declared modeled-state root or a persistent `live T` /
  present `maybe live T` designation of a live modeled identity;
- a designation used as owner operand carries only exact identity for provenance checking; it
  does not become an owning reference or independently grant lifetime authority;
- the designated target must be an already committed dynamic child created under owner-relative
  creation, not a statically declared modeled-state root or a fresh child from the current
  transaction;
- the resolved owner identity must equal the child's recorded rooting owner; membership in
  another owner's structure or merely designating another live model does not grant lifetime
  authority;
- the child must not currently root another live dynamic child; descendant cascade is not
  selected by this surface;
- no persistent plain `live T` designation may still target the child.

On success, lifetime cleanup is compiler/runtime-owned consequence of the one transition:

- every structural membership occurrence of that exact child identity is removed, including
  duplicates and memberships owned by other modeled-state roots;
- every persistent `maybe live T` designation that names the child becomes absent;
- the child's modeled state and model-local derived state cease to be available;
- the child's recorded rooting relation ends.

The whole operation participates in the current shared action transaction. Later failure rolls
back the lifetime end, membership cleanup, designation cleanup, and other staged action work
together. An absent designation, foreign owner claim, rooted descendant, remaining plain-live
designation, or transaction-local fresh child fails transactionally rather than selecting a
new implicit policy.

The current first surface deliberately accepts only persistent `maybe live T` state as the
child-to-destroy designation. The owner proof may be a static root or persistent live
designation, but destruction does not generalize the target operand to scoped designations,
indexed designation expressions, derived designations, arbitrary expressions, statically
declared roots, or fresh-child cancellation.

This destruction operation does not itself transfer or reparent ownership, and does not introduce cascading destruction,
garbage collection, reference counting, general references/borrowing, or a general resource
management system. The source spelling remains provisional; compiler-private lifetime markers
and carrier representations are not language law.

---

# 19. Owner-relative child lifetime `transfer` — provisional/implemented

Elanu accepts one narrow operation that changes the lifetime/root provenance of an existing
committed dynamic **leaf** child without changing that child's identity:

```elanu
state selected: maybe live Document = none

action moveLifetime {
    transfer selected from leftFolder to rightFolder
}
```

The operation changes only the authoritative lifetime owner. It does **not** insert, remove,
reorder, or otherwise edit structural membership. Applications that need both facts compose
`transfer` with ordinary structural edits inside the same action transaction.

Current contract:

- the target must be persistent scalar `live T` or `maybe live T` designation state;
- `maybe live T` must be present when the statement executes; absence fails the action;
- the target must designate an existing **committed dynamic leaf child**; fresh transaction-local
  children, statically declared modeled roots, and children that still root another live child are
  not accepted by this first surface;
- the `from` owner must resolve to the child's exact current recorded root-provenance owner;
- the `to` owner must resolve to an existing live modeled-state identity and must not be the child
  itself;
- owner operands may be static modeled roots or persistent live designations, following the same
  exact-identity owner-proof family used by owner-relative lifetime operations;
- same-owner transfer is a validated no-op;
- the exact child identity is unchanged, so persistent designations targeting it remain targeted
  and already-granted exact writable authority continues to address the same state identities;
- all structural memberships, including cross-owner and duplicate memberships, are unchanged;
- a staged transfer is visible to later lifetime operations in the same action: the destination
  owner immediately proves root provenance and the old owner no longer does;
- successful transfer commits with the surrounding action; later failure rolls the provenance
  change back with all other staged state/structural changes.

`transfer` is deliberately separate from structural `move`, `insert`, and `remove`. Structural
operations do not implicitly reparent lifetime, and lifetime transfer does not infer or perform a
structural move. This preserves the established distinction between child identity, membership,
position, designation, lifetime/root provenance, and writable authority.

Elanu currently declares no parent/child model compatibility relation beyond "this is a live
modeled owner identity." Existing `create T in owner` establishes lifetime provenance independently
of which `[live T]` members the owner model may contain, and `transfer` preserves that law rather
than treating membership shape as ownership typing.

This first surface does not introduce subtree transfer, cascading destruction, general ownership
graphs, ownership-bearing `live T`, borrowing/move semantics for ordinary values, arbitrary
designation expressions, or implicit reparenting. `transfer`, `from`, and `to` are contextual in
this bounded statement surface; compiler-private transfer helpers and transaction overlays are not
language law.

---

# 20. Read-only `reduce` — v0.8.0 provisional/implemented

The current reduction surface is:

```elanu
reduce source from initial as (accumulator, element) {
    step
}
```

Representative model-local aggregate:

```elanu
derived total =
    reduce lines from 0.0 as (sum, line) {
        sum + line.lineTotal
    }
```

Current semantic contract:

- the source is the supported owner-relative `[live T]` sequence/view surface;
- traversal is read-only;
- `element` designates the current live element;
- the initial expression establishes the accumulator type and whole reduction result type;
- the step expression must be assignable back to that accumulator type;
- `Int -> Float` widening is allowed when the accumulator type is `Float`;
- `Float -> Int` is not allowed when the initial accumulator establishes `Int`;
- empty sequence still statically validates the step and returns the initial value;
- dependencies include the source sequence/view plus exact owner/child facts actually read during evaluation;
- order and duplicate designation multiplicity are preserved.

The initial-value-anchored accumulator/result type law is established semantic behavior.

The current reduction parser/transport still contains bootstrap architecture that is not source-language law.

---

# 21. Derived `filter` and ordered views — provisional/implemented

The base derived-filter surface is:

```elanu
derived activeLines =
    filter lines as line {
        line.quantity > 0
    }
```

A filter may additionally derive display/traversal order from one current child `Int`
member:

```elanu
derived visibleTickets =
    filter tickets as ticket {
        ticket.open
    } order by ticket.priority descending
```

The accepted narrow grammar is:

```text
filter <sequence-source> as <element-binding> { <predicate-expression> }
filter <sequence-source> as <element-binding> { <predicate-expression> } order by <element>.<Int-member> ascending|descending
```

In the current grammar, `order` begins immediately after the filter closing `}`. A newline
after an unordered filter remains the ordinary declaration/statement separator.

Current filter contract:

- source type is the supported `[live T]` model-local sequence/view surface;
- the element binding represents the current `live T` designation for read-only predicate access;
- the predicate must statically have type `Bool`;
- the result has type `[live T]`;
- retained designations preserve identity and duplicate multiplicity;
- without `order by`, retained occurrences preserve source order exactly;
- filter creates no writable authority and no child-lifetime ownership;
- dynamic dependencies follow the source sequence and exact owner/child facts actually read by the predicate;
- membership changes replace obsolete dynamic child dependencies;
- transaction-local staged membership/predicate changes are visible;
- rollback discards temporary filter/dependency state;
- downstream `reduce` may consume the derived view at runtime size;
- runtime `Int` indexing may select one current occurrence from the view;
- a selected child may expose stored or model-local derived members for reading;
- direct `through` may mutate a selected stored child member;
- `state through` may grant exact writable authority to a selected stored child member.

For the current `order by` extension specifically:

- the ordering key is exactly one member read through the filter element binding;
- that member must have static type `Int`;
- direction is explicit: `ascending` or `descending`;
- membership is still determined only by the filter predicate before ordering;
- retained occurrences are ordered by the current child key value whenever the derived view evaluates;
- equal keys preserve the source sequence's relative occurrence order;
- ordering preserves child identity and duplicate multiplicity;
- ordering-key reads participate in the derived view's ordinary dynamic dependency tracking;
- staged key changes are visible to transaction-local evaluation and failed actions publish neither the key change nor a temporary reordered view;
- derived ordering never mutates the backing `[live T]` membership or turns derived order into stored structural order;
- an ordered derived view is not accepted as the selector for structural `remove` or `move`; mutating backing order from automatically derived order would require a separate semantic decision;
- ordinary indexed child read, explicit `through`, and `state through` retain their existing exact-child semantics when consuming the current derived view.

Stored membership order and derived view order are therefore distinct application facts. A
priority change may reorder an ordered view without producing a structural edit to the
backing membership.

The derived view itself remains read-only structural computation. Mutation and writable
authority apply to exact selected child state where already supported, not to a view
position or to the derived structural value itself.

This surface does not introduce arbitrary ordering expressions, comparator functions,
multi-key ordering, String/Float collation rules, stable occurrence identity, a general
`sort` operation, comprehensions, `map`, higher-order collection methods, or a query
language. Those mechanisms remain unselected.

---

# 22. Filter → reduction composition — v0.8.0

Representative end-to-end program shape:

```elanu
state model LineItem {
    state quantity = 1
    state unitPrice = 10.0
    derived lineTotal = quantity * unitPrice
}

state model Invoice {
    state minimumQuantity = 1
    state lines: [live LineItem] = []

    derived activeLines =
        filter lines as line {
            line.quantity >= minimumQuantity
        }

    derived activeTotal =
        reduce activeLines from 0.0 as (total, line) {
            total + line.lineTotal
        }
}
```

Static relationship:

```text
lines                   : [live LineItem]
line                    : live LineItem
activeLines             : [live LineItem]
activeTotal             : Float
```

`activeLines` and `lines` have the same sequence value type but different storage semantics:

```text
state lines
    mutable structural application state

derived activeLines
    read-only cached computation producing a structural value
```

That distinction is part of the current semantic model. Both may participate in
positional selection where documented. Direct mutation and writable authority through
a filtered selection target the exact selected child member; they do not make the
derived view itself mutable merely because its value type matches stored membership.

---

# 23. What is not currently language surface

The current compiler/language should **not** be read as already containing any of the following:

```text
ordinary functions
local variable declarations (`let` / `var`)
action return values
general references or pointers
constructors
general collection methods
general append / insert / remove / move collection API
comprehensions
map / sort / group / distinct
general query syntax
maps / sets
general ownership or borrowing
try / catch / exceptions
savepoints
async / await
threads / actors / concurrency isolation
network/filesystem/device effects
clock/time semantics
modules/imports/packages as settled language law
macros
traits/interfaces
classes/inheritance
pattern matching
user-defined operators
generics as a general type-system feature
nullable designations
language-level UI syntax
```

Some of these areas have design pressure or provisional experiments. They are not accepted merely because related source documents discuss them.

Owner-relative creation, structural insertion/removal, owner-relative committed-leaf child `destroy`, and runtime-sized owner-relative indexed read/direct `through`/`state through` authority forwarding are accepted as documented above. Sequence indexing is zero-based. Runtime `Int` index expressions such as `lines[selectedIndex]` are accepted for owner-relative indexed read/direct `through`/`state through` operations with transaction-visible evaluation, runtime bounds failure, and exact grant-time authority. Indexed reads, direct `through` mutation, and `state through` writable-authority forwarding also compose with owner-relative derived `filter` views.

---

# 24. Implementation boundaries are not language law

The current compiler still contains bootstrap mechanisms including preprocessing, generated hidden bindings, specialized lowering passes, compiler-private builtin transport, and older String transports for some structural surfaces.

Unless this reference explicitly states otherwise, those mechanisms are implementation details.

A Elanu programmer should reason from the source semantics described here, not from names such as generated `__meld_*` bindings, private builtin names, or current compiler pass order.

---

# 24. Validation

The normal Rust validation sequence from `compiler/` is:

```powershell
cargo fmt
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build
```

The Python semantic-oracle validation from the repository root is:

```powershell
.\.venv\Scripts\Activate.ps1
py -m pytest -v
```

GitHub CI currently checks formatting, Clippy, Rust tests/build, and the Python semantic reference suite. CI does not substitute for the mutating local `cargo fmt` step in the normal full validation sequence.