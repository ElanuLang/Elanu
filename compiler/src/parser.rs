use crate::ast::{
    encode_live_capture, encode_live_derived_name, encode_live_type_name,
    encode_maybe_live_type_name, encode_through_path, ActionArgument, ActionDecl, ActionParameter,
    ActionParameterKind, AssignmentOperator, BinaryOperator, Declaration, DerivedDecl, Expr,
    Program, SourceLocation, StateDecl, StateModelDecl, StateModelMember, Statement,
};
use crate::diagnostic::Diagnostic;
use crate::token::{Token, TokenKind};

const IMPLICIT_MODEL_INITIALIZER: &str = "__meld_implicit_model_initializer";

pub struct Parser {
    tokens: Vec<Token>,
    current: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, current: 0 }
    }

    pub fn parse_program(mut self) -> Result<Program, Vec<Diagnostic>> {
        let mut declarations = Vec::new();
        let mut state_models = Vec::new();
        let mut errors = Vec::new();
        self.skip_separators();

        while !self.at_end() {
            if self.check_simple(&TokenKind::State)
                && self.check_offset_simple(1, &TokenKind::Model)
            {
                match self.parse_state_model() {
                    Ok(model) => state_models.push(model),
                    Err(error) => {
                        errors.push(error);
                        self.synchronize_top_level();
                    }
                }
            } else {
                match self.parse_declaration() {
                    Ok(decl) => declarations.push(decl),
                    Err(error) => {
                        errors.push(error);
                        self.synchronize_top_level();
                    }
                }
            }
            self.skip_separators();
        }

        if errors.is_empty() {
            Ok(Program {
                declarations,
                state_models,
            })
        } else {
            Err(errors)
        }
    }

    fn parse_declaration(&mut self) -> Result<Declaration, Diagnostic> {
        match self.peek().kind.clone() {
            TokenKind::State => self.parse_state(),
            TokenKind::Derived => self.parse_derived(),
            TokenKind::Action => self.parse_action(),
            _ => Err(self.error_here("expected 'state', 'derived', or 'action' declaration")),
        }
    }

    fn parse_state_model(&mut self) -> Result<StateModelDecl, Diagnostic> {
        let location = self.location_here();
        self.consume_simple(&TokenKind::State, "expected 'state'")?;
        self.consume_simple(&TokenKind::Model, "expected 'model' after 'state'")?;
        let name = self.consume_identifier("expected state model name")?;
        self.consume_simple(&TokenKind::LeftBrace, "expected '{' after state model name")?;
        self.skip_separators();

        let mut members = Vec::new();
        while !self.check_simple(&TokenKind::RightBrace) && !self.at_end() {
            let member = match self.peek().kind.clone() {
                TokenKind::State => StateModelMember::State(self.parse_state_model_state_member()?),
                TokenKind::Derived => {
                    StateModelMember::Derived(self.parse_state_model_derived_member()?)
                }
                _ => {
                    return Err(self.error_here(
                        "state model bodies currently allow only 'state' and 'derived' declarations",
                    ));
                }
            };
            members.push(member);

            if !self.check_simple(&TokenKind::RightBrace) {
                self.require_statement_separator()?;
            }
            self.skip_separators();
        }

        self.consume_simple(
            &TokenKind::RightBrace,
            "expected '}' after state model body",
        )?;

        Ok(StateModelDecl {
            location,
            name,
            members,
        })
    }

    fn parse_state_model_state_member(&mut self) -> Result<StateDecl, Diagnostic> {
        let location = self.location_here();
        self.advance();
        let name = self.consume_identifier("expected state member name")?;
        let type_name = if self.matches_simple(&TokenKind::Colon) {
            if self.check_identifier_value("maybe") {
                self.advance();
                self.consume_simple(
                    &TokenKind::Live,
                    "expected 'live' after 'maybe' in state-model designation member type",
                )?;
                let model = self.consume_identifier(
                    "expected state model name after 'maybe live' in state-model member",
                )?;
                Some(encode_maybe_live_type_name(&model))
            } else {
                Some(self.consume_identifier("expected type name after ':'")?)
            }
        } else {
            None
        };
        self.consume_simple(
            &TokenKind::Equal,
            "state model state members require an initializer",
        )?;
        self.skip_newlines();
        let initializer = self.parse_expression()?;
        Ok(StateDecl {
            location,
            name,
            type_name,
            initializer,
            implicit_model_initializer: false,
        })
    }

    fn parse_state_model_derived_member(&mut self) -> Result<DerivedDecl, Diagnostic> {
        let location = self.location_here();
        self.advance();
        let name = self.consume_identifier("expected derived member name")?;
        self.consume_simple(&TokenKind::Equal, "expected '=' in derived declaration")?;
        self.skip_newlines();
        let expression = self.parse_expression()?;
        Ok(DerivedDecl {
            location,
            name,
            expression,
        })
    }

    fn parse_state(&mut self) -> Result<Declaration, Diagnostic> {
        let location = self.location_here();
        self.advance();
        let name = self.consume_identifier("expected state name")?;
        let type_name = if self.matches_simple(&TokenKind::Colon) {
            if self.check_identifier_value("maybe") {
                self.advance();
                self.consume_simple(
                    &TokenKind::Live,
                    "expected 'live' after 'maybe' in designation state type",
                )?;
                let model =
                    self.consume_identifier("expected state model name after 'maybe live'")?;
                Some(encode_maybe_live_type_name(&model))
            } else if self.matches_simple(&TokenKind::Live) {
                let model = self.consume_identifier("expected state model name after 'live'")?;
                Some(encode_live_type_name(&model))
            } else {
                Some(self.consume_identifier("expected type name after ':'")?)
            }
        } else {
            None
        };

        if self.matches_simple(&TokenKind::Equal) {
            self.skip_newlines();
            let initializer = self.parse_expression()?;
            return Ok(Declaration::State(StateDecl {
                location,
                name,
                type_name,
                initializer,
                implicit_model_initializer: false,
            }));
        }

        if type_name.as_deref().is_some_and(|name| {
            name.starts_with(crate::ast::LIVE_TYPE_PREFIX)
                || name.starts_with(crate::ast::MAYBE_LIVE_TYPE_PREFIX)
        }) {
            return Err(
                self.error_here("live designation state declarations require an initializer")
            );
        }

        if type_name.is_none() {
            return Err(self.error_here("expected '=' in state declaration"));
        }

        if !(self.check_simple(&TokenKind::Newline)
            || self.check_simple(&TokenKind::Semicolon)
            || self.check_simple(&TokenKind::Eof))
        {
            return Err(
                self.error_here("expected '=' or end of declaration after typed state declaration")
            );
        }

        Ok(Declaration::State(StateDecl {
            location,
            name,
            type_name,
            initializer: Expr::Name(IMPLICIT_MODEL_INITIALIZER.to_string()),
            implicit_model_initializer: true,
        }))
    }

    fn parse_derived(&mut self) -> Result<Declaration, Diagnostic> {
        let location = self.location_here();
        self.advance();
        let source_name = self.consume_identifier("expected derived value name")?;
        let name = if self.matches_simple(&TokenKind::Colon) {
            self.consume_simple(
                &TokenKind::Live,
                "derived type annotations currently require 'live <StateModel>'",
            )?;
            let model = self.consume_identifier("expected state model name after 'live'")?;
            encode_live_derived_name(&source_name, &model)
        } else {
            source_name
        };
        self.consume_simple(&TokenKind::Equal, "expected '=' in derived declaration")?;
        self.skip_newlines();
        let expression = self.parse_expression()?;
        Ok(Declaration::Derived(DerivedDecl {
            location,
            name,
            expression,
        }))
    }

    fn parse_action(&mut self) -> Result<Declaration, Diagnostic> {
        let location = self.location_here();
        self.advance();
        let name = self.consume_identifier("expected action name")?;

        let parameters = if self.matches_simple(&TokenKind::LeftParen) {
            if self.check_simple(&TokenKind::RightParen) {
                return Err(self.error_here("zero-parameter action declarations omit parentheses"));
            }

            self.parse_action_parameters()?
        } else {
            Vec::new()
        };

        let statements = self.parse_statement_block(
            "expected '{' after action declaration",
            "expected '}' after action body",
        )?;

        Ok(Declaration::Action(ActionDecl {
            location,
            name,
            parameters,
            statements,
        }))
    }

    fn parse_action_parameters(&mut self) -> Result<Vec<ActionParameter>, Diagnostic> {
        let mut parameters = Vec::new();

        if self.matches_simple(&TokenKind::RightParen) {
            return Ok(parameters);
        }

        loop {
            let location = self.location_here();

            let kind = if self.matches_simple(&TokenKind::State) {
                ActionParameterKind::State
            } else {
                ActionParameterKind::Value
            };

            let name = self.consume_identifier("expected action parameter name")?;

            self.consume_simple(
                &TokenKind::Colon,
                "expected ':' after action parameter name",
            )?;

            let type_name =
                self.consume_identifier("expected type name after action parameter ':'")?;

            parameters.push(ActionParameter {
                location,
                kind,
                name,
                type_name,
            });

            if self.matches_simple(&TokenKind::Comma) {
                continue;
            }

            self.consume_simple(
                &TokenKind::RightParen,
                "expected ')' after action parameters",
            )?;

            break;
        }

        Ok(parameters)
    }

    fn parse_action_arguments(&mut self) -> Result<Vec<ActionArgument>, Diagnostic> {
        let mut arguments = Vec::new();

        if self.matches_simple(&TokenKind::RightParen) {
            return Ok(arguments);
        }

        loop {
            if self.check_simple(&TokenKind::State) {
                let location = self.location_here();
                self.advance();
                let through = self.matches_simple(&TokenKind::Through);

                let mut name = self.consume_name_path(
                    "expected state name or member path after 'state' in action argument",
                )?;
                if self.matches_simple(&TokenKind::LeftBracket) {
                    if !through {
                        return Err(self.error_previous(
                            "indexed designation authority requires 'state through'",
                        ));
                    }
                    let index = self.parse_expression()?;
                    self.consume_simple(
                        &TokenKind::RightBracket,
                        "expected ']' after sequence index expression",
                    )?;
                    self.consume_simple(
                        &TokenKind::Dot,
                        "indexed writable authority must select a modeled-state member",
                    )?;
                    let member = self.consume_identifier(
                        "expected modeled-state member after indexed designation",
                    )?;
                    arguments.push(ActionArgument::IndexedStateGrant {
                        location,
                        source: name,
                        index,
                        member,
                    });
                } else {
                    if through {
                        name = encode_through_path(&name);
                    }
                    arguments.push(ActionArgument::StateGrant { location, name });
                }
            } else {
                arguments.push(ActionArgument::Value(self.parse_expression()?));
            }

            if self.matches_simple(&TokenKind::Comma) {
                continue;
            }

            self.consume_simple(
                &TokenKind::RightParen,
                "expected ')' after action arguments",
            )?;

            break;
        }

        Ok(arguments)
    }

    fn parse_statement(&mut self) -> Result<Statement, Diagnostic> {
        if self.check_simple(&TokenKind::If) {
            let location = self.location_here();
            self.advance();
            return self.parse_if_statement(location);
        }

        if self.check_simple(&TokenKind::Fail) {
            let location = self.location_here();
            self.advance();
            let message = self.parse_expression()?;
            return Ok(Statement::Fail { location, message });
        }

        let location = self.location_here();
        let through = self.matches_simple(&TokenKind::Through);
        let mut name =
            self.consume_name_path("expected assignment target, action name, 'if', or 'fail'")?;

        if self.matches_simple(&TokenKind::LeftBracket) {
            if !through {
                return Err(self.error_previous("indexed designation mutation requires 'through'"));
            }
            let index = self.parse_expression()?;
            self.consume_simple(
                &TokenKind::RightBracket,
                "expected ']' after sequence index expression",
            )?;
            self.consume_simple(
                &TokenKind::Dot,
                "indexed designation mutation must select a modeled-state member",
            )?;
            let member =
                self.consume_identifier("expected modeled-state member after indexed designation")?;
            let operator = match self.advance().kind.clone() {
                TokenKind::Equal => AssignmentOperator::Assign,
                TokenKind::PlusEqual => AssignmentOperator::AddAssign,
                TokenKind::MinusEqual => AssignmentOperator::SubtractAssign,
                TokenKind::StarEqual => AssignmentOperator::MultiplyAssign,
                TokenKind::SlashEqual => AssignmentOperator::DivideAssign,
                _ => return Err(self.error_previous("expected assignment operator")),
            };
            self.skip_newlines();
            let value = self.parse_expression()?;
            return Ok(Statement::IndexedThroughAssignment {
                location,
                source: name,
                index,
                member,
                operator,
                value,
            });
        }

        if self.matches_simple(&TokenKind::LeftParen) {
            if through {
                return Err(
                    self.error_previous("'through' is only valid for indirect assignment targets")
                );
            }
            let arguments = self.parse_action_arguments()?;
            return Ok(Statement::ActionCall {
                location,
                name,
                arguments,
            });
        }

        if through {
            name = encode_through_path(&name);
        }

        let operator = match self.advance().kind.clone() {
            TokenKind::Equal => AssignmentOperator::Assign,
            TokenKind::PlusEqual => AssignmentOperator::AddAssign,
            TokenKind::MinusEqual => AssignmentOperator::SubtractAssign,
            TokenKind::StarEqual => AssignmentOperator::MultiplyAssign,
            TokenKind::SlashEqual => AssignmentOperator::DivideAssign,
            _ => return Err(self.error_previous("expected assignment operator or '('")),
        };
        self.skip_newlines();
        let value = self.parse_expression()?;
        Ok(Statement::Assignment {
            location,
            target: name,
            operator,
            value,
        })
    }

    fn parse_if_statement(&mut self, location: SourceLocation) -> Result<Statement, Diagnostic> {
        let condition = self.parse_expression()?;
        let then_branch = self.parse_statement_block(
            "expected '{' after if condition",
            "expected '}' after if branch",
        )?;

        let after_then = self.current;
        self.skip_newlines();
        let else_branch = if self.matches_simple(&TokenKind::Else) {
            Some(self.parse_statement_block(
                "expected '{' after else",
                "expected '}' after else branch",
            )?)
        } else {
            self.current = after_then;
            None
        };

        Ok(Statement::If {
            location,
            condition,
            then_branch,
            else_branch,
        })
    }

    fn parse_statement_block(
        &mut self,
        open_message: &str,
        close_message: &str,
    ) -> Result<Vec<Statement>, Diagnostic> {
        self.consume_simple(&TokenKind::LeftBrace, open_message)?;
        self.skip_separators();

        let mut statements = Vec::new();
        while !self.check_simple(&TokenKind::RightBrace) && !self.at_end() {
            statements.push(self.parse_statement()?);
            if !self.check_simple(&TokenKind::RightBrace) {
                self.require_statement_separator()?;
            }
            self.skip_separators();
        }

        self.consume_simple(&TokenKind::RightBrace, close_message)?;
        Ok(statements)
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        if self.matches_simple(&TokenKind::If) {
            return self.parse_if_expression();
        }
        if self.check_identifier_value("filter") && self.check_offset_identifier(1) {
            self.advance();
            return self.parse_filter_expression();
        }
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_and()?;
        while self.check_identifier_value("or") {
            self.advance();
            let right = self.parse_and()?;
            expr = Expr::Binary {
                operator: BinaryOperator::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_not()?;
        while self.check_identifier_value("and") {
            self.advance();
            let right = self.parse_not()?;
            expr = Expr::Binary {
                operator: BinaryOperator::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_not(&mut self) -> Result<Expr, Diagnostic> {
        if self.check_identifier_value("not") {
            self.advance();
            let operand = self.parse_not()?;
            // `not` has no distinct dependency behavior beyond evaluating exactly
            // its one Bool operand. Lowering to Bool equality keeps the current
            // structured expression vocabulary narrow while preserving that law.
            return Ok(Expr::Binary {
                operator: BinaryOperator::Equal,
                left: Box::new(operand),
                right: Box::new(Expr::Bool(false)),
            });
        }
        self.parse_equality()
    }

    fn parse_if_expression(&mut self) -> Result<Expr, Diagnostic> {
        let condition = self.parse_expression()?;
        self.consume_simple(&TokenKind::LeftBrace, "expected '{' after if condition")?;
        self.skip_newlines();
        let then_branch = self.parse_expression()?;
        self.skip_newlines();
        self.consume_simple(&TokenKind::RightBrace, "expected '}' after if branch")?;
        self.skip_newlines();
        self.consume_simple(&TokenKind::Else, "expected 'else' after if branch")?;
        self.consume_simple(&TokenKind::LeftBrace, "expected '{' after else")?;
        self.skip_newlines();
        let else_branch = self.parse_expression()?;
        self.skip_newlines();
        self.consume_simple(&TokenKind::RightBrace, "expected '}' after else branch")?;

        Ok(Expr::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
        })
    }

    fn parse_filter_expression(&mut self) -> Result<Expr, Diagnostic> {
        let source = self.consume_name_path("expected sequence source after 'filter'")?;
        self.skip_newlines();
        self.consume_contextual_identifier("as", "expected 'as' after filter source")?;
        self.skip_newlines();
        let element = self.consume_identifier("expected element binding after filter 'as'")?;
        self.skip_newlines();
        self.consume_simple(
            &TokenKind::LeftBrace,
            "expected '{' before filter predicate",
        )?;
        self.skip_newlines();
        let predicate = self.parse_expression()?;
        self.skip_newlines();
        self.consume_simple(
            &TokenKind::RightBrace,
            "expected '}' after filter predicate",
        )?;

        let (order_by, order_descending) = if self.check_identifier_value("order") {
            self.advance();
            self.skip_newlines();
            self.consume_contextual_identifier("by", "expected 'by' after filter 'order'")?;
            self.skip_newlines();
            let key = self.consume_name_path("expected child ordering key after 'order by'")?;
            self.skip_newlines();
            let descending = if self.check_identifier_value("ascending") {
                self.advance();
                false
            } else if self.check_identifier_value("descending") {
                self.advance();
                true
            } else {
                return Err(self
                    .error_here("expected 'ascending' or 'descending' after filter ordering key"));
            };
            (Some(Box::new(Expr::Name(key))), descending)
        } else {
            (None, false)
        };

        Ok(Expr::Filter {
            source: Box::new(Expr::Name(source)),
            element,
            predicate: Box::new(predicate),
            order_by,
            order_descending,
            element_model: None,
        })
    }

    fn parse_equality(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_containment()?;
        loop {
            let operator = if self.matches_simple(&TokenKind::EqualEqual) {
                Some(BinaryOperator::Equal)
            } else if self.matches_simple(&TokenKind::BangEqual) {
                Some(BinaryOperator::NotEqual)
            } else {
                None
            };

            let Some(operator) = operator else { break };
            let right = self.parse_containment()?;
            expr = Expr::Binary {
                operator,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_containment(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_comparison()?;
        while self.check_identifier_value("contains") {
            self.advance();
            let right = self.parse_comparison()?;
            let operator = if self.check_identifier_value("ignoring") {
                self.advance();
                self.consume_contextual_identifier(
                    "case",
                    "expected 'case' after 'ignoring' in containment predicate",
                )?;
                BinaryOperator::ContainsIgnoringCase
            } else {
                BinaryOperator::Contains
            };
            expr = Expr::Binary {
                operator,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_presence()?;
        loop {
            let operator = if self.matches_simple(&TokenKind::Less) {
                Some(BinaryOperator::Less)
            } else if self.matches_simple(&TokenKind::LessEqual) {
                Some(BinaryOperator::LessEqual)
            } else if self.matches_simple(&TokenKind::Greater) {
                Some(BinaryOperator::Greater)
            } else if self.matches_simple(&TokenKind::GreaterEqual) {
                Some(BinaryOperator::GreaterEqual)
            } else {
                None
            };

            let Some(operator) = operator else { break };
            let right = self.parse_presence()?;
            expr = Expr::Binary {
                operator,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_presence(&mut self) -> Result<Expr, Diagnostic> {
        let expr = self.parse_additive()?;
        if self.check_identifier_value("is") {
            self.advance();
            if self.check_identifier_value("present") {
                self.advance();
                return Ok(Expr::Binary {
                    operator: BinaryOperator::IsPresent,
                    left: Box::new(expr),
                    right: Box::new(Expr::Bool(true)),
                });
            }
            if self.check_identifier_value("in") {
                self.advance();
                let right = self.parse_additive()?;
                return Ok(Expr::Binary {
                    operator: BinaryOperator::IsIn,
                    left: Box::new(expr),
                    right: Box::new(right),
                });
            }
            return Err(self.error_here(
                "expected 'present' or 'in' after 'is' in live designation predicate",
            ));
        }
        Ok(expr)
    }

    fn parse_additive(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_multiplicative()?;
        loop {
            let operator = if self.matches_simple(&TokenKind::Plus) {
                Some(BinaryOperator::Add)
            } else if self.matches_simple(&TokenKind::Minus) {
                Some(BinaryOperator::Subtract)
            } else {
                None
            };

            let Some(operator) = operator else { break };
            let right = self.parse_multiplicative()?;
            expr = Expr::Binary {
                operator,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_primary()?;
        loop {
            let operator = if self.matches_simple(&TokenKind::Star) {
                Some(BinaryOperator::Multiply)
            } else if self.matches_simple(&TokenKind::Slash) {
                Some(BinaryOperator::Divide)
            } else {
                None
            };

            let Some(operator) = operator else { break };
            let right = self.parse_primary()?;
            expr = Expr::Binary {
                operator,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, Diagnostic> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Integer(value) => Ok(Expr::Integer(value)),
            TokenKind::Float(value) => Ok(Expr::Float(value)),
            TokenKind::True => Ok(Expr::Bool(true)),
            TokenKind::False => Ok(Expr::Bool(false)),
            TokenKind::String(value) => Ok(Expr::String(value)),
            TokenKind::Identifier(name) => {
                if matches!(name.as_str(), "next" | "previous")
                    && matches!(&self.peek().kind, TokenKind::Identifier(_))
                {
                    let operator = if name == "next" {
                        BinaryOperator::NextIn
                    } else {
                        BinaryOperator::PreviousIn
                    };
                    let anchor = self.consume_name_path(
                        "expected persistent live designation after relative-navigation direction",
                    )?;
                    self.consume_contextual_identifier(
                        "in",
                        "expected 'in' after relative-navigation anchor",
                    )?;
                    let source = self.consume_name_path(
                        "expected ordered [live T] structure or filtered view after 'in'",
                    )?;
                    return Ok(Expr::Binary {
                        operator,
                        left: Box::new(Expr::Name(anchor)),
                        right: Box::new(Expr::Name(source)),
                    });
                }

                let name = self.finish_name_path(name)?;
                if self.matches_simple(&TokenKind::LeftBracket) {
                    let index = self.parse_expression()?;
                    self.consume_simple(
                        &TokenKind::RightBracket,
                        "expected ']' after sequence index expression",
                    )?;
                    if self.matches_simple(&TokenKind::Dot) {
                        let member = self.consume_identifier(
                            "expected modeled-state member after indexed designation",
                        )?;
                        Ok(Expr::IndexedMember {
                            source: name,
                            index: Box::new(index),
                            member,
                        })
                    } else {
                        Ok(Expr::IndexedDesignation {
                            source: name,
                            index: Box::new(index),
                        })
                    }
                } else {
                    Ok(Expr::Name(name))
                }
            }
            TokenKind::Live => {
                let target = self.consume_identifier("expected state binding after 'live'")?;
                Ok(Expr::Name(encode_live_capture(&target)))
            }
            TokenKind::LeftParen => {
                let expr = self.parse_expression()?;
                self.consume_simple(&TokenKind::RightParen, "expected ')' after expression")?;
                Ok(expr)
            }
            _ => Err(Diagnostic::new(
                "expected expression",
                token.line,
                token.column,
            )),
        }
    }

    fn consume_name_path(&mut self, message: &str) -> Result<String, Diagnostic> {
        let first = self.consume_identifier(message)?;
        self.finish_name_path(first)
    }

    fn finish_name_path(&mut self, mut name: String) -> Result<String, Diagnostic> {
        while self.matches_simple(&TokenKind::Dot) {
            let member = self.consume_identifier("expected member name after '.'")?;
            name.push('.');
            name.push_str(&member);
        }
        Ok(name)
    }

    fn consume_identifier(&mut self, message: &str) -> Result<String, Diagnostic> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Identifier(name) => Ok(name),
            _ => Err(Diagnostic::new(message, token.line, token.column)),
        }
    }

    fn consume_contextual_identifier(
        &mut self,
        expected: &str,
        message: &str,
    ) -> Result<(), Diagnostic> {
        if self.check_identifier_value(expected) {
            self.advance();
            Ok(())
        } else {
            Err(self.error_here(message))
        }
    }

    fn require_statement_separator(&mut self) -> Result<(), Diagnostic> {
        if self.check_simple(&TokenKind::Newline) || self.check_simple(&TokenKind::Semicolon) {
            Ok(())
        } else {
            Err(self.error_here("expected newline or ';' between statements"))
        }
    }

    fn skip_newlines(&mut self) {
        while self.check_simple(&TokenKind::Newline) {
            self.advance();
        }
    }

    fn skip_separators(&mut self) {
        while self.check_simple(&TokenKind::Newline) || self.check_simple(&TokenKind::Semicolon) {
            self.advance();
        }
    }

    fn synchronize_top_level(&mut self) {
        while !self.at_end() {
            if matches!(
                &self.peek().kind,
                TokenKind::State | TokenKind::Derived | TokenKind::Action
            ) {
                return;
            }
            self.advance();
        }
    }

    fn consume_simple(&mut self, expected: &TokenKind, message: &str) -> Result<(), Diagnostic> {
        if self.matches_simple(expected) {
            Ok(())
        } else {
            Err(self.error_here(message))
        }
    }

    fn matches_simple(&mut self, expected: &TokenKind) -> bool {
        if self.check_simple(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check_simple(&self, expected: &TokenKind) -> bool {
        self.check_offset_simple(0, expected)
    }

    fn check_offset_simple(&self, offset: usize, expected: &TokenKind) -> bool {
        let Some(token) = self.tokens.get(self.current + offset) else {
            return false;
        };
        std::mem::discriminant(&token.kind) == std::mem::discriminant(expected)
    }

    fn check_identifier_value(&self, expected: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Identifier(value) if value == expected)
    }

    fn check_offset_identifier(&self, offset: usize) -> bool {
        matches!(
            self.tokens
                .get(self.current + offset)
                .map(|token| &token.kind),
            Some(TokenKind::Identifier(_))
        )
    }

    fn at_end(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Eof)
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.current]
    }

    fn location_here(&self) -> SourceLocation {
        SourceLocation::new(self.peek().line, self.peek().column)
    }

    fn advance(&mut self) -> &Token {
        if !self.at_end() {
            self.current += 1;
        }
        &self.tokens[self.current.saturating_sub(1)]
    }

    fn error_here(&self, message: &str) -> Diagnostic {
        Diagnostic::new(message, self.peek().line, self.peek().column)
    }

    fn error_previous(&self, message: &str) -> Diagnostic {
        let token = &self.tokens[self.current.saturating_sub(1)];
        Diagnostic::new(message, token.line, token.column)
    }
}
