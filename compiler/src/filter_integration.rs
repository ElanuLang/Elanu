use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_through_path, ActionArgument, BinaryOperator, Declaration, DerivedDecl, Expr, Program,
    SourceLocation, StateModelDecl, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::model_types::{self, ModelTypeFacts, ModelTypeInfo};
use crate::program_facts::ProgramFacts;
use crate::reduction_surface::decode_reduction;
use crate::semantic::{binary_result_type, common_type, show_type, types_compatible, ValueType};
use crate::sequence_surface::{decode_sequence_index_segment, decode_sequence_live_type};

pub(crate) const MODEL_FILTER_BINDING_PREFIX: &str = "__meld_filter_member$";
const MODEL_SEQUENCE_BINDING_PREFIX: &str = "__meld_mseq$";
const MODEL_BINDING_PREFIX: &str = "__meld_sm$";

#[derive(Debug, Clone)]
struct FilterMemberSpec {
    member_name: String,
    source_member: String,
    element_model: String,
    element: String,
    predicate: Expr,
    order_by: Option<Expr>,
    order_descending: bool,
    owner_value_members: HashSet<String>,
    location: SourceLocation,
}

/// Validate and externalize model-local derived structural views.
///
/// Source parsing produces a real `Expr::Filter`. This pass keeps that one
/// structured AST representation: it resolves the source element model,
/// validates both membership and the optional narrow ordering key, externalizes
/// one view per concrete modeled-state root, and rewrites only owner-local reads
/// to their final hidden member bindings. No encoded String transport or
/// duplicate runtime sidecar is introduced.
pub(crate) fn lower(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    let facts = ProgramFacts::from_program(program);
    let type_facts = model_types::resolve(&program.state_models)?;
    let mut errors = Vec::new();

    reject_non_model_filters(program, &mut errors);
    let filters = collect_model_filters(program, &type_facts, &mut errors);
    reject_unsupported_filter_chaining(program, &filters, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }

    if filters.is_empty() {
        return Ok(program.clone());
    }

    let mut root_filters: HashMap<String, Vec<FilterMemberSpec>> = HashMap::new();
    for root in facts.modeled_state_roots() {
        if let Some(specs) = filters.get(&root.model_name) {
            root_filters.insert(root.name.clone(), specs.clone());
        }
    }

    let indexed_filter_sources: HashMap<String, String> = root_filters
        .iter()
        .flat_map(|(root, specs)| {
            specs.iter().map(move |spec| {
                (
                    format!("{root}.{}", spec.member_name),
                    model_filter_binding_name(root, &spec.member_name),
                )
            })
        })
        .collect();

    let state_models = strip_filter_members(&program.state_models);
    let mut declarations = Vec::new();

    for declaration in &program.declarations {
        declarations.push(rewrite_indexed_filter_reads_in_declaration(
            declaration,
            &indexed_filter_sources,
        ));

        let Declaration::State(state) = declaration else {
            continue;
        };
        if !state.implicit_model_initializer {
            continue;
        }
        let Some(specs) = root_filters.get(&state.name) else {
            continue;
        };

        for spec in specs {
            declarations.push(Declaration::Derived(DerivedDecl {
                location: spec.location,
                name: model_filter_binding_name(&state.name, &spec.member_name),
                expression: Expr::Filter {
                    source: Box::new(Expr::Name(model_sequence_binding_name(
                        &state.name,
                        &spec.source_member,
                    ))),
                    element: spec.element.clone(),
                    predicate: Box::new(lower_owner_names(
                        &spec.predicate,
                        &state.name,
                        &spec.owner_value_members,
                    )),
                    order_by: spec.order_by.as_ref().map(|order_by| {
                        Box::new(lower_owner_names(
                            order_by,
                            &state.name,
                            &spec.owner_value_members,
                        ))
                    }),
                    order_descending: spec.order_descending,
                    element_model: Some(spec.element_model.clone()),
                },
            }));
        }
    }

    Ok(Program {
        declarations,
        state_models,
    })
}

fn rewrite_indexed_filter_reads_in_declaration(
    declaration: &Declaration,
    indexed_filter_sources: &HashMap<String, String>,
) -> Declaration {
    match declaration {
        Declaration::State(state) => {
            let mut state = state.clone();
            state.initializer =
                rewrite_indexed_filter_reads(&state.initializer, indexed_filter_sources);
            Declaration::State(state)
        }
        Declaration::Derived(derived) => Declaration::Derived(DerivedDecl {
            location: derived.location,
            name: derived.name.clone(),
            expression: rewrite_indexed_filter_reads(&derived.expression, indexed_filter_sources),
        }),
        Declaration::Action(action) => {
            let mut action = action.clone();
            action.statements = action
                .statements
                .iter()
                .map(|statement| {
                    rewrite_indexed_filter_reads_in_statement(statement, indexed_filter_sources)
                })
                .collect();
            Declaration::Action(action)
        }
    }
}

fn rewrite_indexed_filter_reads_in_statement(
    statement: &Statement,
    indexed_filter_sources: &HashMap<String, String>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            if let Some((source, index, member)) =
                decode_static_filtered_through(target, indexed_filter_sources)
            {
                Statement::IndexedThroughAssignment {
                    location: *location,
                    source,
                    index: Expr::Integer(index as i64),
                    member,
                    operator: *operator,
                    value: rewrite_indexed_filter_reads(value, indexed_filter_sources),
                }
            } else {
                Statement::Assignment {
                    location: *location,
                    target: target.clone(),
                    operator: *operator,
                    value: rewrite_indexed_filter_reads(value, indexed_filter_sources),
                }
            }
        }
        Statement::IndexedThroughAssignment {
            location,
            source,
            index,
            member,
            operator,
            value,
        } => Statement::IndexedThroughAssignment {
            location: *location,
            source: indexed_filter_sources
                .get(source)
                .cloned()
                .unwrap_or_else(|| source.clone()),
            index: rewrite_indexed_filter_reads(index, indexed_filter_sources),
            member: member.clone(),
            operator: *operator,
            value: rewrite_indexed_filter_reads(value, indexed_filter_sources),
        },
        Statement::RuntimeIndexAssignment { .. }
        | Statement::RuntimeDesignationAssignment { .. } => statement.clone(),
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => Statement::ActionCall {
            location: *location,
            name: name.clone(),
            arguments: arguments
                .iter()
                .enumerate()
                .map(|(argument_index, argument)| {
                    if (name.starts_with(
                        crate::structural_edit_surface::GENERATED_FILTERED_REMOVE_ACTION_PREFIX,
                    ) || name.starts_with(
                        crate::structural_move_surface::GENERATED_FILTERED_MOVE_ACTION_PREFIX,
                    )) && argument_index == 1
                    {
                        if let ActionArgument::Value(Expr::Name(view_name)) = argument {
                            if let Some(lowered_view) = indexed_filter_sources.get(view_name) {
                                return ActionArgument::Value(Expr::Name(lowered_view.clone()));
                            }
                        }
                    }
                    match argument {
                        ActionArgument::Value(expression) => ActionArgument::Value(
                            rewrite_indexed_filter_reads(expression, indexed_filter_sources),
                        ),
                        ActionArgument::StateGrant { location, name } => {
                            if let Some((source, index, member)) =
                                decode_static_filtered_through(name, indexed_filter_sources)
                            {
                                ActionArgument::IndexedStateGrant {
                                    location: *location,
                                    source,
                                    index: Expr::Integer(index as i64),
                                    member,
                                }
                            } else {
                                argument.clone()
                            }
                        }
                        ActionArgument::IndexedStateGrant {
                            location,
                            source,
                            index,
                            member,
                        } => ActionArgument::IndexedStateGrant {
                            location: *location,
                            source: indexed_filter_sources
                                .get(source)
                                .cloned()
                                .unwrap_or_else(|| source.clone()),
                            index: rewrite_indexed_filter_reads(index, indexed_filter_sources),
                            member: member.clone(),
                        },
                    }
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: rewrite_indexed_filter_reads(message, indexed_filter_sources),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: rewrite_indexed_filter_reads(condition, indexed_filter_sources),
            then_branch: then_branch
                .iter()
                .map(|statement| {
                    rewrite_indexed_filter_reads_in_statement(statement, indexed_filter_sources)
                })
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| {
                        rewrite_indexed_filter_reads_in_statement(statement, indexed_filter_sources)
                    })
                    .collect()
            }),
        },
    }
}

fn decode_static_filtered_through(
    encoded_target: &str,
    indexed_filter_sources: &HashMap<String, String>,
) -> Option<(String, usize, String)> {
    decode_static_filtered_index_path(decode_through_path(encoded_target)?, indexed_filter_sources)
}

fn decode_static_filtered_index_path(
    path: &str,
    indexed_filter_sources: &HashMap<String, String>,
) -> Option<(String, usize, String)> {
    for (source, lowered_source) in indexed_filter_sources {
        let Some(suffix) = path
            .strip_prefix(source)
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        let mut parts = suffix.split('.');
        let Some(index_segment) = parts.next() else {
            continue;
        };
        let Some(index) = decode_sequence_index_segment(index_segment) else {
            continue;
        };
        let Some(member) = parts.next() else {
            continue;
        };
        if parts.next().is_none() {
            return Some((lowered_source.clone(), index, member.to_string()));
        }
    }
    None
}

fn decode_static_filtered_designation_path(
    path: &str,
    indexed_filter_sources: &HashMap<String, String>,
) -> Option<(String, usize)> {
    for (source, lowered_source) in indexed_filter_sources {
        let Some(suffix) = path
            .strip_prefix(source)
            .and_then(|rest| rest.strip_prefix('.'))
        else {
            continue;
        };
        let mut parts = suffix.split('.');
        let Some(index_segment) = parts.next() else {
            continue;
        };
        let Some(index) = decode_sequence_index_segment(index_segment) else {
            continue;
        };
        if parts.next().is_none() {
            return Some((lowered_source.clone(), index));
        }
    }
    None
}

fn rewrite_indexed_filter_reads(
    expression: &Expr,
    indexed_filter_sources: &HashMap<String, String>,
) -> Expr {
    match expression {
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => expression.clone(),
        Expr::Name(name) => {
            if let Some((source, index, member)) =
                decode_static_filtered_index_path(name, indexed_filter_sources)
            {
                Expr::IndexedMember {
                    source,
                    index: Box::new(Expr::Integer(index as i64)),
                    member,
                }
            } else if let Some((source, index)) =
                decode_static_filtered_designation_path(name, indexed_filter_sources)
            {
                Expr::IndexedDesignation {
                    source,
                    index: Box::new(Expr::Integer(index as i64)),
                }
            } else {
                expression.clone()
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } if matches!(
            *operator,
            BinaryOperator::IsIn | BinaryOperator::PreviousIn | BinaryOperator::NextIn
        ) =>
        {
            let right = match right.as_ref() {
                Expr::Name(name) => indexed_filter_sources
                    .get(name)
                    .cloned()
                    .map(Expr::Name)
                    .unwrap_or_else(|| rewrite_indexed_filter_reads(right, indexed_filter_sources)),
                _ => rewrite_indexed_filter_reads(right, indexed_filter_sources),
            };
            Expr::Binary {
                operator: *operator,
                left: Box::new(rewrite_indexed_filter_reads(left, indexed_filter_sources)),
                right: Box::new(right),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_indexed_filter_reads(left, indexed_filter_sources)),
            right: Box::new(rewrite_indexed_filter_reads(right, indexed_filter_sources)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_indexed_filter_reads(
                condition,
                indexed_filter_sources,
            )),
            then_branch: Box::new(rewrite_indexed_filter_reads(
                then_branch,
                indexed_filter_sources,
            )),
            else_branch: Box::new(rewrite_indexed_filter_reads(
                else_branch,
                indexed_filter_sources,
            )),
        },
        Expr::IndexedDesignation { source, index } => Expr::IndexedDesignation {
            source: indexed_filter_sources
                .get(source)
                .cloned()
                .unwrap_or_else(|| source.clone()),
            index: Box::new(rewrite_indexed_filter_reads(index, indexed_filter_sources)),
        },
        Expr::IndexedMember {
            source,
            index,
            member,
        } => Expr::IndexedMember {
            source: indexed_filter_sources
                .get(source)
                .cloned()
                .unwrap_or_else(|| source.clone()),
            index: Box::new(rewrite_indexed_filter_reads(index, indexed_filter_sources)),
            member: member.clone(),
        },
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => Expr::Filter {
            source: Box::new(rewrite_indexed_filter_reads(source, indexed_filter_sources)),
            element: element.clone(),
            predicate: Box::new(rewrite_indexed_filter_reads(
                predicate,
                indexed_filter_sources,
            )),
            order_by: order_by.as_ref().map(|order_by| {
                Box::new(rewrite_indexed_filter_reads(
                    order_by,
                    indexed_filter_sources,
                ))
            }),
            order_descending: *order_descending,
            element_model: element_model.clone(),
        },
    }
}

pub(crate) fn model_filter_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_FILTER_BINDING_PREFIX}{root}${member}")
}

fn model_sequence_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_SEQUENCE_BINDING_PREFIX}{root}${member}")
}

fn model_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_BINDING_PREFIX}{root}${member}")
}

fn collect_model_filters(
    program: &Program,
    type_facts: &ModelTypeFacts,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<FilterMemberSpec>> {
    let mut result = HashMap::new();

    for model in &program.state_models {
        let Some(owner_types) = type_facts.model(&model.name) else {
            continue;
        };

        for (member_index, member) in model.members.iter().enumerate() {
            let StateModelMember::Derived(derived) = member else {
                continue;
            };
            let Expr::Filter {
                source,
                element,
                predicate,
                order_by,
                order_descending,
                ..
            } = &derived.expression
            else {
                continue;
            };

            let Expr::Name(source_member) = source.as_ref() else {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local filter '{}.{}' source must be one local [live T] sequence member",
                        model.name, derived.name
                    ),
                ));
                continue;
            };
            if source_member.contains('.') {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local filter '{}.{}' must be rooted in one local sequence member",
                        model.name, derived.name
                    ),
                ));
                continue;
            }

            let Some(element_model) = earlier_sequence_source(model, member_index, source_member)
            else {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local filter '{}.{}' source '{}' is not an earlier local [live T] sequence state member",
                        model.name, derived.name, source_member
                    ),
                ));
                continue;
            };

            let Some(element_types) = type_facts.model(&element_model) else {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local filter '{}.{}' names unavailable element model '{}'",
                        model.name, derived.name, element_model
                    ),
                ));
                continue;
            };

            let owner_value_members = collect_owner_value_members(model, member_index, owner_types);
            let predicate_type = infer_predicate_type(
                predicate,
                element,
                &element_model,
                element_types,
                &model.name,
                owner_types,
                &owner_value_members,
                derived.location,
                errors,
            );
            if let Some(predicate_type) = predicate_type {
                if predicate_type != ValueType::Bool {
                    errors.push(diag(
                        derived.location,
                        format!(
                            "model-local filter '{}.{}' predicate must be Bool, got {}",
                            model.name,
                            derived.name,
                            show_type(&predicate_type)
                        ),
                    ));
                }
            }

            if let Some(order_by) = order_by.as_deref() {
                validate_order_key(
                    order_by,
                    element,
                    &element_model,
                    element_types,
                    &model.name,
                    &derived.name,
                    derived.location,
                    errors,
                );
            }

            result
                .entry(model.name.clone())
                .or_insert_with(Vec::new)
                .push(FilterMemberSpec {
                    member_name: derived.name.clone(),
                    source_member: source_member.clone(),
                    element_model,
                    element: element.clone(),
                    predicate: predicate.as_ref().clone(),
                    order_by: order_by.as_deref().cloned(),
                    order_descending: *order_descending,
                    owner_value_members,
                    location: derived.location,
                });
        }
    }

    result
}

#[allow(clippy::too_many_arguments)]
fn validate_order_key(
    expression: &Expr,
    element: &str,
    element_model: &str,
    element_types: &ModelTypeInfo,
    owner_model: &str,
    view_member: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    let Expr::Name(name) = expression else {
        errors.push(diag(
            location,
            format!(
                "model-local ordered view '{owner_model}.{view_member}' key must be one child Int member"
            ),
        ));
        return;
    };

    let prefix = format!("{element}.");
    let Some(member) = name.strip_prefix(&prefix) else {
        errors.push(diag(
            location,
            format!(
                "model-local ordered view '{owner_model}.{view_member}' key must read one member through element '{element}'"
            ),
        ));
        return;
    };
    if member.is_empty() || member.contains('.') {
        errors.push(diag(
            location,
            format!(
                "model-local ordered view '{owner_model}.{view_member}' key must select exactly one member of '{element_model}'"
            ),
        ));
        return;
    }

    match element_types.member_type(member) {
        Some(ValueType::Int) => {}
        Some(other) => errors.push(diag(
            location,
            format!(
                "model-local ordered view '{owner_model}.{view_member}' key '{name}' must be Int, got {}",
                show_type(other)
            ),
        )),
        None => errors.push(diag(
            location,
            format!("state model '{element_model}' has no member '{member}'"),
        )),
    }
}

fn earlier_sequence_source(
    model: &StateModelDecl,
    before_index: usize,
    source_name: &str,
) -> Option<String> {
    model
        .members
        .iter()
        .take(before_index)
        .find_map(|member| match member {
            StateModelMember::State(state) if state.name == source_name => state
                .type_name
                .as_deref()
                .and_then(decode_sequence_live_type)
                .map(str::to_string),
            _ => None,
        })
}

fn collect_owner_value_members(
    model: &StateModelDecl,
    before_index: usize,
    owner_types: &ModelTypeInfo,
) -> HashSet<String> {
    model
        .members
        .iter()
        .take(before_index)
        .filter_map(|member| {
            let name = member.name();
            match owner_types.member_type(name) {
                Some(ValueType::SequenceLive(_)) | None => None,
                Some(_) => Some(name.to_string()),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn infer_predicate_type(
    expression: &Expr,
    element: &str,
    element_model: &str,
    element_types: &ModelTypeInfo,
    owner_model: &str,
    owner_types: &ModelTypeInfo,
    owner_value_members: &HashSet<String>,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<ValueType> {
    match expression {
        Expr::Integer(_) => Some(ValueType::Int),
        Expr::Float(_) => Some(ValueType::Float),
        Expr::Bool(_) => Some(ValueType::Bool),
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is not available inside a filter predicate",
                ));
                None
            } else {
                Some(ValueType::String)
            }
        }
        Expr::Name(name) if name == element => {
            errors.push(diag(
                location,
                "reading the filter element as an ordinary whole live designation is deferred; read a member",
            ));
            None
        }
        Expr::Name(name) => {
            let prefix = format!("{element}.");
            if let Some(member) = name.strip_prefix(&prefix) {
                if member.is_empty() || member.contains('.') {
                    errors.push(diag(
                        location,
                        "filter element read must select exactly one modeled-state member in the first spike",
                    ));
                    return None;
                }
                return match element_types.member_type(member) {
                    Some(value_type) => Some(value_type.clone()),
                    None => {
                        errors.push(diag(
                            location,
                            format!("state model '{element_model}' has no member '{member}'"),
                        ));
                        None
                    }
                };
            }

            if name.contains('.') {
                errors.push(diag(
                    location,
                    format!(
                        "rooted model-local filter predicate may not capture external path '{name}'"
                    ),
                ));
                return None;
            }

            if owner_value_members.contains(name) {
                return owner_types.member_type(name).cloned();
            }

            errors.push(diag(
                location,
                format!(
                    "rooted model-local filter predicate in '{}' may reference members reached through element '{}' or an earlier ordinary owner member; '{}' is not available in this scope",
                    owner_model, element, name
                ),
            ));
            None
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => {
            let left = infer_predicate_type(
                left,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            let right = infer_predicate_type(
                right,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            match (left, right) {
                (Some(left), Some(right)) => {
                    let result = binary_result_type(*operator, &left, &right);
                    if result.is_none() {
                        errors.push(diag(
                            location,
                            format!(
                                "operator {:?} is not defined for filter predicate types {} and {}",
                                operator,
                                show_type(&left),
                                show_type(&right)
                            ),
                        ));
                    }
                    result
                }
                _ => None,
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let condition_type = infer_predicate_type(
                condition,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            if let Some(condition_type) = condition_type {
                if condition_type != ValueType::Bool {
                    errors.push(diag(
                        location,
                        format!(
                            "filter predicate if condition must be Bool, got {}",
                            show_type(&condition_type)
                        ),
                    ));
                }
            }
            let then_type = infer_predicate_type(
                then_branch,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            let else_type = infer_predicate_type(
                else_branch,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            match (then_type, else_type) {
                (Some(a), Some(b)) if types_compatible(&a, &b) => Some(common_type(a, b)),
                (Some(a), Some(b)) => {
                    errors.push(diag(
                        location,
                        format!(
                            "filter predicate if branches have incompatible types {} and {}",
                            show_type(&a),
                            show_type(&b)
                        ),
                    ));
                    None
                }
                _ => None,
            }
        }
        Expr::Filter { .. } => {
            errors.push(diag(
                location,
                "nested filter expressions are deferred in the first derived-view spike",
            ));
            None
        }
        Expr::IndexedMember { index, .. } => {
            let _ = infer_predicate_type(
                index,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            errors.push(diag(
                location,
                "runtime indexed member access is not yet available inside a filter predicate",
            ));
            None
        }
        Expr::IndexedDesignation { index, .. } => {
            let _ = infer_predicate_type(
                index,
                element,
                element_model,
                element_types,
                owner_model,
                owner_types,
                owner_value_members,
                location,
                errors,
            );
            errors.push(diag(
                location,
                "whole live designation selection is not available inside a filter predicate",
            ));
            None
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
    }
}

fn lower_owner_names(expression: &Expr, root: &str, owner_value_members: &HashSet<String>) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) if owner_value_members.contains(name) => {
            Expr::Name(model_binding_name(root, name))
        }
        Expr::Name(name) => Expr::Name(name.clone()),
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(lower_owner_names(left, root, owner_value_members)),
            right: Box::new(lower_owner_names(right, root, owner_value_members)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_owner_names(condition, root, owner_value_members)),
            then_branch: Box::new(lower_owner_names(then_branch, root, owner_value_members)),
            else_branch: Box::new(lower_owner_names(else_branch, root, owner_value_members)),
        },
        Expr::Filter { .. } => unreachable!("nested filters were rejected during validation"),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("indexed member access was rejected during filter predicate validation")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
    }
}

fn strip_filter_members(models: &[StateModelDecl]) -> Vec<StateModelDecl> {
    models
        .iter()
        .map(|model| StateModelDecl {
            location: model.location,
            name: model.name.clone(),
            members: model
                .members
                .iter()
                .filter(|member| {
                    !matches!(
                        member,
                        StateModelMember::Derived(derived)
                            if matches!(derived.expression, Expr::Filter { .. })
                    )
                })
                .cloned()
                .collect(),
        })
        .collect()
}

fn reject_non_model_filters(program: &Program, errors: &mut Vec<Diagnostic>) {
    for declaration in &program.declarations {
        let (expression, location) = match declaration {
            Declaration::State(state) => (&state.initializer, state.location),
            Declaration::Derived(derived) => (&derived.expression, derived.location),
            Declaration::Action(action) => {
                if action.statements.iter().any(statement_contains_filter) {
                    errors.push(diag(
                        action.location,
                        "the first filter spike supports only model-local derived filter views",
                    ));
                }
                continue;
            }
        };
        if expr_contains_filter(expression) {
            errors.push(diag(
                location,
                "the first filter spike supports only model-local derived filter views",
            ));
        }
    }

    for model in &program.state_models {
        for member in &model.members {
            match member {
                StateModelMember::State(state) if expr_contains_filter(&state.initializer) => {
                    errors.push(diag(
                        state.location,
                        "filter is available only as the direct expression of a model-local derived member in the first spike",
                    ));
                }
                StateModelMember::Derived(derived)
                    if !matches!(derived.expression, Expr::Filter { .. })
                        && expr_contains_filter(&derived.expression) =>
                {
                    errors.push(diag(
                        derived.location,
                        "nested filter expressions are deferred in the first derived-view spike",
                    ));
                }
                _ => {}
            }
        }
    }
}

fn reject_unsupported_filter_chaining(
    program: &Program,
    filters: &HashMap<String, Vec<FilterMemberSpec>>,
    errors: &mut Vec<Diagnostic>,
) {
    for model in &program.state_models {
        let Some(specs) = filters.get(&model.name) else {
            continue;
        };
        let filter_names: HashSet<&str> =
            specs.iter().map(|spec| spec.member_name.as_str()).collect();

        for member in &model.members {
            if filter_names.contains(member.name()) {
                continue;
            }
            let expression = match member {
                StateModelMember::State(state) => &state.initializer,
                StateModelMember::Derived(derived) => &derived.expression,
            };
            if expression_mentions_any(expression, &filter_names) {
                errors.push(diag(
                    member.location(),
                    format!(
                        "the first derived-view spike defers model-local chaining through a filter view in '{}.{}'; only downstream read-only reduction is currently supported",
                        model.name,
                        member.name()
                    ),
                ));
            }
        }
    }
}

fn expression_mentions_any(expression: &Expr, names: &HashSet<&str>) -> bool {
    match expression {
        Expr::Name(name) => names.contains(name.as_str()),
        Expr::Binary { left, right, .. } => {
            expression_mentions_any(left, names) || expression_mentions_any(right, names)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expression_mentions_any(condition, names)
                || expression_mentions_any(then_branch, names)
                || expression_mentions_any(else_branch, names)
        }
        Expr::Filter {
            source,
            predicate,
            order_by,
            ..
        } => {
            expression_mentions_any(source, names)
                || expression_mentions_any(predicate, names)
                || order_by
                    .as_deref()
                    .is_some_and(|order_by| expression_mentions_any(order_by, names))
        }
        Expr::IndexedMember { source, index, .. } | Expr::IndexedDesignation { source, index } => {
            names.contains(source.as_str()) || expression_mentions_any(index, names)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => false,
    }
}

fn expr_contains_filter(expression: &Expr) -> bool {
    match expression {
        Expr::Filter { .. } => true,
        Expr::Binary { left, right, .. } => {
            expr_contains_filter(left) || expr_contains_filter(right)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_contains_filter(condition)
                || expr_contains_filter(then_branch)
                || expr_contains_filter(else_branch)
        }
        Expr::IndexedMember { index, .. } | Expr::IndexedDesignation { index, .. } => {
            expr_contains_filter(index)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) | Expr::Name(_) => {
            false
        }
    }
}

fn statement_contains_filter(statement: &Statement) -> bool {
    match statement {
        Statement::Assignment { value, .. } => expr_contains_filter(value),
        Statement::IndexedThroughAssignment { index, value, .. } => {
            expr_contains_filter(index) || expr_contains_filter(value)
        }
        Statement::RuntimeIndexAssignment { .. }
        | Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime index lowering runs after filter integration")
        }
        Statement::ActionCall { arguments, .. } => {
            arguments.iter().any(|argument| match argument {
                crate::ast::ActionArgument::Value(value) => expr_contains_filter(value),
                crate::ast::ActionArgument::StateGrant { .. } => false,
                crate::ast::ActionArgument::IndexedStateGrant { index, .. } => {
                    expr_contains_filter(index)
                }
            })
        }
        Statement::Fail { message, .. } => expr_contains_filter(message),
        Statement::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            expr_contains_filter(condition)
                || then_branch.iter().any(statement_contains_filter)
                || else_branch
                    .as_ref()
                    .is_some_and(|branch| branch.iter().any(statement_contains_filter))
        }
    }
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
