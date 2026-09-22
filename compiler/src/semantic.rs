use std::collections::{HashMap, HashSet};

use crate::ast::{
    ActionArgument, ActionParameter, ActionParameterKind, AssignmentOperator, BinaryOperator,
    Declaration, Expr, Program, SourceLocation, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::reduction_surface::ReductionSpec;
use crate::runtime_sequence_markers::{
    decode_runtime_sequence_type, decode_runtime_sequence_value,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    Int,
    Float,
    Bool,
    String,
    SequenceLive(String),
    Named(String),
}

#[derive(Debug, Clone)]
pub struct RuntimeReductionPayload {
    pub reduction: ReductionSpec,
    pub result_type: ValueType,
    pub initial: Expr,
    pub step: Expr,
}

#[derive(Debug, Clone)]
pub struct CheckedProgram {
    pub program: Program,
    binding_types: HashMap<String, ValueType>,
    action_parameter_types: HashMap<String, Vec<ValueType>>,
    runtime_reductions: HashMap<String, RuntimeReductionPayload>,
}

impl CheckedProgram {
    pub fn binding_type(&self, name: &str) -> Option<&ValueType> {
        self.binding_types.get(name)
    }

    pub fn action_parameter_types(&self, name: &str) -> Option<&[ValueType]> {
        self.action_parameter_types.get(name).map(Vec::as_slice)
    }

    pub fn runtime_reduction(&self, name: &str) -> Option<&RuntimeReductionPayload> {
        self.runtime_reductions.get(name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKind {
    State,
    Derived,
    Action,
}

#[derive(Debug, Clone)]
struct Symbol {
    kind: SymbolKind,
    value_type: Option<ValueType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalKind {
    ValueParameter,
    StateParameter,
}

#[derive(Debug, Clone)]
struct LocalSymbol {
    kind: LocalKind,
    value_type: ValueType,
}

pub fn check(program: &Program) -> Result<CheckedProgram, Vec<Diagnostic>> {
    check_with_runtime_reductions(program, HashMap::new())
}

pub fn check_with_runtime_reductions(
    program: &Program,
    runtime_reductions: HashMap<String, RuntimeReductionPayload>,
) -> Result<CheckedProgram, Vec<Diagnostic>> {
    let action_names = program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Action(action) => Some(action.name.clone()),
            _ => None,
        })
        .collect();

    let action_signatures: HashMap<String, Vec<ActionParameter>> = program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Action(action) => Some((action.name.clone(), action.parameters.clone())),
            _ => None,
        })
        .collect();

    let runtime_reduction_types = runtime_reductions
        .iter()
        .map(|(name, payload)| (name.clone(), payload.result_type.clone()))
        .collect();

    let mut checker = Checker {
        symbols: HashMap::new(),
        action_names,
        action_signatures,
        parameters: HashMap::new(),
        runtime_reduction_types,
        errors: Vec::new(),
        current_location: SourceLocation::new(1, 1),
    };
    checker.check_program(program);
    if checker.errors.is_empty() {
        let action_parameter_types = checker
            .action_signatures
            .iter()
            .map(|(name, parameters)| {
                (
                    name.clone(),
                    parameters
                        .iter()
                        .map(|parameter| parse_type_name(&parameter.type_name))
                        .collect(),
                )
            })
            .collect();

        let binding_types = checker
            .symbols
            .into_iter()
            .filter_map(|(name, symbol)| symbol.value_type.map(|value_type| (name, value_type)))
            .collect();

        Ok(CheckedProgram {
            program: program.clone(),
            binding_types,
            action_parameter_types,
            runtime_reductions,
        })
    } else {
        Err(checker.errors)
    }
}

struct Checker {
    symbols: HashMap<String, Symbol>,
    action_names: HashSet<String>,
    action_signatures: HashMap<String, Vec<ActionParameter>>,
    parameters: HashMap<String, LocalSymbol>,
    runtime_reduction_types: HashMap<String, ValueType>,
    errors: Vec<Diagnostic>,
    current_location: SourceLocation,
}

impl Checker {
    fn check_program(&mut self, program: &Program) {
        for decl in &program.declarations {
            self.current_location = decl.location();
            match decl {
                Declaration::State(state) => {
                    if self.symbols.contains_key(&state.name) {
                        self.error(format!("duplicate declaration '{}'", state.name));
                        continue;
                    }

                    let inferred = self.infer_expr(&state.initializer);
                    let declared = match &state.type_name {
                        Some(name) => match parse_source_type_name(name) {
                            Some(value_type) => Some(value_type),
                            None => {
                                self.error(format!("unknown type '{name}'"));
                                None
                            }
                        },
                        None => None,
                    };
                    if let (Some(inferred), Some(declared)) = (&inferred, &declared) {
                        if !assignable_to(declared, inferred) {
                            self.error(format!(
                                "state '{}' is declared as {} but initialized with {}",
                                state.name,
                                show_type(declared),
                                show_type(inferred)
                            ));
                        }
                    }
                    self.symbols.insert(
                        state.name.clone(),
                        Symbol {
                            kind: SymbolKind::State,
                            value_type: declared.or(inferred),
                        },
                    );
                }
                Declaration::Derived(derived) => {
                    if self.symbols.contains_key(&derived.name) {
                        self.error(format!("duplicate declaration '{}'", derived.name));
                        continue;
                    }
                    let value_type = self
                        .runtime_reduction_types
                        .get(&derived.name)
                        .cloned()
                        .or_else(|| self.infer_expr(&derived.expression));
                    self.symbols.insert(
                        derived.name.clone(),
                        Symbol {
                            kind: SymbolKind::Derived,
                            value_type,
                        },
                    );
                }
                Declaration::Action(action) => {
                    if self.symbols.contains_key(&action.name) {
                        self.error(format!("duplicate declaration '{}'", action.name));
                        continue;
                    }

                    self.symbols.insert(
                        action.name.clone(),
                        Symbol {
                            kind: SymbolKind::Action,
                            value_type: None,
                        },
                    );

                    let previous_parameters = std::mem::take(&mut self.parameters);

                    for parameter in &action.parameters {
                        self.current_location = parameter.location;

                        if self.parameters.contains_key(&parameter.name) {
                            self.error(format!("duplicate action parameter '{}'", parameter.name));
                            continue;
                        }

                        let kind = match parameter.kind {
                            ActionParameterKind::Value => LocalKind::ValueParameter,
                            ActionParameterKind::State => LocalKind::StateParameter,
                        };

                        let value_type = match parse_source_type_name(&parameter.type_name) {
                            Some(value_type) => value_type,
                            None => {
                                self.error(format!("unknown type '{}'", parameter.type_name));
                                ValueType::Named(parameter.type_name.clone())
                            }
                        };

                        self.parameters
                            .insert(parameter.name.clone(), LocalSymbol { kind, value_type });
                    }

                    self.current_location = action.location;

                    for statement in &action.statements {
                        self.check_statement(statement);
                    }

                    self.parameters = previous_parameters;
                }
            }
        }
    }

    fn check_statement(&mut self, statement: &Statement) {
        let previous_location = self.current_location;
        self.current_location = statement.location();
        self.check_statement_at_current_location(statement);
        self.current_location = previous_location;
    }

    fn check_statement_at_current_location(&mut self, statement: &Statement) {
        match statement {
            Statement::Assignment {
                target,
                operator,
                value,
                ..
            } => {
                if let Some(parameter) = self.parameters.get(target).cloned() {
                    match parameter.kind {
                        LocalKind::ValueParameter => {
                            self.error(format!("cannot assign to value parameter '{target}'"));
                        }
                        LocalKind::StateParameter => {
                            let rhs_type = self.infer_expr(value);
                            if let Some(rhs) = rhs_type {
                                self.check_assignment_against_type(
                                    target,
                                    "state parameter",
                                    operator,
                                    &parameter.value_type,
                                    &rhs,
                                );
                            }
                        }
                    }
                    return;
                }

                let Some(symbol) = self.symbols.get(target).cloned() else {
                    self.error(format!("unknown assignment target '{target}'"));
                    return;
                };

                if symbol.kind != SymbolKind::State {
                    let kind = match symbol.kind {
                        SymbolKind::Derived => "derived value",
                        SymbolKind::Action => "action",
                        SymbolKind::State => unreachable!(),
                    };
                    self.error(format!("cannot assign to {kind} '{target}'"));
                    return;
                }

                let rhs_type = self.infer_expr(value);
                if let (Some(lhs), Some(rhs)) = (&symbol.value_type, &rhs_type) {
                    self.check_assignment_against_type(target, "state", operator, lhs, rhs);
                }
            }
            Statement::IndexedThroughAssignment { .. } => {
                self.error("internal source indexed assignment reached semantic checking before sequence lowering".to_string());
            }
            Statement::RuntimeIndexAssignment {
                source,
                element_model,
                member_type_name,
                operator,
                value,
                ..
            } => {
                match self.symbols.get(source).cloned() {
                    Some(Symbol {
                        value_type: Some(ValueType::SequenceLive(actual)),
                        ..
                    }) if actual == *element_model => {}
                    Some(Symbol {
                        value_type: Some(actual),
                        ..
                    }) => self.error(format!(
                        "runtime indexed source '{}' has type {} but expects [live {}]",
                        source,
                        show_type(&actual),
                        element_model
                    )),
                    Some(_) => self.error(format!(
                        "runtime indexed source '{}' does not have an ordered live-sequence type",
                        source
                    )),
                    None => self.error(format!("unknown runtime indexed source '{source}'")),
                }

                let Some(member_type) = parse_source_type_name(member_type_name) else {
                    self.error(format!(
                        "internal runtime indexed member has unknown type '{}'",
                        member_type_name
                    ));
                    return;
                };
                if let Some(rhs) = self.infer_expr(value) {
                    self.check_assignment_against_type(
                        "runtime indexed member",
                        "runtime indexed state member",
                        operator,
                        &member_type,
                        &rhs,
                    );
                }
            }
            Statement::RuntimeDesignationAssignment {
                designation,
                member_type_name,
                operator,
                value,
                ..
            } => {
                if !matches!(self.infer_expr(designation), Some(ValueType::String)) {
                    self.error(
                        "internal runtime designation assignment requires designation carrier"
                            .to_string(),
                    );
                }
                let Some(target_type) = parse_source_type_name(member_type_name) else {
                    self.error(format!(
                        "internal runtime designation member has unknown type '{member_type_name}'"
                    ));
                    return;
                };
                if let Some(value_type) = self.infer_expr(value) {
                    let result_type = match operator {
                        AssignmentOperator::Assign => value_type.clone(),
                        AssignmentOperator::AddAssign => {
                            binary_result_type(BinaryOperator::Add, &target_type, &value_type)
                                .unwrap_or(value_type.clone())
                        }
                        AssignmentOperator::SubtractAssign => {
                            binary_result_type(BinaryOperator::Subtract, &target_type, &value_type)
                                .unwrap_or(value_type.clone())
                        }
                        AssignmentOperator::MultiplyAssign => {
                            binary_result_type(BinaryOperator::Multiply, &target_type, &value_type)
                                .unwrap_or(value_type.clone())
                        }
                        AssignmentOperator::DivideAssign => {
                            binary_result_type(BinaryOperator::Divide, &target_type, &value_type)
                                .unwrap_or(value_type.clone())
                        }
                    };
                    if !assignable_to(&target_type, &result_type) {
                        self.error(format!(
                            "cannot assign {} to {} designation member",
                            show_type(&result_type),
                            show_type(&target_type)
                        ));
                    }
                }
            }
            Statement::ActionCall {
                name, arguments, ..
            } => {
                self.check_action_call(name, arguments);
            }
            Statement::Fail { message, .. } => {
                if let Some(message_type) = self.infer_expr(message) {
                    if message_type != ValueType::String {
                        self.error(format!(
                            "fail message must be String, got {}",
                            show_type(&message_type)
                        ));
                    }
                }
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                if let Some(condition_type) = self.infer_expr(condition) {
                    if condition_type != ValueType::Bool {
                        self.error(format!(
                            "if statement condition must be Bool, got {}",
                            show_type(&condition_type)
                        ));
                    }
                }

                for statement in then_branch {
                    self.check_statement(statement);
                }
                if let Some(else_branch) = else_branch {
                    for statement in else_branch {
                        self.check_statement(statement);
                    }
                }
            }
        }
    }

    fn check_assignment_against_type(
        &mut self,
        target: &str,
        target_kind: &str,
        operator: &AssignmentOperator,
        lhs: &ValueType,
        rhs: &ValueType,
    ) {
        match operator {
            AssignmentOperator::Assign => {
                if !assignable_to(lhs, rhs) {
                    self.error(format!(
                        "cannot assign {} to {} '{}' of type {}",
                        show_type(rhs),
                        target_kind,
                        target,
                        show_type(lhs)
                    ));
                }
            }
            _ => {
                if !is_numeric(lhs) || !is_numeric(rhs) {
                    self.error(format!(
                        "compound arithmetic assignment on '{}' requires numeric values",
                        target
                    ));
                } else {
                    let result_type = common_type(lhs.clone(), rhs.clone());
                    if !assignable_to(lhs, &result_type) {
                        self.error(format!(
                            "compound arithmetic result {} cannot be stored in {} '{}' of type {}",
                            show_type(&result_type),
                            target_kind,
                            target,
                            show_type(lhs)
                        ));
                    }
                }
            }
        }
    }

    fn check_action_call(&mut self, name: &str, arguments: &[ActionArgument]) {
        if !self.action_names.contains(name) {
            if self.symbols.contains_key(name) {
                self.error(format!("'{name}' is not an action"));
            } else {
                self.error(format!("unknown action '{name}'"));
            }
            return;
        }

        let Some(parameters) = self.action_signatures.get(name).cloned() else {
            self.error(format!("missing signature for action '{name}'"));
            return;
        };

        if parameters.len() != arguments.len() {
            self.error(format!(
                "action '{}' expects {} arguments but got {}",
                name,
                parameters.len(),
                arguments.len()
            ));
            return;
        }

        for (parameter, argument) in parameters.iter().zip(arguments) {
            let expected = parse_type_name(&parameter.type_name);

            match (parameter.kind, argument) {
                (ActionParameterKind::Value, ActionArgument::Value(expression)) => {
                    if let Some(actual) = self.infer_expr(expression) {
                        if !assignable_to(&expected, &actual) {
                            self.error(format!(
                                "argument for value parameter '{}' has type {} but expected {}",
                                parameter.name,
                                show_type(&actual),
                                show_type(&expected)
                            ));
                        }
                    }
                }
                (_, ActionArgument::IndexedStateGrant { .. }) => {
                    self.error("internal source indexed state grant reached semantic checking before grant transport".to_string());
                }
                (ActionParameterKind::Value, ActionArgument::StateGrant { name, .. }) => {
                    self.error(format!(
                        "value parameter '{}' requires a value argument, not writable state grant 'state {}'",
                        parameter.name, name
                    ));
                }
                (ActionParameterKind::State, ActionArgument::Value(expression)) => {
                    if let Expr::Name(argument_name) = expression {
                        self.error(format!(
                            "writable state parameter '{}' requires explicit authority; use 'state {}'",
                            parameter.name, argument_name
                        ));
                    } else {
                        self.error(format!(
                            "writable state parameter '{}' requires an explicit 'state <name>' argument",
                            parameter.name
                        ));
                    }
                }
                (ActionParameterKind::State, ActionArgument::StateGrant { name: granted, .. }) => {
                    if let Some(actual) = self.resolve_writable_state_grant(granted) {
                        if actual != expected {
                            self.error(format!(
                                "writable state argument 'state {}' has type {} but parameter '{}' requires {}",
                                granted,
                                show_type(&actual),
                                parameter.name,
                                show_type(&expected)
                            ));
                        }
                    }
                }
            }
        }
    }

    fn resolve_writable_state_grant(&mut self, name: &str) -> Option<ValueType> {
        if let Some(parameter) = self.parameters.get(name).cloned() {
            return match parameter.kind {
                LocalKind::StateParameter => Some(parameter.value_type),
                LocalKind::ValueParameter => {
                    self.error(format!(
                        "cannot grant value parameter '{}' as writable state",
                        name
                    ));
                    None
                }
            };
        }

        match self.symbols.get(name).cloned() {
            Some(symbol) if symbol.kind == SymbolKind::State => symbol.value_type,
            Some(symbol) if symbol.kind == SymbolKind::Derived => {
                self.error(format!(
                    "cannot grant derived value '{}' as writable state",
                    name
                ));
                None
            }
            Some(_) => {
                self.error(format!("cannot grant action '{}' as writable state", name));
                None
            }
            None => {
                self.error(format!("unknown writable state '{}'", name));
                None
            }
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> Option<ValueType> {
        match expr {
            Expr::Integer(_) => Some(ValueType::Int),
            Expr::Float(_) => Some(ValueType::Float),
            Expr::Bool(_) => Some(ValueType::Bool),
            Expr::String(value) => {
                if let Some((model, _)) = decode_runtime_sequence_value(value) {
                    Some(ValueType::SequenceLive(model))
                } else {
                    Some(ValueType::String)
                }
            }
            Expr::Name(name) => {
                if let Some(parameter) = self.parameters.get(name) {
                    return Some(parameter.value_type.clone());
                }

                match self.symbols.get(name) {
                    Some(symbol) if symbol.kind != SymbolKind::Action => symbol.value_type.clone(),
                    Some(_) => {
                        self.error(format!("action '{name}' cannot be used as a value"));
                        None
                    }
                    None => {
                        self.error(format!("unknown name '{name}'"));
                        None
                    }
                }
            }
            Expr::Binary {
                operator,
                left,
                right,
            } => {
                let left_type = self.infer_expr(left);
                let right_type = self.infer_expr(right);
                match (left_type, right_type) {
                    (Some(left), Some(right)) => self.infer_binary(*operator, left, right),
                    _ => None,
                }
            }
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if let Some(condition_type) = self.infer_expr(condition) {
                    if condition_type != ValueType::Bool {
                        self.error("if condition must be Bool".to_string());
                    }
                }

                let then_type = self.infer_expr(then_branch);
                let else_type = self.infer_expr(else_branch);
                match (then_type, else_type) {
                    (Some(a), Some(b)) if types_compatible(&a, &b) => Some(common_type(a, b)),
                    (Some(a), Some(b)) => {
                        self.error(format!(
                            "if branches have incompatible types {} and {}",
                            show_type(&a),
                            show_type(&b)
                        ));
                        None
                    }
                    _ => None,
                }
            }
            Expr::IndexedMember { .. } | Expr::IndexedDesignation { .. } => {
                self.error("internal source indexed expression reached semantic checking before sequence lowering".to_string());
                None
            }
            Expr::RuntimeIndexDesignation {
                source,
                index,
                element_model,
            } => {
                match self.infer_expr(index) {
                    Some(ValueType::Int) => {}
                    Some(actual) => self.error(format!(
                        "sequence index expression must be Int, got {}",
                        show_type(&actual)
                    )),
                    None => {}
                }
                match self.infer_expr(source) {
                    Some(ValueType::SequenceLive(actual)) if actual == *element_model => {
                        Some(ValueType::String)
                    }
                    Some(actual) => {
                        self.error(format!(
                            "runtime indexed source has type {} but expects [live {}]",
                            show_type(&actual),
                            element_model
                        ));
                        None
                    }
                    None => None,
                }
            }
            Expr::RuntimeDesignationMember {
                designation,
                member_type_name,
                ..
            } => {
                match self.infer_expr(designation) {
                    Some(ValueType::String) => {}
                    Some(actual) => self.error(format!(
                        "internal live designation carrier must be String, got {}",
                        show_type(&actual)
                    )),
                    None => {}
                }
                parse_source_type_name(member_type_name).or_else(|| {
                    self.error(format!(
                        "internal designation member has unknown type '{member_type_name}'"
                    ));
                    None
                })
            }
            Expr::RuntimeIndexMember {
                source,
                index,
                element_model,
                member_type_name,
                ..
            } => {
                match self.infer_expr(index) {
                    Some(ValueType::Int) => {}
                    Some(actual) => self.error(format!(
                        "sequence index expression must be Int, got {}",
                        show_type(&actual)
                    )),
                    None => {}
                }
                match self.infer_expr(source) {
                    Some(ValueType::SequenceLive(actual)) if actual == *element_model => {}
                    Some(actual) => self.error(format!(
                        "runtime indexed source has type {} but expects [live {}]",
                        show_type(&actual),
                        element_model
                    )),
                    None => {}
                }
                match parse_source_type_name(member_type_name) {
                    Some(value_type) => Some(value_type),
                    None => {
                        self.error(format!(
                            "internal runtime indexed member has unknown type '{}'",
                            member_type_name
                        ));
                        None
                    }
                }
            }
            Expr::Filter {
                source,
                element_model,
                ..
            } => {
                let source_type = self.infer_expr(source);
                let expected = element_model
                    .as_ref()
                    .map(|model| ValueType::SequenceLive(model.clone()));
                match (source_type, expected) {
                    (
                        Some(ValueType::SequenceLive(actual)),
                        Some(ValueType::SequenceLive(expected_model)),
                    ) if actual == expected_model => Some(ValueType::SequenceLive(expected_model)),
                    (Some(actual), Some(expected)) => {
                        self.error(format!(
                            "internal filter source has type {} but filter result expects {}",
                            show_type(&actual),
                            show_type(&expected)
                        ));
                        None
                    }
                    (_, None) => {
                        self.error("internal filter is missing its live element model".to_string());
                        None
                    }
                    (None, _) => None,
                }
            }
        }
    }

    fn infer_binary(
        &mut self,
        operator: BinaryOperator,
        left: ValueType,
        right: ValueType,
    ) -> Option<ValueType> {
        if let Some(result) = binary_result_type(operator, &left, &right) {
            return Some(result);
        }

        self.error(format!(
            "operator {:?} is not defined for {} and {}",
            operator,
            show_type(&left),
            show_type(&right)
        ));
        None
    }

    fn error(&mut self, message: String) {
        self.errors.push(Diagnostic::new(
            message,
            self.current_location.line,
            self.current_location.column,
        ));
    }
}

pub(crate) fn parse_primitive_type_name(name: &str) -> Option<ValueType> {
    match name {
        "Int" => Some(ValueType::Int),
        "Float" => Some(ValueType::Float),
        "Bool" => Some(ValueType::Bool),
        "String" => Some(ValueType::String),
        _ => None,
    }
}

fn parse_source_type_name(name: &str) -> Option<ValueType> {
    if let Some(model) = decode_runtime_sequence_type(name) {
        return Some(ValueType::SequenceLive(model.to_string()));
    }

    parse_primitive_type_name(name)
}

fn parse_type_name(name: &str) -> ValueType {
    parse_source_type_name(name).unwrap_or_else(|| ValueType::Named(name.to_string()))
}

pub fn show_type(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Int => "Int".to_string(),
        ValueType::Float => "Float".to_string(),
        ValueType::Bool => "Bool".to_string(),
        ValueType::String => "String".to_string(),
        ValueType::SequenceLive(model) => format!("[live {model}]"),
        ValueType::Named(name) => name.clone(),
    }
}

fn is_numeric(value_type: &ValueType) -> bool {
    matches!(value_type, ValueType::Int | ValueType::Float)
}

pub(crate) fn types_compatible(a: &ValueType, b: &ValueType) -> bool {
    a == b || (is_numeric(a) && is_numeric(b))
}

pub(crate) fn assignable_to(target: &ValueType, source: &ValueType) -> bool {
    target == source || (matches!(target, ValueType::Float) && matches!(source, ValueType::Int))
}

pub(crate) fn common_type(a: ValueType, b: ValueType) -> ValueType {
    if a == ValueType::Float || b == ValueType::Float {
        ValueType::Float
    } else {
        a
    }
}

pub(crate) fn binary_result_type(
    operator: BinaryOperator,
    left: &ValueType,
    right: &ValueType,
) -> Option<ValueType> {
    match operator {
        BinaryOperator::Add => {
            if left == &ValueType::String && right == &ValueType::String {
                Some(ValueType::String)
            } else if is_numeric(left) && is_numeric(right) {
                Some(common_type(left.clone(), right.clone()))
            } else {
                None
            }
        }
        BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide => {
            if is_numeric(left) && is_numeric(right) {
                Some(common_type(left.clone(), right.clone()))
            } else {
                None
            }
        }
        BinaryOperator::And | BinaryOperator::Or => {
            if left == &ValueType::Bool && right == &ValueType::Bool {
                Some(ValueType::Bool)
            } else {
                None
            }
        }
        BinaryOperator::PreviousIn | BinaryOperator::NextIn => {
            if left == &ValueType::String && matches!(right, ValueType::SequenceLive(_)) {
                Some(ValueType::String)
            } else {
                None
            }
        }
        BinaryOperator::IsPresent => None,
        BinaryOperator::IsIn => {
            if left == &ValueType::String && matches!(right, ValueType::SequenceLive(_)) {
                Some(ValueType::Bool)
            } else {
                None
            }
        }
        BinaryOperator::Contains | BinaryOperator::ContainsIgnoringCase => {
            if left == &ValueType::String && right == &ValueType::String {
                Some(ValueType::Bool)
            } else {
                None
            }
        }
        BinaryOperator::Equal | BinaryOperator::NotEqual => {
            if (is_numeric(left) && is_numeric(right))
                || (left == right
                    && matches!(
                        left,
                        ValueType::Bool | ValueType::String | ValueType::SequenceLive(_)
                    ))
            {
                Some(ValueType::Bool)
            } else {
                None
            }
        }
        BinaryOperator::Less
        | BinaryOperator::LessEqual
        | BinaryOperator::Greater
        | BinaryOperator::GreaterEqual => {
            if is_numeric(left) && is_numeric(right) {
                Some(ValueType::Bool)
            } else {
                None
            }
        }
    }
}
