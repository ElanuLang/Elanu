use std::collections::{HashMap, HashSet};

use crate::ast::{
    ActionArgument, ActionDecl, BinaryOperator, Declaration, DerivedDecl, Expr, Program,
    SourceLocation, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::MODEL_FILTER_BINDING_PREFIX;
use crate::reduction_surface::{decode_reduction, parse_expression_fragment, ReductionSpec};

const LOWERED_SEQUENCE_VALUE_PREFIX: &str = "__elanu_sequence_value$";
const MODEL_SEQUENCE_BINDING_PREFIX: &str = "__elanu_mseq$";
const MODEL_BINDING_PREFIX: &str = "__elanu_sm$";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SequenceVariant {
    targets: Vec<String>,
    encoded: String,
}

#[derive(Debug, Clone)]
struct SequenceInfo {
    declaration_index: usize,
    variants: Vec<SequenceVariant>,
}

struct LoweringContext<'a> {
    declaration_index: usize,
    sequences: &'a HashMap<String, SequenceInfo>,
    model_members: &'a HashMap<String, HashSet<String>>,
    preserve_model_sequence_reductions: bool,
}

/// Lower the provisional read-only reduction surface after ordered-sequence
/// lowering has converted sequence state to its private scalar representation.
///
/// The bootstrap strategy enumerates the statically known sequence variants and
/// unrolls the pure step expression for each one. Only the active conditional
/// branch evaluates at runtime, so the existing dynamic dependency tracker sees
/// the sequence state plus exactly the child facts read by the current variant.
pub fn lower(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    lower_with_mode(program, false)
}

/// Preserve owner-relative model-sequence reductions for the runtime-sized
/// sequence realization spike while lowering every other reduction normally.
pub fn lower_preserving_model_sequences(program: &Program) -> Result<Program, Vec<Diagnostic>> {
    lower_with_mode(program, true)
}

fn lower_with_mode(
    program: &Program,
    preserve_model_sequence_reductions: bool,
) -> Result<Program, Vec<Diagnostic>> {
    let sequences = collect_sequences(program);
    let model_members = collect_model_members(program);
    let mut errors = Vec::new();
    reject_unlowered_model_reductions(program, &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }

    let mut declarations = Vec::new();
    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let context = LoweringContext {
            declaration_index,
            sequences: &sequences,
            model_members: &model_members,
            preserve_model_sequence_reductions,
        };
        declarations.push(lower_declaration(declaration, &context, &mut errors));
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

fn collect_sequences(program: &Program) -> HashMap<String, SequenceInfo> {
    let mut result = HashMap::new();

    for (declaration_index, declaration) in program.declarations.iter().enumerate() {
        let Declaration::State(state) = declaration else {
            continue;
        };
        let Expr::String(value) = &state.initializer else {
            continue;
        };
        let Some(targets) = decode_sequence_value(value) else {
            continue;
        };

        result.insert(
            state.name.clone(),
            SequenceInfo {
                declaration_index,
                variants: vec![SequenceVariant {
                    targets,
                    encoded: value.clone(),
                }],
            },
        );
    }

    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        collect_assignment_variants(&action.statements, &mut result);
    }

    result
}

fn collect_assignment_variants(
    statements: &[Statement],
    sequences: &mut HashMap<String, SequenceInfo>,
) {
    for statement in statements {
        match statement {
            Statement::Assignment { target, value, .. } => {
                let Some(info) = sequences.get_mut(target) else {
                    continue;
                };
                let Expr::String(encoded) = value else {
                    continue;
                };
                let Some(targets) = decode_sequence_value(encoded) else {
                    continue;
                };
                if !info
                    .variants
                    .iter()
                    .any(|variant| variant.encoded == *encoded)
                {
                    info.variants.push(SequenceVariant {
                        targets,
                        encoded: encoded.clone(),
                    });
                }
            }
            Statement::If {
                then_branch,
                else_branch,
                ..
            } => {
                collect_assignment_variants(then_branch, sequences);
                if let Some(else_branch) = else_branch {
                    collect_assignment_variants(else_branch, sequences);
                }
            }
            _ => {}
        }
    }
}

fn collect_model_members(program: &Program) -> HashMap<String, HashSet<String>> {
    program
        .state_models
        .iter()
        .map(|model| {
            (
                model.name.clone(),
                model
                    .members
                    .iter()
                    .map(|member| member.name().to_string())
                    .collect(),
            )
        })
        .collect()
}

fn reject_unlowered_model_reductions(program: &Program, errors: &mut Vec<Diagnostic>) {
    for model in &program.state_models {
        for member in &model.members {
            let expression = match member {
                StateModelMember::State(state) => &state.initializer,
                StateModelMember::Derived(derived) => &derived.expression,
            };
            if expr_contains_reduction(expression) {
                errors.push(diag(
                    member.location(),
                    format!(
                        "bootstrap read-only reduction in state model '{}' must be the complete expression of a derived member",
                        model.name
                    ),
                ));
            }
        }
    }
}

fn expr_contains_reduction(expression: &Expr) -> bool {
    match expression {
        Expr::String(value) => decode_reduction(value).is_some(),
        Expr::Binary { left, right, .. } => {
            expr_contains_reduction(left) || expr_contains_reduction(right)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expr_contains_reduction(condition)
                || expr_contains_reduction(then_branch)
                || expr_contains_reduction(else_branch)
        }
        Expr::Filter {
            source, predicate, ..
        } => expr_contains_reduction(source) || expr_contains_reduction(predicate),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => false,
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::Name(_) => false,
    }
}

fn lower_declaration(
    declaration: &Declaration,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Declaration {
    match declaration {
        Declaration::State(state) => {
            let mut state = state.clone();
            state.initializer = lower_expr(&state.initializer, state.location, context, errors);
            Declaration::State(state)
        }
        Declaration::Derived(derived) => Declaration::Derived(DerivedDecl {
            location: derived.location,
            name: derived.name.clone(),
            expression: lower_expr(&derived.expression, derived.location, context, errors),
        }),
        Declaration::Action(action) => Declaration::Action(ActionDecl {
            location: action.location,
            name: action.name.clone(),
            parameters: action.parameters.clone(),
            statements: action
                .statements
                .iter()
                .map(|statement| lower_statement(statement, context, errors))
                .collect(),
        }),
    }
}

fn lower_statement(
    statement: &Statement,
    context: &LoweringContext<'_>,
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
            target: target.clone(),
            operator: *operator,
            value: lower_expr(value, *location, context, errors),
        },
        Statement::IndexedThroughAssignment { .. } => {
            unreachable!("source indexed assignments are consumed by sequence lowering")
        }
        Statement::RuntimeDesignationAssignment { .. } => {
            unreachable!("runtime designation lowering runs after reduction lowering")
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
            value: lower_expr(value, *location, context, errors),
        },
        Statement::ActionCall {
            location,
            name,
            arguments,
        } => Statement::ActionCall {
            location: *location,
            name: name.clone(),
            arguments: arguments
                .iter()
                .map(|argument| match argument {
                    ActionArgument::Value(expression) => {
                        ActionArgument::Value(lower_expr(expression, *location, context, errors))
                    }
                    ActionArgument::IndexedStateGrant { .. } => unreachable!(
                        "source indexed state grants are consumed by runtime index grant transport"
                    ),
                    ActionArgument::StateGrant { location, name } => ActionArgument::StateGrant {
                        location: *location,
                        name: name.clone(),
                    },
                })
                .collect(),
        },
        Statement::Fail { location, message } => Statement::Fail {
            location: *location,
            message: lower_expr(message, *location, context, errors),
        },
        Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        } => Statement::If {
            location: *location,
            condition: lower_expr(condition, *location, context, errors),
            then_branch: then_branch
                .iter()
                .map(|statement| lower_statement(statement, context, errors))
                .collect(),
            else_branch: else_branch.as_ref().map(|branch| {
                branch
                    .iter()
                    .map(|statement| lower_statement(statement, context, errors))
                    .collect()
            }),
        },
    }
}

fn lower_expr(
    expression: &Expr,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::Name(name) => Expr::Name(name.clone()),
        Expr::String(value) => {
            let Some(reduction) = decode_reduction(value) else {
                return Expr::String(value.clone());
            };
            if context.preserve_model_sequence_reductions
                && (reduction.source.starts_with(MODEL_SEQUENCE_BINDING_PREFIX)
                    || reduction.source.starts_with(MODEL_FILTER_BINDING_PREFIX))
            {
                return Expr::String(value.clone());
            }
            lower_reduction(&reduction, location, context, errors)
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(lower_expr(left, location, context, errors)),
            right: Box::new(lower_expr(right, location, context, errors)),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(lower_expr(condition, location, context, errors)),
            then_branch: Box::new(lower_expr(then_branch, location, context, errors)),
            else_branch: Box::new(lower_expr(else_branch, location, context, errors)),
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

fn lower_reduction(
    reduction: &ReductionSpec,
    location: SourceLocation,
    context: &LoweringContext<'_>,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    let Some(info) = context.sequences.get(&reduction.source) else {
        errors.push(diag(
            location,
            format!(
                "reduction source '{}' is not a lowered ordered-sequence state",
                reduction.source
            ),
        ));
        return Expr::Integer(0);
    };

    if info.declaration_index >= context.declaration_index {
        errors.push(diag(
            location,
            format!(
                "reduction source '{}' is not available before its declaration",
                reduction.source
            ),
        ));
    }

    let Some(element_model) = reduction.element_model.as_deref() else {
        errors.push(diag(
            location,
            "internal reduction marker is missing its [live T] element model",
        ));
        return Expr::Integer(0);
    };
    let Some(element_members) = context.model_members.get(element_model) else {
        errors.push(diag(
            location,
            format!("reduction names unknown state model '{element_model}'"),
        ));
        return Expr::Integer(0);
    };
    let owner_root = model_sequence_owner_root(&reduction.source);

    let initial = match parse_expression_fragment(&reduction.initial_source) {
        Ok(expression) => {
            let expression = rewrite_owner_local_names(&expression, reduction, owner_root, false);
            reject_sequence_projection(
                &expression,
                reduction,
                context,
                location,
                "initial",
                false,
                errors,
            );
            lower_expr(&expression, location, context, errors)
        }
        Err(parse_errors) => {
            errors.push(diag(
                location,
                format!(
                    "invalid reduction initial expression: {}",
                    first_message(&parse_errors)
                ),
            ));
            Expr::Integer(0)
        }
    };
    let step = match parse_expression_fragment(&reduction.step_source) {
        Ok(expression) => rewrite_owner_local_names(&expression, reduction, owner_root, true),
        Err(parse_errors) => {
            errors.push(diag(
                location,
                format!(
                    "invalid reduction step expression: {}",
                    first_message(&parse_errors)
                ),
            ));
            return initial;
        }
    };

    reject_sequence_projection(&step, reduction, context, location, "step", true, errors);
    validate_step(
        &step,
        &reduction.accumulator,
        &reduction.element,
        element_model,
        element_members,
        location,
        errors,
    );

    let mut branches = Vec::new();
    for variant in &info.variants {
        let mut accumulator = initial.clone();
        for target in &variant.targets {
            let substituted = substitute_step(
                &step,
                &reduction.accumulator,
                &reduction.element,
                &accumulator,
                target,
                location,
                errors,
            );
            accumulator = lower_expr(&substituted, location, context, errors);
        }
        branches.push((variant, accumulator));
    }

    build_variant_choice(&reduction.source, branches)
}

#[allow(clippy::too_many_arguments)]
fn reject_sequence_projection(
    expression: &Expr,
    reduction: &ReductionSpec,
    context: &LoweringContext<'_>,
    location: SourceLocation,
    part: &str,
    bindings_in_scope: bool,
    errors: &mut Vec<Diagnostic>,
) {
    match expression {
        Expr::Name(name) => {
            if bindings_in_scope {
                let element_prefix = format!("{}.", reduction.element);
                if name == &reduction.accumulator
                    || name == &reduction.element
                    || name.starts_with(&element_prefix)
                {
                    return;
                }
            }

            let base = name.split('.').next().unwrap_or(name);
            if context.sequences.contains_key(base) {
                errors.push(diag(
                    location,
                    format!(
                        "reduction {part} expression cannot project ordered sequence '{base}' as an ordinary value; sequence structure remains private to traversal"
                    ),
                ));
            }
        }
        Expr::Binary { left, right, .. } => {
            reject_sequence_projection(
                left,
                reduction,
                context,
                location,
                part,
                bindings_in_scope,
                errors,
            );
            reject_sequence_projection(
                right,
                reduction,
                context,
                location,
                part,
                bindings_in_scope,
                errors,
            );
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            for branch in [
                condition.as_ref(),
                then_branch.as_ref(),
                else_branch.as_ref(),
            ] {
                reject_sequence_projection(
                    branch,
                    reduction,
                    context,
                    location,
                    part,
                    bindings_in_scope,
                    errors,
                );
            }
        }
        Expr::Filter { .. } => errors.push(diag(
            location,
            "nested filter is not available inside a reduction expression",
        )),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => errors.push(diag(
            location,
            "runtime indexed access is not available inside reduction fragments",
        )),
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) => {}
    }
}

fn validate_step(
    expression: &Expr,
    accumulator: &str,
    element: &str,
    element_model: &str,
    element_members: &HashSet<String>,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) {
    match expression {
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) => {}
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is deferred in the first read-only reduction spike",
                ));
            }
        }
        Expr::Name(name) if name == accumulator => {}
        Expr::Name(name) if name == element => errors.push(diag(
            location,
            "reading the reduction element as an ordinary whole live designation is deferred; read a member",
        )),
        Expr::Name(name) => {
            let prefix = format!("{element}.");
            if let Some(member) = name.strip_prefix(&prefix) {
                if member.is_empty() || member.contains('.') {
                    errors.push(diag(
                        location,
                        "reduction element read must select exactly one modeled-state member in the first spike",
                    ));
                } else if !element_members.contains(member) {
                    errors.push(diag(
                        location,
                        format!("state model '{element_model}' has no member '{member}'"),
                    ));
                }
            }
        }
        Expr::Binary { left, right, .. } => {
            validate_step(
                left,
                accumulator,
                element,
                element_model,
                element_members,
                location,
                errors,
            );
            validate_step(
                right,
                accumulator,
                element,
                element_model,
                element_members,
                location,
                errors,
            );
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            for branch in [condition.as_ref(), then_branch.as_ref(), else_branch.as_ref()] {
                validate_step(
                    branch,
                    accumulator,
                    element,
                    element_model,
                    element_members,
                    location,
                    errors,
                );
            }
        }
        Expr::Filter { .. } => errors.push(diag(
            location,
            "nested filter is not available inside a reduction expression",
        )),
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => unreachable!("source indexed expressions are consumed by sequence lowering"),
        Expr::RuntimeIndexMember { .. } | Expr::RuntimeIndexDesignation { .. } | Expr::RuntimeDesignationMember { .. } => errors.push(diag(
            location,
            "runtime indexed access is not available inside reduction fragments",
        )),
    }
}

fn substitute_step(
    expression: &Expr,
    accumulator_name: &str,
    element_name: &str,
    accumulator: &Expr,
    target: &str,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is deferred in the first read-only reduction spike",
                ));
            }
            Expr::String(value.clone())
        }
        Expr::Name(name) if name == accumulator_name => accumulator.clone(),
        Expr::Name(name) if name == element_name => {
            errors.push(diag(
                location,
                "reduction element designation cannot become the accumulator value in this spike",
            ));
            Expr::Integer(0)
        }
        Expr::Name(name) => {
            let prefix = format!("{element_name}.");
            if let Some(member) = name.strip_prefix(&prefix) {
                Expr::Name(format!("{target}.{member}"))
            } else {
                Expr::Name(name.clone())
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(substitute_step(
                left,
                accumulator_name,
                element_name,
                accumulator,
                target,
                location,
                errors,
            )),
            right: Box::new(substitute_step(
                right,
                accumulator_name,
                element_name,
                accumulator,
                target,
                location,
                errors,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(substitute_step(
                condition,
                accumulator_name,
                element_name,
                accumulator,
                target,
                location,
                errors,
            )),
            then_branch: Box::new(substitute_step(
                then_branch,
                accumulator_name,
                element_name,
                accumulator,
                target,
                location,
                errors,
            )),
            else_branch: Box::new(substitute_step(
                else_branch,
                accumulator_name,
                element_name,
                accumulator,
                target,
                location,
                errors,
            )),
        },
        Expr::Filter { .. } => {
            errors.push(diag(
                location,
                "nested filter is not available inside a reduction expression",
            ));
            expression.clone()
        }
        Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
            unreachable!("source indexed expressions are consumed by sequence lowering")
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            errors.push(diag(
                location,
                "runtime indexed access is not available inside reduction fragments",
            ));
            expression.clone()
        }
    }
}

fn rewrite_owner_local_names(
    expression: &Expr,
    reduction: &ReductionSpec,
    owner_root: Option<&str>,
    bindings_in_scope: bool,
) -> Expr {
    match expression {
        Expr::Integer(value) => Expr::Integer(*value),
        Expr::Float(value) => Expr::Float(*value),
        Expr::Bool(value) => Expr::Bool(*value),
        Expr::String(value) => Expr::String(value.clone()),
        Expr::Name(name) => {
            if bindings_in_scope {
                let element_prefix = format!("{}.", reduction.element);
                if name == &reduction.accumulator
                    || name == &reduction.element
                    || name.starts_with(&element_prefix)
                {
                    return Expr::Name(name.clone());
                }
            }

            match owner_root {
                Some(root) if !name.contains('.') => Expr::Name(model_binding_name(root, name)),
                _ => Expr::Name(name.clone()),
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => Expr::Binary {
            operator: *operator,
            left: Box::new(rewrite_owner_local_names(
                left,
                reduction,
                owner_root,
                bindings_in_scope,
            )),
            right: Box::new(rewrite_owner_local_names(
                right,
                reduction,
                owner_root,
                bindings_in_scope,
            )),
        },
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => Expr::If {
            condition: Box::new(rewrite_owner_local_names(
                condition,
                reduction,
                owner_root,
                bindings_in_scope,
            )),
            then_branch: Box::new(rewrite_owner_local_names(
                then_branch,
                reduction,
                owner_root,
                bindings_in_scope,
            )),
            else_branch: Box::new(rewrite_owner_local_names(
                else_branch,
                reduction,
                owner_root,
                bindings_in_scope,
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

fn model_sequence_owner_root(source: &str) -> Option<&str> {
    let rest = source.strip_prefix(MODEL_SEQUENCE_BINDING_PREFIX)?;
    let (root, member) = rest.split_once('$')?;
    if root.is_empty() || member.is_empty() || member.contains('$') {
        return None;
    }
    Some(root)
}

fn model_binding_name(root: &str, member: &str) -> String {
    format!("{MODEL_BINDING_PREFIX}{root}${member}")
}

fn build_variant_choice(source: &str, mut branches: Vec<(&SequenceVariant, Expr)>) -> Expr {
    let Some((_, mut expression)) = branches.pop() else {
        return Expr::Integer(0);
    };

    for (variant, branch) in branches.into_iter().rev() {
        expression = Expr::If {
            condition: Box::new(Expr::Binary {
                operator: BinaryOperator::Equal,
                left: Box::new(Expr::Name(source.to_string())),
                right: Box::new(Expr::String(variant.encoded.clone())),
            }),
            then_branch: Box::new(branch),
            else_branch: Box::new(expression),
        };
    }

    expression
}

fn decode_sequence_value(value: &str) -> Option<Vec<String>> {
    let rest = value.strip_prefix(LOWERED_SEQUENCE_VALUE_PREFIX)?;
    if rest.is_empty() {
        return Some(Vec::new());
    }
    Some(rest.split('|').map(str::to_string).collect())
}

fn first_message(errors: &[Diagnostic]) -> String {
    errors
        .first()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| "invalid expression".to_string())
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}
