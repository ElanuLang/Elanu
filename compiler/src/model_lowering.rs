use std::collections::{HashMap, HashSet};

use crate::ast::{
    ActionArgument, ActionDecl, ActionParameter, ActionParameterKind, Declaration, DerivedDecl,
    Expr, Program, SourceLocation, StateDecl, StateModelDecl, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::model_types::{self, ModelTypeFacts};
use crate::semantic::{show_type, ValueType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelMemberKind {
    State,
    Derived,
}

#[derive(Debug, Clone)]
struct ModelMemberInfo {
    name: String,
    kind: ModelMemberKind,
    value_type: ValueType,
    expression: Expr,
    location: SourceLocation,
}

#[derive(Debug, Clone)]
struct ModelInfo {
    name: String,
    members: Vec<ModelMemberInfo>,
    member_indices: HashMap<String, usize>,
}

impl ModelInfo {
    fn member(&self, name: &str) -> Option<&ModelMemberInfo> {
        self.member_indices
            .get(name)
            .and_then(|index| self.members.get(*index))
    }

    fn state_members(&self) -> impl Iterator<Item = &ModelMemberInfo> {
        self.members
            .iter()
            .filter(|member| member.kind == ModelMemberKind::State)
    }
}

#[derive(Debug, Clone)]
struct ModelRoot {
    model_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelParameterMode {
    Value,
    State,
}

#[derive(Debug, Clone)]
struct ModelParameterContext {
    model_name: String,
    mode: ModelParameterMode,
    state_member_names: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct ActionContext {
    model_parameters: HashMap<String, ModelParameterContext>,
}

/// Lower the provisional state-model surface into the existing scalar bootstrap
/// kernel. The lowering is intentionally an implementation technique rather
/// than a semantic claim: generated names are opaque compiler identities, and
/// source-level model/value/authority rules are checked before ordinary
/// semantic checking sees the flattened program.
pub fn lower(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    let model_types = model_types::resolve(&program.state_models)?;
    let models = build_models(&program.state_models, &model_types);

    let mut errors = Vec::new();
    check_duplicate_top_level_names(program, &mut errors);
    let roots = collect_model_roots(program, &models, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }

    let action_signatures: HashMap<String, ActionDecl> = program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Action(action) => Some((action.name.clone(), action.clone())),
            _ => None,
        })
        .collect();

    let mut declarations = Vec::new();

    for declaration in &program.declarations {
        match declaration {
            Declaration::State(state) if state.implicit_model_initializer => {
                let Some(model_name) = state.type_name.as_deref() else {
                    errors.push(diag(
                        state.location,
                        format!(
                            "state '{}' is missing a state-model type for implicit initialization",
                            state.name
                        ),
                    ));
                    continue;
                };
                let Some(model) = models.get(model_name) else {
                    errors.push(diag(
                        state.location,
                        format!(
                            "state '{}: {}' requires an initializer because '{}' is not a state model",
                            state.name, model_name, model_name
                        ),
                    ));
                    continue;
                };
                expand_model_state(&state.name, model, &mut declarations);
            }
            Declaration::State(state) => {
                if let Some(type_name) = state.type_name.as_deref() {
                    if models.contains_key(type_name) {
                        errors.push(diag(
                            state.location,
                            format!(
                                "explicit initialization of state-model variable '{}: {}' is deferred; use model defaults for the first state-model milestone",
                                state.name, type_name
                            ),
                        ));
                        continue;
                    }
                }

                let expression = lower_expr(
                    &state.initializer,
                    state.location,
                    &ActionContext::default(),
                    &models,
                    &roots,
                    &mut errors,
                );
                let mut state = state.clone();
                state.initializer = expression;
                state.implicit_model_initializer = false;
                declarations.push(Declaration::State(state));
            }
            Declaration::Derived(derived) => {
                let expression = lower_expr(
                    &derived.expression,
                    derived.location,
                    &ActionContext::default(),
                    &models,
                    &roots,
                    &mut errors,
                );
                declarations.push(Declaration::Derived(DerivedDecl {
                    location: derived.location,
                    name: derived.name.clone(),
                    expression,
                }));
            }
            Declaration::Action(action) => {
                declarations.push(Declaration::Action(lower_action(
                    action,
                    &action_signatures,
                    &models,
                    &roots,
                    &mut errors,
                )));
            }
        }
    }

    if errors.is_empty() {
        Ok(Program {
            declarations,
            state_models: Vec::new(),
        })
    } else {
        Err(errors)
    }
}

fn build_models(
    declarations: &[StateModelDecl],
    type_facts: &ModelTypeFacts,
) -> HashMap<String, ModelInfo> {
    let mut models = HashMap::new();

    for model in declarations {
        let resolved = type_facts
            .model(&model.name)
            .expect("validated state model should have shared type facts");
        let mut member_indices = HashMap::new();
        let mut members = Vec::new();

        for member in &model.members {
            let name = member.name().to_string();
            let (kind, expression) = match member {
                StateModelMember::State(state) => {
                    (ModelMemberKind::State, state.initializer.clone())
                }
                StateModelMember::Derived(derived) => {
                    (ModelMemberKind::Derived, derived.expression.clone())
                }
            };
            let value_type = resolved
                .member_type(&name)
                .expect("validated state-model member should have a shared value type")
                .clone();

            member_indices.insert(name.clone(), members.len());
            members.push(ModelMemberInfo {
                name,
                kind,
                value_type,
                expression,
                location: member.location(),
            });
        }

        models.insert(
            model.name.clone(),
            ModelInfo {
                name: model.name.clone(),
                members,
                member_indices,
            },
        );
    }

    models
}

fn check_duplicate_top_level_names(program: &Program, errors: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for declaration in &program.declarations {
        let (name, location) = match declaration {
            Declaration::State(state) => (&state.name, state.location),
            Declaration::Derived(derived) => (&derived.name, derived.location),
            Declaration::Action(action) => (&action.name, action.location),
        };
        if !seen.insert(name.clone()) {
            errors.push(diag(location, format!("duplicate declaration '{}'", name)));
        }
    }
}

fn collect_model_roots(
    program: &Program,
    models: &HashMap<String, ModelInfo>,
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, ModelRoot> {
    let mut roots = HashMap::new();

    for declaration in &program.declarations {
        let Declaration::State(state) = declaration else {
            continue;
        };
        if !state.implicit_model_initializer {
            continue;
        }

        let Some(type_name) = state.type_name.as_deref() else {
            errors.push(diag(
                state.location,
                format!("state '{}' requires an initializer", state.name),
            ));
            continue;
        };

        if models.contains_key(type_name) {
            roots.insert(
                state.name.clone(),
                ModelRoot {
                    model_name: type_name.to_string(),
                },
            );
        } else {
            errors.push(diag(
                state.location,
                format!(
                    "state '{}: {}' requires an initializer; '{}' is not a state model",
                    state.name, type_name, type_name
                ),
            ));
        }
    }

    roots
}

fn expand_model_state(root: &str, model: &ModelInfo, output: &mut Vec<Declaration>) {
    for member in &model.members {
        let generated_name = model_binding_name(root, &member.name);
        let expression = rewrite_model_expr_for_root(&member.expression, root, model);
        match member.kind {
            ModelMemberKind::State => output.push(Declaration::State(StateDecl {
                location: member.location,
                name: generated_name,
                type_name: Some(show_type(&member.value_type)),
                initializer: expression,
                implicit_model_initializer: false,
            })),
            ModelMemberKind::Derived => output.push(Declaration::Derived(DerivedDecl {
                location: member.location,
                name: generated_name,
                expression,
            })),
        }
    }
}

fn rewrite_model_expr_for_root(expression: &Expr, root: &str, model: &ModelInfo) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            debug_assert!(model.member(name).is_some());
            Expr::Name(model_binding_name(root, name))
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_model_expr_for_root(left, root, model)),
            right: Box::new(rewrite_model_expr_for_root(right, root, model)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_model_expr_for_root(condition, root, model)),
            then_branch: Box::new(rewrite_model_expr_for_root(then_branch, root, model)),
            else_branch: Box::new(rewrite_model_expr_for_root(else_branch, root, model)),
        },
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => expression.clone(),
        Expr::Filter { .. } => expression.clone(),
    }
}

fn lower_action(
    action: &ActionDecl,
    action_signatures: &HashMap<String, ActionDecl>,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> ActionDecl {
    let mut context = ActionContext::default();
    let mut parameters = Vec::new();

    for parameter in &action.parameters {
        if let Some(model) = models.get(&parameter.type_name) {
            let mode = match parameter.kind {
                ActionParameterKind::Value => ModelParameterMode::Value,
                ActionParameterKind::State => ModelParameterMode::State,
            };
            let mut state_member_names = HashMap::new();
            for member in model.state_members() {
                let generated_name = model_parameter_member_name(&parameter.name, &member.name);
                state_member_names.insert(member.name.clone(), generated_name.clone());
                parameters.push(ActionParameter {
                    location: parameter.location,
                    kind: parameter.kind,
                    name: generated_name,
                    type_name: show_type(&member.value_type),
                });
            }
            context.model_parameters.insert(
                parameter.name.clone(),
                ModelParameterContext {
                    model_name: model.name.clone(),
                    mode,
                    state_member_names,
                },
            );
        } else {
            parameters.push(parameter.clone());
        }
    }

    let statements = action
        .statements
        .iter()
        .map(|statement| {
            lower_statement(
                statement,
                &context,
                action_signatures,
                models,
                roots,
                errors,
            )
        })
        .collect();

    ActionDecl {
        location: action.location,
        name: action.name.clone(),
        parameters,
        statements,
    }
}

fn lower_statement(
    statement: &Statement,
    context: &ActionContext,
    action_signatures: &HashMap<String, ActionDecl>,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
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
            target: lower_assignment_target(target, *location, context, models, roots, errors),
            operator: *operator,
            value: lower_expr(value, *location, context, models, roots, errors),
        },
        Statement::IndexedThroughAssignment { .. } => {
            unreachable!("source indexed assignments are consumed by sequence lowering")
        }
        Statement::RuntimeDesignationAssignment { .. } => statement.clone(),
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
            index: Box::new(lower_expr(index, *location, context, models, roots, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
            operator: *operator,
            value: lower_expr(value, *location, context, models, roots, errors),
        },
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => Statement::ActionCall {
            location: *location,
            name: name.clone(),
            arguments: lower_action_call(
                action_signatures.get(name),
                arguments,
                *location,
                context,
                models,
                roots,
                errors,
            ),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: lower_expr(message, *location, context, models, roots, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: lower_expr(condition, *location, context, models, roots, errors),
            then_branch: then_branch
                .iter()
                .map(|statement| {
                    lower_statement(statement, context, action_signatures, models, roots, errors)
                })
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| {
                        lower_statement(
                            statement,
                            context,
                            action_signatures,
                            models,
                            roots,
                            errors,
                        )
                    })
                    .collect()
            }),
        },
    }
}

fn lower_action_call(
    signature: Option<&ActionDecl>,
    arguments: &[ActionArgument],
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> Vec<ActionArgument> {
    let Some(signature) = signature else {
        return arguments
            .iter()
            .map(|argument| {
                lower_generic_argument(argument, location, context, models, roots, errors)
            })
            .collect();
    };

    if signature.parameters.len() != arguments.len() {
        errors.push(diag(
            location,
            format!(
                "action '{}' expects {} arguments but got {}",
                signature.name,
                signature.parameters.len(),
                arguments.len()
            ),
        ));
        return Vec::new();
    }

    let mut lowered = Vec::new();

    for (parameter, argument) in signature.parameters.iter().zip(arguments) {
        let Some(model) = models.get(&parameter.type_name) else {
            lowered.push(lower_generic_argument(
                argument, location, context, models, roots, errors,
            ));
            continue;
        };

        match parameter.kind {
            ActionParameterKind::Value => {
                let ActionArgument::Value(Expr::Name(root_name)) = argument else {
                    errors.push(diag(
                        location,
                        format!(
                            "model value parameter '{}' currently requires a named {} value",
                            parameter.name, model.name
                        ),
                    ));
                    continue;
                };

                let Some(actual_model_name) = model_name_for_value_root(root_name, context, roots)
                else {
                    errors.push(diag(
                        location,
                        format!(
                            "'{}' is not a modeled-state value that can be passed to parameter '{}'",
                            root_name, parameter.name
                        ),
                    ));
                    continue;
                };

                if actual_model_name != model.name {
                    errors.push(diag(
                        location,
                        format!(
                            "model value '{}' has type {} but parameter '{}' requires {}",
                            root_name, actual_model_name, parameter.name, model.name
                        ),
                    ));
                    continue;
                }

                for member in model.state_members() {
                    lowered.push(ActionArgument::Value(model_value_member_expr(
                        root_name, member, context, roots,
                    )));
                }
            }
            ActionParameterKind::State => {
                let ActionArgument::StateGrant {
                    location: grant_location,
                    name: root_name,
                } = argument
                else {
                    errors.push(diag(
                        location,
                        format!(
                            "writable modeled-state parameter '{}' requires explicit authority; use 'state <{} variable>'",
                            parameter.name, model.name
                        ),
                    ));
                    continue;
                };

                let Some((actual_model_name, member_names)) =
                    model_state_authority_members(root_name, context, models, roots)
                else {
                    errors.push(diag(
                        *grant_location,
                        format!(
                            "'state {}' does not designate writable modeled state",
                            root_name
                        ),
                    ));
                    continue;
                };

                if actual_model_name != model.name {
                    errors.push(diag(
                        *grant_location,
                        format!(
                            "writable modeled-state argument 'state {}' has type {} but parameter '{}' requires {}",
                            root_name, actual_model_name, parameter.name, model.name
                        ),
                    ));
                    continue;
                }

                for member in model.state_members() {
                    let Some(name) = member_names.get(&member.name) else {
                        errors.push(diag(
                            *grant_location,
                            format!(
                                "internal lowering error: missing writable member '{}.{}'",
                                root_name, member.name
                            ),
                        ));
                        continue;
                    };
                    lowered.push(ActionArgument::StateGrant {
                        location: *grant_location,
                        name: name.clone(),
                    });
                }
            }
        }
    }

    lowered
}

fn lower_generic_argument(
    argument: &ActionArgument,
    fallback_location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> ActionArgument {
    match argument {
        ActionArgument::Value(expression) => ActionArgument::Value(lower_expr(
            expression,
            fallback_location,
            context,
            models,
            roots,
            errors,
        )),
        ActionArgument::IndexedStateGrant { .. } => unreachable!(
            "source indexed state grants are consumed by runtime index grant transport"
        ),
        ActionArgument::StateGrant { location, name } => ActionArgument::StateGrant {
            location: *location,
            name: lower_state_grant(name, *location, context, models, roots, errors),
        },
    }
}

fn lower_expr(
    expression: &Expr,
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => lower_name_expr(name, location, context, models, roots, errors),
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(lower_expr(left, location, context, models, roots, errors)),
            right: Box::new(lower_expr(right, location, context, models, roots, errors)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(
                condition, location, context, models, roots, errors,
            )),
            then_branch: Box::new(lower_expr(
                then_branch,
                location,
                context,
                models,
                roots,
                errors,
            )),
            else_branch: Box::new(lower_expr(
                else_branch,
                location,
                context,
                models,
                roots,
                errors,
            )),
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
            source: source.clone(),
            index: Box::new(lower_expr(index, location, context, models, roots, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => Expr::RuntimeIndexDesignation {
            source: source.clone(),
            index: Box::new(lower_expr(index, location, context, models, roots, errors)),
            element_model: element_model.clone(),
        },
        Expr::RuntimeDesignationMember {
            designation,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeDesignationMember {
            designation: Box::new(lower_expr(
                designation,
                location,
                context,
                models,
                roots,
                errors,
            )),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::Filter { .. } => expression.clone(),
    }
}

fn lower_name_expr(
    name: &str,
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if let Some((root, member)) = split_member_path(name, location, errors) {
        return lower_member_expr(root, member, location, context, models, roots, errors);
    }

    if roots.contains_key(name) || context.model_parameters.contains_key(name) {
        errors.push(diag(
            location,
            format!(
                "whole modeled-state value '{}' is currently supported only when passed to a model-typed value parameter; select a member for ordinary expressions",
                name
            ),
        ));
        return Expr::Integer(0);
    }

    Expr::Name(name.to_string())
}

fn lower_member_expr(
    root: &str,
    member_name: &str,
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if let Some(root_info) = roots.get(root) {
        let Some(model) = models.get(&root_info.model_name) else {
            return Expr::Integer(0);
        };
        let Some(member) = model.member(member_name) else {
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
    }

    if let Some(parameter) = context.model_parameters.get(root) {
        let Some(model) = models.get(&parameter.model_name) else {
            return Expr::Integer(0);
        };
        let Some(member) = model.member(member_name) else {
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
            ModelMemberKind::State => Expr::Name(
                parameter
                    .state_member_names
                    .get(&member.name)
                    .expect("state-model lowering should create every stored member parameter")
                    .clone(),
            ),
            ModelMemberKind::Derived => {
                expand_model_derived_for_parameter(model, member, parameter)
            }
        };
    }

    errors.push(diag(
        location,
        format!("unknown modeled-state root '{}' in member path", root),
    ));
    Expr::Integer(0)
}

fn expand_model_derived_for_parameter(
    model: &ModelInfo,
    member: &ModelMemberInfo,
    parameter: &ModelParameterContext,
) -> Expr {
    rewrite_model_expr_for_parameter(&member.expression, model, parameter)
}

fn rewrite_model_expr_for_parameter(
    expression: &Expr,
    model: &ModelInfo,
    parameter: &ModelParameterContext,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            let member = model
                .member(name)
                .expect("validated model-local expression should reference a known member");
            match member.kind {
                ModelMemberKind::State => Expr::Name(
                    parameter
                        .state_member_names
                        .get(name)
                        .expect("stored member parameter should exist")
                        .clone(),
                ),
                ModelMemberKind::Derived => {
                    rewrite_model_expr_for_parameter(&member.expression, model, parameter)
                }
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_model_expr_for_parameter(left, model, parameter)),
            right: Box::new(rewrite_model_expr_for_parameter(right, model, parameter)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_model_expr_for_parameter(
                condition, model, parameter,
            )),
            then_branch: Box::new(rewrite_model_expr_for_parameter(
                then_branch,
                model,
                parameter,
            )),
            else_branch: Box::new(rewrite_model_expr_for_parameter(
                else_branch,
                model,
                parameter,
            )),
        },
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => expression.clone(),
        Expr::Filter { .. } => expression.clone(),
    }
}

fn lower_assignment_target(
    target: &str,
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> String {
    if let Some((root, member_name)) = split_member_path(target, location, errors) {
        if let Some(root_info) = roots.get(root) {
            let model = &models[&root_info.model_name];
            let Some(member) = model.member(member_name) else {
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
        }

        if let Some(parameter) = context.model_parameters.get(root) {
            let model = &models[&parameter.model_name];
            let Some(member) = model.member(member_name) else {
                errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no member '{}'",
                        model.name, member_name
                    ),
                ));
                return target.to_string();
            };
            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!("cannot assign to derived member '{}.{}'", root, member_name),
                ));
                return target.to_string();
            }
            return parameter
                .state_member_names
                .get(member_name)
                .expect("stored member parameter should exist")
                .clone();
        }

        errors.push(diag(
            location,
            format!("unknown modeled-state root '{}' in assignment target", root),
        ));
        return target.to_string();
    }

    if roots.contains_key(target) || context.model_parameters.contains_key(target) {
        errors.push(diag(
            location,
            format!(
                "whole-state assignment to '{}' is deferred in the first state-model milestone",
                target
            ),
        ));
    }

    target.to_string()
}

fn lower_state_grant(
    name: &str,
    location: SourceLocation,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
    errors: &mut Vec<Diagnostic>,
) -> String {
    if let Some((root, member_name)) = split_member_path(name, location, errors) {
        if let Some(root_info) = roots.get(root) {
            let model = &models[&root_info.model_name];
            let Some(member) = model.member(member_name) else {
                errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no member '{}'",
                        model.name, member_name
                    ),
                ));
                return name.to_string();
            };
            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!(
                        "cannot grant derived member '{}.{}' as writable state",
                        root, member_name
                    ),
                ));
                return name.to_string();
            }
            return model_binding_name(root, member_name);
        }

        if let Some(parameter) = context.model_parameters.get(root) {
            let model = &models[&parameter.model_name];
            let Some(member) = model.member(member_name) else {
                errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no member '{}'",
                        model.name, member_name
                    ),
                ));
                return name.to_string();
            };
            if member.kind == ModelMemberKind::Derived {
                errors.push(diag(
                    location,
                    format!(
                        "cannot grant derived member '{}.{}' as writable state",
                        root, member_name
                    ),
                ));
                return name.to_string();
            }
            if parameter.mode != ModelParameterMode::State {
                errors.push(diag(
                    location,
                    format!(
                        "cannot grant member '{}.{}' from value parameter '{}' as writable state",
                        root, member_name, root
                    ),
                ));
                return name.to_string();
            }
            return parameter
                .state_member_names
                .get(member_name)
                .expect("stored member parameter should exist")
                .clone();
        }

        errors.push(diag(
            location,
            format!(
                "unknown modeled-state root '{}' in writable state grant",
                root
            ),
        ));
        return name.to_string();
    }

    if roots.contains_key(name) || context.model_parameters.contains_key(name) {
        errors.push(diag(
            location,
            format!(
                "whole modeled-state grant 'state {}' is valid only for a model-typed writable parameter",
                name
            ),
        ));
    }

    name.to_string()
}

fn model_name_for_value_root<'a>(
    root: &str,
    context: &'a ActionContext,
    roots: &'a HashMap<String, ModelRoot>,
) -> Option<&'a str> {
    if root.contains('.') {
        return None;
    }
    if let Some(root_info) = roots.get(root) {
        return Some(&root_info.model_name);
    }
    context
        .model_parameters
        .get(root)
        .map(|parameter| parameter.model_name.as_str())
}

fn model_value_member_expr(
    root: &str,
    member: &ModelMemberInfo,
    context: &ActionContext,
    roots: &HashMap<String, ModelRoot>,
) -> Expr {
    if roots.contains_key(root) {
        return Expr::Name(model_binding_name(root, &member.name));
    }

    let parameter = context
        .model_parameters
        .get(root)
        .expect("model value root should resolve to a global root or model parameter");
    Expr::Name(
        parameter
            .state_member_names
            .get(&member.name)
            .expect("stored member parameter should exist")
            .clone(),
    )
}

fn model_state_authority_members(
    root: &str,
    context: &ActionContext,
    models: &HashMap<String, ModelInfo>,
    roots: &HashMap<String, ModelRoot>,
) -> Option<(String, HashMap<String, String>)> {
    if root.contains('.') {
        return None;
    }

    if let Some(root_info) = roots.get(root) {
        let model = models.get(&root_info.model_name)?;
        let member_names = model
            .state_members()
            .map(|member| (member.name.clone(), model_binding_name(root, &member.name)))
            .collect();
        return Some((root_info.model_name.clone(), member_names));
    }

    let parameter = context.model_parameters.get(root)?;
    if parameter.mode != ModelParameterMode::State {
        return None;
    }
    Some((
        parameter.model_name.clone(),
        parameter.state_member_names.clone(),
    ))
}

fn split_member_path<'a>(
    name: &'a str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<(&'a str, &'a str)> {
    let mut parts = name.split('.');
    let first = parts.next()?;
    let second = parts.next()?;
    if parts.next().is_some() {
        errors.push(diag(
            location,
            format!(
                "nested modeled-state member paths are deferred; '{}' has more than one member step",
                name
            ),
        ));
    }
    Some((first, second))
}

fn model_binding_name(root: &str, member: &str) -> String {
    format!("__meld_sm${root}${member}")
}

fn model_parameter_member_name(parameter: &str, member: &str) -> String {
    format!("__meld_smp${parameter}${member}")
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
