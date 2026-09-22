#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    State,
    Model,
    Derived,
    Action,
    Live,
    Through,
    Fail,
    If,
    Else,
    True,
    False,
    Identifier(String),
    Integer(i64),
    Float(f64),
    String(String),
    Colon,
    Comma,
    Dot,
    Equal,
    EqualEqual,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    PlusEqual,
    MinusEqual,
    StarEqual,
    SlashEqual,
    Plus,
    Minus,
    Star,
    Slash,
    LeftBrace,
    RightBrace,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    Newline,
    Semicolon,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: usize,
    pub column: usize,
}

impl Token {
    pub fn new(kind: TokenKind, line: usize, column: usize) -> Self {
        Self { kind, line, column }
    }
}
