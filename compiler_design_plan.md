# Context

User wants to build custom programming language + LLVM-based compiler, mainly for learning compiler construction end-to-end (lexer → parser → AST → LLVM IR → binary). Decided so far:
- Purpose: educational/toy language
- Type system: dynamic typing (checked at runtime — LLVM codegen will need tagged/boxed values)
- Syntax family: to be settled via this draft syntax doc (below), not multiple-choice — user wants concrete draft to react to

This doc is currently a working draft inside the plan file (plan mode only permits editing this file). Once approved it becomes the real spec, written to something like `docs/language-spec.md` in the project.

# Draft: Language Syntax (v0.1)

Modeled closest to **Lox** (Crafting Interpreters) — C-like braces, dynamic typing — since that's a well-trodden, well-documented path for exactly this kind of educational compiler project, and it maps cleanly onto LLVM codegen concepts (tagged values, closures, etc.) without excess syntax complexity. Name TBD — using `<lang>` as placeholder below.

## Comments

```
%% line comment
!?! block comment !?!
```

## Literals & types (all dynamic — no type annotations)

| Type | Example |
|---|---|
| Int | `42` |
| Float | `3.24` |
| String | `"hello"` |
| Boolean | `true`, `false` |
| Nil | `nil` |

## Variables / assignment

```
assign x 10;
assign name "compiler";
x be x plus 1;
```

## Operators

- Arithmetic: `plus minus mul div mod`
- Comparison: `eqeq neq lo hi loeq hieq`
- Logical: `and or not`
- String concat via `c` (e.g. `"a" c "b"`)

## Control flow

```
if (cond) {
  ...
} else if (other) {
  ...
} else {
  ...
}

while (cond) {
  ...
}

for (assign i 0; i lo 10; i be i plus 1) {
  ...
}
```

## Functions

```
fn add(a, b) {
  return a plus b;
}

result be add(1, 2);
```

## Built-in I/O

```
print(result);
```

# Open questions (for next round)

1. Int/float unified as one number type, or separate?
2. Closures / first-class functions in v1 scope or later?
3. Compound data types in v1 (arrays/lists, maps/objects) or defer to later version?
4. Error handling model (panic/exit vs. exceptions vs. result values)?
5. Language name?
