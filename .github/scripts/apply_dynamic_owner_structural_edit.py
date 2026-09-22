from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


# Register composition tests.
lib = Path("compiler/src/lib.rs")
replace_once(
    lib,
    "mod designation_runtime_metadata;\n",
    "mod designation_runtime_metadata;\n#[cfg(test)]\nmod dynamic_owner_structural_edit_tests;\n",
    "test module",
)

# Existing persistent-designation insertion: resolve destination owner model from either
# a static modeled root or a persistent live designation. Preserve the target path so the
# existing runtime grant transport can carry designation-relative sequence authority.
insert = Path("compiler/src/existing_designation_insert_surface.rs")
replace_once(
    insert,
    '''        let Some(root) = self.roots.get(root_name) else {
            self.errors.push(diag(
                location,
                format!("'{root_name}' is not a modeled-state owner root"),
            ));
            return malformed_insert(location);
        };
        let Some(owner_template) = self.templates.get(&root.model_name) else {
            self.errors.push(diag(
                location,
                format!("unknown state model '{}'", root.model_name),
            ));
            return malformed_insert(location);
        };
''',
    '''        let owner_model = if let Some(root) = self.roots.get(root_name) {
            root.model_name.clone()
        } else if let Some(model) = self.designation_models.get(root_name) {
            model.clone()
        } else {
            self.errors.push(diag(
                location,
                format!(
                    "'{root_name}' is not a modeled-state owner root or persistent live designation"
                ),
            ));
            return malformed_insert(location);
        };
        let Some(owner_template) = self.templates.get(&owner_model) else {
            self.errors.push(diag(
                location,
                format!("unknown state model '{owner_model}'"),
            ));
            return malformed_insert(location);
        };
''',
    "dynamic insertion owner",
)

# Structural removal: same owner-model resolution. Existing generated StateGrant paths are
# intentionally left intact for runtime_index_grant_transport to turn designation.member into
# a structured RuntimeDesignationMember carrier.
remove = Path("compiler/src/structural_edit_surface.rs")
replace_once(
    remove,
    '''        let Some(root) = self.roots.get(root_name) else {
            self.errors.push(Diagnostic::new(
                format!("'{root_name}' is not a modeled-state owner root"),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        let Some(owner_template) = self.templates.get(&root.model_name) else {
            self.errors.push(Diagnostic::new(
                format!("unknown state model '{}'", root.model_name),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
''',
    '''        let owner_model = if let Some(root) = self.roots.get(root_name) {
            root.model_name.clone()
        } else if let Some(model) = self.designation_models.get(root_name) {
            model.clone()
        } else {
            self.errors.push(Diagnostic::new(
                format!(
                    "'{root_name}' is not a modeled-state owner root or persistent live designation"
                ),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        let Some(owner_template) = self.templates.get(&owner_model) else {
            self.errors.push(Diagnostic::new(
                format!("unknown state model '{owner_model}'"),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
''',
    "dynamic removal owner",
)

# Designation-relative grants do not require any static model-sequence binding to exist.
transport = Path("compiler/src/runtime_index_grant_transport.rs")
replace_once(
    transport,
    '''    if indexed_source_models.is_empty() {
        return Ok(program.clone());
    }
''',
    '''    if indexed_source_models.is_empty() && designation_models.is_empty() {
        return Ok(program.clone());
    }
''',
    "designation-only grant transport",
)

# Document the extension narrowly.
reference = Path("docs/LANGUAGE_REFERENCE.md")
text = reference.read_text()
text = text.replace(
    "- the destination must be mutable owner-relative `[live T]` membership with the same\n  element model as the designation;\n",
    "- the destination must be mutable owner-relative `[live T]` membership with the same\n  element model as the designation; its owner may be a statically declared modeled root or a\n  persistent `live T` / present `maybe live T` designation of an exact modeled identity;\n",
    1,
)
text = text.replace(
    "For both forms, the target may be a mutable owner-relative `[live T]` state-model\nmember or a direct owner-relative identity-preserving `filter` view whose source is\nsuch mutable membership. Removal edits membership structure; it does not destroy the\ndesignated child or grant writable authority.\n",
    "For both forms, the target may be a mutable owner-relative `[live T]` state-model\nmember or a direct owner-relative identity-preserving `filter` view whose source is\nsuch mutable membership. For mutable stored membership, the owner may be a statically\ndeclared modeled root or a persistent `live T` / present `maybe live T` designation of an\nexact modeled identity. Removal edits membership structure; it does not destroy the\ndesignated child or grant writable authority.\n",
    1,
)
reference.write_text(text)

# Record semantic rationale without inventing a new structural concept.
decisions = Path("docs/DESIGN_DECISIONS.md")
text = decisions.read_text()
anchor = "## `[live T]` stores designation values, not child ownership\n"
if text.count(anchor) != 1:
    raise SystemExit("design decision anchor missing")
section = '''## Owner-relative structural mutation follows exact live owner identity

Dynamic-owner application composition established that structural mutation should follow the same
exact modeled-owner identity already supported by owner-relative reads and writable member grants.
For supported mutable `[live T]` member insertion/removal, the owner may therefore be a statically
declared modeled root or an exact current modeled identity carried by persistent `live T` / present
`maybe live T` designation state.

This does not make the owner designation owning, turn membership into lifetime provenance, or grant
child writable authority. It only selects which existing owner-local structural state identity is
being mutated. The writable target is resolved through the existing structured runtime designation-
member authority transport rather than by reconstructing an owner key or generated binding name.

The rule is required for realistic runtime-sized owners. A document whose lifetime is transferred
between two dynamically selected folders must be able to compose that provenance change with
independent insertion/removal of the same identity in those exact folders' membership state.

Absent maybe-live owners fail transactionally under the existing designation law. Structural edits
remain independent from lifetime transfer and continue to preserve duplicate/occurrence semantics.

'''
text = text.replace(anchor, section + anchor, 1)
decisions.write_text(text)

changelog = Path("CHANGELOG.md")
text = changelog.read_text()
anchor = "### Language and semantics\n\n"
text = text.replace(
    anchor,
    anchor + "- Extended existing-designation structural insertion/removal to mutable membership owned by exact runtime-selected modeled identities carried through persistent live designations.\n- Dynamic-owner structural edits remain non-owning and compose transactionally with independent lifetime provenance transfer.\n",
    1,
)
changelog.write_text(text)
