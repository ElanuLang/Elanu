use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, decode_through_path, ActionArgument,
    ActionDecl, Declaration, Expr, Program, SourceLocation, StateDecl, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::MODEL_FILTER_BINDING_PREFIX;
use crate::model_sequence_integration::MODEL_SEQUENCE_BINDING_PREFIX;
use crate::runtime_model_templates::{RuntimeModelMemberKind, RuntimeModelTemplate};
use crate::runtime_sequence_markers::encode_runtime_sequence_type;
use crate::semantic::ValueType;
use crate::sequence_surface::{decode_sequence_index_segment, decode_sequence_live_type};

pub(crate) const RUNTIME_INDEX_GRANT_CARRIER_PREFIX: &str = "__meld_runtime_index_grant$";

#[derive(Debug, Clone)]
struct CarrierSpec {
    location: SourceLocation,
    name: String,
    initializer: Expr,
    member_type_name: String,
}

/// Preserve explicit `state through owner.sequence[index].member` authority
/// across the older sequence-lowering pipeline without forcing every earlier
/// pass to understand runtime-selected grants.
///
/// Each runtime-indexed grant is replaced temporarily by an ordinary StateGrant
/// to a compiler-private state carrier. The carrier's initializer is the
/// already-structured `RuntimeIndexMember` node, so the semantic facts are not
/// encoded into or recovered from the generated carrier name. The carrier also
/// gives runtime-sequence realization direct evidence that the source sequence
/// must use runtime-sized storage.
pub fn lower(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
) -> Result<Program, Vec<Diagnostic>> {
    let mut indexed_source_models = collect_model_sequence_types(program);
    indexed_source_models.extend(collect_filter_view_types(program));
    let designation_models = collect_persistent_designation_models(program);
    if indexed_source_models.is_empty() && designation_models.is_empty() {
        return Ok(program.clone());
    }

    let mut used_names = collect_declared_names(program);
    let mut next_id = 0usize;
    let mut carriers = Vec::new();
    let mut errors = Vec::new();
    let mut declarations = Vec::new();

    for declaration in &program.declarations {
        match declaration {
            Declaration::Action(action) => {
                let carrier_start = carriers.len();
                let lowered_action = ActionDecl {
                    location: action.location,
                    name: action.name.clone(),
                    parameters: action.parameters.clone(),
                    statements: lower_statements(
                        &action.statements,
                        &indexed_source_models,
                        &designation_models,
                        templates,
                        &mut used_names,
                        &mut next_id,
                        &mut carriers,
                        &mut errors,
                    ),
                };
                declarations.extend(carriers[carrier_start..].iter().map(carrier_declaration));
                declarations.push(Declaration::Action(lowered_action));
            }
            other => declarations.push(other.clone()),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(Program {
        declarations,
        state_models: program.state_models.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn lower_statements(
    statements: &[Statement],
    sequence_models: &HashMap<String, String>,
    designation_models: &HashMap<String, String>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    used_names: &mut HashSet<String>,
    next_id: &mut usize,
    carriers: &mut Vec<CarrierSpec>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<Statement> {
    statements
        .iter()
        .map(|statement| match statement {
            Statement::ActionCall {
                location,
                name,
                arguments,
            } => Statement::ActionCall {
                location: *location,
                name: name.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| {
                        lower_argument(
                            argument,
                            sequence_models,
                            designation_models,
                            templates,
                            used_names,
                            next_id,
                            carriers,
                            errors,
                        )
                    })
                    .collect(),
            },
            Statement::If {
                location,
                condition,
                then_branch,
                else_branch,
            } => Statement::If {
                location: *location,
                condition: condition.clone(),
                then_branch: lower_statements(
                    then_branch,
                    sequence_models,
                    designation_models,
                    templates,
                    used_names,
                    next_id,
                    carriers,
                    errors,
                ),
                else_branch: else_branch.as_ref().map(|branch| {
                    lower_statements(
                        branch,
                        sequence_models,
                        designation_models,
                        templates,
                        used_names,
                        next_id,
                        carriers,
                        errors,
                    )
                }),
            },
            other => other.clone(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn lower_argument(
    argument: &ActionArgument,
    sequence_models: &HashMap<String, String>,
    designation_models: &HashMap<String, String>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    used_names: &mut HashSet<String>,
    next_id: &mut usize,
    carriers: &mut Vec<CarrierSpec>,
    errors: &mut Vec<Diagnostic>,
) -> ActionArgument {
    let (location, initializer, member_type_name) = match argument {
        ActionArgument::IndexedStateGrant {
            location,
            source,
            index,
            member,
        } => {
            let Some((initializer, member_type_name)) = indexed_grant_expression(
                *location,
                source.clone(),
                index.clone(),
                member.clone(),
                sequence_models,
                templates,
                errors,
            ) else {
                return argument.clone();
            };
            (*location, initializer, member_type_name)
        }
        ActionArgument::StateGrant { location, name } => {
            if let Some((source, index, member)) = parse_runtime_index_grant(name, sequence_models)
            {
                let Some((initializer, member_type_name)) = indexed_grant_expression(
                    *location,
                    source,
                    Expr::Integer(index as i64),
                    member,
                    sequence_models,
                    templates,
                    errors,
                ) else {
                    return argument.clone();
                };
                (*location, initializer, member_type_name)
            } else if let Some((designation, owner_model, member)) =
                parse_designation_member_grant(name, designation_models, templates)
            {
                let member_template = templates
                    .get(&owner_model)
                    .and_then(|template| template.member(&member))
                    .expect("validated designation member grant should have a template member");
                if member_template.kind != RuntimeModelMemberKind::State {
                    errors.push(diag(
                        *location,
                        format!("cannot grant derived member '{name}' as writable state"),
                    ));
                    return argument.clone();
                }
                let member_type_name = runtime_value_type_name(&member_template.value_type);
                (
                    *location,
                    Expr::RuntimeDesignationMember {
                        designation: Box::new(Expr::Name(designation)),
                        member,
                        element_model: owner_model,
                        member_type_name: member_type_name.clone(),
                    },
                    member_type_name,
                )
            } else {
                return argument.clone();
            }
        }
        ActionArgument::Value(_) => return argument.clone(),
    };

    let carrier_name = fresh_carrier_name(used_names, next_id);
    carriers.push(CarrierSpec {
        location,
        name: carrier_name.clone(),
        initializer,
        member_type_name,
    });

    ActionArgument::StateGrant {
        location,
        name: carrier_name,
    }
}

fn indexed_grant_expression(
    location: SourceLocation,
    source: String,
    index: Expr,
    member: String,
    sequence_models: &HashMap<String, String>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    errors: &mut Vec<Diagnostic>,
) -> Option<(Expr, String)> {
    let element_model = sequence_models.get(&source)?.clone();
    let Some(template) = templates.get(&element_model) else {
        errors.push(diag(
            location,
            format!("missing runtime model template for '{element_model}'"),
        ));
        return None;
    };
    let Some(member_template) = template.member(&member) else {
        errors.push(diag(
            location,
            format!("state model '{element_model}' has no member '{member}'"),
        ));
        return None;
    };
    if member_template.kind != RuntimeModelMemberKind::State {
        errors.push(diag(
            location,
            format!("cannot grant derived member '{member}' as writable state"),
        ));
        return None;
    }
    let member_type_name = runtime_value_type_name(&member_template.value_type);
    Some((
        Expr::RuntimeIndexMember {
            source: Box::new(Expr::Name(source)),
            index: Box::new(index),
            member,
            element_model,
            member_type_name: member_type_name.clone(),
        },
        member_type_name,
    ))
}

fn carrier_declaration(spec: &CarrierSpec) -> Declaration {
    Declaration::State(StateDecl {
        location: spec.location,
        name: spec.name.clone(),
        type_name: Some(spec.member_type_name.clone()),
        initializer: spec.initializer.clone(),
        implicit_model_initializer: false,
    })
}

fn collect_persistent_designation_models(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            let type_name = state.type_name.as_deref()?;
            let model = decode_live_type_name(type_name)
                .or_else(|| decode_maybe_live_type_name(type_name))?;
            Some((state.name.clone(), model.to_string()))
        })
        .collect()
}

fn parse_designation_member_grant(
    name: &str,
    designation_models: &HashMap<String, String>,
    templates: &HashMap<String, RuntimeModelTemplate>,
) -> Option<(String, String, String)> {
    let name = decode_through_path(name).unwrap_or(name);
    let (designation, member) = name.split_once('.')?;
    if designation.is_empty() || member.is_empty() || member.contains('.') {
        return None;
    }
    let owner_model = designation_models.get(designation)?.clone();
    let member_template = templates.get(&owner_model)?.member(member)?;
    if member_template.kind != RuntimeModelMemberKind::State {
        return None;
    }
    Some((designation.to_string(), owner_model, member.to_string()))
}

fn collect_filter_view_types(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Derived(derived) = declaration else {
                return None;
            };
            if !derived.name.starts_with(MODEL_FILTER_BINDING_PREFIX) {
                return None;
            }
            let Expr::Filter {
                element_model: Some(element_model),
                ..
            } = &derived.expression
            else {
                return None;
            };
            Some((derived.name.clone(), element_model.clone()))
        })
        .collect()
}

fn collect_model_sequence_types(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            if !state.name.starts_with(MODEL_SEQUENCE_BINDING_PREFIX) {
                return None;
            }
            let model = state
                .type_name
                .as_deref()
                .and_then(decode_sequence_live_type)?;
            Some((state.name.clone(), model.to_string()))
        })
        .collect()
}

fn collect_declared_names(program: &Program) -> HashSet<String> {
    let mut names = HashSet::new();
    for declaration in &program.declarations {
        match declaration {
            Declaration::State(state) => {
                names.insert(state.name.clone());
            }
            Declaration::Derived(derived) => {
                names.insert(derived.name.clone());
            }
            Declaration::Action(action) => {
                names.insert(action.name.clone());
                names.extend(
                    action
                        .parameters
                        .iter()
                        .map(|parameter| parameter.name.clone()),
                );
            }
        }
    }
    names
}

fn parse_runtime_index_grant(
    name: &str,
    sequence_models: &HashMap<String, String>,
) -> Option<(String, usize, String)> {
    let path = decode_through_path(name)?;
    let mut segments = path.split('.');
    let source = segments.next()?;
    let index = decode_sequence_index_segment(segments.next()?)?;
    let member = segments.next()?;
    if segments.next().is_some()
        || !source.starts_with(MODEL_SEQUENCE_BINDING_PREFIX)
        || !sequence_models.contains_key(source)
        || member.is_empty()
    {
        return None;
    }
    Some((source.to_string(), index, member.to_string()))
}

fn fresh_carrier_name(used_names: &mut HashSet<String>, next_id: &mut usize) -> String {
    loop {
        let candidate = format!("{RUNTIME_INDEX_GRANT_CARRIER_PREFIX}{}", *next_id);
        *next_id += 1;
        if used_names.insert(candidate.clone()) {
            return candidate;
        }
    }
}

fn runtime_value_type_name(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Int => "Int".to_string(),
        ValueType::Float => "Float".to_string(),
        ValueType::Bool => "Bool".to_string(),
        ValueType::String => "String".to_string(),
        ValueType::SequenceLive(model) => encode_runtime_sequence_type(model),
        ValueType::Named(name) => name.clone(),
    }
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
