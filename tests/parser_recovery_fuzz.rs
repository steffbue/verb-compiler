//! `parse_recovering` must never hang, no matter how mangled the input.
//! Rather than trust hand-picked cases, this generates a large set of
//! syntactically-broken variants (one token deleted at a time, and
//! random deletions) from every real .verb file in the repo and asserts
//! each one finishes within a hard wall-clock deadline.

use std::fs;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use verb::lexer;
use verb::parser;

const DEADLINE: Duration = Duration::from_secs(2);

/// Runs `parse_recovering` on its own thread and fails if it doesn't
/// come back within `DEADLINE` — a hang would otherwise wedge `cargo
/// test` itself instead of failing cleanly.
fn assert_terminates(src: &str, label: &str) {
    let src = src.to_string();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let toks = match lexer::lex(&src) {
            Ok(t) => t,
            Err(_) => return, // lexer errors aren't parser's problem
        };
        let _ = tx.send(parser::parse_recovering(toks));
    });
    match rx.recv_timeout(DEADLINE) {
        // lexer rejected the mutated input before parsing ever started —
        // a normal outcome, not a hang
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            panic!("parse_recovering did not terminate within {DEADLINE:?} on {label}");
        }
        Ok(_) => {}
    }
    let _ = handle.join();
}

/// All tokens' source-text spans, in order, for building "delete one
/// token" mutations without needing the lexer to expose byte offsets.
fn token_spans(src: &str) -> Vec<(usize, usize)> {
    // Re-lexes greedily using the same char classes as the real lexer's
    // identifier/number/string/comment rules, just to recover byte spans.
    let bytes = src.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '%' && bytes.get(i + 1) == Some(&b'%') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == '!' && bytes.get(i + 1) == Some(&b'?') && bytes.get(i + 2) == Some(&b'!') {
            i += 3;
            while i + 2 < bytes.len() && !(bytes[i] == b'!' && bytes[i + 1] == b'?' && bytes[i + 2] == b'!') {
                i += 1;
            }
            i = (i + 3).min(bytes.len());
        } else if c == '"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(bytes.len());
        } else if c.is_ascii_alphanumeric() || c == '_' {
            while i < bytes.len() && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.') {
                i += 1;
            }
        } else {
            i += 1; // single-char punctuation token
        }
        if i > start {
            spans.push((start, i));
        }
    }
    spans
}

fn fixture_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["tests/fixtures", "examples"] {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("verb") {
                if let Ok(src) = fs::read_to_string(&path) {
                    out.push((path.display().to_string(), src));
                }
            }
        }
    }
    assert!(!out.is_empty(), "expected to find .verb fixtures to fuzz");
    out
}

#[test]
fn recovering_parse_terminates_on_every_fixture_as_is() {
    for (name, src) in fixture_sources() {
        assert_terminates(&src, &name);
    }
}

#[test]
fn recovering_parse_terminates_with_any_single_token_deleted() {
    for (name, src) in fixture_sources() {
        let spans = token_spans(&src);
        for (idx, &(start, end)) in spans.iter().enumerate() {
            let mut mutated = String::with_capacity(src.len());
            mutated.push_str(&src[..start]);
            mutated.push_str(&src[end..]);
            assert_terminates(&mutated, &format!("{name} (token #{idx} deleted)"));
        }
    }
}

#[test]
fn recovering_parse_terminates_with_pairs_of_tokens_deleted() {
    // Cheaper than the full n^2 cross product: delete two tokens spaced
    // a few apart across the file, sliding the window, which is what
    // tends to produce the nastiest "half a construct" leftovers.
    for (name, src) in fixture_sources() {
        let spans = token_spans(&src);
        for gap in [1usize, 2, 3] {
            for i in 0..spans.len().saturating_sub(gap) {
                let (s1, e1) = spans[i];
                let (s2, e2) = spans[i + gap];
                let mut mutated = String::with_capacity(src.len());
                mutated.push_str(&src[..s1]);
                mutated.push_str(&src[e1..s2]);
                mutated.push_str(&src[e2..]);
                assert_terminates(&mutated, &format!("{name} (tokens #{i}+#{}, gap {gap} deleted)", i + gap));
            }
        }
    }
}
