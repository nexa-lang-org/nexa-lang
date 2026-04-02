use std::collections::HashMap;
use std::collections::HashSet;

use crate::domain::ast::{BinOp, ClassKind, Declaration, Expr, Program, Stmt, Type, UnOp};

/// Run all optimization passes on a fully-resolved, semantically-valid Program.
pub fn optimize(program: Program) -> Program {
    let program = remove_dead_code(program);
    let program = inline_components(program);
    let program = flatten_tree(program);
    precompute_props(program)
}

// ── Pass 1: Dead code removal ────────────────────────────────────────────────

fn remove_dead_code(mut program: Program) -> Program {
    let mut live: HashSet<String> = HashSet::new();

    // Seed: all route targets are live
    for route in &program.routes {
        live.insert(route.target.clone());
    }

    // Collect names referenced in all declarations (transitive)
    // Repeat until no new names are added
    let mut changed = true;
    while changed {
        changed = false;
        for decl in &program.declarations {
            let name = decl_name(decl);
            if live.contains(name) {
                let before = live.len();
                collect_decl_names(decl, &mut live);
                if live.len() > before {
                    changed = true;
                }
            }
        }
    }

    program.declarations.retain(|d| live.contains(decl_name(d)));
    program
}

fn decl_name(decl: &Declaration) -> &str {
    match decl {
        Declaration::Class(c) => &c.name,
        Declaration::Interface(i) => &i.name,
    }
}

fn collect_decl_names(decl: &Declaration, out: &mut HashSet<String>) {
    match decl {
        Declaration::Class(c) => {
            if let Some(e) = &c.extends {
                out.insert(e.clone());
            }
            for i in &c.implements {
                out.insert(i.clone());
            }
            for f in &c.fields {
                collect_type_names(&f.ty, out);
            }
            if let Some(ctor) = &c.constructor {
                for p in &ctor.params {
                    collect_type_names(&p.ty, out);
                }
                for s in &ctor.body {
                    collect_stmt_names(s, out);
                }
            }
            for m in &c.methods {
                collect_type_names(&m.return_type, out);
                for p in &m.params {
                    collect_type_names(&p.ty, out);
                }
                for s in &m.body {
                    collect_stmt_names(s, out);
                }
            }
        }
        Declaration::Interface(i) => {
            for sig in &i.methods {
                collect_type_names(&sig.return_type, out);
                for p in &sig.params {
                    collect_type_names(&p.ty, out);
                }
            }
        }
    }
}

fn collect_type_names(ty: &Type, out: &mut HashSet<String>) {
    match ty {
        Type::Custom(n) | Type::Generic(n) => {
            out.insert(n.clone());
        }
        Type::List(inner) => collect_type_names(inner, out),
        Type::Function(params, ret) => {
            for p in params {
                collect_type_names(p, out);
            }
            collect_type_names(ret, out);
        }
        _ => {}
    }
}

fn collect_stmt_names(stmt: &Stmt, out: &mut HashSet<String>) {
    match stmt {
        Stmt::Return(Some(e)) => collect_expr_names(e, out),
        Stmt::Return(None) => {}
        Stmt::Assign { object, value, .. } => {
            collect_expr_names(object, out);
            collect_expr_names(value, out);
        }
        Stmt::Let { init, ty, .. } => {
            collect_expr_names(init, out);
            if let Some(t) = ty {
                collect_type_names(t, out);
            }
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
        } => {
            collect_expr_names(cond, out);
            for s in then_body {
                collect_stmt_names(s, out);
            }
            if let Some(eb) = else_body {
                for s in eb {
                    collect_stmt_names(s, out);
                }
            }
        }
        Stmt::While { cond, body } => {
            collect_expr_names(cond, out);
            for s in body {
                collect_stmt_names(s, out);
            }
        }
        Stmt::For { iter, body, .. } => {
            collect_expr_names(iter, out);
            for s in body {
                collect_stmt_names(s, out);
            }
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Expr(e) => collect_expr_names(e, out),
    }
}

fn collect_expr_names(expr: &Expr, out: &mut HashSet<String>) {
    match expr {
        Expr::Call { callee, args } => {
            out.insert(callee.clone());
            for a in args {
                collect_expr_names(a, out);
            }
        }
        Expr::Block { tag, children } => {
            out.insert(tag.clone());
            for c in children {
                collect_expr_names(c, out);
            }
        }
        Expr::FieldAccess(e, _) => collect_expr_names(e, out),
        Expr::MethodCall { receiver, args, .. } => {
            collect_expr_names(receiver, out);
            for a in args {
                collect_expr_names(a, out);
            }
        }
        Expr::Lambda { params, body } => {
            for p in params {
                collect_type_names(&p.ty, out);
            }
            collect_expr_names(body, out);
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_names(left, out);
            collect_expr_names(right, out);
        }
        Expr::Unary { expr, .. } => collect_expr_names(expr, out),
        _ => {}
    }
}

// ── Pass 2: Inline trivial components ───────────────────────────────────────

fn inline_components(mut program: Program) -> Program {
    // Find zero-field, no-constructor, single-render-method Component classes
    let mut inlinable: HashMap<String, Vec<Stmt>> = HashMap::new();
    for decl in &program.declarations {
        if let Declaration::Class(c) = decl {
            if c.kind == ClassKind::Component
                && c.fields.is_empty()
                && c.constructor.is_none()
                && c.methods.len() == 1
                && c.methods[0].name == "render"
                && c.methods[0].params.is_empty()
            {
                inlinable.insert(c.name.clone(), c.methods[0].body.clone());
            }
        }
    }

    if inlinable.is_empty() {
        return program;
    }

    // Rewrite all method bodies, replacing zero-arg calls to inlinable components
    for decl in &mut program.declarations {
        if let Declaration::Class(c) = decl {
            if inlinable.contains_key(&c.name) {
                continue; // skip the component itself
            }
            for method in &mut c.methods {
                method.body = method
                    .body
                    .drain(..)
                    .map(|s| inline_stmt(s, &inlinable))
                    .collect();
            }
            if let Some(ctor) = &mut c.constructor {
                ctor.body = ctor
                    .body
                    .drain(..)
                    .map(|s| inline_stmt(s, &inlinable))
                    .collect();
            }
        }
    }

    // Remove the inlined component declarations
    program.declarations.retain(|d| {
        if let Declaration::Class(c) = d {
            !inlinable.contains_key(&c.name)
        } else {
            true
        }
    });

    program
}

fn inline_stmt(stmt: Stmt, map: &HashMap<String, Vec<Stmt>>) -> Stmt {
    match stmt {
        Stmt::Return(Some(e)) => Stmt::Return(Some(inline_expr(e, map))),
        Stmt::Assign {
            object,
            field,
            value,
        } => Stmt::Assign {
            object: inline_expr(object, map),
            field,
            value: inline_expr(value, map),
        },
        Stmt::Let { name, ty, init } => Stmt::Let {
            name,
            ty,
            init: inline_expr(init, map),
        },
        Stmt::If {
            cond,
            then_body,
            else_body,
        } => Stmt::If {
            cond: inline_expr(cond, map),
            then_body: then_body.into_iter().map(|s| inline_stmt(s, map)).collect(),
            else_body: else_body.map(|b| b.into_iter().map(|s| inline_stmt(s, map)).collect()),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: inline_expr(cond, map),
            body: body.into_iter().map(|s| inline_stmt(s, map)).collect(),
        },
        Stmt::For { var, iter, body } => Stmt::For {
            var,
            iter: inline_expr(iter, map),
            body: body.into_iter().map(|s| inline_stmt(s, map)).collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(inline_expr(e, map)),
        other => other,
    }
}

fn inline_expr(expr: Expr, map: &HashMap<String, Vec<Stmt>>) -> Expr {
    match expr {
        Expr::Call {
            ref callee,
            ref args,
        } if args.is_empty() => {
            if let Some(body) = map.get(callee) {
                // If the render body is a single return, unwrap the expression
                if body.len() == 1 {
                    if let Stmt::Return(Some(inner)) = &body[0] {
                        return inner.clone();
                    }
                }
            }
            expr
        }
        Expr::FieldAccess(inner, field) => {
            Expr::FieldAccess(Box::new(inline_expr(*inner, map)), field)
        }
        Expr::MethodCall {
            receiver,
            method,
            args,
        } => Expr::MethodCall {
            receiver: Box::new(inline_expr(*receiver, map)),
            method,
            args: args.into_iter().map(|a| inline_expr(a, map)).collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args.into_iter().map(|a| inline_expr(a, map)).collect(),
        },
        Expr::Lambda { params, body } => Expr::Lambda {
            params,
            body: Box::new(inline_expr(*body, map)),
        },
        Expr::Block { tag, children } => Expr::Block {
            tag,
            children: children.into_iter().map(|c| inline_expr(c, map)).collect(),
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op,
            left: Box::new(inline_expr(*left, map)),
            right: Box::new(inline_expr(*right, map)),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(inline_expr(*expr, map)),
        },
        other => other,
    }
}

// ── Pass 3: Flatten nested same-tag blocks ───────────────────────────────────

fn flatten_tree(mut program: Program) -> Program {
    for decl in &mut program.declarations {
        if let Declaration::Class(c) = decl {
            for m in &mut c.methods {
                m.body = m.body.drain(..).map(flatten_stmt).collect();
            }
            if let Some(ctor) = &mut c.constructor {
                ctor.body = ctor.body.drain(..).map(flatten_stmt).collect();
            }
        }
    }
    program
}

fn flatten_stmt(stmt: Stmt) -> Stmt {
    match stmt {
        Stmt::Return(Some(e)) => Stmt::Return(Some(flatten_expr(e))),
        Stmt::Assign {
            object,
            field,
            value,
        } => Stmt::Assign {
            object: flatten_expr(object),
            field,
            value: flatten_expr(value),
        },
        Stmt::Let { name, ty, init } => Stmt::Let {
            name,
            ty,
            init: flatten_expr(init),
        },
        Stmt::If {
            cond,
            then_body,
            else_body,
        } => Stmt::If {
            cond: flatten_expr(cond),
            then_body: then_body.into_iter().map(flatten_stmt).collect(),
            else_body: else_body.map(|b| b.into_iter().map(flatten_stmt).collect()),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: flatten_expr(cond),
            body: body.into_iter().map(flatten_stmt).collect(),
        },
        Stmt::For { var, iter, body } => Stmt::For {
            var,
            iter: flatten_expr(iter),
            body: body.into_iter().map(flatten_stmt).collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(flatten_expr(e)),
        other => other,
    }
}

fn flatten_expr(expr: Expr) -> Expr {
    match expr {
        Expr::Block { tag, children } => {
            // Recurse first (bottom-up)
            let children: Vec<Expr> = children.into_iter().map(flatten_expr).collect();
            // If single child is same-tag block, lift its children
            if children.len() == 1 {
                if let Expr::Block {
                    tag: inner_tag,
                    children: inner_children,
                } = &children[0]
                {
                    if *inner_tag == tag {
                        return Expr::Block {
                            tag,
                            children: inner_children.clone(),
                        };
                    }
                }
            }
            Expr::Block { tag, children }
        }
        Expr::FieldAccess(e, f) => Expr::FieldAccess(Box::new(flatten_expr(*e)), f),
        Expr::MethodCall {
            receiver,
            method,
            args,
        } => Expr::MethodCall {
            receiver: Box::new(flatten_expr(*receiver)),
            method,
            args: args.into_iter().map(flatten_expr).collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args.into_iter().map(flatten_expr).collect(),
        },
        Expr::Lambda { params, body } => Expr::Lambda {
            params,
            body: Box::new(flatten_expr(*body)),
        },
        Expr::Binary { op, left, right } => Expr::Binary {
            op,
            left: Box::new(flatten_expr(*left)),
            right: Box::new(flatten_expr(*right)),
        },
        Expr::Unary { op, expr } => Expr::Unary {
            op,
            expr: Box::new(flatten_expr(*expr)),
        },
        other => other,
    }
}

// ── Pass 4: Constant folding (precompute props) ──────────────────────────────

fn precompute_props(mut program: Program) -> Program {
    for decl in &mut program.declarations {
        if let Declaration::Class(c) = decl {
            for m in &mut c.methods {
                m.body = m.body.drain(..).map(fold_stmt).collect();
            }
            if let Some(ctor) = &mut c.constructor {
                ctor.body = ctor.body.drain(..).map(fold_stmt).collect();
            }
        }
    }
    program
}

fn fold_stmt(stmt: Stmt) -> Stmt {
    match stmt {
        Stmt::Return(Some(e)) => Stmt::Return(Some(fold_expr(e))),
        Stmt::Assign {
            object,
            field,
            value,
        } => Stmt::Assign {
            object: fold_expr(object),
            field,
            value: fold_expr(value),
        },
        Stmt::Let { name, ty, init } => Stmt::Let {
            name,
            ty,
            init: fold_expr(init),
        },
        Stmt::If {
            cond,
            then_body,
            else_body,
        } => Stmt::If {
            cond: fold_expr(cond),
            then_body: then_body.into_iter().map(fold_stmt).collect(),
            else_body: else_body.map(|b| b.into_iter().map(fold_stmt).collect()),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: fold_expr(cond),
            body: body.into_iter().map(fold_stmt).collect(),
        },
        Stmt::For { var, iter, body } => Stmt::For {
            var,
            iter: fold_expr(iter),
            body: body.into_iter().map(fold_stmt).collect(),
        },
        Stmt::Expr(e) => Stmt::Expr(fold_expr(e)),
        other => other,
    }
}

pub fn fold_expr(expr: Expr) -> Expr {
    match expr {
        Expr::Binary { op, left, right } => {
            let left = fold_expr(*left);
            let right = fold_expr(*right);
            match (&op, &left, &right) {
                (BinOp::Add, Expr::IntLit(a), Expr::IntLit(b)) => Expr::IntLit(a + b),
                (BinOp::Sub, Expr::IntLit(a), Expr::IntLit(b)) => Expr::IntLit(a - b),
                (BinOp::Mul, Expr::IntLit(a), Expr::IntLit(b)) => Expr::IntLit(a * b),
                (BinOp::Div, Expr::IntLit(a), Expr::IntLit(b)) if *b != 0 => Expr::IntLit(a / b),
                (BinOp::Mod, Expr::IntLit(a), Expr::IntLit(b)) if *b != 0 => Expr::IntLit(a % b),
                (BinOp::Add, Expr::StringLit(a), Expr::StringLit(b)) => {
                    Expr::StringLit(format!("{a}{b}"))
                }
                (BinOp::And, Expr::BoolLit(a), Expr::BoolLit(b)) => Expr::BoolLit(*a && *b),
                (BinOp::Or, Expr::BoolLit(a), Expr::BoolLit(b)) => Expr::BoolLit(*a || *b),
                _ => Expr::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
            }
        }
        Expr::Unary { op, expr } => {
            let inner = fold_expr(*expr);
            match (&op, &inner) {
                (UnOp::Neg, Expr::IntLit(n)) => Expr::IntLit(-n),
                (UnOp::Not, Expr::BoolLit(b)) => Expr::BoolLit(!b),
                _ => Expr::Unary {
                    op,
                    expr: Box::new(inner),
                },
            }
        }
        Expr::FieldAccess(e, f) => Expr::FieldAccess(Box::new(fold_expr(*e)), f),
        Expr::MethodCall {
            receiver,
            method,
            args,
        } => Expr::MethodCall {
            receiver: Box::new(fold_expr(*receiver)),
            method,
            args: args.into_iter().map(fold_expr).collect(),
        },
        Expr::Call { callee, args } => Expr::Call {
            callee,
            args: args.into_iter().map(fold_expr).collect(),
        },
        Expr::Lambda { params, body } => Expr::Lambda {
            params,
            body: Box::new(fold_expr(*body)),
        },
        Expr::Block { tag, children } => Expr::Block {
            tag,
            children: children.into_iter().map(fold_expr).collect(),
        },
        other => other,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ast::*;

    #[test]
    fn precompute_int_add() {
        let expr = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::IntLit(2)),
            right: Box::new(Expr::IntLit(3)),
        };
        assert!(matches!(fold_expr(expr), Expr::IntLit(5)));
    }

    #[test]
    fn precompute_string_concat() {
        let expr = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::StringLit("hello ".into())),
            right: Box::new(Expr::StringLit("world".into())),
        };
        assert!(matches!(fold_expr(expr), Expr::StringLit(s) if s == "hello world"));
    }

    #[test]
    fn precompute_neg() {
        let expr = Expr::Unary {
            op: UnOp::Neg,
            expr: Box::new(Expr::IntLit(5)),
        };
        assert!(matches!(fold_expr(expr), Expr::IntLit(-5)));
    }

    #[test]
    fn precompute_not() {
        let expr = Expr::Unary {
            op: UnOp::Not,
            expr: Box::new(Expr::BoolLit(true)),
        };
        assert!(matches!(fold_expr(expr), Expr::BoolLit(false)));
    }

    #[test]
    fn flatten_nested_same_tag() {
        let inner = Expr::Block {
            tag: "Page".into(),
            children: vec![Expr::IntLit(1)],
        };
        let outer = Expr::Block {
            tag: "Page".into(),
            children: vec![inner],
        };
        let result = flatten_expr(outer);
        match result {
            Expr::Block { tag, children } => {
                assert_eq!(tag, "Page");
                assert_eq!(children.len(), 1);
                assert!(matches!(children[0], Expr::IntLit(1)));
            }
            _ => panic!("expected Block"),
        }
    }

    #[test]
    fn flatten_different_tags_unchanged() {
        let inner = Expr::Block {
            tag: "Column".into(),
            children: vec![Expr::IntLit(1)],
        };
        let outer = Expr::Block {
            tag: "Page".into(),
            children: vec![inner],
        };
        let result = flatten_expr(outer);
        match result {
            Expr::Block { tag, children } => {
                assert_eq!(tag, "Page");
                assert_eq!(children.len(), 1);
                assert!(matches!(&children[0], Expr::Block { tag, .. } if tag == "Column"));
            }
            _ => panic!("expected Block"),
        }
    }

    #[test]
    fn no_div_by_zero_fold() {
        let expr = Expr::Binary {
            op: BinOp::Div,
            left: Box::new(Expr::IntLit(10)),
            right: Box::new(Expr::IntLit(0)),
        };
        // Should NOT fold — return Binary unchanged
        assert!(matches!(fold_expr(expr), Expr::Binary { .. }));
    }
}
