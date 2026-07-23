use crate::error::CompileError;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64), Float(f64), Str(String), Ident(String),
    Assign, Be, Declare, Make, Return, Check, Orelse, Repeat, Loop, True, False, Nil, Begin, End,
    Import, Mod, Std, List, Shape,
    Add, Sub, Neg, Times, Div,
    Equals, Differs, Trails, Beats, Atmost, Atleast,
    And, Or, Not, Join,
    LParen, RParen, Semi, Comma,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
    pub col: u32,
}

impl TokenKind {
    /// Human-readable form for error messages.
    pub fn describe(&self) -> String {
        use TokenKind::*;
        match self {
            Int(_) => "integer literal".into(),
            Float(_) => "number literal".into(),
            Str(_) => "string literal".into(),
            Ident(n) => format!("identifier '{n}'"),
            Eof => "end of file".into(),
            LParen => "'('".into(), RParen => "')'".into(),
            Semi => "';'".into(), Comma => "','".into(),
            kw => format!("'{}'", format!("{kw:?}").to_lowercase()),
        }
    }
}

/// Pre-verb-sweep keyword -> current keyword, for migration hints.
pub fn renamed_keyword(word: &str) -> Option<&'static str> {
    Some(match word {
        "fn" => "make", "if" => "check", "else" => "orelse",
        "while" => "repeat", "for" => "loop",
        "plus" => "add", "minus" => "sub (or prefix 'neg')", "mul" => "times",
        "c" => "join",
        "eqeq" => "equals", "neq" => "differs",
        "lo" => "trails", "hi" => "beats", "loeq" => "atmost", "hieq" => "atleast",
        _ => return None,
    })
}

/// A comment the lexer would otherwise discard, kept for tools (the
/// formatter) that need to reproduce it. `text` is the full comment
/// including its delimiters (`%% ...` or `!?! ... !?!`).
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub text: String,
    pub line: u32,
    pub col: u32,
}

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    lex_with_comments(src).map(|(toks, _)| toks)
}

/// Same scan as `lex`, but also returns every comment encountered
/// (normally discarded) with its source position, for the formatter.
pub fn lex_with_comments(src: &str) -> Result<(Vec<Token>, Vec<Comment>), CompileError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut comments = Vec::new();
    let mut i = 0usize;
    let (mut line, mut col) = (1u32, 1u32);

    while i < chars.len() {
        let c = chars[i];
        let (tl, tc) = (line, col);
        match c {
            ' ' | '\t' | '\r' => { i += 1; col += 1; }
            '\n' => { i += 1; line += 1; col = 1; }
            '%' if chars.get(i + 1) == Some(&'%') => {
                let start = i;
                while i < chars.len() && chars[i] != '\n' { i += 1; col += 1; }
                let text: String = chars[start..i].iter().collect();
                comments.push(Comment { text, line: tl, col: tc });
            }
            '!' if chars.get(i + 1) == Some(&'?') && chars.get(i + 2) == Some(&'!') => {
                let start = i;
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
                let text: String = chars[start..i].iter().collect();
                comments.push(Comment { text, line: tl, col: tc });
            }
            '(' => { toks.push(Token { kind: TokenKind::LParen, line: tl, col: tc }); i += 1; col += 1; }
            ')' => { toks.push(Token { kind: TokenKind::RParen, line: tl, col: tc }); i += 1; col += 1; }
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
                // A `.verb` suffix directly after an identifier, at a word
                // boundary (not followed by another identifier/digit char),
                // folds into the same token so `import mod utils.verb;`
                // lexes as one name. `.` is otherwise never valid outside a
                // numeric literal, so this can't misfire on any program
                // that lexed successfully before — see
                // docs/superpowers/specs/2026-07-21-verb-file-import-design.md.
                const VERB_SUFFIX: &[char] = &['.', 'v', 'e', 'r', 'b'];
                if chars[i..].starts_with(VERB_SUFFIX) {
                    let after = i + VERB_SUFFIX.len();
                    let boundary = chars.get(after)
                        .map_or(true, |c| !(c.is_ascii_alphanumeric() || *c == '_'));
                    if boundary {
                        i += VERB_SUFFIX.len();
                        col += VERB_SUFFIX.len() as u32;
                    }
                }
                let word: String = chars[start..i].iter().collect();
                use TokenKind::*;
                let kind = match word.as_str() {
                    "assign" => Assign, "be" => Be, "declare" => Declare, "make" => Make, "return" => Return,
                    "check" => Check, "orelse" => Orelse, "repeat" => Repeat, "loop" => Loop,
                    "true" => True, "false" => False, "nil" => Nil,
                    "begin" => Begin, "end" => End,
                    "import" => Import, "mod" => Mod, "std" => Std, "list" => List,
                    "shape" => Shape,
                    "add" => Add, "sub" => Sub, "neg" => Neg,
                    "times" => Times, "div" => Div,
                    "equals" => Equals, "differs" => Differs, "trails" => Trails,
                    "beats" => Beats, "atmost" => Atmost, "atleast" => Atleast,
                    "and" => And, "or" => Or, "not" => Not, "join" => Join,
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
    Ok((toks, comments))
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
            kinds("assign x 10; x be x add 1;"),
            vec![Assign, Ident("x".into()), Int(10), Semi,
                 Ident("x".into()), Be, Ident("x".into()), Add, Int(1), Semi, Eof]
        );
    }

    #[test]
    fn scans_list_keyword() {
        use TokenKind::*;
        assert_eq!(kinds("list 1, 2, 3"), vec![List, Int(1), Comma, Int(2), Comma, Int(3), Eof]);
    }

    #[test]
    fn scans_verb_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("declare make check orelse repeat loop sub neg times join equals differs trails beats atmost atleast"),
            vec![Declare, Make, Check, Orelse, Repeat, Loop, Sub, Neg, Times, Join,
                 Equals, Differs, Trails, Beats, Atmost, Atleast, Eof]
        );
    }

    #[test]
    fn scans_shape_keyword() {
        use TokenKind::*;
        assert_eq!(
            kinds("shape Point begin x, y end"),
            vec![Shape, Ident("Point".into()), Begin, Ident("x".into()), Comma,
                 Ident("y".into()), End, Eof]
        );
    }

    #[test]
    fn old_keywords_are_plain_identifiers() {
        use TokenKind::*;
        assert_eq!(
            kinds("plus if while fn c"),
            vec![Ident("plus".into()), Ident("if".into()), Ident("while".into()),
                 Ident("fn".into()), Ident("c".into()), Eof]
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
        let t = &lex("\n  make").unwrap()[0];
        assert_eq!((t.line, t.col), (2, 3));
    }

    #[test]
    fn rejects_unknown_char() {
        assert!(lex("@").is_err());
    }

    #[test]
    fn scans_begin_end_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("repeat x trails 1 begin end"),
            vec![Repeat, Ident("x".into()), Trails, Int(1), Begin, End, Eof]
        );
    }

    #[test]
    fn scans_import_keywords() {
        use TokenKind::*;
        assert_eq!(
            kinds("import mod mathlib;"),
            vec![Import, Mod, Ident("mathlib".into()), Semi, Eof]
        );
    }

    #[test]
    fn scans_std_import_keyword() {
        use TokenKind::*;
        assert_eq!(
            kinds("import std io;"),
            vec![Import, Std, Ident("io".into()), Semi, Eof]
        );
    }

    #[test]
    fn folds_dot_verb_suffix_into_identifier() {
        use TokenKind::*;
        assert_eq!(
            kinds("import mod utils.verb;"),
            vec![Import, Mod, Ident("utils.verb".into()), Semi, Eof]
        );
    }

    #[test]
    fn does_not_fold_dot_verb_when_followed_by_identifier_chars() {
        // `.verbose` is not the exact `.verb` suffix at a word boundary,
        // so no folding happens and the bare `.` after `utils` is still
        // a lex error, same as it was before this feature existed.
        assert!(lex("utils.verbose").is_err());
    }

    #[test]
    fn bare_dot_after_identifier_is_still_a_lex_error() {
        assert!(lex("mathlib.").is_err());
    }

    #[test]
    fn rejects_braces() {
        assert!(lex("{").is_err());
        assert!(lex("}").is_err());
    }
}
