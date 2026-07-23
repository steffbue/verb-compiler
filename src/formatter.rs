//! Formatter for Verb source, driven by the token+comment stream rather
//! than the AST — `ast::Stmt` carries no comment info (the lexer used to
//! discard comments entirely), so unparsing the AST would drop every
//! comment. Working from tokens avoids that by construction, and also
//! keeps the formatter oblivious to AST shape (e.g. `loop … begin … end`
//! parsing to a `Stmt::For`): it just echoes the tokens it sees.
//!
//! Turns out spacing/newline placement don't need a full grammar walk
//! either: every rule below is a pure function of the current token kind
//! and the *previous* token kind, plus one flag (`in_loop_header`) for
//! the one construct (`loop init; cond; update begin`) where a `;` must
//! NOT start a new line. So this is a single left-to-right pass over the
//! token stream — a token-pair automaton — rather than a second
//! recursive-descent parser shaped like `parser.rs`.

use crate::error::CompileError;
use crate::lexer::{self, Comment, Token, TokenKind};
use crate::parser;

pub fn format(src: &str) -> Result<String, CompileError> {
    // Only ever format source the real compiler pipeline accepts.
    parser::parse(lexer::lex(src)?)?;

    let (toks, comments) = lexer::lex_with_comments(src)?;
    let mut p = Printer {
        toks,
        pos: 0,
        comments,
        cpos: 0,
        out: String::new(),
        indent: 0,
        in_loop_header: false,
        prev_kind: None,
        last_line: 1,
        force_newline: false,
    };
    p.run();

    let mut out = p.out;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

struct Printer {
    toks: Vec<Token>,
    pos: usize,
    comments: Vec<Comment>,
    cpos: usize,
    out: String,
    indent: usize,
    in_loop_header: bool,
    prev_kind: Option<TokenKind>,
    /// Source line the most recently emitted content (token or comment)
    /// ends on — used both to classify the next comment as trailing vs.
    /// standalone and to size the blank-line gap before it.
    last_line: u32,
    /// Set after emitting any comment: line/block comments can never
    /// share their line with what follows, so the next item is always
    /// forced onto a fresh line regardless of what the token-pair rules
    /// would otherwise decide.
    force_newline: bool,
}

impl Printer {
    fn run(&mut self) {
        loop {
            let tok = self.toks[self.pos].clone();
            self.flush_comments_before(&tok);
            if tok.kind == TokenKind::Eof {
                break;
            }
            self.place_token(&tok);
            self.pos += 1;
        }
    }

    fn flush_comments_before(&mut self, next: &Token) {
        loop {
            let ready = match self.comments.get(self.cpos) {
                Some(c) => (c.line, c.col) < (next.line, next.col),
                None => false,
            };
            if !ready {
                return;
            }
            let c = self.comments[self.cpos].clone();
            self.cpos += 1;

            let trailing = c.line == self.last_line && !self.out.is_empty();
            if trailing {
                self.out.push(' ');
                self.out.push_str(&c.text);
            } else {
                self.start_new_output_line(c.line);
                self.out.push_str(&c.text);
            }
            self.last_line = comment_end_line(&c);
            self.force_newline = true;
        }
    }

    fn start_new_output_line(&mut self, src_line: u32) {
        if !self.out.is_empty() {
            let gap = src_line.saturating_sub(self.last_line);
            self.out.push('\n');
            if gap >= 2 {
                self.out.push('\n');
            }
        }
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
    }

    fn place_token(&mut self, tok: &Token) {
        use TokenKind::*;

        // Dedent applies to the `end` token's own line, so it has to
        // happen before we decide/print this token's indentation.
        if tok.kind == End {
            self.indent = self.indent.saturating_sub(1);
        }

        let newline = self.force_newline || self.starts_new_line(tok);
        self.force_newline = false;

        if newline {
            self.start_new_output_line(tok.line);
        } else if self.needs_space_before(&tok.kind) {
            self.out.push(' ');
        }

        self.out.push_str(&token_text(tok));
        self.last_line = tok.line;

        if tok.kind == Loop {
            self.in_loop_header = true;
        }
        if tok.kind == Begin {
            self.in_loop_header = false;
            self.indent += 1;
        }

        self.prev_kind = Some(tok.kind.clone());
    }

    fn starts_new_line(&self, tok: &Token) -> bool {
        use TokenKind::*;
        match &self.prev_kind {
            None => false,
            Some(Semi) => !self.in_loop_header,
            Some(Begin) => true,
            Some(End) => !matches!(tok.kind, Orelse),
            _ => false,
        }
    }

    fn needs_space_before(&self, cur: &TokenKind) -> bool {
        use TokenKind::*;
        let Some(prev) = &self.prev_kind else { return false };
        if matches!(prev, LParen) {
            return false;
        }
        match cur {
            Semi | Comma | RParen => false,
            LParen => !matches!(prev, Ident(_) | RParen),
            _ => true,
        }
    }
}

fn comment_end_line(c: &Comment) -> u32 {
    c.line + c.text.matches('\n').count() as u32
}

fn token_text(tok: &Token) -> String {
    use TokenKind::*;
    match &tok.kind {
        Int(v) => v.to_string(),
        Float(v) => v.to_string(),
        Str(s) => format!("\"{}\"", escape_str(s)),
        Ident(n) => n.clone(),
        Assign => "assign".into(),
        Be => "be".into(),
        Declare => "declare".into(),
        Make => "make".into(),
        Return => "return".into(),
        Check => "check".into(),
        Orelse => "orelse".into(),
        Repeat => "repeat".into(),
        Loop => "loop".into(),
        Leave => "leave".into(),
        Next => "next".into(),
        True => "true".into(),
        False => "false".into(),
        Nil => "nil".into(),
        Begin => "begin".into(),
        End => "end".into(),
        Import => "import".into(),
        Mod => "mod".into(),
        Std => "std".into(),
        List => "list".into(),
        Add => "add".into(),
        Sub => "sub".into(),
        Neg => "neg".into(),
        Times => "times".into(),
        Div => "div".into(),
        Equals => "equals".into(),
        Differs => "differs".into(),
        Trails => "trails".into(),
        Beats => "beats".into(),
        Atmost => "atmost".into(),
        Atleast => "atleast".into(),
        And => "and".into(),
        Or => "or".into(),
        Not => "not".into(),
        Join => "join".into(),
        LParen => "(".into(),
        RParen => ")".into(),
        Semi => ";".into(),
        Comma => ",".into(),
        Eof => String::new(),
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format(src).unwrap_or_else(|e| panic!("format failed: {e:?}"))
    }

    #[test]
    fn formats_assign_and_declare() {
        assert_eq!(fmt("assign   x    10 ;"), "assign x 10;\n");
        assert_eq!(fmt("declare y;"), "declare y;\n");
    }

    #[test]
    fn formats_import_statements() {
        assert_eq!(
            fmt("import   mod    mathlib ;   import std   io ;\nprint(1);"),
            "import mod mathlib;\nimport std io;\nprint(1);\n"
        );
    }

    #[test]
    fn formats_reassign_and_operators() {
        assert_eq!(fmt("x be x add 5;"), "x be x add 5;\n");
        assert_eq!(fmt("print(x mod 4);"), "print(x mod 4);\n");
        assert_eq!(fmt("print(neg x);"), "print(neg x);\n");
        assert_eq!(fmt("print(not ready);"), "print(not ready);\n");
    }

    #[test]
    fn formats_string_concat_call() {
        assert_eq!(
            fmt(r#"print("language" join ": " join lang);"#),
            "print(\"language\" join \": \" join lang);\n"
        );
    }

    #[test]
    fn formats_nested_call_args() {
        assert_eq!(fmt("print(sum(1, 2));"), "print(sum(1, 2));\n");
        assert_eq!(fmt("print(pick(3,11));"), "print(pick(3, 11));\n");
    }

    #[test]
    fn formats_check_orelse_chain() {
        let src = "check x beats 10 begin print(\"big\"); end orelse check x beats 3 begin print(\"medium\"); end orelse begin print(\"small\"); end";
        let expected = "check x beats 10 begin\n  print(\"big\");\nend orelse check x beats 3 begin\n  print(\"medium\");\nend orelse begin\n  print(\"small\");\nend\n";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn formats_nested_blocks_with_indent() {
        let src = "make f(n) begin check n beats 0 begin return n; end return 0; end";
        let expected = "make f(n) begin\n  check n beats 0 begin\n    return n;\n  end\n  return 0;\nend\n";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn preserves_loop_header_on_one_line() {
        let src = "loop assign i 1; i atmost 15; i be i add 1 begin print(i); end";
        let expected = "loop assign i 1; i atmost 15; i be i add 1 begin\n  print(i);\nend\n";
        assert_eq!(fmt(src), expected);
    }

    #[test]
    fn preserves_standalone_line_comment() {
        let src = "%% header\nassign x 1;";
        assert_eq!(fmt(src), "%% header\nassign x 1;\n");
    }

    #[test]
    fn preserves_trailing_line_comment() {
        let src = "assign x 1;   %% one";
        assert_eq!(fmt(src), "assign x 1; %% one\n");
    }

    #[test]
    fn preserves_standalone_block_comment_verbatim() {
        let src = "!?!\n  multi\n  line\n!?!\nassign x 1;";
        assert_eq!(fmt(src), "!?!\n  multi\n  line\n!?!\nassign x 1;\n");
    }

    #[test]
    fn preserves_trailing_block_comment() {
        let src = "assign x 1; !?! note !?!";
        assert_eq!(fmt(src), "assign x 1; !?! note !?!\n");
    }

    #[test]
    fn collapses_multiple_blank_lines_to_one() {
        let src = "assign a 1;\n\n\n\nassign b 2;";
        assert_eq!(fmt(src), "assign a 1;\n\nassign b 2;\n");
    }

    #[test]
    fn keeps_single_blank_line() {
        let src = "assign a 1;\n\nassign b 2;";
        assert_eq!(fmt(src), "assign a 1;\n\nassign b 2;\n");
    }

    #[test]
    fn no_blank_line_added_when_none_present() {
        let src = "assign a 1;\nassign b 2;";
        assert_eq!(fmt(src), "assign a 1;\nassign b 2;\n");
    }

    #[test]
    fn is_idempotent_across_a_range_of_constructs() {
        let src = "%% tour\nassign x 10;\nx be x add 5; %% now 15\n\ncheck x beats 10 begin\n  print(\"big\");\nend orelse begin\n  print(\"small\");\nend\n\nmake sq(n) begin\n  return n times n;\nend\nloop assign i 1; i atmost 3; i be i add 1 begin\n  print(i);\nend\n";
        let once = fmt(src);
        let twice = fmt(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn rejects_invalid_syntax() {
        assert!(format("assign x;").is_err());
    }

    #[test]
    fn formats_list_literal() {
        assert_eq!(fmt("assign a list 1,   2,3;"), "assign a list 1, 2, 3;\n");
    }
}
