from pathlib import Path

# Expose the actual model-lowering binding producer within the crate so
# CheckedSource can retain exact ordinary stored-member bindings rather than
# another subsystem reconstructing the private naming convention.
model = Path("compiler/src/model_lowering.rs")
text = model.read_text()
old = '''fn model_binding_name(root: &str, member: &str) -> String {
    format!("__meld_sm${root}${member}")
}
'''
new = '''pub(crate) fn model_binding_name(root: &str, member: &str) -> String {
    format!("__meld_sm${root}${member}")
}
'''
if old in text:
    model.write_text(text.replace(old, new, 1))
elif new not in text:
    raise SystemExit("model binding producer anchor not found")

lib = Path("compiler/src/lib.rs")
text = lib.read_text()
field_anchor = '''    pub runtime_model_roots: HashMap<String, RuntimeModelRoot>,
'''
field = field_anchor + '''    pub(crate) runtime_model_root_member_bindings: HashMap<String, HashMap<String, String>>,
'''
if "runtime_model_root_member_bindings" not in text:
    if field_anchor not in text:
        raise SystemExit("CheckedSource root field anchor not found")
    text = text.replace(field_anchor, field, 1)

# Build the final source root/member -> runtime state mapping only after model
# sequence externalization is known. Externalized sequence provenance already
# carries owner_root/member_name structurally, so it overrides the ordinary
# model-lowering binding for those members.
sequence_anchor = '''    let model_sequence_integration::ModelSequenceLowering {
        program: model_sequence_lowered,
        externalized_sequences,
    } = model_sequence_integration::lower(&filter_prepared)?;
'''
sequence_addition = sequence_anchor + '''    let externalized_root_member_bindings = externalized_sequences
        .iter()
        .map(|(binding, sequence)| {
            (
                (sequence.owner_root.clone(), sequence.member_name.clone()),
                binding.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let runtime_model_root_member_bindings = runtime_model_roots
        .iter()
        .filter_map(|(root_name, root)| {
            let template = runtime_model_templates.get(&root.model_name)?;
            let bindings = template
                .members
                .iter()
                .filter(|member| member.kind == runtime_model_templates::RuntimeModelMemberKind::State)
                .map(|member| {
                    let binding = externalized_root_member_bindings
                        .get(&(root_name.clone(), member.name.clone()))
                        .cloned()
                        .unwrap_or_else(|| model_lowering::model_binding_name(root_name, &member.name));
                    (member.name.clone(), binding)
                })
                .collect::<HashMap<_, _>>();
            Some((root_name.clone(), bindings))
        })
        .collect::<HashMap<_, _>>();
'''
if "let externalized_root_member_bindings" not in text:
    if sequence_anchor not in text:
        raise SystemExit("model sequence lowering anchor not found")
    text = text.replace(sequence_anchor, sequence_addition, 1)

init_anchor = '''        runtime_model_templates,
        runtime_model_roots,
'''
init = init_anchor + '''        runtime_model_root_member_bindings,
'''
if "        runtime_model_root_member_bindings,\n" not in text:
    if init_anchor not in text:
        raise SystemExit("CheckedSource root initializer anchor not found")
    text = text.replace(init_anchor, init, 1)
lib.write_text(text)

partial = Path("compiler/src/runtime/persistence/partial.rs")
text = partial.read_text()
old = '''        let state_name = super::super::model_binding_name(root, member);
'''
new = '''        let state_name = self
            .checked
            .runtime_model_root_member_bindings
            .get(root)
            .and_then(|members| members.get(member))
            .cloned()
            .ok_or_else(|| RuntimeError::new(format!(
                "runtime binding for modeled root selection '{root}.{member}' is missing"
            )))?;
'''
if old in text:
    partial.write_text(text.replace(old, new, 1))
elif new not in text:
    raise SystemExit("structural root binding consumer anchor not found")
