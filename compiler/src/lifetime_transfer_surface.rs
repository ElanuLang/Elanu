use std::collections::HashMap;

use crate::ast::{
    decode_live_type_name, decode_maybe_live_type_name, ActionArgument, Declaration, Expr, Program,
    Statement,
};
use crate::diagnostic::Diagnostic;
use crate::runtime_model_templates::RuntimeModelRoot;

pub const TRANSFER_BUILTIN_ACTION: &str = "__meld_surface_transfer_child_builtin";

/// Preprocess the intentionally narrow rooted-child lifetime transfer surface:
///
/// `transfer selected from left to right`
///
/// Target and dynamic owner operands remain ordinary persistent designation names until
/// live-designation lowering converts them to private identity carriers. Static modeled
/// owner roots are converted to String identity values by `lower_owners`.
pub fn preprocess(source: &str) -> Result<String, Vec<Diagnostic>> {
    if source.contains(TRANSFER_BUILTIN_ACTION) {
        return Err(vec![Diagnostic::new(
            "reserved compiler child-transfer marker cannot appear in source",
            1,
            1,
        )]);
    }

    let mut output = String::with_capacity(source.len());
    let mut index = 0;
    let mut saw_transfer = false;

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

        if keyword_at(source, index, "transfer") {
            if let Some((end, target, source_owner, destination_owner)) =
                parse_transfer_statement(source, index)
            {
                output.push_str(TRANSFER_BUILTIN_ACTION);
                output.push('(');
                output.push_str(&target);
                output.push_str(", ");
                output.push_str(&source_owner);
                output.push_str(", ");
                output.push_str(&destination_owner);
                output.push(')');
                index = end;
                saw_transfer = true;
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

    if saw_transfer {
        output.push_str("\naction ");
        output.push_str(TRANSFER_BUILTIN_ACTION);
        output.push_str("(target: String, sourceOwner: String, destinationOwner: String) {}\n");
    }

    Ok(output)
}

pub fn validate(
    program: &Program,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<(), Vec<Diagnostic>> {
    let mut designations = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::State(state) = declaration else {
            continue;
        };
        let Some(type_name) = state.type_name.as_deref() else {
            continue;
        };
        if let Some(model) = decode_maybe_live_type_name(type_name) {
            designations.insert(state.name.clone(), (model.to_string(), true));
        } else if let Some(model) = decode_live_type_name(type_name) {
            designations.insert(state.name.clone(), (model.to_string(), false));
        }
    }

    let mut errors = Vec::new();
    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        validate_statements(&action.statements, roots, &designations, &mut errors);
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
                } if name == TRANSFER_BUILTIN_ACTION => {
                    let mut arguments = arguments.clone();
                    for index in [1usize, 2usize] {
                        if let Some(ActionArgument::Value(Expr::Name(owner))) = arguments.get(index)
                        {
                            if roots.contains_key(owner) {
                                arguments[index] =
                                    ActionArgument::Value(Expr::String(owner.to_string()));
                            }
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
    designations: &HashMap<String, (String, bool)>,
    errors: &mut Vec<Diagnostic>,
) {
    for statement in statements {
        match statement {
            Statement::ActionCall {
                location,
                name,
                arguments,
            } if name == TRANSFER_BUILTIN_ACTION => {
                let [ActionArgument::Value(Expr::Name(target)), ActionArgument::Value(Expr::Name(source_owner)), ActionArgument::Value(Expr::Name(destination_owner))] =
                    arguments.as_slice()
                else {
                    errors.push(Diagnostic::new(
                        "internal child-transfer marker is malformed",
                        location.line,
                        location.column,
                    ));
                    continue;
                };

                if !designations.contains_key(target) {
                    errors.push(Diagnostic::new(
                        format!(
                            "transfer requires persistent live or maybe live designation state; '{target}' is not one"
                        ),
                        location.line,
                        location.column,
                    ));
                }

                for (role, owner) in [
                    ("source", source_owner.as_str()),
                    ("destination", destination_owner.as_str()),
                ] {
                    if !roots.contains_key(owner) && !designations.contains_key(owner) {
                        errors.push(Diagnostic::new(
                            format!(
                                "transfer {role} owner '{owner}' is not a modeled-state root or persistent live designation"
                            ),
                            location.line,
                            location.column,
                        ));
                    }
                }
            }
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                validate_statements(then_branch, roots, designations, errors);
                if let Some(else_branch) = else_branch {
                    validate_statements(else_branch, roots, designations, errors);
                }
            }
            _ => {}
        }
    }
}

fn parse_transfer_statement(
    source: &str,
    start: usize,
) -> Option<(usize, String, String, String)> {
    let mut index = start + "transfer".len();
    index = skip_inline_whitespace(source, index);

    let (target, next) = parse_identifier(source, index)?;
    index = skip_inline_whitespace(source, next);
    if !keyword_at(source, index, "from") {
        return None;
    }
    index += "from".len();
    index = skip_inline_whitespace(source, index);

    let (source_owner, next) = parse_name_path(source, index)?;
    index = skip_inline_whitespace(source, next);
    if !keyword_at(source, index, "to") {
        return None;
    }
    index += "to".len();
    index = skip_inline_whitespace(source, index);

    let (destination_owner, next) = parse_name_path(source, index)?;
    index = skip_inline_whitespace(source, next);

    match source.as_bytes().get(index).copied() {
        None | Some(b'\n' | b'\r' | b';' | b'}') => {
            Some((index, target, source_owner, destination_owner))
        }
        Some(b'/') if source[index..].starts_with("//") => {
            Some((index, target, source_owner, destination_owner))
        }
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
    use crate::runtime::{Runtime, Value};

    #[test]
    fn preprocesses_transfer_statement_and_injects_private_builtin() {
        let source = "action moveOwnership {\n    transfer selected from left to right\n}\n";
        let output = preprocess(source).unwrap();
        assert!(output.contains(
            "__meld_surface_transfer_child_builtin(selected, left, right)"
        ));
        assert!(output.contains(
            "action __meld_surface_transfer_child_builtin(target: String, sourceOwner: String, destinationOwner: String) {}"
        ));
    }

    #[test]
    fn transfer_inside_string_or_comment_is_not_rewritten() {
        let source = "// transfer selected from left to right\nderived text = \"transfer selected from left to right\"\n";
        assert_eq!(preprocess(source).unwrap(), source);
    }

    #[test]
    fn check_accepts_plain_and_maybe_live_transfer_targets() {
        let source = r#"
state model Document { state title = "" }
state model Folder { state documents: [live Document] = [] }
state fallback: Document
state left: Folder
state right: Folder
state plain: live Document = live fallback
state optional: maybe live Document = none

action transferPlain { transfer plain from left to right }
action transferOptional { transfer optional from left to right }
"#;
        crate::check_source(source).expect("both persistent designation forms should check");
    }

    #[test]
    fn check_rejects_non_designation_target_and_non_owner_operands() {
        let source = r#"
state model Document { state title = "" }
state model Folder { state documents: [live Document] = [] }
state left: Folder
state right: Folder
state ordinary = 0

action invalid {
    transfer ordinary from ordinary to right
}
"#;
        let errors = crate::check_source(source).expect_err("invalid transfer should fail");
        assert!(errors
            .iter()
            .any(|error| error.message.contains("requires persistent live or maybe live")));
        assert!(errors
            .iter()
            .any(|error| error.message.contains("transfer source owner 'ordinary'")));
    }

    #[test]
    fn source_transfer_changes_lifetime_proof_without_editing_membership() {
        let source = r#"
state model Document { state title = "" }
state model Folder { state documents: [live Document] = [] }
state left: Folder
state right: Folder
state selected: maybe live Document = none

action seed {
    create Document in left as document {
        through document.title = "Draft"
        insert document into left.documents
        insert document into right.documents
    }
    selected = left.documents[0]
}

action transferSelected {
    transfer selected from left to right
}

action destroyFromLeft {
    destroy selected in left
}

action destroyFromRight {
    destroy selected in right
}
"#;
        let checked = crate::check_source_with_runtime_models(source).expect("source should check");
        let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");
        runtime.run_action("seed").expect("seed should commit");

        let left_before = runtime.value("__meld_mseq$left$documents").unwrap();
        let right_before = runtime.value("__meld_mseq$right$documents").unwrap();
        assert_eq!(left_before, right_before);

        runtime
            .run_action("transferSelected")
            .expect("transfer should commit");
        assert_eq!(
            runtime.value("__meld_mseq$left$documents").unwrap(),
            left_before
        );
        assert_eq!(
            runtime.value("__meld_mseq$right$documents").unwrap(),
            right_before
        );

        let error = runtime
            .run_action("destroyFromLeft")
            .expect_err("old owner must no longer prove lifetime authority");
        assert!(error.message.contains("requires rooting owner 'right'"));

        runtime
            .run_action("destroyFromRight")
            .expect("new owner should prove lifetime authority");
        assert_eq!(
            runtime.value("__meld_live$selected").unwrap(),
            Value::String(String::new())
        );
    }

    #[test]
    fn absent_target_and_later_failure_abort_transfer_transactionally() {
        let source = r#"
state model Document { state title = "" }
state model Folder { state documents: [live Document] = [] }
state left: Folder
state right: Folder
state selected: maybe live Document = none

action absentTransfer {
    transfer selected from left to right
}

action seed {
    create Document in left as document {
        insert document into left.documents
    }
    selected = left.documents[0]
}

action transferThenFail {
    transfer selected from left to right
    fail "abort"
}

action destroyFromLeft {
    destroy selected in left
}
"#;
        let checked = crate::check_source_with_runtime_models(source).expect("source should check");
        let mut runtime = Runtime::from_checked_source(&checked).expect("runtime should initialize");

        let absent = runtime
            .run_action("absentTransfer")
            .expect_err("absent maybe-live target should fail");
        assert!(absent.message.contains("present live designation"));

        runtime.run_action("seed").expect("seed should commit");
        runtime
            .run_action("transferThenFail")
            .expect_err("later failure should roll transfer back");
        runtime
            .run_action("destroyFromLeft")
            .expect("original owner should remain authoritative after rollback");
    }
}
