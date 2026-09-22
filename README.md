# Elanu

Elanu is an experimental programming language exploring what changes when the compiler directly understands application state, derived dependencies, identity, structural relationships, and mutation authority.

> **Write the application, not the framework.**

Elanu is being developed from semantics outward rather than by filling in a conventional language-feature checklist.

## Status

Elanu is pre-1.0 and under active development.

The current tagged language baseline is **v0.9.0**. The repository also contains compiler-accepted work developed after that baseline.

The current implementation includes:

- mutable `state`;
- dynamically tracked read-only `derived` values;
- atomic `action` transitions with rollback on failure;
- reusable `state model` definitions;
- explicit writable state authority;
- persistent `live T` and `maybe live T` designations;
- ordered non-owning `[live T]` structural membership;
- identity-preserving filtering and ordering;
- runtime-sized indexed selection and mutation;
- dynamic modeled-state creation;
- structural insertion, removal, and movement;
- relative designation navigation;
- explicit rooted-child lifetime termination.

These mechanisms deliberately keep several concepts distinct:

```text
modeled-state identity
structural membership
structural occurrence
numeric position
designation
root/lifetime provenance
writable authority
```

Elanu does not treat these as interchangeable forms of a general reference or collection system.

## Project thesis

Modern application frameworks often reconstruct facts that were already present in source intent: which state is authoritative, which values depend on which facts, which modeled identity a selection refers to, which structural relationship is being edited, and which code has authority to mutate state.

Elanu explores whether moving more of those facts into the language and compiler can reduce framework-level reconstruction while preserving explicit semantics.

The working criterion is:

> **The compiler should understand application intent that today's languages force frameworks and programmers to reconstruct.**

That criterion is intended to be falsifiable. A language feature should earn its complexity by providing a meaningful compiler-owned advantage over a credible library or framework implementation.

## Example

```elanu
state balance = 100
state attempts = 2

derived empty = balance == 0

action withdraw {
    if balance < 50 {
        fail "Insufficient funds"
    }

    balance -= 50
}
```

State mutation is transactional. `derived` values track the facts actually read during evaluation, and action failure rolls back the current transition.

Modeled application structure can preserve identity directly:

```elanu
state model Task {
    state title = ""
    state active = true
}

state model Project {
    state tasks: [live Task] = []
    state search = ""

    derived visibleTasks = filter tasks as task {
        task.active and
        (search == "" or task.title contains search ignoring case)
    }
}
```

Each actual `Project` identity has its own declared state, including its own `tasks` structure and `visibleTasks` view. Persistent designations can identify an exact modeled-state instance without turning that designation into ownership or writable authority.

## Repository layout

```text
Elanu/
├── compiler/       Rust compiler, checker, bootstrap runtime, CLI, and tests
├── reference/      Python executable semantic reference model
├── tests/          Python semantic-reference tests
├── examples/       Elanu examples and composition probes
├── docs/
│   ├── LANGUAGE_REFERENCE.md
│   ├── DESIGN_DECISIONS.md
│   └── DEVELOPMENT.md
├── CHANGELOG.md
└── pyproject.toml
```

The two implementations have different roles:

```text
Python reference model
    executable oracle for foundational semantic laws and selected experiments

Rust compiler/runtime
    implementation of the current compiler-accepted language
```

The Python model is not intended to mirror every compiler feature or internal representation.

## Language documentation

- [`docs/LANGUAGE_REFERENCE.md`](docs/LANGUAGE_REFERENCE.md) describes the current compiler-accepted Elanu source surface.
- [`docs/DESIGN_DECISIONS.md`](docs/DESIGN_DECISIONS.md) records durable semantic laws and their rationale.
- [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) describes the public development and validation workflow.
- [`CHANGELOG.md`](CHANGELOG.md) records released milestones and significant post-release development.

Compiler-private generated names, carriers, lowering nodes, and other bootstrap encodings are implementation details unless explicitly documented as language semantics.

## Building and testing

### Rust compiler/runtime

From `compiler/`:

```powershell
cargo fmt
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build
```

`cargo fmt` is intentionally part of the normal local validation sequence before `cargo fmt --check`.

### Python semantic reference

From the repository root, with the project virtual environment activated:

```powershell
py -m pytest -v
```

GitHub Actions runs the non-mutating CI equivalents for Rust and the Python semantic-reference suite.

## Running the compiler

After building:

```powershell
.\compiler\target\debug\elanu.exe check .\examples\comparisons.elnu
```

The current runtime remains a bootstrap interpreter intended to reproduce Elanu semantics. Native code generation is not yet part of the language implementation.

## Scope

Elanu remains intentionally narrow. Features are not added merely because conventional languages have them.

In particular, the project does not assume that it needs general references, ownership/borrowing, general collection APIs, general local variables, action returns, constructors, exceptions/savepoints, async/concurrency, or a broad compiler IR hierarchy.

When broader machinery becomes necessary, it should be justified by concrete composition pressure.

## Contributing

Elanu is still early enough that semantic changes require more than making the compiler accept new syntax.

Before changing source-visible behavior, identify:

1. the intended semantic change;
2. established behavior that must remain invariant;
3. the authoritative language/documentation/test contract;
4. whether the change introduces a new semantic mechanism or merely repairs compiler representation.

See [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md) for the full development discipline.
## License

Elanu is dual-licensed under the MIT License or the Apache License, Version 2.0, at your option.

See [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).