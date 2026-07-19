use crate::error::CompileError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64), Float(f64), Str(String), Ident(String),
    Assign, Be, Fn, Return, If, Else, While, For, True, False, Nil,
    Plus, Minus, Mul, Div, Mod,
    Eqeq, Neq, Lo, Hi, Loeq, Hieq,
    And, Or, Not, Concat,
    LParen, RParen, LBrace, RBrace, Semi, Comma,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let (mut line, mut col) = (1u32, 1u32);

    while i < chars.len() {
        let c = chars[i];
        let (tl, tc) = (line, col);
        match c {
            ' ' | '\t' | '\r' => { i += 1; col += 1; }
            '\n' => { i += 1; line += 1; col = 1; }
            '%' if chars.get(i + 1) == Some(&'%') => {
                while i < chars.len() && chars[i] != '\n' { i += 1; col += 1; }
            }
            '!' if chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'!') => {
                i += 3; col += 3;
                loop {
                    if i + 2 >= chars.len() + 1 && i >= chars.len() {
                        return Err(CompileError::new("unterminated block comment", tl, tc));
                    }
                    if chars[i] == '!' && chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'!') {
                        i += 3; col += 3; break;
                    }
                    if chars[i] == '\n' { line += 1; col = 1; } else { col += 1; }
                    i += 1;
                    if i >= chars.len() {
                        return Err(CompileError::new("unterminated block comment", tl, tc));
                    }
                }
            }
            '(' => { toks.push(Token { kind: TokenKind::LParen, line: tl, col: tc }); i += 1; col += 1; }
            ')' => { toks.push(Token { kind: TokenKind::RParen, line: tl, col: tc }); i += 1; col += 1; }
            '{' => { toks.push(Token { kind: TokenKind::LBrace, line: tl, col: tc }); i += 1; col += 1; }
            '}' => { toks.push(Token { kind: TokenKind::RBrace, line: tl, col: tc }); i += 1; col += 1; }
            ';' => { toks.push(Token { kind: TokenKind::Semi, line: tl, col: tc }); i += 1; col += 1; }
            ',' => { toks.push(Token { kind: TokenKind::Comma, line: tl, col: tc }); i += 1; col += 1; }
            '"' => {
                i += 1; col += 1;
                let mut s = String::new();
                loop {
                    if i >= chars.len() {
                        return Err(CompileError::new("unterminated string", tl, tc));
                    }
                    match chars[i] {
                        '"' => { i += 1; col += 1; break; }
                        '\\' => {
                            let esc = chars.get(i + 1).copied()
                                .ok_or_else(|| CompileError::new("unterminated string", tl, tc))?;
                            s.push(match esc {
                                'n' => '\n', 't' => '\t', '"' => '"', '\\' => '\\',
                                other => return Err(CompileError::new(
                                    format!("unknown escape '\\{other}'"), line, col)),
                            });
                            i += 2; col += 2;
                        }
                        '\n' => return Err(CompileError::new("unterminated string", tl, tc)),
                        ch => { s.push(ch); i += 1; col += 1; }
                    }
                }
                toks.push(Token { kind: TokenKind::Str(s), line: tl, col: tc });
            }
            d if d.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() { i += 1; col += 1; }
                let mut is_float = false;
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    is_float = true;
                    i += 1; col += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() { i += 1; col += 1; }
                }
                let text: String = chars[start..i].iter().collect();
                let kind = if is_float {
                    TokenKind::Float(text.parse().map_err(|_| CompileError::new("bad float", tl, tc))?)
                } else {
                    TokenKind::Int(text.parse().map_err(|_| CompileError::new("int too large", tl, tc))?)
                };
                toks.push(Token { kind, line: tl, col: tc });
            }
            a if a.is_ascii_alphabetic() || a == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1; col += 1;
                }
                let word: String = chars[start..i].iter().collect();
                use TokenKind::*;
                let kind = match word.as_str() {
                    "assign" => Assign, "be" => Be, "fn" => Fn, "return" => Return,
                    "if" => If, "else" => Else, "while" => While, "for" => For,
                    "true" => True, "false" => False, "nil" => Nil,
                    "plus" => Plus, "minus" => Minus, "mul" => Mul, "div" => Div, "mod" => Mod,
                    "eqeq" => Eqeq, "neq" => Neq, "lo" => Lo, "hi" => Hi,
                    "loeq" => Loeq, "hieq" => Hieq,
                    "and" => And, "or" => Or, "not" => Not, "c" => Concat,
                    _ => Ident(word),
                };
                toks.push(Token { kind, line: tl, col: tc });
            }
            other => {
                return Err(CompileError::new(format!("unexpected character '{other}'"), tl, tc));
            }
        }
    }
    toks.push(Token { kind: TokenKind::Eof, line, col });
    Ok(toks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn scans_keywords_and_operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("assign x 10; x be x plus 1;"),
            vec![Assign, Ident("x".into()), Int(10), Semi,
                 Ident("x".into()), Be, Ident("x".into()), Plus, Int(1), Semi, Eof]
        );
    }

    #[test]
    fn scans_literals() {
        use TokenKind::*;
        assert_eq!(
            kinds(r#"3.24 "hi\n" true false nil"#),
            vec![Float(3.24), Str("hi\n".into()), True, False, Nil, Eof]
        );
    }

    #[test]
    fn skips_comments() {
        use TokenKind::*;
        assert_eq!(kinds("%% line\n1 !?! block\nstill !?! 2"), vec![Int(1), Int(2), Eof]);
    }

    #[test]
    fn tracks_position() {
        let t = &lex("\n  fn").unwrap()[0];
        assert_eq!((t.line, t.col), (2, 3));
    }

    #[test]
    fn rejects_unknown_char() {
        assert!(lex("@").is_err());
    }
}
