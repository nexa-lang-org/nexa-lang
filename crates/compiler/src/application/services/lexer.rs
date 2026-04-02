use crate::domain::span::Span;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // ── App-level keywords ──────────────────────────────────────────────────
    App,
    Server,
    Route,
    Package,
    Import,
    // ── OO keywords ────────────────────────────────────────────────────────
    Class,
    Interface,
    Component,
    Window,
    Public,
    Private,
    Extends,
    Implements,
    Constructor,
    Return,
    This,
    // ── Control flow ───────────────────────────────────────────────────────
    If,
    Else,
    For,
    While,
    Break,
    Continue,
    Let,
    In,
    // ── Built-in types ─────────────────────────────────────────────────────
    TInt,
    TString,
    TBool,
    TVoid,
    TList,
    // ── Symbols ────────────────────────────────────────────────────────────
    LBrace,
    RBrace,
    LParen,
    RParen,
    LAngle,   // <
    RAngle,   // >
    FatArrow, // =>
    Semicolon,
    Colon,
    Comma,
    Dot,
    // ── Assignment / comparison ────────────────────────────────────────────
    Equals,       // =
    EqualEqual,   // ==
    BangEqual,    // !=
    LessEqual,    // <=
    GreaterEqual, // >=
    // ── Arithmetic ─────────────────────────────────────────────────────────
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    // ── Logic ──────────────────────────────────────────────────────────────
    And,  // &&
    Or,   // ||
    Bang, // !
    // ── Literals ───────────────────────────────────────────────────────────
    StringLit(String),
    IntLit(i64),
    BoolLit(bool),
    // ── Identifier / EOF ───────────────────────────────────────────────────
    Ident(String),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Spanned {
    pub token: Token,
    pub span: Span,
}

#[derive(Debug, Error)]
pub enum LexError {
    #[error("Unexpected character '{0}' at line {}, col {}", .1.line, .1.col)]
    UnexpectedChar(char, Span),
    #[error("Unterminated string at line {}", .0.line)]
    UnterminatedString(Span),
}

impl LexError {
    pub fn span(&self) -> Span {
        match self {
            LexError::UnexpectedChar(_, s) => *s,
            LexError::UnterminatedString(s) => *s,
        }
    }
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Lexer {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if let Some(ch) = c {
            self.pos += 1;
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
        c
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.peek().map(|c| c.is_whitespace()).unwrap_or(false) {
                self.advance();
            }
            if self.peek() == Some('/') && self.peek2() == Some('/') {
                while self.peek().map(|c| c != '\n').unwrap_or(false) {
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn read_string(&mut self) -> Result<Token, LexError> {
        let line = self.line;
        let start_col = self.col;
        self.advance(); // consume opening "
        let mut s = String::new();
        loop {
            match self.peek() {
                None | Some('\n') => {
                    return Err(LexError::UnterminatedString(Span::new(
                        line as u32,
                        start_col as u32,
                        1,
                    )));
                }
                Some('"') => {
                    self.advance();
                    return Ok(Token::StringLit(s));
                }
                Some('\\') => {
                    self.advance();
                    match self.advance() {
                        Some('n') => s.push('\n'),
                        Some('t') => s.push('\t'),
                        Some('"') => s.push('"'),
                        Some('\\') => s.push('\\'),
                        _ => {}
                    }
                }
                Some(c) => {
                    s.push(c);
                    self.advance();
                }
            }
        }
    }

    fn read_number(&mut self) -> Token {
        let mut n = String::new();
        while self.peek().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            n.push(self.advance().unwrap());
        }
        Token::IntLit(n.parse().unwrap_or(0))
    }

    fn read_ident_or_keyword(&mut self) -> Token {
        let mut s = String::new();
        while self
            .peek()
            .map(|c| c.is_alphanumeric() || c == '_')
            .unwrap_or(false)
        {
            s.push(self.advance().unwrap());
        }
        match s.as_str() {
            "app" => Token::App,
            "server" => Token::Server,
            "route" => Token::Route,
            "package" => Token::Package,
            "import" => Token::Import,
            "class" => Token::Class,
            "interface" => Token::Interface,
            "component" => Token::Component,
            "window" => Token::Window,
            "public" => Token::Public,
            "private" => Token::Private,
            "extends" => Token::Extends,
            "implements" => Token::Implements,
            "constructor" => Token::Constructor,
            "return" => Token::Return,
            "this" => Token::This,
            "if" => Token::If,
            "else" => Token::Else,
            "for" => Token::For,
            "while" => Token::While,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "let" => Token::Let,
            "in" => Token::In,
            "Int" => Token::TInt,
            "String" => Token::TString,
            "Bool" => Token::TBool,
            "Void" => Token::TVoid,
            "List" => Token::TList,
            "true" => Token::BoolLit(true),
            "false" => Token::BoolLit(false),
            _ => Token::Ident(s),
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Spanned>, LexError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            let line = self.line;
            let col = self.col;

            let tok = match self.peek() {
                None => {
                    tokens.push(Spanned {
                        token: Token::Eof,
                        span: Span::new(line as u32, col as u32, 0),
                    });
                    break;
                }
                Some('"') => self.read_string()?,
                Some(c) if c.is_ascii_digit() => self.read_number(),
                Some(c) if c.is_alphabetic() || c == '_' => self.read_ident_or_keyword(),
                Some('{') => {
                    self.advance();
                    Token::LBrace
                }
                Some('}') => {
                    self.advance();
                    Token::RBrace
                }
                Some('(') => {
                    self.advance();
                    Token::LParen
                }
                Some(')') => {
                    self.advance();
                    Token::RParen
                }
                Some(';') => {
                    self.advance();
                    Token::Semicolon
                }
                Some(':') => {
                    self.advance();
                    Token::Colon
                }
                Some(',') => {
                    self.advance();
                    Token::Comma
                }
                Some('.') => {
                    self.advance();
                    Token::Dot
                }
                Some('+') => {
                    self.advance();
                    Token::Plus
                }
                Some('-') => {
                    self.advance();
                    Token::Minus
                }
                Some('*') => {
                    self.advance();
                    Token::Star
                }
                Some('%') => {
                    self.advance();
                    Token::Percent
                }
                Some('=') => {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        Token::FatArrow
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::EqualEqual
                    } else {
                        Token::Equals
                    }
                }
                Some('!') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::BangEqual
                    } else {
                        Token::Bang
                    }
                }
                Some('<') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::LessEqual
                    } else {
                        Token::LAngle
                    }
                }
                Some('>') => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Token::GreaterEqual
                    } else {
                        Token::RAngle
                    }
                }
                Some('&') => {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance();
                        Token::And
                    } else {
                        return Err(LexError::UnexpectedChar(
                            '&',
                            Span::new(line as u32, col as u32, 1),
                        ));
                    }
                }
                Some('|') => {
                    self.advance();
                    if self.peek() == Some('|') {
                        self.advance();
                        Token::Or
                    } else {
                        return Err(LexError::UnexpectedChar(
                            '|',
                            Span::new(line as u32, col as u32, 1),
                        ));
                    }
                }
                Some('/') => {
                    self.advance();
                    // single slash as division operator (comments already handled above)
                    Token::Slash
                }
                Some(c) => {
                    return Err(LexError::UnexpectedChar(
                        c,
                        Span::new(line as u32, col as u32, 1),
                    ));
                }
            };
            let len = (self.col as u32).saturating_sub(col as u32);
            tokens.push(Spanned {
                token: tok,
                span: Span::new(line as u32, col as u32, len),
            });
        }
        Ok(tokens)
    }
}
