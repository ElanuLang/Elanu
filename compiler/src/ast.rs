use std::fmt;

use crate::reduction_surface::{decode_reduction, ReductionSpec};
use crate::sequence_surface::{
    decode_sequence_index_segment, decode_sequence_literal, decode_sequence_live_type,
};

pub const LIVE_TYPE_PREFIX: &str = "__elanu_surface_live_type$";
pub const MAYBE_LIVE_TYPE_PREFIX: &str = "__elanu_surface_maybe_live_type$";
pub const LIVE_DERIVED_PREFIX: &str = "__elanu_surface_live_derived$";
pub const LIVE_CAPTURE_PREFIX: &str = "__elanu_surface_live_capture$";
pub const THROUGH_PREFIX: &str = "__elanu_surface_through$";

pub fn encode_live_type_name(model: &str) -> String {
    format!("{LIVE_TYPE_PREFIX}{model}")
}

pub fn decode_live_type_name(type_name: &str) -> Option<&str> {
    type_name.strip_prefix(LIVE_TYPE_PREFIX)
}

pub fn encode_maybe_live_type_name(model: &str) -> String {
    format!("{MAYBE_LIVE_TYPE_PREFIX}{model}")
}

pub fn decode_maybe_live_type_name(type_name: &str) -> Option<&str> {
    type_name.strip_prefix(MAYBE_LIVE_TYPE_PREFIX)
}

pub fn encode_live_derived_name(name: &str, model: &str) -> String {
    format!("{LIVE_DERIVED_PREFIX}{model}${name}")
}

pub fn decode_live_derived_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(LIVE_DERIVED_PREFIX)?;
    let (model, source_name) = rest.split_once('$')?;
    Some((source_name, model))
}

pub fn encode_live_capture(name: &str) -> String {
    format!("{LIVE_CAPTURE_PREFIX}{name}")
}

pub fn decode_live_capture(name: &str) -> Option<&str> {
    name.strip_prefix(LIVE_CAPTURE_PREFIX)
}

pub fn encode_through_path(path: &str) -> String {
    format!("{THROUGH_PREFIX}{path}")
}

pub fn decode_through_path(path: &str) -> Option<&str> {
    path.strip_prefix(THROUGH_PREFIX)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub declarations: Vec<Declaration>,
    pub state_models: Vec<StateModelDecl>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceLocation {
    pub line: usize,
    pub column: usize,
}

impl SourceLocation {
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Declaration {
    State(StateDecl),
    Derived(DerivedDecl),
    Action(ActionDecl),
}

impl Declaration {
    pub fn location(&self) -> SourceLocation {
        match self {
            Declaration::State(state) => state.location,
            Declaration::Derived(derived) => derived.location,
            Declaration::Action(action) => action.location,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StateModelDecl {
    pub location: SourceLocation,
    pub name: String,
    pub members: Vec<StateModelMember>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateModelMember {
    State(StateDecl),
    Derived(DerivedDecl),
}

impl StateModelMember {
    pub fn location(&self) -> SourceLocation {
        match self {
            StateModelMember::State(state) => state.location,
            StateModelMember::Derived(derived) => derived.location,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            StateModelMember::State(state) => &state.name,
            StateModelMember::Derived(derived) => &derived.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StateDecl {
    pub location: SourceLocation,
    pub name: String,
    pub type_name: Option<String>,
    pub initializer: Expr,
    /// Surface-only marker for `state name: Model` declarations whose member
    /// defaults are supplied by a `state model`. The state-model lowering pass
    /// consumes these before ordinary semantic checking/runtime construction.
    pub implicit_model_initializer: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DerivedDecl {
    pub location: SourceLocation,
    pub name: String,
    pub expression: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionDecl {
    pub location: SourceLocation,
    pub name: String,
    pub parameters: Vec<ActionParameter>,
    pub statements: Vec<Statement>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionParameter {
    pub location: SourceLocation,
    pub kind: ActionParameterKind,
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionParameterKind {
    Value,
    State,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActionArgument {
    Value(Expr),
    StateGrant {
        location: SourceLocation,
        name: String,
    },
    /// Source-level writable authority selected through a runtime index
    /// expression. The grant-transport pass resolves model/member typing and
    /// lowers this to the existing private grant carrier before semantic check.
    IndexedStateGrant {
        location: SourceLocation,
        source: String,
        index: Expr,
        member: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    Assignment {
        location: SourceLocation,
        target: String,
        operator: AssignmentOperator,
        value: Expr,
    },
    /// Source-level indirect mutation through a runtime index expression.
    /// Sequence lowering resolves model/member typing before runtime execution.
    IndexedThroughAssignment {
        location: SourceLocation,
        source: String,
        index: Expr,
        member: String,
        operator: AssignmentOperator,
        value: Expr,
    },
    /// Compiler-resolved mutation through the designation currently stored at
    /// one runtime-sized sequence position. This is an internal structured
    /// representation, not an independently writable sequence slot.
    RuntimeIndexAssignment {
        location: SourceLocation,
        source: String,
        index: Box<Expr>,
        member: String,
        element_model: String,
        member_type_name: String,
        operator: AssignmentOperator,
        value: Expr,
    },
    /// Compiler-resolved mutation through a stored live designation whose
    /// current target is selected at runtime.
    RuntimeDesignationAssignment {
        location: SourceLocation,
        designation: Box<Expr>,
        member: String,
        element_model: String,
        member_type_name: String,
        operator: AssignmentOperator,
        value: Expr,
    },
    ActionCall {
        location: SourceLocation,
        name: String,
        arguments: Vec<ActionArgument>,
    },
    Fail {
        location: SourceLocation,
        message: Expr,
    },
    If {
        location: SourceLocation,
        condition: Expr,
        then_branch: Vec<Statement>,
        else_branch: Option<Vec<Statement>>,
    },
}

impl Statement {
    pub fn location(&self) -> SourceLocation {
        match self {
            Statement::Assignment { location, .. }
            | Statement::IndexedThroughAssignment { location, .. }
            | Statement::RuntimeIndexAssignment { location, .. }
            | Statement::RuntimeDesignationAssignment { location, .. }
            | Statement::ActionCall { location, .. }
            | Statement::Fail { location, .. }
            | Statement::If { location, .. } => *location,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,
    AddAssign,
    SubtractAssign,
    MultiplyAssign,
    DivideAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Integer(i64),
    Float(f64),
    Bool(bool),
    String(String),
    /// Names may contain source-level member qualification such as
    /// `invoice.quantity`. The state-model lowering pass resolves such paths
    /// before the ordinary semantic checker/runtime see the program.
    Name(String),
    Binary {
        operator: BinaryOperator,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    If {
        condition: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    /// Identity-preserving derived structural sequence view.
    ///
    /// `filter` always controls membership. An optional ordering key may also
    /// determine view order without mutating the backing membership. The key is
    /// structured compiler data, not a private encoded source fragment.
    Filter {
        source: Box<Expr>,
        element: String,
        predicate: Box<Expr>,
        order_by: Option<Box<Expr>>,
        order_descending: bool,
        element_model: Option<String>,
    },
    /// Source-level member read through a runtime index expression.
    /// Sequence lowering resolves the element model/member type.
    IndexedMember {
        source: String,
        index: Box<Expr>,
        member: String,
    },
    /// Source-level whole live designation selected from `[live T]`.
    IndexedDesignation {
        source: String,
        index: Box<Expr>,
    },
    /// Compiler-resolved read through the designation currently stored at one
    /// runtime-sized sequence position.
    RuntimeIndexMember {
        source: Box<Expr>,
        index: Box<Expr>,
        member: String,
        element_model: String,
        member_type_name: String,
    },
    /// Compiler-resolved exact designation selected from a runtime-sized
    /// `[live T]` sequence. Runtime representation is private.
    RuntimeIndexDesignation {
        source: Box<Expr>,
        index: Box<Expr>,
        element_model: String,
    },
    /// Read a member through a stored designation resolved at runtime.
    RuntimeDesignationMember {
        designation: Box<Expr>,
        member: String,
        element_model: String,
        member_type_name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    And,
    Or,
    Contains,
    ContainsIgnoringCase,
    /// Compiler-only contextual predicate produced by `<designation> is present`.
    /// Live-designation lowering consumes this before semantic checking/runtime.
    IsPresent,
    /// Contextual child-membership relation produced by `<designation> is in <view>`.
    /// Sequence/live lowering validates and resolves its two semantic operands.
    IsIn,
    /// Contextual designation-producing relation: the unique previous child identity
    /// relative to one persistent designation in a current ordered [live T] structure.
    PreviousIn,
    /// Contextual designation-producing relation: the unique next child identity
    /// relative to one persistent designation in a current ordered [live T] structure.
    NextIn,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Program")?;
        for model in &self.state_models {
            write_state_model(f, model, 1)?;
        }
        for decl in &self.declarations {
            write_declaration(f, decl, 1)?;
        }
        Ok(())
    }
}

fn indent(f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
    for _ in 0..depth {
        write!(f, "  ")?;
    }
    Ok(())
}

fn write_state_model(
    f: &mut fmt::Formatter<'_>,
    model: &StateModelDecl,
    depth: usize,
) -> fmt::Result {
    indent(f, depth)?;
    writeln!(f, "StateModelDecl {}", model.name)?;
    for member in &model.members {
        match member {
            StateModelMember::State(state) => {
                indent(f, depth + 1)?;
                if let Some(type_name) = &state.type_name {
                    writeln!(
                        f,
                        "StateDecl {}: {}",
                        state.name,
                        display_surface_type_name(type_name)
                    )?;
                } else {
                    writeln!(f, "StateDecl {}", state.name)?;
                }
                write_expr(f, &state.initializer, depth + 2)?;
            }
            StateModelMember::Derived(derived) => {
                indent(f, depth + 1)?;
                writeln!(f, "DerivedDecl {}", derived.name)?;
                write_expr(f, &derived.expression, depth + 2)?;
            }
        }
    }
    Ok(())
}

fn write_declaration(f: &mut fmt::Formatter<'_>, decl: &Declaration, depth: usize) -> fmt::Result {
    indent(f, depth)?;
    match decl {
        Declaration::State(state) => {
            if let Some(type_name) = &state.type_name {
                writeln!(
                    f,
                    "StateDecl {}: {}",
                    state.name,
                    display_surface_type_name(type_name)
                )?;
            } else {
                writeln!(f, "StateDecl {}", state.name)?;
            }
            if state.implicit_model_initializer {
                indent(f, depth + 1)?;
                writeln!(f, "ImplicitModelInitializer")
            } else {
                write_expr(f, &state.initializer, depth + 1)
            }
        }
        Declaration::Derived(derived) => {
            if let Some((name, model)) = decode_live_derived_name(&derived.name) {
                writeln!(f, "DerivedDecl {}: live {}", name, model)?;
            } else {
                writeln!(f, "DerivedDecl {}", derived.name)?;
            }
            write_expr(f, &derived.expression, depth + 1)
        }
        Declaration::Action(action) => {
            writeln!(f, "ActionDecl {}", action.name)?;
            for parameter in &action.parameters {
                indent(f, depth + 1)?;
                let kind = match parameter.kind {
                    ActionParameterKind::Value => "Value",
                    ActionParameterKind::State => "State",
                };
                writeln!(
                    f,
                    "ActionParameter {} {}: {}",
                    kind,
                    parameter.name,
                    display_surface_type_name(&parameter.type_name)
                )?;
            }
            for stmt in &action.statements {
                write_statement(f, stmt, depth + 1)?;
            }
            Ok(())
        }
    }
}

fn write_statement(f: &mut fmt::Formatter<'_>, stmt: &Statement, depth: usize) -> fmt::Result {
    match stmt {
        Statement::Assignment {
            target,
            operator,
            value,
            ..
        } => {
            indent(f, depth)?;
            if let Some(path) = decode_through_path(target) {
                writeln!(
                    f,
                    "ThroughAssignment {} {:?}",
                    display_surface_name(path),
                    operator
                )?;
            } else {
                writeln!(
                    f,
                    "Assignment {} {:?}",
                    display_surface_name(target),
                    operator
                )?;
            }
            write_expr(f, value, depth + 1)
        }
        Statement::IndexedThroughAssignment {
            source,
            index,
            member,
            operator,
            value,
            ..
        } => {
            indent(f, depth)?;
            writeln!(
                f,
                "IndexedThroughAssignment {}[...].{} {:?}",
                display_surface_name(source),
                member,
                operator
            )?;
            write_expr(f, index, depth + 1)?;
            write_expr(f, value, depth + 1)
        }
        Statement::RuntimeIndexAssignment {
            source,
            index,
            member,
            operator,
            value,
            ..
        } => {
            indent(f, depth)?;
            writeln!(
                f,
                "RuntimeIndexAssignment {}[...].{} {:?}",
                display_surface_name(source),
                member,
                operator
            )?;
            write_expr(f, index, depth + 1)?;
            write_expr(f, value, depth + 1)
        }
        Statement::RuntimeDesignationAssignment {
            designation,
            member,
            ..
        } => {
            indent(f, depth)?;
            writeln!(f, "RuntimeDesignationAssignment .{member}")?;
            write_expr(f, designation, depth + 1)
        }
        Statement::ActionCall {
            name, arguments, ..
        } => {
            indent(f, depth)?;
            writeln!(f, "ActionCall {name}")?;
            for argument in arguments {
                match argument {
                    ActionArgument::StateGrant { name, .. } => {
                        indent(f, depth + 1)?;
                        if let Some(path) = decode_through_path(name) {
                            writeln!(f, "StateGrant through {}", display_surface_name(path))?;
                        } else {
                            writeln!(f, "StateGrant {}", display_surface_name(name))?;
                        }
                    }
                    ActionArgument::IndexedStateGrant {
                        source,
                        index,
                        member,
                        ..
                    } => {
                        indent(f, depth + 1)?;
                        writeln!(
                            f,
                            "StateGrant through {}[...].{}",
                            display_surface_name(source),
                            member
                        )?;
                        write_expr(f, index, depth + 2)?;
                    }
                    ActionArgument::Value(expression) => {
                        indent(f, depth + 1)?;
                        writeln!(f, "ValueArgument")?;
                        write_expr(f, expression, depth + 2)?;
                    }
                }
            }
            Ok(())
        }
        Statement::Fail { message, .. } => {
            indent(f, depth)?;
            writeln!(f, "Fail")?;
            write_expr(f, message, depth + 1)
        }
        Statement::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            indent(f, depth)?;
            writeln!(f, "IfStatement")?;
            indent(f, depth + 1)?;
            writeln!(f, "Condition")?;
            write_expr(f, condition, depth + 2)?;
            indent(f, depth + 1)?;
            writeln!(f, "Then")?;
            for statement in then_branch {
                write_statement(f, statement, depth + 2)?;
            }
            if let Some(else_branch) = else_branch {
                indent(f, depth + 1)?;
                writeln!(f, "Else")?;
                for statement in else_branch {
                    write_statement(f, statement, depth + 2)?;
                }
            }
            Ok(())
        }
    }
}

fn write_expr(f: &mut fmt::Formatter<'_>, expr: &Expr, depth: usize) -> fmt::Result {
    indent(f, depth)?;
    match expr {
        Expr::Integer(value) => writeln!(f, "Int({value})"),
        Expr::Float(value) => writeln!(f, "Float({value})"),
        Expr::Bool(value) => writeln!(f, "Bool({value})"),
        Expr::String(value) => {
            if let Some(targets) = decode_sequence_literal(value) {
                writeln!(f, "SequenceLiteral")?;
                for target in targets {
                    indent(f, depth + 1)?;
                    writeln!(f, "Live({target})")?;
                }
                Ok(())
            } else if let Some(reduction) = decode_reduction(value) {
                write_reduction(f, &reduction, depth)
            } else {
                writeln!(f, "String({value:?})")
            }
        }
        Expr::Name(name) => {
            if let Some(target) = decode_live_capture(name) {
                writeln!(f, "Live({})", display_surface_name(target))
            } else {
                writeln!(f, "Name({})", display_surface_name(name))
            }
        }
        Expr::Binary {
            operator,
            left,
            right,
        } => {
            writeln!(f, "Binary({operator:?})")?;
            write_expr(f, left, depth + 1)?;
            write_expr(f, right, depth + 1)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            writeln!(f, "If")?;
            indent(f, depth + 1)?;
            writeln!(f, "Condition")?;
            write_expr(f, condition, depth + 2)?;
            indent(f, depth + 1)?;
            writeln!(f, "Then")?;
            write_expr(f, then_branch, depth + 2)?;
            indent(f, depth + 1)?;
            writeln!(f, "Else")?;
            write_expr(f, else_branch, depth + 2)
        }
        Expr::IndexedMember {
            source,
            index,
            member,
        } => {
            writeln!(
                f,
                "IndexedMember {}[...].{}",
                display_surface_name(source),
                member
            )?;
            write_expr(f, index, depth + 1)
        }
        Expr::RuntimeIndexMember {
            source,
            index,
            member,
            element_model,
            ..
        } => {
            writeln!(f, "RuntimeIndexMember live {element_model} [...].{member}")?;
            write_expr(f, source, depth + 1)?;
            write_expr(f, index, depth + 1)
        }
        Expr::IndexedDesignation { source, index } => {
            indent(f, depth)?;
            writeln!(f, "IndexedDesignation {source}")?;
            write_expr(f, index, depth + 1)
        }
        Expr::RuntimeIndexDesignation {
            source,
            index,
            element_model,
        } => {
            indent(f, depth)?;
            writeln!(f, "RuntimeIndexDesignation live {element_model}")?;
            write_expr(f, source, depth + 1)?;
            write_expr(f, index, depth + 1)
        }
        Expr::RuntimeDesignationMember {
            designation,
            member,
            element_model,
            ..
        } => {
            indent(f, depth)?;
            writeln!(f, "RuntimeDesignationMember live {element_model}.{member}")?;
            write_expr(f, designation, depth + 1)
        }
        Expr::Filter {
            source,
            element,
            predicate,
            order_by,
            order_descending,
            element_model,
        } => {
            writeln!(f, "Filter")?;
            indent(f, depth + 1)?;
            writeln!(f, "Source")?;
            write_expr(f, source, depth + 2)?;
            indent(f, depth + 1)?;
            if let Some(model) = element_model {
                writeln!(f, "Element({element}: live {model})")?;
            } else {
                writeln!(f, "Element({element})")?;
            }
            indent(f, depth + 1)?;
            writeln!(f, "Predicate")?;
            write_expr(f, predicate, depth + 2)?;
            if let Some(order_by) = order_by {
                indent(f, depth + 1)?;
                writeln!(
                    f,
                    "OrderBy({})",
                    if *order_descending {
                        "descending"
                    } else {
                        "ascending"
                    }
                )?;
                write_expr(f, order_by, depth + 2)?;
            }
            Ok(())
        }
    }
}

fn display_surface_type_name(type_name: &str) -> String {
    if let Some(model) = decode_live_type_name(type_name) {
        format!("live {model}")
    } else if let Some(model) = decode_sequence_live_type(type_name) {
        format!("[live {model}]")
    } else {
        type_name.to_string()
    }
}

fn display_surface_name(name: &str) -> String {
    let mut output = String::new();
    for segment in name.split('.') {
        if let Some(index) = decode_sequence_index_segment(segment) {
            output.push('[');
            output.push_str(&index.to_string());
            output.push(']');
        } else {
            if !output.is_empty() {
                output.push('.');
            }
            output.push_str(segment);
        }
    }
    output
}

fn write_reduction(
    f: &mut fmt::Formatter<'_>,
    reduction: &ReductionSpec,
    depth: usize,
) -> fmt::Result {
    writeln!(f, "Reduction")?;
    indent(f, depth + 1)?;
    writeln!(f, "Source({})", display_surface_name(&reduction.source))?;
    indent(f, depth + 1)?;
    writeln!(f, "InitialSource({:?})", reduction.initial_source)?;
    indent(f, depth + 1)?;
    writeln!(
        f,
        "Bindings({}, {})",
        reduction.accumulator, reduction.element
    )?;
    indent(f, depth + 1)?;
    writeln!(f, "StepSource({:?})", reduction.step_source)
}
