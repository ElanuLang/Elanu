use std::collections::{HashMap, HashSet};

use crate::ast::{
    decode_live_capture, decode_live_derived_name, decode_live_type_name,
    decode_maybe_live_type_name, decode_through_path, ActionArgument, ActionDecl,
    AssignmentOperator, BinaryOperator, Declaration, DerivedDecl, Expr, Program, SourceLocation,
    StateDecl, Statement,
};
use crate::designation_runtime_metadata::LOWERED_DESIGNATION_PREFIX;
use crate::diagnostic::Diagnostic;
use crate::program_facts::{
    ModelFacts as ModelInfo, ModelMemberKind as MemberKind, ModeledStateRoot as RootInfo,
    ProgramFacts,
};
use crate::runtime_model_templates::RuntimeModelTemplate;
use crate::scoped_designation_surface::{decode_scope_identity_param, SCOPE_BUILTIN_ACTION};
use crate::semantic::ValueType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesignationKind {
    State,
    Derived,
    Scoped,
}

#[derive(Debug, Clone)]
struct DesignationInfo {
    source_name: String,
    model_name: String,
    lowered_name: String,
    kind: DesignationKind,
    allows_none: bool,
    declaration_index: usize,
    candidates: Vec<RootInfo>,
}

struct DesignationExprContext<'a> {
    location: SourceLocation,
    declaration_index: usize,
    designations: &'a HashMap<String, DesignationInfo>,
    roots: &'a HashMap<String, RootInfo>,
    models: &'a HashMap<String, ModelInfo>,
    templates: &'a HashMap<String, RuntimeModelTemplate>,
    scoped_create_designations: &'a HashMap<String, String>,
}

/// Lower the experimental `live` / `through` source surface into ordinary
/// scalar/state-model operations before the existing state-model lowering pass.
///
/// The bootstrap representation deliberately uses an opaque String tag for a
/// live designation. Source code cannot observe that representation: bare
/// designation values, equality, action parameters, and whole-value projection
/// remain rejected. Read-through and `through` mutation are expanded into
/// ordinary `if` expressions/statements over statically declared modeled roots.
pub fn lower(
    program: &Program,
    templates: &HashMap<String, RuntimeModelTemplate>,
    scoped_create_designations: &HashMap<String, String>,
) -> Result<Program, Vec<Diagnostic>> {
    let facts = ProgramFacts::from_program(program);
    let models = facts.models();
    let roots = facts.modeled_state_roots();
    let mut errors = Vec::new();

    check_duplicate_source_names(program, &mut errors);
    let designations = collect_designations(program, models, roots, &mut errors);

    if !errors.is_empty() {
        return Err(errors);
    }

    let root_by_name: HashMap<String, RootInfo> = roots
        .iter()
        .cloned()
        .map(|root| (root.name.clone(), root))
        .collect();

    let mut declarations = Vec::new();
    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        match declaration {
            Declaration::State(state) => {
                if let Some((model_name, _)) = state
                    .type_name
                    .as_deref()
                    .and_then(decode_designation_state_type)
                {
                    let Some(info) = designations.get(&state.name) else {
                        continue;
                    };
                    let context = DesignationExprContext {
                        location: state.location,
                        declaration_index,
                        designations: &designations,
                        roots: &root_by_name,
                        models,
                        templates,
                        scoped_create_designations,
                    };
                    let initializer =
                        lower_designation_expr(&state.initializer, info, &context, &mut errors);
                    declarations.push(Declaration::State(StateDecl {
                        location: state.location,
                        name: info.lowered_name.clone(),
                        type_name: Some("String".to_string()),
                        initializer,
                        implicit_model_initializer: false,
                    }));
                    debug_assert_eq!(model_name, info.model_name);
                } else {
                    let mut lowered = state.clone();
                    lowered.initializer = lower_expr(
                        &state.initializer,
                        state.location,
                        declaration_index,
                        &designations,
                        models,
                        templates,
                        &mut errors,
                    );
                    declarations.push(Declaration::State(lowered));
                }
            }
            Declaration::Derived(derived) => {
                if let Some((source_name, model_name)) = decode_live_derived_name(&derived.name) {
                    let Some(info) = designations.get(source_name) else {
                        continue;
                    };
                    let context = DesignationExprContext {
                        location: derived.location,
                        declaration_index,
                        designations: &designations,
                        roots: &root_by_name,
                        models,
                        templates,
                        scoped_create_designations,
                    };
                    let expression =
                        lower_designation_expr(&derived.expression, info, &context, &mut errors);
                    declarations.push(Declaration::Derived(DerivedDecl {
                        location: derived.location,
                        name: info.lowered_name.clone(),
                        expression,
                    }));
                    debug_assert_eq!(model_name, info.model_name);
                } else {
                    declarations.push(Declaration::Derived(DerivedDecl {
                        location: derived.location,
                        name: derived.name.clone(),
                        expression: lower_expr(
                            &derived.expression,
                            derived.location,
                            declaration_index,
                            &designations,
                            models,
                            templates,
                            &mut errors,
                        ),
                    }));
                }
            }
            Declaration::Action(action) => {
                let statements = action
                    .statements
                    .iter()
                    .map(|statement| {
                        lower_statement(
                            statement,
                            declaration_index,
                            &designations,
                            models,
                            templates,
                            scoped_create_designations,
                            &mut errors,
                        )
                    })
                    .collect();
                declarations.push(Declaration::Action(ActionDecl {
                    location: action.location,
                    name: action.name.clone(),
                    parameters: action.parameters.clone(),
                    statements,
                }));
            }
        }
    }

    if errors.is_empty() {
        Ok(Program {
            declarations,
            state_models: program.state_models.clone(),
        })
    } else {
        Err(errors)
    }
}

fn collect_designations(
    program: &Program,
    models: &HashMap<String, ModelInfo>,
    roots: &[RootInfo],
    errors: &mut Vec<Diagnostic>,
) -> HashMap<String, DesignationInfo> {
    let mut designations = HashMap::new();

    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let candidate = match declaration {
            Declaration::State(state) => state
                .type_name
                .as_deref()
                .and_then(decode_designation_state_type)
                .map(|(model_name, allows_none)| {
                    (
                        state.name.as_str(),
                        model_name,
                        DesignationKind::State,
                        allows_none,
                        state.location,
                    )
                }),
            Declaration::Derived(derived) => {
                decode_live_derived_name(&derived.name).map(|(source_name, model_name)| {
                    (
                        source_name,
                        model_name,
                        DesignationKind::Derived,
                        false,
                        derived.location,
                    )
                })
            }
            Declaration::Action(_) => None,
        };

        let Some((source_name, model_name, kind, allows_none, location)) = candidate else {
            continue;
        };

        if !models.contains_key(model_name) {
            errors.push(diag(
                location,
                format!(
                    "live designation '{}' names unknown state model '{}'",
                    source_name, model_name
                ),
            ));
            continue;
        }

        let candidates = roots
            .iter()
            .filter(|root| {
                root.model_name == model_name && root.declaration_index < declaration_index
            })
            .cloned()
            .collect();

        designations.insert(
            source_name.to_string(),
            DesignationInfo {
                source_name: source_name.to_string(),
                model_name: model_name.to_string(),
                lowered_name: lowered_designation_name(source_name),
                kind,
                allows_none,
                declaration_index,
                candidates,
            },
        );
    }

    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        for parameter in &action.parameters {
            let Some(model_name) = decode_scope_identity_param(&parameter.name) else {
                continue;
            };
            if !models.contains_key(model_name) {
                errors.push(diag(
                    parameter.location,
                    format!("scoped designation names unknown state model '{model_name}'"),
                ));
                continue;
            }
            designations.insert(
                parameter.name.clone(),
                DesignationInfo {
                    source_name: "scoped designation".to_string(),
                    model_name: model_name.to_string(),
                    lowered_name: parameter.name.clone(),
                    kind: DesignationKind::Scoped,
                    allows_none: false,
                    declaration_index,
                    candidates: Vec::new(),
                },
            );
        }
    }

    designations
}

fn designation_is_available(info: &DesignationInfo, declaration_index: usize) -> bool {
    info.kind == DesignationKind::Scoped || info.declaration_index < declaration_index
}

fn lower_model_local_designation_read(
    name: &str,
    expected: &DesignationInfo,
    context: &DesignationExprContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Option<Expr> {
    let (owner_name, member) = name.split_once('.')?;
    if member.contains('.') {
        return None;
    }
    let owner = context.designations.get(owner_name)?;
    if !designation_is_available(owner, context.declaration_index) {
        errors.push(diag(
            context.location,
            format!(
                "live designation '{}' is not available before its declaration",
                owner_name
            ),
        ));
        return Some(Expr::String(String::new()));
    }
    let template = context.templates.get(&owner.model_name)?;
    let member_template = template.member(member)?;
    let designation = member_template.designation.as_ref()?;
    if designation.model_name != expected.model_name {
        errors.push(diag(
            context.location,
            format!(
                "model-local designation '{}.{}' has type maybe live {} but '{}' requires live {}",
                owner_name,
                member,
                designation.model_name,
                expected.source_name,
                expected.model_name
            ),
        ));
        return Some(Expr::String(String::new()));
    }
    if designation.allows_none && !expected.allows_none {
        errors.push(diag(
            context.location,
            format!(
                "model-local maybe live {} designation '{}.{}' may be absent but '{}' requires plain live {}",
                designation.model_name, owner_name, member, expected.source_name, expected.model_name
            ),
        ));
        return Some(Expr::String(String::new()));
    }
    Some(Expr::RuntimeDesignationMember {
        designation: Box::new(Expr::Name(owner.lowered_name.clone())),
        member: member.to_string(),
        element_model: owner.model_name.clone(),
        member_type_name: "String".to_string(),
    })
}

fn lower_model_local_designation_assignment_value(
    expression: &Expr,
    target_model: &str,
    location: SourceLocation,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Expr::Name(name) = expression else {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {target_model} assignment requires a compatible persistent designation or 'none'"
            ),
        ));
        return Expr::String(String::new());
    };
    if name == "none" {
        return Expr::String(String::new());
    }
    let Some(source) = designations.get(name) else {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {target_model} assignment requires a compatible persistent designation or 'none'; '{name}' is not one"
            ),
        ));
        return Expr::String(String::new());
    };
    if !designation_is_available(source, declaration_index) {
        errors.push(diag(
            location,
            format!(
                "live designation '{}' is not available before its declaration",
                name
            ),
        ));
        return Expr::String(String::new());
    }
    if source.model_name != target_model {
        errors.push(diag(
            location,
            format!(
                "live designation '{}' has type live {} but model-local slot requires maybe live {}",
                source.source_name, source.model_name, target_model
            ),
        ));
        return Expr::String(String::new());
    }
    Expr::Name(source.lowered_name.clone())
}

fn check_duplicate_source_names(program: &Program, errors: &mut Vec<Diagnostic>) {
    let mut seen = HashSet::new();
    for declaration in &program.declarations {
        let (name, location) = match declaration {
            Declaration::State(state) => (state.name.as_str(), state.location),
            Declaration::Derived(derived) => {
                if let Some((source_name, _)) = decode_live_derived_name(&derived.name) {
                    (source_name, derived.location)
                } else {
                    (derived.name.as_str(), derived.location)
                }
            }
            Declaration::Action(action) => (action.name.as_str(), action.location),
        };

        if !seen.insert(name.to_string()) {
            errors.push(diag(location, format!("duplicate declaration '{}'", name)));
        }
    }
}

fn lower_designation_expr(
    expression: &Expr,
    expected: &DesignationInfo,
    context: &DesignationExprContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Name(name) => {
            if name == "none" {
                if expected.allows_none {
                    return Expr::String(String::new());
                }
                errors.push(diag(
                    context.location,
                    format!(
                        "'none' cannot be used for plain live {} designation '{}'; use 'maybe live {}' when absence is part of the state",
                        expected.model_name, expected.source_name, expected.model_name
                    ),
                ));
                return Expr::String(String::new());
            }

            if let Some(lowered) =
                lower_model_local_designation_read(name, expected, context, errors)
            {
                return lowered;
            }

            if let Some(root_name) = decode_live_capture(name) {
                let Some(root) = context.roots.get(root_name) else {
                    errors.push(diag(
                        context.location,
                        format!(
                            "'live {}' requires a statically declared modeled-state variable",
                            root_name
                        ),
                    ));
                    return Expr::String(String::new());
                };
                if root.model_name != expected.model_name {
                    errors.push(diag(
                        context.location,
                        format!(
                            "'live {}' designates {} but '{}' requires live {}",
                            root_name, root.model_name, expected.source_name, expected.model_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                if root.declaration_index >= context.declaration_index
                    || !expected
                        .candidates
                        .iter()
                        .any(|candidate| candidate.name == root.name)
                {
                    errors.push(diag(
                        context.location,
                        format!(
                            "bootstrap live-designation spike requires target '{}' to be declared before designation '{}'",
                            root_name, expected.source_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                return Expr::String(root.name.clone());
            }

            if let Some(model_name) = context.scoped_create_designations.get(name) {
                if model_name != &expected.model_name {
                    errors.push(diag(
                        context.location,
                        format!(
                            "fresh scoped designation has type live {model_name} but '{}' requires live {}",
                            expected.source_name, expected.model_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                return Expr::Name(name.clone());
            }

            if let Some(source) = context.designations.get(name) {
                if source.model_name != expected.model_name {
                    errors.push(diag(
                        context.location,
                        format!(
                            "live designation '{}' has type live {} but '{}' requires live {}",
                            source.source_name,
                            source.model_name,
                            expected.source_name,
                            expected.model_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                if source.allows_none && !expected.allows_none {
                    errors.push(diag(
                        context.location,
                        format!(
                            "maybe live {} designation '{}' may be absent but '{}' requires plain live {}",
                            source.model_name, name, expected.source_name, expected.model_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                if !designation_is_available(source, context.declaration_index) {
                    errors.push(diag(
                        context.location,
                        format!(
                            "live designation '{}' is not available before its declaration",
                            name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                let target_names: HashSet<&str> = expected
                    .candidates
                    .iter()
                    .map(|root| root.name.as_str())
                    .collect();
                if source
                    .candidates
                    .iter()
                    .any(|root| !target_names.contains(root.name.as_str()))
                {
                    errors.push(diag(
                        context.location,
                        format!(
                            "live designation '{}' may select targets unavailable to '{}' in the bootstrap spike",
                            name, expected.source_name
                        ),
                    ));
                    return Expr::String(String::new());
                }
                return Expr::Name(source.lowered_name.clone());
            }

            errors.push(diag(
                context.location,
                format!(
                    "live {} expression for '{}' must use 'live <state-binding>', a compatible earlier live designation, an indexed [live {}] designation, or an if expression",
                    expected.model_name, expected.source_name, expected.model_name
                ),
            ));
            Expr::String(String::new())
        }
        Expr::Binary {
            operator,
            left,
            right,
        } if matches!(
            *operator,
            BinaryOperator::PreviousIn | BinaryOperator::NextIn
        ) =>
        {
            let Expr::Name(name) = left.as_ref() else {
                errors.push(diag(
                    context.location,
                    "relative-navigation anchor must be a persistent live designation".to_string(),
                ));
                return Expr::String(String::new());
            };
            let Some(source) = context.designations.get(name) else {
                errors.push(diag(
                    context.location,
                    format!(
                        "relative-navigation anchor '{}' is not a persistent live designation",
                        name
                    ),
                ));
                return Expr::String(String::new());
            };
            if source.model_name != expected.model_name {
                errors.push(diag(
                    context.location,
                    format!(
                        "relative-navigation anchor '{}' has type live {} but '{}' requires live {}",
                        source.source_name, source.model_name, expected.source_name, expected.model_name
                    ),
                ));
                return Expr::String(String::new());
            }
            if !designation_is_available(source, context.declaration_index) {
                errors.push(diag(
                    context.location,
                    format!(
                        "live designation '{}' is not available before its declaration",
                        name
                    ),
                ));
                return Expr::String(String::new());
            }

            Expr::Binary {
                operator: *operator,
                left: Box::new(Expr::Name(source.lowered_name.clone())),
                right: Box::new(lower_expr(
                    right,
                    context.location,
                    context.declaration_index,
                    context.designations,
                    context.models,
                    context.templates,
                    errors,
                )),
            }
        }
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => {
            if element_model != &expected.model_name {
                errors.push(diag(
                    context.location,
                    format!("indexed designation has type live {element_model} but '{}' requires live {}", expected.source_name, expected.model_name),
                ));
                Expr::String(String::new())
            } else {
                Expr::RuntimeIndexDesignation {
                    source: Box::new(lower_expr(
                        source,
                        context.location,
                        context.declaration_index,
                        context.designations,
                        context.models,
                        context.templates,
                        errors,
                    )),
                    index: Box::new(lower_expr(
                        index,
                        context.location,
                        context.declaration_index,
                        context.designations,
                        context.models,
                        context.templates,
                        errors,
                    )),
                    element_model: element_model.clone(),
                }
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(
                condition,
                context.location,
                context.declaration_index,
                context.designations,
                context.models,
                context.templates,
                errors,
            )),
            then_branch: Box::new(lower_designation_expr(
                then_branch,
                expected,
                context,
                errors,
            )),
            else_branch: Box::new(lower_designation_expr(
                else_branch,
                expected,
                context,
                errors,
            )),
        },
        _ => {
            errors.push(diag(
                context.location,
                format!(
                    "live {} expression for '{}' must preserve live identity rather than produce an ordinary value",
                    expected.model_name, expected.source_name
                ),
            ));
            Expr::String(String::new())
        }
    }
}

fn lower_expr(
    expression: &Expr,
    location: SourceLocation,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            if let Some(target) = decode_live_capture(name) {
                errors.push(diag(
                    location,
                    format!(
                        "'live {}' is a live designation, not an ordinary value; use it in a live designation declaration or assignment",
                        target
                    ),
                ));
                return Expr::String(String::new());
            }

            if let Some((root, member)) = split_exact_member_path(name, location, errors) {
                if let Some(info) = designations.get(root) {
                    if !designation_is_available(info, declaration_index) {
                        errors.push(diag(
                            location,
                            format!(
                                "live designation '{}' is not available before its declaration",
                                root
                            ),
                        ));
                        return Expr::Integer(0);
                    }
                    return lower_designation_member_read(
                        info, member, location, models, templates, errors,
                    );
                }
            }

            if designations.contains_key(name) {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' is not an ordinary value; whole target value projection is deferred",
                        name
                    ),
                ));
                return Expr::Integer(0);
            }

            Expr::Name(name.clone())
        }
        Expr::Binary { operator, .. }
            if matches!(
                *operator,
                BinaryOperator::PreviousIn | BinaryOperator::NextIn
            ) =>
        {
            errors.push(diag(
                location,
                "relative navigation produces a live designation and is only valid in a compatible live designation context".to_string(),
            ));
            Expr::String(String::new())
        }
        Expr::Binary {
            operator: BinaryOperator::IsIn,
            left,
            right,
        } => {
            let Expr::Name(name) = left.as_ref() else {
                errors.push(diag(
                    location,
                    "left operand of 'is in' must be a persistent live designation".to_string(),
                ));
                return Expr::Bool(false);
            };
            let Some(info) = designations.get(name) else {
                errors.push(diag(
                    location,
                    format!("'{}' is not a persistent live designation", name),
                ));
                return Expr::Bool(false);
            };
            if !designation_is_available(info, declaration_index) {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' is not available before its declaration",
                        name
                    ),
                ));
                return Expr::Bool(false);
            }
            Expr::Binary {
                operator: BinaryOperator::IsIn,
                left: Box::new(Expr::Name(info.lowered_name.clone())),
                right: Box::new(lower_expr(
                    right,
                    location,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    errors,
                )),
            }
        }
        Expr::Binary {
            operator: BinaryOperator::IsPresent,
            left,
            ..
        } => {
            let Expr::Name(name) = left.as_ref() else {
                errors.push(diag(
                    location,
                    "'is present' requires a maybe live designation name".to_string(),
                ));
                return Expr::Bool(false);
            };
            let Some(info) = designations.get(name) else {
                errors.push(diag(
                    location,
                    format!(
                        "'is present' requires maybe live T, but '{}' is not a live designation",
                        name
                    ),
                ));
                return Expr::Bool(false);
            };
            if !info.allows_none {
                errors.push(diag(
                    location,
                    format!(
                        "'is present' requires maybe live {}, but '{}' is plain live {}",
                        info.model_name, info.source_name, info.model_name
                    ),
                ));
                return Expr::Bool(false);
            }
            Expr::Binary {
                operator: BinaryOperator::NotEqual,
                left: Box::new(Expr::Name(info.lowered_name.clone())),
                right: Box::new(Expr::String(String::new())),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(lower_expr(
                left,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            right: Box::new(lower_expr(
                right,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(
                condition,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            then_branch: Box::new(lower_expr(
                then_branch,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            else_branch: Box::new(lower_expr(
                else_branch,
                location,
                declaration_index,
                designations,
                models,
                templates,
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
            source: Box::new(lower_expr(
                source,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            index: Box::new(lower_expr(
                index,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => Expr::RuntimeIndexDesignation {
            source: Box::new(lower_expr(
                source,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            index: Box::new(lower_expr(
                index,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            element_model: element_model.clone(),
        },
        Expr::RuntimeDesignationMember {
            designation,
            member,
            element_model,
            member_type_name,
        } => Expr::RuntimeDesignationMember {
            designation: Box::new(lower_runtime_designation_operand(
                designation,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
        },
        Expr::Filter { .. } => expression.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_runtime_designation_operand(
    expression: &Expr,
    location: SourceLocation,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    if let Expr::Name(name) = expression {
        if let Some(info) = designations.get(name) {
            if !designation_is_available(info, declaration_index) {
                errors.push(diag(
                    location,
                    format!(
                        "live designation '{}' is not available before its declaration",
                        name
                    ),
                ));
                return Expr::String(String::new());
            }
            return Expr::Name(info.lowered_name.clone());
        }
    }
    lower_expr(
        expression,
        location,
        declaration_index,
        designations,
        models,
        templates,
        errors,
    )
}

fn lower_designation_member_read(
    info: &DesignationInfo,
    member: &str,
    location: SourceLocation,
    _models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Some(template) = templates.get(&info.model_name) else {
        errors.push(diag(
            location,
            format!("missing runtime model template for '{}'", info.model_name),
        ));
        return Expr::Integer(0);
    };
    let Some(member_template) = template.member(member) else {
        return Expr::Integer(0);
    };
    if let Some(designation) = &member_template.designation {
        errors.push(diag(
            location,
            format!(
                "model-local maybe live {} designation '{}.{}' is not an ordinary value; use it in a compatible designation context",
                designation.model_name, info.source_name, member
            ),
        ));
        return Expr::Integer(0);
    }
    Expr::RuntimeDesignationMember {
        designation: Box::new(Expr::Name(info.lowered_name.clone())),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
    }
}

fn lower_statement(
    statement: &Statement,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    scoped_create_designations: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    match statement {
        Statement::Assignment {
            location,
            target,
            operator,
            value,
        } => {
            if let Some(path) = decode_through_path(target) {
                return lower_through_assignment(
                    path,
                    *location,
                    *operator,
                    value,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    errors,
                );
            }

            if let Some(info) = designations.get(target) {
                if info.kind != DesignationKind::State {
                    let message = if info.kind == DesignationKind::Scoped {
                        "scoped live designation is immutable".to_string()
                    } else {
                        format!("cannot assign to derived live designation '{}'", target)
                    };
                    errors.push(diag(*location, message));
                    return statement.clone();
                }
                if *operator != AssignmentOperator::Assign {
                    errors.push(diag(
                        *location,
                        format!(
                            "live designation state '{}' supports selection assignment only, not compound arithmetic assignment",
                            target
                        ),
                    ));
                    return statement.clone();
                }
                let value = lower_designation_expr_for_assignment(
                    value,
                    info,
                    *location,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    scoped_create_designations,
                    errors,
                );
                return Statement::Assignment {
                    location: *location,
                    target: info.lowered_name.clone(),
                    operator: AssignmentOperator::Assign,
                    value,
                };
            }

            if let Some((root, _)) = split_exact_member_path(target, *location, errors) {
                if designations.contains_key(root) {
                    errors.push(diag(
                        *location,
                        format!(
                            "indirect mutation through live designation '{}' requires 'through {}'",
                            root, target
                        ),
                    ));
                    return statement.clone();
                }
            }

            Statement::Assignment {
                location: *location,
                target: target.clone(),
                operator: *operator,
                value: lower_expr(
                    value,
                    *location,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    errors,
                ),
            }
        }
        Statement::IndexedThroughAssignment { .. } => {
            unreachable!("source indexed assignments are consumed by sequence lowering")
        }
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
            index: index.clone(),
            member: member.clone(),
            element_model: element_model.clone(),
            member_type_name: member_type_name.clone(),
            operator: *operator,
            value: lower_expr(
                value,
                *location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            ),
        },
        Statement::RuntimeDesignationAssignment { .. } => statement.clone(),
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => expand_action_call(
            *location,
            name,
            arguments,
            declaration_index,
            designations,
            models,
            templates,
            scoped_create_designations,
            errors,
        ),
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: lower_expr(
                message,
                *location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            ),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: lower_expr(
                condition,
                *location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            ),
            then_branch: then_branch
                .iter()
                .map(|statement| {
                    lower_statement(
                        statement,
                        declaration_index,
                        designations,
                        models,
                        templates,
                        scoped_create_designations,
                        errors,
                    )
                })
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| {
                        lower_statement(
                            statement,
                            declaration_index,
                            designations,
                            models,
                            templates,
                            scoped_create_designations,
                            errors,
                        )
                    })
                    .collect()
            }),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_designation_expr_for_assignment(
    expression: &Expr,
    expected: &DesignationInfo,
    location: SourceLocation,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    scoped_create_designations: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let roots: HashMap<String, RootInfo> = expected
        .candidates
        .iter()
        .cloned()
        .map(|root| (root.name.clone(), root))
        .collect();
    let context = DesignationExprContext {
        location,
        declaration_index,
        designations,
        roots: &roots,
        models,
        templates,
        scoped_create_designations,
    };
    lower_designation_expr(expression, expected, &context, errors)
}

#[allow(clippy::too_many_arguments)]
fn lower_through_assignment(
    path: &str,
    location: SourceLocation,
    operator: AssignmentOperator,
    value: &Expr,
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    let Some((designation_name, member)) = split_exact_member_path(path, location, errors) else {
        errors.push(diag(
            location,
            "'through' assignment requires '<designation>.<state-member>'".to_string(),
        ));
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };

    let Some(info) = designations.get(designation_name) else {
        errors.push(diag(
            location,
            format!("'{}' is not a live designation", designation_name),
        ));
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };

    let Some(model) = models.get(&info.model_name) else {
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };
    match model.members.get(member) {
        Some(MemberKind::State) => {}
        Some(MemberKind::Derived) => {
            errors.push(diag(
                location,
                format!(
                    "cannot mutate derived member '{}.{}' through live designation",
                    designation_name, member
                ),
            ));
        }
        None => {
            errors.push(diag(
                location,
                format!(
                    "state model '{}' has no member '{}'",
                    info.model_name, member
                ),
            ));
        }
    }

    let Some(template) = templates.get(&info.model_name) else {
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };
    let Some(member_template) = template.member(member) else {
        return Statement::Assignment {
            location,
            target: path.to_string(),
            operator,
            value: value.clone(),
        };
    };
    if let Some(designation) = &member_template.designation {
        if operator != AssignmentOperator::Assign {
            errors.push(diag(
                location,
                format!(
                    "model-local maybe live {} designation '{}.{}' supports selection assignment only",
                    designation.model_name, designation_name, member
                ),
            ));
        }
        let lowered_value = lower_model_local_designation_assignment_value(
            value,
            &designation.model_name,
            location,
            declaration_index,
            designations,
            errors,
        );
        return Statement::RuntimeDesignationAssignment {
            location,
            designation: Box::new(Expr::Name(info.lowered_name.clone())),
            member: member.to_string(),
            element_model: info.model_name.clone(),
            member_type_name: "String".to_string(),
            operator: AssignmentOperator::Assign,
            value: lowered_value,
        };
    }
    let lowered_value = lower_expr(
        value,
        location,
        declaration_index,
        designations,
        models,
        templates,
        errors,
    );
    Statement::RuntimeDesignationAssignment {
        location,
        designation: Box::new(Expr::Name(info.lowered_name.clone())),
        member: member.to_string(),
        element_model: info.model_name.clone(),
        member_type_name: runtime_value_type_name(&member_template.value_type),
        operator,
        value: lowered_value,
    }
}

#[allow(clippy::too_many_arguments)]
fn expand_action_call(
    location: SourceLocation,
    name: &str,
    arguments: &[ActionArgument],
    declaration_index: usize,
    designations: &HashMap<String, DesignationInfo>,
    models: &HashMap<String, ModelInfo>,
    templates: &HashMap<String, RuntimeModelTemplate>,
    scoped_create_designations: &HashMap<String, String>,
    errors: &mut Vec<Diagnostic>,
) -> Statement {
    if name == SCOPE_BUILTIN_ACTION {
        let [ActionArgument::Value(Expr::String(model_name)), ActionArgument::Value(capture), ActionArgument::Value(Expr::String(scope_action))] =
            arguments
        else {
            errors.push(diag(
                location,
                "internal scoped designation builtin is malformed",
            ));
            return Statement::Fail {
                location,
                message: Expr::String("invalid scoped designation".to_string()),
            };
        };
        let candidates = match capture {
            Expr::Name(source_name) => designations
                .get(source_name)
                .map(|info| info.candidates.clone())
                .unwrap_or_default(),
            Expr::Binary { left, .. } => match left.as_ref() {
                Expr::Name(source_name) => designations
                    .get(source_name)
                    .map(|info| info.candidates.clone())
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let expected = DesignationInfo {
            source_name: "scoped designation capture".to_string(),
            model_name: model_name.clone(),
            lowered_name: String::new(),
            kind: DesignationKind::Scoped,
            allows_none: true,
            declaration_index,
            candidates,
        };
        let capture = lower_designation_expr_for_assignment(
            capture,
            &expected,
            location,
            declaration_index,
            designations,
            models,
            templates,
            scoped_create_designations,
            errors,
        );
        return Statement::ActionCall {
            location,
            name: name.to_string(),
            arguments: vec![
                ActionArgument::Value(Expr::String(model_name.clone())),
                ActionArgument::Value(capture),
                ActionArgument::Value(Expr::String(scope_action.clone())),
            ],
        };
    }

    let through_index = arguments.iter().position(|argument| {
        matches!(
            argument,
            ActionArgument::StateGrant { name, .. } if decode_through_path(name).is_some()
        )
    });

    if let Some(index) = through_index {
        let ActionArgument::StateGrant {
            location: grant_location,
            name: encoded_path,
        } = &arguments[index]
        else {
            unreachable!();
        };
        let path = decode_through_path(encoded_path).expect("through argument should decode");
        let (designation_name, member) = split_designation_path(path, *grant_location, errors);
        let Some(designation_name) = designation_name else {
            return Statement::ActionCall {
                location,
                name: name.to_string(),
                arguments: arguments.to_vec(),
            };
        };
        let Some(info) = designations.get(designation_name) else {
            errors.push(diag(
                *grant_location,
                format!("'{}' is not a live designation", designation_name),
            ));
            return Statement::ActionCall {
                location,
                name: name.to_string(),
                arguments: arguments.to_vec(),
            };
        };

        if let Some(member) = member {
            let Some(model) = models.get(&info.model_name) else {
                return Statement::ActionCall {
                    location,
                    name: name.to_string(),
                    arguments: arguments.to_vec(),
                };
            };
            match model.members.get(member) {
                Some(MemberKind::State) => {}
                Some(MemberKind::Derived) => errors.push(diag(
                    *grant_location,
                    format!(
                        "cannot grant derived member '{}.{}' as writable state through designation",
                        designation_name, member
                    ),
                )),
                None => errors.push(diag(
                    *grant_location,
                    format!(
                        "state model '{}' has no member '{}'",
                        info.model_name, member
                    ),
                )),
            }
        }

        if info.candidates.is_empty() {
            errors.push(diag(
                *grant_location,
                format!(
                    "live designation '{}' has no writable targets in the bootstrap spike",
                    designation_name
                ),
            ));
            return Statement::ActionCall {
                location,
                name: name.to_string(),
                arguments: arguments.to_vec(),
            };
        }

        let branches = info
            .candidates
            .iter()
            .map(|root| {
                let mut replaced = arguments.to_vec();
                let resolved_name = match member {
                    Some(member) => format!("{}.{}", root.name, member),
                    None => root.name.clone(),
                };
                replaced[index] = ActionArgument::StateGrant {
                    location: *grant_location,
                    name: resolved_name,
                };
                expand_action_call(
                    location,
                    name,
                    &replaced,
                    declaration_index,
                    designations,
                    models,
                    templates,
                    scoped_create_designations,
                    errors,
                )
            })
            .collect();
        return build_statement_choice(info, location, branches);
    }

    let arguments = arguments
        .iter()
        .map(|argument| match argument {
            ActionArgument::Value(Expr::Name(source_name))
                if (name == crate::lifetime_transfer_surface::TRANSFER_BUILTIN_ACTION
                    || name == crate::lifetime_termination_surface::DESTROY_BUILTIN_ACTION
                    || name == crate::lifetime_termination_surface::PURGE_BUILTIN_ACTION
                    || name == crate::scoped_create_surface::CREATE_SCOPE_BUILTIN_ACTION
                    || name.starts_with(
                        crate::existing_designation_insert_surface::GENERATED_EXISTING_INSERT_ACTION_PREFIX,
                    )
                    || name.starts_with(crate::structural_edit_surface::GENERATED_REMOVE_ACTION_PREFIX)
                    || name.starts_with(
                        crate::structural_edit_surface::GENERATED_FILTERED_REMOVE_ACTION_PREFIX,
                    )
                    || name.starts_with(crate::structural_move_surface::GENERATED_MOVE_ACTION_PREFIX)
                    || name.starts_with(
                        crate::structural_move_surface::GENERATED_FILTERED_MOVE_ACTION_PREFIX,
                    ))
                    && designations.contains_key(source_name) =>
            {
                let info = &designations[source_name];
                ActionArgument::Value(Expr::Name(info.lowered_name.clone()))
            }
            ActionArgument::Value(expression) => ActionArgument::Value(lower_expr(
                expression,
                location,
                declaration_index,
                designations,
                models,
                templates,
                errors,
            )),
            ActionArgument::IndexedStateGrant { .. } => unreachable!("source indexed state grants are consumed by runtime index grant transport"),
            ActionArgument::StateGrant {
                location: grant_location,
                name: granted,
            } => {
                if let Some(info) = designations.get(granted) {
                    errors.push(diag(
                        *grant_location,
                        format!(
                            "passing authority to live designation slot '{}' is deferred with live action parameters; use direct selection assignment or 'state through {}' for target authority",
                            info.source_name, info.source_name
                        ),
                    ));
                    return ActionArgument::StateGrant {
                        location: *grant_location,
                        name: info.lowered_name.clone(),
                    };
                }

                if let Some((root, _)) = split_exact_member_path(granted, *grant_location, errors) {
                    if designations.contains_key(root) {
                        errors.push(diag(
                            *grant_location,
                            format!(
                                "writable authority through live designation '{}' requires 'state through {}'",
                                root, granted
                            ),
                        ));
                    }
                }

                ActionArgument::StateGrant {
                    location: *grant_location,
                    name: granted.clone(),
                }
            }
        })
        .collect();

    Statement::ActionCall {
        location,
        name: name.to_string(),
        arguments,
    }
}

fn build_statement_choice(
    info: &DesignationInfo,
    location: SourceLocation,
    mut branches: Vec<Statement>,
) -> Statement {
    debug_assert_eq!(branches.len(), info.candidates.len());

    if info.allows_none {
        let mut statement = Statement::Fail {
            location,
            message: Expr::String("live designation has no target".to_string()),
        };
        for (root, branch) in info.candidates.iter().zip(branches).rev() {
            statement = Statement::If {
                location,
                condition: selection_condition(info, root),
                then_branch: vec![branch],
                else_branch: Some(vec![statement]),
            };
        }
        return statement;
    }

    let mut statement = branches
        .pop()
        .expect("designation choice requires at least one target");

    for (root, branch) in info.candidates.iter().zip(branches).rev() {
        statement = Statement::If {
            location,
            condition: selection_condition(info, root),
            then_branch: vec![branch],
            else_branch: Some(vec![statement]),
        };
    }
    statement
}

fn selection_condition(info: &DesignationInfo, root: &RootInfo) -> Expr {
    Expr::Binary {
        operator: BinaryOperator::Equal,
        left: Box::new(Expr::Name(info.lowered_name.clone())),
        right: Box::new(Expr::String(root.name.clone())),
    }
}

fn split_exact_member_path<'a>(
    path: &'a str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<(&'a str, &'a str)> {
    let mut parts = path.split('.');
    let root = parts.next()?;
    let member = parts.next()?;
    if parts.next().is_some() {
        errors.push(diag(
            location,
            format!(
                "designation chains are deferred; '{}' has more than one member step",
                path
            ),
        ));
        return None;
    }
    Some((root, member))
}

fn split_designation_path<'a>(
    path: &'a str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> (Option<&'a str>, Option<&'a str>) {
    let mut parts = path.split('.');
    let root = parts.next();
    let member = parts.next();
    if parts.next().is_some() {
        errors.push(diag(
            location,
            format!(
                "designation chains are deferred; '{}' has more than one member step",
                path
            ),
        ));
        return (root, member);
    }
    (root, member)
}

fn decode_designation_state_type(type_name: &str) -> Option<(&str, bool)> {
    if let Some(model) = decode_live_type_name(type_name) {
        return Some((model, false));
    }
    decode_maybe_live_type_name(type_name).map(|model| (model, true))
}

fn lowered_designation_name(source_name: &str) -> String {
    format!("{LOWERED_DESIGNATION_PREFIX}{source_name}")
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}

fn runtime_value_type_name(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Int => "Int".to_string(),
        ValueType::Float => "Float".to_string(),
        ValueType::Bool => "Bool".to_string(),
        ValueType::String => "String".to_string(),
        ValueType::SequenceLive(model) => {
            crate::runtime_sequence_markers::encode_runtime_sequence_type(model)
        }
        ValueType::Named(name) => name.clone(),
    }
}
