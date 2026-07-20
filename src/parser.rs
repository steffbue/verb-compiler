use crate::ast::*;
use crate::error::CompileError;
use crate::lexer::{Token, TokenKind};

pub fn parse(toks: Vec<Token>) -> Result<Vec<Stmt>, CompileError> {
    let mut p = Parser { toks, pos: 0, fn_depth: 0 };
    let mut stmts = Vec::new();
    while !p.check(&TokenKind::Eof) {
        stmts.push(p.statement()?);
    }
    Ok(stmts)
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
        if self.check(k) { Ok(self.advance()) } else { Err(self.err(format!("expected {what}"))) }
    }
    fn err(&self, msg: impl Into<String>) -> CompileError {
        let (l, c) = self.here();
        CompileError::new(msg, l, c)
    }
    fn expect_ident(&mut self, what: &str) -> Result<(String, u32, u32), CompileError> {
        let (l, c) = self.here();
        match self.peek().clone() {
            TokenKind::Ident(n) => { self.advance(); Ok((n, l, c)) }
            _ => Err(self.err(format!("expected {what}"))),
        }
    }

    fn statement(&mut self) -> Result<Stmt, CompileError> {
        match self.peek() {
            TokenKind::Assign => self.assign_stmt(true),
            TokenKind::Fn => self.fn_stmt(),
            TokenKind::Return => self.return_stmt(),
            TokenKind::If => self.if_stmt(),
            TokenKind::While => self.while_stmt(),
            TokenKind::For => self.for_stmt(),
            TokenKind::Begin => Ok(Stmt::Block(self.block()?)),
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

    fn reassign_stmt(&mut self, semi: bool) -> Result<Stmt, CompileError> {
        let (name, line, col) = self.expect_ident("variable name")?;
        self.expect(&TokenKind::Be, "'be'")?;
        let value = self.expression()?;
        if semi { self.expect(&TokenKind::Semi, "';'")?; }
        Ok(Stmt::Reassign { name, value, line, col })
    }

    fn fn_stmt(&mut self) -> Result<Stmt, CompileError> {
        let (line, col) = self.here();
        self.advance(); // fn
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
        self.advance(); // if
        let cond = self.expression()?;
        let then_body = self.block()?;
        let else_body = if self.matches(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                Some(vec![self.if_stmt()?]) // else if
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        Ok(Stmt::If { cond, then_body, else_body })
    }

    fn while_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // while
        let cond = self.expression()?;
        let body = self.block()?;
        Ok(Stmt::While { cond, body })
    }

    fn for_stmt(&mut self) -> Result<Stmt, CompileError> {
        self.advance(); // for
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
        while self.matches(&TokenKind::Or) {
            let r = self.and_expr()?;
            e = Expr::Binary { op: BinOp::Or, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn and_expr(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.equality()?;
        while self.matches(&TokenKind::And) {
            let r = self.equality()?;
            e = Expr::Binary { op: BinOp::And, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn equality(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.comparison()?;
        loop {
            let op = match self.peek() {
                TokenKind::Eqeq => BinOp::Eq,
                TokenKind::Neq => BinOp::Ne,
                _ => break,
            };
            self.advance();
            let r = self.comparison()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn comparison(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.term()?;
        loop {
            let op = match self.peek() {
                TokenKind::Lo => BinOp::Lt,
                TokenKind::Hi => BinOp::Gt,
                TokenKind::Loeq => BinOp::Le,
                TokenKind::Hieq => BinOp::Ge,
                _ => break,
            };
            self.advance();
            let r = self.term()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn term(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.factor()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                TokenKind::Concat => BinOp::Concat,
                _ => break,
            };
            self.advance();
            let r = self.factor()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn factor(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Mul => BinOp::Mul,
                TokenKind::Div => BinOp::Div,
                TokenKind::Mod => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let r = self.unary()?;
            e = Expr::Binary { op, lhs: Box::new(e), rhs: Box::new(r) };
        }
        Ok(e)
    }
    fn unary(&mut self) -> Result<Expr, CompileError> {
        let op = match self.peek() {
            TokenKind::Not => Some(UnOp::Not),
            TokenKind::Minus => Some(UnOp::Neg),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let e = self.unary()?;
            return Ok(Expr::Unary { op, expr: Box::new(e) });
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
            _ => return Err(self.err("expected expression")),
        };
        Ok(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::lexer::lex;

    fn expr(src: &str) -> Expr {
        // parse "src;" as a single expression statement
        let stmts = parse(lex(&format!("{src};")).unwrap()).unwrap();
        match stmts.into_iter().next().unwrap() {
            Stmt::ExprStmt(e) => e,
            other => panic!("expected ExprStmt, got {other:?}"),
        }
    }

    #[test]
    fn precedence_mul_over_plus() {
        assert_eq!(
            expr("1 plus 2 mul 3"),
            Expr::Binary {
                op: BinOp::Add,
                lhs: Box::new(Expr::Int(1)),
                rhs: Box::new(Expr::Binary {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Int(2)),
                    rhs: Box::new(Expr::Int(3)),
                }),
            }
        );
    }

    #[test]
    fn unary_and_grouping() {
        assert_eq!(
            expr("minus (1 plus 2)"),
            Expr::Unary {
                op: UnOp::Neg,
                expr: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Int(1)),
                    rhs: Box::new(Expr::Int(2)),
                }),
            }
        );
    }

    #[test]
    fn call_with_args() {
        let e = expr("add(1, 2)");
        match e {
            Expr::Call { callee, args, .. } => {
                assert!(matches!(*callee, Expr::Var(ref n, _, _) if n == "add"));
                assert_eq!(args, vec![Expr::Int(1), Expr::Int(2)]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn logic_precedence_or_lowest() {
        // a or b and c  =>  a or (b and c)
        let e = expr("true or false and true");
        assert!(matches!(e, Expr::Binary { op: BinOp::Or, .. }));
    }

    #[test]
    fn parses_assign_and_reassign() {
        let s = parse(lex("assign x 10; x be x plus 1;").unwrap()).unwrap();
        assert!(matches!(&s[0], Stmt::Assign { name, .. } if name == "x"));
        assert!(matches!(&s[1], Stmt::Reassign { name, .. } if name == "x"));
    }

    #[test]
    fn parses_if_else_chain() {
        let s = parse(lex("if true begin print(1); end else if false begin print(2); end else begin print(3); end").unwrap()).unwrap();
        match &s[0] {
            Stmt::If { else_body: Some(eb), .. } => {
                assert!(matches!(&eb[0], Stmt::If { else_body: Some(_), .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn desugars_for_to_while() {
        let s = parse(lex("for assign i 0; i lo 10; i be i plus 1 begin print(i); end").unwrap()).unwrap();
        match &s[0] {
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
        let s = parse(lex("fn add(a, b) begin return a plus b; end").unwrap()).unwrap();
        match &s[0] {
            Stmt::Fn { name, params, body, .. } => {
                assert_eq!(name, "add");
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
}
