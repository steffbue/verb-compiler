# C++ Import — Design Spec (v1)

Date: 2026-07-20
Status: approved

## Purpose

Let Verb programs call into C++ libraries via C-ABI FFI. Scope is deliberately narrow: link a native library and call its `extern "C"` functions by name. No package manager, no header parsing, no C++ classes/templates/overloads, no name demangling.

## Language surface

```
import mod mathlib;

assign r c_sqrt(2.0);
print(r);
```

- `import mod <ident>;` — top-level statement, bare identifier (no quotes), one library per statement, repeatable.
- All `import mod` statements must appear before any other top-level statement in the file.
- No extern function signature list, no type annotations anywhere. This is a deliberate simplification over a typed-FFI design (rejected in favor of this one): Verb's own tagged value struct doubles as the C-ABI boundary type (see below), so there is nothing to declare per-function.
- `<ident>` maps directly to a linker `-l<ident>` flag. No support for hyphenated/dotted library names in v1 (identifier lexical rules only).

## Call resolution

A call `name(args...)` where `name` does not resolve to a local variable or a Verb `fn`:

- If the program has **zero** `import mod` statements: existing behavior — "undefined variable" compile error (unchanged from today).
- If the program has **at least one** `import mod` statement: `name` is assumed to be an extern C++ function. No compile-time check that the symbol actually exists or that arity is "correct" for the C++ side — a bad name surfaces as a linker "undefined symbol" error when `verb build` links; only the second call site with a mismatched arity for a name already used earlier is caught by Verb directly (see Codegen).

This is a known, accepted gap: no typo/arity checking against the actual C++ side. Traded deliberately for zero declaration syntax.

## ABI: reuse Verb's own tagged value struct

Verb's internal runtime value is the LLVM struct `%verb.value = { i8, i64 }` (tag, payload) — already defined in `codegen.rs` (`value_ty`), already used by-value across all of Verb's internal runtime helpers (`verb_add`, `verb_eq`, etc.). This struct is not packed, so its in-memory layout (1-byte tag, 7 bytes padding, 8-byte payload, size 16, align 8) matches an ordinary non-packed C/C++ struct:

```c
// runtime/verb.h — shipped with Verb, included by C++ extern fn implementations
#include <stdint.h>

typedef struct { int8_t tag; int64_t payload; } VerbValue;

enum { VERB_NIL = 0, VERB_BOOL = 1, VERB_INT = 2, VERB_FLOAT = 3, VERB_STRING = 4 };
// tag 5 (closure) never crosses the FFI boundary — no C++-side representation.

static inline VerbValue verb_nil(void)            { return (VerbValue){ VERB_NIL, 0 }; }
static inline VerbValue verb_bool(int b)           { return (VerbValue){ VERB_BOOL, b ? 1 : 0 }; }
static inline VerbValue verb_int(int64_t v)        { return (VerbValue){ VERB_INT, v }; }
static inline VerbValue verb_float(double v)       { VerbValue r; r.tag = VERB_FLOAT; __builtin_memcpy(&r.payload, &v, 8); return r; }
static inline VerbValue verb_string(const char* s) { VerbValue r; r.tag = VERB_STRING; __builtin_memcpy(&r.payload, &s, 8); return r; }

static inline int     verb_is(VerbValue v, int tag) { return v.tag == tag; }
static inline int64_t verb_as_int(VerbValue v)      { return v.payload; }
static inline double  verb_as_float(VerbValue v)    { double d; __builtin_memcpy(&d, &v.payload, 8); return d; }
static inline const char* verb_as_string(VerbValue v) { const char* s; __builtin_memcpy(&s, &v.payload, 8); return s; }
```

C++ extern functions are written directly against `VerbValue`, e.g.:

```cpp
#include "verb.h"
#include <cmath>

extern "C" VerbValue c_sqrt(VerbValue x) {
    return verb_float(std::sqrt(verb_as_float(x)));
}
```

Any type-checking of arguments (is this actually a float?) is the C++ implementation's own responsibility, using `verb_is`/tag constants — Verb performs none at the call boundary. This mirrors the "no per-fn declared types" decision: there is no static type to check against.

## AST

```rust
pub struct Program {
    pub imports: Vec<String>,   // library names, in source order, deduplicated at parse time
    pub body: Vec<Stmt>,
}
```

`parser::parse` returns `Program` instead of `Vec<Stmt>`. No new `Stmt`/`Expr` variants — extern calls are ordinary `Expr::Call { callee: Expr::Var(name), .. }`, disambiguated at codegen time per the resolution rule above.

## Codegen

- `Codegen` gains `imports: Vec<String>` (copied from `Program`) and `externs: HashMap<String, FunctionValue<'ctx>>`.
- In `gen_call`, when `callee` is `Expr::Var(name)` and `name` is not a local/known Verb fn and `!self.imports.is_empty()`:
  - If `externs` has `name`: verify `fnv.count_params() as usize == args.len()`; mismatch → `CompileError` ("extern fn 'name' called with N args, previously called with M", at the new call site's line/col).
  - Else: declare `self.module.add_function(name, value_ty.fn_type(&[value_ty; args.len()], false), None)`, insert into `externs`.
  - Evaluate args to `StructValue`s (existing `gen_expr`), direct (non-indirect) `build_call` with those values, result is the returned `StructValue` (or, for a `void`-returning-nothing case — not applicable, every extern fn's declared return type is `VerbValue`; a C++ fn that wants "no useful result" just returns `verb_nil()`).
- `compile_program` takes `&Program` instead of `&[Stmt]`.

## `verb build` (AOT) — implemented for real

`build_aot` is currently a stub (`eprintln!("build: not implemented yet"); exit(1);`). This feature requires it to actually work, since extern symbols only resolve at link time:

1. Emit an object file from the module via `inkwell::targets::TargetMachine`.
2. Link with `c++` (not `cc`) whenever `imports` is non-empty, so the C++ runtime (libstdc++/libc++, exceptions, etc. pulled in transitively by the target library) resolves correctly; plain `cc` otherwise (unchanged for import-free programs).
3. Pass `-l<name>` for each entry in `imports`.
4. New repeatable CLI flag on `verb build`: `-L<dir>`, passed through as additional linker search paths.

`verb run` (JIT): as of this spec, if `!program.imports.is_empty()`, reject with a compile-time-style error before attempting to JIT ("imports require 'verb build' — JIT does not support extern C++ calls in v1"). JIT-side `dlopen`/`dlsym` support for imports was out of scope here, deferred to v2 — it has since shipped under **FFI-V2-01**; see `docs/superpowers/specs/2026-07-23-jit-import-support-design.md` for the `verb run` JIT-import design (mod libs via `dlopen`, `std io`/`std map` via compiled-in symbol registration).

## Testing

- Parser unit tests: `import mod x;` parsing, import-must-precede-statements ordering error, dedup of repeated imports.
- Codegen unit test: arity-mismatch-across-call-sites compile error.
- New fixture library: `tests/fixtures/cpp/mathlib.cpp` (`extern "C"` functions using `runtime/verb.h`), compiled to a shared library at test time via `Command` invoking `c++` from the Rust test harness.
- New e2e fixture: a `.verb` file that does `import mod mathlib;`, calls its functions, run through `verb build` + execute-the-binary + diff stdout (extends the existing golden/e2e harness, which currently only JIT-runs).

## Out of scope (v2+)

Package manager/fetching, C++ header parsing/auto-binding, classes/templates/overloads/name demangling, ~~JIT-mode (`verb run`) extern support~~ (now supported — see **FFI-V2-01** and `docs/superpowers/specs/2026-07-23-jit-import-support-design.md`), extern functions as first-class values, typed per-function signatures, compile-time symbol/typo checking, non-identifier library names.
