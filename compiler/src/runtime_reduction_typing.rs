use std::collections::HashMap;

use crate::ast::{Declaration, Expr, Program, SourceLocation};
use crate::diagnostic::Diagnostic;
use crate::filter_integration::MODEL_FILTER_BINDING_PREFIX;
use crate::model_types::{self, ModelTypeInfo};
use crate::reduction_surface::{decode_reduction, parse_expression_fragment, ReductionSpec};
use crate::semantic::{
    assignable_to, binary_result_type, common_type, show_type, types_compatible, ValueType,
};

const MODEL_SEQUENCE_BINDING_PREFIX: &str = "__meld_mseq$";

/// Type owner-relative runtime reductions without choosing a representative
/// concrete live target.
///
/// The initial expression establishes the stable accumulator/result type. The
/// step is then checked with that accumulator type, the element model schema,
/// and the owner model's ordinary member schema. Each step result must be
/// assignable back to the accumulator type under ordinary Meld rules.
pub(crate) fn check(program: &Program) -> Result<HashMap<String, ValueType>, Vec<Diagnostic>> {
    let model_types = model_types::resolve(&program.state_models)?;
    let owner_roots = collect_owner_roots(program);
    let mut result = HashMap::new();
    let mut errors = Vec::new();

    for declaration in &program.declarations {
        let Declaration::Derived(derived) = declaration else {
            continue;
        };
        let Expr::String(value) = &derived.expression else {
            continue;
        };
        let Some(reduction) = decode_reduction(value) else {
            continue;
        };
        if !is_owner_relative_source(&reduction.source) {
            continue;
        }

        let Some(owner_root) = owner_root(&reduction.source) else {
            errors.push(diag(
                derived.location,
                format!(
                    "internal runtime reduction '{}' has an invalid owner-relative sequence source '{}'",
                    derived.name, reduction.source
                ),
            ));
            continue;
        };
        let Some(owner_model_name) = owner_roots.get(owner_root) else {
            errors.push(diag(
                derived.location,
                format!(
                    "internal runtime reduction '{}' cannot resolve owner root '{}'",
                    derived.name, owner_root
                ),
            ));
            continue;
        };
        let Some(owner_model) = model_types.model(owner_model_name) else {
            errors.push(diag(
                derived.location,
                format!(
                    "internal runtime reduction '{}' cannot resolve owner model '{}'",
                    derived.name, owner_model_name
                ),
            ));
            continue;
        };
        let Some(element_model_name) = reduction.element_model.as_deref() else {
            errors.push(diag(
                derived.location,
                format!(
                    "internal runtime reduction '{}' is missing its live element model",
                    derived.name
                ),
            ));
            continue;
        };
        let Some(element_model) = model_types.model(element_model_name) else {
            errors.push(diag(
                derived.location,
                format!(
                    "runtime reduction '{}' names unavailable element model '{}'",
                    derived.name, element_model_name
                ),
            ));
            continue;
        };

        let initial = match parse_expression_fragment(&reduction.initial_source) {
            Ok(expression) => expression,
            Err(parse_errors) => {
                errors.push(diag(
                    derived.location,
                    format!(
                        "invalid runtime reduction initial expression for '{}': {}",
                        derived.name,
                        first_message(&parse_errors)
                    ),
                ));
                continue;
            }
        };
        let Some(accumulator_type) = infer_reduction_expr(
            &initial,
            &reduction,
            owner_model,
            element_model,
            None,
            derived.location,
            &mut errors,
        ) else {
            continue;
        };

        if matches!(
            accumulator_type,
            ValueType::Named(_) | ValueType::SequenceLive(_)
        ) {
            errors.push(diag(
                derived.location,
                format!(
                    "runtime reduction '{}' has unsupported bootstrap accumulator type {}",
                    derived.name,
                    show_type(&accumulator_type)
                ),
            ));
            continue;
        }

        let step = match parse_expression_fragment(&reduction.step_source) {
            Ok(expression) => expression,
            Err(parse_errors) => {
                errors.push(diag(
                    derived.location,
                    format!(
                        "invalid runtime reduction step expression for '{}': {}",
                        derived.name,
                        first_message(&parse_errors)
                    ),
                ));
                continue;
            }
        };
        let Some(step_type) = infer_reduction_expr(
            &step,
            &reduction,
            owner_model,
            element_model,
            Some(&accumulator_type),
            derived.location,
            &mut errors,
        ) else {
            continue;
        };

        if !assignable_to(&accumulator_type, &step_type) {
            errors.push(diag(
                derived.location,
                format!(
                    "runtime reduction '{}' step has type {} but its accumulator established by the initial expression has type {}",
                    derived.name,
                    show_type(&step_type),
                    show_type(&accumulator_type)
                ),
            ));
            continue;
        }

        result.insert(derived.name.clone(), accumulator_type);
    }

    if errors.is_empty() {
        Ok(result)
    } else {
        Err(errors)
    }
}

fn collect_owner_roots(program: &Program) -> HashMap<String, String> {
    program
        .declarations
        .iter()
        .filter_map(|declaration| {
            let Declaration::State(state) = declaration else {
                return None;
            };
            if !state.implicit_model_initializer {
                return None;
            }
            state
                .type_name
                .as_ref()
                .map(|model_name| (state.name.clone(), model_name.clone()))
        })
        .collect()
}

fn infer_reduction_expr(
    expression: &Expr,
    reduction: &ReductionSpec,
    owner_model: &ModelTypeInfo,
    element_model: &ModelTypeInfo,
    accumulator_type: Option<&ValueType>,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<ValueType> {
    match expression {
        Expr::Integer(_) => Some(ValueType::Int),
        Expr::Float(_) => Some(ValueType::Float),
        Expr::Bool(_) => Some(ValueType::Bool),
        Expr::String(value) => {
            if decode_reduction(value).is_some() {
                errors.push(diag(
                    location,
                    "nested reduction is deferred in the first read-only reduction spike",
                ));
                None
            } else {
                Some(ValueType::String)
            }
        }
        Expr::Name(name) => {
            if name == &reduction.accumulator {
                return match accumulator_type {
                    Some(value_type) => Some(value_type.clone()),
                    None => {
                        errors.push(diag(
                            location,
                            "reduction accumulator is not in scope in the initial expression",
                        ));
                        None
                    }
                };
            }

            if name == &reduction.element {
                errors.push(diag(
                    location,
                    "reading the reduction element as an ordinary whole live designation is deferred; read a member",
                ));
                return None;
            }

            let element_prefix = format!("{}.", reduction.element);
            if let Some(member) = name.strip_prefix(&element_prefix) {
                if accumulator_type.is_none() {
                    errors.push(diag(
                        location,
                        "reduction element is not in scope in the initial expression",
                    ));
                    return None;
                }
                if member.is_empty() || member.contains('.') {
                    errors.push(diag(
                        location,
                        "reduction element read must select exactly one modeled-state member",
                    ));
                    return None;
                }
                return match element_model.member_type(member) {
                    Some(value_type) => Some(value_type.clone()),
                    None => {
                        errors.push(diag(
                            location,
                            format!(
                                "runtime reduction element model has no typeable member '{}'",
                                member
                            ),
                        ));
                        None
                    }
                };
            }

            if name.contains('.') {
                errors.push(diag(
                    location,
                    format!(
                        "runtime reduction expression cannot resolve qualified name '{}'",
                        name
                    ),
                ));
                return None;
            }

            match owner_model.member_type(name) {
                Some(value_type) => Some(value_type.clone()),
                None => {
                    errors.push(diag(
                        location,
                        format!(
                            "runtime reduction expression cannot resolve owner-local member '{}'",
                            name
                        ),
                    ));
                    None
                }
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => {
            let left = infer_reduction_expr(
                left,
                reduction,
                owner_model,
                element_model,
                accumulator_type,
                location,
                errors,
            );
            let right = infer_reduction_expr(
                right,
                reduction,
                owner_model,
                element_model,
                accumulator_type,
                location,
                errors,
            );
            match (left, right) {
                (Some(left), Some(right)) => {
                    let result = binary_result_type(*operator, &left, &right);
                    if result.is_none() {
                        errors.push(diag(
                            location,
                            format!(
                                "operator {:?} is not defined for runtime reduction types {} and {}",
                                operator,
                                show_type(&left),
                                show_type(&right)
                            ),
                        ));
                    }
                    result
                }
                _ => None,
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            let condition_type = infer_reduction_expr(
                condition,
                reduction,
                owner_model,
                element_model,
                accumulator_type,
                location,
                errors,
            );
            if let Some(condition_type) = condition_type {
                if condition_type != ValueType::Bool {
                    errors.push(diag(
                        location,
                        format!(
                            "runtime reduction if condition must be Bool, got {}",
                            show_type(&condition_type)
                        ),
                    ));
                }
            }

            let then_type = infer_reduction_expr(
                then_branch,
                reduction,
                owner_model,
                element_model,
                accumulator_type,
                location,
                errors,
            );
            let else_type = infer_reduction_expr(
                else_branch,
                reduction,
                owner_model,
                element_model,
                accumulator_type,
                location,
                errors,
            );
            match (then_type, else_type) {
                (Some(a), Some(b)) if types_compatible(&a, &b) => Some(common_type(a, b)),
                (Some(a), Some(b)) => {
                    errors.push(diag(
                        location,
                        format!(
                            "runtime reduction if branches have incompatible types {} and {}",
                            show_type(&a),
                            show_type(&b)
                        ),
                    ));
                    None
                }
                _ => None,
            }
        }
        Expr::Filter { .. } => {
            errors.push(diag(
                location,
                "filter expressions are not available inside reduction fragments",
            ));
            None
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
            None
        }
    }
}

fn is_owner_relative_source(source: &str) -> bool {
    source.starts_with(MODEL_SEQUENCE_BINDING_PREFIX)
        || source.starts_with(MODEL_FILTER_BINDING_PREFIX)
}

fn owner_root(source: &str) -> Option<&str> {
    let rest = source
        .strip_prefix(MODEL_SEQUENCE_BINDING_PREFIX)
        .or_else(|| source.strip_prefix(MODEL_FILTER_BINDING_PREFIX))?;
    let (root, member) = rest.split_once('$')?;
    if root.is_empty() || member.is_empty() || member.contains('$') {
        return None;
    }
    Some(root)
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
