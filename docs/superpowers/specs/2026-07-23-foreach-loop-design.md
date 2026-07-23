# For-each loop design

Status: approved (2026-07-23)

## Goal

Add a `for-each` loop that binds a name to successive elements of a
collection. Complements the existing constructs:

- `repeat <cond> begin … end` — while loop.
- `loop <init>; <cond>; <incr> begin … end` — C-style counting for.

The for-each removes the manual index bookkeeping those require when you
just want to visit every element.

## Syntax

```
each <name> in <collection> begin
  … body uses <name> …
end
```

- New keywords: `each`, `in`, `to`.
- Element-only binding: exactly one name. No index/key second binding in
  v1.
- Body is a `begin … end` block, same as `repeat`/`loop`/`check`.
- `<name>` is scoped to the loop body, like existing loop variables. It
  is fresh each iteration (its own body scope, released at end of the
  iteration).

### Range head

```
each x in <a> to <b>
```

- `to` is only valid in a for-each head, immediately after the first
  expression. It is **not** a general-purpose expression operator.
- Half-open interval **[a, b)**: `each x in 0 to 3` binds `0, 1, 2`.
- Ascending only. If `a >= b`, the loop body runs zero times.
- `a` and `b` are evaluated once, before the loop.

## Iteration by runtime type

Verb is dynamically typed, so `each x in <collection>` dispatches on the
collection value's runtime tag:

| Runtime type   | `<name>` binds to                         |
|----------------|-------------------------------------------|
| array (`list`) | each element, index `0 .. len-1`          |
| string         | each byte, as a 1-byte string (UTF-8 multibyte chars split) |
| map            | each **key** (use `map_get(m, name)` for the value) |
| (range head)   | each int in `[a, b)` — see above          |

- Iterating any other value (int, float, bool, nil, closure) is a
  **runtime error**, consistent with Verb's other type errors
  (`abort_at "cannot iterate <type>"`).
- Empty collection (len 0) → body runs zero times.
- Map key order is unspecified (backed by `std::unordered_map`).

### Length snapshot / mutation

The collection length is **snapshot once** at loop entry, not re-checked
each iteration. Mutating the collection mid-iteration is unspecified:

- Growing an array: new tail elements are not visited.
- Shrinking an array: a fetch past the new end is an out-of-bounds
  runtime error (same as `get`).
- Mutating a map mid-iteration: unspecified (may skip/repeat/error).

Documented as "don't mutate the collection you're iterating."

## Implementation

Touch-points confirmed against the current tree.

### Lexer — `src/lexer.rs`

- Add `TokenKind` variants `Each`, `In`, `To` (near `Repeat, Loop`).
- Add word arms in the identifier matcher: `"each" => Each`,
  `"in" => In`, `"to" => To`.
- `describe()` auto-formats new keyword tokens for error messages; add
  explicit arms if the default is unclear.
- Optional `renamed_keyword` hints, e.g. `"foreach" => "each"`.

### Parser — `src/parser.rs`

- `statement()`: add `TokenKind::Each => self.foreach_stmt()`.
- `foreach_stmt()`:
  1. `advance()` past `each`.
  2. Parse `<name>` (expect an identifier token).
  3. `expect(In)`.
  4. Parse the first expression.
  5. If the next token is `To`: parse the second expression, then
     **desugar to a counter loop** and return
     `Stmt::Block([init, While{cond, body+incr}])` — same shape as the
     existing `for_stmt` (parser.rs:284-300). Interval is half-open, so
     `cond` is `x trails b` (`<`) and `incr` is `x be x add 1`.
  6. Otherwise the first expression is the collection: parse the
     `begin … end` block and return
     `Stmt::ForEach { name, coll, body }`.

The range path needs no new AST node or codegen — it reuses `While`.

### AST — `src/ast.rs`

Add one variant:

```rust
ForEach { name: String, coll: Expr, body: Vec<Stmt> },
```

### Codegen — `src/codegen.rs`

New arm in the `match stmt` dispatcher (line ~1579), modeled on the
`Stmt::While` block structure (1654-1679): `append_basic_block`,
`position_at_end`, conditional/unconditional branches, per-iteration
scope push + `release_scope`, `cur_block_open()` guards.

1. `gen_expr(coll)` once → store in an alloca (`cptr`). Read `tag` via
   `tag_of`.
2. Dispatch block: `switch(tag)` to compute the iteration length `len`
   and a normalized `kind` flag:
   - `TAG_ARRAY` → `verb_array_len(coll)` (helper at codegen.rs:847).
   - `TAG_STR`   → `strlen(payload_of(coll))`.
   - `TAG_MAP`   → `map_len(coll)`.
   - default     → `abort_at(line, col, "cannot iterate %s",
     [type_name(tag)])`.
3. Counter loop over `i in [0, len)` (len snapshot in an alloca). In the
   body, `switch(kind)` to fetch the current element:
   - array  → `verb_array_get(coll, make_val(TAG_INT, i))`
     (helper at codegen.rs:942).
   - string → `verb_char_at(coll, make_val(TAG_INT, i))` (new, below).
   - map    → `map_key_at(coll, make_val(TAG_INT, i))` (new, below).
4. Bind the fetched element to `name` in a fresh body scope, `gen_stmts`
   the body, then retain/release per existing conventions and `i += 1`.

Reuse `verb_retain_value`/`verb_release_value` exactly as `Stmt::While`
and the array helpers do, so refcounts stay balanced across early exits
(`return` inside the body) via the `cur_block_open()` checks.

### Runtime — new primitives

**String char-at** — new translation unit `runtime/verb_str.cpp`:

```cpp
extern "C" VerbValue verb_char_at(VerbValue s, VerbValue i);
```

Returns a freshly GC-allocated 1-char NUL-terminated string for byte
`i` of `s`. Allocates via the same `verb_alloc` path other heap values
use so the GC header is present. Index is a `TAG_INT`; out-of-range or
non-string input is a runtime error (mirror `verb_array_check`).

Wiring:
- Add `runtime/verb_str.cpp` to the `cc::Build` in `build.rs`.
- Register `verb_char_at` for the JIT in `main.rs`
  (`register_jit_runtime_symbols`).
- Declare the extern in codegen (near the other runtime `add_function`
  declarations) so `call_named`/the std-call path can reach it.

**Map key-at** — extend `runtime/verb_map.cpp`:

```cpp
extern "C" VerbValue map_key_at(VerbValue m, VerbValue i);
```

Returns the `i`-th key via `std::next(impl->begin(), i)` (retained before
return, like `map_get`). O(n) per call → O(n²) over a full iteration;
acceptable for v1's educational scope, noted as a known limitation.
Register by adding `("map_key_at", 2)` to `MAP_FUNCS`
(codegen.rs:2133) — no other call-path change needed since
`verb_map.cpp` is already linked.

## Testing

- **Parser** (`src/parser.rs` tests): `each x in coll begin … end` parses
  to `Stmt::ForEach`; `each x in a to b begin … end` desugars to
  `Block([Assign, While])`; error cases (missing `in`, missing name,
  missing block).
- **Lexer**: new keyword tokens lex; `foreach` gives a rename hint.
- **End-to-end** `.verb` programs (run via `verb build`, since map/io
  need AOT — JIT does not support std imports):
  - array: sum/print every element.
  - string: print each char, count length.
  - map: iterate keys, look up each value with `map_get`.
  - range: `each x in 0 to 5` prints `0 1 2 3 4`; empty range
    `each x in 3 to 3` prints nothing.
  - non-iterable (`each x in 42`) → runtime error.
- Add a for-each section to `examples/demo.verb` and to the integration
  example so cross-compile coverage exercises it.

## Out of scope (v1)

- Index/key + element dual binding (`each i, x in …`).
- Iterating map values or entries directly (keys only; values via
  `map_get`).
- Descending or stepped ranges.
- `break` / `continue` (no such statements exist in Verb yet).
- Lazy/first-class range values — `to` is loop-head-only sugar.
