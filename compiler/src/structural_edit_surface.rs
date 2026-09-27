use std::collections::HashMap;

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, ActionArgument, ActionDecl,
    ActionParameter, ActionParameterKind, Declaration, Expr, Program, SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::{
    RuntimeModelMemberKind, RuntimeModelRoot, RuntimeModelTemplate,
};
use crate::runtime_sequence_markers::encode_runtime_sequence_type;
use crate::scoped_designation_surface::decode_scope_identity_param;
use crate::semantic::ValueType;

pub const REMOVE_OCCURRENCE_MARKER_ACTION: &str = "__elanu_surface_remove_occurrence_marker";
pub(crate) const REMOVE_DESIGNATION_SELECTOR_PREFIX: &str = "__elanu_remove_designation_selector$";
pub const GENERATED_REMOVE_ACTION_PREFIX: &str = "__elanu_remove_occurrence_";
pub const GENERATED_FILTERED_REMOVE_ACTION_PREFIX: &str = "__elanu_remove_filtered_occurrence_";

pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    for reserved in [
        REMOVE_OCCURRENCE_MARKER_ACTION,
        REMOVE_DESIGNATION_SELECTOR_PREFIX,
        GENERATED_REMOVE_ACTION_PREFIX,
        GENERATED_FILTERED_REMOVE_ACTION_PREFIX,
    ] {
        if source.contains(reserved) {
            return Err(vec![Diagnostic::new(
                "reserved compiler structural-edit marker cannot appear in source",
                1,
                1,
            )]);
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;

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

        if keyword_at(source, index, "remove") {
            let mut cursor = index + "remove".len();
            cursor = skip_inline_whitespace(source, cursor);
            if let Some((first, next)) = parse_name_path(source, cursor) {
                cursor = skip_inline_whitespace(source, next);
                if source.as_bytes().get(cursor) == Some(&b'[') {
                    if let Some((index_expression, end)) = parse_index_expression(source, cursor) {
                        output.push_str(REMOVE_OCCURRENCE_MARKER_ACTION);
                        output.push_str("(\"");
                        output.push_str(&first);
                        output.push_str("\", ");
                        output.push_str(index_expression);
                        output.push(')');
                        index = end;
                        continue;
                    }
                }

                if keyword_at(source, cursor, "from") {
                    cursor += "from".len();
                    cursor = skip_inline_whitespace(source, cursor);
                    if let Some((target, end)) = parse_name_path(source, cursor) {
                        output.push_str(REMOVE_OCCURRENCE_MARKER_ACTION);
                        output.push_str("(\"");
                        output.push_str(&target);
                        output.push_str("\", \"");
                        output.push_str(REMOVE_DESIGNATION_SELECTOR_PREFIX);
                        output.push_str(&first);
                        output.push_str("\")");
                        index = end;
                        continue;
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

    Ok(output)
}

pub fn lower(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<Program, Vec<Diagnostic>> {
    let designation_models = collect_persistent_designations(program);
    let mut lowerer = RemoveLowerer {
        templates,
        roots,
        designation_models: &designation_models,
        next_id: 0,
        generated_actions: Vec::new(),
        errors: Vec::new(),
    };

    let declarations = program
        .declarations
        .iter()
        .map(|declaration| match declaration {
            Declaration::Action(action) => Declaration::Action(ActionDecl {
                location: action.location,
                name: action.name.clone(),
                parameters: action.parameters.clone(),
                statements: lowerer.lower_statements(&action.statements),
            }),
            other => other.clone(),
        })
        .collect::<Vec<_>>();

    if !lowerer.errors.is_empty() {
        return Err(lowerer.errors);
    }

    let mut declarations = declarations;
    declarations.extend(lowerer.generated_actions);
    Ok(Program {
        declarations,
        state_models: program.state_models.clone(),
    })
}

fn collect_persistent_designations(program: &Program) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for declaration in &program.declarations {
        if let Declaration::State(state) = declaration {
            if let Some(type_name) = state.type_name.as_deref() {
                if let Some(model) = decode_live_type_name(type_name)
                    .or_else(|| decode_maybe_live_type_name(type_name))
                {
                    result.insert(state.name.clone(), model.to_string());
                }
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

#[derive(Debug, Clone)]
enum RemoveSelector {
    Position(Expr),
    Designation { name: String, model: String },
}

struct RemoveLowerer<'a> {
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    roots: &'a HashMap<String, RuntimeModelRoot>,
    designation_models: &'a HashMap<String, String>,
    next_id: usize,
    generated_actions: Vec<Declaration>,
    errors: Vec<Diagnostic>,
}

impl RemoveLowerer<'_> {
    fn lower_statements(&mut self, statements: &[Statement]) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| match statement {
                Statement::ActionCall {
                    location,
                    name,
                    arguments,
                } if name == REMOVE_OCCURRENCE_MARKER_ACTION => {
                    self.lower_remove(*location, arguments)
                }
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
            })
            .collect()
    }

    fn lower_remove(
        &mut self,
        location: SourceLocation,
        arguments: &[ActionArgument],
    ) -> Statement {
        let [ActionArgument::Value(Expr::String(target)), ActionArgument::Value(selector)] =
            arguments
        else {
            self.errors.push(Diagnostic::new(
                "internal structural removal marker is malformed",
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };

        let selector = match selector {
            Expr::String(encoded) => {
                if let Some(name) = encoded.strip_prefix(REMOVE_DESIGNATION_SELECTOR_PREFIX) {
                    let Some(model) = self.designation_models.get(name) else {
                        self.errors.push(Diagnostic::new(
                            format!("'{name}' is not a persistent live designation state"),
                            location.line,
                            location.column,
                        ));
                        return malformed_call(location);
                    };
                    RemoveSelector::Designation {
                        name: name.to_string(),
                        model: model.clone(),
                    }
                } else {
                    RemoveSelector::Position(Expr::String(encoded.clone()))
                }
            }
            other => RemoveSelector::Position(other.clone()),
        };

        let Some((root_name, member_name)) = split_owner_member(target) else {
            self.errors.push(Diagnostic::new(
                format!(
                    "structural removal target '{target}' must name one owner-relative sequence member"
                ),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };

        let owner_model = if let Some(root) = self.roots.get(root_name) {
            root.model_name.clone()
        } else if let Some(model) = self.designation_models.get(root_name) {
            model.clone()
        } else {
            self.errors.push(Diagnostic::new(
                format!(
                    "'{root_name}' is not a modeled-state owner root or persistent live designation"
                ),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        let Some(owner_template) = self.templates.get(&owner_model) else {
            self.errors.push(Diagnostic::new(
                format!("unknown state model '{owner_model}'"),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        let Some(member) = owner_template.member(member_name) else {
            self.errors.push(Diagnostic::new(
                format!(
                    "state model '{}' has no member '{}'",
                    owner_template.name, member_name
                ),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        if member.kind == RuntimeModelMemberKind::Derived {
            let Expr::Filter {
                source, order_by, ..
            } = &member.expression
            else {
                self.errors.push(Diagnostic::new(
                    format!("structural removal target '{target}' must be mutable sequence state or a direct filtered view of mutable sequence state"),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            };
            if order_by.is_some() {
                self.errors.push(Diagnostic::new(
                    format!(
                        "ordered derived view '{target}' is read-only and cannot be a structural removal target"
                    ),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            }
            let Expr::Name(source_member_name) = source.as_ref() else {
                self.errors.push(Diagnostic::new(
                    format!("filtered structural removal target '{target}' must have one named mutable sequence source"),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            };
            let Some(source_member) = owner_template.member(source_member_name) else {
                self.errors.push(Diagnostic::new(
                    format!(
                        "state model '{}' has no filter source member '{}'",
                        owner_template.name, source_member_name
                    ),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            };
            let ValueType::SequenceLive(element_model) = &member.value_type else {
                self.errors.push(Diagnostic::new(
                    format!(
                        "filtered structural removal target '{target}' must have type [live T]"
                    ),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            };
            let Some((selector_type, selector_argument)) =
                prepare_selector(&selector, element_model, location, &mut self.errors)
            else {
                return malformed_call(location);
            };
            if source_member.kind != RuntimeModelMemberKind::State
                || source_member.value_type != member.value_type
            {
                self.errors.push(Diagnostic::new(
                    format!("filtered structural removal target '{target}' must derive directly from mutable [live {element_model}] state"),
                    location.line,
                    location.column,
                ));
                return malformed_call(location);
            }

            let helper_name = format!("{GENERATED_FILTERED_REMOVE_ACTION_PREFIX}{}", self.next_id);
            self.next_id += 1;
            self.generated_actions.push(Declaration::Action(ActionDecl {
                location,
                name: helper_name.clone(),
                parameters: vec![
                    ActionParameter {
                        location,
                        kind: ActionParameterKind::State,
                        name: "target".to_string(),
                        type_name: encode_runtime_sequence_type(element_model),
                    },
                    ActionParameter {
                        location,
                        kind: ActionParameterKind::Value,
                        name: "view".to_string(),
                        type_name: encode_runtime_sequence_type(element_model),
                    },
                    ActionParameter {
                        location,
                        kind: ActionParameterKind::Value,
                        name: "index".to_string(),
                        type_name: selector_type,
                    },
                ],
                statements: Vec::new(),
            }));

            return Statement::ActionCall {
                location,
                name: helper_name,
                arguments: vec![
                    ActionArgument::StateGrant {
                        location,
                        name: format!("{root_name}.{source_member_name}"),
                    },
                    ActionArgument::Value(Expr::Name(target.clone())),
                    selector_argument,
                ],
            };
        }
        if member.kind != RuntimeModelMemberKind::State {
            self.errors.push(Diagnostic::new(
                format!("structural removal target '{target}' must be mutable sequence state"),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        }
        let ValueType::SequenceLive(element_model) = &member.value_type else {
            self.errors.push(Diagnostic::new(
                format!("structural removal target '{target}' must have type [live T]"),
                location.line,
                location.column,
            ));
            return malformed_call(location);
        };
        let Some((selector_type, selector_argument)) =
            prepare_selector(&selector, element_model, location, &mut self.errors)
        else {
            return malformed_call(location);
        };

        let helper_name = format!("{GENERATED_REMOVE_ACTION_PREFIX}{}", self.next_id);
        self.next_id += 1;
        self.generated_actions.push(Declaration::Action(ActionDecl {
            location,
            name: helper_name.clone(),
            parameters: vec![
                ActionParameter {
                    location,
                    kind: ActionParameterKind::State,
                    name: "target".to_string(),
                    type_name: encode_runtime_sequence_type(element_model),
                },
                ActionParameter {
                    location,
                    kind: ActionParameterKind::Value,
                    name: "index".to_string(),
                    type_name: selector_type,
                },
            ],
            statements: Vec::new(),
        }));

        Statement::ActionCall {
            location,
            name: helper_name,
            arguments: vec![
                ActionArgument::StateGrant {
                    location,
                    name: target.clone(),
                },
                selector_argument,
            ],
        }
    }
}

fn prepare_selector(
    selector: &RemoveSelector,
    element_model: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<(String, ActionArgument)> {
    match selector {
        RemoveSelector::Position(expression) => {
            Some(("Int".to_string(), ActionArgument::Value(expression.clone())))
        }
        RemoveSelector::Designation { name, model } => {
            if model != element_model {
                errors.push(Diagnostic::new(
                    format!(
                        "live designation '{name}' has type live {model} but structural removal target contains live {element_model}"
                    ),
                    location.line,
                    location.column,
                ));
                return None;
            }
            Some((
                "String".to_string(),
                ActionArgument::Value(Expr::Name(name.clone())),
            ))
        }
    }
}

fn malformed_call(location: SourceLocation) -> Statement {
    Statement::Fail {
        location,
        message: Expr::String("invalid structural removal".to_string()),
    }
}

fn split_owner_member(path: &str) -> Option<(&str, &str)> {
    let (owner, member) = path.split_once('.')?;
    (!owner.is_empty() && !member.is_empty() && !member.contains('.')).then_some((owner, member))
}

fn parse_index_expression(source: &str, start: usize) -> Option<(&str, usize)> {
    let mut index = start + 1;
    let expression_start = index;
    let mut bracket_depth = 1usize;

    while index < source.len() {
        if source.as_bytes()[index] == b'"' {
            index = scan_string(source, index);
            continue;
        }

        if source[index..].starts_with("//") {
            index = source[index..]
                .find('\n')
                .map(|offset| index + offset)
                .unwrap_or(source.len());
            continue;
        }

        match source.as_bytes()[index] {
            b'[' => bracket_depth += 1,
            b']' => {
                bracket_depth -= 1;
                if bracket_depth == 0 {
                    let expression = source[expression_start..index].trim();
                    return (!expression.is_empty()).then_some((expression, index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
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
    fn preprocesses_remove_occurrence_without_touching_comments_or_strings() {
        let source = r#"
action edit {
    remove invoice.lines[0]
    note("remove invoice.lines[1]")
    // remove invoice.lines[2]
}
"#;
        let output = preprocess(source).unwrap();
        assert!(output.contains("__elanu_surface_remove_occurrence_marker(\"invoice.lines\", 0)"));
        assert!(output.contains("\"remove invoice.lines[1]\""));
        assert!(output.contains("// remove invoice.lines[2]"));
    }

    #[test]
    fn preprocesses_remove_index_expression_without_flattening_it() {
        let source = r#"
action edit {
    remove invoice.lines[selectedLine + 1]
}
"#;
        let output = preprocess(source).unwrap();
        assert!(output.contains(
            "__elanu_surface_remove_occurrence_marker(\"invoice.lines\", selectedLine + 1)"
        ));
    }
}
