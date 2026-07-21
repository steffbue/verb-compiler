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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign { name: String, value: Expr, line: u32, col: u32 },
    Declare { name: String, line: u32, col: u32 },
    Reassign { name: String, value: Expr, line: u32, col: u32 },
    ExprStmt(Expr, u32, u32),
    If { cond: Expr, then_body: Vec<Stmt>, else_body: Option<Vec<Stmt>>, line: u32, col: u32 },
    While { cond: Expr, body: Vec<Stmt>, line: u32, col: u32 },
    Fn { name: String, params: Vec<String>, body: Vec<Stmt>, line: u32, col: u32 },
    Return { value: Option<Expr> },
    Block(Vec<Stmt>, u32, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub imports: Vec<String>,
    pub std_imports: Vec<String>,
    pub verb_imports: Vec<String>,
    pub body: Vec<Stmt>,
}
