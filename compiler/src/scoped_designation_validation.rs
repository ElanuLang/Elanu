use crate::ast::{
    ActionArgument, BinaryOperator, Declaration, Expr, Program, SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::scoped_designation_surface::decode_scope_identity_param;

/// Enforce source-level restrictions that remain visible after scoped-designation
/// surface lowering.
///
/// A scoped `with` binding is intentionally accepted as a designation operand for
/// navigation, structural removal/movement, member access, and explicit authority
/// grants. The membership predicate is narrower language law: its left operand is
/// a persistent live designation. Scope lowering rewrites a scoped binding to a
/// private identity parameter, so validate that distinction before later generic
/// live-designation lowering can accidentally treat the parameter as persistent.
pub fn validate(program: &Program) -> Result<(), Vec<Diagnostic>> {
    let mut errors = Vec::new();

    for declaration in &program.declarations {
        let Declaration::Action(action) = declaration else {
            continue;
        };
        validate_statements(&action.statements, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_statements(statements: &[Statement], errors: &mut Vec<Diagnostic>) {
    for statement in statements {
        let location = statement.location();
        match statement {
            Statement::Assignment { value, .. } => validate_expr(value, location, errors),
            Statement::IndexedThroughAssignment { index, value, .. } => {
                validate_expr(index, location, errors);
                validate_expr(value, location, errors);
            }
            Statement::RuntimeIndexAssignment { index, value, .. } => {
                validate_expr(index, location, errors);
                validate_expr(value, location, errors);
            }
            Statement::RuntimeDesignationAssignment {
                designation, value, ..
            } => {
                validate_expr(designation, location, errors);
                validate_expr(value, location, errors);
            }
            Statement::ActionCall { arguments, .. } => {
                for argument in arguments {
                    match argument {
                        ActionArgument::Value(expr) => validate_expr(expr, location, errors),
                        ActionArgument::IndexedStateGrant { index, .. } => {
                            validate_expr(index, location, errors)
                        }
                        ActionArgument::StateGrant { .. } => {}
                    }
                }
            }
            Statement::Fail { message, .. } => validate_expr(message, location, errors),
            Statement::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                validate_expr(condition, location, errors);
                validate_statements(then_branch, errors);
                if let Some(else_branch) = else_branch {
                    validate_statements(else_branch, errors);
                }
            }
        }
    }
}

fn validate_expr(expression: &Expr, location: SourceLocation, errors: &mut Vec<Diagnostic>) {
    match expression {
        Expr::Binary {
            operator: BinaryOperator::IsIn,
            left,
            right,
        } => {
            if matches!(left.as_ref(), Expr::Name(name) if decode_scope_identity_param(name).is_some())
            {
                errors.push(Diagnostic::new(
                    "left operand of 'is in' must be a persistent live designation",
                    location.line,
                    location.column,
                ));
            }
            validate_expr(left, location, errors);
            validate_expr(right, location, errors);
        }
        Expr::Binary { left, right, .. } => {
            validate_expr(left, location, errors);
            validate_expr(right, location, errors);
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            validate_expr(condition, location, errors);
            validate_expr(then_branch, location, errors);
            validate_expr(else_branch, location, errors);
        }
        Expr::Filter {
            source,
            predicate,
            order_by,
            ..
        } => {
            validate_expr(source, location, errors);
            validate_expr(predicate, location, errors);
            if let Some(order_by) = order_by {
                validate_expr(order_by, location, errors);
            }
        }
        Expr::IndexedMember { index, .. } | Expr::IndexedDesignation { index, .. } => {
            validate_expr(index, location, errors);
        }
        Expr::RuntimeIndexMember { source, index, .. }
        | Expr::RuntimeIndexDesignation { source, index, .. } => {
            validate_expr(source, location, errors);
            validate_expr(index, location, errors);
        }
        Expr::RuntimeDesignationMember { designation, .. } => {
            validate_expr(designation, location, errors);
        }
        Expr::Integer(_) | Expr::Float(_) | Expr::Bool(_) | Expr::String(_) | Expr::Name(_) => {}
    }
}
