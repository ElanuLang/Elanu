use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, decode_through_path, ActionArgument,
    ActionDecl, ActionParameter, ActionParameterKind, AssignmentOperator, Declaration, Expr,
    Program, SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::{
    RuntimeModelMemberKind, RuntimeModelRoot, RuntimeModelTemplate,
};
use crate::runtime_sequence_markers::encode_runtime_sequence_type;
use crate::semantic::ValueType;

pub const CREATE_SCOPE_MARKER_ACTION: &str = "__meld_surface_create_scope_marker";
pub const CREATE_SCOPE_BUILTIN_ACTION: &str = "__meld_surface_create_scope_builtin";
pub const INSERT_SCOPE_MARKER_ACTION: &str = "__meld_surface_insert_scope_marker";
pub const GENERATED_INSERT_ACTION_PREFIX: &str = "__meld_create_scope_insert_";
const GENERATED_SCOPE_ACTION_PREFIX: &str = "__meld_create_scope_body_";
const GENERATED_SCOPE_MEMBER_PREFIX: &str = "__meld_create_scope_member_";
const GENERATED_SCOPE_IDENTITY_PREFIX: &str = "__meld_create_scope_identity_";

/// Experimental surface transport for:
///
/// `create T in owner as name { ... }`
///
/// The temporary `if true` carrier is consumed by `lower_scopes` before
/// ordinary semantic checking. It is bootstrap representation, not source law.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    for reserved in [
        CREATE_SCOPE_MARKER_ACTION,
        CREATE_SCOPE_BUILTIN_ACTION,
        INSERT_SCOPE_MARKER_ACTION,
        GENERATED_INSERT_ACTION_PREFIX,
        GENERATED_SCOPE_ACTION_PREFIX,
        GENERATED_SCOPE_MEMBER_PREFIX,
        GENERATED_SCOPE_IDENTITY_PREFIX,
    ] {
        if source.contains(reserved) {
            return Err(vec![Diagnostic::new(
                "reserved compiler scoped-create marker cannot appear in source",
                1,
                1,
            )]);
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    let mut saw_scope = false;

    while index < source.len() {
        if source.as_bytes()[index] == b'"' {
            let end = scan_string(source, index);
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        if source[index..].starts_with("//") {
            let end = source[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            output.push_str(&source[index..end]);
            index = end;
            continue;
        }

        if keyword_at(source, index, "insert") {
            let mut insert_index = index + "insert".len();
            insert_index = skip_inline_whitespace(source, insert_index);
            if let Some((binding, next)) = parse_identifier(source, insert_index) {
                insert_index = skip_inline_whitespace(source, next);
                if keyword_at(source, insert_index, "into") {
                    insert_index += "into".len();
                    insert_index = skip_inline_whitespace(source, insert_index);
                    if let Some((target, next)) = parse_name_path(source, insert_index) {
                        output.push_str(INSERT_SCOPE_MARKER_ACTION);
                        output.push_str("(\"");
                        output.push_str(&binding);
                        output.push_str("\", state ");
                        output.push_str(&target);
                        output.push(')');
                        index = next;
                        continue;
                    }
                }
            }
        }

        if keyword_at(source, index, "create") {
            if let Some((head_end, model, owner)) = parse_create_head(source, index) {
                let mut scope_index = skip_inline_whitespace(source, head_end);
                if keyword_at(source, scope_index, "as") {
                    scope_index += "as".len();
                    scope_index = skip_inline_whitespace(source, scope_index);
                    if let Some((binding, next)) = parse_identifier(source, scope_index) {
                        scope_index = skip_inline_whitespace(source, next);
                        if source.as_bytes().get(scope_index) == Some(&b'{') {
                            output.push_str("if true {\n");
                            output.push_str(CREATE_SCOPE_MARKER_ACTION);
                            output.push_str("(\"");
                            output.push_str(&model);
                            output.push_str("\", ");
                            output.push_str(&owner);
                            output.push_str(", \"");
                            output.push_str(&binding);
                            output.push_str("\")\n");
                            index = scope_index + 1;
                            saw_scope = true;
                            continue;
                        }
                    }
                }
            }
        }

        let ch = source[index..]
            .chars()
            .next()
            .expect("index should remain on a character boundary");
        output.push(ch);
        index += ch.len_utf8();
    }

    if saw_scope {
        output.push_str("\naction ");
        output.push_str(CREATE_SCOPE_MARKER_ACTION);
        output.push_str("(modelName: String, ownerName: String, bindingName: String) {}\n");
        output.push_str("action ");
        output.push_str(CREATE_SCOPE_BUILTIN_ACTION);
        output.push_str("(modelName: String, ownerName: String, scopeAction: String) {}\n");
    }

    Ok(output)
}

pub fn validate(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<(), Vec<Diagnostic>> {
    let persistent_owners = collect_persistent_designations(program);
    let mut errors = Vec::new();
    for declaration in &program.declarations {
        if let Declaration::Action(action) = declaration {
            validate_statements(
                &action.statements,
                templates,
                roots,
                &persistent_owners,
                &HashSet::new(),
                &mut errors,
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn collect_persistent_designations(program: &Program) -> HashMap<String, String> {
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

pub struct ScopedCreateLowering {
    pub program: Program,
    pub designation_models: HashMap<String, String>,
}

pub fn lower_scopes(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<ScopedCreateLowering, Vec<Diagnostic>> {
    let persistent_designations = collect_persistent_designations(program);
    let mut lowerer = ScopeLowerer {
        templates,
        roots,
        persistent_designations: &persistent_designations,
        next_scope_id: 0,
        generated_actions: Vec::new(),
        designation_models: HashMap::new(),
        errors: Vec::new(),
    };

    let mut declarations = Vec::with_capacity(program.declarations.len());
    for declaration in &program.declarations {
        match declaration {
            Declaration::Action(action) => {
                declarations.push(Declaration::Action(ActionDecl {
                    location: action.location,
                    name: action.name.clone(),
                    parameters: action.parameters.clone(),
                    statements: lowerer.lower_statements(&action.statements),
                }));
            }
            other => declarations.push(other.clone()),
        }
    }

    if !lowerer.errors.is_empty() {
        return Err(lowerer.errors);
    }
    let designation_models = lowerer.designation_models;
    declarations.extend(lowerer.generated_actions);
    Ok(ScopedCreateLowering {
        program: Program {
            declarations,
            state_models: program.state_models.clone(),
        },
        designation_models,
    })
}

struct ScopeLowerer<'a> {
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    roots: &'a HashMap<String, RuntimeModelRoot>,
    persistent_designations: &'a HashMap<String, String>,
    next_scope_id: usize,
    generated_actions: Vec<Declaration>,
    designation_models: HashMap<String, String>,
    errors: Vec<Diagnostic>,
}

impl ScopeLowerer<'_> {
    fn lower_statements(&mut self, statements: &[Statement]) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| {
                if let Some(scope) = decode_scope_carrier(statement) {
                    return self.lower_scope(scope);
                }
                match statement {
                    Statement::If {
                        location,
                        condition,
                        then_branch,
                        else_branch,
                    } => Statement::If {
                        location: *location,
                        condition: condition.clone(),
                        then_branch: self.lower_statements(then_branch),
                        else_branch: else_branch
                            .as_ref()
                            .map(|branch| self.lower_statements(branch)),
                    },
                    other => other.clone(),
                }
            })
            .collect()
    }

    fn lower_scope(&mut self, scope: CreateScopeCarrier<'_>) -> Statement {
        let scope_id = self.next_scope_id;
        self.next_scope_id += 1;
        let action_name = format!("{GENERATED_SCOPE_ACTION_PREFIX}{scope_id}");
        let insert_action_name = format!("{GENERATED_INSERT_ACTION_PREFIX}{scope_id}");
        let identity_param = format!("{GENERATED_SCOPE_IDENTITY_PREFIX}{scope_id}");
        self.designation_models
            .insert(identity_param.clone(), scope.model.to_string());

        let Some(template) = self.templates.get(scope.model).cloned() else {
            return scope_call(
                scope.location,
                scope.model,
                owner_expression(scope.owner, self.roots),
                &action_name,
            );
        };

        let mut allowed_insert_targets = Vec::new();
        for (root_name, root) in self.roots {
            let Some(owner_template) = self.templates.get(&root.model_name) else {
                continue;
            };
            for member in &owner_template.members {
                if matches!(
                    &member.value_type,
                    ValueType::SequenceLive(model) if model == scope.model
                ) {
                    allowed_insert_targets.push(format!("{root_name}.{}", member.name));
                }
            }
        }

        for (designation_name, owner_model) in self.persistent_designations {
            let Some(owner_template) = self.templates.get(owner_model) else {
                continue;
            };
            for member in &owner_template.members {
                if member.kind == RuntimeModelMemberKind::State
                    && matches!(
                        &member.value_type,
                        ValueType::SequenceLive(model) if model == scope.model
                    )
                {
                    allowed_insert_targets.push(format!("{designation_name}.{}", member.name));
                }
            }
        }

        let state_members: Vec<_> = template
            .members
            .iter()
            .filter(|member| member.kind == RuntimeModelMemberKind::State)
            .collect();
        let member_params: HashMap<_, _> = state_members
            .iter()
            .map(|member| {
                (
                    member.name.clone(),
                    format!("{GENERATED_SCOPE_MEMBER_PREFIX}{scope_id}_{}", member.name),
                )
            })
            .collect();

        let mut parameters = vec![ActionParameter {
            location: scope.location,
            kind: ActionParameterKind::Value,
            name: identity_param.clone(),
            type_name: "String".to_string(),
        }];
        parameters.extend(state_members.iter().map(|member| ActionParameter {
            location: scope.location,
            kind: ActionParameterKind::State,
            name: member_params[&member.name].clone(),
            type_name: lowered_type_name(&member.value_type),
        }));

        let nested = self.lower_statements(scope.body);
        let statements = nested
            .iter()
            .map(|statement| {
                let context = ScopeRewriteContext {
                    binding: scope.binding,
                    identity_param: &identity_param,
                    insert_action_name: &insert_action_name,
                    allowed_insert_targets: &allowed_insert_targets,
                    template: &template,
                    member_params: &member_params,
                    persistent_designations: self.persistent_designations,
                };
                rewrite_statement(statement, &context, &mut self.errors)
            })
            .collect();

        self.generated_actions.push(Declaration::Action(ActionDecl {
            location: scope.location,
            name: action_name.clone(),
            parameters,
            statements,
        }));
        self.generated_actions.push(Declaration::Action(ActionDecl {
            location: scope.location,
            name: insert_action_name,
            parameters: vec![
                ActionParameter {
                    location: scope.location,
                    kind: ActionParameterKind::Value,
                    name: "identity".to_string(),
                    type_name: "String".to_string(),
                },
                ActionParameter {
                    location: scope.location,
                    kind: ActionParameterKind::State,
                    name: "target".to_string(),
                    type_name: encode_runtime_sequence_type(scope.model),
                },
            ],
            statements: Vec::new(),
        }));

        scope_call(
            scope.location,
            scope.model,
            owner_expression(scope.owner, self.roots),
            &action_name,
        )
    }
}

struct CreateScopeCarrier<'a> {
    location: SourceLocation,
    model: &'a str,
    owner: &'a str,
    binding: &'a str,
    body: &'a [Statement],
}

fn decode_scope_carrier(statement: &Statement) -> Option<CreateScopeCarrier<'_>> {
    let Statement::If {
        location,
        condition: Expr::Bool(true),
        then_branch,
        else_branch: None,
    } = statement
    else {
        return None;
    };
    let (first, body) = then_branch.split_first()?;
    let Statement::ActionCall {
        name, arguments, ..
    } = first
    else {
        return None;
    };
    if name != CREATE_SCOPE_MARKER_ACTION {
        return None;
    }
    let [ActionArgument::Value(Expr::String(model)), ActionArgument::Value(Expr::Name(owner)), ActionArgument::Value(Expr::String(binding))] =
        arguments.as_slice()
    else {
        return None;
    };
    Some(CreateScopeCarrier {
        location: *location,
        model,
        owner,
        binding,
        body,
    })
}

fn owner_expression(owner: &str, roots: &HashMap<String, RuntimeModelRoot>) -> Expr {
    if roots.contains_key(owner) {
        Expr::String(owner.to_string())
    } else {
        Expr::Name(owner.to_string())
    }
}

fn scope_call(location: SourceLocation, model: &str, owner: Expr, action: &str) -> Statement {
    Statement::ActionCall {
        location,
        name: CREATE_SCOPE_BUILTIN_ACTION.to_string(),
        arguments: vec![
            ActionArgument::Value(Expr::String(model.to_string())),
            ActionArgument::Value(owner),
            ActionArgument::Value(Expr::String(action.to_string())),
        ],
    }
}

fn validate_statements(
    statements: &[Statement],
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
    persistent_owners: &HashMap<String, String>,
    scoped_owners: &HashSet<String>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        if let Some(scope) = decode_scope_carrier(statement) {
            if !templates.contains_key(scope.model) {
                errors.push(Diagnostic::new(
                    format!("unknown state model '{}' in scoped create", scope.model),
                    scope.location.line,
                    scope.location.column,
                ));
            }
            if !(roots.contains_key(scope.owner)
                || persistent_owners.contains_key(scope.owner)
                || scoped_owners.contains(scope.owner))
            {
                errors.push(Diagnostic::new(
                    format!(
                        "'{}' is not a modeled-state root or live owner designation",
                        scope.owner
                    ),
                    scope.location.line,
                    scope.location.column,
                ));
            }
            if scope.binding.is_empty() {
                errors.push(Diagnostic::new(
                    "scoped create designation name cannot be empty",
                    scope.location.line,
                    scope.location.column,
                ));
            }

            let mut nested_owners = scoped_owners.clone();
            nested_owners.insert(scope.binding.to_string());
            validate_statements(
                scope.body,
                templates,
                roots,
                persistent_owners,
                &nested_owners,
                errors,
            );
            continue;
        }

        if let Statement::If {
            then_branch,
            else_branch,
            ..
        } = statement
        {
            validate_statements(
                then_branch,
                templates,
                roots,
                persistent_owners,
                scoped_owners,
                errors,
            );
            if let Some(branch) = else_branch {
                validate_statements(
                    branch,
                    templates,
                    roots,
                    persistent_owners,
                    scoped_owners,
                    errors,
                );
            }
        }
    }
}

struct ScopeRewriteContext<'a> {
    binding: &'a str,
    identity_param: &'a str,
    insert_action_name: &'a str,
    allowed_insert_targets: &'a [String],
    template: &'a RuntimeModelTemplate,
    member_params: &'a HashMap<String, String>,
    persistent_designations: &'a HashMap<String, String>,
}

fn rewrite_statement(
    statement: &Statement,
    context: &ScopeRewriteContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    let binding = context.binding;
    let template = context.template;
    let member_params = context.member_params;
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            if matches!(value, Expr::Name(name) if name == binding) {
                if *operator != AssignmentOperator::Assign {
                    errors.push(Diagnostic::new(
                        format!(
                            "scoped live designation '{binding}' supports only direct persistence assignment"
                        ),
                        location.line,
                        location.column,
                    ));
                    return statement.clone();
                }
                let Some(target_model) = context.persistent_designations.get(target) else {
                    errors.push(Diagnostic::new(
                        format!(
                            "scoped live designation '{binding}' may only be persisted into compatible persistent live or maybe-live designation state"
                        ),
                        location.line,
                        location.column,
                    ));
                    return statement.clone();
                };
                if target_model != &template.name {
                    errors.push(Diagnostic::new(
                        format!(
                            "scoped live designation '{binding}' has type live {} but persistent designation '{target}' requires live {target_model}",
                            template.name
                        ),
                        location.line,
                        location.column,
                    ));
                    return statement.clone();
                }
                return Statement::Assignment {
                    location: *location,
                    target: target.clone(),
                    operator: *operator,
                    value: Expr::Name(context.identity_param.to_string()),
                };
            }

            let value = rewrite_expr(value, *location, binding, template, member_params, errors);
            if let Some(path) = decode_through_path(target) {
                if let Some(member) = scoped_member(path, binding) {
                    let target = writable_member(
                        member,
                        *location,
                        binding,
                        template,
                        member_params,
                        errors,
                    )
                    .unwrap_or_else(|| target.clone());
                    return Statement::Assignment {
                        location: *location,
                        target,
                        operator: *operator,
                        value,
                    };
                }
            }
            if let Some(member) = scoped_member(target, binding) {
                errors.push(Diagnostic::new(
                    format!(
                        "indirect mutation through scoped designation '{binding}' requires 'through {binding}.{member}'"
                    ),
                    location.line,
                    location.column,
                ));
            }
            Statement::Assignment {
                location: *location,
                target: target.clone(),
                operator: *operator,
                value,
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
            source: source.clone(),
            index: rewrite_expr(index, *location, binding, template, member_params, errors),
            member: member.clone(),
            operator: *operator,
            value: rewrite_expr(value, *location, binding, template, member_params, errors),
        },
        Statement::RuntimeIndexAssignment { .. }
        | Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime index lowering runs after scoped-create lowering")
        }
        Statement::ActionCall {
            location,
            name,
            arguments,
        } if name == CREATE_SCOPE_BUILTIN_ACTION => {
            let rewritten = arguments
                .iter()
                .enumerate()
                .map(|(index, argument)| {
                    if index == 1 {
                        if let ActionArgument::Value(Expr::Name(owner)) = argument {
                            if owner == binding {
                                return ActionArgument::Value(Expr::Name(
                                    context.identity_param.to_string(),
                                ));
                            }
                        }
                    }
                    rewrite_argument(
                        argument,
                        *location,
                        binding,
                        template,
                        member_params,
                        errors,
                    )
                })
                .collect();
            Statement::ActionCall {
                location: *location,
                name: name.clone(),
                arguments: rewritten,
            }
        }
        Statement::ActionCall {
            location,
            name,
            arguments,
        } if name == INSERT_SCOPE_MARKER_ACTION => {
            let [ActionArgument::Value(Expr::String(insert_binding)), ActionArgument::StateGrant {
                location: grant_location,
                name: target,
            }] = arguments.as_slice()
            else {
                errors.push(Diagnostic::new(
                    "internal scoped insertion marker is malformed",
                    location.line,
                    location.column,
                ));
                return statement.clone();
            };

            if insert_binding != binding {
                errors.push(Diagnostic::new(
                    format!("scoped insertion must use the current fresh designation '{binding}'"),
                    location.line,
                    location.column,
                ));
            }
            if !context
                .allowed_insert_targets
                .iter()
                .any(|allowed| allowed == target)
            {
                errors.push(Diagnostic::new(
                    format!(
                        "scoped insertion target '{target}' must be a compatible mutable [live {}] owner-relative member",
                        template.name
                    ),
                    grant_location.line,
                    grant_location.column,
                ));
            }

            Statement::ActionCall {
                location: *location,
                name: context.insert_action_name.to_string(),
                arguments: vec![
                    ActionArgument::Value(Expr::Name(context.identity_param.to_string())),
                    ActionArgument::StateGrant {
                        location: *grant_location,
                        name: target.clone(),
                    },
                ],
            }
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
                .map(|argument| {
                    rewrite_argument(
                        argument,
                        *location,
                        binding,
                        template,
                        member_params,
                        errors,
                    )
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: rewrite_expr(message, *location, binding, template, member_params, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: rewrite_expr(
                condition,
                *location,
                binding,
                template,
                member_params,
                errors,
            ),
            then_branch: then_branch
                .iter()
                .map(|statement| rewrite_statement(statement, context, errors))
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| rewrite_statement(statement, context, errors))
                    .collect()
            }),
        },
    }
}

fn rewrite_argument(
    argument: &ActionArgument,
    location: SourceLocation,
    binding: &str,
    template: &RuntimeModelTemplate,
    member_params: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> ActionArgument {
    match argument {
        ActionArgument::Value(expression) => ActionArgument::Value(rewrite_expr(
            expression,
            location,
            binding,
            template,
            member_params,
            errors,
        )),
        ActionArgument::StateGrant {
            location: grant_location,
            name,
        } => {
            if let Some(path) = decode_through_path(name) {
                if let Some(member) = scoped_member(path, binding) {
                    if let Some(name) = writable_member(
                        member,
                        *grant_location,
                        binding,
                        template,
                        member_params,
                        errors,
                    ) {
                        return ActionArgument::StateGrant {
                            location: *grant_location,
                            name,
                        };
                    }
                }
            }
            if let Some(member) = scoped_member(name, binding) {
                errors.push(Diagnostic::new(
                    format!(
                        "writable authority through scoped designation '{binding}' requires 'state through {binding}.{member}'"
                    ),
                    grant_location.line,
                    grant_location.column,
                ));
            }
            ActionArgument::StateGrant {
                location: *grant_location,
                name: name.clone(),
            }
        }
        ActionArgument::IndexedStateGrant {
            location: grant_location,
            source,
            index,
            member,
        } => ActionArgument::IndexedStateGrant {
            location: *grant_location,
            source: source.clone(),
            index: rewrite_expr(
                index,
                *grant_location,
                binding,
                template,
                member_params,
                errors,
            ),
            member: member.clone(),
        },
    }
}

fn rewrite_expr(
    expression: &Expr,
    location: SourceLocation,
    binding: &str,
    template: &RuntimeModelTemplate,
    member_params: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            if name == binding {
                errors.push(Diagnostic::new(
                    format!("scoped live designation '{binding}' is not an ordinary whole value"),
                    location.line,
                    location.column,
                ));
                return Expr::Integer(0);
            }
            if let Some(member) = scoped_member(name, binding) {
                return read_member(member, location, binding, template, member_params, errors);
            }
            Expr::Name(name.clone())
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_expr(
                left,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
            right: Box::new(rewrite_expr(
                right,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_expr(
                condition,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
            then_branch: Box::new(rewrite_expr(
                then_branch,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
            else_branch: Box::new(rewrite_expr(
                else_branch,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
        },
        Expr::Filter { .. } => {
            errors.push(Diagnostic::new(
                "scoped create does not yet compose with a nested filter expression",
                location.line,
                location.column,
            ));
            expression.clone()
        }
        Expr::IndexedMember {
            source,
            index,
            member,
        } => Expr::IndexedMember {
            source: source.clone(),
            index: Box::new(rewrite_expr(
                index,
                location,
                binding,
                template,
                member_params,
                errors,
            )),
            member: member.clone(),
        },
        Expr::IndexedDesignation { source, index } => Expr::IndexedDesignation {
            source: source.clone(),
            index: index.clone(),
        },
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after scoped-create lowering")
        }
    }
}

fn read_member(
    member_name: &str,
    location: SourceLocation,
    binding: &str,
    template: &RuntimeModelTemplate,
    member_params: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Some(member) = template.member(member_name) else {
        errors.push(Diagnostic::new(
            format!(
                "state model '{}' has no member '{}' for scoped designation '{}'",
                template.name, member_name, binding
            ),
            location.line,
            location.column,
        ));
        return Expr::Integer(0);
    };
    match member.kind {
        RuntimeModelMemberKind::State => Expr::Name(member_params[member_name].clone()),
        RuntimeModelMemberKind::Derived => rewrite_model_expr(
            &member.expression,
            location,
            template,
            member_params,
            errors,
        ),
    }
}

fn writable_member(
    member_name: &str,
    location: SourceLocation,
    binding: &str,
    template: &RuntimeModelTemplate,
    member_params: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Option<String> {
    let Some(member) = template.member(member_name) else {
        errors.push(Diagnostic::new(
            format!(
                "state model '{}' has no member '{}' for scoped designation '{}'",
                template.name, member_name, binding
            ),
            location.line,
            location.column,
        ));
        return None;
    };
    if member.kind != RuntimeModelMemberKind::State {
        errors.push(Diagnostic::new(
            format!("cannot mutate derived member '{binding}.{member_name}'"),
            location.line,
            location.column,
        ));
        return None;
    }
    member_params.get(member_name).cloned()
}

fn rewrite_model_expr(
    expression: &Expr,
    location: SourceLocation,
    template: &RuntimeModelTemplate,
    member_params: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            let Some(member) = template.member(name) else {
                errors.push(Diagnostic::new(
                    format!(
                        "model-local expression for '{}' references unknown member '{}'",
                        template.name, name
                    ),
                    location.line,
                    location.column,
                ));
                return Expr::Integer(0);
            };
            match member.kind {
                RuntimeModelMemberKind::State => Expr::Name(member_params[name].clone()),
                RuntimeModelMemberKind::Derived => rewrite_model_expr(
                    &member.expression,
                    location,
                    template,
                    member_params,
                    errors,
                ),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_model_expr(
                left,
                location,
                template,
                member_params,
                errors,
            )),
            right: Box::new(rewrite_model_expr(
                right,
                location,
                template,
                member_params,
                errors,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_model_expr(
                condition,
                location,
                template,
                member_params,
                errors,
            )),
            then_branch: Box::new(rewrite_model_expr(
                then_branch,
                location,
                template,
                member_params,
                errors,
            )),
            else_branch: Box::new(rewrite_model_expr(
                else_branch,
                location,
                template,
                member_params,
                errors,
            )),
        },
        Expr::Filter { .. } => {
            errors.push(Diagnostic::new(
                "scoped create derived read does not yet expand model-local filter",
                location.line,
                location.column,
            ));
            Expr::Integer(0)
        }
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            errors.push(Diagnostic::new(
                "scoped create derived read does not yet expand runtime indexed access",
                location.line,
                location.column,
            ));
            Expr::Integer(0)
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after scoped-create lowering")
        }
    }
}

fn scoped_member<'a>(path: &'a str, binding: &str) -> Option<&'a str> {
    let prefix = format!("{binding}.");
    let member = path.strip_prefix(&prefix)?;
    (!member.is_empty() && !member.contains('.')).then_some(member)
}

fn lowered_type_name(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Int => "Int".to_string(),
        ValueType::Float => "Float".to_string(),
        ValueType::Bool => "Bool".to_string(),
        ValueType::String => "String".to_string(),
        ValueType::SequenceLive(model) => encode_runtime_sequence_type(model),
        ValueType::Named(name) => name.clone(),
    }
}

fn parse_create_head(source: &str, start: usize) -> Option<(usize, String, String)> {
    let mut index = start + "create".len();
    index = skip_inline_whitespace(source, index);
    let (model, next) = parse_identifier(source, index)?;
    index = skip_inline_whitespace(source, next);
    if !keyword_at(source, index, "in") {
        return None;
    }
    index += "in".len();
    index = skip_inline_whitespace(source, index);
    let (owner, next) = parse_name_path(source, index)?;
    Some((next, model, owner))
}

fn keyword_at(source: &str, index: usize, keyword: &str) -> bool {
    if !source[index..].starts_with(keyword) {
        return false;
    }
    let before_ok = index == 0
        || !source[..index]
            .chars()
            .next_back()
            .is_some_and(is_identifier_continue);
    let end = index + keyword.len();
    let after_ok = end == source.len()
        || !source[end..]
            .chars()
            .next()
            .is_some_and(is_identifier_continue);
    before_ok && after_ok
}

fn parse_identifier(source: &str, start: usize) -> Option<(String, usize)> {
    let mut chars = source[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_identifier_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (offset, ch) in chars {
        if !is_identifier_continue(ch) {
            break;
        }
        end = start + offset + ch.len_utf8();
    }
    Some((source[start..end].to_string(), end))
}

fn parse_name_path(source: &str, start: usize) -> Option<(String, usize)> {
    let (mut path, mut index) = parse_identifier(source, start)?;
    loop {
        if source.as_bytes().get(index) != Some(&b'.') {
            break;
        }
        let (segment, next) = parse_identifier(source, index + 1)?;
        path.push('.');
        path.push_str(&segment);
        index = next;
    }
    Some((path, index))
}

fn skip_inline_whitespace(source: &str, mut index: usize) -> usize {
    while let Some(byte) = source.as_bytes().get(index) {
        if matches!(byte, b' ' | b'\t') {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn scan_string(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index = (index + 2).min(bytes.len()),
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    bytes.len()
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    is_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocesses_scoped_create_to_private_carrier() {
        let source = r#"action add {
    create LineItem in invoice as line {
        through line.quantity = 3
    }
}
"#;
        let output = preprocess(source).unwrap();
        assert!(output.contains(
            "if true {\n__meld_surface_create_scope_marker(\"LineItem\", invoice, \"line\")"
        ));
        assert!(output.contains("through line.quantity = 3"));
    }

    #[test]
    fn scoped_create_checks_member_mutation_and_derived_read() {
        let source = r#"
state model LineItem {
    state quantity = 2
    state unitPrice = 5.0
    derived lineTotal = quantity * unitPrice
}
state model Invoice {
    state count = 0
}
state invoice: Invoice

action add {
    create LineItem in invoice as line {
        through line.quantity = 3
        if line.lineTotal > 10.0 {
            through line.quantity += 1
        }
    }
}
"#;
        crate::check_source(source).expect("scoped create should check");
    }

    #[test]
    fn scoped_create_requires_through_for_mutation() {
        let source = r#"
state model LineItem {
    state quantity = 2
}
state model Invoice {
    state count = 0
}
state invoice: Invoice

action add {
    create LineItem in invoice as line {
        line.quantity = 3
    }
}
"#;
        let errors = crate::check_source(source).expect_err("mutation should require through");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("requires 'through line.quantity'")));
    }
}
