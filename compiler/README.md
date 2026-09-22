# Elanu compiler

This directory contains the Rust implementation of the Elanu compiler frontend and bootstrap execution runtime.

The compiler currently provides lexical analysis, parsing, semantic checking, Elanu-specific lowering and validation, and execution of the current compiler-accepted language surface through a bootstrap runtime.

The implementation is intentionally subordinate to Elanu semantics: compiler-private generated names, carriers, lowering nodes, side tables, and pass structure are implementation representation rather than source-language law.

## Build and test

From this directory:

```powershell
cargo fmt
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build
```

From the repository root, a built compiler can be used against the examples:

```powershell
.\compiler\target\debug\elanu.exe check .\examples\comparisons.elnu
```

The current runtime is a bootstrap interpreter intended to reproduce Elanu semantics. Native code generation is not currently part of the implementation.

## Language and development documentation

See:

- [`../docs/LANGUAGE_REFERENCE.md`](../docs/LANGUAGE_REFERENCE.md) for the current compiler-accepted source surface;
- [`../docs/DESIGN_DECISIONS.md`](../docs/DESIGN_DECISIONS.md) for durable semantic laws and rationale;
- [`../docs/DEVELOPMENT.md`](../docs/DEVELOPMENT.md) for validation and compiler-development discipline.

Implementation changes should preserve established semantics unless a semantic change is explicitly part of the work.