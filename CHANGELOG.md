# Changelog

## Unreleased

### Language and semantics

- Creation-scoped fresh child designations may be assigned directly to compatible persistent
  `live T` / `maybe live T` state, preserving the exact fresh identity without structural or
  business-key reconstruction.
- Fresh-designation persistence remains contextual: it grants no authority or ownership,
  changes no rooting or membership, rolls back transactionally, and does not expose the private
  identity carrier as an ordinary value.
- Scoped owner-relative creation may root a fresh dynamic child in another exact live modeled
  child identity carried by an enclosing fresh scoped designation or persistent `live T` /
  present `maybe live T` state.
- `destroy target in owner` may prove dynamic root provenance through persistent live owner
  designation state while still requiring exact equality with the child's recorded rooting owner.
- Dynamic owner designations remain non-owning identity carriers; descendant presence still
  blocks parent destruction and no cascade, reparenting, or ownership-transfer semantics were added.
- Added narrow child-fact-derived ordering to model-local `filter` views with explicit
  `ascending` / `descending` direction and one child `Int` key.
- Equal ordering keys preserve source order; derived ordering preserves child identity and
  duplicate multiplicity without mutating backing membership.
- Ordered derived views remain read-only and are not structural `remove` / `move` selectors.
- Preserved plain-filter source-order behavior, dynamic dependency tracking, and transactional
  rollback while adding ordering-key dependencies.

### Compiler/runtime

- Carries the ordering key and direction as structured `Expr::Filter` facts through compiler
  rewrites rather than reconstructing or encoding them in generated names/text.
- Added regression coverage for reactive reordering, stable ties, backing-order preservation,
  ordering-key validation, ordered-view structural-edit rejection, rollback, and plain-filter
  declaration separators.

## v0.9.0 — identity-preserving application composition

Elanu v0.9.0 promotes the post-v0.8.0 workstation composition work into the current language baseline and completes the pre-1.0 rename from Meld to Elanu.

### Language and semantics

- Renamed the language to **Elanu**, the compiler/package/binary to `elanu`, and adopted `.elnu` as the canonical source-file extension convention without changing established semantics.
- Added owner-relative dynamic modeled-state child creation and scoped creation/insertion while preserving transaction-local fresh identity.
- Added runtime-sized indexed designation/member access and exact indexed writable-authority grants.
- Added persistent `maybe live T` designation state with contextual `none` and read-only `is present`.
- Added child-identity membership testing with `designation is in structure`.
- Added exact and Unicode-default-full-case-folded String containment plus short-circuit `not`, `and`, and `or` composition.
- Added designation-directed structural removal from stored membership and direct filtered views.
- Added unique-current-occurrence relative navigation with `next` / `previous` over stored membership and direct identity-preserving filtered views.
- Added scoped immutable designation binding for carrying one exact child identity across statements without persistent scratch state or general locals.
- Added child-directed structural movement with `move moving before|after anchor in structure`, including filtered-view-to-backing movement and transactional rollback.

### Semantic boundaries preserved

- Child identity, structural membership, structural occurrence, numeric position, designation, child lifetime, and writable authority remain distinct facts.
- Numeric occurrence locations used for navigation/removal/movement are transient runtime implementation details, not persistent language-visible identity.
- Duplicate occurrences remain ordinary multiplicity; operations that require one occurrence fail on ambiguity rather than inventing stable occurrence handles.
- No general collection API, `find`/`indexOf`, cursors/iterators, general locals, references/pointers, ownership/borrowing, action returns, exceptions/savepoints, or async/concurrency model was added.

### Compiler/runtime

- Reused structured designation, sequence, filter, and transaction machinery for the new identity-preserving operations rather than introducing a general IR or parallel authority system.
- Kept compiler-generated structural helper actions and private `__meld_*` carriers as bootstrap representation rather than language law.
- Expanded regression coverage for transaction-visible filtered views, designation reselection, duplicate/absent occurrence failures, exact authority, and rollback across the full workstation composition path.

### Current frontier

The previously identified workstation selection/removal/reorder blockers now compose using compiler-accepted source behavior. Development returns to realistic composition pressure to identify the next application fact that still has to be reconstructed outside the compiler rather than selecting another conventional language feature by default.

## v0.8.0 — live modeled-state composition

Meld v0.8.0 promotes the post-v0.7.1 composition work into the current language baseline.

### Language and semantics

- Added `state model` reusable modeled-state shapes with distinct live constituent identities.
- Added `live T` non-owning designations.
- Added explicit indirect access and writable authority through `through` and `state through`.
- Added ordered structural sequences, including `[live T]`.
- Added state-model sequence members for owner-relative structural membership.
- Added sequence indexing.
- Added read-only owner-relative `reduce`.
- Established initial-value-anchored reduction accumulator/result typing.
- Added model-local derived `filter`.
- Established filter → reduction composition over runtime-sized structural views.
- Clarified lazy derived invalidation and same-value stabilization as a permitted runtime optimization rather than new language-visible state.

### Compiler/runtime

- Removed representative-target reduction validation scaling.
- Added direct typed runtime-reduction payloads.
- Consolidated shared model/member type analysis and compiler topology facts.
- Added typed runtime model-template metadata and carried it into runtime construction in preparation for dynamic child creation.

### Current frontier

Owner-relative dynamic child creation remains the next composition pressure and is **not** part of v0.8.0.

## v0.7.1 — stabilization and reconciliation

- Reject unknown source type names in state annotations and action parameter declarations instead of treating them as usable named runtime types.
- Preserve checker ownership of resolved action-parameter types in `CheckedProgram`; the runtime no longer re-parses action parameter type names from source AST strings.
- Keep invalid unknown types inside checker error recovery only; they cannot escape in a successful checked program.
- Extend AST / `meld parse` display to include action parameters, value arguments, and explicit writable-state grants.
- Reconcile the current language reference, project/compiler READMEs, roadmap, composition plan, changelog, and development workflow with the released v0.7 state.
- Deliberately add no new Meld syntax or semantic domain. The next semantic milestone remains dependent on composition evidence.

Validation: the full Windows/MSVC Rust validation contract and Python semantic-oracle suite passed locally. GitHub Actions passed on the v0.7.1 implementation pull request and again on its post-merge `master` commit. Release closure additionally requires the release-status documentation merge to pass post-merge CI and the annotated `v0.7.1` tag to be verified against that release commit.

## v0.7.0 — parameterized actions and explicit writable state authority

- Added typed immutable value parameters to actions.
- Added typed writable state parameters using `state name: Type`.
- Added explicit writable-authority grants at action call sites using `state name`.
- Preserved the existing rule that ordinary state reads produce values rather than implicit writable aliases.
- Added explicit forwarding of writable state authority through nested action calls.
- Kept writable state typing invariant/exact while preserving existing value-argument assignment compatibility such as `Int -> Float`.
- Added action-call arity, argument-kind, and parameter-type checking.
- Added action-local parameter scope while preserving top-level lexical resolution for `derived` declarations.
- Added call-local runtime frames for value parameters and writable state authority.
- Value arguments are evaluated in the caller's current transaction-local world, so they observe earlier staged writes.
- Writes through writable state parameters target the existing underlying state identity and participate in the caller's shared transaction.
- Failure through parameterized nested actions preserves existing rollback semantics.
- Promoted writable authority from the Python experiment into the authoritative semantic oracle.
- Added `docs/ACTION_PARAMETERS_0.7.md` documenting the decision, rationale, alternatives, boundaries, and revisit triggers.
- Reconciled the language reference, design decisions, roadmap, README, and changelog around the v0.7 authority model.
- Deliberately did not introduce general references, pointers, borrowing/lifetimes, functions, locals, collections, return values, async actions, independent nested transactions, or recovery/savepoints.

Version 0.7 is the first language milestone selected directly by the post-v0.6.1 workstation composition process. The original law that state reads yield values survived its first direct composition challenge; reusable mutation is expressed through separate explicit authority instead.

Validation: the full Windows/MSVC Rust validation contract and Python semantic-oracle suite passed before the v0.7 pull request. GitHub Actions passed on the pull request and again on the post-merge `master` commit. The annotated `v0.7.0` tag was verified to reference the merged release commit.

## v0.6.1 — stabilization and reconciliation

- Preserved semantic type information in `CheckedProgram` and made the runtime use checker-established binding types instead of reconstructing types from whichever initializer branch executed.
- Fixed mixed `Int` / `Float` conditional initialization so statically unified `Float` bindings remain `Float` at runtime.
- Fixed runtime snapshot ordering so interleaved `state` and `derived` declarations remain in source order.
- Added declaration/statement source locations so semantic diagnostics no longer collapse to `0:0`.
- Ported currently expressible semantic-kernel parity cases from the Python reference model into Rust runtime tests, including transaction-local derived invalidation, rollback of temporary derived state, failed-derived retry, transitive chains, diamond recomputation, and actual-read conditional dependencies.
- Audited numeric behavior without changing unrelated arithmetic semantics; established behavior is now distinguished from provisional bootstrap choices in `docs/NUMERIC_SEMANTICS.md`.
- Added `docs/LANGUAGE_REFERENCE.md` as the current accepted language-surface reference.
- Reconciled README/example wording, including removal of an example that visually implied unsupported string interpolation.
- Removed tracked Python build/cache artifacts and expanded `.gitignore` to keep generated artifacts out of the repository.
- Added minimal GitHub Actions CI for Rust formatting, Clippy, tests, build, and Python semantic-reference tests.
- Deliberately added no new Meld syntax; post-v0.6.1 work returns to composition experiments rather than a conventional feature treadmill.

Validation: the full Windows/MSVC validation contract passed locally (`cargo fmt`, `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo build`, and `python -m pytest -v`). The new GitHub Actions CI also passed both Rust and Python jobs on the v0.6.1 pull request.

## v0.6.0 — comparison and equality expressions

- Added `==`, `!=`, `<`, `<=`, `>`, and `>=` expressions.
- Equality/inequality support numeric values, `Bool`, and `String`.
- Ordering supports numeric values only in this milestone; string and Bool ordering remain undefined.
- Mixed `Int` / `Float` comparisons preserve numeric correctness across the full `i64` range rather than blindly converting integers through `f64`.
- Comparison/equality operators produce `Bool` and bind less tightly than arithmetic.
- Kept ordinary program equality distinct from Meld's state invalidation equivalence: ordinary NaN equality follows IEEE-style behavior, while repeated NaN state values remain semantically equivalent for invalidation purposes.
- Added runtime edge-case tests for NaN, signed zero, and large mixed numeric values above exact `f64` integer precision.
- Added `docs/COMPARISONS_0.6.md` with rationale, alternatives, tradeoffs, boundaries, and revisit triggers.
- Added `examples/comparisons.meld`.
- Deliberately deferred logical operators, structural equality, string ordering, user-defined comparison, sorting protocols, locals, and broader control flow.

Validation: `cargo fmt`, `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo build` passed on the Windows/MSVC toolchain for the feature implementation. The Python semantic reference implementation is unchanged.

## v0.5.0 — conditional action control flow

- Added statement-level `if` control flow inside actions.
- Statement `if` requires a `Bool` condition and may omit `else`.
- Expression `if` remains value-producing and therefore still requires both branches.
- Conditional action reads observe transaction-local staged state, consistent with the existing action read model.
- Only the selected branch executes; unselected branches do not mutate state, call actions, or fail.
- Branch execution remains inside the existing action transaction; `if` does not create a new transaction or commit boundary.
- Added recursive semantic checking for statements inside conditional branches.
- Added `docs/CONDITIONAL_ACTIONS_0.5.md` with rationale, alternatives, tradeoffs, boundaries, and revisit triggers.
- Added `examples/conditional_action.meld`.
- Deliberately deferred comparison/equality/logical operators, loops, local bindings, `else if` shorthand, pattern matching, and early-return semantics.

Validation: `cargo fmt`, `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` passed on the Windows/MSVC toolchain for the feature implementation: 15 frontend tests and 16 runtime tests passed. The Python semantic reference implementation is unchanged.

## v0.4.0 — explicit action failure

- Added `fail <String expression>` as a Meld statement inside actions.
- `fail` intentionally aborts the current action instead of requiring an incidental runtime error to trigger rollback.
- A top-level failed action discards all staged writes.
- A failure inside a nested action propagates through the shared transaction and rolls back the whole outer action.
- The semantic checker rejects non-String failure expressions.
- Chose `fail` rather than `throw` so this milestone does not imply an exception hierarchy, arbitrary thrown values, catch matching, or other recovery semantics that Meld has not designed yet.
- Added `docs/EXPLICIT_FAILURE_0.4.md` with the decision rationale, alternatives, tradeoffs, boundaries, and revisit triggers.
- Added `examples/explicit_failure.meld`.
- Kept source-level catching/recovery, typed errors, result types, action return values, async failure propagation, cancellation, and external-effect compensation deferred.

Validation: `cargo fmt`, `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` passed on the Windows/MSVC toolchain for the feature implementation: 12 frontend tests and 13 runtime tests passed. The Python semantic reference implementation is unchanged.

## v0.3.0 — source-level action invocation

- Added zero-argument action invocation as a Meld statement, e.g. `inner()` inside another action.
- Nested action calls join the caller's existing transaction rather than committing independently.
- Nested actions therefore read and write the same staged state and transaction-local derived world as the outer action.
- Runtime failure inside a nested action aborts the outer action and rolls back all staged writes.
- Added forward references between actions so callable behavior does not depend on declaration order.
- Added semantic checking that action calls resolve to actions rather than state or derived values.
- Added bootstrap runtime detection for recursive action-call cycles instead of allowing an uncontrolled Rust stack overflow.
- Added `examples/nested_actions.meld` and `docs/ACTION_INVOCATION_0.3.md`.
- Kept action calls statement-only and zero-argument for this milestone; ordinary functions, parameters, return values, call expressions, and an explicit Meld failure construct remain deferred.

Validation: `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` were verified on the Windows/MSVC toolchain for the feature implementation: 10 frontend tests and 11 runtime tests passed. The Python semantic reference remains unchanged.

## v0.2.0 — first execution runtime

- Added the first Rust execution layer as an interpreter over the checked AST.
- Added runtime values for the primitive bootstrap types.
- Added lazy `derived` evaluation with committed dependency tracking and caching.
- Added transactional action execution with staged writes, commit, and rollback.
- Added transaction-local derived caching/dependencies so derived reads inside an action observe staged state.
- Added semantic-equivalence suppression for no-op writes in the Rust runtime, including the existing NaN/signed-zero float rule internally.
- Added conditional dependency replacement and committed invalidation propagation.
- Added runtime cycle detection and checked integer arithmetic failures.
- Added deterministic runtime snapshots in declaration order.
- Added `meld run <file.meld> [action]` as a temporary CLI execution harness without adding entry-point/action-call syntax to the language.
- Added Rust runtime tests for counter execution, staged derived reads, no-op writes, conditional dependencies, rollback, unknown actions, and snapshot ordering.
- Tightened compound-assignment type checking so, for example, `Int += Float` is rejected before execution instead of becoming a runtime type failure.
- Added `docs/RUNTIME_0.2.md` explaining the bootstrap runtime boundary and current limitations.
- Deferred an explicit IR until execution teaches us what information that IR actually needs to carry.

Validation: the Rust runtime bootstrap passed `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and `cargo build` on the Windows/MSVC toolchain. The Python semantic suite passed all 23 tests.

## v0.1.0 — first compiler frontend

- Added the first explicit Meld bootstrap grammar in `docs/GRAMMAR_0.1.md`.
- Added a dependency-free Rust compiler crate under `compiler/`.
- Added lexer support for the first Meld keywords, literals, operators, comments, newlines, and blocks.
- Added an AST for `state`, `derived`, `action`, assignment, arithmetic expressions, and `if` expressions.
- Added basic semantic checking for names, primitive types, assignment targets, and immutable `derived` values.
- Added a `meld parse` command to print the parsed AST.
- Added a `meld check` command to perform parse + semantic validation.
- Added Rust frontend tests for the canonical counter, basic type mismatch, illegal derived assignment, and conditional derived syntax.
- Added `compiler/target/` to `.gitignore`.
- Corrected the Python reference package version metadata to `0.0.4`.

Validation: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `cargo build`, `meld parse`, and `meld check` were verified on the Windows/MSVC toolchain before tagging v0.1.0.

## v0.0.4 — semantic state equivalence

- Defined a distinct semantic-equivalence rule for state-change detection.
- NaN is equivalent to NaN for invalidation purposes.
- `+0.0` and `-0.0` are equivalent for invalidation purposes.
- Structural value comparisons apply semantic equivalence recursively.
- Closed the final intentional xfail in the initial semantic kernel.

## v0.0.3 — transaction-local derived semantics

- `derived` reads inside actions observe staged writes.
- Transaction-local derived values use a private cache/dependency graph.
- Staged writes invalidate affected temporary derived values.
- Temporary derived state is discarded on rollback/end of the action.

## v0.0.2 — adversarial semantic pass

- Added stress tests for conditional dependencies, diamond dependencies, no-op writes, rollback, nested actions, failures, mutable collection aliasing, conditional cycles, and staged reads.
- Closed the mutable-container alias hole in the Python reference model by enforcing value semantics at the state boundary.

## v0.0.1 — semantic seed

- Introduced executable reference semantics for `state`, `derived`, and `action`.
- Established the initial test suite and canonical examples.
