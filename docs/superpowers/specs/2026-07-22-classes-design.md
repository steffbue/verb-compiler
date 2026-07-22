# Verb Classes (v1) — Design

Adds heap-allocated, GC-managed **classes** to Verb: user-defined types
that bundle named fields with methods and a constructor. Classes are a
reference type with object identity, distinct — by keyword, semantics, and
runtime tag — from a future plain-data `struct` construct.

Status: approved design, pre-implementation.
Branch: builds on `refcounting-gc-v2` (reuses its refcount GC).

## Goals

- `forge` a class with instance fields and methods.
- `spawn` an instance; a `init` method runs as constructor.
- Read/write fields and call methods through the new `.` operator.
- Objects live on the heap and are freed by the existing reference-counting
  GC — no leaks, no manual free.
- Works in both `verb run` (JIT) and `verb build`/`compile` (AOT) with **no
  C++ runtime and no `import`** — everything is generated LLVM IR. This is a
  deliberate contrast with `std io`/`std map`, which require importing and
  linking C++.

## Non-goals (v1)

- No inheritance, no `super`, no method override across parents.
- No plain-data `struct` — the `shape` keyword is *reserved* for it (see
  "Reserved words") but not implemented here.
- No static/class methods, no visibility modifiers, no operator overloading.
- No closures over enclosing scope in methods (same limitation as `make`).
- No field access on non-object values beyond a clean runtime error.

## Distinguishing classes from future structs

The user requirement: classes must stay visibly and structurally different
from a `struct` feature added on a later branch. Three axes keep them apart:

| Axis | Class (this spec) | Future struct (reserved) |
|---|---|---|
| Keyword | `forge` | `shape` |
| Semantics | reference type, identity, methods, `init` | plain value data, fields only |
| Runtime tag | `TAG_OBJECT = 8` (own heap object) | its own future tag/repr |
| Creation | `spawn Name(args)` runs `init` | future literal, no constructor |

`shape` is reserved at the lexer/parser level now so adding structs later is
not a breaking change.

## Syntax

### Definition — `forge`

```
forge Point begin
  has x;
  has y;

  make init(self, a, b) begin
    self.x be a;
    self.y be b;
  end

  make sum(self) begin
    return self.x add self.y;
  end
end
```

- `forge <Name> begin ... end` — `<Name>` is an identifier in a **separate
  class namespace** (not a variable). Redefining a class name is a compile
  error.
- Body contains, in any order:
  - `has <ident>;` — declares one instance field. Duplicate field names in
    one class are a compile error.
  - `make <name>(<params>) begin ... end` — a method. Parsed exactly like a
    top-level function. By convention the first parameter is `self`, but
    `self` is **not** a keyword — it is an ordinary parameter name the method
    body uses to reach the receiver. Duplicate method names in one class are
    a compile error.
- `init` is an optional method treated as the constructor (see `spawn`).
- Nothing but `has` and `make` may appear in a class body; anything else is a
  parse error.

### Instantiation — `spawn`

```
assign p spawn Point(1, 2);
```

- `spawn <Name>(<args>)` is an expression. `<Name>` must name a class defined
  anywhere in the program (classes are resolved whole-program, so forward
  references are allowed).
- Allocation: heap object via `verb_alloc`, all field slots initialized to
  `nil`, `class_id` stamped.
- If the class defines `init`, it is called as `init(obj, args...)` — the
  arg count at the `spawn` site must equal `init`'s parameter count minus one
  (the implicit `self`). Mismatch is a compile error where statically
  determinable, else a runtime error.
- If the class has no `init`, `spawn Name(...)` must be called with **zero**
  args; fields start `nil`.
- Result is a `VerbValue` with `tag = TAG_OBJECT`.

### Field access & method call — `.`

```
print(p.x);        %% field read
p.x be 5;          %% field write
print(p.sum());    %% method call
```

- New postfix operator `.`:
  - `<expr>.<ident>` → field read (`FieldGet`).
  - `<expr>.<ident>(<args>)` → method call (`MethodCall`); receiver is passed
    as the method's first parameter (`self`).
  - `<lvalue>.<ident> be <expr>;` → field write (`FieldSet`), where `<lvalue>`
    is any expression evaluating to an object.
- `.` chains left-to-right: `a.b.c`, `a.b().c`, etc.
- The field/method **name is always a source-level identifier** — never a
  dynamic value. There is no computed member access in v1.

### Equality & printing

- `equals` / `differs` on two objects compare by **reference identity** (same
  heap pointer), matching arrays' documented behavior. An object never equals
  a non-object.
- `print(p)` renders `Point { x: 1, y: 2 }` — class name, then fields in
  declaration order. Nested objects/arrays/strings render via the normal
  value printer.

## Runtime representation

`value.rs` gains:

```
pub const TAG_OBJECT: u64 = 8; // payload = ptr to heap object
```

Heap layout (pointer returned by `verb_alloc`, which prefixes the 8-byte
GC refcount header initialized to 1):

```
[ i64  class_id ]            offset 0
[ VerbValue slot_0 ]         offset 8
[ VerbValue slot_1 ]
...
[ VerbValue slot_{n-1} ]
```

`VerbValue` is `{ i8 tag, i64 payload }` (16 bytes with alignment). Total
allocation = `8 + 16 * nfields`. `class_id` is a small integer assigned at
compile time; it indexes the compiler's class table and is the sole runtime
discriminator for dispatch.

## Compile-time class table

Built during codegen (and referenced by the parser only for name
resolution). For every `forge`:

- `class_id: u32` — dense, assigned in definition order.
- `name: String`.
- `fields: Vec<String>` — declaration order; slot index = position.
- `methods: HashMap<String, FunctionValue>` — method name → generated LLVM
  function named `<Class>$<method>`, first param the receiver.
- `init: Option<...>` — present iff a method named `init` exists.

Method functions are lowered exactly like top-level `make` functions (same
codegen path, same closure limitation), just under a mangled name and always
taking the receiver as parameter 0.

## Dispatch: static name, dynamic class

Verb is dynamically typed, so at a `p.x` or `p.sum()` site the compiler does
**not** know `p`'s class — but it **does** know the field/method name (it is a
literal identifier in the source). This asymmetry drives the codegen:

For each access site, emit:

1. Check `p.tag == TAG_OBJECT`; if not → runtime error
   `field access on non-object` / `method call on non-object`.
2. Load `class_id` from the object.
3. `switch` on `class_id`:
   - **FieldGet/FieldSet `f`:** one case per class whose `fields` contains
     `f`; case body GEPs to that class's slot index and loads/stores.
     `default` → runtime error `no field f`.
   - **MethodCall `m`:** one case per class whose `methods` contains `m`;
     case body calls `<Class>$m(obj, args...)`. `default` → runtime error
     `no method m`. Arg-count mismatch against a matched method → runtime
     error.

No runtime string/hash tables, no per-object vtable pointer — the whole-
program class table makes every switch statically complete. Cost is one tag
check, one load, one `switch` per access.

`spawn Name(args)` needs no switch: `Name` resolves to a single `class_id` at
compile time, so allocation, slot count, and the `init` call are all emitted
directly.

## GC integration (reference counting, from `refcounting-gc-v2`)

Objects participate in the existing refcount GC exactly like strings, arrays,
and maps — they are `verb_alloc`'d (header initialized to 1) and flow through
`verb_retain_value` / `verb_release_value` (`codegen.rs:1150–1332`), which
currently tag-dispatch over string/array/map. Extend both:

- **`verb_retain_value`** — add a `TAG_OBJECT` arm: increment the refcount
  header at `ptr - 8`. (Same as the other heap arms.)
- **`verb_release_value`** — add a `TAG_OBJECT` arm: decrement the header;
  when it reaches 0, iterate `class_id`'s field slots and call
  `verb_release_value` on each (recursively freeing owned children), then free
  the object. Determining slot count from `class_id` uses a small generated
  switch (class_id → nfields), mirroring the field-count knowledge the
  compiler already has.
- **Field write `p.x be v`** — release the old slot value, retain `v`, then
  store (same retain-new/release-old discipline already used for variable
  reassignment and array `set`).
- **`spawn`** — the constructor path retains any value stored into a field
  through normal `self.x be ...` assignment, so no special casing beyond the
  field-write rule.

Because retain/release are generated **in-module**, GC works identically
under JIT and AOT. No changes to `runtime/*.cpp` or `runtime/verb.h` are
required — `TAG_OBJECT` never crosses the C ABI boundary in v1 (extern C++
functions can neither receive nor construct objects), matching how closures
(`tag 5`) are already excluded from that boundary.

## Reserved words

Add three active keywords and one reserved keyword to `lexer.rs`
(keyword table `lexer.rs:159–171`):

- `forge`, `has`, `spawn` — active, tokens `Forge` / `Has` / `Spawn`.
- `shape` — reserved. Lexes to a `Shape` token; the parser rejects it at
  statement position with `structs (shape) are not yet implemented`. This
  reserves the surface syntax for the future struct feature without
  implementing it, so structs land as a non-breaking addition.

`.` becomes a standalone `Dot` token. The number lexer already consumes `.`
only between digits for float literals (`lexer.rs:139`); a `.` not so
positioned now lexes as `Dot` (e.g. `p.x` → `Ident Dot Ident`). `init` and
`self` are **not** keywords — `init` is recognized only by position (a method
named `init` in a class body); `self` is an ordinary identifier.

## AST changes (`ast.rs`)

New `Expr` variants:

```
Spawn      { class: String, args: Vec<Expr>, line: u32, col: u32 },
FieldGet   { obj: Box<Expr>, field: String, line: u32, col: u32 },
MethodCall { obj: Box<Expr>, method: String, args: Vec<Expr>, line: u32, col: u32 },
```

New `Stmt` variants:

```
Forge    { name: String, fields: Vec<String>, methods: Vec<Method>, line: u32, col: u32 },
FieldSet { obj: Expr, field: String, value: Expr, line: u32, col: u32 },
```

New struct:

```
pub struct Method { pub name: String, pub params: Vec<String>, pub body: Vec<Stmt>, pub line: u32, pub col: u32 }
```

## Parser changes (`parser.rs`)

- `forge_stmt` (dispatched from the statement head at `parser.rs:184–200`):
  parse `forge Ident begin`, then loop over `has ident ;` and
  `make ...` (reusing the existing function-body parse to build `Method`),
  until `end`.
- Postfix `.` in the expression parser: after a primary/postfix expression,
  while the next token is `Dot`, consume `Dot Ident`; if `(` follows →
  `MethodCall`, else `FieldGet`. Integrates with existing call postfix so
  `p.f().g` chains.
- `spawn` primary: `spawn Ident ( args )` → `Spawn`.
- Reassignment (`reassign_stmt`, currently `ident be expr`): generalize so the
  left side may be a `FieldGet`, producing `FieldSet`; a bare `Var` still
  produces `Reassign`. Any other left side before `be` is a parse error.
- `shape` at statement head → error `structs (shape) are not yet
  implemented`.

## Codegen changes (`codegen.rs`)

- Pre-pass: walk `Program.body`, collect all `Forge` statements, assign
  `class_id`s, build the class table, and declare every `<Class>$<method>`
  function (so methods and `spawn` can forward-reference each other and
  classes).
- Lower each method body via the existing function-lowering path under its
  mangled name.
- `Spawn`: emit `verb_alloc`, store `class_id`, nil-fill slots, optional
  `init` call, yield a `TAG_OBJECT` value.
- `FieldGet` / `FieldSet` / `MethodCall`: emit the tag-check + `class_id`
  switch described above, with retain/release on field writes.
- Extend `verb_retain_value` / `verb_release_value` with the `TAG_OBJECT`
  arm and the `class_id → nfields` (and slot-release) switch.
- `print` / value formatter: add a `TAG_OBJECT` case that dispatches on
  `class_id` to render `Name { field: value, ... }`.

## Formatter changes (`formatter.rs`)

`verb fmt` must round-trip the new syntax without corrupting it: format
`forge`/`has`/method blocks (indent like functions), `spawn`, `.` access, and
`p.x be ...`. Covered by formatter idempotency tests.

## Out of scope but noted

tree-sitter grammar (`editors/tree-sitter-verb/grammar.js`), the VS Code
extension, and the LSP (`src/bin/verb-lsp.rs`) will not yet highlight or
understand class syntax. This is acceptable for v1 and called out so it is a
conscious follow-on, not a silent gap.

## Testing

End-to-end `.verb` programs compiled and run (JIT and AOT where the harness
supports both):

- **Happy path:** `forge` + `spawn` + field read; `init` sets fields; method
  reads/computes from `self`; field write then read; method returning derived
  value.
- **Identity:** two `spawn`s of the same class `differ`; `assign q p` then
  `q equals p`; object never equals a non-object.
- **Print:** `print(p)` shows `Name { ... }` in field order, including nested
  object/array/string fields.
- **GC:** allocate many objects in a loop (including objects holding
  strings/arrays and objects holding objects) and confirm no leak via the
  existing GC test harness; confirm a field overwrite releases the old value.
- **Errors:** `spawn` of an undefined class; field/method not on the object's
  class; field access / method call on a non-object; `init` arity mismatch;
  method arity mismatch; duplicate field/method/class names; `shape` used.
- **Formatter:** idempotent formatting of a class-heavy source file.
```
