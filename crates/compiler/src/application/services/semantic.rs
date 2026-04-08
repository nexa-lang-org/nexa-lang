//! Semantic analyser.
//!
//! Checks:
//!   - No duplicate class/interface names
//!   - extends/implements refer to existing names
//!   - Routes point to Window declarations
//!   - Imported symbols exist (names only — full type-checking is future work)
//!   - Type mismatches in `let` annotations and `return` statements (Pass 5)

use crate::domain::ast::*;
use crate::domain::span::Span;
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SemanticError {
    #[error("Undefined type '{name}'")]
    UndefinedType { name: String, span: Span },
    #[error("Duplicate declaration '{name}'")]
    Duplicate { name: String, span: Span },
    #[error("Route target '{name}' is not a window")]
    NotAWindow { name: String, span: Span },
    #[error("Import '{path}' refers to unknown symbol")]
    UnknownImport { path: String, span: Span },
    #[error("Symbol '{name}' is not public and cannot be imported")]
    NotPublic { name: String, span: Span },
    #[error("Type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String, span: Span },
    #[error("Undeclared type parameter '{param}' used in '{context}' (declared: [{declared}])")]
    UndeclaredTypeParam { param: String, context: String, declared: String, span: Span },
    #[error("Infinite type: {0}")]
    InfiniteType(String, Span),
}

impl SemanticError {
    pub fn span(&self) -> Span {
        match self {
            SemanticError::UndefinedType { span, .. } => *span,
            SemanticError::Duplicate { span, .. } => *span,
            SemanticError::NotAWindow { span, .. } => *span,
            SemanticError::UnknownImport { span, .. } => *span,
            SemanticError::NotPublic { span, .. } => *span,
            SemanticError::TypeMismatch { span, .. } => *span,
            SemanticError::UndeclaredTypeParam { span, .. } => *span,
            SemanticError::InfiniteType(_, span) => *span,
        }
    }
}

// ── Hindley-Milner — Damas-Milner Algorithm W ────────────────────────────────
//
// Architecture:
//   HmType          — type representation with Var(u32) unification variables
//   TypeScheme      — ∀α₁…αₙ. τ  (universally quantified type)
//   HmEnv           — environment mapping names → TypeScheme
//   Unifier         — substitution map + unify() + occurs-check
//   free_vars*      — collect free unification variables
//   generalize()    — ∀-close free vars not in env  (the "let" rule)
//   instantiate()   — replace ∀ vars with fresh vars (the "var" rule)
//   subst_vars()    — structural substitution without touching the Unifier
//   instantiate_type() — replace AST Generic(T) with HmType from a name map
// ─────────────────────────────────────────────────────────────────────────────

/// Internal type representation used during unification.
#[derive(Debug, Clone, PartialEq)]
enum HmType {
    Int,
    String,
    Bool,
    Void,
    List(Box<HmType>),
    Custom(String),
    Function(Vec<HmType>, Box<HmType>),
    /// Unification variable (solved by `Unifier::unify`).
    Var(u32),
}

impl HmType {
    fn from_ast(ty: &Type) -> Self {
        match ty {
            Type::Int => HmType::Int,
            Type::String => HmType::String,
            Type::Bool => HmType::Bool,
            Type::Void => HmType::Void,
            Type::List(inner) => HmType::List(Box::new(HmType::from_ast(inner))),
            Type::Custom(n) => HmType::Custom(n.clone()),
            Type::Generic(n) => HmType::Custom(n.clone()), // call-site handles generics
            Type::Function(params, ret) => HmType::Function(
                params.iter().map(HmType::from_ast).collect(),
                Box::new(HmType::from_ast(ret)),
            ),
        }
    }

    fn display(&self) -> String {
        match self {
            HmType::Int => "Int".into(),
            HmType::String => "String".into(),
            HmType::Bool => "Bool".into(),
            HmType::Void => "Void".into(),
            HmType::List(inner) => format!("List<{}>", inner.display()),
            HmType::Custom(n) => n.clone(),
            HmType::Function(params, ret) => {
                let ps: Vec<String> = params.iter().map(|p| p.display()).collect();
                format!("({}) => {}", ps.join(", "), ret.display())
            }
            HmType::Var(id) => format!("α{id}"),
        }
    }
}

// ── TypeScheme ────────────────────────────────────────────────────────────────

/// A type scheme `∀α₁…αₙ. τ`.
///
/// When `quantified` is empty the scheme is a plain monotype.
#[derive(Debug, Clone)]
struct TypeScheme {
    /// Universally-quantified variable IDs (the ∀ prefix).
    quantified: Vec<u32>,
    ty: HmType,
}

impl TypeScheme {
    /// Wrap a monotype (no quantification).
    fn mono(ty: HmType) -> Self {
        TypeScheme { quantified: vec![], ty }
    }
}

/// The typing environment: maps names to their (possibly polymorphic) schemes.
type HmEnv = HashMap<String, TypeScheme>;

// ── Free variables ────────────────────────────────────────────────────────────

/// Collect all free unification variables in `ty` (follows the substitution).
fn free_vars(ty: &HmType, subst: &HashMap<u32, HmType>) -> HashSet<u32> {
    match ty {
        HmType::Var(id) => match subst.get(id) {
            Some(t) => free_vars(t, subst),
            None => std::iter::once(*id).collect(),
        },
        HmType::List(inner) => free_vars(inner, subst),
        HmType::Function(params, ret) => {
            let mut vs = free_vars(ret, subst);
            for p in params {
                vs.extend(free_vars(p, subst));
            }
            vs
        }
        _ => HashSet::new(),
    }
}

/// Free variables in a scheme (variables not bound by ∀).
fn free_vars_scheme(s: &TypeScheme, subst: &HashMap<u32, HmType>) -> HashSet<u32> {
    let mut vs = free_vars(&s.ty, subst);
    for q in &s.quantified {
        vs.remove(q);
    }
    vs
}

/// Union of free variables across all schemes in the environment.
fn free_vars_env(env: &HmEnv, subst: &HashMap<u32, HmType>) -> HashSet<u32> {
    env.values()
        .flat_map(|s| free_vars_scheme(s, subst))
        .collect()
}

// ── Generalization and instantiation ─────────────────────────────────────────

/// Generalize `ty` w.r.t. `env`: ∀-close every unification variable that is
/// free in `ty` but **not** free in `env` (those are the "generic" variables).
///
/// This is the "let" rule of Damas-Milner:
/// ```text
/// Γ ⊢ e : τ    vars(τ) \ free(Γ) = {α₁…αₙ}
/// ──────────────────────────────────────────
///          Γ ⊢ let x = e : ∀α₁…αₙ. τ
/// ```
fn generalize(env: &HmEnv, ty: &HmType, u: &mut Unifier) -> TypeScheme {
    let resolved = u.apply(ty);
    let ty_free = free_vars(&resolved, &u.subst);
    let env_free = free_vars_env(env, &u.subst);
    let mut quantified: Vec<u32> = ty_free.difference(&env_free).cloned().collect();
    quantified.sort_unstable(); // deterministic order for tests
    TypeScheme { quantified, ty: resolved }
}

/// Instantiate a scheme: replace each ∀-bound variable with a **fresh** unification
/// variable.  Every use of a polymorphic binding gets independent variables.
///
/// ```text
///   Γ(x) = ∀α. τ     β fresh
/// ───────────────────────────
///        Γ ⊢ x : τ[α↦β]
/// ```
fn instantiate(scheme: &TypeScheme, u: &mut Unifier) -> HmType {
    if scheme.quantified.is_empty() {
        return u.apply(&scheme.ty);
    }
    let mapping: HashMap<u32, HmType> = scheme
        .quantified
        .iter()
        .map(|&id| (id, HmType::Var(u.fresh())))
        .collect();
    subst_vars(&scheme.ty, &mapping)
}

/// Structural substitution: replace `Var(id)` with `mapping[id]` without
/// touching the `Unifier` (used by `instantiate`).
fn subst_vars(ty: &HmType, mapping: &HashMap<u32, HmType>) -> HmType {
    match ty {
        HmType::Var(id) => mapping.get(id).cloned().unwrap_or_else(|| ty.clone()),
        HmType::List(inner) => HmType::List(Box::new(subst_vars(inner, mapping))),
        HmType::Function(params, ret) => HmType::Function(
            params.iter().map(|p| subst_vars(p, mapping)).collect(),
            Box::new(subst_vars(ret, mapping)),
        ),
        other => other.clone(),
    }
}

// ── Unifier ───────────────────────────────────────────────────────────────────

/// Substitution map + unification engine.
struct Unifier {
    next_id: u32,
    subst: HashMap<u32, HmType>,
}

impl Unifier {
    fn new() -> Self {
        Unifier { next_id: 0, subst: HashMap::new() }
    }

    /// Allocate a fresh unification variable.
    fn fresh(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Apply the current substitution to `ty` (follows chains to normal form).
    ///
    /// Uses **path compression**: after resolving a chain `α1 → α2 → τ`,
    /// `α1` is updated to point directly to `τ`, reducing future traversal
    /// from O(n) to O(1) amortised — the same guarantee as union-find.
    fn apply(&mut self, ty: &HmType) -> HmType {
        match ty {
            HmType::Var(id) => {
                let id = *id;
                // Clone the bound type out of `subst` so we can call `apply`
                // recursively without holding a borrow into `self.subst`.
                match self.subst.get(&id).cloned() {
                    Some(bound) => {
                        let resolved = self.apply(&bound);
                        // Path compress: point `id` directly at the terminal type,
                        // skipping the intermediate chain on the next lookup.
                        self.subst.insert(id, resolved.clone());
                        resolved
                    }
                    None => ty.clone(),
                }
            }
            HmType::List(inner) => HmType::List(Box::new(self.apply(inner))),
            HmType::Function(params, ret) => {
                // Collect params into an owned Vec first so we release the
                // borrow on `params` before the mutable `self.apply(ret)` call.
                let params_applied: Vec<HmType> = params.iter().map(|p| self.apply(p)).collect();
                let ret_applied = self.apply(ret);
                HmType::Function(params_applied, Box::new(ret_applied))
            }
            other => other.clone(),
        }
    }

    /// Occurs check: `id` must not appear in `ty` (prevents infinite types).
    fn occurs(&self, id: u32, ty: &HmType) -> bool {
        match ty {
            HmType::Var(vid) => {
                if *vid == id {
                    return true;
                }
                self.subst.get(vid).is_some_and(|b| self.occurs(id, b))
            }
            HmType::List(inner) => self.occurs(id, inner),
            HmType::Function(params, ret) => {
                params.iter().any(|p| self.occurs(id, p)) || self.occurs(id, ret)
            }
            _ => false,
        }
    }

    /// Robinson unification: extend the substitution so that `t1 = t2`.
    /// Returns `Err(message)` on failure.
    fn unify(&mut self, t1: &HmType, t2: &HmType) -> Result<(), String> {
        let t1 = self.apply(t1);
        let t2 = self.apply(t2);
        if t1 == t2 {
            return Ok(());
        }
        match (t1, t2) {
            (HmType::Var(id), other) => {
                if self.occurs(id, &other) {
                    return Err(format!("infinite type: α{id} = {}", other.display()));
                }
                self.subst.insert(id, other);
                Ok(())
            }
            (other, HmType::Var(id)) => {
                if self.occurs(id, &other) {
                    return Err(format!("infinite type: α{id} = {}", other.display()));
                }
                self.subst.insert(id, other);
                Ok(())
            }
            (HmType::List(a), HmType::List(b)) => self.unify(&a, &b),
            (HmType::Function(ps1, r1), HmType::Function(ps2, r2)) => {
                if ps1.len() != ps2.len() {
                    return Err(format!(
                        "function arity mismatch: {} vs {}",
                        ps1.len(),
                        ps2.len()
                    ));
                }
                let pairs: Vec<(HmType, HmType)> =
                    ps1.into_iter().zip(ps2).collect();
                for (a, b) in &pairs {
                    self.unify(a, b)?;
                }
                self.unify(&r1, &r2)
            }
            (a, b) => Err(format!("cannot unify {} with {}", a.display(), b.display())),
        }
    }
}

// ── AST generic instantiation helper ─────────────────────────────────────────

/// Replace `Type::Generic(name)` in an AST type with the corresponding fresh
/// `HmType` from `vars` (built per call-site for generic classes/methods).
fn instantiate_type(ty: &Type, vars: &HashMap<String, HmType>) -> HmType {
    match ty {
        Type::Generic(name) => {
            vars.get(name).cloned().unwrap_or_else(|| HmType::Custom(name.clone()))
        }
        Type::List(inner) => HmType::List(Box::new(instantiate_type(inner, vars))),
        Type::Function(params, ret) => HmType::Function(
            params.iter().map(|p| instantiate_type(p, vars)).collect(),
            Box::new(instantiate_type(ret, vars)),
        ),
        other => HmType::from_ast(other),
    }
}

pub struct SemanticAnalyzer {
    classes: HashMap<String, ClassDecl>,
    interfaces: HashMap<String, InterfaceDecl>,
}

impl SemanticAnalyzer {
    pub fn new() -> Self {
        SemanticAnalyzer {
            classes: HashMap::new(),
            interfaces: HashMap::new(),
        }
    }

    pub fn analyze(&mut self, program: &Program) -> Result<(), SemanticError> {
        // ── Pass 1: collect all names ───────────────────────────────────────
        for decl in &program.declarations {
            match decl {
                Declaration::Class(cls) => {
                    if self.classes.contains_key(&cls.name) {
                        return Err(SemanticError::Duplicate {
                            name: cls.name.clone(),
                            span: Span::dummy(),
                        });
                    }
                    self.classes.insert(cls.name.clone(), cls.clone());
                }
                Declaration::Interface(iface) => {
                    if self.interfaces.contains_key(&iface.name) {
                        return Err(SemanticError::Duplicate {
                            name: iface.name.clone(),
                            span: Span::dummy(),
                        });
                    }
                    self.interfaces.insert(iface.name.clone(), iface.clone());
                }
                // Enums and test blocks do not require name-collection in Pass 1.
                Declaration::Enum(_) | Declaration::Test(_) => {}
            }
        }

        // ── Pass 2: validate references ─────────────────────────────────────
        for decl in &program.declarations {
            if let Declaration::Class(cls) = decl {
                self.check_class(cls)?;
            }
        }

        // ── Pass 3: validate imports ────────────────────────────────────────
        // The resolver has already merged imported declarations into `program.declarations`,
        // so we just check that each import path's last segment resolves to a known symbol.
        let all_names: HashSet<&str> = self
            .classes
            .keys()
            .map(|s| s.as_str())
            .chain(self.interfaces.keys().map(|s| s.as_str()))
            .collect();

        for import in &program.imports {
            let symbol = import.path.split('.').next_back().unwrap_or("");
            if !all_names.contains(symbol) {
                return Err(SemanticError::UnknownImport {
                    path: import.path.clone(),
                    span: Span::dummy(),
                });
            }
            // Check it's public
            if let Some(cls) = self.classes.get(symbol) {
                if cls.visibility != Visibility::Public {
                    return Err(SemanticError::NotPublic {
                        name: symbol.to_string(),
                        span: Span::dummy(),
                    });
                }
            }
        }

        // ── Pass 4: validate routes ─────────────────────────────────────────
        for route in &program.routes {
            match self.classes.get(&route.target) {
                None => {
                    return Err(SemanticError::UndefinedType {
                        name: route.target.clone(),
                        span: Span::dummy(),
                    })
                }
                Some(cls) if cls.kind != ClassKind::Window => {
                    return Err(SemanticError::NotAWindow {
                        name: route.target.clone(),
                        span: Span::dummy(),
                    });
                }
                _ => {}
            }
        }

        // ── Pass 5: type checking ───────────────────────────────────────────
        for decl in &program.declarations {
            if let Declaration::Class(cls) = decl {
                self.check_class_types(cls)?;
            }
        }

        // ── Pass 6: generic type parameter validation ───────────────────────
        for decl in &program.declarations {
            match decl {
                Declaration::Class(cls) => self.check_class_generics(cls)?,
                Declaration::Interface(iface) => self.check_interface_generics(iface)?,
                Declaration::Enum(_) | Declaration::Test(_) => {}
            }
        }

        // ── Pass 7: Hindley-Milner type inference ───────────────────────────
        for decl in &program.declarations {
            if let Declaration::Class(cls) = decl {
                self.check_class_hm(cls)?;
            }
        }

        Ok(())
    }

    fn check_class(&self, cls: &ClassDecl) -> Result<(), SemanticError> {
        if let Some(parent) = &cls.extends {
            if !self.classes.contains_key(parent) {
                return Err(SemanticError::UndefinedType {
                    name: parent.clone(),
                    span: Span::dummy(),
                });
            }
        }
        for iface in &cls.implements {
            if !self.interfaces.contains_key(iface) {
                return Err(SemanticError::UndefinedType {
                    name: iface.clone(),
                    span: Span::dummy(),
                });
            }
        }
        Ok(())
    }

    // ── Type checking (Pass 5) ────────────────────────────────────────────────

    fn check_class_types(&self, cls: &ClassDecl) -> Result<(), SemanticError> {
        // Seed the env with field types so method bodies can resolve them.
        let field_env: HashMap<String, Type> = cls
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.clone()))
            .collect();

        if let Some(ctor) = &cls.constructor {
            let mut env = field_env.clone();
            for p in &ctor.params {
                env.insert(p.name.clone(), p.ty.clone());
            }
            for stmt in &ctor.body {
                self.check_stmt_types(stmt, &Type::Void, cls, &mut env)?;
            }
        }

        for method in &cls.methods {
            let mut env = field_env.clone();
            for p in &method.params {
                env.insert(p.name.clone(), p.ty.clone());
            }
            for stmt in &method.body {
                self.check_stmt_types(stmt, &method.return_type, cls, &mut env)?;
            }
        }
        Ok(())
    }

    /// Infer the AST type of an expression given the current local environment.
    /// Returns `None` when the type cannot be determined statically (method calls, etc.).
    fn infer_expr_type(
        &self,
        expr: &Expr,
        cls: &ClassDecl,
        env: &HashMap<String, Type>,
    ) -> Option<Type> {
        match expr {
            Expr::IntLit(_) => Some(Type::Int),
            Expr::StringLit(_) => Some(Type::String),
            Expr::BoolLit(_) => Some(Type::Bool),
            Expr::Ident(name) => env.get(name).cloned(),
            Expr::This => Some(Type::Custom(cls.name.clone())),
            Expr::FieldAccess(obj, field) => {
                // Determine the class of the receiver then look up the field type.
                let cls_name = match obj.as_ref() {
                    Expr::This => cls.name.clone(),
                    other => match self.infer_expr_type(other, cls, env)? {
                        Type::Custom(n) => n,
                        _ => return None,
                    },
                };
                self.classes
                    .get(&cls_name)?
                    .fields
                    .iter()
                    .find(|f| &f.name == field)
                    .map(|f| f.ty.clone())
            }
            Expr::MethodCall { receiver, method, .. } => {
                // Determine the class of the receiver then look up the method's return type.
                let cls_name = match receiver.as_ref() {
                    Expr::This => cls.name.clone(),
                    other => match self.infer_expr_type(other, cls, env)? {
                        Type::Custom(n) => n,
                        _ => return None,
                    },
                };
                self.classes
                    .get(&cls_name)?
                    .methods
                    .iter()
                    .find(|m| &m.name == method)
                    .map(|m| m.return_type.clone())
            }
            Expr::Binary { op, left, right } => {
                let lt = self.infer_expr_type(left, cls, env)?;
                let rt = self.infer_expr_type(right, cls, env)?;
                match op {
                    BinOp::Add => match (&lt, &rt) {
                        (Type::Int, Type::Int) => Some(Type::Int),
                        (Type::String, _) | (_, Type::String) => Some(Type::String),
                        _ => None,
                    },
                    BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                        if lt == Type::Int && rt == Type::Int {
                            Some(Type::Int)
                        } else {
                            None
                        }
                    }
                    BinOp::Eq
                    | BinOp::Ne
                    | BinOp::Lt
                    | BinOp::Gt
                    | BinOp::Le
                    | BinOp::Ge
                    | BinOp::And
                    | BinOp::Or => Some(Type::Bool),
                }
            }
            Expr::Unary { op, expr: inner } => match op {
                UnOp::Not => Some(Type::Bool),
                UnOp::Neg => {
                    let t = self.infer_expr_type(inner, cls, env)?;
                    if t == Type::Int {
                        Some(Type::Int)
                    } else {
                        None
                    }
                }
            },
            // Method calls, lambdas, blocks: type not inferable without a full type
            // system — skip checking, the lower pass will mark them Unknown.
            _ => None,
        }
    }

    fn check_stmt_types(
        &self,
        stmt: &Stmt,
        return_type: &Type,
        cls: &ClassDecl,
        env: &mut HashMap<String, Type>,
    ) -> Result<(), SemanticError> {
        match stmt {
            Stmt::Let { name, ty: Some(declared), init } => {
                if let Some(inferred) = self.infer_expr_type(init, cls, env) {
                    if &inferred != declared {
                        return Err(SemanticError::TypeMismatch {
                            expected: format!("{declared:?}"),
                            found: format!("{inferred:?}"),
                            span: Span::dummy(),
                        });
                    }
                }
                env.insert(name.clone(), declared.clone());
            }
            Stmt::Let { name, ty: None, init } => {
                // No annotation: infer and record for subsequent statements.
                if let Some(t) = self.infer_expr_type(init, cls, env) {
                    env.insert(name.clone(), t);
                }
            }
            Stmt::Return { expr: Some(expr), span } if return_type != &Type::Void => {
                if let Some(found) = self.infer_expr_type(expr, cls, env) {
                    if &found != return_type {
                        return Err(SemanticError::TypeMismatch {
                            expected: format!("{return_type:?}"),
                            found: format!("{found:?}"),
                            span: *span,
                        });
                    }
                }
            }
            Stmt::If { then_body, else_body, .. } => {
                // Each branch gets its own child scope (clone of parent env).
                // Intentional O(bindings) clone per branch: scopes are small
                // (5-20 entries) and this only runs once per if-stmt per method.
                // Use Cow<HashMap> or an arena if profiling shows this as a
                // bottleneck on very large programs.
                let mut then_env = env.clone();
                for s in then_body {
                    self.check_stmt_types(s, return_type, cls, &mut then_env)?;
                }
                if let Some(eb) = else_body {
                    let mut else_env = env.clone();
                    for s in eb {
                        self.check_stmt_types(s, return_type, cls, &mut else_env)?;
                    }
                }
            }
            Stmt::While { body, .. } => {
                // Intentional clone — same rationale as Stmt::If above.
                let mut inner_env = env.clone();
                for s in body {
                    self.check_stmt_types(s, return_type, cls, &mut inner_env)?;
                }
            }
            Stmt::For { var, body, .. } => {
                // Intentional clone — same rationale as Stmt::If above.
                let mut inner_env = env.clone();
                // Element type unknown without knowing the iterator's item type.
                inner_env.insert(var.clone(), Type::Generic("T".into()));
                for s in body {
                    self.check_stmt_types(s, return_type, cls, &mut inner_env)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ── Pass 7: Hindley-Milner type inference ─────────────────────────────────

    /// Infer the `HmType` of `expr` using Damas-Milner Algorithm W.
    ///
    /// - `env`  — polymorphic type environment (`name → TypeScheme`)
    /// - `cls`  — enclosing class (for `this` and field/method lookup)
    /// - `u`    — shared unifier for this method body
    ///
    /// Unknown identifiers / unresolvable calls produce a fresh `Var` instead
    /// of an error; unification will constrain them as more context arrives.
    fn infer_expr_hm(
        &self,
        expr: &Expr,
        env: &HmEnv,
        cls: &ClassDecl,
        u: &mut Unifier,
    ) -> Result<HmType, SemanticError> {
        match expr {
            Expr::IntLit(_) => Ok(HmType::Int),
            Expr::StringLit(_) => Ok(HmType::String),
            Expr::BoolLit(_) => Ok(HmType::Bool),
            Expr::This => Ok(HmType::Custom(cls.name.clone())),

            // VAR rule: look up name and *instantiate* its scheme with fresh vars.
            // This is what enables let-polymorphism: every use of `id` (∀α. α→α)
            // gets independent fresh vars, so `id(1)` and `id("hi")` both typecheck.
            Expr::Ident(name) => {
                if let Some(scheme) = env.get(name) {
                    Ok(instantiate(scheme, u))
                } else {
                    Ok(HmType::Var(u.fresh()))
                }
            }

            // ABS rule: infer λ-body with params in env as *monotypes*.
            // Untyped params (placeholder `Custom("T")`) get a fresh Var —
            // the body constrains them via unification.
            Expr::Lambda { params, body } => {
                let mut lambda_env = env.clone();
                let mut param_tys = Vec::new();
                for p in params {
                    let ty = if matches!(&p.ty, Type::Custom(s) if s == "T") {
                        HmType::Var(u.fresh())
                    } else {
                        HmType::from_ast(&p.ty)
                    };
                    // Params are monotypes inside the lambda body (no generalisation).
                    lambda_env.insert(p.name.clone(), TypeScheme::mono(ty.clone()));
                    param_tys.push(ty);
                }
                let body_ty = self.infer_expr_hm(body, &lambda_env, cls, u)?;
                Ok(HmType::Function(param_tys, Box::new(body_ty)))
            }

            // APP rule:
            //   1. Class constructor — instantiate generic params, unify args.
            //   2. Function variable in env — apply the function type.
            //   3. Unknown callee — return fresh Var (deferred).
            Expr::Call { callee, args } => {
                if let Some(called_cls) = self.classes.get(callee) {
                    let gvars: HashMap<String, HmType> = called_cls
                        .type_params
                        .iter()
                        .map(|tp| (tp.clone(), HmType::Var(u.fresh())))
                        .collect();
                    if let Some(ctor) = &called_cls.constructor {
                        for (arg, param) in args.iter().zip(ctor.params.iter()) {
                            let arg_ty = self.infer_expr_hm(arg, env, cls, u)?;
                            let param_ty = instantiate_type(&param.ty, &gvars);
                            u.unify(&arg_ty, &param_ty).map_err(|msg| SemanticError::TypeMismatch {
                                expected: param_ty.display(),
                                found: msg,
                                span: Span::dummy(),
                            })?;
                        }
                    }
                    Ok(HmType::Custom(callee.clone()))
                } else if let Some(scheme) = env.get(callee).cloned() {
                    // Function variable: instantiate scheme, then apply like a function.
                    let fn_ty = instantiate(&scheme, u);
                    let fn_ty = u.apply(&fn_ty);
                    if let HmType::Function(param_tys, ret_ty) = fn_ty {
                        for (arg, param_ty) in args.iter().zip(param_tys.iter()) {
                            let arg_ty = self.infer_expr_hm(arg, env, cls, u)?;
                            u.unify(&arg_ty, param_ty).map_err(|msg| SemanticError::TypeMismatch {
                                expected: param_ty.display(),
                                found: msg,
                                span: Span::dummy(),
                            })?;
                        }
                        Ok(u.apply(&ret_ty))
                    } else {
                        Ok(HmType::Var(u.fresh()))
                    }
                } else {
                    Ok(HmType::Var(u.fresh()))
                }
            }

            // Field access: infer receiver, resolve class, look up field type.
            Expr::FieldAccess(obj, field) => {
                let obj_ty = self.infer_expr_hm(obj, env, cls, u)?;
                let obj_resolved = u.apply(&obj_ty);
                let cls_name = match &obj_resolved {
                    HmType::Custom(n) => n.clone(),
                    _ => return Ok(HmType::Var(u.fresh())),
                };
                if let Some(cls_def) = self.classes.get(&cls_name) {
                    if let Some(f) = cls_def.fields.iter().find(|f| &f.name == field) {
                        return Ok(HmType::from_ast(&f.ty));
                    }
                }
                Ok(HmType::Var(u.fresh()))
            }

            // Method call: instantiate class generic params, unify args, return ret type.
            Expr::MethodCall { receiver, method, args } => {
                let recv_ty = self.infer_expr_hm(receiver, env, cls, u)?;
                let recv_resolved = u.apply(&recv_ty);
                let cls_name = match &recv_resolved {
                    HmType::Custom(n) => n.clone(),
                    _ => return Ok(HmType::Var(u.fresh())),
                };
                if let Some(cls_def) = self.classes.get(&cls_name) {
                    if let Some(meth) = cls_def.methods.iter().find(|m| &m.name == method) {
                        let gvars: HashMap<String, HmType> = cls_def
                            .type_params
                            .iter()
                            .map(|tp| (tp.clone(), HmType::Var(u.fresh())))
                            .collect();
                        for (arg, param) in args.iter().zip(meth.params.iter()) {
                            let arg_ty = self.infer_expr_hm(arg, env, cls, u)?;
                            let param_ty = instantiate_type(&param.ty, &gvars);
                            u.unify(&arg_ty, &param_ty).map_err(|msg| SemanticError::TypeMismatch {
                                expected: param_ty.display(),
                                found: msg,
                                span: Span::dummy(),
                            })?;
                        }
                        let ret = instantiate_type(&meth.return_type, &gvars);
                        return Ok(u.apply(&ret));
                    }
                }
                Ok(HmType::Var(u.fresh()))
            }

            // Binary: constrain operands according to the operator.
            Expr::Binary { op, left, right } => {
                let lt = self.infer_expr_hm(left, env, cls, u)?;
                let rt = self.infer_expr_hm(right, env, cls, u)?;
                let lt = u.apply(&lt);
                let rt = u.apply(&rt);
                match op {
                    BinOp::Add => match (&lt, &rt) {
                        (HmType::String, _) | (_, HmType::String) => Ok(HmType::String),
                        _ => {
                            u.unify(&lt, &HmType::Int).ok();
                            u.unify(&rt, &HmType::Int).ok();
                            Ok(HmType::Int)
                        }
                    },
                    BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                        u.unify(&lt, &HmType::Int).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Int".into(), found: msg, span: Span::dummy(),
                        })?;
                        u.unify(&rt, &HmType::Int).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Int".into(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Int)
                    }
                    BinOp::Eq | BinOp::Ne => {
                        u.unify(&lt, &rt).map_err(|msg| SemanticError::TypeMismatch {
                            expected: lt.display(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Bool)
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        u.unify(&lt, &HmType::Int).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Int".into(), found: msg, span: Span::dummy(),
                        })?;
                        u.unify(&rt, &HmType::Int).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Int".into(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Bool)
                    }
                    BinOp::And | BinOp::Or => {
                        u.unify(&lt, &HmType::Bool).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Bool".into(), found: msg, span: Span::dummy(),
                        })?;
                        u.unify(&rt, &HmType::Bool).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Bool".into(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Bool)
                    }
                }
            }

            Expr::Unary { op, expr: inner } => {
                let ty = self.infer_expr_hm(inner, env, cls, u)?;
                match op {
                    UnOp::Not => {
                        u.unify(&ty, &HmType::Bool).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Bool".into(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Bool)
                    }
                    UnOp::Neg => {
                        u.unify(&ty, &HmType::Int).map_err(|msg| SemanticError::TypeMismatch {
                            expected: "Int".into(), found: msg, span: Span::dummy(),
                        })?;
                        Ok(HmType::Int)
                    }
                }
            }

            // `await expr` — the result type is the inner type
            // (async/await is transparent to the type system at this level).
            Expr::Await(inner) => self.infer_expr_hm(inner, env, cls, u),

            // `[e1, e2, ...]` — a list literal produces List<α> for a fresh α.
            // The element type gets constrained when items are unified with α.
            Expr::ListLiteral(items) => {
                let elem_var = HmType::Var(u.fresh());
                for item in items {
                    let ty = self.infer_expr_hm(item, env, cls, u)?;
                    u.unify(&ty, &elem_var).map_err(|msg| SemanticError::TypeMismatch {
                        expected: elem_var.display(),
                        found: msg,
                        span: Span::dummy(),
                    })?;
                }
                Ok(HmType::List(Box::new(u.apply(&elem_var))))
            }

            // `import("path")` — dynamic import; type is not known statically.
            Expr::LazyImport(_) => Ok(HmType::Var(u.fresh())),

            _ => Ok(HmType::Var(u.fresh())),
        }
    }

    fn check_stmt_hm(
        &self,
        stmt: &Stmt,
        return_ty: &HmType,
        cls: &ClassDecl,
        env: &mut HmEnv,
        u: &mut Unifier,
    ) -> Result<(), SemanticError> {
        match stmt {
            // LET rule (annotated): unify inferred type with annotation, store as mono.
            Stmt::Let { name, ty: Some(declared), init } => {
                let declared_hm = HmType::from_ast(declared);
                let inferred = self.infer_expr_hm(init, env, cls, u)?;
                u.unify(&inferred, &declared_hm).map_err(|msg| SemanticError::TypeMismatch {
                    expected: declared_hm.display(),
                    found: msg,
                    span: Span::dummy(),
                })?;
                // Annotation fixes the type — no generalization, use as monotype.
                env.insert(name.clone(), TypeScheme::mono(u.apply(&declared_hm)));
            }
            // LET rule (unannotated): infer type, then *generalize* free vars.
            // This is the core of let-polymorphism:
            //   let id = x => x   →   id: ∀α. α → α
            // so that `id(1)` and `id("hi")` both typecheck independently.
            Stmt::Let { name, ty: None, init } => {
                let inferred = self.infer_expr_hm(init, env, cls, u)?;
                let scheme = generalize(env, &inferred, u);
                env.insert(name.clone(), scheme);
            }
            // RETURN: check concrete types; skip if either side is still a Var.
            Stmt::Return { expr: Some(expr), span } if *return_ty != HmType::Void => {
                let found = self.infer_expr_hm(expr, env, cls, u)?;
                let found_r = u.apply(&found);
                let ret_r = u.apply(return_ty);
                if !matches!(found_r, HmType::Var(_)) && !matches!(ret_r, HmType::Var(_)) {
                    u.unify(&found_r, &ret_r).map_err(|_| SemanticError::TypeMismatch {
                        expected: ret_r.display(),
                        found: found_r.display(),
                        span: *span,
                    })?;
                }
            }
            Stmt::If { then_body, else_body, .. } => {
                // Intentional clone — same rationale as check_stmt_types above.
                let mut then_env = env.clone();
                for s in then_body {
                    self.check_stmt_hm(s, return_ty, cls, &mut then_env, u)?;
                }
                if let Some(eb) = else_body {
                    let mut else_env = env.clone();
                    for s in eb {
                        self.check_stmt_hm(s, return_ty, cls, &mut else_env, u)?;
                    }
                }
            }
            Stmt::While { body, .. } => {
                // Intentional clone — same rationale as check_stmt_types above.
                let mut inner = env.clone();
                for s in body {
                    self.check_stmt_hm(s, return_ty, cls, &mut inner, u)?;
                }
            }
            Stmt::For { var, body, .. } => {
                // Intentional clone — same rationale as check_stmt_types above.
                let mut inner = env.clone();
                // The loop variable gets a fresh var (element type unknown until stdlib).
                inner.insert(var.clone(), TypeScheme::mono(HmType::Var(u.fresh())));
                for s in body {
                    self.check_stmt_hm(s, return_ty, cls, &mut inner, u)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn check_class_hm(&self, cls: &ClassDecl) -> Result<(), SemanticError> {
        // Seed the environment with field types as monotypes.
        let field_env: HmEnv = cls
            .fields
            .iter()
            .map(|f| (f.name.clone(), TypeScheme::mono(HmType::from_ast(&f.ty))))
            .collect();

        if let Some(ctor) = &cls.constructor {
            let mut env = field_env.clone();
            for p in &ctor.params {
                env.insert(p.name.clone(), TypeScheme::mono(HmType::from_ast(&p.ty)));
            }
            let mut u = Unifier::new();
            for stmt in &ctor.body {
                self.check_stmt_hm(stmt, &HmType::Void, cls, &mut env, &mut u)?;
            }
        }

        for method in &cls.methods {
            let mut env = field_env.clone();
            for p in &method.params {
                env.insert(p.name.clone(), TypeScheme::mono(HmType::from_ast(&p.ty)));
            }
            let ret_ty = HmType::from_ast(&method.return_type);
            let mut u = Unifier::new();
            for stmt in &method.body {
                self.check_stmt_hm(stmt, &ret_ty, cls, &mut env, &mut u)?;
            }
        }
        Ok(())
    }

    // ── Pass 6: generic type parameter validation ─────────────────────────────

    /// Verify that every `Type::Generic(T)` used in `cls` has `T` declared in
    /// `cls.type_params`.  Catches typos like `fn foo(): Lst<Int>` where the
    /// programmer meant `List<Int>` and `Lst` is not a declared type param.
    fn check_class_generics(&self, cls: &ClassDecl) -> Result<(), SemanticError> {
        let declared: HashSet<&str> = cls.type_params.iter().map(String::as_str).collect();
        let context = &cls.name;

        for field in &cls.fields {
            self.check_type_params_in_type(&field.ty, &declared, context)?;
        }

        if let Some(ctor) = &cls.constructor {
            for p in &ctor.params {
                self.check_type_params_in_type(&p.ty, &declared, context)?;
            }
        }

        for method in &cls.methods {
            self.check_type_params_in_type(&method.return_type, &declared, context)?;
            for p in &method.params {
                self.check_type_params_in_type(&p.ty, &declared, context)?;
            }
        }
        Ok(())
    }

    fn check_interface_generics(&self, iface: &InterfaceDecl) -> Result<(), SemanticError> {
        let declared: HashSet<&str> = iface.type_params.iter().map(String::as_str).collect();
        let context = &iface.name;

        for method in &iface.methods {
            self.check_type_params_in_type(&method.return_type, &declared, context)?;
            for p in &method.params {
                self.check_type_params_in_type(&p.ty, &declared, context)?;
            }
        }
        Ok(())
    }

    /// Recursively verify that any `Type::Generic(T)` has `T` in `declared`.
    fn check_type_params_in_type(
        &self,
        ty: &Type,
        declared: &HashSet<&str>,
        context: &str,
    ) -> Result<(), SemanticError> {
        match ty {
            Type::Generic(param) => {
                if !declared.contains(param.as_str()) {
                    return Err(SemanticError::UndeclaredTypeParam {
                        param: param.clone(),
                        context: context.to_string(),
                        declared: declared.iter().copied().collect::<Vec<_>>().join(", "),
                        span: Span::dummy(),
                    });
                }
            }
            Type::List(inner) => {
                self.check_type_params_in_type(inner, declared, context)?;
            }
            Type::Function(params, ret) => {
                for p in params {
                    self.check_type_params_in_type(p, declared, context)?;
                }
                self.check_type_params_in_type(ret, declared, context)?;
            }
            // Primitives and Custom (concrete named) types need no check here.
            _ => {}
        }
        Ok(())
    }
}

impl Default for SemanticAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_class(name: &str, kind: ClassKind, vis: Visibility) -> ClassDecl {
        ClassDecl {
            visibility: vis,
            kind,
            name: name.into(),
            type_params: vec![],
            extends: None,
            implements: vec![],
            fields: vec![],
            constructor: None,
            methods: vec![],
        }
    }

    fn make_program(decls: Vec<Declaration>, routes: Vec<Route>) -> Program {
        Program {
            name: "test".into(),
            package: None,
            imports: vec![],
            server: None,
            declarations: decls,
            routes,
        }
    }

    // ── Pass 1-4 ─────────────────────────────────────────────────────────────

    #[test]
    fn duplicate_class_error() {
        let prog = make_program(
            vec![
                Declaration::Class(make_class("Foo", ClassKind::Class, Visibility::Public)),
                Declaration::Class(make_class("Foo", ClassKind::Class, Visibility::Public)),
            ],
            vec![],
        );
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::Duplicate { .. }));
    }

    #[test]
    fn route_must_target_window() {
        let prog = make_program(
            vec![Declaration::Class(make_class("Home", ClassKind::Class, Visibility::Public))],
            vec![Route { path: "/".into(), target: "Home".into() }],
        );
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::NotAWindow { .. }));
    }

    #[test]
    fn valid_window_route_passes() {
        let prog = make_program(
            vec![Declaration::Class(make_class("Home", ClassKind::Window, Visibility::Public))],
            vec![Route { path: "/".into(), target: "Home".into() }],
        );
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    // ── Pass 5: type checking ─────────────────────────────────────────────────

    #[test]
    fn let_type_mismatch_detected() {
        // let x: String = 42  → should fail
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![Stmt::Let {
                name: "x".into(),
                ty: Some(Type::String),
                init: Expr::IntLit(42),
            }],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(
            matches!(err, SemanticError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );
    }

    #[test]
    fn let_no_annotation_infers_int() {
        // let x = 42  → should pass (inferred as Int)
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![Stmt::Let {
                name: "x".into(),
                ty: None,
                init: Expr::IntLit(42),
            }],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    #[test]
    fn return_type_mismatch_detected() {
        // fn get(): Int { return "oops" }  → should fail
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "get".into(),
            params: vec![],
            return_type: Type::Int,
            body: vec![Stmt::Return { expr: Some(Expr::StringLit("oops".into())), span: Span::dummy() }],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(
            matches!(err, SemanticError::TypeMismatch { .. }),
            "expected TypeMismatch, got {err:?}"
        );
    }

    #[test]
    fn return_type_match_passes() {
        // fn get(): Int { return 7 }  → should pass
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "get".into(),
            params: vec![],
            return_type: Type::Int,
            body: vec![Stmt::Return { expr: Some(Expr::IntLit(7)), span: Span::dummy() }],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    // ── Pass 6: generics ──────────────────────────────────────────────────────

    fn class_with_generic(type_params: Vec<String>, field_ty: Type) -> ClassDecl {
        ClassDecl {
            type_params,
            fields: vec![Field {
                visibility: Visibility::Private,
                ty: field_ty,
                name: "value".into(),
            }],
            ..make_class("Box", ClassKind::Class, Visibility::Public)
        }
    }

    #[test]
    fn generic_param_declared_and_used_passes() {
        // class Box<T> { value: T }  → ok
        let cls = class_with_generic(vec!["T".into()], Type::Generic("T".into()));
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    #[test]
    fn undeclared_generic_in_field_is_rejected() {
        // class Box { value: T }  — T not declared → error
        let cls = class_with_generic(vec![], Type::Generic("T".into()));
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(
            matches!(err, SemanticError::UndeclaredTypeParam { ref param, .. } if param == "T"),
            "expected UndeclaredTypeParam for T, got {err:?}"
        );
    }

    #[test]
    fn undeclared_generic_in_method_param_is_rejected() {
        // class Foo { fn get(x: U): Void }  — U not declared → error
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "get".into(),
            params: vec![Param { name: "x".into(), ty: Type::Generic("U".into()) }],
            return_type: Type::Void,
            body: vec![],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("Foo", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::UndeclaredTypeParam { .. }));
    }

    #[test]
    fn generic_in_list_field_checks_inner_type() {
        // class Bag<T> { items: List<T> }  → ok
        let cls = class_with_generic(
            vec!["T".into()],
            Type::List(Box::new(Type::Generic("T".into()))),
        );
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    #[test]
    fn undeclared_generic_in_list_field_is_rejected() {
        // class Bag { items: List<X> }  — X not declared → error
        let cls = class_with_generic(
            vec![],
            Type::List(Box::new(Type::Generic("X".into()))),
        );
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::UndeclaredTypeParam { ref param, .. } if param == "X"));
    }

    // ── Enhanced type inference ───────────────────────────────────────────────

    #[test]
    fn field_access_on_this_infers_field_type() {
        // class App { count: Int; fn get(): Int { return this.count; } }  → ok
        let cls = ClassDecl {
            fields: vec![Field {
                visibility: Visibility::Private,
                ty: Type::Int,
                name: "count".into(),
            }],
            methods: vec![Method {
                visibility: Visibility::Public,
            is_async: false,
                name: "get".into(),
                params: vec![],
                return_type: Type::Int,
                body: vec![Stmt::Return { expr: Some(Expr::FieldAccess(
                    Box::new(Expr::This),
                    "count".into(),
                )), span: Span::dummy() }],
            }],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    #[test]
    fn field_access_type_mismatch_detected() {
        // class App { count: Int; fn get(): String { return this.count; } }  → error
        let cls = ClassDecl {
            fields: vec![Field {
                visibility: Visibility::Private,
                ty: Type::Int,
                name: "count".into(),
            }],
            methods: vec![Method {
                visibility: Visibility::Public,
            is_async: false,
                name: "get".into(),
                params: vec![],
                return_type: Type::String,
                body: vec![Stmt::Return { expr: Some(Expr::FieldAccess(
                    Box::new(Expr::This),
                    "count".into(),
                )), span: Span::dummy() }],
            }],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::TypeMismatch { .. }));
    }

    #[test]
    fn method_call_return_type_inferred() {
        // class Counter {
        //   fn get(): Int { return 0; }
        //   fn run(): Int { return this.get(); }
        // }  → ok (method return type propagated)
        let cls = ClassDecl {
            methods: vec![
                Method {
                    visibility: Visibility::Public,
            is_async: false,
                    name: "get".into(),
                    params: vec![],
                    return_type: Type::Int,
                    body: vec![Stmt::Return { expr: Some(Expr::IntLit(0)), span: Span::dummy() }],
                },
                Method {
                    visibility: Visibility::Public,
            is_async: false,
                    name: "run".into(),
                    params: vec![],
                    return_type: Type::Int,
                    body: vec![Stmt::Return { expr: Some(Expr::MethodCall {
                        receiver: Box::new(Expr::This),
                        method: "get".into(),
                        args: vec![],
                    }), span: Span::dummy() }],
                },
            ],
            ..make_class("Counter", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    #[test]
    fn inferred_let_used_in_binary() {
        // let x = 1; let y = x + 2  → should pass (y inferred as Int)
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                Stmt::Let { name: "x".into(), ty: None, init: Expr::IntLit(1) },
                Stmt::Let {
                    name: "y".into(),
                    ty: None,
                    init: Expr::Binary {
                        op: BinOp::Add,
                        left: Box::new(Expr::Ident("x".into())),
                        right: Box::new(Expr::IntLit(2)),
                    },
                },
            ],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    // ── Pass 7: Hindley-Milner tests ──────────────────────────────────────────

    // Helper: build a Unifier and call infer_expr_hm on a standalone expression.
    // `env` entries are treated as monotypes (no generalization).
    fn infer(expr: Expr, env: &[(&str, HmType)], cls_name: &str) -> HmType {
        let env_map: HmEnv = env
            .iter()
            .map(|(k, v)| (k.to_string(), TypeScheme::mono(v.clone())))
            .collect();
        let cls = make_class(cls_name, ClassKind::Class, Visibility::Public);
        let mut u = Unifier::new();
        let analyzer = SemanticAnalyzer::new();
        let ty = analyzer.infer_expr_hm(&expr, &env_map, &cls, &mut u).unwrap();
        u.apply(&ty)
    }

    // ── literal inference ─────────────────────────────────────────────────────

    #[test]
    fn hm_int_literal() {
        assert_eq!(infer(Expr::IntLit(0), &[], "A"), HmType::Int);
    }

    #[test]
    fn hm_bool_literal() {
        assert_eq!(infer(Expr::BoolLit(true), &[], "A"), HmType::Bool);
    }

    #[test]
    fn hm_string_literal() {
        assert_eq!(infer(Expr::StringLit("hi".into()), &[], "A"), HmType::String);
    }

    // ── arithmetic unification ────────────────────────────────────────────────

    #[test]
    fn hm_arithmetic_resolves_to_int() {
        // 1 + 2 → Int
        let expr = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::IntLit(1)),
            right: Box::new(Expr::IntLit(2)),
        };
        assert_eq!(infer(expr, &[], "A"), HmType::Int);
    }

    #[test]
    fn hm_comparison_resolves_to_bool() {
        // 3 < 5 → Bool
        let expr = Expr::Binary {
            op: BinOp::Lt,
            left: Box::new(Expr::IntLit(3)),
            right: Box::new(Expr::IntLit(5)),
        };
        assert_eq!(infer(expr, &[], "A"), HmType::Bool);
    }

    // ── lambda type inference ─────────────────────────────────────────────────

    #[test]
    fn hm_lambda_param_inferred_from_body() {
        // x => x + 1  —  x unifies with Int (via Int + Int)
        let body = Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Ident("x".into())),
            right: Box::new(Expr::IntLit(1)),
        };
        let lambda = Expr::Lambda {
            params: vec![Param { name: "x".into(), ty: Type::Custom("T".into()) }],
            body: Box::new(body),
        };
        let ty = infer(lambda, &[], "A");
        // should be Function([Int], Int)
        assert!(
            matches!(&ty, HmType::Function(params, ret) if params.len() == 1
                && params[0] == HmType::Int
                && **ret == HmType::Int),
            "expected (Int) => Int, got {:?}",
            ty
        );
    }

    #[test]
    fn hm_lambda_typed_param_preserved() {
        // (x: String) => x  — no unification needed, param stays String
        let lambda = Expr::Lambda {
            params: vec![Param { name: "x".into(), ty: Type::String }],
            body: Box::new(Expr::Ident("x".into())),
        };
        let ty = infer(lambda, &[], "A");
        assert!(
            matches!(&ty, HmType::Function(params, ret)
                if params[0] == HmType::String && **ret == HmType::String),
            "{:?}",
            ty
        );
    }

    // ── let propagation through method calls ──────────────────────────────────

    #[test]
    fn hm_let_propagates_method_return_type() {
        // class Counter { get(): Int { ... } }
        // let c = Counter();  let v = c.get();  v should be Int
        let counter = ClassDecl {
            methods: vec![Method {
                visibility: Visibility::Public,
            is_async: false,
                name: "get".into(),
                params: vec![],
                return_type: Type::Int,
                body: vec![Stmt::Return { expr: Some(Expr::IntLit(0)), span: Span::dummy() }],
            }],
            ..make_class("Counter", ClassKind::Class, Visibility::Public)
        };
        let checker_cls = make_class("App", ClassKind::Class, Visibility::Public);
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                // let c = Counter();
                Stmt::Let {
                    name: "c".into(),
                    ty: None,
                    init: Expr::Call { callee: "Counter".into(), args: vec![] },
                },
                // let v: Int = c.get();
                Stmt::Let {
                    name: "v".into(),
                    ty: Some(Type::Int),
                    init: Expr::MethodCall {
                        receiver: Box::new(Expr::Ident("c".into())),
                        method: "get".into(),
                        args: vec![],
                    },
                },
            ],
        };
        let app = ClassDecl { methods: vec![method], ..checker_cls };
        let mut analyzer = SemanticAnalyzer::new();
        analyzer.classes.insert("Counter".into(), counter);
        analyzer.classes.insert("App".into(), app.clone());
        // check_class_hm should not error
        assert!(analyzer.check_class_hm(&app).is_ok());
    }

    // ── type mismatch via HM ──────────────────────────────────────────────────

    #[test]
    fn hm_detects_string_plus_int_mismatch_in_subtraction() {
        // "hello" - 1  — Sub requires both Int → error
        let expr = Expr::Binary {
            op: BinOp::Sub,
            left: Box::new(Expr::StringLit("hello".into())),
            right: Box::new(Expr::IntLit(1)),
        };
        let env_map: HmEnv = HashMap::new();
        let cls = make_class("A", ClassKind::Class, Visibility::Public);
        let mut u = Unifier::new();
        let analyzer = SemanticAnalyzer::new();
        let res = analyzer.infer_expr_hm(&expr, &env_map, &cls, &mut u);
        assert!(res.is_err(), "expected type error for String - Int");
    }

    #[test]
    fn hm_detects_let_annotation_mismatch() {
        // let x: Bool = 42  — should fail through Pass 7
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![Stmt::Let {
                name: "x".into(),
                ty: Some(Type::Bool),
                init: Expr::IntLit(42),
            }],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        let err = SemanticAnalyzer::new().analyze(&prog).unwrap_err();
        assert!(matches!(err, SemanticError::TypeMismatch { .. }), "{err:?}");
    }

    // ── occurs check ─────────────────────────────────────────────────────────

    #[test]
    fn hm_occurs_check_prevents_infinite_type() {
        // Unifier::unify(α0, List<α0>) must fail with an occurs-check error.
        let mut u = Unifier::new();
        let id = u.fresh();
        let t1 = HmType::Var(id);
        let t2 = HmType::List(Box::new(HmType::Var(id)));
        let res = u.unify(&t1, &t2);
        assert!(res.is_err(), "expected infinite-type error");
        let msg = res.unwrap_err();
        assert!(msg.contains("infinite"), "message should mention 'infinite': {msg}");
    }

    // ── generic constructor instantiation ─────────────────────────────────────

    #[test]
    fn hm_generic_constructor_unifies_arg_type() {
        // class Box<T> { constructor(val: T) {} }
        // Box(42)  →  T unified with Int, result is Custom("Box")
        let box_cls = ClassDecl {
            type_params: vec!["T".into()],
            constructor: Some(Constructor {
                params: vec![Param { name: "val".into(), ty: Type::Generic("T".into()) }],
                body: vec![],
            }),
            ..make_class("Box", ClassKind::Class, Visibility::Public)
        };
        let caller = make_class("App", ClassKind::Class, Visibility::Public);
        let env_map: HmEnv = HashMap::new();
        let mut u = Unifier::new();
        let mut analyzer = SemanticAnalyzer::new();
        analyzer.classes.insert("Box".into(), box_cls);

        let call = Expr::Call {
            callee: "Box".into(),
            args: vec![Expr::IntLit(42)],
        };
        let ty = analyzer.infer_expr_hm(&call, &env_map, &caller, &mut u).unwrap();
        assert_eq!(ty, HmType::Custom("Box".into()));
    }

    // ── equality operand unification ──────────────────────────────────────────

    #[test]
    fn hm_equality_on_different_types_fails() {
        // 42 == "hi"  — Eq requires both sides to unify → error
        let expr = Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::IntLit(42)),
            right: Box::new(Expr::StringLit("hi".into())),
        };
        let env_map: HmEnv = HashMap::new();
        let cls = make_class("A", ClassKind::Class, Visibility::Public);
        let mut u = Unifier::new();
        let analyzer = SemanticAnalyzer::new();
        let res = analyzer.infer_expr_hm(&expr, &env_map, &cls, &mut u);
        assert!(res.is_err(), "expected type error for Int == String");
    }

    // ── Damas-Milner let-polymorphism ─────────────────────────────────────────

    /// `let id = x => x` generalizes to `∀α. α → α`.
    /// Using `id` at both Int and String in the same scope must pass —
    /// each call gets independent fresh variables.
    #[test]
    fn hm_let_polymorphism_identity() {
        // fn run(): Void {
        //   let id = x => x;   // id : ∀α. α → α
        //   let a: Int    = id(1);
        //   let b: String = id("hi");
        // }
        let id_lambda = Expr::Lambda {
            params: vec![Param { name: "x".into(), ty: Type::Custom("T".into()) }],
            body: Box::new(Expr::Ident("x".into())),
        };
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                Stmt::Let { name: "id".into(), ty: None, init: id_lambda },
                Stmt::Let {
                    name: "a".into(),
                    ty: Some(Type::Int),
                    init: Expr::Call { callee: "id".into(), args: vec![Expr::IntLit(1)] },
                },
                Stmt::Let {
                    name: "b".into(),
                    ty: Some(Type::String),
                    init: Expr::Call {
                        callee: "id".into(),
                        args: vec![Expr::StringLit("hi".into())],
                    },
                },
            ],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(
            SemanticAnalyzer::new().analyze(&prog).is_ok(),
            "let-polymorphism: id used at Int and String must both typecheck"
        );
    }

    /// Monomorphic let (type annotation) is NOT generalized.
    /// `let f: (Int) => Int = x => x` fixes `f` to Int→Int;
    /// using `f("hi")` must fail.
    #[test]
    fn hm_annotated_let_is_not_polymorphic() {
        // fn run(): Void {
        //   let f: (Int) => Int = x => x;
        //   let b: String = f("hi");   // must fail: f is monomorphic
        // }
        let id_lambda = Expr::Lambda {
            params: vec![Param { name: "x".into(), ty: Type::Custom("T".into()) }],
            body: Box::new(Expr::Ident("x".into())),
        };
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                Stmt::Let {
                    name: "f".into(),
                    ty: Some(Type::Function(vec![Type::Int], Box::new(Type::Int))),
                    init: id_lambda,
                },
                Stmt::Let {
                    name: "b".into(),
                    ty: Some(Type::String),
                    init: Expr::Call {
                        callee: "f".into(),
                        args: vec![Expr::StringLit("hi".into())],
                    },
                },
            ],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        // f("hi") should fail because f : Int → Int was fixed by annotation
        assert!(
            SemanticAnalyzer::new().analyze(&prog).is_err(),
            "annotated monomorphic let must not allow String argument"
        );
    }

    /// `let add = (x, y) => x + y` — both params unify to Int via the `+` body.
    /// The scheme is `∀. (Int, Int) → Int` (no free vars to generalize).
    /// Using it at Int twice must pass; passing a String must fail.
    #[test]
    fn hm_let_infers_int_function_from_arithmetic_body() {
        let add_lambda = Expr::Lambda {
            params: vec![
                Param { name: "x".into(), ty: Type::Custom("T".into()) },
                Param { name: "y".into(), ty: Type::Custom("T".into()) },
            ],
            body: Box::new(Expr::Binary {
                op: BinOp::Add,
                left: Box::new(Expr::Ident("x".into())),
                right: Box::new(Expr::Ident("y".into())),
            }),
        };
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                Stmt::Let { name: "add".into(), ty: None, init: add_lambda },
                // let r: Int = add(1, 2)  → ok
                Stmt::Let {
                    name: "r".into(),
                    ty: Some(Type::Int),
                    init: Expr::Call {
                        callee: "add".into(),
                        args: vec![Expr::IntLit(1), Expr::IntLit(2)],
                    },
                },
            ],
        };
        let cls = ClassDecl {
            methods: vec![method],
            ..make_class("App", ClassKind::Class, Visibility::Public)
        };
        let prog = make_program(vec![Declaration::Class(cls)], vec![]);
        assert!(SemanticAnalyzer::new().analyze(&prog).is_ok());
    }

    /// Two independent uses of a polymorphic binding with different types
    /// must not interfere with each other (independent unification chains).
    #[test]
    fn hm_polymorphic_uses_are_independent() {
        // let wrap = x => x;
        // let p = wrap(true);    — Bool use
        // let q = wrap(99);      — Int use  (must not see Bool constraint)
        let wrap_lambda = Expr::Lambda {
            params: vec![Param { name: "x".into(), ty: Type::Custom("T".into()) }],
            body: Box::new(Expr::Ident("x".into())),
        };
        let cls_def = make_class("App", ClassKind::Class, Visibility::Public);
        let analyzer = SemanticAnalyzer::new();
        let mut env: HmEnv = HashMap::new();
        let cls = cls_def.clone();
        let mut u = Unifier::new();

        // Insert let wrap = x => x into env (simulating check_stmt_hm LET rule).
        let lambda_ty = analyzer.infer_expr_hm(&wrap_lambda, &env, &cls, &mut u).unwrap();
        let scheme = generalize(&env, &lambda_ty, &mut u);
        assert!(!scheme.quantified.is_empty(), "wrap should be polymorphic");
        env.insert("wrap".into(), scheme);

        // Use wrap(true) → should produce Bool.
        let call_bool = Expr::Call {
            callee: "wrap".into(),
            args: vec![Expr::BoolLit(true)],
        };
        // Use wrap(99) → should produce Int (independent var).
        let call_int = Expr::Call {
            callee: "wrap".into(),
            args: vec![Expr::IntLit(99)],
        };

        // Both calls through check_stmt_hm must not interfere.
        let method = Method {
            visibility: Visibility::Public,
            is_async: false,
            name: "run".into(),
            params: vec![],
            return_type: Type::Void,
            body: vec![
                Stmt::Let { name: "wrap".into(), ty: None, init: Expr::Lambda {
                    params: vec![Param { name: "x".into(), ty: Type::Custom("T".into()) }],
                    body: Box::new(Expr::Ident("x".into())),
                }},
                Stmt::Let { name: "p".into(), ty: Some(Type::Bool), init: call_bool },
                Stmt::Let { name: "q".into(), ty: Some(Type::Int), init: call_int },
            ],
        };
        let cls2 = ClassDecl {
            methods: vec![method],
            ..cls_def
        };
        let prog = make_program(vec![Declaration::Class(cls2)], vec![]);
        assert!(
            SemanticAnalyzer::new().analyze(&prog).is_ok(),
            "polymorphic uses of wrap must not interfere"
        );
    }
}
