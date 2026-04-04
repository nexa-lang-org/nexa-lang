//! Nexa Intermediate Representation (NIR).
//!
//! The IR sits between the optimized AST and any backend (JS, WASM, native).
//! It is intentionally target-agnostic: no JS syntax, no HTML concepts.
//!
//! Pipeline:
//!   AST → Resolver → SemanticAnalyzer → Optimizer → **Lower (AST→IR)** → Backend
//!
//! A JS backend lowers IR→JS; a future WASM backend would lower IR→WAT, etc.

// ── Value types ───────────────────────────────────────────────────────────────

/// A resolved type in the IR.
#[derive(Debug, Clone, PartialEq)]
pub enum IrType {
    Int,
    Bool,
    String,
    Void,
    /// An opaque user-defined class/component/window.
    Named(String),
    /// A list of values.
    List(Box<IrType>),
    /// A callable: (params) → return_type.
    Fn(Vec<IrType>, Box<IrType>),
    /// Type not yet inferred (placeholder from lower pass).
    Unknown,
}

// ── Expressions ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum IrExpr {
    Int(i64),
    Bool(bool),
    Str(String),
    /// A local variable or parameter reference.
    Local(String),
    /// `self` / `this` reference.
    SelfRef,
    /// Field access: `receiver.field`.
    Field { receiver: Box<IrExpr>, name: String },
    /// Method call: `receiver.method(args)`.
    Call {
        receiver: Box<IrExpr>,
        method: String,
        args: Vec<IrExpr>,
    },
    /// Free function / constructor call: `callee(args)`.
    Invoke { callee: String, args: Vec<IrExpr> },
    /// Anonymous function: `|params| body`.
    Closure { params: Vec<IrParam>, body: Box<IrExpr> },
    /// UI node: a named tag with children (replaces AST Block).
    Node { tag: String, children: Vec<IrExpr> },
    /// Binary operation.
    Bin { op: IrBinOp, lhs: Box<IrExpr>, rhs: Box<IrExpr> },
    /// Unary operation.
    Unary { op: IrUnOp, operand: Box<IrExpr> },
    /// `await expr`  — inside an async method.
    Await(Box<IrExpr>),
    /// `[e1, e2, ...]`  — list / array literal.
    List(Vec<IrExpr>),
    /// `import("path")`  — dynamic lazy import.
    DynamicImport(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrBinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IrUnOp {
    Not,
    Neg,
}

// ── Statements ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum IrStmt {
    /// `let name: ty = init`
    Let { name: String, ty: IrType, init: IrExpr },
    /// `target = value` (field or local).
    Assign { target: IrExpr, value: IrExpr },
    /// `return expr?`
    Return(Option<IrExpr>),
    /// `if cond { then } else { else_ }`
    If { cond: IrExpr, then_body: Vec<IrStmt>, else_body: Option<Vec<IrStmt>> },
    /// `while cond { body }`
    While { cond: IrExpr, body: Vec<IrStmt> },
    /// `for var in iter { body }`
    For { var: String, iter: IrExpr, body: Vec<IrStmt> },
    Break,
    Continue,
    /// Expression used as statement (side effects).
    Discard(IrExpr),
}

// ── Top-level definitions ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IrParam {
    pub name: String,
    pub ty: IrType,
}

#[derive(Debug, Clone)]
pub struct IrMethod {
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: IrType,
    pub body: Vec<IrStmt>,
    pub is_public: bool,
    pub is_async: bool,
}

#[derive(Debug, Clone)]
pub struct IrField {
    pub name: String,
    pub ty: IrType,
    pub is_public: bool,
}

/// Kind of a class-like definition.
#[derive(Debug, Clone, PartialEq)]
pub enum IrClassKind {
    /// Plain data / logic class.
    Class,
    /// Reusable UI component.
    Component,
    /// Top-level page (routable).
    Window,
}

#[derive(Debug, Clone)]
pub struct IrClass {
    pub name: String,
    pub kind: IrClassKind,
    pub is_public: bool,
    pub fields: Vec<IrField>,
    pub constructor_params: Vec<IrParam>,
    pub constructor_body: Vec<IrStmt>,
    pub methods: Vec<IrMethod>,
}

/// A server port declaration.
#[derive(Debug, Clone)]
pub struct IrServer {
    pub port: u16,
}

/// A route binding: `path → window_class`.
#[derive(Debug, Clone)]
pub struct IrRoute {
    pub path: String,
    pub target: String,
}

/// The root IR module — one per Nexa source file / module.
#[derive(Debug, Clone)]
pub struct IrModule {
    /// Application name (from `app Foo { }` or package declaration).
    pub name: String,
    pub server: Option<IrServer>,
    pub classes: Vec<IrClass>,
    pub routes: Vec<IrRoute>,
}
