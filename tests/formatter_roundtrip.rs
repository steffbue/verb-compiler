//! Round-trips every valid `.verb` fixture (and `examples/demo.verb`)
//! through `verb::formatter::format`: the result must still lex+parse
//! and formatting it again must produce identical output.

use verb::{formatter, lexer, parser};

const VALID_FIXTURES: &[&str] = &[
    "arith", "control", "declare", "functions", "literals", "strings", "vars",
];

fn assert_roundtrips(path: &str) {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));

    let once = formatter::format(&src).unwrap_or_else(|e| panic!("format {path}: {e:?}"));

    let toks = lexer::lex(&once).unwrap_or_else(|e| panic!("re-lex formatted {path}: {e:?}"));
    parser::parse(toks).unwrap_or_else(|e| panic!("re-parse formatted {path}: {e:?}"));

    let twice = formatter::format(&once).unwrap_or_else(|e| panic!("re-format {path}: {e:?}"));
    assert_eq!(once, twice, "formatting {path} is not idempotent");
}

#[test]
fn fixtures_roundtrip() {
    for name in VALID_FIXTURES {
        assert_roundtrips(&format!("tests/fixtures/{name}.verb"));
    }
}

#[test]
fn demo_roundtrips() {
    assert_roundtrips("examples/demo.verb");
}
