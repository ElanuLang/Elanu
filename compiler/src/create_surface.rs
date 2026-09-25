use std::collections::HashMap;

use crate::ast::{ActionArgument, Declaration, Expr, Program, Statement};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::{RuntimeModelRoot, RuntimeModelTemplate};

pub const CREATE_BUILTIN_ACTION: &str = "__elanu_surface_create_builtin";

/// Bootstrap preprocessor for the selected owner-relative creation statement:
///
/// `create LineItem in invoice`
///
/// The surface is rewritten to one compiler-private action call whose empty
/// declaration is appended only when creation is present. Later validation
/// checks the requested model and owner before ordinary lowering/checking.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    if source.contains(CREATE_BUILTIN_ACTION) {
        return Err(vec![Diagnostic::new(
            "reserved compiler create marker cannot appear in source",
            1,
            1,
        )]);
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    let mut saw_create = false;

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

        if keyword_at(source, index, "create") {
            if let Some((end, model, owner)) = parse_create_statement(source, index) {
                output.push_str(CREATE_BUILTIN_ACTION);
                output.push('(');
                output.push('"');
                output.push_str(&model);
                output.push_str("\", \"");
                output.push_str(&owner);
                output.push_str("\")");
                index = end;
                saw_create = true;
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

    if saw_create {
        output.push_str("\naction ");
        output.push_str(CREATE_BUILTIN_ACTION);
        output.push_str("(modelName: String, ownerName: String) {}\n");
    }

    Ok(output)
}

pub fn validate(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<(), Vec<Diagnostic>> {
    let mut errors = Vec::new();

    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        validate_statements(&action.statements, templates, roots, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_statements(
    statements: &[Statement],
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::ActionCall {
                location,
                name,
                arguments,
            } if name == CREATE_BUILTIN_ACTION => {
                let [ActionArgument::Value(Expr::String(model)), ActionArgument::Value(Expr::String(owner))] =
                    arguments.as_slice()
                else {
                    errors.push(Diagnostic::new(
                        "internal create marker is malformed",
                        location.line,
                        location.column,
                    ));
                    continue;
                };

                if !templates.contains_key(model) {
                    errors.push(Diagnostic::new(
                        format!("unknown state model '{model}' in create statement"),
                        location.line,
                        location.column,
                    ));
                }

                if !roots.contains_key(owner) {
                    errors.push(Diagnostic::new(
                        format!("'{owner}' is not a modeled-state owner root"),
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
                validate_statements(then_branch, templates, roots, errors);
                if let Some(else_branch) = else_branch {
                    validate_statements(else_branch, templates, roots, errors);
                }
            }
            _ => {}
        }
    }
}

fn parse_create_statement(source: &str, start: usize) -> Option<(usize, String, String)> {
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
    index = skip_inline_whitespace(source, next);

    match source.as_bytes().get(index).copied() {
        None | Some(b'\n' | b'\r' | b';' | b'}') => Some((index, model, owner)),
        Some(b'/') if source[index..].starts_with("//") => Some((index, model, owner)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocesses_create_statement_and_injects_private_builtin() {
        let source = "action add {\n    create LineItem in invoice\n}\n";
        let output = preprocess(source).unwrap();
        assert!(output.contains("__elanu_surface_create_builtin(\"LineItem\", \"invoice\")"));
        assert!(output.contains(
            "action __elanu_surface_create_builtin(modelName: String, ownerName: String) {}"
        ));
    }

    #[test]
    fn create_inside_string_or_comment_is_not_rewritten() {
        let source =
            "// create LineItem in invoice\nderived text = \"create LineItem in invoice\"\n";
        assert_eq!(preprocess(source).unwrap(), source);
    }

    #[test]
    fn check_rejects_unknown_create_model() {
        let source = r#"
state model Invoice {
    state count = 0
}
state invoice: Invoice

action add {
    create Missing in invoice
}
"#;
        let errors = crate::check_source(source).expect_err("unknown create model should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("unknown state model 'Missing'")));
    }

    #[test]
    fn check_rejects_non_owner_create_target() {
        let source = r#"
state model LineItem {
    state quantity = 1
}
state model Invoice {
    state count = 0
}
state invoice: Invoice
state ordinary = 0

action add {
    create LineItem in ordinary
}
"#;
        let errors = crate::check_source(source).expect_err("non-model owner should fail");
        assert!(errors.iter().any(|error| error
            .message
            .contains("'ordinary' is not a modeled-state owner root")));
    }
}
