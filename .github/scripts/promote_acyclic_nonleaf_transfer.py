from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))

runtime = Path("compiler/src/runtime.rs")
text = runtime.read_text()
text = text.replace(
    "    #[cfg(test)]\n    fn transfer_destination_would_create_provenance_cycle(\n",
    "    fn transfer_destination_would_create_provenance_cycle(\n",
    1,
)
old_leaf = '''        if self.has_live_dynamic_child_rooted_in(identity) {
            return Err(RuntimeError::new(
                "runtime modeled-state child roots another live child; subtree provenance transfer is not selected",
            ));
        }
'''
new_cycle = '''        if self.transfer_destination_would_create_provenance_cycle(identity, destination_owner) {
            return Err(RuntimeError::new(
                "runtime modeled-state provenance transfer would create an owner cycle",
            ));
        }
'''
if text.count(old_leaf) != 1:
    raise SystemExit(f"production leaf check: expected one match, found {text.count(old_leaf)}")
text = text.replace(old_leaf, new_cycle, 1)
start_marker = "    #[cfg(test)]\n    fn transfer_runtime_model_owner_nonleaf_experiment(\n"
start = text.find(start_marker)
if start < 0:
    raise SystemExit("test-only non-leaf transfer implementation not found")
end_marker = "\n    fn terminate_runtime_model(\n"
end = text.find(end_marker, start)
if end < 0:
    raise SystemExit("terminate_runtime_model anchor not found")
text = text[:start] + text[end+1:]
runtime.write_text(text)

# Promote the focused experiment tests to production-transfer regression tests.
tests = Path("compiler/src/runtime/provenance_transfer_execution.rs")
text = tests.read_text()
text = text.replace(
    "// Runtime-semantic experiment only: these tests pressure one narrow leaf-child\n// provenance transition without selecting compiler-accepted source syntax.\n",
    "// Runtime provenance regression coverage for the compiler-accepted transfer law.\n",
    1,
)
text = text.replace(
    "transfer_runtime_model_owner_nonleaf_experiment",
    "transfer_runtime_model_owner",
)
text = text.replace("nonleaf_experiment_", "nonleaf_transfer_")
old_test = '''#[test]
fn transfer_rejects_non_leaf_child() {
    let mut runtime = runtime(SOURCE);
    runtime.run_action("seed").expect("seed should commit");
    let identity = targets(&mut runtime, "__meld_mseq$left$documents")[0].clone();

    runtime.transaction = Some(Transaction::default());
    let descendant = runtime
        .instantiate_runtime_model("Document", &identity)
        .expect("child should be usable as a runtime owner");
    let created = runtime
        .transaction
        .take()
        .expect("transaction should exist");
    runtime.commit(created);
    assert_eq!(
        runtime.dynamic_model_owners.get(&descendant),
        Some(&identity)
    );

    let error = run_test_transaction(&mut runtime, |runtime| {
        runtime.transfer_runtime_model_owner(&identity, "left", "right")
    })
    .expect_err("first transfer semantic must stay leaf-only");

    assert!(error.message.contains("roots another live child"));
    assert_eq!(
        runtime.current_dynamic_model_owner(&identity).as_deref(),
        Some("left")
    );
}

'''
if text.count(old_test) != 1:
    raise SystemExit(f"old leaf rejection test: expected one match, found {text.count(old_test)}")
text = text.replace(old_test, "", 1)
tests.write_text(text)

# Language reference: broaden existing transfer law without changing syntax.
reference = Path("docs/LANGUAGE_REFERENCE.md")
text = reference.read_text()
text = text.replace(
    "committed dynamic **leaf** child without changing that child's identity:",
    "committed dynamic child without changing that child's identity:",
    1,
)
old = '''- the target must designate an existing **committed dynamic leaf child**; fresh transaction-local
  children, statically declared modeled roots, and children that still root another live child are
  not accepted by this first surface;
'''
new = '''- the target must designate an existing committed dynamic child; fresh transaction-local children
  and statically declared modeled roots are not accepted by this surface;
- the target may itself root live dynamic descendants; transfer changes only the target's own
  parent/root-provenance edge and leaves descendant provenance unchanged;
'''
if text.count(old) != 1:
    raise SystemExit("LANGUAGE_REFERENCE target boundary not found")
text = text.replace(old, new, 1)
text = text.replace(
    '''- the `to` owner must resolve to an existing live modeled-state identity and must not be the child
  itself;
''',
    '''- the `to` owner must resolve to an existing live modeled-state identity and must not be the child
  itself or any transaction-visible descendant of that child; the resulting provenance relation must
  remain acyclic;
''',
    1,
)
text = text.replace(
    '''This first surface does not introduce subtree transfer, cascading destruction, general ownership
graphs, ownership-bearing `live T`, borrowing/move semantics for ordinary values, arbitrary
designation expressions, or implicit reparenting. `transfer`, `from`, and `to` are contextual in
this bounded statement surface; compiler-private transfer helpers and transaction overlays are not
language law.
''',
    '''This surface does not introduce descendant provenance migration, cascading destruction, a public
ownership-graph API, ownership-bearing `live T`, borrowing/move semantics for ordinary values,
arbitrary designation expressions, or implicit reparenting. A non-leaf transfer preserves the
existing descendant-to-owner edges; it is not subtree migration. `transfer`, `from`, and `to` are
contextual in this bounded statement surface; compiler-private transfer helpers and transaction
overlays are not language law.
''',
    1,
)
reference.write_text(text)

# Durable semantic decision.
decisions = Path("docs/DESIGN_DECISIONS.md")
text = decisions.read_text()
text = text.replace(
    "changes only the authoritative root/lifetime provenance of the exact designated committed dynamic\nleaf child.",
    "changes only the authoritative root/lifetime provenance of the exact designated committed dynamic\nidentity.",
    1,
)
text = text.replace(
    '''identity. The destination becomes authoritative immediately for later lifetime checks in the same
shared action transaction, and failure rolls the transfer back. A child that roots another live
dynamic child is rejected rather than implicitly moving a subtree.
''',
    '''identity. The destination becomes authoritative immediately for later lifetime checks in the same
shared action transaction, and failure rolls the transfer back. The transferred identity may itself
root live descendants; their existing provenance edges remain unchanged. The destination owner chain
must not reach the transferred identity, so the transaction-visible provenance relation remains
acyclic.
''',
    1,
)
old = '''Boundaries:

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
new = '''Boundaries:

- the surface is limited to existing committed dynamic identities selected through persistent scalar
  `live T` / `maybe live T` state;
- owner operands follow the current static-root/persistent-live owner-proof family;
- same-owner transfer is a validated no-op;
- transfer does not perform structural edits, designation clearing, authority changes, or descendant
  provenance rewriting;
- self-rooting, absent operands, stale source-owner proof, unknown/dead destinations, and any
  destination ancestry that would create a transaction-visible provenance cycle fail transactionally;
- Elanu currently does not infer owner/child compatibility from structural `[live T]` members;
- no descendant migration, cascade, fresh-child transfer, public ownership graph, or ordinary-value
  move semantics is selected;
- non-leaf destruction remains a separate unresolved question and `destroy` remains leaf-only.

Revisit when composition requires descendant migration, non-leaf destruction policy, a broader
operand family, or a real owner/child compatibility relation that cannot remain independent of
structural membership.
'''
if text.count(old) != 1:
    raise SystemExit("DESIGN_DECISIONS transfer boundaries not found")
text = text.replace(old, new, 1)
decisions.write_text(text)

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
anchor = "### Language and semantics\n\n"
addition = '''### Language and semantics

- Broadened existing lifetime `transfer` from leaf-only children to any committed dynamic identity when replacing its parent provenance edge preserves an acyclic transaction-visible provenance relation.
- Non-leaf transfer preserves all descendant provenance edges; it is parent-edge replacement, not subtree migration, and `destroy` remains leaf-only.
'''
if text.count(anchor) < 1:
    raise SystemExit("CHANGELOG language anchor missing")
text = text.replace(anchor, addition, 1)
compiler_anchor = "### Compiler/runtime\n\n"
compiler_add = '''### Compiler/runtime

- Promoted the proven transaction-visible destination-owner-chain cycle check into production transfer and removed the duplicate test-only non-leaf transfer implementation.
'''
if text.count(compiler_anchor) < 1:
    raise SystemExit("CHANGELOG compiler anchor missing")
text = text.replace(compiler_anchor, compiler_add, 1)
changelog.write_text(text)
