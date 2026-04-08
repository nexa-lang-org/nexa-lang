use crate::domain::span::Span;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    #[default]
    Private,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClassKind {
    Class,
    Component,
    Window,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Type {
    Int,
    String,
    Bool,
    Void,
    Custom(String),
    Generic(String),
    List(Box<Type>),
    Function(Vec<Type>, Box<Type>),
}

// ── Binary / Unary operators ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn as_js(&self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "===",
            BinOp::Ne => "!==",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    /// Binding power: higher = tighter. Returns (left_bp, right_bp).
    pub fn binding_power(&self) -> (u8, u8) {
        match self {
            BinOp::Or => (1, 2),
            BinOp::And => (3, 4),
            BinOp::Eq | BinOp::Ne => (5, 6),
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => (7, 8),
            BinOp::Add | BinOp::Sub => (9, 10),
            BinOp::Mul | BinOp::Div | BinOp::Mod => (11, 12),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnOp {
    Not,
    Neg,
}

// ── Top-level program ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub package: Option<String>,
    pub imports: Vec<ImportDecl>,
    pub server: Option<ServerConfig>,
    pub declarations: Vec<Declaration>,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportDecl {
    /// Fully-qualified path, e.g. "com.myapp.models.User"
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
}

// ── Enum / ADT ───────────────────────────────────────────────────────────────

/// A single variant of an enum, e.g. `Circle(Int)` or `Point`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumVariant {
    pub name: String,
    /// Field types for tuple-style variants; empty for unit variants.
    pub fields: Vec<Type>,
}

/// An algebraic data type (ADT) declaration.
/// Syntax: `enum Color { Red, Green, Blue }` or `enum Shape { Circle(Int), Point }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumDecl {
    pub visibility: Visibility,
    pub name: String,
    pub variants: Vec<EnumVariant>,
}

// ── Test block ───────────────────────────────────────────────────────────────

/// A named test block: `test "description" { stmts }`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestDecl {
    pub name: String,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Declaration {
    Class(ClassDecl),
    Interface(InterfaceDecl),
    Enum(EnumDecl),
    Test(TestDecl),
}

// ── Class / Component / Window ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassDecl {
    pub visibility: Visibility,
    pub kind: ClassKind,
    pub name: String,
    pub type_params: Vec<String>,
    pub extends: Option<String>,
    pub implements: Vec<String>,
    pub fields: Vec<Field>,
    pub constructor: Option<Constructor>,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub visibility: Visibility,
    pub ty: Type,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Method {
    pub visibility: Visibility,
    #[serde(default)]
    pub is_async: bool,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constructor {
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceDecl {
    pub visibility: Visibility,
    pub name: String,
    pub type_params: Vec<String>,
    pub methods: Vec<MethodSig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Type,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub path: String,
    pub target: String,
}

// ── Patterns (for match expressions) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Pattern {
    /// `_` — matches anything, binds nothing.
    Wildcard,
    /// A bare name that may be an enum variant OR a binding.
    /// The compiler resolves which at lower-time; for now both work.
    Name(String),
    /// `EnumName.Variant` — qualified enum variant pattern.
    QualifiedVariant { enum_name: String, variant: String },
    /// Literal `true` or `false`.
    LitBool(bool),
    /// Integer literal.
    LitInt(i64),
    /// String literal.
    LitStr(String),
}

/// One arm of a `match` statement: `pattern => { body }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Vec<Stmt>,
}

// ── Statements ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Stmt {
    Return {
        expr: Option<Expr>,
        /// Source location of the `return` keyword.
        /// Set to `Span::dummy()` when constructed synthetically (tests, NXB round-trips).
        #[serde(skip)]
        span: Span,
    },
    /// this.field = value  OR  ident = value
    Assign {
        object: Expr,
        field: String,
        value: Expr,
    },
    /// let name [: Type] = expr;
    Let {
        name: String,
        ty: Option<Type>,
        init: Expr,
    },
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// for (name in iter) { body }
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Expr(Expr),
    /// `match (expr) { pattern => { body } ... }`
    Match {
        expr: Expr,
        arms: Vec<MatchArm>,
    },
}

// ── Expressions ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    StringLit(String),
    IntLit(i64),
    BoolLit(bool),
    Ident(String),
    This,
    FieldAccess(Box<Expr>, String),
    MethodCall {
        receiver: Box<Expr>,
        method: String,
        args: Vec<Expr>,
    },
    Call {
        callee: String,
        args: Vec<Expr>,
    },
    Lambda {
        params: Vec<Param>,
        body: Box<Expr>,
    },
    /// JSX-like block: Tag { child1; child2; }
    Block {
        tag: String,
        children: Vec<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    /// `await expr`  — valid inside an async method, compiles to JS `await`.
    Await(Box<Expr>),
    /// `[e1, e2, ...]`  — list literal, compiles to JS array.
    ListLiteral(Vec<Expr>),
    /// `import("path.to.Module")`  — dynamic lazy import.
    LazyImport(String),
}
