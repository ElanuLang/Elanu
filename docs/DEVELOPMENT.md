# Elanu development

This document describes the public development and validation workflow for Elanu.

The repository is the implementation authority for the compiler-accepted language. Current source behavior is documented in `LANGUAGE_REFERENCE.md`; durable semantic laws and rationale are documented in `DESIGN_DECISIONS.md`.

## Development principle

Elanu is developed from application composition pressure rather than from a conventional language-feature checklist.

The project thesis is:

> The compiler should understand application intent that today's languages force frameworks and programmers to reconstruct.

A proposed language feature should therefore answer a concrete application-level need and should provide a meaningful advantage over a credible library or framework implementation.

Do not infer that Elanu needs a mechanism merely because another language has it.

## Semantic status

Keep four categories distinct when working on the project:

### Established Elanu semantics

Intentional current language behavior supported by the language reference, design decisions, and semantic tests.

### Bootstrap implementation behavior

What the current compiler or reference model happens to do internally. Generated names, lowering carriers, pass organization, and private encodings are not language law unless explicitly documented as such.

### Provisional design direction

A design direction supported by evidence but not yet established as durable language semantics.

### Speculative future idea

A possibility that has not yet earned implementation or language commitment.

Implementation convenience must not silently promote bootstrap behavior into established semantics.

## Semantic preservation discipline

Before a compiler or runtime change that could affect observable behavior, identify:

```text
intended semantic delta
    the source-visible behavior allowed to change

must remain invariant
    established behavior outside that delta
```

Then identify the authoritative contract for the affected behavior. Depending on the change, this may include:

- `LANGUAGE_REFERENCE.md`;
- a relevant entry in `DESIGN_DECISIONS.md`;
- Rust source-level semantic regression tests;
- the Python semantic reference for foundational laws or selected experiments.

An implementation problem does not by itself authorize changing the semantic contract.

If the implementation disagrees with an established expectation, determine whether the implementation is wrong or whether the semantic decision itself genuinely needs revision. Do not weaken a test merely to accommodate a convenient representation.

## Semantic tests and representation tests

Keep two test roles conceptually distinct:

### Semantic/contract tests

Specify observable Elanu behavior and protect established language expectations.

### Implementation/representation tests

Verify that a compiler-private structure or lowering step preserves the semantic fact it is intended to carry.

A representation test must not turn an implementation encoding into language law. Conversely, semantic tests should not be weakened because an internal representation makes them inconvenient.

## Representation problems

When compiler work exposes a representation problem:

1. identify the exact semantic fact being lost, reconstructed, encoded, or reparsed;
2. inspect only the relevant compiler path;
3. introduce the smallest structured representation that preserves that fact;
4. validate existing behavior;
5. return to the original semantic pressure.

Do not use a local representation problem as automatic justification for a compiler rewrite or a general IR hierarchy.

## Runtime persistence host boundary

The public runtime exposes a provider-agnostic persistence boundary in `runtime::persistence`.
This is implementation infrastructure, not Elanu source syntax.

`PersistenceProvider` has two synchronous operations:

```text
load() -> zero or one opaque PersistenceImage
replace(candidate) -> accept the complete replacement or fail without changing the prior image
```

`PersistentRuntime` restores a loaded image only when its runtime-owned persisted reconstruction
shape is compatible with the current checked application. Top-level actions execute against the
ordinary Elanu transaction machinery, produce a candidate committed world, ask the provider to
accept the candidate image, and publish the candidate runtime only after acceptance. Semantic
action failure does not call the provider; provider failure leaves the prior runtime world in
place.

`PersistenceImage` is intentionally opaque to providers. Private generated runtime names may
occur inside the runtime-owned encoding, but providers must not parse them or treat them as host
semantics. Persistence compatibility is based on stored reconstruction topology rather than
source text or whole-program identity, so formatting and action-body-only changes do not by
themselves invalidate an image.

This boundary does not select a disk/database format, source `save`/`load`, schema migration,
async durability, crash recovery, retries, or general external-effect semantics.

`PersistenceImage::encode` and `PersistenceImage::decode` provide the runtime-owned opaque byte
boundary for process-to-process storage. The runtime format carries its own encoding version and
validates malformed or unsupported payloads before application-shape compatibility and restore.
Providers may store these bytes but must not interpret the private payload. The byte format remains
runtime implementation compatibility rather than Elanu source semantics or a provider schema.


For large partially resident worlds, `runtime::persistence::partial` provides a separate production
host boundary without changing the existing monolithic `PersistentRuntime`. `PartialPersistentRuntime`
backs each dynamic modeled identity with a runtime-owned opaque key when that identity first becomes
durable. After process restart, backed identities retain exact model/lifetime identity metadata while
their member cells remain dormant until the host explicitly calls `materialize` with one opaque key.

`PartialPersistenceProvider` stores an opaque manifest plus opaque key/payload pairs and exposes one
atomic `replace_candidate(manifest, changed_backing)` operation. Resident-only actions may publish a
new manifest without reading or rewriting dormant backing. If one explicitly materialized identity
changes, only that identity's opaque payload plus the candidate manifest need be replaced; unrelated
backing remains untouched. Provider acceptance still precedes authoritative runtime publication.

`PartialPersistentRuntime::materialize_designation` lets host integration request materialization
through a top-level source `live T` / `maybe live T` designation name. `CheckedSource` preserves the
source-designation-to-lowered-binding relationship structurally, so the runtime resolves the exact
current target and its private backing handle without parsing generated names or exposing dynamic
identity/backing keys to the host. This remains explicit host-driven materialization, not automatic
loading on member access.

Backing keys, residency, manifest layout, and materialization are runtime/host infrastructure rather
than Elanu source concepts. The boundary does not select automatic loading, eviction/cache policy,
backing garbage collection, content addressing, a database/ORM/query API, or a concrete storage
backend. Providers must treat manifest, keys, and backing payloads as uninterpreted runtime-owned
bytes and must not reconstruct model-member or generated-name semantics from them.

## Rust validation

Normal local validation from `compiler/` is:

```powershell
cargo fmt
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build
```

`cargo fmt` is intentionally included before `cargo fmt --check`.

GitHub CI uses the non-mutating formatting check:

```text
cargo fmt --check
```

A successful GitHub Actions run should therefore not be described as having run the mutating local `cargo fmt` step.

## Python semantic-reference validation

From the repository root:

```powershell
py -m pytest -v
```

The Python reference model is an executable oracle for foundational semantic laws and selected experiments. It is not the implementation architecture of the Rust compiler and does not necessarily implement every current Elanu feature.

## Semantic-diff review

After normal validation, perform a short semantic-diff review:

1. What observable behavior changed besides the intended semantic delta?
2. Did any existing semantic expectation change?
3. If so, was that change explicitly justified?
4. Did an implementation convenience accidentally become a language rule?
5. Is any known semantic fact being flattened, reparsed, inferred from generated names, or reconstructed downstream instead of being preserved structurally?
6. Do the canonical documentation and tests still describe the same language?

Compiler acceptance and green tests are necessary evidence, but they do not by themselves prove that the intended semantics were preserved.

## Documentation responsibilities

When source-visible behavior changes:

- update `LANGUAGE_REFERENCE.md`.

When a durable semantic rule or rationale changes:

- update `DESIGN_DECISIONS.md`.

When a released or significant compiler-accepted milestone changes:

- update `CHANGELOG.md` as appropriate.

Do not create additional competing current specifications for the same language behavior.

Historical experiments and implementation investigations may provide evidence, but accepted semantics belong in the canonical documents above.

## Git safety

Never discard unknown local work.

Before operations that could affect the working tree or branch state, inspect it with commands such as:

```powershell
git status
git diff
git diff --cached
```

Avoid destructive commands such as `git reset --hard`, `git clean -fd`, forced checkout, or forced branch updates unless the affected work is explicitly known to be disposable.

When synchronizing a clean `main` branch, prefer a fast-forward-only pull:

```powershell
git fetch origin
git switch main
git pull --ff-only origin main
```

## Pull requests and CI

Keep a pull request focused on one coherent concern where practical.

Before merge:

1. inspect the complete diff;
2. run the full applicable local validation;
3. perform the semantic-diff review;
4. push the exact validated candidate;
5. verify CI for the exact PR head;
6. resolve failures before merging.

Do not rely on a successful workflow run from an earlier commit after the PR head has changed.

## Compiler terminology and internal names

Use precise compiler terminology in code and documentation.

Generated `__meld_*`, `__elanu_*`, carrier names, private runtime nodes, and similar encodings are compiler representation. They should not appear in user-facing semantics except when a test or implementation discussion is specifically about lowering or leakage.

Prefer preserving semantic facts structurally rather than reconstructing them later from naming conventions.

## Scope discipline

Do not automatically add:

- general references or pointers;
- ownership/borrowing systems;
- general collection/query APIs;
- `let` / `var` or general mutable locals;
- ordinary functions or action returns;
- constructors;
- generalized nullable/option types;
- exceptions/savepoints;
- async/concurrency;
- a general compiler IR hierarchy;
- a compiler rewrite.

If a broader mechanism becomes necessary, identify the concrete application or compiler pressure that earned it and implement the smallest coherent mechanism that addresses that pressure.