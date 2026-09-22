from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


# Compiler pipeline.
lib = Path("compiler/src/lib.rs")
replace_once(
    lib,
    "mod lifetime_termination_surface;\n",
    "mod lifetime_termination_surface;\nmod lifetime_transfer_surface;\n",
    "module registration",
)
replace_once(
    lib,
    "    let source = lifetime_termination_surface::preprocess(source)?;\n",
    "    let source = lifetime_transfer_surface::preprocess(source)?;\n    let source = lifetime_termination_surface::preprocess(&source)?;\n",
    "preprocess ordering",
)
replace_once(
    lib,
    "    lifetime_termination_surface::validate(&program, &runtime_model_roots)?;\n",
    "    lifetime_transfer_surface::validate(&program, &runtime_model_roots)?;\n    lifetime_termination_surface::validate(&program, &runtime_model_roots)?;\n",
    "transfer validation",
)
replace_once(
    lib,
    "    let lifetime_owner_prepared =\n        lifetime_termination_surface::lower_owners(&program, &runtime_model_roots);\n",
    "    let lifetime_transfer_prepared =\n        lifetime_transfer_surface::lower_owners(&program, &runtime_model_roots);\n    let lifetime_owner_prepared = lifetime_termination_surface::lower_owners(\n        &lifetime_transfer_prepared,\n        &runtime_model_roots,\n    );\n",
    "owner lowering",
)

# Live designation lowering: transfer operands use the same structured persistent-designation path
# as destroy/scoped structural helpers.
live = Path("compiler/src/live_designation_lowering.rs")
replace_once(
    live,
    "                if (name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION\n",
    "                if (name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION\n                    || name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION\n",
    "transfer designation lowering",
)

# Runtime production entry.
runtime = Path("compiler/src/runtime.rs")
replace_once(
    runtime,
    "    #[cfg(test)]\n    fn transfer_runtime_model_owner(\n",
    "    fn transfer_runtime_model_owner(\n",
    "promote transfer entry",
)
replace_once(
    runtime,
    "    fn invoke_destroy_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {\n",
    '''    fn invoke_transfer_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(target), ActionArgument::Value(source_owner), ActionArgument::Value(destination_owner)] = arguments else {
            return Err(RuntimeError::new(
                "internal child-transfer builtin expects target, source owner, and destination owner",
            ));
        };

        let target = match self.eval_expr(target, None)? {
            Value::String(target) if target.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present live designation",
                ));
            }
            Value::String(target) => target,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer target must be String, got {}",
                    other.type_name()
                )));
            }
        };
        let source_owner = match self.eval_expr(source_owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present source owner designation",
                ));
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer source owner must be String, got {}",
                    other.type_name()
                )));
            }
        };
        let destination_owner = match self.eval_expr(destination_owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "transfer requires a present destination owner designation",
                ));
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal child-transfer destination owner must be String, got {}",
                    other.type_name()
                )));
            }
        };

        self.transfer_runtime_model_owner(&target, &source_owner, &destination_owner)
    }

    fn invoke_destroy_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
''',
    "runtime transfer builtin",
)
replace_once(
    runtime,
    "        if name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION {\n",
    "        if name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION {\n            return self.invoke_transfer_builtin(arguments);\n        }\n        if name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION {\n",
    "runtime dispatch",
)

# Language reference: insert a new numbered section and shift later section numbers.
reference = Path("docs/LANGUAGE_REFERENCE.md")
text = reference.read_text()
for old, new in [
    ("# 23. Implementation boundaries are not language law", "# 24. Implementation boundaries are not language law"),
    ("# 22. What is not currently language surface", "# 23. What is not currently language surface"),
    ("# 21. Filter → reduction composition — v0.8.0", "# 22. Filter → reduction composition — v0.8.0"),
    ("# 20. Derived `filter` and ordered views — provisional/implemented", "# 21. Derived `filter` and ordered views — provisional/implemented"),
    ("# 19. Read-only `reduce` — v0.8.0 provisional/implemented", "# 20. Read-only `reduce` — v0.8.0 provisional/implemented"),
]:
    if text.count(old) != 1:
        raise SystemExit(f"LANGUAGE_REFERENCE heading {old!r}: expected one match")
    text = text.replace(old, new, 1)

anchor = "---\n\n# 20. Read-only `reduce` — v0.8.0 provisional/implemented\n"
if text.count(anchor) != 1:
    raise SystemExit("LANGUAGE_REFERENCE transfer insertion anchor missing")
section = '''---

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
'''
text = text.replace(anchor, section, 1)
text = text.replace(
    "This operation does not introduce ownership transfer, reparenting, cascading destruction,\n",
    "This destruction operation does not itself transfer or reparent ownership, and does not introduce cascading destruction,\n",
    1,
)
reference.write_text(text)

# Durable design decision.
decisions = Path("docs/DESIGN_DECISIONS.md")
text = decisions.read_text()
anchor = "Revisit only when realistic composition demonstrates one of those stronger facts is needed.\n"
if text.count(anchor) != 1:
    raise SystemExit("DESIGN_DECISIONS transfer insertion anchor missing")
section = '''Revisit only when realistic composition demonstrates one of those stronger facts is needed.

## Lifetime provenance transfer is independent from structural movement

Folder/document composition established one application fact that cannot be reproduced by
structural membership edits when the current folder itself is the child's lifetime owner: the
same existing child identity may need a different exact root-provenance owner.

Decision:

```elanu
transfer selected from leftFolder to rightFolder
```

changes only the authoritative root/lifetime provenance of the exact designated committed dynamic
leaf child. The child identity does not change. Structural membership does not change. Persistent
`live T` / `maybe live T` designations continue to identify the same child, and exact writable
authority already granted to child member state continues to address the same state identities.

The `from` operand is an explicit stale-owner proof: it must resolve to the exact owner recorded by
current transaction-visible provenance. The `to` operand supplies the replacement live modeled owner
identity. The destination becomes authoritative immediately for later lifetime checks in the same
shared action transaction, and failure rolls the transfer back. A child that roots another live
dynamic child is rejected rather than implicitly moving a subtree.

Rationale:

Membership and lifetime provenance have repeatedly composed as separate application facts. A
combined move-and-reparent operation would make ordinary UI/container vocabulary silently change
lifetime, while implicit transfer on `insert`/`remove` would contradict established cross-owner
non-owning membership. Keeping transfer independent lets an application compose provenance and
structure atomically when both genuinely need to change, while applications with workspace-rooted
lifetimes need no transfer at all.

A framework can maintain a parallel current-owner key/table, but that duplicates a fact the compiler
already owns: compiler lifetime operations would otherwise continue to use the original root
provenance. For applications whose current owner is genuinely a lifetime fact, one compiler-owned
relation is therefore materially more coherent than reconstructing ownership beside the language.

Alternatives rejected by the supporting pressure:

- treating structural `move` as lifetime reparenting;
- implicit transfer when membership is inserted or removed;
- replacing the child with a newly created identity under the destination;
- a general ownership/borrowing/reference system;
- subtree migration as part of the first operation.

Boundaries:

- the first surface is limited to existing committed dynamic leaf children selected through
  persistent scalar `live T` / `maybe live T` state;
- owner operands follow the current static-root/persistent-live owner-proof family;
- same-owner transfer is a validated no-op;
- transfer does not perform structural edits, designation clearing, or authority changes;
- self-rooting, absent operands, stale source-owner proof, unknown/dead destinations, and rooted
  descendants fail transactionally;
- Elanu currently does not infer owner/child compatibility from structural `[live T]` members;
- no subtree transfer, cascade, fresh-child transfer, ownership graph, or ordinary-value move
  semantics is selected.

Revisit when composition requires descendant migration, a broader target/owner operand family, or a
real owner/child compatibility relation that cannot remain independent of structural membership.
'''
text = text.replace(anchor, section, 1)
decisions.write_text(text)

# Changelog.
changelog = Path("CHANGELOG.md")
text = changelog.read_text()
lang_anchor = "### Language and semantics\n\n"
if text.count(lang_anchor) < 1:
    raise SystemExit("CHANGELOG language anchor missing")
lang_add = '''### Language and semantics

- Added bounded lifetime provenance transfer with `transfer target from sourceOwner to destinationOwner` for existing committed dynamic leaf children.
- Transfer preserves exact child identity, persistent designation targets, writable authority, and all structural memberships while transactionally replacing only the authoritative root owner.
- The source owner must prove current provenance; the destination must be a live modeled identity; subtree transfer, implicit structural reparenting, and general ownership/borrowing remain unselected.
'''
text = text.replace(lang_anchor, lang_add, 1)
compiler_anchor = "### Compiler/runtime\n\n"
if text.count(compiler_anchor) < 1:
    raise SystemExit("CHANGELOG compiler anchor missing")
compiler_add = '''### Compiler/runtime

- Added transaction-visible dynamic-owner replacement so later lifetime checks in the same action observe staged provenance transfer and rollback discards it cleanly.
- Reused persistent designation lowering and explicit owner identity transport rather than reconstructing provenance from generated names or structural membership.
'''
text = text.replace(compiler_anchor, compiler_add, 1)
changelog.write_text(text)
