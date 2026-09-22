from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


# Extend the existing lifetime-termination surface rather than creating another pass.
surface = Path("compiler/src/lifetime_termination_surface.rs")
text = surface.read_text()
text = text.replace(
    'pub const DESTROY_BUILTIN_ACTION: &str = "__meld_surface_destroy_child_builtin";\n',
    'pub const DESTROY_BUILTIN_ACTION: &str = "__meld_surface_destroy_child_builtin";\npub const PURGE_BUILTIN_ACTION: &str = "__meld_surface_purge_subtree_builtin";\n',
    1,
)
text = text.replace(
    '''    if source.contains(DESTROY_BUILTIN_ACTION) {
        return Err(vec![Diagnostic::new(
            "reserved compiler child-destroy marker cannot appear in source",
            1,
            1,
        )]);
    }
''',
    '''    for (reserved, label) in [
        (DESTROY_BUILTIN_ACTION, "child-destroy"),
        (PURGE_BUILTIN_ACTION, "subtree-purge"),
    ] {
        if source.contains(reserved) {
            return Err(vec![Diagnostic::new(
                format!("reserved compiler {label} marker cannot appear in source"),
                1,
                1,
            )]);
        }
    }
''',
    1,
)
text = text.replace(
    '''    let mut index = 0;
    let mut saw_destroy = false;
''',
    '''    let mut index = 0;
    let mut saw_destroy = false;
    let mut saw_purge = false;
''',
    1,
)
needle = '''        if keyword_at(source, index, "destroy") {
            if let Some((end, designation, owner)) = parse_destroy_statement(source, index) {
                output.push_str(DESTROY_BUILTIN_ACTION);
                output.push('(');
                output.push_str(&designation);
                output.push_str(", ");
                output.push_str(&owner);
                output.push(')');
                index = end;
                saw_destroy = true;
                continue;
            }
        }
'''
replacement = needle + '''
        if keyword_at(source, index, "purge") {
            if let Some((end, designation, owner)) = parse_purge_statement(source, index) {
                output.push_str(PURGE_BUILTIN_ACTION);
                output.push('(');
                output.push_str(&designation);
                output.push_str(", ");
                output.push_str(&owner);
                output.push(')');
                index = end;
                saw_purge = true;
                continue;
            }
        }
'''
if text.count(needle) != 1:
    raise SystemExit("destroy preprocess block not found")
text = text.replace(needle, replacement, 1)
text = text.replace(
    '''    if saw_destroy {
        output.push_str("\\naction ");
        output.push_str(DESTROY_BUILTIN_ACTION);
        output.push_str("(target: String, ownerName: String) {}\\n");
    }

    Ok(output)
''',
    '''    if saw_destroy {
        output.push_str("\\naction ");
        output.push_str(DESTROY_BUILTIN_ACTION);
        output.push_str("(target: String, ownerName: String) {}\\n");
    }
    if saw_purge {
        output.push_str("\\naction ");
        output.push_str(PURGE_BUILTIN_ACTION);
        output.push_str("(target: String, ownerName: String) {}\\n");
    }

    Ok(output)
''',
    1,
)
text = text.replace(
    ''') if name == DESTROY_BUILTIN_ACTION => {
''',
    ''') if name == DESTROY_BUILTIN_ACTION || name == PURGE_BUILTIN_ACTION => {
''',
    1,
)
# Validation has a second destroy-specific match; make diagnostics operation-aware.
old_validate = '''            } if name == DESTROY_BUILTIN_ACTION => {
                let [ActionArgument::Value(Expr::Name(designation)), ActionArgument::Value(Expr::Name(owner))] =
                    arguments.as_slice()
                else {
                    errors.push(Diagnostic::new(
                        "internal child-destroy marker is malformed",
                        location.line,
                        location.column,
                    ));
                    continue;
                };

                match designation_kinds.get(designation) {
                    Some((_, true)) => {}
                    Some((model, false)) => errors.push(Diagnostic::new(
                        format!(
                            "destroy requires persistent maybe live {model}; '{designation}' is plain live {model} and cannot become absent"
                        ),
                        location.line,
                        location.column,
                    )),
                    None => errors.push(Diagnostic::new(
                        format!(
                            "destroy requires a persistent maybe live designation state; '{designation}' is not one"
                        ),
                        location.line,
                        location.column,
                    )),
                }

                if !roots.contains_key(owner) && !designation_kinds.contains_key(owner) {
                    errors.push(Diagnostic::new(
                        format!(
                            "destroy owner '{owner}' is not a modeled-state root or persistent live designation"
                        ),
                        location.line,
                        location.column,
                    ));
                }
            }
'''
new_validate = '''            } if name == DESTROY_BUILTIN_ACTION || name == PURGE_BUILTIN_ACTION => {
                let operation = if name == DESTROY_BUILTIN_ACTION { "destroy" } else { "purge" };
                let [ActionArgument::Value(Expr::Name(designation)), ActionArgument::Value(Expr::Name(owner))] =
                    arguments.as_slice()
                else {
                    errors.push(Diagnostic::new(
                        format!("internal child-{operation} marker is malformed"),
                        location.line,
                        location.column,
                    ));
                    continue;
                };

                match designation_kinds.get(designation) {
                    Some((_, true)) => {}
                    Some((model, false)) => errors.push(Diagnostic::new(
                        format!(
                            "{operation} requires persistent maybe live {model}; '{designation}' is plain live {model} and cannot become absent"
                        ),
                        location.line,
                        location.column,
                    )),
                    None => errors.push(Diagnostic::new(
                        format!(
                            "{operation} requires a persistent maybe live designation state; '{designation}' is not one"
                        ),
                        location.line,
                        location.column,
                    )),
                }

                if !roots.contains_key(owner) && !designation_kinds.contains_key(owner) {
                    errors.push(Diagnostic::new(
                        format!(
                            "{operation} owner '{owner}' is not a modeled-state root or persistent live designation"
                        ),
                        location.line,
                        location.column,
                    ));
                }
            }
'''
if text.count(old_validate) != 1:
    raise SystemExit("destroy validation block not found")
text = text.replace(old_validate, new_validate, 1)
# Add purge parser alongside destroy parser.
anchor = '''fn parse_destroy_statement(source: &str, start: usize) -> Option<(usize, String, String)> {
    let mut index = start + "destroy".len();
'''
if text.count(anchor) != 1:
    raise SystemExit("destroy parser anchor missing")
purge_parser = '''fn parse_purge_statement(source: &str, start: usize) -> Option<(usize, String, String)> {
    let mut index = start + "purge".len();
    index = skip_inline_whitespace(source, index);

    let (designation, next) = parse_identifier(source, index)?;
    index = skip_inline_whitespace(source, next);
    if !keyword_at(source, index, "in") {
        return None;
    }
    index += "in".len();
    index = skip_inline_whitespace(source, index);

    let (owner, next) = parse_name_path(source, index)?;
    index = skip_inline_whitespace(source, next);

    match source.as_bytes().get(index).copied() {
        None | Some(b'\\n' | b'\\r' | b';' | b'}') => Some((index, designation, owner)),
        Some(b'/') if source[index..].starts_with("//") => Some((index, designation, owner)),
        _ => None,
    }
}

'''
text = text.replace(anchor, purge_parser + anchor, 1)
surface.write_text(text)

# Let live designation lowering carry purge operands through the same private identity path.
live = Path("compiler/src/live_designation_lowering.rs")
text = live.read_text()
old = '''                if (name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION
                    || name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION
'''
new = '''                if (name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION
                    || name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION
                    || name == crate::lifetime_termination_surface::PURGE_BUILTIN_ACTION
'''
if text.count(old) != 1:
    raise SystemExit("live designation lifetime allow-list anchor missing")
live.write_text(text.replace(old, new, 1))

# Promote the proven runtime subtree composition and route a separate purge builtin to it.
runtime = Path("compiler/src/runtime.rs")
text = runtime.read_text()
text = text.replace(
    '''    #[cfg(test)]
    fn committed_provenance_subtree_postorder_experiment(
''',
    '''    fn committed_provenance_subtree_postorder(
''',
    1,
)
text = text.replace(
    '''    #[cfg(test)]
    fn terminate_runtime_model_subtree_experiment(
''',
    '''    fn terminate_runtime_model_subtree(
''',
    1,
)
text = text.replace(
    "self.committed_provenance_subtree_postorder_experiment(identity)?",
    "self.committed_provenance_subtree_postorder(identity)?",
    1,
)
# Insert runtime purge builtin immediately after destroy builtin.
destroy_end = '''        self.terminate_runtime_model(&target, &owner)
    }

    fn invoke_action(
'''
purge_builtin = '''        self.terminate_runtime_model(&target, &owner)
    }

    fn invoke_purge_builtin(&mut self, arguments: &[ActionArgument]) -> Result<(), RuntimeError> {
        let [ActionArgument::Value(target), ActionArgument::Value(owner)] = arguments else {
            return Err(RuntimeError::new(
                "internal subtree-purge builtin expects designation and owner",
            ));
        };

        let target = match self.eval_expr(target, None)? {
            Value::String(target) => target,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal subtree-purge designation must be String, got {}",
                    other.type_name()
                )))
            }
        };
        if target.is_empty() {
            return Err(RuntimeError::new(
                "purge requires a present maybe live designation",
            ));
        }

        let owner = match self.eval_expr(owner, None)? {
            Value::String(owner) if owner.is_empty() => {
                return Err(RuntimeError::new(
                    "purge requires a present live owner designation",
                ))
            }
            Value::String(owner) => owner,
            other => {
                return Err(RuntimeError::new(format!(
                    "internal subtree-purge owner must be String, got {}",
                    other.type_name()
                )))
            }
        };

        self.terminate_runtime_model_subtree(&target, &owner)
    }

    fn invoke_action(
'''
if text.count(destroy_end) != 1:
    raise SystemExit("runtime destroy builtin end anchor missing")
text = text.replace(destroy_end, purge_builtin, 1)
action_anchor = '''        if name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION {
            return self.invoke_destroy_builtin(arguments);
        }
'''
action_new = action_anchor + '''        if name == crate::lifetime_termination_surface::PURGE_BUILTIN_ACTION {
            return self.invoke_purge_builtin(arguments);
        }
'''
if text.count(action_anchor) != 1:
    raise SystemExit("runtime lifetime dispatch anchor missing")
text = text.replace(action_anchor, action_new, 1)
runtime.write_text(text)

# Promote experiment calls and add an end-to-end source action assertion.
tests = Path("compiler/src/runtime/lifetime_termination_execution.rs")
text = tests.read_text()
text = text.replace("terminate_runtime_model_subtree_experiment", "terminate_runtime_model_subtree")
source_anchor = '''action pinChild {
    required = root.children[0]
}
"#;
'''
source_new = '''action pinChild {
    required = root.children[0]
}

action purgeRoot {
    purge root in left
}
"#;
'''
if text.count(source_anchor) != 1:
    raise SystemExit("subtree source action anchor missing")
text = text.replace(source_anchor, source_new, 1)
append_test = r'''

#[test]
fn compiler_accepted_purge_ends_the_current_committed_provenance_subtree() {
    let mut runtime = runtime(SUBTREE_PURGE_SOURCE);
    runtime
        .run_action("seedSubtree")
        .expect("subtree source should seed");

    let root = designation_target(&mut runtime, "root");
    let child = designation_target(&mut runtime, "child");
    let grandchild = designation_target(&mut runtime, "grandchild");
    let outsider = designation_target(&mut runtime, "outsider");

    runtime
        .run_action("purgeRoot")
        .expect("compiler-accepted purge should terminate the lifetime subtree");

    assert!(!runtime.model_identity_exists(&root));
    assert!(!runtime.model_identity_exists(&child));
    assert!(!runtime.model_identity_exists(&grandchild));
    assert!(runtime.model_identity_exists(&outsider));
    assert_eq!(
        runtime.value("__meld_live$root").unwrap(),
        Value::String(String::new())
    );
}
'''
if "compiler_accepted_purge_ends_the_current_committed_provenance_subtree" in text:
    raise SystemExit("source purge test already present")
tests.write_text(text + append_test)

# Public language reference: add explicit purge section while preserving leaf destroy.
reference = Path("docs/LANGUAGE_REFERENCE.md")
text = reference.read_text()
anchor = "# 19. Owner-relative child lifetime `transfer` — provisional/implemented\n"
if text.count(anchor) != 1:
    raise SystemExit("LANGUAGE_REFERENCE transfer section anchor missing")
purge_section = '''# 19. Explicit lifetime-subtree `purge` — provisional/implemented

Elanu accepts an explicit operation for permanently ending one committed dynamic lifetime subtree:

```elanu
purge selectedFolder in trashFolder
```

`purge` is intentionally distinct from leaf-only `destroy`. `destroy` requests termination of exactly
one identity and fails while that identity still roots another live child. `purge` explicitly requests
termination of the selected identity plus all committed dynamic identities transitively rooted beneath
it in the current transaction-visible lifetime-provenance relation.

Current contract:

- the target must be persistent `maybe live T` designation state and must be present at execution;
- the target must designate an existing committed dynamic identity;
- the owner operand must prove the target's exact current root-provenance owner and may be a static
  modeled root or persistent live owner designation;
- descendants are selected from compiler-owned lifetime provenance, never from structural membership;
- committed descendants are terminated leaves-first through the same per-identity cleanup law used by
  leaf `destroy`;
- staged provenance transfers earlier in the same action affect the purge set: a committed branch moved
  out survives, while a committed identity moved into the subtree joins the purge;
- every terminated identity receives the existing lifetime cleanup: all structural occurrences are
  removed, persistent `maybe live T` designations are cleared, model-local state/derived state end, and
  its provenance edge ends;
- any persistent plain `live T` designation targeting any identity in the selected subtree blocks the
  whole purge rather than becoming dangling;
- a fresh transaction-local descendant anywhere in the selected subtree currently blocks purge because
  fresh-child cancellation/termination remains unselected;
- purge participates in the surrounding action transaction, so any failure rolls the entire subtree
  transition and all other staged work back.

Moving a folder to Trash does not imply purge. A rooted hierarchy may be moved cheaply by provenance
transfer plus structural edits while all descendants remain live. `purge` represents the stronger,
explicit application intent that the lifetime subtree itself permanently ends.

`purge` does not add structural tree traversal, descendant reparenting, garbage collection, reference
counting, external-resource cleanup, persistence deletion policy, or a public ownership/provenance
API. Its subtree is the current lifetime-provenance subtree, not a collection-membership subtree.

---

# 20. Owner-relative child lifetime `transfer` — provisional/implemented
'''
text = text.replace(anchor, purge_section, 1)
# Shift subsequent section numbers 20+ by one for the known nearby headings only, conservatively.
for old_num, new_num in [(20,21),(21,22),(22,23),(23,24),(24,25),(25,26),(26,27),(27,28),(28,29),(29,30),(30,31)]:
    text = text.replace(f"# {old_num}. ", f"# {new_num}. ", 1)
reference.write_text(text)

# Durable semantic law: add purge after destruction, before transfer.
decisions = Path("docs/DESIGN_DECISIONS.md")
text = decisions.read_text()
anchor = "## Lifetime provenance transfer is independent from structural movement\n"
if text.count(anchor) != 1:
    raise SystemExit("DESIGN_DECISIONS transfer anchor missing")
decision = '''## Explicit purge ends a transaction-visible lifetime-provenance subtree

Decision:

```elanu
purge selected in owner
```

is the explicit subtree-ending counterpart to leaf-only `destroy`.

The durable semantic distinction is:

```text
destroy child in owner
    -> end exactly this committed identity
    -> fail if it still roots a live dynamic child

purge child in owner
    -> intentionally end this committed identity
       plus all committed transaction-visible provenance descendants
```

The purge set is defined by lifetime/root provenance, not structural membership. Cross-owner,
duplicate, filtered, or absent structural occurrences neither grant lifetime authority nor determine
which descendants die.

Runtime pressure established that no second cleanup mechanism is required. The compiler/runtime
orders the current committed provenance subtree leaves-first and applies the existing leaf lifetime
termination transition to every identity. As a result, the established cleanup law is preserved for
each descendant: optional designations clear, all structural occurrences disappear, model-local state
ends, and plain `live T` designations continue to block termination rather than dangling.

The subtree is transaction-visible. A committed branch transferred out before purge survives; a
committed identity transferred into the subtree before purge participates. Any failure during the
leaves-first sequence rolls all earlier staged terminations back with the surrounding action.

Fresh transaction-local descendants remain outside the selected semantic. If one is rooted in the
selected subtree, purge fails rather than silently inventing fresh-child cancellation.

Rationale:

Application code cannot enumerate an arbitrary runtime-sized lifetime subtree from current public
primitives without mirroring the compiler-owned provenance relation. Structural membership cannot be
used as a substitute because membership is deliberately non-owning. The compiler also already owns
the global cleanup consequences of lifetime termination. Explicit purge therefore removes duplicated
lifetime reconstruction while keeping product policy such as when to empty Trash outside the language.

The separate source operation is intentional. Making `destroy` silently recursive would turn a request
to end one identity into potentially unbounded descendant deletion. Retaining leaf `destroy` preserves
a useful failure boundary; `purge` states the stronger destructive intent explicitly.

This decision does not select implicit purge on Trash movement, fresh-child cancellation, descendant
reparenting, garbage collection/reference counting, external-resource cleanup, persistence deletion
policy, asynchronous deletion, or a public provenance/tree traversal API.

'''
text = text.replace(anchor, decision + anchor, 1)
decisions.write_text(text)

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
lang_anchor = "### Language and semantics\n\n"
lang_add = '''### Language and semantics

- Added explicit `purge target in owner` for permanent termination of a committed transaction-visible lifetime-provenance subtree while retaining leaf-only `destroy` as a separate operation.
- Purge discovers descendants from provenance rather than structure, reuses existing leaf cleanup leaves-first, observes staged transfers, blocks on plain-live designations, and rejects fresh transaction-local descendants.
'''
if text.count(lang_anchor) < 1:
    raise SystemExit("CHANGELOG language anchor missing")
text = text.replace(lang_anchor, lang_add, 1)
compiler_anchor = "### Compiler/runtime\n\n"
compiler_add = '''### Compiler/runtime

- Promoted the proven provenance-subtree post-order termination experiment into the runtime and routed explicit source `purge` through the existing lifetime-termination surface and per-identity cleanup path.
'''
if text.count(compiler_anchor) < 1:
    raise SystemExit("CHANGELOG compiler anchor missing")
text = text.replace(compiler_anchor, compiler_add, 1)
changelog.write_text(text)
