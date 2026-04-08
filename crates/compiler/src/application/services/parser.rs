use crate::application::services::lexer::{Spanned, Token};
use crate::domain::ast::*;
use crate::domain::span::Span;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("Unexpected token {0:?} at line {1:?}")]
    Unexpected(Token, Span),
    #[error("Expected {0} but got {1:?} at {2:?}")]
    Expected(String, Token, Span),
    #[error("Unexpected end of file")]
    Eof,
}

impl ParseError {
    pub fn span(&self) -> Span {
        match self {
            ParseError::Unexpected(_, s) => *s,
            ParseError::Expected(_, _, s) => *s,
            ParseError::Eof => Span::dummy(),
        }
    }
}

pub struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
    /// Non-fatal errors accumulated during recovery (panic-mode).
    collected_errors: Vec<ParseError>,
}

impl Parser {
    pub fn new(tokens: Vec<Spanned>) -> Self {
        Parser { tokens, pos: 0, collected_errors: Vec::new() }
    }

    /// Errors collected via panic-mode recovery during the last parse call.
    /// These are in addition to any fatal error returned by `parse()`.
    pub fn collected_errors(&self) -> &[ParseError] {
        &self.collected_errors
    }

    // ── Token navigation ────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .map(|s| &s.token)
            .unwrap_or(&Token::Eof)
    }

    fn peek_spanned(&self) -> Spanned {
        self.tokens[self.pos.min(self.tokens.len().saturating_sub(1))].clone()
    }

    fn advance(&mut self) {
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ParseError> {
        let s = self.peek_spanned();
        if &s.token == expected {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::Expected(
                format!("{expected:?}"),
                s.token,
                s.span,
            ))
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        let s = self.peek_spanned();
        match s.token {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            // Contextual keywords: valid as identifiers when in identifier position.
            Token::Test  => { self.advance(); Ok("test".into()) }
            Token::Match => { self.advance(); Ok("match".into()) }
            Token::Enum  => { self.advance(); Ok("enum".into()) }
            Token::Async  => { self.advance(); Ok("async".into()) }
            Token::Server => { self.advance(); Ok("server".into()) }
            _ => Err(ParseError::Expected("identifier".into(), s.token, s.span)),
        }
    }

    // ── Dotted identifier (for package/import paths) ─────────────────────────
    // Parses: com.myapp.models.User  as a String
    fn parse_dotted_ident(&mut self) -> Result<String, ParseError> {
        let mut parts = vec![self.expect_ident()?];
        while self.peek() == &Token::Dot {
            // only consume the dot if followed by an ident or a contextual keyword
            if self
                .tokens
                .get(self.pos + 1)
                .map(|s| matches!(
                    s.token,
                    Token::Ident(_)
                        | Token::Test
                        | Token::Match
                        | Token::Enum
                        | Token::Async
                        | Token::Server
                ))
                .unwrap_or(false)
            {
                self.advance(); // dot
                parts.push(self.expect_ident()?);
            } else {
                break;
            }
        }
        Ok(parts.join("."))
    }

    // ── Entry point (a full .nx file) ────────────────────────────────────
    pub fn parse(&mut self) -> Result<Program, ParseError> {
        let mut package = None;
        let mut imports = Vec::new();

        // Optional: package declaration
        if self.peek() == &Token::Package {
            self.advance();
            package = Some(self.parse_dotted_ident()?);
            self.expect(&Token::Semicolon)?;
        }

        // Optional: import declarations
        while self.peek() == &Token::Import {
            self.advance();
            let path = self.parse_dotted_ident()?;
            self.expect(&Token::Semicolon)?;
            imports.push(ImportDecl { path });
        }

        // Mandatory: app block
        self.expect(&Token::App)?;
        let name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;

        let mut server = None;
        let mut declarations = Vec::new();
        let mut routes = Vec::new();

        loop {
            match self.peek() {
                Token::RBrace | Token::Eof => break,
                Token::Server => server = Some(self.parse_server()?),
                Token::Route => routes.push(self.parse_route()?),
                Token::Public
                | Token::Private
                | Token::Class
                | Token::Interface
                | Token::Component
                | Token::Window
                | Token::Enum => {
                    declarations.push(self.parse_declaration()?);
                }
                _ => {
                    let s = self.peek_spanned();
                    return Err(ParseError::Unexpected(s.token, s.span));
                }
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(Program {
            name,
            package,
            imports,
            server,
            declarations,
            routes,
        })
    }

    /// Parse a library file: only package + declarations (no `app` block).
    pub fn parse_lib(&mut self) -> Result<Program, ParseError> {
        let mut package = None;
        let mut imports = Vec::new();
        let mut declarations = Vec::new();

        if self.peek() == &Token::Package {
            self.advance();
            package = Some(self.parse_dotted_ident()?);
            self.expect(&Token::Semicolon)?;
        }
        while self.peek() == &Token::Import {
            self.advance();
            let path = self.parse_dotted_ident()?;
            self.expect(&Token::Semicolon)?;
            imports.push(ImportDecl { path });
        }
        while self.peek() != &Token::Eof {
            match self.peek() {
                Token::Test => {
                    declarations.push(self.parse_test_decl()?);
                }
                _ => {
                    declarations.push(self.parse_declaration()?);
                }
            }
        }
        Ok(Program {
            name: String::new(),
            package,
            imports,
            server: None,
            declarations,
            routes: vec![],
        })
    }

    // ── server block ────────────────────────────────────────────────────────
    fn parse_server(&mut self) -> Result<ServerConfig, ParseError> {
        self.expect(&Token::Server)?;
        self.expect(&Token::LBrace)?;
        let mut port = 3000u16;
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            let key = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            if key == "port" {
                if let Token::IntLit(n) = self.peek().clone() {
                    self.advance();
                    port = n as u16;
                }
            }
            self.expect(&Token::Semicolon)?;
        }
        self.expect(&Token::RBrace)?;
        Ok(ServerConfig { port })
    }

    // ── route ───────────────────────────────────────────────────────────────
    fn parse_route(&mut self) -> Result<Route, ParseError> {
        self.expect(&Token::Route)?;
        let path = if let Token::StringLit(s) = self.peek().clone() {
            self.advance();
            s
        } else {
            let s = self.peek_spanned();
            return Err(ParseError::Expected(
                "string literal".into(),
                s.token,
                s.span,
            ));
        };
        self.expect(&Token::FatArrow)?;
        let target = self.expect_ident()?;
        self.expect(&Token::Semicolon)?;
        Ok(Route { path, target })
    }

    // ── Visibility ──────────────────────────────────────────────────────────
    fn parse_visibility(&mut self) -> Visibility {
        match self.peek() {
            Token::Public => {
                self.advance();
                Visibility::Public
            }
            Token::Private => {
                self.advance();
                Visibility::Private
            }
            _ => Visibility::Public,
        }
    }

    // ── Declaration (class | interface | enum) ──────────────────────────────
    fn parse_declaration(&mut self) -> Result<Declaration, ParseError> {
        let vis = self.parse_visibility();
        match self.peek() {
            Token::Interface => {
                let mut iface = self.parse_interface()?;
                iface.visibility = vis;
                Ok(Declaration::Interface(iface))
            }
            Token::Class | Token::Component | Token::Window => {
                let mut cls = self.parse_class()?;
                cls.visibility = vis;
                Ok(Declaration::Class(cls))
            }
            Token::Enum => {
                let mut en = self.parse_enum()?;
                en.visibility = vis;
                Ok(Declaration::Enum(en))
            }
            _ => {
                let s = self.peek_spanned();
                Err(ParseError::Unexpected(s.token, s.span))
            }
        }
    }

    // ── Enum declaration ─────────────────────────────────────────────────────
    // enum Color { Red, Green, Blue }
    // enum Shape { Circle(Int), Rectangle(Int, Int), Point }
    fn parse_enum(&mut self) -> Result<EnumDecl, ParseError> {
        self.expect(&Token::Enum)?;
        let name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;
        let mut variants = Vec::new();

        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            let vname = self.expect_ident()?;
            let fields = if self.peek() == &Token::LParen {
                self.advance(); // (
                let mut flds = Vec::new();
                while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                    flds.push(self.parse_type()?);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                self.expect(&Token::RParen)?;
                flds
            } else {
                vec![]
            };
            variants.push(EnumVariant { name: vname, fields });
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(EnumDecl {
            visibility: Visibility::Public,
            name,
            variants,
        })
    }

    // ── Test declaration ─────────────────────────────────────────────────────
    // test "description" { stmts }
    fn parse_test_decl(&mut self) -> Result<Declaration, ParseError> {
        self.expect(&Token::Test)?;
        let name = if let Token::StringLit(s) = self.peek().clone() {
            self.advance();
            s
        } else {
            let s = self.peek_spanned();
            return Err(ParseError::Expected(
                "test name string".into(),
                s.token,
                s.span,
            ));
        };
        self.expect(&Token::LBrace)?;
        let body = self.parse_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Declaration::Test(TestDecl { name, body }))
    }

    // ── Generic type params <T, U> ──────────────────────────────────────────
    fn parse_type_params(&mut self) -> Result<Vec<String>, ParseError> {
        if self.peek() != &Token::LAngle {
            return Ok(vec![]);
        }
        self.advance(); // <
        let mut params = vec![self.expect_ident()?];
        while self.peek() == &Token::Comma {
            self.advance();
            params.push(self.expect_ident()?);
        }
        self.expect(&Token::RAngle)?;
        Ok(params)
    }

    // ── Type parsing ────────────────────────────────────────────────────────
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        // Function type: (T, U) => RetType
        if self.peek() == &Token::LParen {
            self.advance();
            let mut param_types = Vec::new();
            while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
                param_types.push(self.parse_type()?);
                if self.peek() == &Token::Comma {
                    self.advance();
                }
            }
            self.expect(&Token::RParen)?;
            self.expect(&Token::FatArrow)?;
            let ret = self.parse_type()?;
            return Ok(Type::Function(param_types, Box::new(ret)));
        }
        match self.peek().clone() {
            Token::TInt => {
                self.advance();
                Ok(Type::Int)
            }
            Token::TString => {
                self.advance();
                Ok(Type::String)
            }
            Token::TBool => {
                self.advance();
                Ok(Type::Bool)
            }
            Token::TVoid => {
                self.advance();
                Ok(Type::Void)
            }
            Token::TList => {
                self.advance();
                self.expect(&Token::LAngle)?;
                let inner = self.parse_type()?;
                self.expect(&Token::RAngle)?;
                Ok(Type::List(Box::new(inner)))
            }
            Token::Ident(name) => {
                self.advance();
                if self.peek() == &Token::LAngle {
                    self.advance();
                    self.parse_type()?; // consume inner generic (ignored structurally)
                    self.expect(&Token::RAngle)?;
                }
                Ok(Type::Custom(name))
            }
            _ => {
                let s = self.peek_spanned();
                Err(ParseError::Expected("type".into(), s.token, s.span))
            }
        }
    }

    // ── Parameter list ──────────────────────────────────────────────────────
    fn parse_params(&mut self) -> Result<Vec<Param>, ParseError> {
        self.expect(&Token::LParen)?;
        let mut params = Vec::new();
        while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
            let name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty });
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        self.expect(&Token::RParen)?;
        Ok(params)
    }

    // ── Class / Component / Window ──────────────────────────────────────────
    fn parse_class(&mut self) -> Result<ClassDecl, ParseError> {
        let kind = match self.peek() {
            Token::Class => {
                self.advance();
                ClassKind::Class
            }
            Token::Component => {
                self.advance();
                ClassKind::Component
            }
            Token::Window => {
                self.advance();
                ClassKind::Window
            }
            _ => {
                let s = self.peek_spanned();
                return Err(ParseError::Unexpected(s.token, s.span));
            }
        };
        let name = self.expect_ident()?;
        let type_params = self.parse_type_params()?;

        let mut extends = None;
        if self.peek() == &Token::Extends {
            self.advance();
            extends = Some(self.expect_ident()?);
        }
        let mut implements = Vec::new();
        if self.peek() == &Token::Implements {
            self.advance();
            implements.push(self.expect_ident()?);
            while self.peek() == &Token::Comma {
                self.advance();
                implements.push(self.expect_ident()?);
            }
        }

        self.expect(&Token::LBrace)?;
        let mut fields = Vec::new();
        let mut constructor = None;
        let mut methods = Vec::new();

        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            match self.peek().clone() {
                Token::Constructor => {
                    self.advance();
                    let params = self.parse_params()?;
                    self.expect(&Token::LBrace)?;
                    let body = self.parse_body()?;
                    self.expect(&Token::RBrace)?;
                    constructor = Some(Constructor { params, body });
                }
                Token::Public | Token::Private => {
                    let vis = self.parse_visibility();
                    if self.is_type_start() || self.is_custom_type_field_start() {
                        let ty = self.parse_type()?;
                        let fname = self.expect_ident()?;
                        self.expect(&Token::Semicolon)?;
                        fields.push(Field {
                            visibility: vis,
                            ty,
                            name: fname,
                        });
                    } else {
                        // Handles both `public async fn` and `public fn`
                        methods.push(self.parse_method(vis)?);
                    }
                }
                _ if self.is_type_start() || self.is_custom_type_field_start() => {
                    let ty = self.parse_type()?;
                    let fname = self.expect_ident()?;
                    self.expect(&Token::Semicolon)?;
                    fields.push(Field {
                        visibility: Visibility::Private,
                        ty,
                        name: fname,
                    });
                }
                Token::Async | Token::Ident(_) => {
                    methods.push(self.parse_method(Visibility::Public)?);
                }
                _ => {
                    let s = self.peek_spanned();
                    return Err(ParseError::Unexpected(s.token, s.span));
                }
            }
        }
        self.expect(&Token::RBrace)?;
        Ok(ClassDecl {
            visibility: Visibility::Public,
            kind,
            name,
            type_params,
            extends,
            implements,
            fields,
            constructor,
            methods,
        })
    }

    fn is_type_start(&self) -> bool {
        matches!(
            self.peek(),
            Token::TInt | Token::TString | Token::TBool | Token::TVoid | Token::TList
                | Token::LParen  // function-typed field: (T) => R fieldName;
        )
    }

    /// Returns `true` when the current token is a custom-type identifier that starts
    /// a **field declaration** (not a method).  Uses 2-token lookahead: the current
    /// token must be an `Ident` (the type name, e.g. `T` or `MyClass`) and the next
    /// token must also be an `Ident` (the field name).  This distinguishes
    /// `private T value;` (field) from `private render()` (method).
    fn is_custom_type_field_start(&self) -> bool {
        if matches!(self.peek(), Token::Ident(_)) {
            if let Some(next) = self.tokens.get(self.pos + 1) {
                return matches!(next.token, Token::Ident(_));
            }
        }
        false
    }

    fn parse_method(&mut self, vis: Visibility) -> Result<Method, ParseError> {
        // Optional `async` modifier before the method name.
        let is_async = if self.peek() == &Token::Async {
            self.advance();
            true
        } else {
            false
        };
        let name = self.expect_ident()?;
        // Optional generic type params: method<T, U>(...)  — consumed and discarded.
        if self.peek() == &Token::LAngle {
            self.advance();
            while self.peek() != &Token::RAngle && self.peek() != &Token::Eof {
                self.advance();
            }
            self.expect(&Token::RAngle)?;
        }
        let params = self.parse_params()?;
        self.expect(&Token::FatArrow)?;
        let return_type = self.parse_type()?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_body()?;
        self.expect(&Token::RBrace)?;
        Ok(Method {
            visibility: vis,
            is_async,
            name,
            params,
            return_type,
            body,
        })
    }

    // ── Interface ───────────────────────────────────────────────────────────
    fn parse_interface(&mut self) -> Result<InterfaceDecl, ParseError> {
        self.expect(&Token::Interface)?;
        let name = self.expect_ident()?;
        let type_params = self.parse_type_params()?;
        self.expect(&Token::LBrace)?;
        let mut methods = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            let method_name = self.expect_ident()?;
            let params = self.parse_params()?;
            self.expect(&Token::FatArrow)?;
            let return_type = self.parse_type()?;
            self.expect(&Token::Semicolon)?;
            methods.push(MethodSig {
                name: method_name,
                params,
                return_type,
            });
        }
        self.expect(&Token::RBrace)?;
        Ok(InterfaceDecl {
            visibility: Visibility::Public,
            name,
            type_params,
            methods,
        })
    }

    // ── Panic-mode recovery ──────────────────────────────────────────────────
    /// Skip tokens until we reach a likely statement boundary (`;` or `}`)
    /// so that parsing can resume after an error.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                Token::Eof | Token::RBrace => return,
                Token::Semicolon => {
                    self.advance();
                    return;
                }
                // Keyword that starts a new statement — stop before consuming it
                Token::Return
                | Token::Let
                | Token::If
                | Token::While
                | Token::For
                | Token::Break
                | Token::Continue
                | Token::Match => return,
                _ => { self.advance(); }
            }
        }
    }

    // ── Statement body ──────────────────────────────────────────────────────
    /// Parse a sequence of statements, recovering from errors at statement
    /// boundaries instead of aborting on the first failure.
    fn parse_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            match self.parse_stmt() {
                Ok(stmt) => stmts.push(stmt),
                Err(e) => {
                    // Record the error and skip to the next safe boundary.
                    self.collected_errors.push(e);
                    self.synchronize();
                }
            }
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.peek().clone() {
            // return [expr];
            Token::Return => {
                let span = self.peek_spanned().span;
                self.advance();
                if self.peek() == &Token::Semicolon {
                    self.advance();
                    return Ok(Stmt::Return { expr: None, span });
                }
                let expr = self.parse_expr()?;
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Return { expr: Some(expr), span })
            }

            // let name [: Type] = expr;
            Token::Let => {
                self.advance();
                let name = self.expect_ident()?;
                let ty = if self.peek() == &Token::Colon {
                    self.advance();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.expect(&Token::Equals)?;
                let init = self.parse_expr()?;
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Let { name, ty, init })
            }

            // if (cond) { ... } [else { ... }]
            Token::If => {
                self.advance();
                self.expect(&Token::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                self.expect(&Token::LBrace)?;
                let then_body = self.parse_body()?;
                self.expect(&Token::RBrace)?;
                let else_body = if self.peek() == &Token::Else {
                    self.advance();
                    self.expect(&Token::LBrace)?;
                    let b = self.parse_body()?;
                    self.expect(&Token::RBrace)?;
                    Some(b)
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then_body,
                    else_body,
                })
            }

            // while (cond) { body }
            Token::While => {
                self.advance();
                self.expect(&Token::LParen)?;
                let cond = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                self.expect(&Token::LBrace)?;
                let body = self.parse_body()?;
                self.expect(&Token::RBrace)?;
                Ok(Stmt::While { cond, body })
            }

            // for (var in iter) { body }
            Token::For => {
                self.advance();
                self.expect(&Token::LParen)?;
                let var = self.expect_ident()?;
                self.expect(&Token::In)?;
                let iter = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                self.expect(&Token::LBrace)?;
                let body = self.parse_body()?;
                self.expect(&Token::RBrace)?;
                Ok(Stmt::For { var, iter, body })
            }

            Token::Break => {
                self.advance();
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Break)
            }

            Token::Continue => {
                self.advance();
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Continue)
            }

            // match (expr) { pattern => { body } ... }
            Token::Match => {
                self.parse_match_stmt()
            }

            // expr;  — or  ident = expr;  — or  this.field = expr;
            _ => {
                let expr = self.parse_expr()?;
                if self.peek() == &Token::Equals {
                    match expr {
                        // ident = value;  (local variable assignment)
                        Expr::Ident(name) => {
                            self.advance(); // =
                            let value = self.parse_expr()?;
                            self.expect(&Token::Semicolon)?;
                            return Ok(Stmt::Assign {
                                object: Expr::Ident(name.clone()),
                                field: name,
                                value,
                            });
                        }
                        // this.field = value;  (field assignment)
                        Expr::FieldAccess(ref obj, ref field)
                            if matches!(**obj, Expr::This) =>
                        {
                            let field = field.clone();
                            self.advance(); // =
                            let value = self.parse_expr()?;
                            self.expect(&Token::Semicolon)?;
                            return Ok(Stmt::Assign {
                                object: Expr::This,
                                field,
                                value,
                            });
                        }
                        _ => {}
                    }
                }
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Expr(expr))
            }
        }
    }

    // ── Expression parsing (Pratt / precedence climbing) ───────────────────

    pub fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_binary(0)
    }

    fn parse_binary(&mut self, min_bp: u8) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;

        while let Some(op) = self.token_to_binop() {
            let (l_bp, r_bp) = op.binding_power();
            if l_bp < min_bp {
                break;
            }
            self.advance(); // consume operator
            let right = self.parse_binary(r_bp)?;
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn token_to_binop(&self) -> Option<BinOp> {
        match self.peek() {
            Token::Or => Some(BinOp::Or),
            Token::And => Some(BinOp::And),
            Token::EqualEqual => Some(BinOp::Eq),
            Token::BangEqual => Some(BinOp::Ne),
            Token::LAngle => Some(BinOp::Lt),
            Token::RAngle => Some(BinOp::Gt),
            Token::LessEqual => Some(BinOp::Le),
            Token::GreaterEqual => Some(BinOp::Ge),
            Token::Plus => Some(BinOp::Add),
            Token::Minus => Some(BinOp::Sub),
            Token::Star => Some(BinOp::Mul),
            Token::Slash => Some(BinOp::Div),
            Token::Percent => Some(BinOp::Mod),
            _ => None,
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Token::Bang => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(expr),
                })
            }
            // `await expr`
            Token::Await => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Await(Box::new(expr)))
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.peek() == &Token::Dot {
                self.advance();
                let field = self.expect_ident()?;
                if self.peek() == &Token::LParen {
                    let args = self.parse_call_args()?;
                    expr = Expr::MethodCall {
                        receiver: Box::new(expr),
                        method: field,
                        args,
                    };
                } else {
                    expr = Expr::FieldAccess(Box::new(expr), field);
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        // Check for single-ident lambda: ident => expr
        if let Token::Ident(name) = self.peek().clone() {
            if self.tokens.get(self.pos + 1).map(|s| &s.token) == Some(&Token::FatArrow) {
                self.advance(); // ident
                self.advance(); // =>
                let body = self.parse_expr()?;
                return Ok(Expr::Lambda {
                    params: vec![Param {
                        name,
                        ty: Type::Custom("T".into()),
                    }],
                    body: Box::new(body),
                });
            }
        }

        match self.peek().clone() {
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::StringLit(s))
            }
            Token::IntLit(n) => {
                self.advance();
                Ok(Expr::IntLit(n))
            }
            Token::BoolLit(b) => {
                self.advance();
                Ok(Expr::BoolLit(b))
            }
            Token::This => {
                self.advance();
                Ok(Expr::This)
            }

            // Parenthesised expression
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }

            // `[e1, e2, ...]`  — list literal
            Token::LBracket => {
                self.advance();
                let mut items = Vec::new();
                while self.peek() != &Token::RBracket && self.peek() != &Token::Eof {
                    items.push(self.parse_expr()?);
                    if self.peek() == &Token::Comma {
                        self.advance();
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::ListLiteral(items))
            }

            // `import("path")`  — dynamic lazy import
            Token::Import => {
                self.advance();
                self.expect(&Token::LParen)?;
                let path = if let Token::StringLit(s) = self.peek().clone() {
                    self.advance();
                    s
                } else {
                    let s = self.peek_spanned();
                    return Err(ParseError::Expected(
                        "string literal".into(),
                        s.token,
                        s.span,
                    ));
                };
                self.expect(&Token::RParen)?;
                Ok(Expr::LazyImport(path))
            }

            Token::Ident(name) => {
                self.advance();
                // Optional generic type args: Callee<T1, T2>(args) — parsed and erased.
                // Heuristic to distinguish `Box<Int>(x)` from `a < b`:
                //   - primitive type keywords → type arg
                //   - uppercase-starting ident (PascalCase class name or single-letter
                //     type param `T`, `U`, …) → type arg
                //   - lowercase ident (variable name) → comparison operator
                if self.peek() == &Token::LAngle {
                    let lookahead = self.tokens.get(self.pos + 1).map(|s| &s.token);
                    let is_type_arg = match lookahead {
                        Some(
                            Token::TInt | Token::TString | Token::TBool
                            | Token::TVoid | Token::TList,
                        ) => true,
                        Some(Token::Ident(s)) => {
                            s.starts_with(|c: char| c.is_uppercase())
                        }
                        _ => false,
                    };
                    if is_type_arg {
                        self.parse_call_type_args()?; // consume and discard
                    }
                }
                if self.peek() == &Token::LParen {
                    // Function / constructor call
                    let args = self.parse_call_args()?;
                    Ok(Expr::Call { callee: name, args })
                } else if self.peek() == &Token::LBrace {
                    // JSX-like block: Tag { child1; child2; }
                    self.advance(); // {
                    let mut children = Vec::new();
                    while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
                        children.push(self.parse_expr()?);
                        if self.peek() == &Token::Semicolon {
                            self.advance();
                        }
                    }
                    self.expect(&Token::RBrace)?;
                    Ok(Expr::Block {
                        tag: name,
                        children,
                    })
                } else {
                    Ok(Expr::Ident(name))
                }
            }

            _ => {
                let s = self.peek_spanned();
                Err(ParseError::Unexpected(s.token, s.span))
            }
        }
    }

    /// Consume and discard `<T1, T2, ...>` at a call site (type erasure).
    fn parse_call_type_args(&mut self) -> Result<(), ParseError> {
        self.expect(&Token::LAngle)?;
        let mut depth = 1u32;
        while depth > 0 && self.peek() != &Token::Eof {
            match self.peek() {
                Token::LAngle => { depth += 1; self.advance(); }
                Token::RAngle => { depth -= 1; self.advance(); }
                _ => { self.advance(); }
            }
        }
        Ok(())
    }

    fn parse_call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        self.expect(&Token::LParen)?;
        let mut args = Vec::new();
        while self.peek() != &Token::RParen && self.peek() != &Token::Eof {
            args.push(self.parse_expr()?);
            if self.peek() == &Token::Comma {
                self.advance();
            }
        }
        self.expect(&Token::RParen)?;
        Ok(args)
    }

    // ── match statement ──────────────────────────────────────────────────────
    // match (expr) {
    //     PatternA => { body }
    //     PatternB => { body }
    //     _        => { body }
    // }
    fn parse_match_stmt(&mut self) -> Result<Stmt, ParseError> {
        self.expect(&Token::Match)?;
        self.expect(&Token::LParen)?;
        let expr = self.parse_expr()?;
        self.expect(&Token::RParen)?;
        self.expect(&Token::LBrace)?;

        let mut arms = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            let pattern = self.parse_pattern()?;
            self.expect(&Token::FatArrow)?;
            self.expect(&Token::LBrace)?;
            let body = self.parse_body()?;
            self.expect(&Token::RBrace)?;
            arms.push(MatchArm { pattern, body });
        }
        self.expect(&Token::RBrace)?;
        Ok(Stmt::Match { expr, arms })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            Token::BoolLit(b) => {
                self.advance();
                Ok(Pattern::LitBool(b))
            }
            Token::IntLit(n) => {
                self.advance();
                Ok(Pattern::LitInt(n))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Pattern::LitStr(s))
            }
            Token::Ident(name) => {
                self.advance();
                // Check for qualified variant: EnumName.VariantName
                if self.peek() == &Token::Dot {
                    // Lookahead: is next an ident?
                    if self
                        .tokens
                        .get(self.pos + 1)
                        .map(|s| matches!(s.token, Token::Ident(_)))
                        .unwrap_or(false)
                    {
                        self.advance(); // dot
                        let variant = self.expect_ident()?;
                        return Ok(Pattern::QualifiedVariant {
                            enum_name: name,
                            variant,
                        });
                    }
                }
                // `_` is the wildcard; any other bare name is a variant/binding
                if name == "_" {
                    Ok(Pattern::Wildcard)
                } else {
                    Ok(Pattern::Name(name))
                }
            }
            _ => {
                let s = self.peek_spanned();
                Err(ParseError::Expected("pattern".into(), s.token, s.span))
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::lexer::Lexer;
    use crate::domain::ast::*;

    fn parse(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        Parser::new(tokens).parse().expect("parse error")
    }

    fn parse_err(src: &str) -> ParseError {
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        Parser::new(tokens).parse().unwrap_err()
    }

    fn parse_lib(src: &str) -> Program {
        let tokens = Lexer::new(src).tokenize().expect("lex error");
        Parser::new(tokens).parse_lib().expect("parse_lib error")
    }

    // ── app block ─────────────────────────────────────────────────────────────

    #[test]
    fn minimal_app() {
        let p = parse("app MyApp { }");
        assert_eq!(p.name, "MyApp");
        assert!(p.declarations.is_empty());
        assert!(p.routes.is_empty());
        assert!(p.server.is_none());
    }

    #[test]
    fn app_with_server_block() {
        let p = parse("app MyApp { server { port: 8080; } }");
        assert_eq!(p.server.unwrap().port, 8080);
    }

    #[test]
    fn app_with_route() {
        let p = parse(r#"app MyApp { route "/" => HomeWindow; }"#);
        assert_eq!(p.routes.len(), 1);
        assert_eq!(p.routes[0].path, "/");
        assert_eq!(p.routes[0].target, "HomeWindow");
    }

    // ── package / import ──────────────────────────────────────────────────────

    #[test]
    fn package_declaration() {
        let p = parse("package com.myapp; app A { }");
        assert_eq!(p.package.unwrap(), "com.myapp");
    }

    #[test]
    fn import_declaration() {
        let p = parse("import com.ui.Button; app A { }");
        assert_eq!(p.imports.len(), 1);
        assert_eq!(p.imports[0].path, "com.ui.Button");
    }

    #[test]
    fn multiple_imports() {
        let p = parse("import a.B; import c.D; app A { }");
        assert_eq!(p.imports.len(), 2);
    }

    // ── class declaration ─────────────────────────────────────────────────────

    #[test]
    fn empty_class() {
        let p = parse("app A { class Foo { } }");
        assert_eq!(p.declarations.len(), 1);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert_eq!(cls.name, "Foo");
            assert!(matches!(cls.kind, ClassKind::Class));
        } else {
            panic!("expected class");
        }
    }

    #[test]
    fn class_with_field() {
        let p = parse("app A { class Foo { public Int x; } }");
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert_eq!(cls.fields.len(), 1);
            assert_eq!(cls.fields[0].name, "x");
            assert!(matches!(cls.fields[0].ty, Type::Int));
        } else {
            panic!();
        }
    }

    #[test]
    fn class_with_constructor_and_method() {
        let src = r#"
            app A {
                class Counter {
                    constructor(start: Int) { this.count = start; }
                    get() => Int { return this.count; }
                }
            }
        "#;
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(cls.constructor.is_some());
            assert_eq!(cls.methods.len(), 1);
            assert_eq!(cls.methods[0].name, "get");
        } else {
            panic!();
        }
    }

    #[test]
    fn class_with_type_params() {
        let p = parse("app A { class Box<T> { } }");
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert_eq!(cls.type_params, vec!["T"]);
        } else {
            panic!();
        }
    }

    #[test]
    fn class_extends_implements() {
        let p = parse("app A { class Dog extends Animal implements Pet { } }");
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert_eq!(cls.extends.as_deref(), Some("Animal"));
            assert!(cls.implements.contains(&"Pet".to_string()));
        } else {
            panic!();
        }
    }

    #[test]
    fn window_declaration() {
        let p = parse("app A { window HomeWindow { } }");
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(matches!(cls.kind, ClassKind::Window));
        } else {
            panic!();
        }
    }

    #[test]
    fn component_declaration() {
        let p = parse("app A { component Button { } }");
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(matches!(cls.kind, ClassKind::Component));
        } else {
            panic!();
        }
    }

    // ── interface ─────────────────────────────────────────────────────────────

    #[test]
    fn interface_with_method_sig() {
        let p = parse("app A { interface Printable { print() => Void; } }");
        if let Declaration::Interface(iface) = &p.declarations[0] {
            assert_eq!(iface.name, "Printable");
            assert_eq!(iface.methods.len(), 1);
            assert_eq!(iface.methods[0].name, "print");
            assert!(matches!(iface.methods[0].return_type, Type::Void));
        } else {
            panic!();
        }
    }

    // ── statements ────────────────────────────────────────────────────────────

    #[test]
    fn let_with_type_annotation() {
        let src = "app A { class C { run() => Void { let x: Int = 1; } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { name, ty, .. } = &cls.methods[0].body[0] {
                assert_eq!(name, "x");
                assert!(matches!(ty, Some(Type::Int)));
            } else {
                panic!();
            }
        }
    }

    #[test]
    fn if_else_statement() {
        let src = "app A { class C { run() => Void { if (true) { return; } else { return; } } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(matches!(cls.methods[0].body[0], Stmt::If { else_body: Some(_), .. }));
        }
    }

    #[test]
    fn while_statement() {
        let src = "app A { class C { run() => Void { while (true) { break; } } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(matches!(cls.methods[0].body[0], Stmt::While { .. }));
        }
    }

    #[test]
    fn for_in_statement() {
        let src = "app A { class C { run() => Void { for (x in items) { continue; } } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(matches!(cls.methods[0].body[0], Stmt::For { .. }));
        }
    }

    // ── expressions ───────────────────────────────────────────────────────────

    #[test]
    fn binary_arithmetic_expression() {
        let src = "app A { class C { run() => Int { return 1 + 2 * 3; } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Return { expr: Some(Expr::Binary { op: BinOp::Add, .. }), .. } = &cls.methods[0].body[0] {
                // correct — addition is the outermost (lower precedence than *)
            } else {
                panic!("expected binary add at top level");
            }
        }
    }

    #[test]
    fn method_call_chain() {
        // receiver is a local ident, not `this`, so it goes through parse_expr
        let src = r#"app A { class C { run() => Void { let r = obj.foo().bar(); } } }"#;
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::MethodCall { method, .. }, .. } = &cls.methods[0].body[0] {
                assert_eq!(method, "bar");
            } else {
                panic!("expected method call chain");
            }
        }
    }

    // ── lib parse ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_lib_extracts_declarations() {
        let p = parse_lib("class Util { } class Helper { }");
        assert_eq!(p.declarations.len(), 2);
    }

    // ── async / await ─────────────────────────────────────────────────────────

    #[test]
    fn async_method_sets_flag() {
        let src = "app A { class C { async fetch() => Void { } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(cls.methods[0].is_async, "fetch should be marked async");
        }
    }

    #[test]
    fn sync_method_not_async() {
        let src = "app A { class C { run() => Void { } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            assert!(!cls.methods[0].is_async);
        }
    }

    #[test]
    fn await_expr_in_async_method() {
        let src = r#"app A { class C { async load() => Void { let r = await fetch("url"); } } }"#;
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init, .. } = &cls.methods[0].body[0] {
                assert!(
                    matches!(init, Expr::Await(_)),
                    "expected Expr::Await, got {:?}", init
                );
            }
        }
    }

    // ── list literals ─────────────────────────────────────────────────────────

    #[test]
    fn list_literal_empty() {
        let src = "app A { class C { run() => Void { let xs = []; } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::ListLiteral(items), .. } = &cls.methods[0].body[0] {
                assert!(items.is_empty());
            } else {
                panic!("expected empty list literal");
            }
        }
    }

    #[test]
    fn list_literal_with_items() {
        let src = "app A { class C { run() => Void { let xs = [1, 2, 3]; } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::ListLiteral(items), .. } = &cls.methods[0].body[0] {
                assert_eq!(items.len(), 3);
                assert!(matches!(&items[0], Expr::IntLit(1)));
                assert!(matches!(&items[2], Expr::IntLit(3)));
            } else {
                panic!("expected list literal with 3 items");
            }
        }
    }

    // ── dynamic import ────────────────────────────────────────────────────────

    #[test]
    fn lazy_import_expr() {
        let src = r#"app A { class C { async load() => Void { let m = import("std.math.Math"); } } }"#;
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::LazyImport(path), .. } = &cls.methods[0].body[0] {
                assert_eq!(path, "std.math.Math");
            } else {
                panic!("expected LazyImport expr");
            }
        }
    }

    // ── generic type args at call site ────────────────────────────────────────

    #[test]
    fn generic_call_type_args_erased() {
        // Box<Int>(42) should produce Expr::Call { callee: "Box", args: [IntLit(42)] }
        let src = "app A { class C { run() => Void { let b = Box<Int>(42); } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::Call { callee, args }, .. } = &cls.methods[0].body[0] {
                assert_eq!(callee, "Box");
                assert_eq!(args.len(), 1);
            } else {
                panic!("expected Call after type arg erasure");
            }
        }
    }

    #[test]
    fn comparison_lt_not_confused_with_type_args() {
        // a < b must NOT be treated as type args
        let src = "app A { class C { run() => Void { let r = a < b; } } }";
        let p = parse(src);
        if let Declaration::Class(cls) = &p.declarations[0] {
            if let Stmt::Let { init: Expr::Binary { op: BinOp::Lt, .. }, .. } = &cls.methods[0].body[0] {
                // correct
            } else {
                panic!("a < b should produce BinOp::Lt, not be consumed as type args");
            }
        }
    }

    // ── error cases ───────────────────────────────────────────────────────────

    #[test]
    fn unexpected_token_returns_error() {
        matches!(parse_err("42"), ParseError::Unexpected(..));
    }

    #[test]
    fn missing_app_name_returns_error() {
        matches!(parse_err("app { }"), ParseError::Expected(..));
    }
}
