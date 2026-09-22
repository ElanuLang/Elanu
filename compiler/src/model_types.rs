use std::collections::HashMap;

use crate::ast::{Expr, SourceLocation, StateModelDecl, StateModelMember};
use crate::diagnostic::Diagnostic;
use crate::semantic::{
    assignable_to, binary_result_type, common_type, parse_primitive_type_name, show_type,
    types_compatible, ValueType,
};
use crate::sequence_surface::{decode_sequence_literal, decode_sequence_live_type};

#[derive(Debug, Clone)]
pub(crate) struct ModelTypeInfo {
    member_types: HashMap<String, ValueType>,
}

impl ModelTypeInfo {
    pub(crate) fn member_type(&self, name: &str) -> Option<&ValueType> {
        self.member_types.get(name)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ModelTypeFacts {
    models: HashMap<String, ModelTypeInfo>,
}

impl ModelTypeFacts {
    pub(crate) fn model(&self, name: &str) -> Option<&ModelTypeInfo> {
        self.models.get(name)
    }
}

/// Resolve state-model member value types once in the semantic `ValueType`
/// vocabulary.
///
/// This remains a narrow shared analysis rather than a second general checker.
/// In addition to the original primitive model members, it now carries the
/// established `[live T]` structural sequence type and the shape-level result
/// type of the provisional filter view. Filter predicate validation remains in
/// the dedicated filter integration pass, where the element binding has its
/// model schema.
pub(crate) fn resolve(declarations: &[StateModelDecl]) -> Result<ModelTypeFacts, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut models = HashMap::new();

    for model in declarations {
        if parse_primitive_type_name(&model.name).is_some() {
            errors.push(diag(
                model.location,
                format!(
                    "state model name '{}' conflicts with a built-in value type",
                    model.name
                ),
            ));
            continue;
        }

        if models.contains_key(&model.name) {
            errors.push(diag(
                model.location,
                format!("duplicate state model '{}'", model.name),
            ));
            continue;
        }

        let info = resolve_model(model, &mut errors);
        models.insert(model.name.clone(), info);
    }

    if errors.is_empty() {
        Ok(ModelTypeFacts { models })
    } else {
        Err(errors)
    }
}

fn resolve_model(model: &StateModelDecl, errors: &mut Vec<Diagnostic>) -> ModelTypeInfo {
    let mut member_types = HashMap::new();

    for member in &model.members {
        let name = member.name().to_string();
        let location = member.location();

        if member_types.contains_key(&name) {
            errors.push(diag(
                location,
                format!(
                    "duplicate member '{}' in state model '{}'",
                    name, model.name
                ),
            ));
            continue;
        }

        let (expression, declared_type) = match member {
            StateModelMember::State(state) => {
                let declared_type = state.type_name.as_deref().and_then(|type_name| {
                    if let Some(parsed) = parse_primitive_type_name(type_name) {
                        return Some(parsed);
                    }
                    if let Some(element_model) = decode_sequence_live_type(type_name) {
                        return Some(ValueType::SequenceLive(element_model.to_string()));
                    }
                    errors.push(diag(
                        state.location,
                        format!(
                            "state-model member '{}.{}' has unsupported bootstrap type '{}'",
                            model.name, state.name, type_name
                        ),
                    ));
                    None
                });
                (&state.initializer, declared_type)
            }
            StateModelMember::Derived(derived) => (&derived.expression, None),
        };

        let inferred = match (&declared_type, expression) {
            (Some(ValueType::SequenceLive(_)), Expr::String(value))
                if decode_sequence_literal(value).is_some() =>
            {
                declared_type.clone()
            }
            _ => infer_model_expr_type(expression, &member_types, model, location, errors),
        };

        let value_type = match (declared_type, inferred) {
            (Some(declared), Some(inferred)) => {
                if !assignable_to(&declared, &inferred) {
                    errors.push(diag(
                        location,
                        format!(
                            "state-model member '{}.{}' is declared as {} but initialized with {}",
                            model.name,
                            name,
                            show_type(&declared),
                            show_type(&inferred)
                        ),
                    ));
                }
                declared
            }
            (Some(declared), None) => declared,
            (None, Some(inferred)) => inferred,
            (None, None) => ValueType::Int,
        };

        member_types.insert(name, value_type);
    }

    ModelTypeInfo { member_types }
}

fn infer_model_expr_type(
    expression: &Expr,
    scope: &HashMap<String, ValueType>,
    model: &StateModelDecl,
    location: SourceLocation,
    errors: &mut Vec<Diagnostic>,
) -> Option<ValueType> {
    match expression {
        Expr::Integer(_) => Some(ValueType::Int),
        Expr::Float(_) => Some(ValueType::Float),
        Expr::Bool(_) => Some(ValueType::Bool),
        Expr::String(_) => Some(ValueType::String),
        Expr::Name(name) => {
            if name.contains('.') {
                errors.push(diag(
                    location,
                    format!(
                        "state-model member expressions are local to '{}'; qualified/external name '{}' is not allowed",
                        model.name, name
                    ),
                ));
                return None;
            }

            match scope.get(name) {
                Some(value_type) => Some(value_type.clone()),
                None => {
                    errors.push(diag(
                        location,
                        format!(
                            "state-model member in '{}' may only reference earlier local state/derived members; unknown local name '{}'",
                            model.name, name
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
            let left = infer_model_expr_type(left, scope, model, location, errors);
            let right = infer_model_expr_type(right, scope, model, location, errors);
            match (left, right) {
                (Some(left), Some(right)) => {
                    let result = binary_result_type(*operator, &left, &right);
                    if result.is_none() {
                        errors.push(diag(
                            location,
                            format!(
                                "operator {:?} is not defined for state-model member types {} and {}",
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
            let condition_type = infer_model_expr_type(condition, scope, model, location, errors);
            if let Some(condition_type) = condition_type {
                if condition_type != ValueType::Bool {
                    errors.push(diag(
                        location,
                        format!(
                            "state-model if condition must be Bool, got {}",
                            show_type(&condition_type)
                        ),
                    ));
                }
            }

            let then_type = infer_model_expr_type(then_branch, scope, model, location, errors);
            let else_type = infer_model_expr_type(else_branch, scope, model, location, errors);
            match (then_type, else_type) {
                (Some(a), Some(b)) if types_compatible(&a, &b) => Some(common_type(a, b)),
                (Some(a), Some(b)) => {
                    errors.push(diag(
                        location,
                        format!(
                            "state-model if branches have incompatible types {} and {}",
                            show_type(&a),
                            show_type(&b)
                        ),
                    ));
                    None
                }
                _ => None,
            }
        }
        Expr::IndexedDesignation { index, .. } => {
            if let Some(index_type) = infer_model_expr_type(index, scope, model, location, errors) {
                if index_type != ValueType::Int {
                    errors.push(diag(
                        location,
                        format!(
                            "sequence index expression must be Int, got {}",
                            show_type(&index_type)
                        ),
                    ));
                }
            }
            errors.push(diag(
                location,
                "whole live designation indexing inside a state-model member expression is deferred",
            ));
            None
        }
        Expr::IndexedMember { index, .. } => {
            if let Some(index_type) = infer_model_expr_type(index, scope, model, location, errors) {
                if index_type != ValueType::Int {
                    errors.push(diag(
                        location,
                        format!(
                            "sequence index expression must be Int, got {}",
                            show_type(&index_type)
                        ),
                    ));
                }
            }
            errors.push(diag(
                location,
                "runtime indexed member access inside a state-model member expression is deferred",
            ));
            None
        }
        Expr::RuntimeIndexMember { .. }
        | Expr::RuntimeIndexDesignation { .. }
        | Expr::RuntimeDesignationMember { .. } => {
            unreachable!("runtime index lowering runs after model type resolution")
        }
        Expr::Filter { source, .. } => {
            let source_type = infer_model_expr_type(source, scope, model, location, errors);
            match source_type {
                Some(ValueType::SequenceLive(element_model)) => {
                    Some(ValueType::SequenceLive(element_model))
                }
                Some(other) => {
                    errors.push(diag(
                        location,
                        format!("filter source must be [live T], got {}", show_type(&other)),
                    ));
                    None
                }
                None => None,
            }
        }
    }
}

fn diag(location: SourceLocation, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(message, location.line, location.column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_source;

    #[test]
    fn resolves_model_members_in_semantic_value_type_vocabulary() {
        let program = parse_source(
            r#"
state model LineItem {
    state quantity = 1
    state unitPrice: Float = 2
    derived lineTotal = quantity * unitPrice
    derived displayTotal = if quantity > 0 { lineTotal } else { 0 }
}
"#,
        )
        .expect("source should parse");

        let facts = resolve(&program.state_models).expect("model types should resolve");
        let line_item = facts.model("LineItem").expect("LineItem should exist");

        assert_eq!(line_item.member_type("quantity"), Some(&ValueType::Int));
        assert_eq!(line_item.member_type("unitPrice"), Some(&ValueType::Float));
        assert_eq!(line_item.member_type("lineTotal"), Some(&ValueType::Float));
        assert_eq!(
            line_item.member_type("displayTotal"),
            Some(&ValueType::Float)
        );
    }

    #[test]
    fn carries_sequence_and_filter_result_types() {
        let program = parse_source(
            r#"
state model LineItem {
    state quantity = 1
}

state model Invoice {
    state lines: [live LineItem] = []
    derived activeLines = filter lines as line { line.quantity > 0 }
}
"#,
        )
        .expect("source should parse");

        let facts = resolve(&program.state_models).expect("model types should resolve");
        let invoice = facts.model("Invoice").expect("Invoice should exist");
        assert_eq!(
            invoice.member_type("lines"),
            Some(&ValueType::SequenceLive("LineItem".to_string()))
        );
        assert_eq!(
            invoice.member_type("activeLines"),
            Some(&ValueType::SequenceLive("LineItem".to_string()))
        );
    }

    #[test]
    fn preserves_float_to_int_member_initialization_rejection() {
        let program = parse_source(
            r#"
state model Sample {
    state count: Int = 1.5
}
"#,
        )
        .expect("source should parse");

        let errors = resolve(&program.state_models).expect_err("model typing should fail");
        assert!(errors.iter().any(|error| {
            error.message
                == "state-model member 'Sample.count' is declared as Int but initialized with Float"
        }));
    }
}
