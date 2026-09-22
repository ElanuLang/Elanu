use std::collections::HashMap;

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, ActionArgument, Declaration, Expr, Program,
    Statement,
};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::RuntimeModelRoot;

pub const DESTROY_BUILTIN_ACTION: &str = "__meld_surface_destroy_child_builtin";

/// Preprocess the intentionally narrow rooted-child lifetime surface:
///
/// `destroy selected in workspace`
///
/// The source designation remains an ordinary persistent designation name until
/// live-designation lowering converts it to its private identity carrier. The
/// owner is encoded as a String because the operation names/proves root
/// provenance rather than reading an ordinary owner value.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    if source.contains(DESTROY_BUILTIN_ACTION) {
        return Err(vec![Diagnostic::new(
            "reserved compiler child-destroy marker cannot appear in source",
            1,
            1,
        )]);
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    let mut saw_destroy = false;

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

        if keyword_at(source, index, "destroy") {
            if let Some((end, designation, owner)) = parse_destroy_statement(source, index) {
                output.push_str(DESTROY_BUILTIN_ACTION);
                output.push('(');
                output.push_str(&designation);
                output.push_str(", ");
                output.push_str(&owner);
                output.push(')');
                index = end;
                saw_destroy = true;
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

    if saw_destroy {
        output.push_str("\naction ");
        output.push_str(DESTROY_BUILTIN_ACTION);
        output.push_str("(target: String, ownerName: String) {}\n");
    }

    Ok(output)
}

pub fn validate(
    program: &Program,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<(), Vec<Diagnostic>> {
    let mut designation_kinds = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::State(state) = declaration else {
            continue;
        };
        let Some(type_name) = state.type_name.as_deref() else {
            continue;
        };
        if let Some(model) = decode_maybe_live_type_name(type_name) {
            designation_kinds.insert(state.name.clone(), (model.to_string(), true));
        } else if let Some(model) = decode_live_type_name(type_name) {
            designation_kinds.insert(state.name.clone(), (model.to_string(), false));
        }
    }

    let mut errors = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        validate_statements(&action.statements, roots, &designation_kinds, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn lower_owners(program: &Program, roots: &HashMap<String, RuntimeModelRoot>) -> Program {
    fn lower_statements(
        statements: &[Statement],
        roots: &HashMap<String, RuntimeModelRoot>,
    ) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| match statement {
                Statement::ActionCall {
                    location,
                    name,
                    arguments,
                } if name == DESTROY_BUILTIN_ACTION => {
                    let mut arguments = arguments.clone();
                    if let Some(ActionArgument::Value(Expr::Name(owner))) = arguments.get(1) {
                        if roots.contains_key(owner) {
                            arguments[1] = ActionArgument::Value(Expr::String(owner.to_string()));
                        }
                    }
                    Statement::ActionCall {
                        location: *location,
                        name: name.clone(),
                        arguments,
                    }
                }
                Statement::If {
                    location,
                    condition,
                    then_branch,
                    else_branch,
                } => Statement::If {
                    location: *location,
                    condition: condition.clone(),
                    then_branch: lower_statements(then_branch, roots),
                    else_branch: else_branch
                        .as_ref()
                        .map(|branch| lower_statements(branch, roots)),
                },
                other => other.clone(),
            })
            .collect()
    }

    Program {
        declarations: program
            .declarations
            .iter()
            .map(|declaration| match declaration {
                Declaration::Action(action) => Declaration::Action(crate::ast::ActionDecl {
                    location: action.location,
                    name: action.name.clone(),
                    parameters: action.parameters.clone(),
                    statements: lower_statements(&action.statements, roots),
                }),
                other => other.clone(),
            })
            .collect(),
        state_models: program.state_models.clone(),
    }
}

fn validate_statements(
    statements: &[Statement],
    roots: &HashMap<String, RuntimeModelRoot>,
    designation_kinds: &HashMap<String, (String, bool)>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::ActionCall {
                location,
                name,
                arguments,
            } if name == DESTROY_BUILTIN_ACTION => {
                let [ActionArgument::Value(Expr::Name(designation)), ActionArgument::Value(Expr::Name(owner))] =
                    arguments.as_slice()
                else {
                    errors.push(Diagnostic::new(
                        "internal child-destroy marker is malformed",
                        location.line,
                        location.column,
                    ));
                    continue;
                };

                match designation_kinds.get(designation) {
                    Some((_, true)) => {}
                    Some((model, false)) => errors.push(Diagnostic::new(
                        format!(
                            "destroy requires persistent maybe live {model}; '{designation}' is plain live {model} and cannot become absent"
                        ),
                        location.line,
                        location.column,
                    )),
                    None => errors.push(Diagnostic::new(
                        format!(
                            "destroy requires a persistent maybe live designation state; '{designation}' is not one"
                        ),
                        location.line,
                        location.column,
                    )),
                }

                if !roots.contains_key(owner) && !designation_kinds.contains_key(owner) {
                    errors.push(Diagnostic::new(
                        format!(
                            "destroy owner '{owner}' is not a modeled-state root or persistent live designation"
                        ),
                        location.line,
                        location.column,
                    ));
                }
            }
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_statements(then_branch, roots, designation_kinds, errors);
                if let Some(else_branch) = else_branch {
                    validate_statements(else_branch, roots, designation_kinds, errors);
                }
            }
            _ => {}
        }
    }
}

fn parse_destroy_statement(source: &str, start: usize) -> Option<(usize, String, String)> {
    let mut index = start + "destroy".len();
    index = skip_inline_whitespace(source, index);

    let (designation, next) = parse_identifier(source, index)?;
    index = skip_inline_whitespace(source, next);
    if !keyword_at(source, index, "in") {
        return None;
    }
    index += "in".len();
    index = skip_inline_whitespace(source, index);

    let (owner, next) = parse_name_path(source, index)?;
    index = skip_inline_whitespace(source, next);

    match source.as_bytes().get(index).copied() {
        None | Some(b'\n' | b'\r' | b';' | b'}') => Some((index, designation, owner)),
        Some(b'/') if source[index..].starts_with("//") => Some((index, designation, owner)),
        _ => None,
    }
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
