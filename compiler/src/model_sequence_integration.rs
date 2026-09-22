use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_live_type_name, decode_through_path, encode_through_path, ActionArgument, ActionDecl,
    Declaration, DerivedDecl, Expr, Program, SourceLocation, StateDecl, StateModelDecl,
    StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::program_facts::ProgramFacts;
use crate::sequence_surface::{decode_sequence_literal, decode_sequence_live_type};

pub(crate) const MODEL_SEQUENCE_BINDING_PREFIX: &str = "__meld_mseq$";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalizedModelSequence {
    pub(crate) owner_root: String,
    pub(crate) owner_model: String,
    pub(crate) member_name: String,
    pub(crate) element_model: String,
}

#[derive(Debug, Clone)]
pub struct ModelSequenceLowering {
    pub(crate) program: Program,
    pub(crate) externalized_sequences: HashMap<String, ExternalizedModelSequence>,
}

#[derive(Debug, Clone)]
struct SequenceMemberSpec {
    member_name: String,
    encoded_type_name: String,
    initializer: Expr,
    location: SourceLocation,
}

/// Bridge state-model sequence members into the existing top-level ordered-sequence
/// spike without changing either semantic subsystem yet.
///
/// Each concrete modeled-state root receives one hidden sequence state keyed by
/// `(root identity, member declaration)`. Source paths such as `invoice.lines`
/// are rewritten to that hidden state before ordered-sequence lowering runs.
/// The sequence member is then removed from the model passed to the older
/// state-model lowering so it is not duplicated there.
///
/// This is intentionally bootstrap architecture. Identity-only live designation
/// of an owner model remains valid and may resolve that exact owner's members at
/// runtime. Whole modeled-state value/authority projection is still deferred where
/// the older lowering cannot include the externalized sequence member coherently.
pub fn lower(program: &Program) -> Result<ModelSequenceLowering, Vec<Diagnostic>> {
    let facts = ProgramFacts::from_program(program);
    let mut errors = Vec::new();
    let specs = collect_sequence_members(program, &facts, &mut errors);

    if specs.is_empty() {
        return if errors.is_empty() {
            Ok(ModelSequenceLowering {
                program: program.clone(),
                externalized_sequences: HashMap::new(),
            })
        } else {
            Err(errors)
        };
    }

    let owner_models: HashSet<String> = specs.keys().cloned().collect();
    reject_model_local_sequence_reads(program, &specs, &mut errors);
    reject_incomplete_owner_model_projection(program, &owner_models, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }

    let root_members = collect_root_members(&facts, &specs);
    let externalized_sequences: HashMap<String, ExternalizedModelSequence> = root_members
        .iter()
        .flat_map(|(root, members)| {
            let owner_model = facts
                .modeled_state_roots()
                .iter()
                .find(|candidate| candidate.name == *root)
                .map(|candidate| candidate.model_name.clone())
                .expect("externalized model-sequence root must retain modeled-root facts");
            members.iter().map(move |member| {
                let binding_name = model_sequence_binding_name(root, &member.member_name);
                let element_model = decode_sequence_live_type(&member.encoded_type_name)
                    .expect("externalized model-sequence type must remain [live T]")
                    .to_string();
                (
                    binding_name,
                    ExternalizedModelSequence {
                        owner_root: root.clone(),
                        owner_model: owner_model.clone(),
                        member_name: member.member_name.clone(),
                        element_model,
                    },
                )
            })
        })
        .collect();
    let path_rewrites: HashMap<String, String> = root_members
        .iter()
        .flat_map(|(root, members)| {
            members.iter().map(move |member| {
                (
                    format!("{root}.{}", member.member_name),
                    model_sequence_binding_name(root, &member.member_name),
                )
            })
        })
        .collect();

    let state_models = strip_sequence_members(&program.state_models);
    let mut declarations = Vec::new();

    for declaration in &program.declarations {
        declarations.push(rewrite_declaration(declaration, &path_rewrites));

        let Declaration::State(state) = declaration else {
            continue;
        };
        if !state.implicit_model_initializer {
            continue;
        }
        let Some(members) = root_members.get(&state.name) else {
            continue;
        };

        for member in members {
            declarations.push(Declaration::State(StateDecl {
                location: member.location,
                name: model_sequence_binding_name(&state.name, &member.member_name),
                type_name: Some(member.encoded_type_name.clone()),
                initializer: member.initializer.clone(),
                implicit_model_initializer: false,
            }));
        }
    }

    Ok(ModelSequenceLowering {
        program: Program {
            declarations,
            state_models,
        },
        externalized_sequences,
    })
}

fn collect_sequence_members(
    program: &Program,
    facts: &ProgramFacts,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, Vec<SequenceMemberSpec>> {
    let mut result: HashMap<String, Vec<SequenceMemberSpec>> = HashMap::new();

    for model in &program.state_models {
        for member in &model.members {
            let StateModelMember::State(state) = member else {
                continue;
            };
            let Some(encoded_type_name) = state.type_name.as_deref() else {
                continue;
            };
            let Some(element_model) = decode_sequence_live_type(encoded_type_name) else {
                continue;
            };

            if !facts.has_model(element_model) {
                errors.push(diag(
                    state.location,
                    format!(
                        "state-model ordered sequence '{}.{}' names unknown state model '{}'",
                        model.name, state.name, element_model
                    ),
                ));
                continue;
            }

            match sequence_literal_targets(&state.initializer) {
                Some(targets) if targets.is_empty() => {}
                Some(_) => errors.push(diag(
                    state.location,
                    format!(
                        "bootstrap state-model sequence member '{}.{}' must start as []; reusable model defaults cannot capture ambient live roots",
                        model.name, state.name
                    ),
                )),
                None => errors.push(diag(
                    state.location,
                    format!(
                        "state-model ordered sequence '{}.{}' requires a sequence literal initializer",
                        model.name, state.name
                    ),
                )),
            }

            result
                .entry(model.name.clone())
                .or_default()
                .push(SequenceMemberSpec {
                    member_name: state.name.clone(),
                    encoded_type_name: encoded_type_name.to_string(),
                    initializer: state.initializer.clone(),
                    location: state.location,
                });
        }
    }

    result
}

fn reject_model_local_sequence_reads(
    program: &Program,
    specs: &HashMap<String, Vec<SequenceMemberSpec>>,
    errors: &mut Vec<Diagnostic>,
) {
    for model in &program.state_models {
        let Some(sequence_members) = specs.get(&model.name) else {
            continue;
        };
        let names: HashSet<&str> = sequence_members
            .iter()
            .map(|member| member.member_name.as_str())
            .collect();

        for member in &model.members {
            if matches!(member, StateModelMember::State(state) if state.type_name.as_deref().and_then(decode_sequence_live_type).is_some())
            {
                continue;
            }

            let expression = match member {
                StateModelMember::State(state) => &state.initializer,
                StateModelMember::Derived(derived) => &derived.expression,
            };

            for name in &names {
                if expr_mentions_member(expression, name) {
                    errors.push(diag(
                        member.location(),
                        format!(
                            "bootstrap state-model sequence-member spike does not yet support model-local reads of '{}.{}'; read-only traversal/aggregation is the next pressure",
                            model.name, name
                        ),
                    ));
                }
            }
        }
    }
}

fn reject_incomplete_owner_model_projection(
    program: &Program,
    owner_models: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    // Identity-only designation and [live T] membership do not project a whole model.
    // Keep the older bootstrap restriction only where an action parameter would require
    // a complete model value/writable projection.
    for declaration in &program.declarations {
        if let Declaration::Action(action) = declaration {
            reject_owner_model_parameters(action, owner_models, errors);
        }
    }
}

fn reject_owner_model_parameters(
    action: &ActionDecl,
    owner_models: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for parameter in &action.parameters {
        let model_name =
            decode_live_type_name(&parameter.type_name).unwrap_or(&parameter.type_name);
        if owner_models.contains(model_name) {
            errors.push(diag(
                parameter.location,
                format!(
                    "bootstrap state-model sequence-member spike defers model-typed parameter '{}: {}' because whole-model value/authority must include sequence members",
                    parameter.name, model_name
                ),
            ));
        }
    }
}

fn collect_root_members(
    facts: &ProgramFacts,
    specs: &HashMap<String, Vec<SequenceMemberSpec>>,
) -> HashMap<String, Vec<SequenceMemberSpec>> {
    facts
        .modeled_state_roots()
        .iter()
        .filter_map(|root| {
            specs
                .get(&root.model_name)
                .cloned()
                .map(|members| (root.name.clone(), members))
        })
        .collect()
}

fn strip_sequence_members(models: &[StateModelDecl]) -> Vec<StateModelDecl> {
    models
        .iter()
        .map(|model| StateModelDecl {
            location: model.location,
            name: model.name.clone(),
            members: model
                .members
                .iter()
                .filter(|member| {
                    !matches!(member, StateModelMember::State(state) if state.type_name.as_deref().and_then(decode_sequence_live_type).is_some())
                })
                .cloned()
                .collect(),
        })
        .collect()
}

fn rewrite_declaration(
    declaration: &Declaration,
    path_rewrites: &HashMap<String, String>,
) -> Declaration {
    match declaration {
        Declaration::State(state) => Declaration::State(StateDecl {
            location: state.location,
            name: state.name.clone(),
            type_name: state.type_name.clone(),
            initializer: rewrite_expr(&state.initializer, path_rewrites),
            implicit_model_initializer: state.implicit_model_initializer,
        }),
        Declaration::Derived(derived) => Declaration::Derived(DerivedDecl {
            location: derived.location,
            name: derived.name.clone(),
            expression: rewrite_expr(&derived.expression, path_rewrites),
        }),
        Declaration::Action(action) => Declaration::Action(ActionDecl {
            location: action.location,
            name: action.name.clone(),
            parameters: action.parameters.clone(),
            statements: action
                .statements
                .iter()
                .map(|statement| rewrite_statement(statement, path_rewrites))
                .collect(),
        }),
    }
}

fn rewrite_statement(statement: &Statement, path_rewrites: &HashMap<String, String>) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => Statement::Assignment {
            location: *location,
            target: rewrite_state_path(target, path_rewrites),
            operator: *operator,
            value: rewrite_expr(value, path_rewrites),
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
            source: rewrite_plain_path(source, path_rewrites),
            index: rewrite_expr(index, path_rewrites),
            member: member.clone(),
            operator: *operator,
            value: rewrite_expr(value, path_rewrites),
        },
        Statement::RuntimeIndexAssignment { .. }
        | Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime index lowering runs after model-sequence integration")
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
                        ActionArgument::Value(rewrite_expr(expression, path_rewrites))
                    }
                    ActionArgument::StateGrant { location, name } => ActionArgument::StateGrant {
                        location: *location,
                        name: rewrite_state_path(name, path_rewrites),
                    },
                    ActionArgument::IndexedStateGrant {
                        location,
                        source,
                        index,
                        member,
                    } => ActionArgument::IndexedStateGrant {
                        location: *location,
                        source: rewrite_plain_path(source, path_rewrites),
                        index: rewrite_expr(index, path_rewrites),
                        member: member.clone(),
                    },
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: rewrite_expr(message, path_rewrites),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: rewrite_expr(condition, path_rewrites),
            then_branch: then_branch
                .iter()
                .map(|statement| rewrite_statement(statement, path_rewrites))
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| rewrite_statement(statement, path_rewrites))
                    .collect()
            }),
        },
    }
}

fn rewrite_expr(expression: &Expr, path_rewrites: &HashMap<String, String>) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => Expr::Name(rewrite_plain_path(name, path_rewrites)),
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_expr(left, path_rewrites)),
            right: Box::new(rewrite_expr(right, path_rewrites)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_expr(condition, path_rewrites)),
            then_branch: Box::new(rewrite_expr(then_branch, path_rewrites)),
            else_branch: Box::new(rewrite_expr(else_branch, path_rewrites)),
        },
        Expr::IndexedDesignation { source, index } => Expr::IndexedDesignation {
            source: path_rewrites
                .get(source)
                .cloned()
                .unwrap_or_else(|| source.clone()),
            index: Box::new(rewrite_expr(index, path_rewrites)),
        },
        Expr::IndexedMember {
            source,
            index,
            member,
        } => Expr::IndexedMember {
            source: rewrite_plain_path(source, path_rewrites),
            index: Box::new(rewrite_expr(index, path_rewrites)),
            member: member.clone(),
        },
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after model-sequence integration")
        }
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => Expr::Filter {
            source: Box::new(rewrite_expr(source, path_rewrites)),
            element: element.clone(),
            predicate: Box::new(rewrite_expr(predicate, path_rewrites)),
            order_by: order_by
                .as_ref()
                .map(|order_by| Box::new(rewrite_expr(order_by, path_rewrites))),
            order_descending: *order_descending,
            element_model: element_model.clone(),
        },
    }
}

fn rewrite_state_path(name: &str, path_rewrites: &HashMap<String, String>) -> String {
    if let Some(path) = decode_through_path(name) {
        return encode_through_path(&rewrite_plain_path(path, path_rewrites));
    }
    rewrite_plain_path(name, path_rewrites)
}

fn rewrite_plain_path(name: &str, path_rewrites: &HashMap<String, String>) -> String {
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

fn expr_mentions_member(expression: &Expr, member: &str) -> bool {
    match expression {
        Expr::Name(name) => {
            name == member
                || name
                    .strip_prefix(member)
                    .is_some_and(|rest| rest.starts_with('.'))
        }
        Expr::Binary { left, right, .. } => {
            expr_mentions_member(left, member) || expr_mentions_member(right, member)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_mentions_member(condition, member)
                || expr_mentions_member(then_branch, member)
                || expr_mentions_member(else_branch, member)
        }
        Expr::Filter {
            source, predicate, ..
        } => expr_mentions_member(source, member) || expr_mentions_member(predicate, member),
        Expr::IndexedMember { source, index, .. } | Expr::IndexedDesignation { source, index } => {
            source == member || expr_mentions_member(index, member)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after model-sequence integration")
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => false,
    }
}

fn sequence_literal_targets(expression: &Expr) -> Option<Vec<String>> {
    let Expr::String(value) = expression else {
        return None;
    };
    decode_sequence_literal(value)
}

fn model_sequence_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_SEQUENCE_BINDING_PREFIX}{root}${member}")
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
