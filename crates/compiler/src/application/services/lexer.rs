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
    // ── Async ──────────────────────────────────────────────────────────────
    Async,
    Await,
    // ── ADT / pattern matching ─────────────────────────────────────────────
    Enum,
    Match,
    // ── Test blocks ────────────────────────────────────────────────────────
    Test,
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
    LBracket, // [
    RBracket, // ]
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
            "async" => Token::Async,
            "await" => Token::Await,
            "enum" => Token::Enum,
            "match" => Token::Match,
            "test" => Token::Test,
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

    /// Convenience: tokenize and strip the trailing `Eof`.
    #[cfg(test)]
    fn tokens(source: &str) -> Vec<Token> {
        let mut l = Lexer::new(source);
        let spanned = l.tokenize().expect("lex error");
        spanned.into_iter().map(|s| s.token).filter(|t| *t != Token::Eof).collect()
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
                Some('[') => {
                    self.advance();
                    Token::LBracket
                }
                Some(']') => {
                    self.advance();
                    Token::RBracket
                }
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn lex(src: &str) -> Vec<Token> {
        Lexer::tokens(src)
    }

    fn lex_err(src: &str) -> LexError {
        Lexer::new(src).tokenize().unwrap_err()
    }

    // ── empty / whitespace ────────────────────────────────────────────────────

    #[test]
    fn empty_source_produces_no_tokens() {
        assert!(lex("").is_empty());
    }

    #[test]
    fn whitespace_only_produces_no_tokens() {
        assert!(lex("   \t\n  ").is_empty());
    }

    // ── comments ──────────────────────────────────────────────────────────────

    #[test]
    fn line_comment_is_skipped() {
        assert!(lex("// this is a comment").is_empty());
    }

    #[test]
    fn comment_followed_by_token() {
        assert_eq!(lex("// comment\n42"), vec![Token::IntLit(42)]);
    }

    // ── identifiers ───────────────────────────────────────────────────────────

    #[test]
    fn plain_identifier() {
        assert_eq!(lex("foo"), vec![Token::Ident("foo".into())]);
    }

    #[test]
    fn underscore_identifier() {
        assert_eq!(lex("_bar"), vec![Token::Ident("_bar".into())]);
    }

    #[test]
    fn mixed_case_identifier() {
        assert_eq!(lex("MyClass"), vec![Token::Ident("MyClass".into())]);
    }

    #[test]
    fn identifier_with_digits() {
        assert_eq!(lex("x1"), vec![Token::Ident("x1".into())]);
    }

    // ── integer literals ──────────────────────────────────────────────────────

    #[test]
    fn integer_zero() {
        assert_eq!(lex("0"), vec![Token::IntLit(0)]);
    }

    #[test]
    fn integer_positive() {
        assert_eq!(lex("42"), vec![Token::IntLit(42)]);
    }

    #[test]
    fn large_integer() {
        assert_eq!(lex("1000000"), vec![Token::IntLit(1_000_000)]);
    }

    // ── boolean literals ──────────────────────────────────────────────────────

    #[test]
    fn bool_true() {
        assert_eq!(lex("true"), vec![Token::BoolLit(true)]);
    }

    #[test]
    fn bool_false() {
        assert_eq!(lex("false"), vec![Token::BoolLit(false)]);
    }

    // ── string literals ───────────────────────────────────────────────────────

    #[test]
    fn empty_string() {
        assert_eq!(lex(r#""""#), vec![Token::StringLit(String::new())]);
    }

    #[test]
    fn simple_string() {
        assert_eq!(lex(r#""hello""#), vec![Token::StringLit("hello".into())]);
    }

    #[test]
    fn string_with_escape_newline() {
        assert_eq!(lex(r#""a\nb""#), vec![Token::StringLit("a\nb".into())]);
    }

    #[test]
    fn string_with_escape_tab() {
        assert_eq!(lex(r#""a\tb""#), vec![Token::StringLit("a\tb".into())]);
    }

    #[test]
    fn string_with_escaped_quote() {
        assert_eq!(lex(r#""say \"hi\"""#), vec![Token::StringLit("say \"hi\"".into())]);
    }

    #[test]
    fn unterminated_string_returns_error() {
        matches!(lex_err(r#""hello"#), LexError::UnterminatedString(_));
    }

    // ── keywords ──────────────────────────────────────────────────────────────

    #[test]
    fn all_control_flow_keywords() {
        assert_eq!(
            lex("if else for while break continue let in return"),
            vec![
                Token::If, Token::Else, Token::For, Token::While,
                Token::Break, Token::Continue, Token::Let, Token::In,
                Token::Return,
            ]
        );
    }

    #[test]
    fn all_oo_keywords() {
        assert_eq!(
            lex("class interface component window public private extends implements constructor this"),
            vec![
                Token::Class, Token::Interface, Token::Component, Token::Window,
                Token::Public, Token::Private, Token::Extends, Token::Implements,
                Token::Constructor, Token::This,
            ]
        );
    }

    #[test]
    fn all_app_keywords() {
        assert_eq!(
            lex("app server route package import"),
            vec![Token::App, Token::Server, Token::Route, Token::Package, Token::Import]
        );
    }

    #[test]
    fn type_keywords() {
        assert_eq!(
            lex("Int String Bool Void List"),
            vec![Token::TInt, Token::TString, Token::TBool, Token::TVoid, Token::TList]
        );
    }

    // ── operators ─────────────────────────────────────────────────────────────

    #[test]
    fn arithmetic_operators() {
        assert_eq!(
            lex("+ - * / %"),
            vec![Token::Plus, Token::Minus, Token::Star, Token::Slash, Token::Percent]
        );
    }

    #[test]
    fn comparison_operators() {
        assert_eq!(
            lex("== != < > <= >="),
            vec![
                Token::EqualEqual, Token::BangEqual,
                Token::LAngle, Token::RAngle,
                Token::LessEqual, Token::GreaterEqual,
            ]
        );
    }

    #[test]
    fn logical_operators() {
        assert_eq!(lex("&& || !"), vec![Token::And, Token::Or, Token::Bang]);
    }

    #[test]
    fn assignment_and_fat_arrow() {
        assert_eq!(lex("= =>"), vec![Token::Equals, Token::FatArrow]);
    }

    // ── symbols ───────────────────────────────────────────────────────────────

    #[test]
    fn punctuation_symbols() {
        assert_eq!(
            lex("{ } ( ) ; : , ."),
            vec![
                Token::LBrace, Token::RBrace,
                Token::LParen, Token::RParen,
                Token::Semicolon, Token::Colon,
                Token::Comma, Token::Dot,
            ]
        );
    }

    // ── error cases ───────────────────────────────────────────────────────────

    #[test]
    fn unexpected_char_returns_error() {
        matches!(lex_err("@"), LexError::UnexpectedChar('@', _));
    }

    #[test]
    fn single_ampersand_returns_error() {
        matches!(lex_err("&"), LexError::UnexpectedChar('&', _));
    }

    #[test]
    fn single_pipe_returns_error() {
        matches!(lex_err("|"), LexError::UnexpectedChar('|', _));
    }

    // ── span tracking ─────────────────────────────────────────────────────────

    #[test]
    fn span_line_and_col_are_tracked() {
        let mut l = Lexer::new("foo\nbar");
        let tokens = l.tokenize().unwrap();
        assert_eq!(tokens[0].span.line, 1);
        assert_eq!(tokens[0].span.col, 1);
        assert_eq!(tokens[1].span.line, 2);
        assert_eq!(tokens[1].span.col, 1);
    }

    // ── realistic snippet ─────────────────────────────────────────────────────

    #[test]
    fn class_declaration_snippet() {
        let src = "class Foo { public x: Int; }";
        assert_eq!(
            lex(src),
            vec![
                Token::Class,
                Token::Ident("Foo".into()),
                Token::LBrace,
                Token::Public,
                Token::Ident("x".into()),
                Token::Colon,
                Token::TInt,
                Token::Semicolon,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn let_assignment_snippet() {
        let src = "let x: Int = 42;";
        assert_eq!(
            lex(src),
            vec![
                Token::Let,
                Token::Ident("x".into()),
                Token::Colon,
                Token::TInt,
                Token::Equals,
                Token::IntLit(42),
                Token::Semicolon,
            ]
        );
    }
}
