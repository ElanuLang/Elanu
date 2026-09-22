from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: {label}: expected one match, found {count}")
    path.write_text(text.replace(old, new, 1))


# model_types.rs: preserve optional designation role separately from ordinary ValueType.
path = Path("compiler/src/model_types.rs")
text = path.read_text()
text = text.replace("use std::collections::HashMap;", "use std::collections::{HashMap, HashSet};", 1)
text = text.replace(
    "use crate::ast::{Expr, SourceLocation, StateModelDecl, StateModelMember};",
    "use crate::ast::{decode_maybe_live_type_name, Expr, SourceLocation, StateModelDecl, StateModelMember};",
    1,
)
old = '''#[derive(Debug, Clone)]
pub(crate) struct ModelTypeInfo {
    member_types: HashMap<String, ValueType>,
}

impl ModelTypeInfo {
    pub(crate) fn member_type(&self, name: &str) -> Option<&ValueType> {
        self.member_types.get(name)
    }
}
'''
new = '''#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelDesignationType {
    pub(crate) model_name: String,
    pub(crate) allows_none: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelTypeInfo {
    member_types: HashMap<String, ValueType>,
    member_designations: HashMap<String, ModelDesignationType>,
}

impl ModelTypeInfo {
    pub(crate) fn member_type(&self, name: &str) -> Option<&ValueType> {
        self.member_types.get(name)
    }

    pub(crate) fn member_designation(&self, name: &str) -> Option<&ModelDesignationType> {
        self.member_designations.get(name)
    }
}
'''
if text.count(old) != 1:
    raise SystemExit("model_types ModelTypeInfo anchor missing")
text = text.replace(old, new, 1)
old = '''    let mut errors = Vec::new();
    let mut models = HashMap::new();

    for model in declarations {
'''
new = '''    let mut errors = Vec::new();
    let mut models = HashMap::new();
    let known_models = declarations
        .iter()
        .map(|model| model.name.clone())
        .collect::<HashSet<_>>();

    for model in declarations {
'''
text = text.replace(old, new, 1)
text = text.replace(
    "        let info = resolve_model(model, &mut errors);",
    "        let info = resolve_model(model, &known_models, &mut errors);",
    1,
)
text = text.replace(
    "fn resolve_model(model: &StateModelDecl, errors: &mut Vec<Diagnostic>) -> ModelTypeInfo {\n    let mut member_types = HashMap::new();",
    "fn resolve_model(\n    model: &StateModelDecl,\n    known_models: &HashSet<String>,\n    errors: &mut Vec<Diagnostic>,\n) -> ModelTypeInfo {\n    let mut member_types = HashMap::new();\n    let mut member_designations = HashMap::new();",
    1,
)
old = '''        let (expression, declared_type) = match member {
            StateModelMember::State(state) => {
                let declared_type = state.type_name.as_deref().and_then(|type_name| {
                    if let Some(parsed) = parse_primitive_type_name(type_name) {
                        return Some(parsed);
                    }
                    if let Some(element_model) = decode_sequence_live_type(type_name) {
                        return Some(ValueType::SequenceLive(element_model.to_string()));
                    }
                    errors.push(diag(
                        state.location,
                        format!(
                            "state-model member '{}.{}' has unsupported bootstrap type '{}'",
                            model.name, state.name, type_name
                        ),
                    ));
                    None
                });
                (&state.initializer, declared_type)
            }
            StateModelMember::Derived(derived) => (&derived.expression, None),
        };

        let inferred = match (&declared_type, expression) {
            (Some(ValueType::SequenceLive(_)), Expr::String(value))
                if decode_sequence_literal(value).is_some() =>
            {
                declared_type.clone()
            }
            _ => infer_model_expr_type(expression, &member_types, model, location, errors),
        };
'''
new = '''        let mut designation = None;
        let (expression, declared_type) = match member {
            StateModelMember::State(state) => {
                let declared_type = state.type_name.as_deref().and_then(|type_name| {
                    if let Some(parsed) = parse_primitive_type_name(type_name) {
                        return Some(parsed);
                    }
                    if let Some(element_model) = decode_sequence_live_type(type_name) {
                        return Some(ValueType::SequenceLive(element_model.to_string()));
                    }
                    if let Some(target_model) = decode_maybe_live_type_name(type_name) {
                        if !known_models.contains(target_model) {
                            errors.push(diag(
                                state.location,
                                format!(
                                    "model-local maybe live designation '{}.{}' names unknown state model '{}'",
                                    model.name, state.name, target_model
                                ),
                            ));
                        }
                        designation = Some(ModelDesignationType {
                            model_name: target_model.to_string(),
                            allows_none: true,
                        });
                        return Some(ValueType::String);
                    }
                    errors.push(diag(
                        state.location,
                        format!(
                            "state-model member '{}.{}' has unsupported bootstrap type '{}'",
                            model.name, state.name, type_name
                        ),
                    ));
                    None
                });
                (&state.initializer, declared_type)
            }
            StateModelMember::Derived(derived) => (&derived.expression, None),
        };

        if designation.is_none()
            && expression_references_designation(expression, &member_designations)
        {
            errors.push(diag(
                location,
                format!(
                    "state-model member '{}.{}' cannot use a model-local live designation as an ordinary value",
                    model.name, name
                ),
            ));
        }

        let inferred = if designation.is_some() {
            if !matches!(expression, Expr::Name(value) if value == "none") {
                errors.push(diag(
                    location,
                    format!(
                        "model-local maybe live designation '{}.{}' currently requires initializer 'none'",
                        model.name, name
                    ),
                ));
            }
            Some(ValueType::String)
        } else {
            match (&declared_type, expression) {
                (Some(ValueType::SequenceLive(_)), Expr::String(value))
                    if decode_sequence_literal(value).is_some() =>
                {
                    declared_type.clone()
                }
                _ => infer_model_expr_type(expression, &member_types, model, location, errors),
            }
        };
'''
if text.count(old) != 1:
    raise SystemExit("model_types member resolution block missing")
text = text.replace(old, new, 1)
old = '''        member_types.insert(name, value_type);
    }

    ModelTypeInfo { member_types }
}

fn infer_model_expr_type(
'''
new = '''        if let Some(designation) = designation {
            member_designations.insert(name.clone(), designation);
        }
        member_types.insert(name, value_type);
    }

    ModelTypeInfo {
        member_types,
        member_designations,
    }
}

fn expression_references_designation(
    expression: &Expr,
    designations: &HashMap<String, ModelDesignationType>,
) -> bool {
    match expression {
        Expr::Name(name) => designations.contains_key(name),
        Expr::Binary { left, right, .. } => {
            expression_references_designation(left, designations)
                || expression_references_designation(right, designations)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expression_references_designation(condition, designations)
                || expression_references_designation(then_branch, designations)
                || expression_references_designation(else_branch, designations)
        }
        Expr::IndexedMember { index, .. } | Expr::IndexedDesignation { index, .. } => {
            expression_references_designation(index, designations)
        }
        Expr::Filter { source, .. } => expression_references_designation(source, designations),
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => false,
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => false,
    }
}

fn infer_model_expr_type(
'''
if text.count(old) != 1:
    raise SystemExit("model_types result anchor missing")
text = text.replace(old, new, 1)
path.write_text(text)


# runtime_model_templates.rs: carry designation role with each model member.
path = Path("compiler/src/runtime_model_templates.rs")
text = path.read_text()
text = text.replace(
    "use crate::diagnostic::Diagnostic;",
    "use crate::designation_runtime_metadata::RuntimeDesignationMetadata;\nuse crate::diagnostic::Diagnostic;",
    1,
)
text = text.replace(
    '''    pub value_type: ValueType,
    pub expression: Expr,
''',
    '''    pub value_type: ValueType,
    pub(crate) designation: Option<RuntimeDesignationMetadata>,
    pub expression: Expr,
''',
    1,
)
old = '''                let value_type = resolved
                    .member_type(&name)
                    .expect("validated state-model member should have a shared value type")
                    .clone();

                let (kind, mut expression) = match member {
'''
new = '''                let value_type = resolved
                    .member_type(&name)
                    .expect("validated state-model member should have a shared value type")
                    .clone();
                let designation = resolved.member_designation(&name).map(|designation| {
                    RuntimeDesignationMetadata {
                        model_name: designation.model_name.clone(),
                        allows_none: designation.allows_none,
                    }
                });

                let (kind, mut expression) = match member {
'''
text = text.replace(old, new, 1)
old = '''                if let Expr::Filter {
                    source,
                    element_model,
                    ..
                } = &mut expression
'''
new = '''                if designation.is_some() {
                    expression = Expr::String(String::new());
                }

                if let Expr::Filter {
                    source,
                    element_model,
                    ..
                } = &mut expression
'''
text = text.replace(old, new, 1)
text = text.replace(
    '''                    value_type,
                    expression,
''',
    '''                    value_type,
                    designation,
                    expression,
''',
    1,
)
path.write_text(text)


# model_lowering.rs: keep designation members out of ordinary value/authority paths.
path = Path("compiler/src/model_lowering.rs")
text = path.read_text()
text = text.replace(
    '''    value_type: ValueType,
    expression: Expr,
''',
    '''    value_type: ValueType,
    designation: bool,
    expression: Expr,
''',
    1,
)
old = '''            let (kind, expression) = match member {
                StateModelMember::State(state) => {
                    (ModelMemberKind::State, state.initializer.clone())
                }
                StateModelMember::Derived(derived) => {
                    (ModelMemberKind::Derived, derived.expression.clone())
                }
            };
            let value_type = resolved
'''
new = '''            let designation = resolved.member_designation(&name).is_some();
            let (kind, expression) = match member {
                StateModelMember::State(state) => (
                    ModelMemberKind::State,
                    if designation {
                        Expr::String(String::new())
                    } else {
                        state.initializer.clone()
                    },
                ),
                StateModelMember::Derived(derived) => {
                    (ModelMemberKind::Derived, derived.expression.clone())
                }
            };
            let value_type = resolved
'''
text = text.replace(old, new, 1)
text = text.replace(
    '''                value_type,
                expression,
''',
    '''                value_type,
                designation,
                expression,
''',
    1,
)
# Reject ordinary member reads at both static-root and model-parameter paths.
old = '''        let Some(member) = model.member(member_name) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    model.name, member_name
                ),
            ));
            return Expr::Integer(0);
        };
        return Expr::Name(model_binding_name(root, &member.name));
'''
new = '''        let Some(member) = model.member(member_name) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    model.name, member_name
                ),
            ));
            return Expr::Integer(0);
        };
        if member.designation {
            errors.push(diag(
                location,
                format!(
                    "model-local maybe live designation '{}.{}' is not an ordinary value; use it in a compatible designation context",
                    root, member_name
                ),
            ));
            return Expr::Integer(0);
        }
        return Expr::Name(model_binding_name(root, &member.name));
'''
if text.count(old) < 1:
    raise SystemExit("model_lowering static member read anchor missing")
text = text.replace(old, new, 1)
old = '''        let Some(member) = model.member(member_name) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    model.name, member_name
                ),
            ));
            return Expr::Integer(0);
        };

        return match member.kind {
'''
new = '''        let Some(member) = model.member(member_name) else {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    model.name, member_name
                ),
            ));
            return Expr::Integer(0);
        };
        if member.designation {
            errors.push(diag(
                location,
                format!(
                    "model-local maybe live designation '{}.{}' is not an ordinary value; use it in a compatible designation context",
                    root, member_name
                ),
            ));
            return Expr::Integer(0);
        }

        return match member.kind {
'''
text = text.replace(old, new, 1)
# Reject direct assignment and writable-state grants to designation members.
needle = '''            let Some(member) = model.member(member_name) else {
                errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no member '{}'",
                        model.name, member_name
                    ),
                ));
                return target.to_string();
            };
            return model_binding_name(root, &member.name);
'''
replacement = '''            let Some(member) = model.member(member_name) else {
                errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no member '{}'",
                        model.name, member_name
                    ),
                ));
                return target.to_string();
            };
            if member.designation {
                errors.push(diag(
                    location,
                    format!(
                        "model-local maybe live designation '{}.{}' requires designation assignment through a live owner designation",
                        root, member_name
                    ),
                ));
                return target.to_string();
            }
            return model_binding_name(root, &member.name);
'''
if text.count(needle) != 1:
    raise SystemExit("model_lowering static assignment anchor missing")
text = text.replace(needle, replacement, 1)
needle = '''            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!("cannot assign to derived member '{}.{}'", root, member_name),
                ));
                return target.to_string();
            }
            return parameter
'''
replacement = '''            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!("cannot assign to derived member '{}.{}'", root, member_name),
                ));
                return target.to_string();
            }
            if member.designation {
                errors.push(diag(
                    location,
                    format!(
                        "model-local maybe live designation '{}.{}' requires designation assignment through a live owner designation",
                        root, member_name
                    ),
                ));
                return target.to_string();
            }
            return parameter
'''
if text.count(needle) != 1:
    raise SystemExit("model_lowering parameter assignment anchor missing")
text = text.replace(needle, replacement, 1)
# Both state-grant paths share a derived-member check; add designation rejection after each.
old = '''            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!(
                        "cannot grant derived member '{}.{}' as writable state",
                        root, member_name
                    ),
                ));
                return name.to_string();
            }
'''
new = old + '''            if member.designation {
                errors.push(diag(
                    location,
                    format!(
                        "model-local maybe live designation '{}.{}' cannot be granted as ordinary writable state",
                        root, member_name
                    ),
                ));
                return name.to_string();
            }
'''
if text.count(old) != 2:
    raise SystemExit(f"model_lowering writable grant anchors expected 2, found {text.count(old)}")
text = text.replace(old, new, 2)
path.write_text(text)


# designation_runtime_metadata.rs: register static model-member designation slots.
path = Path("compiler/src/designation_runtime_metadata.rs")
text = path.read_text()
text = text.replace(
    "use crate::ast::{decode_live_type_name, decode_maybe_live_type_name, Declaration, Program};",
    "use crate::ast::{decode_live_type_name, decode_maybe_live_type_name, Declaration, Program};\nuse crate::runtime_model_templates::{RuntimeModelRoot, RuntimeModelTemplate};",
    1,
)
text += '''

pub(crate) fn collect_model_members(
    roots: &HashMap<String, RuntimeModelRoot>,
    templates: &HashMap<String, RuntimeModelTemplate>,
) -> HashMap<String, RuntimeDesignationMetadata> {
    let mut designations = HashMap::new();
    for root in roots.values() {
        let Some(template) = templates.get(&root.model_name) else {
            continue;
        };
        for member in &template.members {
            let Some(metadata) = &member.designation else {
                continue;
            };
            designations.insert(
                format!("__meld_sm${}${}", root.name, format!("${}", member.name)),
                metadata.clone(),
            );
        }
    }
    designations
}
'''
path.write_text(text)


# lib.rs: merge static model-member designation metadata into runtime metadata.
path = Path("compiler/src/lib.rs")
text = path.read_text()
old = '''    let runtime_designations = designation_runtime_metadata::collect(&runtime_reduction_lowered);

    let runtime_live_lowered = live_designation_lowering::lower(
'''
new = '''    let mut runtime_designations =
        designation_runtime_metadata::collect(&runtime_reduction_lowered);
    runtime_designations.extend(designation_runtime_metadata::collect_model_members(
        &runtime_model_roots,
        &runtime_model_templates,
    ));

    let runtime_live_lowered = live_designation_lowering::lower(
'''
if text.count(old) != 1:
    raise SystemExit("lib runtime designation collection anchor missing")
text = text.replace(old, new, 1)
path.write_text(text)


# runtime.rs: dynamic member state cells inherit designation role from templates.
path = Path("compiler/src/runtime.rs")
text = path.read_text()
old = '''            let cell = StateCell {
                value,
                value_type: member.value_type.clone(),
                designation: None,
                dependents: HashSet::new(),
            };
'''
new = '''            let cell = StateCell {
                value,
                value_type: member.value_type.clone(),
                designation: member.designation.clone(),
                dependents: HashSet::new(),
            };
'''
if text.count(old) != 1:
    raise SystemExit("runtime dynamic StateCell anchor missing")
text = text.replace(old, new, 1)
path.write_text(text)


# live_designation_lowering.rs: designation-context read/write without ordinary String projection.
path = Path("compiler/src/live_designation_lowering.rs")
text = path.read_text()
insert_anchor = '''fn designation_is_available(info: &DesignationInfo, declaration_index: usize) -> bool {
    info.kind == DesignationKind::Scoped || info.declaration_index < declaration_index
}
'''
helper = insert_anchor + '''

fn lower_model_local_designation_read(
    name: &str,
    expected: &DesignationInfo,
    context: &DesignationExprContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let (owner_name, member) = name.split_once('.')?;
    if member.contains('.') {
        return None;
    }
    let owner = context.designations.get(owner_name)?;
    if !designation_is_available(owner, context.declaration_index) {
        errors.push(diag(
            context.location,
            format!("live designation '{}' is not available before its declaration", owner_name),
        ));
        return Some(Expr::String(String::new()));
    }
    let template = context.templates.get(&owner.model_name)?;
    let member_template = template.member(member)?;
    let designation = member_template.designation.as_ref()?;
    if designation.model_name != expected.model_name {
        errors.push(diag(
            context.location,
            format!(
                "model-local designation '{}.{}' has type maybe live {} but '{}' requires live {}",
                owner_name, member, designation.model_name, expected.source_name, expected.model_name
            ),
        ));
        return Some(Expr::String(String::new()));
    }
    if designation.allows_none && !expected.allows_none {
        errors.push(diag(
            context.location,
            format!(
                "model-local maybe live {} designation '{}.{}' may be absent but '{}' requires plain live {}",
                designation.model_name, owner_name, member, expected.source_name, expected.model_name
            ),
        ));
        return Some(Expr::String(String::new()));
    }
    Some(Expr::RuntimeDesignationMember {
        designation: Box::new(Expr::Name(owner.lowered_name.clone())),
        member: member.to_string(),
        element_model: owner.model_name.clone(),
        member_type_name: "String".to_string(),
    })
}

fn lower_model_local_designation_assignment_value(
    expression: &Expr,
    target_model: &str,
    location: SourceLocation,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Expr::Name(name) = expression else {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {target_model} assignment requires a compatible persistent designation or 'none'"
            ),
        ));
        return Expr::String(String::new());
    };
    if name == "none" {
        return Expr::String(String::new());
    }
    let Some(source) = designations.get(name) else {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {target_model} assignment requires a compatible persistent designation or 'none'; '{name}' is not one"
            ),
        ));
        return Expr::String(String::new());
    };
    if !designation_is_available(source, declaration_index) {
        errors.push(diag(
            location,
            format!("live designation '{}' is not available before its declaration", name),
        ));
        return Expr::String(String::new());
    }
    if source.model_name != target_model {
        errors.push(diag(
            location,
            format!(
                "live designation '{}' has type live {} but model-local slot requires maybe live {}",
                source.source_name, source.model_name, target_model
            ),
        ));
        return Expr::String(String::new());
    }
    Expr::Name(source.lowered_name.clone())
}
'''
if text.count(insert_anchor) != 1:
    raise SystemExit("designation availability anchor missing")
text = text.replace(insert_anchor, helper, 1)
# Designation-context member read.
needle = '''        Expr::Name(name) => {
            if name == "none" {
'''
replacement = '''        Expr::Name(name) => {
            if name == "none" {
'''
# Insert after the none block by a later precise anchor.
if text.count(needle) != 1:
    raise SystemExit("lower_designation_expr Name anchor missing")
none_end = '''                return Expr::String(String::new());
            }

            if let Some(root_name) = decode_live_capture(name) {
'''
none_new = '''                return Expr::String(String::new());
            }

            if let Some(lowered) =
                lower_model_local_designation_read(name, expected, context, errors)
            {
                return lowered;
            }

            if let Some(root_name) = decode_live_capture(name) {
'''
if text.count(none_end) != 1:
    raise SystemExit("lower_designation_expr none-end anchor missing")
text = text.replace(none_end, none_new, 1)
# Ordinary member read rejects designation-valued member.
old = '''    let Some(member_template) = template.member(member) else {
        return Expr::Integer(0);
    };
    Expr::RuntimeDesignationMember {
'''
new = '''    let Some(member_template) = template.member(member) else {
        return Expr::Integer(0);
    };
    if let Some(designation) = &member_template.designation {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {} designation '{}.{}' is not an ordinary value; use it in a compatible designation context",
                designation.model_name, info.source_name, member
            ),
        ));
        return Expr::Integer(0);
    }
    Expr::RuntimeDesignationMember {
'''
if text.count(old) != 1:
    raise SystemExit("ordinary designation member read anchor missing")
text = text.replace(old, new, 1)
# Pass raw RHS into through lowering so designation values are not lowered as ordinary values first.
old = '''            if let Some(path) = decode_through_path(target) {
                let lowered_value = lower_expr(
                    value,
                    *location,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    errors,
                );
                return lower_through_assignment(
                    path,
                    *location,
                    *operator,
                    lowered_value,
                    designations,
                    models,
                    templates,
                    errors,
                );
            }
'''
new = '''            if let Some(path) = decode_through_path(target) {
                return lower_through_assignment(
                    path,
                    *location,
                    *operator,
                    value,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    errors,
                );
            }
'''
if text.count(old) != 1:
    raise SystemExit("through caller anchor missing")
text = text.replace(old, new, 1)
# Update lower_through signature raw Expr reference + declaration index.
text = text.replace(
    '''    operator: AssignmentOperator,
    value: Expr,
    designations: &HashMap<String, DesignationInfo>,
''',
    '''    operator: AssignmentOperator,
    value: &Expr,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
''',
    1,
)
# Fallback return values now clone raw source expression.
text = text.replace("            value,\n        };", "            value: value.clone(),\n        };", 4)
# After member template resolution, branch designation assignment; otherwise lower ordinary RHS.
old = '''    let Some(member_template) = template.member(member) else {
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value,
        };
    };
    Statement::RuntimeDesignationAssignment {
        location,
        designation: Box::new(Expr::Name(info.lowered_name.clone())),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
        operator,
        value,
    }
}
'''
new = '''    let Some(member_template) = template.member(member) else {
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };
    if let Some(designation) = &member_template.designation {
        if operator != AssignmentOperator::Assign {
            errors.push(diag(
                location,
                format!(
                    "model-local maybe live {} designation '{}.{}' supports selection assignment only",
                    designation.model_name, designation_name, member
                ),
            ));
        }
        let lowered_value = lower_model_local_designation_assignment_value(
            value,
            &designation.model_name,
            location,
            declaration_index,
            designations,
            errors,
        );
        return Statement::RuntimeDesignationAssignment {
            location,
            designation: Box::new(Expr::Name(info.lowered_name.clone())),
            member: member.to_string(),
            element_model: info.model_name.clone(),
            member_type_name: "String".to_string(),
            operator: AssignmentOperator::Assign,
            value: lowered_value,
        };
    }
    let lowered_value = lower_expr(
        value,
        location,
        declaration_index,
        designations,
        models,
        templates,
        errors,
    );
    Statement::RuntimeDesignationAssignment {
        location,
        designation: Box::new(Expr::Name(info.lowered_name.clone())),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
        operator,
        value: lowered_value,
    }
}
'''
if text.count(old) != 1:
    raise SystemExit("lower_through final anchor missing")
text = text.replace(old, new, 1)
path.write_text(text)
