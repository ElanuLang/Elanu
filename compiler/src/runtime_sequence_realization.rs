use std::collections::HashMap;

use crate::ast::{
    ActionArgument, ActionDecl, BinaryOperator, Declaration, DerivedDecl, Expr, Program,
    SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::MODEL_FILTER_BINDING_PREFIX;
use crate::model_sequence_integration::ExternalizedModelSequence;
use crate::reduction_surface::{decode_reduction, parse_expression_fragment, ReductionSpec};
use crate::runtime_sequence_markers::{
    decode_runtime_sequence_type, encode_runtime_sequence_type, encode_runtime_sequence_value,
};
use crate::semantic::{show_type, RuntimeReductionPayload, ValueType};

const MODEL_BINDING_PREFIX: &str = "__meld_sm$";
const LOWERED_SEQUENCE_VALUE_PREFIX: &str = "__meld_sequence_value$";

#[derive(Debug, Clone)]
struct RuntimeSequenceInfo {
    model_name: String,
}

#[derive(Debug, Clone)]
pub struct RealizedProgram {
    pub program: Program,
    pub runtime_reductions: HashMap<String, RuntimeReductionPayload>,
}

pub fn lower(
    program: &Program,
    runtime_reduction_types: &HashMap<String, ValueType>,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
) -> Result<RealizedProgram, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let sequence_infos = collect_runtime_sequences(program, externalized_sequences, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut runtime_reductions = HashMap::new();
    let declarations = program
        .declarations
        .iter()
        .map(|declaration| {
            lower_declaration(
                declaration,
                &sequence_infos,
                runtime_reduction_types,
                externalized_sequences,
                &mut runtime_reductions,
                &mut errors,
            )
        })
        .collect();

    if errors.is_empty() {
        Ok(RealizedProgram {
            program: Program {
                declarations,
                state_models: program.state_models.clone(),
            },
            runtime_reductions,
        })
    } else {
        Err(errors)
    }
}

fn collect_runtime_sequences(
    program: &Program,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, RuntimeSequenceInfo> {
    let mut result: HashMap<String, RuntimeSequenceInfo> = HashMap::new();
    let insert_actions: HashMap<String, String> = program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Action(action) = declaration else {
                return None;
            };
            if !action
                .name
                .starts_with(crate::scoped_create_surface::GENERATED_INSERT_ACTION_PREFIX)
            {
                return None;
            }
            let target = action.parameters.get(1)?;
            let model = decode_runtime_sequence_type(&target.type_name)?;
            Some((action.name.clone(), model.to_string()))
        })
        .collect();
    let remove_actions: HashMap<String, String> = program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::Action(action) = declaration else {
                return None;
            };
            if !action
                .name
                .starts_with(crate::structural_edit_surface::GENERATED_REMOVE_ACTION_PREFIX)
                && !action.name.starts_with(
                    crate::structural_edit_surface::GENERATED_FILTERED_REMOVE_ACTION_PREFIX,
                )
                && !action
                    .name
                    .starts_with(crate::structural_move_surface::GENERATED_MOVE_ACTION_PREFIX)
                && !action.name.starts_with(
                    crate::structural_move_surface::GENERATED_FILTERED_MOVE_ACTION_PREFIX,
                )
            {
                return None;
            }
            let target = action.parameters.first()?;
            let model = decode_runtime_sequence_type(&target.type_name)?;
            Some((action.name.clone(), model.to_string()))
        })
        .collect();

    for declaration in &program.declarations {
        match declaration {
            Declaration::Derived(derived) => match &derived.expression {
                Expr::Filter {
                    source,
                    element_model,
                    ..
                } => {
                    let Expr::Name(source) = source.as_ref() else {
                        errors.push(diag(
                            derived.location,
                            "internal runtime filter source is not a named sequence",
                        ));
                        continue;
                    };
                    if !externalized_sequences.contains_key(source) {
                        continue;
                    }
                    let Some(model_name) = element_model.as_deref() else {
                        errors.push(diag(
                            derived.location,
                            "internal runtime filter is missing its live element model",
                        ));
                        continue;
                    };
                    insert_runtime_sequence(
                        &mut result,
                        source,
                        model_name,
                        derived.location,
                        errors,
                    );
                }
                Expr::String(value) => {
                    let Some(reduction) = decode_reduction(value) else {
                        continue;
                    };
                    if !externalized_sequences.contains_key(&reduction.source) {
                        continue;
                    }
                    let Some(model_name) = reduction.element_model.as_deref() else {
                        errors.push(diag(
                            derived.location,
                            "internal owner-relative reduction is missing its live element model",
                        ));
                        continue;
                    };
                    insert_runtime_sequence(
                        &mut result,
                        &reduction.source,
                        model_name,
                        derived.location,
                        errors,
                    );
                }
                other => {
                    collect_runtime_index_sequences_from_expr(
                        other,
                        derived.location,
                        &mut result,
                        errors,
                    );
                }
            },
            Declaration::Action(action) => collect_mutation_sequences_from_statements(
                &action.statements,
                &insert_actions,
                &remove_actions,
                &mut result,
                errors,
            ),
            Declaration::State(state) => {
                collect_runtime_index_sequences_from_expr(
                    &state.initializer,
                    state.location,
                    &mut result,
                    errors,
                );
            }
        }
    }
    result
}

fn collect_mutation_sequences_from_statements(
    statements: &[Statement],
    insert_actions: &HashMap<String, String>,
    remove_actions: &HashMap<String, String>,
    result: &mut HashMap<String, RuntimeSequenceInfo>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::ActionCall {
                location,
                name,
                arguments,
            } => {
                if let Some(model_name) = insert_actions.get(name) {
                    let Some(ActionArgument::StateGrant { name: target, .. }) = arguments.get(1)
                    else {
                        errors.push(diag(
                            *location,
                            "internal scoped insertion call is missing its sequence authority target",
                        ));
                        continue;
                    };
                    if !target.starts_with(
                        crate::runtime_index_grant_transport::RUNTIME_INDEX_GRANT_CARRIER_PREFIX,
                    ) {
                        insert_runtime_sequence(result, target, model_name, *location, errors);
                    }
                    continue;
                }

                if let Some(model_name) = remove_actions.get(name) {
                    let Some(ActionArgument::StateGrant { name: target, .. }) = arguments.first()
                    else {
                        errors.push(diag(
                            *location,
                            "internal structural removal call is missing its sequence authority target",
                        ));
                        continue;
                    };
                    if !target.starts_with(
                        crate::runtime_index_grant_transport::RUNTIME_INDEX_GRANT_CARRIER_PREFIX,
                    ) {
                        insert_runtime_sequence(result, target, model_name, *location, errors);
                    }
                }
            }
            Statement::IndexedThroughAssignment { .. } => {
                unreachable!("source indexed assignments are consumed by sequence lowering")
            }
            Statement::RuntimeIndexAssignment {
                location,
                source,
                element_model,
                value,
                ..
            } => {
                insert_runtime_sequence(result, source, element_model, *location, errors);
                collect_runtime_index_sequences_from_expr(value, *location, result, errors);
            }
            Statement::RuntimeDesignationAssignment {
                location,
                designation,
                value,
                ..
            } => {
                collect_runtime_index_sequences_from_expr(designation, *location, result, errors);
                collect_runtime_index_sequences_from_expr(value, *location, result, errors);
            }
            Statement::Assignment {
                location, value, ..
            } => {
                collect_runtime_index_sequences_from_expr(value, *location, result, errors);
            }
            Statement::Fail { location, message } => {
                collect_runtime_index_sequences_from_expr(message, *location, result, errors);
            }
            Statement::If {
                location,
                condition,
                then_branch,
                else_branch,
            } => {
                collect_runtime_index_sequences_from_expr(condition, *location, result, errors);
                collect_mutation_sequences_from_statements(
                    then_branch,
                    insert_actions,
                    remove_actions,
                    result,
                    errors,
                );
                if let Some(branch) = else_branch {
                    collect_mutation_sequences_from_statements(
                        branch,
                        insert_actions,
                        remove_actions,
                        result,
                        errors,
                    );
                }
            }
        }
    }
}

fn collect_runtime_index_sequences_from_expr(
    expression: &Expr,
    location: SourceLocation,
    result: &mut HashMap<String, RuntimeSequenceInfo>,
    errors: &mut Vec<Diagnostic>,
) {
    match expression {
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember {
            source,
            element_model,
            ..
        }
        | Expr::RuntimeIndexDesignation {
            source,
            element_model,
            ..
        } => {
            if let Expr::Name(source) = source.as_ref() {
                insert_runtime_sequence(result, source, element_model, location, errors);
            } else {
                collect_runtime_index_sequences_from_expr(source, location, result, errors);
            }
        }
        Expr::RuntimeDesignationMember { designation, .. } => {
            collect_runtime_index_sequences_from_expr(designation, location, result, errors);
        }
        Expr::Binary { left, right, .. } => {
            collect_runtime_index_sequences_from_expr(left, location, result, errors);
            collect_runtime_index_sequences_from_expr(right, location, result, errors);
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_runtime_index_sequences_from_expr(condition, location, result, errors);
            collect_runtime_index_sequences_from_expr(then_branch, location, result, errors);
            collect_runtime_index_sequences_from_expr(else_branch, location, result, errors);
        }
        Expr::Filter {
            source, predicate, ..
        } => {
            collect_runtime_index_sequences_from_expr(source, location, result, errors);
            collect_runtime_index_sequences_from_expr(predicate, location, result, errors);
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) | Expr::Name(_) => {}
    }
}

fn insert_runtime_sequence(
    result: &mut HashMap<String, RuntimeSequenceInfo>,
    source: &str,
    model_name: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    match result.get(source) {
        Some(existing) if existing.model_name != model_name => errors.push(diag(
            location,
            format!(
                "internal sequence '{}' is used as both live {} and live {}",
                source, existing.model_name, model_name
            ),
        )),
        _ => {
            result.insert(
                source.to_string(),
                RuntimeSequenceInfo {
                    model_name: model_name.to_string(),
                },
            );
        }
    }
}

fn lower_declaration(
    declaration: &Declaration,
    sequences: &HashMap<String, RuntimeSequenceInfo>,
    runtime_reduction_types: &HashMap<String, ValueType>,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
    runtime_reductions: &mut HashMap<String, RuntimeReductionPayload>,
    errors: &mut Vec<Diagnostic>,
) -> Declaration {
    match declaration {
        Declaration::State(state) => {
            let mut state = state.clone();
            if let Some(info) = sequences.get(&state.name) {
                state.type_name = Some(encode_runtime_sequence_type(&info.model_name));
                state.initializer = lower_sequence_value_for(
                    &state.initializer,
                    &info.model_name,
                    state.location,
                    errors,
                );
            } else {
                state.initializer = lower_expr(&state.initializer, sequences, errors);
            }
            Declaration::State(state)
        }
        Declaration::Derived(derived) => {
            let expression = if let Expr::String(value) = &derived.expression {
                if let Some(reduction) = decode_reduction(value) {
                    if is_runtime_reduction_source(&reduction.source, externalized_sequences) {
                        lower_runtime_reduction(
                            derived,
                            reduction,
                            runtime_reduction_types,
                            externalized_sequences,
                            runtime_reductions,
                            errors,
                        )
                    } else {
                        lower_expr(&derived.expression, sequences, errors)
                    }
                } else {
                    lower_expr(&derived.expression, sequences, errors)
                }
            } else {
                lower_expr(&derived.expression, sequences, errors)
            };
            Declaration::Derived(DerivedDecl {
                location: derived.location,
                name: derived.name.clone(),
                expression,
            })
        }
        Declaration::Action(action) => Declaration::Action(ActionDecl {
            location: action.location,
            name: action.name.clone(),
            parameters: action.parameters.clone(),
            statements: action
                .statements
                .iter()
                .map(|statement| lower_statement(statement, sequences, errors))
                .collect(),
        }),
    }
}

fn is_runtime_reduction_source(
    source: &str,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
) -> bool {
    externalized_sequences.contains_key(source) || source.starts_with(MODEL_FILTER_BINDING_PREFIX)
}

fn lower_runtime_reduction(
    derived: &DerivedDecl,
    reduction: ReductionSpec,
    runtime_reduction_types: &HashMap<String, ValueType>,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
    runtime_reductions: &mut HashMap<String, RuntimeReductionPayload>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Some(result_type) = runtime_reduction_types.get(&derived.name) else {
        errors.push(diag(
            derived.location,
            format!(
                "direct validation did not produce a type for runtime reduction '{}'",
                derived.name
            ),
        ));
        return Expr::Integer(0);
    };
    if matches!(
        result_type,
        ValueType::Named(_) | ValueType::SequenceLive(_)
    ) {
        errors.push(diag(
            derived.location,
            format!(
                "runtime reduction '{}' has unsupported bootstrap result type {}",
                derived.name,
                show_type(result_type)
            ),
        ));
        return Expr::Integer(0);
    }

    let owner_root = runtime_reduction_owner_root(&reduction.source, externalized_sequences);
    let initial = match parse_expression_fragment(&reduction.initial_source) {
        Ok(initial) => rewrite_runtime_reduction_owner_names(
            &initial,
            &reduction,
            false,
            owner_root.as_deref(),
        ),
        Err(parse_errors) => {
            errors.push(diag(
                derived.location,
                format!(
                    "invalid internal runtime reduction initial expression for '{}': {}",
                    derived.name,
                    first_diagnostic_message(&parse_errors)
                ),
            ));
            return Expr::Integer(0);
        }
    };
    let step = match parse_expression_fragment(&reduction.step_source) {
        Ok(step) => {
            rewrite_runtime_reduction_owner_names(&step, &reduction, true, owner_root.as_deref())
        }
        Err(parse_errors) => {
            errors.push(diag(
                derived.location,
                format!(
                    "invalid internal runtime reduction step expression for '{}': {}",
                    derived.name,
                    first_diagnostic_message(&parse_errors)
                ),
            ));
            return Expr::Integer(0);
        }
    };

    runtime_reductions.insert(
        derived.name.clone(),
        RuntimeReductionPayload {
            reduction: reduction.clone(),
            result_type: result_type.clone(),
            initial,
            step,
        },
    );

    match result_type {
        ValueType::Int => Expr::Integer(0),
        ValueType::Float => Expr::Float(0.0),
        ValueType::Bool => Expr::Bool(false),
        ValueType::String => Expr::String(String::new()),
        ValueType::Named(_) | ValueType::SequenceLive(_) => {
            unreachable!("unsupported runtime reduction result type was rejected above")
        }
    }
}

fn lower_statement(
    statement: &Statement,
    sequences: &HashMap<String, RuntimeSequenceInfo>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            let value = match sequences.get(target) {
                Some(info) => lower_sequence_value_for(value, &info.model_name, *location, errors),
                None => lower_expr(value, sequences, errors),
            };
            Statement::Assignment {
                location: *location,
                target: target.clone(),
                operator: *operator,
                value,
            }
        }
        Statement::IndexedThroughAssignment { .. } => {
            unreachable!("source indexed assignments are consumed by sequence lowering")
        }
        Statement::RuntimeIndexAssignment {
            location,
            source,
            index,
            member,
            element_model,
            member_type_name,
            operator,
            value,
        } => Statement::RuntimeIndexAssignment {
            location: *location,
            source: source.clone(),
            index: index.clone(),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
            operator: *operator,
            value: lower_expr(value, sequences, errors),
        },
        Statement::RuntimeDesignationAssignment {
            location,
            designation,
            member,
            element_model,
            member_type_name,
            operator,
            value,
        } => Statement::RuntimeDesignationAssignment {
            location: *location,
            designation: Box::new(lower_expr(designation, sequences, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
            operator: *operator,
            value: lower_expr(value, sequences, errors),
        },
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => Statement::ActionCall {
            location: *location,
            name: name.clone(),
            arguments: arguments
                .iter()
                .map(|argument| match argument {
                    ActionArgument::Value(value) => {
                        ActionArgument::Value(lower_expr(value, sequences, errors))
                    }
                    ActionArgument::IndexedStateGrant { .. } => unreachable!(
                        "source indexed state grants are consumed by runtime index grant transport"
                    ),
                    ActionArgument::StateGrant { location, name } => ActionArgument::StateGrant {
                        location: *location,
                        name: name.clone(),
                    },
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: lower_expr(message, sequences, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: lower_expr(condition, sequences, errors),
            then_branch: then_branch
                .iter()
                .map(|statement| lower_statement(statement, sequences, errors))
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| lower_statement(statement, sequences, errors))
                    .collect()
            }),
        },
    }
}

fn lower_expr(
    expression: &Expr,
    sequences: &HashMap<String, RuntimeSequenceInfo>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => Expr::Name(name.clone()),
        Expr::Binary {
            operator,
            left,
            right,
        } => {
            let mut left = lower_expr(left, sequences, errors);
            let mut right = lower_expr(right, sequences, errors);
            if matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual) {
                lower_sequence_comparison_side(&mut left, &mut right, sequences, errors);
                lower_sequence_comparison_side(&mut right, &mut left, sequences, errors);
            }
            Expr::Binary {
                operator: *operator,
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(condition, sequences, errors)),
            then_branch: Box::new(lower_expr(then_branch, sequences, errors)),
            else_branch: Box::new(lower_expr(else_branch, sequences, errors)),
        },
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember {
            source,
            index,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeIndexMember {
            source: Box::new(lower_expr(source, sequences, errors)),
            index: Box::new(lower_expr(index, sequences, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => Expr::RuntimeIndexDesignation {
            source: Box::new(lower_expr(source, sequences, errors)),
            index: Box::new(lower_expr(index, sequences, errors)),
            element_model: element_model.clone(),
        },
        Expr::RuntimeDesignationMember {
            designation,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeDesignationMember {
            designation: Box::new(lower_expr(designation, sequences, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => Expr::Filter {
            source: source.clone(),
            element: element.clone(),
            predicate: predicate.clone(),
            order_by: order_by.clone(),
            order_descending: *order_descending,
            element_model: element_model.clone(),
        },
    }
}

fn lower_sequence_comparison_side(
    value_side: &mut Expr,
    name_side: &mut Expr,
    sequences: &HashMap<String, RuntimeSequenceInfo>,
    errors: &mut Vec<Diagnostic>,
) {
    let Expr::Name(source) = name_side else {
        return;
    };
    let Some(info) = sequences.get(source) else {
        return;
    };
    let Expr::String(value) = value_side else {
        return;
    };
    let Some(targets) = decode_lowered_sequence_value(value) else {
        return;
    };
    *value_side = Expr::String(encode_runtime_sequence_value(&info.model_name, &targets));
    let _ = errors;
}

fn lower_sequence_value_for(
    expression: &Expr,
    model_name: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Expr::String(value) = expression else {
        errors.push(diag(
            location,
            "internal runtime sequence state expected a lowered sequence literal value",
        ));
        return expression.clone();
    };
    let Some(targets) = decode_lowered_sequence_value(value) else {
        errors.push(diag(
            location,
            "internal runtime sequence state value is not a lowered sequence literal",
        ));
        return expression.clone();
    };
    Expr::String(encode_runtime_sequence_value(model_name, &targets))
}

fn rewrite_runtime_reduction_owner_names(
    expression: &Expr,
    reduction: &ReductionSpec,
    bindings_in_scope: bool,
    owner_root: Option<&str>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            if bindings_in_scope {
                let element_prefix = format!("{}.", reduction.element);
                if name == &reduction.accumulator
                    || name == &reduction.element
                    || name.starts_with(&element_prefix)
                {
                    return Expr::Name(name.clone());
                }
            }
            match owner_root {
                Some(root) if !name.contains('.') => Expr::Name(model_binding_name(root, name)),
                _ => Expr::Name(name.clone()),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_runtime_reduction_owner_names(
                left,
                reduction,
                bindings_in_scope,
                owner_root,
            )),
            right: Box::new(rewrite_runtime_reduction_owner_names(
                right,
                reduction,
                bindings_in_scope,
                owner_root,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_runtime_reduction_owner_names(
                condition,
                reduction,
                bindings_in_scope,
                owner_root,
            )),
            then_branch: Box::new(rewrite_runtime_reduction_owner_names(
                then_branch,
                reduction,
                bindings_in_scope,
                owner_root,
            )),
            else_branch: Box::new(rewrite_runtime_reduction_owner_names(
                else_branch,
                reduction,
                bindings_in_scope,
                owner_root,
            )),
        },
        Expr::Filter { .. } => unreachable!("filters are not reduction expression fragments"),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!(
                "runtime indexed/designation access is not a reduction expression fragment"
            )
        }
    }
}

fn runtime_reduction_owner_root(
    source: &str,
    externalized_sequences: &HashMap<String, ExternalizedModelSequence>,
) -> Option<String> {
    externalized_sequences
        .get(source)
        .map(|sequence| sequence.owner_root.clone())
        .or_else(|| filter_owner_root(source).map(str::to_string))
}

fn filter_owner_root(source: &str) -> Option<&str> {
    let rest = source.strip_prefix(MODEL_FILTER_BINDING_PREFIX)?;
    let (root, member) = rest.split_once('$')?;
    if root.is_empty() || member.is_empty() || member.contains('$') {
        return None;
    }
    Some(root)
}

fn model_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_BINDING_PREFIX}{root}${member}")
}

fn decode_lowered_sequence_value(value: &str) -> Option<Vec<String>> {
    let rest = value.strip_prefix(LOWERED_SEQUENCE_VALUE_PREFIX)?;
    if rest.is_empty() {
        return Some(Vec::new());
    }
    Some(rest.split('|').map(str::to_string).collect())
}

fn first_diagnostic_message(errors: &[Diagnostic]) -> String {
    errors
        .first()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| "invalid expression".to_string())
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn externalized_sequence_role_uses_provenance_not_generated_name() {
        let externalized_sequences = HashMap::from([(
            "opaque-sequence-binding".to_string(),
            ExternalizedModelSequence {
                owner_root: "project".to_string(),
                owner_model: "Project".to_string(),
                member_name: "tasks".to_string(),
                element_model: "Task".to_string(),
            },
        )]);

        assert!(is_runtime_reduction_source(
            "opaque-sequence-binding",
            &externalized_sequences,
        ));
        assert_eq!(
            runtime_reduction_owner_root("opaque-sequence-binding", &externalized_sequences)
                .as_deref(),
            Some("project")
        );
    }
}
