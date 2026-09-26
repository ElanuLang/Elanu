use std::collections::{HashMap, HashSet};

use crate::ast::{
    ActionArgument, ActionDecl, Declaration, DerivedDecl, Expr, Program, SourceLocation,
    StateModelDecl, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::model_filter_binding_name;
use crate::model_types::{self, ModelTypeFacts, ModelTypeInfo};
use crate::program_facts::ProgramFacts;
use crate::reduction_surface::{
    decode_reduction, encode_reduction, parse_expression_fragment, ReductionSpec,
};
use crate::semantic::ValueType;
use crate::sequence_surface::decode_sequence_live_type;

const MODEL_SEQUENCE_BINDING_PREFIX: &str = "__elanu_mseq$";
const MODEL_REDUCTION_BINDING_PREFIX: &str = "__elanu_reduce_member$";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReductionSourceKind {
    StateSequence,
    FilterView,
}

#[derive(Debug, Clone)]
struct ReductionMemberSpec {
    member_name: String,
    reduction: ReductionSpec,
    source_kind: ReductionSourceKind,
    location: SourceLocation,
}

/// Prepare read-only reductions before the state-model/sequence bootstrap passes.
///
/// Model-local reduction-derived members are externalized per concrete owner root.
/// Their source may now be either an owner-relative sequence state or the first
/// typed derived filter view. Both have `[live T]` value type, but their storage
/// semantics remain distinct in the generated binding chosen below.
pub fn lower(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    let facts = ProgramFacts::from_program(program);
    let type_facts = model_types::resolve(&program.state_models)?;
    let mut errors = Vec::new();
    let reductions = collect_model_reductions(program, &facts, &type_facts, &mut errors);
    reject_model_local_reduction_chaining(program, &reductions, &mut errors);

    let top_level_sequences = collect_top_level_sequences(program);
    let mut path_rewrites = HashMap::new();
    for root in facts.modeled_state_roots() {
        let Some(members) = reductions.get(&root.model_name) else {
            continue;
        };
        for member in members {
            path_rewrites.insert(
                format!("{}.{}", root.name, member.member_name),
                model_reduction_binding_name(&root.name, &member.member_name),
            );
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let state_models = strip_reduction_members(&program.state_models);
    let mut declarations = Vec::new();

    for declaration in &program.declarations {
        let annotated = annotate_top_level_declaration(
            declaration,
            &top_level_sequences,
            &path_rewrites,
            &mut errors,
        );
        declarations.push(annotated);

        let Declaration::State(state) = declaration else {
            continue;
        };
        if !state.implicit_model_initializer {
            continue;
        }
        let Some(model_name) = state.type_name.as_deref() else {
            continue;
        };
        let Some(members) = reductions.get(model_name) else {
            continue;
        };

        for member in members {
            let mut reduction = member.reduction.clone();
            reduction.source = match member.source_kind {
                ReductionSourceKind::StateSequence => {
                    model_sequence_binding_name(&state.name, &reduction.source)
                }
                ReductionSourceKind::FilterView => {
                    model_filter_binding_name(&state.name, &reduction.source)
                }
            };
            declarations.push(Declaration::Derived(DerivedDecl {
                location: member.location,
                name: model_reduction_binding_name(&state.name, &member.member_name),
                expression: Expr::String(encode_reduction(&reduction)),
            }));
        }
    }

    if errors.is_empty() {
        Ok(Program {
            declarations,
            state_models,
        })
    } else {
        Err(errors)
    }
}

fn collect_model_reductions(
    program: &Program,
    facts: &ProgramFacts,
    type_facts: &ModelTypeFacts,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<ReductionMemberSpec>> {
    let mut result = HashMap::new();

    for model in &program.state_models {
        let Some(owner_types) = type_facts.model(&model.name) else {
            continue;
        };

        for (member_index, member) in model.members.iter().enumerate() {
            let StateModelMember::Derived(derived) = member else {
                continue;
            };
            let Expr::String(value) = &derived.expression else {
                continue;
            };
            let Some(mut reduction) = decode_reduction(value) else {
                continue;
            };

            if reduction.source.contains('.') {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local reduction '{}.{}' must be rooted in one local sequence member",
                        model.name, derived.name
                    ),
                ));
                continue;
            }

            let Some((element_model, source_kind)) =
                reduction_sequence_source(model, member_index, &reduction.source, owner_types)
            else {
                errors.push(diag(
                    derived.location,
                    format!(
                        "model-local reduction '{}.{}' source '{}' is not an earlier local [live T] sequence state or filter view",
                        model.name, derived.name, reduction.source
                    ),
                ));
                continue;
            };
            reduction.element_model = Some(element_model);

            if reduction.accumulator == reduction.element {
                errors.push(diag(
                    derived.location,
                    "reduction accumulator and element bindings must have different names",
                ));
            }

            let owner_value_members = collect_owner_value_members(model, member_index, owner_types);
            validate_model_local_reduction(
                &model.name,
                &derived.name,
                &reduction,
                facts,
                &owner_value_members,
                derived.location,
                errors,
            );

            result
                .entry(model.name.clone())
                .or_insert_with(Vec::new)
                .push(ReductionMemberSpec {
                    member_name: derived.name.clone(),
                    reduction,
                    source_kind,
                    location: derived.location,
                });
        }
    }

    result
}

fn reduction_sequence_source(
    model: &StateModelDecl,
    before_index: usize,
    source_name: &str,
    owner_types: &ModelTypeInfo,
) -> Option<(String, ReductionSourceKind)> {
    let source_member = model
        .members
        .iter()
        .take(before_index)
        .find(|member| member.name() == source_name)?;
    let ValueType::SequenceLive(element_model) = owner_types.member_type(source_name)? else {
        return None;
    };

    let kind = match source_member {
        StateModelMember::State(_) => ReductionSourceKind::StateSequence,
        StateModelMember::Derived(derived) if matches!(derived.expression, Expr::Filter { .. }) => {
            ReductionSourceKind::FilterView
        }
        _ => return None,
    };

    Some((element_model.clone(), kind))
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
            if matches!(owner_types.member_type(name), Some(ValueType::SequenceLive(_))) {
                return None;
            }
            if matches!(
                member,
                StateModelMember::Derived(derived)
                    if matches!(&derived.expression, Expr::String(value) if decode_reduction(value).is_some())
            ) {
                return None;
            }
            Some(name.to_string())
        })
        .collect()
}

fn validate_model_local_reduction(
    owner_model: &str,
    member_name: &str,
    reduction: &ReductionSpec,
    facts: &ProgramFacts,
    owner_value_members: &HashSet<String>,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    let initial = match parse_expression_fragment(&reduction.initial_source) {
        Ok(expression) => Some(expression),
        Err(parse_errors) => {
            errors.push(diag(
                location,
                format!(
                    "invalid initial expression in model-local reduction '{}.{}': {}",
                    owner_model,
                    member_name,
                    first_message(&parse_errors)
                ),
            ));
            None
        }
    };

    if let Some(initial) = &initial {
        validate_rooted_owner_expr(
            initial,
            owner_value_members,
            owner_model,
            member_name,
            "initial",
            location,
            errors,
        );
    }

    let step = match parse_expression_fragment(&reduction.step_source) {
        Ok(expression) => Some(expression),
        Err(parse_errors) => {
            errors.push(diag(
                location,
                format!(
                    "invalid step expression in model-local reduction '{}.{}': {}",
                    owner_model,
                    member_name,
                    first_message(&parse_errors)
                ),
            ));
            None
        }
    };

    let Some(step) = step else {
        return;
    };
    let Some(element_model) = reduction.element_model.as_deref() else {
        return;
    };
    let element_members = facts
        .models()
        .get(element_model)
        .map(|model| model.members.keys().cloned().collect())
        .unwrap_or_default();

    validate_rooted_step(
        &step,
        &reduction.accumulator,
        &reduction.element,
        element_model,
        &element_members,
        owner_value_members,
        owner_model,
        member_name,
        location,
        errors,
    );
}

fn validate_rooted_owner_expr(
    expression: &Expr,
    owner_value_members: &HashSet<String>,
    owner_model: &str,
    member_name: &str,
    part: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    match expression {
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) => {}
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is deferred in the first read-only reduction spike",
                ));
            }
        }
        Expr::Name(name) if owner_value_members.contains(name) => {}
        Expr::Name(name) => errors.push(diag(
            location,
            format!(
                "rooted model-local reduction '{}.{}' {part} may reference only earlier ordinary members of owner model '{}'; '{}' is not available in this scope",
                owner_model, member_name, owner_model, name
            ),
        )),
        Expr::Binary { left, right, .. } => {
            validate_rooted_owner_expr(
                left,
                owner_value_members,
                owner_model,
                member_name,
                part,
                location,
                errors,
            );
            validate_rooted_owner_expr(
                right,
                owner_value_members,
                owner_model,
                member_name,
                part,
                location,
                errors,
            );
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            for branch in [
                condition.as_ref(),
                then_branch.as_ref(),
                else_branch.as_ref(),
            ] {
                validate_rooted_owner_expr(
                    branch,
                    owner_value_members,
                    owner_model,
                    member_name,
                    part,
                    location,
                    errors,
                );
            }
        }
        Expr::Filter { .. } => errors.push(diag(
            location,
            "nested filter is not available inside a reduction expression",
        )),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => errors.push(diag(
            location,
            "runtime indexed member access is not available inside a reduction expression",
        )),
        Expr::RuntimeIndexMember { .. } | Expr::RuntimeIndexDesignation { .. } | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after reduction integration")
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_rooted_step(
    expression: &Expr,
    accumulator: &str,
    element: &str,
    element_model: &str,
    element_members: &HashSet<String>,
    owner_value_members: &HashSet<String>,
    owner_model: &str,
    member_name: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    match expression {
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) => {}
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is deferred in the first read-only reduction spike",
                ));
            }
        }
        Expr::Name(name) if name == accumulator => {}
        Expr::Name(name) if name == element => errors.push(diag(
            location,
            "reading the reduction element as an ordinary whole live designation is deferred; read a member",
        )),
        Expr::Name(name) => {
            let prefix = format!("{element}.");
            if let Some(member) = name.strip_prefix(&prefix) {
                if member.is_empty() || member.contains('.') {
                    errors.push(diag(
                        location,
                        "reduction element read must select exactly one modeled-state member in the first spike",
                    ));
                } else if !element_members.contains(member) {
                    errors.push(diag(
                        location,
                        format!("state model '{element_model}' has no member '{member}'"),
                    ));
                }
            } else if owner_value_members.contains(name) {
                // Direct reads of earlier ordinary members are the established
                // rooted-locality behavior for model-local derived computation.
            } else {
                errors.push(diag(
                    location,
                    format!(
                        "rooted model-local reduction '{}.{}' step may reference accumulator '{}', members reached through element '{}', or an earlier ordinary member of owner model '{}'; '{}' is not available in this scope",
                        owner_model, member_name, accumulator, element, owner_model, name
                    ),
                ));
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_rooted_step(
                left,
                accumulator,
                element,
                element_model,
                element_members,
                owner_value_members,
                owner_model,
                member_name,
                location,
                errors,
            );
            validate_rooted_step(
                right,
                accumulator,
                element,
                element_model,
                element_members,
                owner_value_members,
                owner_model,
                member_name,
                location,
                errors,
            );
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            for branch in [
                condition.as_ref(),
                then_branch.as_ref(),
                else_branch.as_ref(),
            ] {
                validate_rooted_step(
                    branch,
                    accumulator,
                    element,
                    element_model,
                    element_members,
                    owner_value_members,
                    owner_model,
                    member_name,
                    location,
                    errors,
                );
            }
        }
        Expr::Filter { .. } => errors.push(diag(
            location,
            "nested filter is not available inside a reduction expression",
        )),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => errors.push(diag(
            location,
            "runtime indexed member access is not available inside a reduction expression",
        )),
        Expr::RuntimeIndexMember { .. } | Expr::RuntimeIndexDesignation { .. } | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after reduction integration")
        }
    }
}

fn reject_model_local_reduction_chaining(
    program: &Program,
    reductions: &HashMap<String, Vec<ReductionMemberSpec>>,
    errors: &mut Vec<Diagnostic>,
) {
    for model in &program.state_models {
        let Some(reduction_members) = reductions.get(&model.name) else {
            continue;
        };
        let reduction_names: HashSet<&str> = reduction_members
            .iter()
            .map(|member| member.member_name.as_str())
            .collect();

        for member in &model.members {
            if reduction_names.contains(member.name()) {
                continue;
            }
            let expression = match member {
                StateModelMember::State(state) => &state.initializer,
                StateModelMember::Derived(derived) => &derived.expression,
            };
            if expression_mentions_any(expression, &reduction_names) {
                errors.push(diag(
                    member.location(),
                    format!(
                        "bootstrap read-only reduction spike defers model-local chaining through reduction-derived member '{}.{}'",
                        model.name,
                        member.name()
                    ),
                ));
            }
        }
    }
}

fn collect_top_level_sequences(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            let model = state
                .type_name
                .as_deref()
                .and_then(decode_sequence_live_type)?;
            Some((state.name.clone(), model.to_string()))
        })
        .collect()
}

fn annotate_top_level_declaration(
    declaration: &Declaration,
    top_level_sequences: &HashMap<String, String>,
    path_rewrites: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Declaration {
    match declaration {
        Declaration::State(state) => {
            let mut state = state.clone();
            state.initializer = annotate_top_level_expr(
                &state.initializer,
                state.location,
                top_level_sequences,
                path_rewrites,
                errors,
            );
            Declaration::State(state)
        }
        Declaration::Derived(derived) => Declaration::Derived(DerivedDecl {
            location: derived.location,
            name: derived.name.clone(),
            expression: annotate_top_level_expr(
                &derived.expression,
                derived.location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
        }),
        Declaration::Action(action) => Declaration::Action(ActionDecl {
            location: action.location,
            name: action.name.clone(),
            parameters: action.parameters.clone(),
            statements: action
                .statements
                .iter()
                .map(|statement| {
                    annotate_statement(statement, top_level_sequences, path_rewrites, errors)
                })
                .collect(),
        }),
    }
}

fn annotate_statement(
    statement: &Statement,
    top_level_sequences: &HashMap<String, String>,
    path_rewrites: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => Statement::Assignment {
            location: *location,
            target: rewrite_path(target, path_rewrites),
            operator: *operator,
            value: annotate_top_level_expr(
                value,
                *location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
        },
        Statement::IndexedThroughAssignment {
            location,
            source,
            index,
            member,
            operator,
            value,
        } => Statement::IndexedThroughAssignment {
            location: *location,
            source: rewrite_path(source, path_rewrites),
            index: annotate_top_level_expr(
                index,
                *location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
            member: member.clone(),
            operator: *operator,
            value: annotate_top_level_expr(
                value,
                *location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
        },
        Statement::RuntimeIndexAssignment { .. }
        | Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime index lowering runs after reduction integration")
        }
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
                    ActionArgument::Value(expression) => {
                        ActionArgument::Value(annotate_top_level_expr(
                            expression,
                            *location,
                            top_level_sequences,
                            path_rewrites,
                            errors,
                        ))
                    }
                    ActionArgument::StateGrant { location, name } => ActionArgument::StateGrant {
                        location: *location,
                        name: rewrite_path(name, path_rewrites),
                    },
                    ActionArgument::IndexedStateGrant {
                        location,
                        source,
                        index,
                        member,
                    } => ActionArgument::IndexedStateGrant {
                        location: *location,
                        source: rewrite_path(source, path_rewrites),
                        index: annotate_top_level_expr(
                            index,
                            *location,
                            top_level_sequences,
                            path_rewrites,
                            errors,
                        ),
                        member: member.clone(),
                    },
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: annotate_top_level_expr(
                message,
                *location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: annotate_top_level_expr(
                condition,
                *location,
                top_level_sequences,
                path_rewrites,
                errors,
            ),
            then_branch: then_branch
                .iter()
                .map(|statement| {
                    annotate_statement(statement, top_level_sequences, path_rewrites, errors)
                })
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| {
                        annotate_statement(statement, top_level_sequences, path_rewrites, errors)
                    })
                    .collect()
            }),
        },
    }
}

fn annotate_top_level_expr(
    expression: &Expr,
    location: SourceLocation,
    top_level_sequences: &HashMap<String, String>,
    path_rewrites: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::Name(name) => Expr::Name(rewrite_path(name, path_rewrites)),
        Expr::String(value) => {
            let Some(mut reduction) = decode_reduction(value) else {
                return Expr::String(value.clone());
            };
            let Some(model) = top_level_sequences.get(&reduction.source) else {
                errors.push(diag(
                    location,
                    format!(
                        "top-level reduction source '{}' is not a named top-level [live T] sequence state; owner-relative reductions belong inside their state model in this spike",
                        reduction.source
                    ),
                ));
                return Expr::String(value.clone());
            };
            reduction.element_model = Some(model.clone());
            Expr::String(encode_reduction(&reduction))
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(annotate_top_level_expr(
                left,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            right: Box::new(annotate_top_level_expr(
                right,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(annotate_top_level_expr(
                condition,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            then_branch: Box::new(annotate_top_level_expr(
                then_branch,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            else_branch: Box::new(annotate_top_level_expr(
                else_branch,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
        },
        Expr::IndexedDesignation { source, index } => Expr::IndexedDesignation {
            source: rewrite_path(source, path_rewrites),
            index: Box::new(annotate_top_level_expr(
                index,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
        },
        Expr::IndexedMember {
            source,
            index,
            member,
        } => Expr::IndexedMember {
            source: rewrite_path(source, path_rewrites),
            index: Box::new(annotate_top_level_expr(
                index,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            member: member.clone(),
        },
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after reduction integration")
        }
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => Expr::Filter {
            source: Box::new(annotate_top_level_expr(
                source,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            element: element.clone(),
            predicate: Box::new(annotate_top_level_expr(
                predicate,
                location,
                top_level_sequences,
                path_rewrites,
                errors,
            )),
            order_by: order_by.as_ref().map(|order_by| {
                Box::new(annotate_top_level_expr(
                    order_by,
                    location,
                    top_level_sequences,
                    path_rewrites,
                    errors,
                ))
            }),
            order_descending: *order_descending,
            element_model: element_model.clone(),
        },
    }
}

fn strip_reduction_members(models: &[StateModelDecl]) -> Vec<StateModelDecl> {
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
                            if matches!(&derived.expression, Expr::String(value) if decode_reduction(value).is_some())
                    )
                })
                .cloned()
                .collect(),
        })
        .collect()
}

fn expression_mentions_any(expression: &Expr, names: &HashSet<&str>) -> bool {
    match expression {
        Expr::Name(name) => names.iter().any(|candidate| {
            name == *candidate
                || name
                    .strip_prefix(*candidate)
                    .is_some_and(|rest| rest.starts_with('.'))
        }),
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
            source, predicate, ..
        } => expression_mentions_any(source, names) || expression_mentions_any(predicate, names),
        Expr::IndexedMember { source, index, .. } | Expr::IndexedDesignation { source, index } => {
            names.iter().any(|candidate| source == *candidate)
                || expression_mentions_any(index, names)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after reduction integration")
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => false,
    }
}

fn rewrite_path(name: &str, path_rewrites: &HashMap<String, String>) -> String {
    for (source, lowered) in path_rewrites {
        if name == source {
            return lowered.clone();
        }
        if let Some(rest) = name.strip_prefix(source) {
            if rest.starts_with('.') {
                return format!("{lowered}{rest}");
            }
        }
    }
    name.to_string()
}

fn model_sequence_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_SEQUENCE_BINDING_PREFIX}{root}${member}")
}

fn model_reduction_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_REDUCTION_BINDING_PREFIX}{root}${member}")
}

fn first_message(errors: &[Diagnostic]) -> String {
    errors
        .first()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| "invalid expression".to_string())
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
