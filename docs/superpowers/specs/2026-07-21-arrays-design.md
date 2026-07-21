# Arrays — Design Spec

Date: 2026-07-21
Status: approved

## Purpose

Add growable arrays to Verb. Currently listed as an explicit v1 limitation
("No arrays/maps") in `README.md` and the compiler design spec. This spec
covers arrays only — maps remain out of scope.

## Syntax

No new bracket tokens. Arrays use word-style syntax consistent with the
rest of the language (`add`, `join`, `trails`, etc.):

```
assign a list 0, 5, 6;
print(get(a, 0));       %% 0
set(a, 0, 9);
push(a, 42);
print(len(a));          %% 4
assign x pop(a);
print(x);                %% 42
```

- `list e1, e2, ..., en` — array literal expression, introduced by the new
  `list` keyword. Not a function call: it greedily parses one
  `expression()` then consumes `,`-separated `expression()`s until the
  next token isn't a comma. No parens, no closing delimiter.
- `get(arr, i)`, `set(arr, i, v)`, `push(arr, v)`, `pop(arr)`, `len(arr)` —
  built-in functions, dispatched by name the same way `print` already is
  (`gen_call` in `src/codegen.rs`, name check before the general
  user-function/closure-call path).

### `list` parsing ambiguity (accepted limitation)

Because `list` has no closing delimiter, it greedily swallows every
subsequent comma-separated expression until a non-comma token. This is
fine standalone or as the last thing before a `;`/`)`, but it means a
`list` literal can't be followed by a sibling argument inside a call:

```
push(a, list 1, 2)        %% fine — list eats "1, 2", then ')' stops it
foo(list 1, 2, 3)          %% list eats "1, 2, 3" — no way to give foo
                             %% a second argument after the list
```

The same swallowing applies to `list` nested as a non-final element of
another `list`: `list list 1, 2, list 3, 4` does **not** produce two
two-element arrays. The first element's `expression()` call hits `list`
again and that inner call greedily eats everything through the end of
the input (`1, 2, list 3, 4`, itself absorbing the second `list` too),
leaving the outer `list` with exactly one element. Building arrays of
arrays inline isn't supported by this syntax; assign the inner arrays to
variables first and reference them by name, since a bare `Var` is a
single token and doesn't trigger the swallowing:

```
assign inner1 list 1, 2;
assign inner2 list 3, 4;
assign outer list inner1, inner2;   %% [[1, 2], [3, 4]]
```

This is a known, accepted restriction of the no-delimiter syntax
(documented here and in a parser comment), not something the
implementation needs to work around.

## Value representation

New tag:

| Tag | Type | Payload |
|---|---|---|
| 6 | Array | ptr to heap `{ i64 len, i64 cap, ptr elems }` |

Add `TAG_ARRAY: u64 = 6` to `src/value.rs`.

`elems` is a `malloc`'d buffer of `%verb.value` structs (16 bytes each,
tag + i64 payload — same struct arrays store internally). The array
header itself (`{ len, cap, elems }`, 24 bytes: i64 + i64 + ptr) is also
`malloc`'d once at creation; the payload of a `TAG_ARRAY` value is that
header pointer, stored via `build_ptr_to_int`/read via `build_int_to_ptr`
— the same pattern already used for strings (`codegen.rs:514-515`) and
closures (`codegen.rs:619-620`).

Arrays nest for free: an array element is just a `%verb.value`, so an
array can hold ints, strings, closures, or other arrays (array-of-arrays,
array-of-closures) with no special-casing.

Growth strategy for `push`: when `len == cap`, `malloc` a new `elems`
buffer at `max(1, cap * 2)`, copy the old contents over (`memcpy` via a
manual element-by-element load/store loop, since there's no `memcpy`
declared yet — simplest to add one alongside `malloc`/`strlen`/etc. in
`declare_libc`), write the new element, bump `len`. No `free` of the old
buffer — consistent with the project's "no GC in v1" stance; old buffers
leak like every other heap allocation in Verb today.

## Built-ins

All dispatched in `gen_call` (`codegen.rs:949`) by name, same tier as the
existing `print` check — checked before the `is_bound`/user-function
lookup so a user can't accidentally shadow them (matches existing `print`
behavior per the README's "Shadowing the builtin `print` has no effect"
note).

- **`list e1, ..., en`** — not a call; a primary-expression parse rule
  producing `Expr::ArrayLit(Vec<Expr>)`. Codegen: `malloc`s the header +
  an `elems` buffer sized exactly `n`, evaluates each element expression
  in order and stores it, sets `len = cap = n`.
- **`get(arr, i)`** — 2 args. Runtime-checks `arr` is `TAG_ARRAY` and `i`
  is `TAG_INT`; checks `0 <= i < len`; returns `elems[i]`.
- **`set(arr, i, v)`** — 3 args. Same checks as `get`; stores `v` at
  `elems[i]`; returns `v`.
- **`push(arr, v)`** — 2 args. Checks `arr` is `TAG_ARRAY`; grows if
  needed (see above); appends `v`; returns `nil`.
- **`pop(arr)`** — 1 arg. Checks `arr` is `TAG_ARRAY`; runtime error if
  `len == 0`; decrements `len`, returns the removed element.
- **`len(arr)`** — 1 arg. Checks `arr` is `TAG_ARRAY`; returns `len` as a
  `TAG_INT` value.

Arity mismatches on any of these are a compile-time error, same style as
`print takes exactly 1 argument` (`codegen.rs:956`).

## Runtime errors

Same `abort_at` pattern as every other runtime error in the compiler
(`codegen.rs:105-116`: printf a `runtime error [%d:%d]: ...` message,
then `exit(1)`):

- Non-array first argument to `get`/`set`/`push`/`pop`/`len` — type
  mismatch, reports the actual type via `verb_type_name`/`type_name`
  (same helper `build_concat_fn` already uses for its own type-mismatch
  message, `codegen.rs:519-520`).
- Non-int index argument to `get`/`set` — type mismatch.
- Out-of-bounds index (`i < 0 or i >= len`) on `get`/`set` — bounds error,
  message includes the index and length.
- `pop` on an empty array — "pop from empty array" error.

## Printing

Extend `verb_print`'s tag switch (`codegen.rs:173-180`) with a `TAG_ARRAY`
case: prints `[e0, e1, e2]` — `[`, each element via a recursive call to
`verb_print`'s per-element logic (or a shared element-formatting helper),
comma-space separated, `]`, newline. Strings inside arrays print without
extra quoting (matches how `print("hi")` prints `hi`, not `"hi"`) —
consistent but means `list "a", "b"` prints as `[a, b]`, not `["a", "b"]`;
accepted as consistent with the rest of the language's plain `print`
behavior rather than adding a separate "repr" formatter.

## Testing

Same layers as the rest of the compiler (per
`docs/superpowers/specs/2026-07-19-verb-compiler-design.md`):

- **Lexer**: `list` scans as a keyword, not an identifier.
- **Parser**: `list 1, 2, 3` parses to `Expr::ArrayLit([Int(1), Int(2),
  Int(3)])`; `list` as a trailing call argument (`push(a, list 1, 2)`);
  `list` swallowing a would-be sibling call argument (`foo(list 1, 2, 3)`
  — asserts it parses as a 1-arg call, not 2) and swallowing a nested
  `list` (`list list 1, 2, list 3, 4` — asserts the outer literal ends up
  with exactly one element) — both document the greedy-swallow behavior
  with a passing test, not just prose.
- **Golden/e2e** (`tests/fixtures/*.verb` + `.expected`): literal +
  print, `get`/`set`, `push`/`pop`/`len`, growth past initial capacity
  (push enough elements to force at least one resize), array of arrays
  built via variables (per the nesting workaround above), array of
  closures, each runtime-error case (non-array arg, bad index type,
  out-of-bounds, pop-empty) with expected error text.
- **Snapshot**: one `.ll` substring check confirming the `TAG_ARRAY`
  malloc/store path shows up in emitted IR for a simple literal, matching
  the style of existing snapshot tests.

## Out of scope

- Map/dictionary type.
- Deep/structural array equality. `build_eq_fn`'s switch (`codegen.rs:436`)
  falls through to its `raw_bb` default for any tag it doesn't special-case,
  which compares payloads directly — so `eqeq`/`neq` on two `TAG_ARRAY`
  values already works today with no codegen changes, but as **pointer**
  (reference) equality: two arrays are `eqeq` only if they're the same
  heap object, not if they hold equal elements. This is existing,
  well-defined behavior (same as closures get today), not a gap to fix —
  just calling it out so it isn't mistaken for structural equality later.
- Slicing (`a[1..3]`-style).
- `for`-each iteration sugar — the existing C-style `loop` combined with
  `get`/`len` already covers iteration; can be revisited later if it
  proves painful in practice.
- `--emit-llvm`/formatter support for `list`/array builtins is included
  (formatter must round-trip the new syntax without corrupting it, same
  bar as every existing construct) but is not called out as a separate
  feature — it's covered by the general "don't break the formatter"
  expectation already implied by existing formatter tests.
