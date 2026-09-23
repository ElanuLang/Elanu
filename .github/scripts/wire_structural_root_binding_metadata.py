from pathlib import Path

# Make the lowering producer available within the crate so CheckedSource can
# retain the exact binding rather than another subsystem reconstructing it.
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

collect_anchor = '''    let runtime_model_roots =
        runtime_model_templates::collect_roots(&program, &runtime_model_templates);
'''
collect = collect_anchor + '''    let runtime_model_root_member_bindings = runtime_model_roots
        .iter()
        .filter_map(|(root_name, root)| {
            let template = runtime_model_templates.get(&root.model_name)?;
            let bindings = template
                .members
                .iter()
                .filter(|member| member.kind == runtime_model_templates::RuntimeModelMemberKind::State)
                .map(|member| {
                    (
                        member.name.clone(),
                        model_lowering::model_binding_name(root_name, &member.name),
                    )
                })
                .collect::<HashMap<_, _>>();
            Some((root_name.clone(), bindings))
        })
        .collect::<HashMap<_, _>>();
'''
if "let runtime_model_root_member_bindings" not in text:
    if collect_anchor not in text:
        raise SystemExit("root collection anchor not found")
    text = text.replace(collect_anchor, collect, 1)

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
