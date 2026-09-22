use std::collections::HashMap;

use crate::ast::{
    decode_live_capture, decode_live_type_name, decode_maybe_live_type_name, decode_through_path,
    encode_through_path, ActionArgument, ActionDecl, ActionParameter, ActionParameterKind,
    BinaryOperator, Declaration, Expr, Program, SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::{
    RuntimeModelMemberKind, RuntimeModelRoot, RuntimeModelTemplate,
};
use crate::runtime_sequence_markers::encode_runtime_sequence_type;
use crate::semantic::ValueType;

pub(crate) const SCOPE_MARKER_ACTION: &str = "__meld_surface_designation_scope_marker";
pub(crate) const SCOPE_BUILTIN_ACTION: &str = "__meld_surface_designation_scope_builtin";
const GENERATED_SCOPE_ACTION_PREFIX: &str = "__meld_designation_scope_body_";
const GENERATED_SCOPE_IDENTITY_PREFIX: &str = "__meld_designation_scope_identity$";
const GENERATED_SCOPE_MEMBER_PREFIX: &str = "__meld_designation_scope_member_";

pub(crate) fn encode_scope_identity_param(model: &str, scope_id: usize) -> String {
    format!("{GENERATED_SCOPE_IDENTITY_PREFIX}{model}${scope_id}")
}

pub(crate) fn decode_scope_identity_param(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(GENERATED_SCOPE_IDENTITY_PREFIX)?;
    let (model, _) = rest.split_once('$')?;
    (!model.is_empty()).then_some(model)
}

/// Bootstrap surface transport for the provisional scoped designation form:
///
/// `with <designation> as name { ... }`
///
/// The preprocessor preserves the designation expression structurally as an action
/// argument. `lower_scopes` consumes the temporary `if true` carrier before semantic
/// checking. The carrier spelling and generated names are implementation representation,
/// not language law.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    for reserved in [
        SCOPE_MARKER_ACTION,
        SCOPE_BUILTIN_ACTION,
        GENERATED_SCOPE_ACTION_PREFIX,
        GENERATED_SCOPE_IDENTITY_PREFIX,
        GENERATED_SCOPE_MEMBER_PREFIX,
    ] {
        if source.contains(reserved) {
            return Err(vec![Diagnostic::new(
                "reserved compiler scoped-designation marker cannot appear in source",
                1,
                1,
            )]);
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0usize;
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

        if keyword_at(source, index, "with") {
            match parse_with_head(source, index) {
                Some((expression, binding, body_start)) => {
                    output.push_str("if true {\n");
                    output.push_str(SCOPE_MARKER_ACTION);
                    output.push_str("(\"");
                    output.push_str(&binding);
                    output.push_str("\", ");
                    output.push_str(expression);
                    output.push_str(")\n");
                    index = body_start;
                    saw_scope = true;
                    continue;
                }
                None => {
                    return Err(vec![Diagnostic::new(
                        "scoped designation requires 'with <designation> as <name> { ... }'",
                        1,
                        1,
                    )]);
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
        output.push_str(SCOPE_MARKER_ACTION);
        output.push_str("(bindingName: String, designation: String) {}\n");
        output.push_str("action ");
        output.push_str(SCOPE_BUILTIN_ACTION);
        output.push_str("(modelName: String, designation: String, scopeAction: String) {}\n");
    }

    Ok(output)
}

pub fn lower_scopes(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<Program, Vec<Diagnostic>> {
    let designation_models = collect_designation_models(program);
    let mut lowerer = ScopeLowerer {
        templates,
        roots,
        designation_models,
        next_scope_id: 0,
        generated_actions: Vec::new(),
        errors: Vec::new(),
    };

    let mut declarations = Vec::with_capacity(program.declarations.len());
    for declaration in &program.declarations {
        match declaration {
            Declaration::Action(action) => declarations.push(Declaration::Action(ActionDecl {
                location: action.location,
                name: action.name.clone(),
                parameters: action.parameters.clone(),
                statements: lowerer.lower_statements(&action.statements),
            })),
            other => declarations.push(other.clone()),
        }
    }

    if !lowerer.errors.is_empty() {
        return Err(lowerer.errors);
    }

    declarations.extend(lowerer.generated_actions);
    Ok(Program {
        declarations,
        state_models: program.state_models.clone(),
    })
}

struct ScopeLowerer<'a> {
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    roots: &'a HashMap<String, RuntimeModelRoot>,
    designation_models: HashMap<String, String>,
    next_scope_id: usize,
    generated_actions: Vec<Declaration>,
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

    fn lower_scope(&mut self, scope: ScopeCarrier<'_>) -> Statement {
        let Some(model) = self.infer_designation_model(scope.designation, scope.location) else {
            return malformed_scope(scope.location);
        };
        let Some(template) = self.templates.get(&model).cloned() else {
            self.errors.push(diag(
                scope.location,
                format!("missing runtime model template for '{model}'"),
            ));
            return malformed_scope(scope.location);
        };

        let scope_id = self.next_scope_id;
        self.next_scope_id += 1;
        let action_name = format!("{GENERATED_SCOPE_ACTION_PREFIX}{scope_id}");
        let identity_param = encode_scope_identity_param(&model, scope_id);

        let state_members = template
            .members
            .iter()
            .filter(|member| member.kind == RuntimeModelMemberKind::State)
            .collect::<Vec<_>>();
        let member_params = state_members
            .iter()
            .map(|member| {
                (
                    member.name.clone(),
                    format!("{GENERATED_SCOPE_MEMBER_PREFIX}{scope_id}_{}", member.name),
                )
            })
            .collect::<HashMap<_, _>>();

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
            type_name: runtime_value_type_name(&member.value_type),
        }));

        let context = RewriteContext {
            binding: scope.binding,
            identity_param: &identity_param,
            model: &model,
            template: &template,
            member_params: &member_params,
        };
        let statements = scope
            .body
            .iter()
            .map(|statement| rewrite_statement(statement, &context, &mut self.errors))
            .collect::<Vec<_>>();

        self.generated_actions.push(Declaration::Action(ActionDecl {
            location: scope.location,
            name: action_name.clone(),
            parameters,
            statements,
        }));

        Statement::ActionCall {
            location: scope.location,
            name: SCOPE_BUILTIN_ACTION.to_string(),
            arguments: vec![
                ActionArgument::Value(Expr::String(model)),
                ActionArgument::Value(scope.designation.clone()),
                ActionArgument::Value(Expr::String(action_name)),
            ],
        }
    }

    fn infer_designation_model(
        &mut self,
        expression: &Expr,
        location: SourceLocation,
    ) -> Option<String> {
        match expression {
            Expr::Name(name) => {
                if let Some(model) = decode_scope_identity_param(name) {
                    return Some(model.to_string());
                }
                if let Some(model) = self.designation_models.get(name) {
                    return Some(model.clone());
                }
                if let Some(source) = indexed_source_path(name) {
                    return self.sequence_model_for_path(source, location);
                }
                if let Some(root_name) = decode_live_capture(name) {
                    let Some(root) = self.roots.get(root_name) else {
                        self.errors.push(diag(
                            location,
                            format!("'live {root_name}' does not name a modeled-state root"),
                        ));
                        return None;
                    };
                    return Some(root.model_name.clone());
                }
                self.errors.push(diag(
                    location,
                    format!("scoped designation source '{name}' is not a live designation"),
                ));
                None
            }
            Expr::IndexedDesignation { source, .. } => {
                self.sequence_model_for_path(source, location)
            }
            Expr::RuntimeIndexDesignation { element_model, .. } => Some(element_model.clone()),
            Expr::Binary { operator, left, .. }
                if matches!(
                    *operator,
                    BinaryOperator::PreviousIn | BinaryOperator::NextIn
                ) =>
            {
                self.infer_designation_model(left, location)
            }
            Expr::If {
                then_branch,
                else_branch,
                ..
            } => {
                let left = self.infer_designation_model(then_branch, location)?;
                let right = self.infer_designation_model(else_branch, location)?;
                if left == right {
                    Some(left)
                } else {
                    self.errors.push(diag(
                        location,
                        format!("scoped designation branches produce live {left} and live {right}"),
                    ));
                    None
                }
            }
            _ => {
                self.errors.push(diag(
                    location,
                    "scoped designation source must preserve one live child identity",
                ));
                None
            }
        }
    }

    fn sequence_model_for_path(&mut self, path: &str, location: SourceLocation) -> Option<String> {
        let Some((root_name, member_name)) = path.split_once('.') else {
            self.errors.push(diag(
                location,
                "scoped indexed designation currently requires an owner-relative [live T] source",
            ));
            return None;
        };
        let Some(root) = self.roots.get(root_name) else {
            self.errors.push(diag(
                location,
                format!("'{root_name}' is not a modeled-state owner root"),
            ));
            return None;
        };
        let Some(template) = self.templates.get(&root.model_name) else {
            self.errors.push(diag(
                location,
                format!("missing runtime model template for '{}'", root.model_name),
            ));
            return None;
        };
        let Some(member) = template.member(member_name) else {
            self.errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{member_name}'",
                    root.model_name
                ),
            ));
            return None;
        };
        let ValueType::SequenceLive(model) = &member.value_type else {
            self.errors.push(diag(
                location,
                format!("'{path}' is not a [live T] structural value"),
            ));
            return None;
        };
        Some(model.clone())
    }
}

struct ScopeCarrier<'a> {
    location: SourceLocation,
    binding: &'a str,
    designation: &'a Expr,
    body: &'a [Statement],
}

fn decode_scope_carrier(statement: &Statement) -> Option<ScopeCarrier<'_>> {
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
    if name != SCOPE_MARKER_ACTION {
        return None;
    }
    let [ActionArgument::Value(Expr::String(binding)), ActionArgument::Value(designation)] =
        arguments.as_slice()
    else {
        return None;
    };
    Some(ScopeCarrier {
        location: *location,
        binding,
        designation,
        body,
    })
}

struct RewriteContext<'a> {
    binding: &'a str,
    identity_param: &'a str,
    model: &'a str,
    template: &'a RuntimeModelTemplate,
    member_params: &'a HashMap<String, String>,
}

fn rewrite_statement(
    statement: &Statement,
    context: &RewriteContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            if target == context.binding {
                errors.push(diag(
                    *location,
                    format!("scoped live designation '{}' is immutable", context.binding),
                ));
            }
            Statement::Assignment {
                location: *location,
                target: rewrite_target(target, context),
                operator: *operator,
                value: rewrite_expr(value, context, errors),
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
            source: rewrite_plain_path(source, context),
            index: rewrite_expr(index, context, errors),
            member: member.clone(),
            operator: *operator,
            value: rewrite_expr(value, context, errors),
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
                .map(|argument| rewrite_argument(argument, context, errors))
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: rewrite_expr(message, context, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: rewrite_expr(condition, context, errors),
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
    context: &RewriteContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> ActionArgument {
    match argument {
        ActionArgument::Value(Expr::String(value)) => {
            if let Some(name) = value
                .strip_prefix(crate::structural_edit_surface::REMOVE_DESIGNATION_SELECTOR_PREFIX)
            {
                if name == context.binding {
                    return ActionArgument::Value(Expr::String(format!(
                        "{}{}",
                        crate::structural_edit_surface::REMOVE_DESIGNATION_SELECTOR_PREFIX,
                        context.identity_param
                    )));
                }
            }
            ActionArgument::Value(Expr::String(value.clone()))
        }
        ActionArgument::Value(expression) => {
            ActionArgument::Value(rewrite_expr(expression, context, errors))
        }
        ActionArgument::StateGrant { location, name } => {
            if let Some(path) = decode_through_path(name) {
                if let Some(member) = scoped_member(path, context.binding) {
                    let Some(member_template) = context.template.member(member) else {
                        errors.push(diag(
                            *location,
                            format!("state model '{}' has no member '{member}'", context.model),
                        ));
                        return argument.clone();
                    };
                    if member_template.kind != RuntimeModelMemberKind::State {
                        errors.push(diag(
                            *location,
                            format!(
                                "cannot grant derived member '{}.{member}' as writable state",
                                context.binding
                            ),
                        ));
                        return argument.clone();
                    }
                    return ActionArgument::StateGrant {
                        location: *location,
                        name: context.member_params[member].clone(),
                    };
                }
                if path == context.binding {
                    errors.push(diag(
                        *location,
                        "scoped whole-model writable authority is not part of this experiment",
                    ));
                }
            }
            ActionArgument::StateGrant {
                location: *location,
                name: rewrite_plain_path(name, context),
            }
        }
        ActionArgument::IndexedStateGrant {
            location,
            source,
            index,
            member,
        } => ActionArgument::IndexedStateGrant {
            location: *location,
            source: rewrite_plain_path(source, context),
            index: rewrite_expr(index, context, errors),
            member: member.clone(),
        },
    }
}

fn rewrite_expr(
    expression: &Expr,
    context: &RewriteContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => Expr::Name(rewrite_plain_path(name, context)),
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_expr(left, context, errors)),
            right: Box::new(rewrite_expr(right, context, errors)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_expr(condition, context, errors)),
            then_branch: Box::new(rewrite_expr(then_branch, context, errors)),
            else_branch: Box::new(rewrite_expr(else_branch, context, errors)),
        },
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => {
            if element == context.binding {
                errors.push(diag(
                    SourceLocation::new(1, 1),
                    format!(
                        "filter binding cannot shadow scoped designation '{}'",
                        context.binding
                    ),
                ));
            }
            Expr::Filter {
                source: Box::new(rewrite_expr(source, context, errors)),
                element: element.clone(),
                predicate: Box::new(rewrite_expr(predicate, context, errors)),
                order_by: order_by
                    .as_ref()
                    .map(|order_by| Box::new(rewrite_expr(order_by, context, errors))),
                order_descending: *order_descending,
                element_model: element_model.clone(),
            }
        }
        Expr::IndexedMember {
            source,
            index,
            member,
        } => Expr::IndexedMember {
            source: rewrite_plain_path(source, context),
            index: Box::new(rewrite_expr(index, context, errors)),
            member: member.clone(),
        },
        Expr::IndexedDesignation { source, index } => Expr::IndexedDesignation {
            source: rewrite_plain_path(source, context),
            index: Box::new(rewrite_expr(index, context, errors)),
        },
        Expr::RuntimeIndexMember {
            source,
            index,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeIndexMember {
            source: Box::new(rewrite_expr(source, context, errors)),
            index: Box::new(rewrite_expr(index, context, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => Expr::RuntimeIndexDesignation {
            source: Box::new(rewrite_expr(source, context, errors)),
            index: Box::new(rewrite_expr(index, context, errors)),
            element_model: element_model.clone(),
        },
        Expr::RuntimeDesignationMember {
            designation,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeDesignationMember {
            designation: Box::new(rewrite_expr(designation, context, errors)),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
    }
}

fn rewrite_target(target: &str, context: &RewriteContext<'_>) -> String {
    if let Some(path) = decode_through_path(target) {
        return encode_through_path(&rewrite_plain_path(path, context));
    }
    rewrite_plain_path(target, context)
}

fn rewrite_plain_path(path: &str, context: &RewriteContext<'_>) -> String {
    if path == context.binding {
        return context.identity_param.to_string();
    }
    let prefix = format!("{}.", context.binding);
    if let Some(rest) = path.strip_prefix(&prefix) {
        return format!("{}.{}", context.identity_param, rest);
    }
    path.to_string()
}

fn indexed_source_path(path: &str) -> Option<&str> {
    let (source, index_segment) = path.rsplit_once('.')?;
    crate::sequence_surface::decode_sequence_index_segment(index_segment)?;
    Some(source)
}

fn scoped_member<'a>(path: &'a str, binding: &str) -> Option<&'a str> {
    let prefix = format!("{binding}.");
    let member = path.strip_prefix(&prefix)?;
    (!member.is_empty() && !member.contains('.')).then_some(member)
}

fn collect_designation_models(program: &Program) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::State(state) = declaration {
            let Some(type_name) = state.type_name.as_deref() else {
                continue;
            };
            if let Some(model) =
                decode_live_type_name(type_name).or_else(|| decode_maybe_live_type_name(type_name))
            {
                result.insert(state.name.clone(), model.to_string());
            }
        }
        if let Declaration::Action(action) = declaration {
            for parameter in &action.parameters {
                if let Some(model) = decode_scope_identity_param(&parameter.name) {
                    result.insert(parameter.name.clone(), model.to_string());
                }
            }
        }
    }
    result
}

fn malformed_scope(location: SourceLocation) -> Statement {
    Statement::Fail {
        location,
        message: Expr::String("invalid scoped live designation".to_string()),
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

fn parse_with_head(source: &str, start: usize) -> Option<(&str, String, usize)> {
    let mut cursor = skip_inline_whitespace(source, start + "with".len());
    let expression_start = cursor;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;

    while cursor < source.len() {
        if source.as_bytes()[cursor] == b'"' {
            cursor = scan_string(source, cursor);
            continue;
        }
        match source.as_bytes()[cursor] {
            b'(' => paren_depth += 1,
            b')' => paren_depth = paren_depth.saturating_sub(1),
            b'[' => bracket_depth += 1,
            b']' => bracket_depth = bracket_depth.saturating_sub(1),
            _ => {}
        }
        if paren_depth == 0 && bracket_depth == 0 && keyword_at(source, cursor, "as") {
            let expression = source[expression_start..cursor].trim();
            if expression.is_empty() {
                return None;
            }
            cursor = skip_inline_whitespace(source, cursor + "as".len());
            let (binding, next) = parse_identifier(source, cursor)?;
            cursor = skip_inline_whitespace(source, next);
            if source.as_bytes().get(cursor) != Some(&b'{') {
                return None;
            }
            return Some((expression, binding, cursor + 1));
        }
        let ch = source[cursor..].chars().next()?;
        cursor += ch.len_utf8();
    }
    None
}

fn parse_identifier(source: &str, start: usize) -> Option<(String, usize)> {
    let mut chars = source[start..].char_indices();
    let (_, first) = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (offset, ch) in chars {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            end = start + offset + ch.len_utf8();
        } else {
            break;
        }
    }
    Some((source[start..end].to_string(), end))
}

fn skip_inline_whitespace(source: &str, mut index: usize) -> usize {
    while let Some(byte) = source.as_bytes().get(index) {
        if matches!(byte, b' ' | b'\t' | b'\r' | b'\n') {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn keyword_at(source: &str, index: usize, keyword: &str) -> bool {
    if !source[index..].starts_with(keyword) {
        return false;
    }
    let before_ok = index == 0
        || !source[..index]
            .chars()
            .next_back()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    let after = index + keyword.len();
    let after_ok = after >= source.len()
        || !source[after..]
            .chars()
            .next()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric());
    before_ok && after_ok
}

fn scan_string(source: &str, start: usize) -> usize {
    let mut index = start + 1;
    let mut escaped = false;
    while index < source.len() {
        let byte = source.as_bytes()[index];
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return index + 1;
        }
        index += 1;
    }
    source.len()
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
