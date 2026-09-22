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
use crate::scoped_create_surface::INSERT_SCOPE_MARKER_ACTION;
use crate::semantic::ValueType;

pub(crate) const GENERATED_EXISTING_INSERT_ACTION_PREFIX: &str =
    "__meld_create_scope_insert_existing_";
const FINALIZED_EXISTING_INSERT_ACTION_PREFIX: &str = "__meld_create_scope_insert_persistent_";

/// Validate residual `insert designation into owner.membership` markers after scoped-create
/// and scoped-designation lowering, then preserve them until live-designation lowering has
/// converted persistent designation state into its private identity carrier.
pub fn lower(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    roots: &HashMap<String, RuntimeModelRoot>,
) -> Result<Program, Vec<Diagnostic>> {
    let designation_models = collect_persistent_designation_models(program);
    let mut lowerer = InsertLowerer {
        templates,
        roots,
        designation_models: &designation_models,
        next_id: 0,
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

    Ok(Program {
        declarations,
        state_models: program.state_models.clone(),
    })
}

/// Realize validated persistent-designation insertion after live-designation lowering.
///
/// At this point the first marker argument is the compiler-private identity carrier. The
/// finalized helper intentionally enters the same structural runtime insertion path as fresh
/// scoped insertion. Structural membership therefore does not change the child's owner/rooting
/// relation and does not grant writable child authority.
pub fn finalize(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    let mut finalizer = InsertFinalizer {
        generated_actions: Vec::new(),
        errors: Vec::new(),
    };

    let mut declarations = program
        .declarations
        .iter()
        .map(|declaration| match declaration {
            Declaration::Action(action) => Declaration::Action(ActionDecl {
                location: action.location,
                name: action.name.clone(),
                parameters: action.parameters.clone(),
                statements: finalizer.finalize_statements(&action.statements),
            }),
            other => other.clone(),
        })
        .collect::<Vec<_>>();

    if !finalizer.errors.is_empty() {
        return Err(finalizer.errors);
    }

    declarations.extend(finalizer.generated_actions);
    Ok(Program {
        declarations,
        state_models: program.state_models.clone(),
    })
}

fn collect_persistent_designation_models(program: &Program) -> HashMap<String, String> {
    let mut result = HashMap::new();
    for declaration in &program.declarations {
        let Declaration::State(state) = declaration else {
            continue;
        };
        let Some(type_name) = state.type_name.as_deref() else {
            continue;
        };
        if let Some(model) =
            decode_live_type_name(type_name).or_else(|| decode_maybe_live_type_name(type_name))
        {
            result.insert(state.name.clone(), model.to_string());
        }
    }
    result
}

struct InsertLowerer<'a> {
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    roots: &'a HashMap<String, RuntimeModelRoot>,
    designation_models: &'a HashMap<String, String>,
    next_id: usize,
    errors: Vec<Diagnostic>,
}

impl InsertLowerer<'_> {
    fn lower_statements(&mut self, statements: &[Statement]) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| match statement {
                Statement::ActionCall {
                    location,
                    name,
                    arguments,
                } if name == INSERT_SCOPE_MARKER_ACTION => self.lower_insert(*location, arguments),
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

    fn lower_insert(
        &mut self,
        location: SourceLocation,
        arguments: &[ActionArgument],
    ) -> Statement {
        let [ActionArgument::Value(Expr::String(designation_name)), ActionArgument::StateGrant { name: target, .. }] =
            arguments
        else {
            self.errors.push(diag(
                location,
                "internal existing-designation insertion marker is malformed",
            ));
            return malformed_insert(location);
        };

        let Some(designation_model) = self.designation_models.get(designation_name) else {
            self.errors.push(diag(
                location,
                format!(
                    "insertion source '{designation_name}' must be a persistent live designation"
                ),
            ));
            return malformed_insert(location);
        };

        let Some((root_name, member_name)) = split_owner_member(target) else {
            self.errors.push(diag(
                location,
                format!("insertion target '{target}' must name one owner-relative sequence member"),
            ));
            return malformed_insert(location);
        };
        let owner_model = if let Some(root) = self.roots.get(root_name) {
            root.model_name.clone()
        } else if let Some(model) = self.designation_models.get(root_name) {
            model.clone()
        } else {
            self.errors.push(diag(
                location,
                format!(
                    "'{root_name}' is not a modeled-state owner root or persistent live designation"
                ),
            ));
            return malformed_insert(location);
        };
        let Some(owner_template) = self.templates.get(&owner_model) else {
            self.errors.push(diag(
                location,
                format!("unknown state model '{owner_model}'"),
            ));
            return malformed_insert(location);
        };
        let Some(member) = owner_template.member(member_name) else {
            self.errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    owner_template.name, member_name
                ),
            ));
            return malformed_insert(location);
        };
        if member.kind != RuntimeModelMemberKind::State {
            self.errors.push(diag(
                location,
                format!("insertion target '{target}' must be mutable [live T] state"),
            ));
            return malformed_insert(location);
        }
        let ValueType::SequenceLive(element_model) = &member.value_type else {
            self.errors.push(diag(
                location,
                format!("insertion target '{target}' must have type [live T]"),
            ));
            return malformed_insert(location);
        };
        if designation_model != element_model {
            self.errors.push(diag(
                location,
                format!(
                    "live designation '{designation_name}' has type live {designation_model} but insertion target contains live {element_model}"
                ),
            ));
            return malformed_insert(location);
        }

        let marker_name = format!("{GENERATED_EXISTING_INSERT_ACTION_PREFIX}{}", self.next_id);
        self.next_id += 1;

        Statement::ActionCall {
            location,
            name: marker_name,
            arguments: vec![
                ActionArgument::Value(Expr::Name(designation_name.clone())),
                ActionArgument::StateGrant {
                    location,
                    name: target.clone(),
                },
                ActionArgument::Value(Expr::String(element_model.clone())),
            ],
        }
    }
}

struct InsertFinalizer {
    generated_actions: Vec<Declaration>,
    errors: Vec<Diagnostic>,
}

impl InsertFinalizer {
    fn finalize_statements(&mut self, statements: &[Statement]) -> Vec<Statement> {
        statements
            .iter()
            .map(|statement| match statement {
                Statement::ActionCall {
                    location,
                    name,
                    arguments,
                } if name.starts_with(GENERATED_EXISTING_INSERT_ACTION_PREFIX) => {
                    self.finalize_insert(*location, name, arguments)
                }
                Statement::If {
                    location,
                    condition,
                    then_branch,
                    else_branch,
                } => Statement::If {
                    location: *location,
                    condition: condition.clone(),
                    then_branch: self.finalize_statements(then_branch),
                    else_branch: else_branch
                        .as_ref()
                        .map(|branch| self.finalize_statements(branch)),
                },
                other => other.clone(),
            })
            .collect()
    }

    fn finalize_insert(
        &mut self,
        location: SourceLocation,
        marker_name: &str,
        arguments: &[ActionArgument],
    ) -> Statement {
        let [ActionArgument::Value(identity), ActionArgument::StateGrant { name: target, .. }, ActionArgument::Value(Expr::String(element_model))] =
            arguments
        else {
            self.errors.push(diag(
                location,
                "internal existing-designation insertion carrier is malformed",
            ));
            return malformed_insert(location);
        };

        let suffix = marker_name
            .strip_prefix(GENERATED_EXISTING_INSERT_ACTION_PREFIX)
            .expect("existing insertion marker prefix should match");
        let helper_name = format!("{FINALIZED_EXISTING_INSERT_ACTION_PREFIX}{suffix}");
        self.generated_actions.push(Declaration::Action(ActionDecl {
            location,
            name: helper_name.clone(),
            parameters: vec![
                ActionParameter {
                    location,
                    kind: ActionParameterKind::Value,
                    name: "identity".to_string(),
                    type_name: "String".to_string(),
                },
                ActionParameter {
                    location,
                    kind: ActionParameterKind::State,
                    name: "target".to_string(),
                    type_name: encode_runtime_sequence_type(element_model),
                },
            ],
            statements: Vec::new(),
        }));

        Statement::ActionCall {
            location,
            name: helper_name,
            arguments: vec![
                ActionArgument::Value(identity.clone()),
                ActionArgument::StateGrant {
                    location,
                    name: target.clone(),
                },
            ],
        }
    }
}

fn split_owner_member(path: &str) -> Option<(&str, &str)> {
    let (owner, member) = path.split_once('.')?;
    (!owner.is_empty() && !member.is_empty() && !member.contains('.')).then_some((owner, member))
}

fn malformed_insert(location: SourceLocation) -> Statement {
    Statement::Fail {
        location,
        message: Expr::String("invalid structural insertion".to_string()),
    }
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message.into(), location.line, location.column)
}
