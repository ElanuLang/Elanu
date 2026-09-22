use crate::diagnostic::Diagnostic;
use crate::token::{Token, TokenKind};

pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    Lexer::new(source).lex_all()
}

struct Lexer<'a> {
    chars: Vec<char>,
    index: usize,
    line: usize,
    column: usize,
    _source: &'a str,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            chars: source.chars().collect(),
            index: 0,
            line: 1,
            column: 1,
            _source: source,
        }
    }

    fn lex_all(mut self) -> Result<Vec<Token>, Vec<Diagnostic>> {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();

        while let Some(ch) = self.peek() {
            let line = self.line;
            let column = self.column;

            match ch {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                '\n' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::Newline, line, column));
                }
                '/' if self.peek_next() == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.advance();
                    }
                }
                ':' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::Colon, line, column));
                }
                ',' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::Comma, line, column));
                }
                '.' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::Dot, line, column));
                }
                ';' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::Semicolon, line, column));
                }
                '{' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::LeftBrace, line, column));
                }
                '}' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::RightBrace, line, column));
                }
                '(' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::LeftParen, line, column));
                }
                ')' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::RightParen, line, column));
                }
                '[' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::LeftBracket, line, column));
                }
                ']' => {
                    self.advance();
                    tokens.push(Token::new(TokenKind::RightBracket, line, column));
                }
                '+' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::PlusEqual
                    } else {
                        TokenKind::Plus
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '-' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::MinusEqual
                    } else {
                        TokenKind::Minus
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '*' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::StarEqual
                    } else {
                        TokenKind::Star
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '/' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::SlashEqual
                    } else {
                        TokenKind::Slash
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '=' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::EqualEqual
                    } else {
                        TokenKind::Equal
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '!' => {
                    self.advance();
                    if self.match_char('=') {
                        tokens.push(Token::new(TokenKind::BangEqual, line, column));
                    } else {
                        errors.push(Diagnostic::new("expected '=' after '!'", line, column));
                    }
                }
                '<' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::LessEqual
                    } else {
                        TokenKind::Less
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '>' => {
                    self.advance();
                    let kind = if self.match_char('=') {
                        TokenKind::GreaterEqual
                    } else {
                        TokenKind::Greater
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                '"' => match self.string_literal() {
                    Ok(value) => tokens.push(Token::new(TokenKind::String(value), line, column)),
                    Err(message) => errors.push(Diagnostic::new(message, line, column)),
                },
                c if c.is_ascii_digit() => match self.number_literal() {
                    Ok(kind) => tokens.push(Token::new(kind, line, column)),
                    Err(message) => errors.push(Diagnostic::new(message, line, column)),
                },
                c if is_ident_start(c) => {
                    let ident = self.identifier();
                    let kind = match ident.as_str() {
                        "state" => TokenKind::State,
                        "model" => TokenKind::Model,
                        "derived" => TokenKind::Derived,
                        "action" => TokenKind::Action,
                        "live" => TokenKind::Live,
                        "through" => TokenKind::Through,
                        "fail" => TokenKind::Fail,
                        "if" => TokenKind::If,
                        "else" => TokenKind::Else,
                        "true" => TokenKind::True,
                        "false" => TokenKind::False,
                        _ => TokenKind::Identifier(ident),
                    };
                    tokens.push(Token::new(kind, line, column));
                }
                other => {
                    errors.push(Diagnostic::new(
                        format!("unexpected character '{other}'"),
                        line,
                        column,
                    ));
                    self.advance();
                }
            }
        }

        tokens.push(Token::new(TokenKind::Eof, self.line, self.column));

        if errors.is_empty() {
            Ok(tokens)
        } else {
            Err(errors)
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.index + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn match_char(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn identifier(&mut self) -> String {
        let mut value = String::new();
        while let Some(ch) = self.peek() {
            if is_ident_continue(ch) {
                value.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        value
    }

    fn number_literal(&mut self) -> Result<TokenKind, String> {
        let mut value = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_digit() {
                value.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if self.peek() == Some('.') && self.peek_next().is_some_and(|c| c.is_ascii_digit()) {
            value.push('.');
            self.advance();
            while let Some(ch) = self.peek() {
                if ch.is_ascii_digit() {
                    value.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
            value
                .parse::<f64>()
                .map(TokenKind::Float)
                .map_err(|_| "invalid floating-point literal".to_string())
        } else {
            value
                .parse::<i64>()
                .map(TokenKind::Integer)
                .map_err(|_| "integer literal is out of range".to_string())
        }
    }

    fn string_literal(&mut self) -> Result<String, String> {
        self.advance(); // opening quote
        let mut value = String::new();

        while let Some(ch) = self.peek() {
            match ch {
                '"' => {
                    self.advance();
                    return Ok(value);
                }
                '\n' => return Err("unterminated string literal".to_string()),
                '\\' => {
                    self.advance();
                    let escaped = self
                        .advance()
                        .ok_or_else(|| "unterminated string escape".to_string())?;
                    match escaped {
                        'n' => value.push('\n'),
                        'r' => value.push('\r'),
                        't' => value.push('\t'),
                        '"' => value.push('"'),
                        '\\' => value.push('\\'),
                        other => return Err(format!("unsupported escape sequence \\{other}")),
                    }
                }
                other => {
                    value.push(other);
                    self.advance();
                }
            }
        }

        Err("unterminated string literal".to_string())
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    is_ident_start(ch) || ch.is_ascii_digit()
}
