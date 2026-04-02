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
}

impl Parser {
    pub fn new(tokens: Vec<Spanned>) -> Self {
        Parser { tokens, pos: 0 }
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
        if let Token::Ident(name) = s.token {
            self.advance();
            Ok(name)
        } else {
            Err(ParseError::Expected("identifier".into(), s.token, s.span))
        }
    }

    // ── Dotted identifier (for package/import paths) ─────────────────────────
    // Parses: com.myapp.models.User  as a String
    fn parse_dotted_ident(&mut self) -> Result<String, ParseError> {
        let mut parts = vec![self.expect_ident()?];
        while self.peek() == &Token::Dot {
            // only consume the dot if followed by an ident
            if self
                .tokens
                .get(self.pos + 1)
                .map(|s| matches!(s.token, Token::Ident(_)))
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
                | Token::Window => {
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
            declarations.push(self.parse_declaration()?);
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

    // ── Declaration (class | interface) ────────────────────────────────────
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
            _ => {
                let s = self.peek_spanned();
                Err(ParseError::Unexpected(s.token, s.span))
            }
        }
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
                    if self.is_type_start() {
                        let ty = self.parse_type()?;
                        let fname = self.expect_ident()?;
                        self.expect(&Token::Semicolon)?;
                        fields.push(Field {
                            visibility: vis,
                            ty,
                            name: fname,
                        });
                    } else {
                        methods.push(self.parse_method(vis)?);
                    }
                }
                _ if self.is_type_start() => {
                    let ty = self.parse_type()?;
                    let fname = self.expect_ident()?;
                    self.expect(&Token::Semicolon)?;
                    fields.push(Field {
                        visibility: Visibility::Private,
                        ty,
                        name: fname,
                    });
                }
                Token::Ident(_) => {
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
        )
    }

    fn parse_method(&mut self, vis: Visibility) -> Result<Method, ParseError> {
        let name = self.expect_ident()?;
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

    // ── Statement body ──────────────────────────────────────────────────────
    fn parse_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut stmts = Vec::new();
        while self.peek() != &Token::RBrace && self.peek() != &Token::Eof {
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ParseError> {
        match self.peek().clone() {
            // return [expr];
            Token::Return => {
                self.advance();
                if self.peek() == &Token::Semicolon {
                    self.advance();
                    return Ok(Stmt::Return(None));
                }
                let expr = self.parse_expr()?;
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Return(Some(expr)))
            }

            // this.field = expr;
            Token::This => {
                self.advance();
                self.expect(&Token::Dot)?;
                let field = self.expect_ident()?;
                self.expect(&Token::Equals)?;
                let value = self.parse_expr()?;
                self.expect(&Token::Semicolon)?;
                Ok(Stmt::Assign {
                    object: Expr::This,
                    field,
                    value,
                })
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

            // expr;  — or  ident = expr;  (local variable assignment)
            _ => {
                let expr = self.parse_expr()?;
                // If followed by `=`, it's a local assignment: ident = value;
                if self.peek() == &Token::Equals {
                    if let Expr::Ident(name) = expr {
                        self.advance(); // =
                        let value = self.parse_expr()?;
                        self.expect(&Token::Semicolon)?;
                        return Ok(Stmt::Assign {
                            object: Expr::Ident(name.clone()),
                            field: name,
                            value,
                        });
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

            Token::Ident(name) => {
                self.advance();
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
}
