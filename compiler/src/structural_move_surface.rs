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
use crate::structural_edit_surface::REMOVE_DESIGNATION_SELECTOR_PREFIX;

pub const MOVE_RELATIVE_MARKER_ACTION: &str = "__meld_surface_move_relative_marker";
pub const GENERATED_MOVE_ACTION_PREFIX: &str = "__meld_move_relative_";
pub const GENERATED_FILTERED_MOVE_ACTION_PREFIX: &str = "__meld_move_filtered_relative_";

pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    for reserved in [
        MOVE_RELATIVE_MARKER_ACTION,
        GENERATED_MOVE_ACTION_PREFIX,
        GENERATED_FILTERED_MOVE_ACTION_PREFIX,
    ] {
        if source.contains(reserved) {
            return Err(vec![Diagnostic::new(
                "reserved compiler structural-move marker cannot appear in source",
                1,
                1,
            )]);
        }
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0usize;

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

        if keyword_at(source, index, "move") {
            if let Some((moving, anchor, placement, target, end)) = parse_move(source, index) {
                output.push_str(MOVE_RELATIVE_MARKER_ACTION);
                output.push_str("(\"");
                output.push_str(&target);
                output.push_str("\", \"");
                output.push_str(REMOVE_DESIGNATION_SELECTOR_PREFIX);
                output.push_str(&moving);
                output.push_str("\", \"");
                output.push_str(REMOVE_DESIGNATION_SELECTOR_PREFIX);
                output.push_str(&anchor);
                output.push_str("\", \"");
                output.push_str(placement);
                output.push_str("\")");
                index = end;
                continue;
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
    let designation_models = collect_designation_models(program);
    let mut lowerer = MoveLowerer {
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

fn collect_designation_models(program: &Program) -> HashMap<String, String> {
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

struct MoveLowerer<'a> {
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    roots: &'a HashMap<String, RuntimeModelRoot>,
    designation_models: &'a HashMap<String, String>,
    next_id: usize,
    generated_actions: Vec<Declaration>,
    errors: Vec<Diagnostic>,
}

impl MoveLowerer<'_> {
    fn lower_statements(&mut self, statements: &[Statement]) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| match statement {
                Statement::ActionCall {
                    location,
                    name,
                    arguments,
                } if name == MOVE_RELATIVE_MARKER_ACTION => self.lower_move(*location, arguments),
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

    fn lower_move(&mut self, location: SourceLocation, arguments: &[ActionArgument]) -> Statement {
        let [ActionArgument::Value(Expr::String(target)), ActionArgument::Value(Expr::String(moving)), ActionArgument::Value(Expr::String(anchor)), ActionArgument::Value(Expr::String(placement))] =
            arguments
        else {
            self.errors.push(diag(
                location,
                "internal structural move marker is malformed",
            ));
            return malformed_move(location);
        };

        if placement != "before" && placement != "after" {
            self.errors.push(diag(
                location,
                "structural move placement must be 'before' or 'after'",
            ));
            return malformed_move(location);
        }

        let Some(moving_name) = moving.strip_prefix(REMOVE_DESIGNATION_SELECTOR_PREFIX) else {
            self.errors.push(diag(
                location,
                "structural move source must be a live designation",
            ));
            return malformed_move(location);
        };
        let Some(anchor_name) = anchor.strip_prefix(REMOVE_DESIGNATION_SELECTOR_PREFIX) else {
            self.errors.push(diag(
                location,
                "structural move anchor must be a live designation",
            ));
            return malformed_move(location);
        };
        let Some(moving_model) = self.designation_models.get(moving_name) else {
            self.errors.push(diag(
                location,
                format!("'{moving_name}' is not a persistent or scoped live designation"),
            ));
            return malformed_move(location);
        };
        let Some(anchor_model) = self.designation_models.get(anchor_name) else {
            self.errors.push(diag(
                location,
                format!("'{anchor_name}' is not a persistent or scoped live designation"),
            ));
            return malformed_move(location);
        };

        let Some((root_name, member_name)) = split_owner_member(target) else {
            self.errors.push(diag(
                location,
                format!(
                    "structural move target '{target}' must name one owner-relative sequence member"
                ),
            ));
            return malformed_move(location);
        };
        let (owner_model, designation_owned) = if let Some(root) = self.roots.get(root_name) {
            (root.model_name.clone(), false)
        } else if let Some(model) = self.designation_models.get(root_name) {
            (model.clone(), true)
        } else {
            self.errors.push(diag(
                location,
                format!(
                    "'{root_name}' is not a modeled-state owner root or persistent live designation"
                ),
            ));
            return malformed_move(location);
        };
        let Some(owner_template) = self.templates.get(&owner_model) else {
            self.errors.push(diag(
                location,
                format!("unknown state model '{owner_model}'"),
            ));
            return malformed_move(location);
        };
        let Some(member) = owner_template.member(member_name) else {
            self.errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    owner_template.name, member_name
                ),
            ));
            return malformed_move(location);
        };
        let ValueType::SequenceLive(element_model) = &member.value_type else {
            self.errors.push(diag(
                location,
                format!("structural move target '{target}' must have type [live T]"),
            ));
            return malformed_move(location);
        };

        if moving_model != element_model {
            self.errors.push(diag(
                location,
                format!(
                    "moving live designation '{moving_name}' has type live {moving_model} but structural move target contains live {element_model}"
                ),
            ));
            return malformed_move(location);
        }
        if anchor_model != element_model {
            self.errors.push(diag(
                location,
                format!(
                    "anchor live designation '{anchor_name}' has type live {anchor_model} but structural move target contains live {element_model}"
                ),
            ));
            return malformed_move(location);
        }

        if member.kind == RuntimeModelMemberKind::Derived {
            if designation_owned {
                self.errors.push(diag(
                    location,
                    "designation-owned filtered structural movement is not yet selected",
                ));
                return malformed_move(location);
            }
            let Expr::Filter {
                source, order_by, ..
            } = &member.expression
            else {
                self.errors.push(diag(
                    location,
                    format!("structural move target '{target}' must be mutable sequence state or a direct filtered view of mutable sequence state"),
                ));
                return malformed_move(location);
            };
            if order_by.is_some() {
                self.errors.push(diag(
                    location,
                    format!(
                        "ordered derived view '{target}' is read-only and cannot be a structural move target"
                    ),
                ));
                return malformed_move(location);
            }
            let Expr::Name(source_member_name) = source.as_ref() else {
                self.errors.push(diag(
                    location,
                    format!("filtered structural move target '{target}' must have one named mutable sequence source"),
                ));
                return malformed_move(location);
            };
            let Some(source_member) = owner_template.member(source_member_name) else {
                self.errors.push(diag(
                    location,
                    format!(
                        "state model '{}' has no filter source member '{}'",
                        owner_template.name, source_member_name
                    ),
                ));
                return malformed_move(location);
            };
            if source_member.kind != RuntimeModelMemberKind::State
                || source_member.value_type != member.value_type
            {
                self.errors.push(diag(
                    location,
                    format!("filtered structural move target '{target}' must derive directly from mutable [live {element_model}] state"),
                ));
                return malformed_move(location);
            }

            let helper_name = format!("{GENERATED_FILTERED_MOVE_ACTION_PREFIX}{}", self.next_id);
            self.next_id += 1;
            self.generated_actions.push(Declaration::Action(ActionDecl {
                location,
                name: helper_name.clone(),
                parameters: vec![
                    state_parameter(location, "target", element_model),
                    value_parameter(
                        location,
                        "view",
                        &encode_runtime_sequence_type(element_model),
                    ),
                    value_parameter(location, "moving", "String"),
                    value_parameter(location, "anchor", "String"),
                    value_parameter(location, "placement", "String"),
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
                    ActionArgument::Value(Expr::Name(moving_name.to_string())),
                    ActionArgument::Value(Expr::Name(anchor_name.to_string())),
                    ActionArgument::Value(Expr::String(placement.clone())),
                ],
            };
        }

        if member.kind != RuntimeModelMemberKind::State {
            self.errors.push(diag(
                location,
                format!("structural move target '{target}' must be mutable sequence state"),
            ));
            return malformed_move(location);
        }

        let helper_name = format!("{GENERATED_MOVE_ACTION_PREFIX}{}", self.next_id);
        self.next_id += 1;
        self.generated_actions.push(Declaration::Action(ActionDecl {
            location,
            name: helper_name.clone(),
            parameters: vec![
                state_parameter(location, "target", element_model),
                value_parameter(location, "moving", "String"),
                value_parameter(location, "anchor", "String"),
                value_parameter(location, "placement", "String"),
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
                ActionArgument::Value(Expr::Name(moving_name.to_string())),
                ActionArgument::Value(Expr::Name(anchor_name.to_string())),
                ActionArgument::Value(Expr::String(placement.clone())),
            ],
        }
    }
}

fn state_parameter(location: SourceLocation, name: &str, model: &str) -> ActionParameter {
    ActionParameter {
        location,
        kind: ActionParameterKind::State,
        name: name.to_string(),
        type_name: encode_runtime_sequence_type(model),
    }
}

fn value_parameter(location: SourceLocation, name: &str, type_name: &str) -> ActionParameter {
    ActionParameter {
        location,
        kind: ActionParameterKind::Value,
        name: name.to_string(),
        type_name: type_name.to_string(),
    }
}

fn malformed_move(location: SourceLocation) -> Statement {
    Statement::Fail {
        location,
        message: Expr::String("invalid structural move".to_string()),
    }
}

fn split_owner_member(path: &str) -> Option<(&str, &str)> {
    let (owner, member) = path.split_once('.')?;
    (!owner.is_empty() && !member.is_empty() && !member.contains('.')).then_some((owner, member))
}

fn parse_move(source: &str, start: usize) -> Option<(String, String, &'static str, String, usize)> {
    let mut cursor = start + "move".len();
    cursor = skip_inline_whitespace(source, cursor);
    let (moving, next) = parse_name_path(source, cursor)?;
    cursor = skip_inline_whitespace(source, next);

    let placement = if keyword_at(source, cursor, "before") {
        cursor += "before".len();
        "before"
    } else if keyword_at(source, cursor, "after") {
        cursor += "after".len();
        "after"
    } else {
        return None;
    };

    cursor = skip_inline_whitespace(source, cursor);
    let (anchor, next) = parse_name_path(source, cursor)?;
    cursor = skip_inline_whitespace(source, next);
    if !keyword_at(source, cursor, "in") {
        return None;
    }
    cursor += "in".len();
    cursor = skip_inline_whitespace(source, cursor);
    let (target, end) = parse_name_path(source, cursor)?;
    Some((moving, anchor, placement, target, end))
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
        if matches!(*byte, b' ' | b'\t' | b'\r') {
            index += 1;
        } else {
            break;
        }
    }
    index
}

fn scan_string(source: &str, start: usize) -> usize {
    let mut index = start + 1;
    while index < source.len() {
        match source.as_bytes()[index] {
            b'\\' => index = (index + 2).min(source.len()),
            b'"' => return index + 1,
            _ => index += 1,
        }
    }
    source.len()
}

fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_identifier_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocesses_move_without_touching_strings_or_comments() {
        let source = r#"
action reorder {
    // move selected after anchor in workstation.tasks
    message = "move selected before anchor in workstation.tasks"
    move selected before anchor in workstation.tasks
}
"#;
        let lowered = preprocess(source).unwrap();
        assert!(lowered.contains(MOVE_RELATIVE_MARKER_ACTION));
        assert!(lowered.contains("// move selected after anchor in workstation.tasks"));
        assert!(lowered.contains("\"move selected before anchor in workstation.tasks\""));
    }
}
