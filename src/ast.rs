#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp { Add, Sub, Mul, Div, Mod, Eq, Ne, Lt, Gt, Le, Ge, And, Or, Concat }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp { Neg, Not }

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
    Var(String, u32, u32), // name, line, col (for undefined-variable errors)
    Binary { op: BinOp, lhs: Box<Expr>, rhs: Box<Expr>, line: u32, col: u32 },
    Unary { op: UnOp, expr: Box<Expr>, line: u32, col: u32 },
    Call { callee: Box<Expr>, args: Vec<Expr>, line: u32, col: u32 },
    ArrayLit(Vec<Expr>),
    FieldGet { obj: Box<Expr>, field: String, line: u32, col: u32 }, // <field> of <expr>
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { name: String, value: Expr },                    // assign x expr;
    Declare { name: String },                                // declare x;  (starts as nil)
    Reassign { name: String, value: Expr, line: u32, col: u32 }, // x be expr;
    ExprStmt(Expr),
    If { cond: Expr, then_body: Vec<Stmt>, else_body: Option<Vec<Stmt>> },
    While { cond: Expr, body: Vec<Stmt> },
    Fn { name: String, params: Vec<String>, body: Vec<Stmt>, line: u32, col: u32 },
    Return { value: Option<Expr> },
    Block(Vec<Stmt>),
    Record { name: String, fields: Vec<String>, line: u32, col: u32 }, // record Name begin f, g end
    FieldSet { obj: Expr, field: String, value: Expr, line: u32, col: u32 }, // <field> of <expr> be <value>;
    // choice Name begin V1(a, b) or V2(c) or V3 end -- a tagged-union type
    // declaration. Each variant has a name and an ordered field list.
    Choice { name: String, variants: Vec<(String, Vec<String>)>, line: u32, col: u32 },
    // match <expr> begin when V(a, b) begin .. end .. otherwise begin .. end end
    Match { scrutinee: Expr, arms: Vec<MatchArm>, otherwise: Option<Vec<Stmt>>, line: u32, col: u32 },
}

/// One `when V(a, b) begin .. end` arm of a `match`: the variant name, the
/// names it binds each field to (positional), and the body run on a match.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub variant: String,
    pub bindings: Vec<String>,
    pub body: Vec<Stmt>,
}

/// A native scalar type usable in a typed extern signature
/// (`import mod ... exposing f(float) -> float;`). Scalars only in v1 —
/// `Str` is parsed but not yet marshalled by codegen (documented).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty { Int, Float, Str, Bool }

/// A declared typed signature for an external (FFI) function, carried on
/// an `import mod` statement. When present, codegen lowers calls to `name`
/// as native (unboxed) LLVM calls instead of the default tagged-value path.
#[derive(Debug, Clone, PartialEq)]
pub struct ExternSig {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
    pub extern_sigs: Vec<ExternSig>,
    pub body: Vec<Stmt>,
}

use std::collections::HashSet;

/// Free variables of a function: the names it *reads* (via `Expr::Var`) or
/// *reassigns* (`x be ...`) that are not bound by its own parameters, its own
/// name (self-recursion), or a local `assign`/`declare` that reaches the use
/// site. Blocks (`begin/end`, `check`, `repeat`) introduce nested binding
/// scopes, mirroring codegen's `self.scopes` stack, so a name assigned inside
/// a block is local only there. A nested `make`'s *own* free variables (minus
/// its params/name and anything bound out here) are transitively free out here
/// too -- the enclosing function must capture them so it can, in turn, hand
/// them to the inner one. Returns names in first-seen order (that order fixes
/// each capture's env-slot index, so it must be identical at closure creation
/// and inside the body).
///
/// The list is a *candidate* set: codegen keeps only names that resolve to an
/// enclosing local (capture); names that resolve to a global, a record type,
/// or a builtin (`print`, `Point`, ...) fall through untouched, exactly as
/// before.
pub fn free_vars(name: &str, params: &[String], body: &[Stmt]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut bound: Vec<HashSet<String>> = vec![{
        let mut s: HashSet<String> = params.iter().cloned().collect();
        s.insert(name.to_string());
        s
    }];
    collect_free_stmts(body, &mut bound, &mut out, &mut seen);
    out
}

fn is_bound(bound: &[HashSet<String>], name: &str) -> bool {
    bound.iter().any(|s| s.contains(name))
}

fn note_free(
    name: &str,
    bound: &[HashSet<String>],
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    if !is_bound(bound, name) && seen.insert(name.to_string()) {
        out.push(name.to_string());
    }
}

fn collect_free_stmts(
    stmts: &[Stmt],
    bound: &mut Vec<HashSet<String>>,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for s in stmts {
        collect_free_stmt(s, bound, out, seen);
    }
}

fn collect_free_stmt(
    s: &Stmt,
    bound: &mut Vec<HashSet<String>>,
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match s {
        Stmt::Assign { name, value } => {
            collect_free_expr(value, bound, out, seen);
            bound.last_mut().unwrap().insert(name.clone());
        }
        Stmt::Declare { name } => {
            bound.last_mut().unwrap().insert(name.clone());
        }
        Stmt::Reassign { name, value, .. } => {
            // `x be ...` requires x to already exist: it is a *use* of an
            // existing binding (so a capture if x lives in an enclosing frame).
            note_free(name, bound, out, seen);
            collect_free_expr(value, bound, out, seen);
        }
        Stmt::ExprStmt(e) => collect_free_expr(e, bound, out, seen),
        Stmt::Return { value } => {
            if let Some(e) = value {
                collect_free_expr(e, bound, out, seen);
            }
        }
        Stmt::If { cond, then_body, else_body } => {
            collect_free_expr(cond, bound, out, seen);
            bound.push(HashSet::new());
            collect_free_stmts(then_body, bound, out, seen);
            bound.pop();
            if let Some(eb) = else_body {
                bound.push(HashSet::new());
                collect_free_stmts(eb, bound, out, seen);
                bound.pop();
            }
        }
        Stmt::While { cond, body } => {
            collect_free_expr(cond, bound, out, seen);
            bound.push(HashSet::new());
            collect_free_stmts(body, bound, out, seen);
            bound.pop();
        }
        Stmt::Block(stmts) => {
            bound.push(HashSet::new());
            collect_free_stmts(stmts, bound, out, seen);
            bound.pop();
        }
        Stmt::Fn { name, params, body, .. } => {
            // The nested fn's name binds in the current scope...
            bound.last_mut().unwrap().insert(name.clone());
            // ...and its own free vars, if not satisfied by a binding out here,
            // are free out here too (transitive capture).
            for n in free_vars(name, params, body) {
                note_free(&n, bound, out, seen);
            }
        }
        Stmt::Record { .. } => {}
        Stmt::FieldSet { obj, value, .. } => {
            collect_free_expr(obj, bound, out, seen);
            collect_free_expr(value, bound, out, seen);
        }
        // A `choice` type declaration binds no runtime names (like `record`).
        Stmt::Choice { .. } => {}
        Stmt::Match { scrutinee, arms, otherwise, .. } => {
            collect_free_expr(scrutinee, bound, out, seen);
            // Each arm's bindings are a fresh scope frame (like params), local
            // to that arm's body only.
            for arm in arms {
                let mut frame = HashSet::new();
                for b in &arm.bindings { frame.insert(b.clone()); }
                bound.push(frame);
                collect_free_stmts(&arm.body, bound, out, seen);
                bound.pop();
            }
            if let Some(ob) = otherwise {
                bound.push(HashSet::new());
                collect_free_stmts(ob, bound, out, seen);
                bound.pop();
            }
        }
    }
}

fn collect_free_expr(
    e: &Expr,
    bound: &[HashSet<String>],
    out: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    match e {
        Expr::Var(name, ..) => note_free(name, bound, out, seen),
        Expr::Binary { lhs, rhs, .. } => {
            collect_free_expr(lhs, bound, out, seen);
            collect_free_expr(rhs, bound, out, seen);
        }
        Expr::Unary { expr, .. } => collect_free_expr(expr, bound, out, seen),
        Expr::Call { callee, args, .. } => {
            collect_free_expr(callee, bound, out, seen);
            for a in args {
                collect_free_expr(a, bound, out, seen);
            }
        }
        Expr::ArrayLit(items) => {
            for it in items {
                collect_free_expr(it, bound, out, seen);
            }
        }
        Expr::FieldGet { obj, .. } => collect_free_expr(obj, bound, out, seen),
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Nil => {}
    }
}
