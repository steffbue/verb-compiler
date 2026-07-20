use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{renamed_keyword, Token, TokenKind};

pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let (imports, std_imports) = p.imports()?;
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            return Err(p.err("'import' must appear before any other statement"));
        }
        body.push(p.statement()?);
    }
    Ok(Program { imports, std_imports, body })
}

/// Same grammar as `parse`, but doesn't stop at the first syntax error:
/// after a bad statement it synchronizes to the next likely statement
/// boundary and keeps going, collecting every statement it could parse
/// and every error it hit along the way. Meant for editor tooling (an
/// LSP wants every syntax mistake in a file at once); the compiler proper
/// keeps using `parse`, which stops at the first error like a normal
/// compiler.
pub fn parse_recovering(toks: Vec<Token>) -> (Program, Vec<CompileError>) {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let mut imports = Vec::new();
    let mut std_imports = Vec::new();
    let mut errors = Vec::new();
    while p.check(&TokenKind::Import) {
        match p.import_stmt() {
            Ok(ImportStmt::Mod(name)) => {
                if !imports.contains(&name) { imports.push(name); }
            }
            Ok(ImportStmt::Std(name)) => {
                if !std_imports.contains(&name) { std_imports.push(name); }
            }
            Err(e) => { errors.push(e); p.synchronize(); }
        }
    }
    let mut body = Vec::new();
    while !p.check(&TokenKind::Eof) {
        if p.check(&TokenKind::Import) {
            errors.push(p.err("'import' must appear before any other statement"));
            p.advance();
            continue;
        }
        let pos_before = p.pos;
        match p.statement() {
            Ok(s) => body.push(s),
            Err(e) => {
                errors.push(e);
                // An error deep inside an unfinished `make` body leaves
                // fn_depth incremented (the decrement after `block()?`
                // never runs); recovery always resumes at top level, so
                // depth tracking mid-error can't be trusted regardless.
                p.fn_depth = 0;
                p.synchronize();
                // Some productions (e.g. `return` outside a function)
                // fail without consuming their own token, and
                // `synchronize` treats that same token as an
                // already-safe boundary it stops at without advancing
                // either — so the two together can make zero progress.
                // Force one token forward whenever that happens, or the
                // next iteration would hit the exact same error forever.
                if p.pos == pos_before {
                    p.advance();
                }
            }
        }
    }
    (Program { imports, std_imports, body }, errors)
}

enum ImportStmt {
    Mod(String),
    Std(String),
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    fn_depth: u32,
}

impl Parser {
    fn peek(&self) -> &TokenKind { &self.toks[self.pos].kind }
    fn peek2(&self) -> &TokenKind {
        &self.toks[(self.pos + 1).min(self.toks.len() - 1)].kind
    }
    fn here(&self) -> (u32, u32) {
        let t = &self.toks[self.pos];
        (t.line, t.col)
    }
    fn check(&self, k: &TokenKind) -> bool { self.peek() == k }
    fn advance(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 { self.pos += 1; }
        t
    }
    fn matches(&mut self, k: &TokenKind) -> bool {
        if self.check(k) { self.advance(); true } else { false }
    }
    fn expect(&mut self, k: &TokenKind, what: &str) -> Result<Token, CompileError> {
        if self.check(k) { Ok(self.advance()) } else { Err(self.err_found(format!("expected {what}"))) }
    }
    fn err(&self, msg: impl Into<String>) -> CompileError {
        let (l, c) = self.here();
        CompileError::new(msg, l, c)
    }
    /// Error that names the token actually found; hints when it is a pre-sweep keyword.
    fn err_found(&self, msg: impl Into<String>) -> CompileError {
        let found = self.peek();
        let e = self.err(format!("{}, found {}", msg.into(), found.describe()));
        if let TokenKind::Ident(n) = found {
            if let Some(new) = renamed_keyword(n) {
                return e.with_hint(format!("'{n}' was renamed to '{new}'"));
            }
        }
        e
    }
    fn expect_ident(&mut self, what: &str) -> Result<(String, u32, u32), CompileError> {
        let (l, c) = self.here();
        match self.peek().clone() {
            TokenKind::Ident(n) => { self.advance(); Ok((n, l, c)) }
            _ => Err(self.err_found(format!("expected {what}"))),
        }
    }

    fn imports(&mut self) -> Result<(Vec<String>, Vec<String>), CompileError> {
        let mut imports = Vec::new();
        let mut std_imports = Vec::new();
        while self.check(&TokenKind::Import) {
            match self.import_stmt()? {
                ImportStmt::Mod(name) => { if !imports.contains(&name) { imports.push(name); } }
                ImportStmt::Std(name) => { if !std_imports.contains(&name) { std_imports.push(name); } }
            }
        }
        Ok((imports, std_imports))
    }

    fn import_stmt(&mut self) -> Result<ImportStmt, CompileError> {
        self.advance(); // 'import'
        if self.matches(&TokenKind::Mod) {
            let (name, ..) = self.expect_ident("library name after 'mod'")?;
            self.expect(&TokenKind::Semi, "';'")?;
            return Ok(ImportStmt::Mod(name));
        }
        self.expect(&TokenKind::Std, "'mod' or 'std'")?;
        let (name, l, c) = self.expect_ident("module name after 'std'")?;
        if name != "io" {
            return Err(CompileError::new(
                format!("unknown std module '{name}' (known std modules: io)"),
                l, c,
            ));
        }
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(ImportStmt::Std(name))
    }

    /// Skip tokens until we're likely sitting at the start of a new
    /// statement, so `parse_recovering` can resume instead of giving up
    /// on the rest of the file after one error.
    fn synchronize(&mut self) {
        loop {
            match self.peek() {
                TokenKind::Eof => return,
                // consuming the ';' or 'end' means the *next* token is
                // the start of whatever comes after the broken statement
                TokenKind::Semi | TokenKind::End => { self.advance(); return; }
                TokenKind::Assign | TokenKind::Declare | TokenKind::Make | TokenKind::Return
                | TokenKind::Check | TokenKind::Repeat | TokenKind::Loop | TokenKind::Begin => return,
                _ => { self.advance(); }
            }
        }
    }

    fn statement(&mut self) -> Result<Stmt, CompileError> {
        match self.peek() {
            TokenKind::Assign => self.assign_stmt(true),
            TokenKind::Declare => self.declare_stmt(),
            TokenKind::Make => self.fn_stmt(),
            TokenKind::Return => self.return_stmt(),
            TokenKind::Check => self.if_stmt(),
            TokenKind::Repeat => self.while_stmt(),
            TokenKind::Loop => self.for_stmt(),
            TokenKind::Begin => Ok(Stmt::Block(self.block()?)),
            // old statement keywords lex as identifiers now — catch them for a rename hint
            TokenKind::Ident(n) if *self.peek2() != TokenKind::Be
                && matches!(n.as_str(), "if" | "else" | "while" | "for" | "fn") =>
            {
                let new = renamed_keyword(n).unwrap();
                Err(self.err(format!("unknown statement keyword '{n}'"))
                    .with_hint(format!("'{n}' was renamed to '{new}'")))
            }
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(true),
            _ => {
                let e = self.expression()?;
                self.expect(&TokenKind::Semi, "';'")?;
                Ok(Stmt::ExprStmt(e))
            }
        }
    }

    fn assign_stmt(&mut self, semi: bool) -> Result<Stmt, CompileError> {
        self.advance(); // assign
        let (name, _, _) = self.expect_ident("variable name after 'assign'")?;
        let value = self.expression()?;
        if semi { self.expect(&TokenKind::Semi, "';'")?; }
        Ok(Stmt::Assign { name, value })
    }

    fn declare_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // declare
        let (name, _, _) = self.expect_ident("variable name after 'declare'")?;
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(Stmt::Declare { name })
    }

    fn reassign_stmt(&mut self, semi: bool) -> Result<Stmt, CompileError> {
        let (name, line, col) = self.expect_ident("variable name")?;
        self.expect(&TokenKind::Be, "'be'")?;
        let value = self.expression()?;
        if semi { self.expect(&TokenKind::Semi, "';'")?; }
        Ok(Stmt::Reassign { name, value, line, col })
    }

    fn fn_stmt(&mut self) -> Result<Stmt, CompileError> {
        let (line, col) = self.here();
        self.advance(); // make
        let (name, _, _) = self.expect_ident("function name")?;
        self.expect(&TokenKind::LParen, "'('")?;
        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                params.push(self.expect_ident("parameter name")?.0);
                if !self.matches(&TokenKind::Comma) { break; }
            }
        }
        self.expect(&TokenKind::RParen, "')'")?;
        self.fn_depth += 1;
        let body = self.block()?;
        self.fn_depth -= 1;
        Ok(Stmt::Fn { name, params, body, line, col })
    }

    fn return_stmt(&mut self) -> Result<Stmt, CompileError> {
        if self.fn_depth == 0 {
            return Err(self.err("'return' outside function"));
        }
        self.advance(); // return
        let value = if self.check(&TokenKind::Semi) { None } else { Some(self.expression()?) };
        self.expect(&TokenKind::Semi, "';'")?;
        Ok(Stmt::Return { value })
    }

    fn if_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // check
        let cond = self.expression()?;
        let then_body = self.block()?;
        let else_body = if self.matches(&TokenKind::Orelse) {
            if self.check(&TokenKind::Check) {
                Some(vec![self.if_stmt()?]) // orelse check …
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If { cond, then_body, else_body })
    }

    fn while_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // repeat
        let cond = self.expression()?;
        let body = self.block()?;
        Ok(Stmt::While { cond, body })
    }

    fn for_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // loop
        let init = match self.peek() {
            TokenKind::Assign => self.assign_stmt(true)?, // consumes ';'
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(true)?,
            _ => return Err(self.err("expected 'assign' or reassignment in for-init")),
        };
        let cond = self.expression()?;
        self.expect(&TokenKind::Semi, "';'")?;
        let incr = match self.peek() {
            TokenKind::Ident(_) if *self.peek2() == TokenKind::Be => self.reassign_stmt(false)?,
            _ => Stmt::ExprStmt(self.expression()?),
        };
        let mut body = self.block()?;
        body.push(incr);
        Ok(Stmt::Block(vec![init, Stmt::While { cond, body }]))
    }

    fn block(&mut self) -> Result<Vec<Stmt>, CompileError> {
        self.expect(&TokenKind::Begin, "'begin'")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::End) && !self.check(&TokenKind::Eof) {
            stmts.push(self.statement()?);
        }
        self.expect(&TokenKind::End, "'end'")?;
        Ok(stmts)
    }

    // ----- expressions: precedence climbing -----
    pub(crate) fn expression(&mut self) -> Result<Expr, CompileError> { self.or_expr() }

    fn or_expr(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.and_expr()?;
        loop {
            let (line, col) = self.here();
            if !self.matches(&TokenKind::Or) { break; }
            let r = self.and_expr()?;
            e = Expr::Binary { op: BinOp::Or, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn and_expr(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.equality()?;
        loop {
            let (line, col) = self.here();
            if !self.matches(&TokenKind::And) { break; }
            let r = self.equality()?;
            e = Expr::Binary { op: BinOp::And, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn equality(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.comparison()?;
        loop {
            let op = match self.peek() {
                TokenKind::Equals => BinOp::Eq,
                TokenKind::Differs => BinOp::Ne,
                _ => break,
            };
            let (line, col) = self.here();
            self.advance();
            let r = self.comparison()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn comparison(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.term()?;
        loop {
            let op = match self.peek() {
                TokenKind::Trails => BinOp::Lt,
                TokenKind::Beats => BinOp::Gt,
                TokenKind::Atmost => BinOp::Le,
                TokenKind::Atleast => BinOp::Ge,
                _ => break,
            };
            let (line, col) = self.here();
            self.advance();
            let r = self.term()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn term(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.factor()?;
        loop {
            let op = match self.peek() {
                TokenKind::Add => BinOp::Add,
                TokenKind::Sub => BinOp::Sub,
                TokenKind::Join => BinOp::Concat,
                _ => break,
            };
            let (line, col) = self.here();
            self.advance();
            let r = self.factor()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn factor(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Times => BinOp::Mul,
                TokenKind::Div => BinOp::Div,
                TokenKind::Mod => BinOp::Mod,
                _ => break,
            };
            let (line, col) = self.here();
            self.advance();
            let r = self.unary()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r), line, col };
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, CompileError> {
        let op = match self.peek() {
            TokenKind::Not => Some(UnOp::Not),
            TokenKind::Neg => Some(UnOp::Neg),
            _ => None,
        };
        if let Some(op) = op {
            let (line, col) = self.here();
            self.advance();
            let e = self.unary()?;
            return Ok(Expr::Unary { op, expr: Box::new(e), line, col });
        }
        self.call()
    }
    fn call(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.primary()?;
        while self.check(&TokenKind::LParen) {
            let (l, c) = self.here();
            self.advance();
            let mut args = Vec::new();
            if !self.check(&TokenKind::RParen) {
                loop {
                    args.push(self.expression()?);
                    if !self.matches(&TokenKind::Comma) { break; }
                }
            }
            self.expect(&TokenKind::RParen, "')'")?;
            e = Expr::Call { callee: Box::new(e), args, line: l, col: c };
        }
        Ok(e)
    }
    fn primary(&mut self) -> Result<Expr, CompileError> {
        let (l, c) = self.here();
        let e = match self.peek().clone() {
            TokenKind::Int(v) => { self.advance(); Expr::Int(v) }
            TokenKind::Float(v) => { self.advance(); Expr::Float(v) }
            TokenKind::Str(s) => { self.advance(); Expr::Str(s) }
            TokenKind::True => { self.advance(); Expr::Bool(true) }
            TokenKind::False => { self.advance(); Expr::Bool(false) }
            TokenKind::Nil => { self.advance(); Expr::Nil }
            TokenKind::Ident(n) => { self.advance(); Expr::Var(n, l, c) }
            TokenKind::LParen => {
                self.advance();
                let e = self.expression()?;
                self.expect(&TokenKind::RParen, "')'")?;
                e
            }
            _ => return Err(self.err_found("expected expression")),
        };
        Ok(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn expr(src: &str) -> Expr {
        // parse "src;" as a single expression statement
        let prog = parse(lex(&format!("{src};")).unwrap()).unwrap();
        match prog.body.into_iter().next().unwrap() {
            Stmt::ExprStmt(e) => e,
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn precedence_mul_over_plus() {
        match expr("1 add 2 times 3") {
            Expr::Binary { op: BinOp::Add, lhs, rhs, .. } => {
                assert_eq!(*lhs, Expr::Int(1));
                assert!(matches!(*rhs, Expr::Binary { op: BinOp::Mul, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unary_and_grouping() {
        match expr("neg (1 add 2)") {
            Expr::Unary { op: UnOp::Neg, expr: inner, .. } => {
                assert!(matches!(*inner, Expr::Binary { op: BinOp::Add, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn call_with_args() {
        let e = expr("sum(1, 2)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Var(ref n, _, _) if n == "sum"));
                assert_eq!(args, vec![Expr::Int(1), Expr::Int(2)]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn logic_precedence_or_lowest() {
        let e = expr("true or false and true");
        assert!(matches!(e, Expr::Binary { op: BinOp::Or, .. }));
    }

    #[test]
    fn parses_assign_and_reassign() {
        let p = parse(lex("assign x 10; x be x add 1;").unwrap()).unwrap();
        assert!(matches!(&p.body[0], Stmt::Assign { name, .. } if name == "x"));
        assert!(matches!(&p.body[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_declare() {
        let p = parse(lex("declare x; x be 1;").unwrap()).unwrap();
        assert!(matches!(&p.body[0], Stmt::Declare { name } if name == "x"));
        assert!(matches!(&p.body[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_if_else_chain() {
        let p = parse(lex("check true begin print(1); end orelse check false begin print(2); end orelse begin print(3); end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::If { else_body: Some(eb), .. } => {
                assert!(matches!(&eb[0], Stmt::If { else_body: Some(_), .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn desugars_for_to_while() {
        let p = parse(lex("loop assign i 0; i trails 10; i be i add 1 begin print(i); end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::Block(inner) => {
                assert!(matches!(&inner[0], Stmt::Assign { name, .. } if name == "i"));
                match &inner[1] {
                    Stmt::While { body, .. } => {
                        assert!(matches!(body.last().unwrap(), Stmt::Reassign { name, .. } if name == "i"));
                    }
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_fn_and_return() {
        let p = parse(lex("make sum(a, b) begin return a add b; end").unwrap()).unwrap();
        match &p.body[0] {
            Stmt::Fn { name, params, body, .. } => {
                assert_eq!(name, "sum");
                assert_eq!(params, &vec!["a".to_string(), "b".to_string()]);
                assert!(matches!(&body[0], Stmt::Return { value: Some(_) }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_return_at_top_level() {
        assert!(parse(lex("return 1;").unwrap()).is_err());
    }

    #[test]
    fn parses_single_import() {
        let p = parse(lex("import mod mathlib;").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert!(p.body.is_empty());
    }

    #[test]
    fn parses_multiple_imports_and_dedups() {
        let p = parse(lex("import mod mathlib; import mod strlib; import mod mathlib;").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string(), "strlib".to_string()]);
    }

    #[test]
    fn import_before_body_is_fine() {
        let p = parse(lex("import mod mathlib; print(1);").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert_eq!(p.body.len(), 1);
    }

    #[test]
    fn import_after_a_statement_is_a_compile_error() {
        let err = parse(lex("print(1); import mod mathlib;").unwrap()).unwrap_err();
        assert!(err.msg.contains("must appear before"), "{}", err.msg);
    }

    #[test]
    fn program_with_no_imports_has_empty_imports_vec() {
        let p = parse(lex("print(1);").unwrap()).unwrap();
        assert!(p.imports.is_empty());
    }

    #[test]
    fn recovering_collects_imports_too() {
        let src = "import mod mathlib; print(1);";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(prog.imports, vec!["mathlib".to_string()]);
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn parses_std_io_import() {
        let p = parse(lex("import std io;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["io".to_string()]);
        assert!(p.imports.is_empty());
    }

    #[test]
    fn dedups_repeated_std_import() {
        let p = parse(lex("import std io; import std io;").unwrap()).unwrap();
        assert_eq!(p.std_imports, vec!["io".to_string()]);
    }

    #[test]
    fn std_and_mod_imports_coexist() {
        let p = parse(lex("import mod mathlib; import std io; print(1);").unwrap()).unwrap();
        assert_eq!(p.imports, vec!["mathlib".to_string()]);
        assert_eq!(p.std_imports, vec!["io".to_string()]);
        assert_eq!(p.body.len(), 1);
    }

    #[test]
    fn unknown_std_module_is_a_compile_error() {
        let err = parse(lex("import std vector;").unwrap()).unwrap_err();
        assert!(err.msg.contains("unknown std module 'vector'"), "{}", err.msg);
        assert!(err.msg.contains("io"), "{}", err.msg);
    }

    #[test]
    fn std_import_after_a_statement_is_a_compile_error() {
        let err = parse(lex("print(1); import std io;").unwrap()).unwrap_err();
        assert!(err.msg.contains("must appear before"), "{}", err.msg);
    }

    #[test]
    fn recovering_collects_std_imports_too() {
        let src = "import std io; print(1);";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(prog.std_imports, vec!["io".to_string()]);
        assert_eq!(prog.body.len(), 1);
    }

    #[test]
    fn recovering_collects_every_error_across_semicolons() {
        // three broken statements in a row, each missing its expression
        let src = "assign a ; assign b ; assign c ;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(prog.body.is_empty());
    }

    #[test]
    fn recovering_keeps_good_statements_around_a_bad_one() {
        let src = "assign a 1; assign b ; assign c 3;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(prog.body.len(), 2);
        assert!(matches!(&prog.body[0], Stmt::Assign { name, .. } if name == "a"));
        assert!(matches!(&prog.body[1], Stmt::Assign { name, .. } if name == "c"));
    }

    #[test]
    fn recovering_resyncs_at_begin_after_a_broken_condition() {
        // `check`'s condition is broken (missing entirely); `begin` is
        // itself a safe restart point, so recovery re-parses the rest
        // as a bare block, then keeps going to the statement after it
        let src = "check begin print(1); end assign x 2;";
        let (prog, errors) = parse_recovering(lex(src).unwrap());
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(matches!(prog.body.last(), Some(Stmt::Assign { name, .. }) if name == "x"));
    }

    #[test]
    fn recovering_resets_fn_depth_so_return_is_still_rejected_after_an_error() {
        // error inside the (unclosed) `make` body must not leave fn_depth
        // stuck incremented, or `return` at top level after it would
        // wrongly be accepted
        let src = "make broken(n) begin assign ; return 1;";
        let (_, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.iter().any(|e| e.msg.contains("return")), "{errors:?}");
    }

    #[test]
    fn recovering_matches_parse_on_valid_input() {
        let src = "make sum(a, b) begin return a add b; end print(sum(1, 2));";
        let ok = parse(lex(src).unwrap()).unwrap();
        let (recovering, errors) = parse_recovering(lex(src).unwrap());
        assert!(errors.is_empty());
        assert_eq!(ok, recovering);
    }
}
