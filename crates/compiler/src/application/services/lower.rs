//! AST → IR lowering pass.
//!
//! Converts a (post-optimization) `Program` into an `IrModule`.
//! Performs inline type inference for `let` bindings without annotations.

use std::collections::HashMap;

use crate::domain::ast::{
    BinOp, ClassKind, Declaration, Expr, Program, Stmt, Type, UnOp, Visibility,
};
use crate::domain::ir::*;

/// Per-scope type environment: maps local variable names to their inferred IR type.
type TypeEnv = HashMap<String, IrType>;

pub fn lower(program: &Program) -> IrModule {
    let mut classes = Vec::new();
    for decl in &program.declarations {
        if let Declaration::Class(cls) = decl {
            classes.push(lower_class(cls));
        }
        // Interfaces are structural types; not emitted as IR definitions for now.
    }
    let routes = program
        .routes
        .iter()
        .map(|r| IrRoute {
            path: r.path.clone(),
            target: r.target.clone(),
        })
        .collect();
    let server = program.server.as_ref().map(|s| IrServer { port: s.port });

    IrModule {
        name: program.name.clone(),
        server,
        classes,
        routes,
    }
}

fn lower_class(cls: &crate::domain::ast::ClassDecl) -> IrClass {
    let kind = match cls.kind {
        ClassKind::Class => IrClassKind::Class,
        ClassKind::Component => IrClassKind::Component,
        ClassKind::Window => IrClassKind::Window,
    };
    let fields = cls
        .fields
        .iter()
        .map(|f| IrField {
            name: f.name.clone(),
            ty: lower_type(&f.ty),
            is_public: f.visibility == Visibility::Public,
        })
        .collect();

    // Build a field-type env so constructor/method bodies can resolve `this.x`.
    let field_env: TypeEnv = cls
        .fields
        .iter()
        .map(|f| (f.name.clone(), lower_type(&f.ty)))
        .collect();

    let (ctor_params, ctor_body) = cls
        .constructor
        .as_ref()
        .map(|c| {
            let mut env = field_env.clone();
            for p in &c.params {
                env.insert(p.name.clone(), lower_type(&p.ty));
            }
            (
                c.params.iter().map(lower_param).collect(),
                lower_stmts_in_scope(&c.body, &env),
            )
        })
        .unwrap_or_default();

    // Keep field_env mutable so methods can use it as a base.
    let methods = cls
        .methods
        .iter()
        .map(|m| lower_method_with_env(m, &field_env))
        .collect();

    IrClass {
        name: cls.name.clone(),
        kind,
        is_public: cls.visibility == Visibility::Public,
        fields,
        constructor_params: ctor_params,
        constructor_body: ctor_body,
        methods,
    }
}

fn lower_method_with_env(
    m: &crate::domain::ast::Method,
    field_env: &TypeEnv,
) -> IrMethod {
    let mut env = field_env.clone();
    for p in &m.params {
        env.insert(p.name.clone(), lower_type(&p.ty));
    }
    IrMethod {
        name: m.name.clone(),
        params: m.params.iter().map(lower_param).collect(),
        return_ty: lower_type(&m.return_type),
        body: lower_stmts_in_scope(&m.body, &env),
        is_public: m.visibility == Visibility::Public,
        is_async: m.is_async,
    }
}

fn lower_param(p: &crate::domain::ast::Param) -> IrParam {
    IrParam {
        name: p.name.clone(),
        ty: lower_type(&p.ty),
    }
}

fn lower_type(ty: &Type) -> IrType {
    match ty {
        Type::Int => IrType::Int,
        Type::Bool => IrType::Bool,
        Type::String => IrType::String,
        Type::Void => IrType::Void,
        Type::Custom(n) => IrType::Named(n.clone()),
        Type::Generic(n) => IrType::Named(n.clone()),
        Type::List(inner) => IrType::List(Box::new(lower_type(inner))),
        Type::Function(params, ret) => IrType::Fn(
            params.iter().map(lower_type).collect(),
            Box::new(lower_type(ret)),
        ),
    }
}

// ── Type inference ────────────────────────────────────────────────────────────

/// Infer the IR type of an expression from the current type environment.
/// Returns `IrType::Unknown` when the type cannot be determined locally.
fn infer_expr_type(expr: &Expr, env: &TypeEnv) -> IrType {
    match expr {
        Expr::IntLit(_) => IrType::Int,
        Expr::StringLit(_) => IrType::String,
        Expr::BoolLit(_) => IrType::Bool,
        Expr::Ident(name) => env.get(name).cloned().unwrap_or(IrType::Unknown),
        Expr::Binary { op, left, right } => {
            let lt = infer_expr_type(left, env);
            let rt = infer_expr_type(right, env);
            match op {
                BinOp::Add => match (&lt, &rt) {
                    (IrType::Int, IrType::Int) => IrType::Int,
                    (IrType::String, _) | (_, IrType::String) => IrType::String,
                    _ => IrType::Unknown,
                },
                BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    if lt == IrType::Int && rt == IrType::Int {
                        IrType::Int
                    } else {
                        IrType::Unknown
                    }
                }
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => IrType::Bool,
            }
        }
        Expr::Unary { op, .. } => match op {
            UnOp::Not => IrType::Bool,
            UnOp::Neg => IrType::Int,
        },
        _ => IrType::Unknown,
    }
}

// ── Statement lowering ────────────────────────────────────────────────────────

/// Lower a slice of statements into a new child scope (inherits parent env,
/// but new bindings do not escape back to the parent).
fn lower_stmts_in_scope(stmts: &[Stmt], parent_env: &TypeEnv) -> Vec<IrStmt> {
    let mut env = parent_env.clone();
    stmts.iter().map(|s| lower_stmt(s, &mut env)).collect()
}

fn lower_stmt(stmt: &Stmt, env: &mut TypeEnv) -> IrStmt {
    match stmt {
        Stmt::Return(None) => IrStmt::Return(None),
        Stmt::Return(Some(e)) => IrStmt::Return(Some(lower_expr(e))),
        Stmt::Assign { object, field, value } => {
            let target = match object {
                Expr::This => IrExpr::Field {
                    receiver: Box::new(IrExpr::SelfRef),
                    name: field.clone(),
                },
                other => IrExpr::Field {
                    receiver: Box::new(lower_expr(other)),
                    name: field.clone(),
                },
            };
            IrStmt::Assign {
                target,
                value: lower_expr(value),
            }
        }
        Stmt::Let { name, ty, init } => {
            // Prefer the explicit annotation; fall back to inference.
            let resolved_ty = ty
                .as_ref()
                .map(lower_type)
                .unwrap_or_else(|| infer_expr_type(init, env));
            env.insert(name.clone(), resolved_ty.clone());
            IrStmt::Let {
                name: name.clone(),
                ty: resolved_ty,
                init: lower_expr(init),
            }
        }
        Stmt::If { cond, then_body, else_body } => IrStmt::If {
            cond: lower_expr(cond),
            then_body: lower_stmts_in_scope(then_body, env),
            else_body: else_body
                .as_ref()
                .map(|b| lower_stmts_in_scope(b, env)),
        },
        Stmt::While { cond, body } => IrStmt::While {
            cond: lower_expr(cond),
            body: lower_stmts_in_scope(body, env),
        },
        Stmt::For { var, iter, body } => {
            // The loop variable's element type is not yet tracked; use Unknown.
            let mut inner_env = env.clone();
            inner_env.insert(var.clone(), IrType::Unknown);
            IrStmt::For {
                var: var.clone(),
                iter: lower_expr(iter),
                body: lower_stmts_in_scope(body, &inner_env),
            }
        }
        Stmt::Break => IrStmt::Break,
        Stmt::Continue => IrStmt::Continue,
        Stmt::Expr(e) => IrStmt::Discard(lower_expr(e)),
    }
}

// ── Expression lowering ───────────────────────────────────────────────────────

fn lower_expr(expr: &Expr) -> IrExpr {
    match expr {
        Expr::StringLit(s) => IrExpr::Str(s.clone()),
        Expr::IntLit(n) => IrExpr::Int(*n),
        Expr::BoolLit(b) => IrExpr::Bool(*b),
        Expr::Ident(n) => IrExpr::Local(n.clone()),
        Expr::This => IrExpr::SelfRef,
        Expr::FieldAccess(obj, field) => IrExpr::Field {
            receiver: Box::new(lower_expr(obj)),
            name: field.clone(),
        },
        Expr::MethodCall { receiver, method, args } => IrExpr::Call {
            receiver: Box::new(lower_expr(receiver)),
            method: method.clone(),
            args: args.iter().map(lower_expr).collect(),
        },
        Expr::Call { callee, args } => IrExpr::Invoke {
            callee: callee.clone(),
            args: args.iter().map(lower_expr).collect(),
        },
        Expr::Lambda { params, body } => IrExpr::Closure {
            params: params.iter().map(lower_param).collect(),
            body: Box::new(lower_expr(body)),
        },
        Expr::Block { tag, children } => IrExpr::Node {
            tag: tag.clone(),
            children: children.iter().map(lower_expr).collect(),
        },
        Expr::Binary { op, left, right } => IrExpr::Bin {
            op: lower_binop(op),
            lhs: Box::new(lower_expr(left)),
            rhs: Box::new(lower_expr(right)),
        },
        Expr::Unary { op, expr } => IrExpr::Unary {
            op: match op {
                UnOp::Not => IrUnOp::Not,
                UnOp::Neg => IrUnOp::Neg,
            },
            operand: Box::new(lower_expr(expr)),
        },
        Expr::Await(inner) => IrExpr::Await(Box::new(lower_expr(inner))),
        Expr::ListLiteral(items) => IrExpr::List(items.iter().map(lower_expr).collect()),
        Expr::LazyImport(path) => IrExpr::DynamicImport(path.clone()),
    }
}

fn lower_binop(op: &BinOp) -> IrBinOp {
    match op {
        BinOp::Add => IrBinOp::Add,
        BinOp::Sub => IrBinOp::Sub,
        BinOp::Mul => IrBinOp::Mul,
        BinOp::Div => IrBinOp::Div,
        BinOp::Mod => IrBinOp::Mod,
        BinOp::Eq  => IrBinOp::Eq,
        BinOp::Ne  => IrBinOp::Ne,
        BinOp::Lt  => IrBinOp::Lt,
        BinOp::Gt  => IrBinOp::Gt,
        BinOp::Le  => IrBinOp::Le,
        BinOp::Ge  => IrBinOp::Ge,
        BinOp::And => IrBinOp::And,
        BinOp::Or  => IrBinOp::Or,
    }
}
