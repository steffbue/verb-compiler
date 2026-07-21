# Constraints (SPEC intel)

12 SPEC-classified documents were synthesized, all `confidence: high`, all
`locked: false`, all `precedence: null` (default ordering `ADR > SPEC > PRD >
DOC` applies — no ADRs present in this batch, so no SPEC content is overridden
by a higher-precedence source). Entries below are ordered by source date.

---

## Verb Compiler — Design Spec (v1)
- source: docs/superpowers/specs/2026-07-19-verb-compiler-design.md
- type: protocol
- content:
  Status: approved (2026-07-19). Defines the whole v1 language + compiler
  contract: dynamically typed language (Int/Float/String/Boolean/Nil, no
  arrays/maps/compound types in v1 — deferred), word-operators, lexical block
  scoping, closures capturing by reference (heap-boxed cells), no
  break/continue/anonymous fns in v1. Compiler: Rust + inkwell 0.9
  (`llvm20-1`), LLVM 20.1.3 via Homebrew. Runtime value = LLVM struct
  `%verb.value = { i8 tag, i64 payload }`: tag 0=Nil, 1=Bool, 2=Int, 3=Float,
  4=String(ptr), 5=Closure(ptr). Explicitly "No GC in v1" — string/closure/
  cell allocations `malloc`'d and never freed (deliberate simplification,
  later revisited by the refcounting-gc specs below). External symbols:
  libc only. Compile errors to stderr exit 1; runtime errors via printf then
  exit(1). Out of scope (v2+): arrays/maps, GC, break/continue, anonymous
  fns, string methods, modules/imports, result-style error handling — each
  of these is picked up by a later spec in this batch.

## C++ Import — Design Spec (v1)
- source: docs/superpowers/specs/2026-07-20-cpp-import-design.md
- type: api-contract
- content:
  Status: approved (2026-07-20). `import mod <ident>;` — top-level, bare
  identifier, must precede all other top-level statements, repeatable,
  deduplicated. ABI: reuses Verb's own tagged `VerbValue = { int8_t tag;
  int64_t payload; }` struct as the C-ABI boundary type — no per-function
  signatures, no type annotations. Call resolution: unresolved call name is
  treated as extern iff `Program.imports` is non-empty; arity is fixed by
  first call site, later mismatched-arity call is a compile error (not a
  linker error). `verb run` (JIT) rejects any program with imports; `verb
  build` (AOT) — was a stub, this spec requires it to actually work (link
  with `c++` when imports non-empty, else `cc`; new `-L<dir>` CLI flag).
  Out of scope (v2+): package manager, header parsing, classes/templates/
  overloads/demangling, JIT-mode extern support, typed per-function
  signatures, compile-time symbol/typo checking.

## Cross-platform `verb compile` — design
- source: docs/superpowers/specs/2026-07-20-cross-platform-compile-design.md
- type: protocol
- content:
  Context: `build_aot` was a stub since the original verb-compiler plan's
  Task 9 was never implemented. Goals: `verb build f.verb -o out` (no
  --target) = host `cc` build, no new dependency; `--target <os>-<arch>`
  cross-compiles to 1 of 6 combos (linux/macos/windows x x86_64/arm64,
  `x86` aliases `x86_64`); `--target all` builds all 6. Non-goals: no
  target auto-detection, no packaging/installers, no `verb targets`
  introspection command. Linking strategy: no-target path uses
  `Target::initialize_native` + host `cc`; cross-target path uses
  `Target::initialize_all` + `zig cc -target <zig-triple>` (zig chosen over
  requiring per-target native toolchains or leaving objects unlinked; zig
  must be on PATH or exit 1 with an install hint). Output naming: `.exe`
  auto-appended for windows targets; `--target all` writes
  `<out>-<os>-<arch>`. `--target all` is best-effort (continues past a
  single-combo failure, prints summary, exit 0 only if all 6 succeed).
  Cross-refs the original verb-compiler plan (Task 9) as prior art.

## Multi-file `.verb` linking — design spec
- source: docs/superpowers/specs/2026-07-20-multi-file-linking-design.md
- type: protocol
- content:
  Adds `verb run a.verb b.verb c.verb` / `verb build a.verb b.verb c.verb -o
  out` — multiple files concatenated (lexed/parsed independently, then
  their `Vec<Stmt>` lists concatenated in CLI-argument order) into one
  program passed to the existing `compile_program`. Non-goals: no in-source
  import/include syntax (explicitly rejected in favor of CLI-only linking),
  no "library file" convention, no new duplicate-symbol detection (later
  definition silently shadows, same as today's intra-file behavior).
  Cross-file function-call ordering follows the existing single-pass,
  no-forward-declaration rule, now spanning file boundaries. Adds
  `CompileError.file: Option<String>` + `with_file` builder for
  `file:line:col` error attribution on lexer/parser and codegen errors.
  Accepted limitation: runtime (JIT-generated, printf-baked) errors keep no
  filename — explicitly deferred, documented as a known v1 gap.

## `import std io` — Design Spec (v1)
- source: docs/superpowers/specs/2026-07-20-std-io-import-design.md
- type: api-contract
- content:
  Status: approved (2026-07-20). Adds curated stdlib I/O (stdin, whole-file
  read/write, blocking TCP) via `import std <ident>;` (new `std` keyword);
  v1 recognizes exactly one module name, `io` — unrecognized name is a
  compile-time error (unlike `import mod`, `std` module names are
  first-party and checked at parse time). 10 functions in
  `runtime/verb_std_io.cpp`: `read_line/file_read/file_write/file_append/
  tcp_connect/tcp_listen/tcp_accept/send_line/recv_line/close_conn` — all
  arity-checked at compile time against a known table (stronger guarantee
  than generic `import mod` externs). Uniform error convention: failure
  returns `verb_nil()`, no C++ exception crosses the FFI boundary. File/
  socket handles reuse the existing `VERB_INT` tag — no new `VerbValue`
  tag. Build-only (like `import mod`): `verb run`/JIT rejects programs with
  `std_imports` non-empty. Out of scope: additional std modules,
  generic containers/templates, non-blocking/async sockets, UDP, TLS.

## VERB_EXPORT macro — design spec
- source: docs/superpowers/specs/2026-07-20-verb-export-macro-design.md
- type: api-contract
- content:
  Header-only C++ macro (`runtime/verb.h`, C++-only, guarded so the
  existing C-compatible section is untouched) that auto-generates the
  `extern "C" VerbValue` wrapper boilerplate `import mod` otherwise
  requires hand-writing. `VERB_EXPORT(exported_name, arity, callable)` —
  `arity` is a literal integer 0-6; `callable` must resolve to one concrete
  (non-overloaded) function, disambiguated via cast if overloaded (e.g.
  `std::sqrt`). Exactly 4 supported C++ types map to Verb tags: `int64_t`
  (VERB_INT), `double` (VERB_FLOAT), `const char*` (VERB_STRING), `int`
  standing in for Verb bool (VERB_BOOL, NOT C++ `bool`), plus `void` as a
  return-only type. All type/arity mismatches are C++ compile-time
  `static_assert` failures pointing at the `VERB_EXPORT(...)` call site.
  Purely additive — no change to `src/`, `.verb` language, or `verb run`'s
  "imports require verb build" restriction. Non-goals: arity > 6, types
  beyond the 4 mapped, lambdas/function objects, auto-resolving ambiguous
  overloads.

## Verb formatter + Neovim format-on-save — design
- source: docs/superpowers/specs/2026-07-20-verb-formatter-design.md
- type: protocol
- content:
  Token-stream printer (not AST-unparse, not tree-sitter/topiary) walking
  `lexer.rs`'s token stream, shaped like `parser.rs`'s recursive-descent
  grammar, emitting formatted text instead of an AST — chosen because
  `ast::Stmt`/`Expr` carry no comment info and `parser.rs` desugars `loop`
  into `Block([Assign, While])`, so unparsing the AST would destroy `loop`
  and drop all comments. Accepted trade-off: this is a third place encoding
  Verb's grammar shape (after `parser.rs` and `tree-sitter-verb/grammar.js`).
  New `lexer::lex_with_comments` (additive; existing `lex` keeps exact
  signature/behavior). New `src/formatter.rs::format(src) -> Result<String,
  CompileError>` — validates via the real lex+parse pipeline first, errors
  out unchanged on invalid input. Formatting rules: 2-space indent per
  begin/end level, one space around binary/keyword operators, no space
  before `;`/`,`, blank-line collapsing (max 1 consecutive). Wired into
  `verb-lsp` via `textDocument/formatting` (whole-document `TextEdit`) and
  a Neovim `BufWritePre` autocmd (outside this repo). Non-goals: no
  standalone `verb fmt` CLI, no line-wrapping/reflow, no trailing-comment
  column alignment.

## VSCode extension for Verb — LSP client, formatter, tree-sitter highlighting
- source: docs/superpowers/specs/2026-07-20-vscode-extension-design.md
- type: protocol
- content:
  VSCode extension providing LSP (hover/completion/diagnostics via
  `verb-lsp` over stdio), format-on-save (`configurationDefaults` sets
  `editor.formatOnSave: true`), and tree-sitter-WASM-based syntax
  highlighting — chosen over a TextMate grammar to avoid a 4th
  duplicate-grammar copy (after `lexer.rs`/`parser.rs`,
  `tree-sitter-verb/grammar.js`, and the formatter's own grammar-shaped
  printer). Compiles `editors/tree-sitter-verb` to WASM (`tree-sitter build
  --wasm`, Docker fallback for emscripten) and loads it via
  `web-tree-sitter` at runtime; semantic-tokens provider walks the parsed
  tree using the same node/field patterns as
  `tree-sitter-verb/queries/highlights.scm`. `verb.lspPath` setting default
  is an absolute, machine-specific path (matches nvim config's hardcoded
  convention). `web-tree-sitter` version must match `tree-sitter-cli`'s ABI
  (`^0.25`). Non-goals: no Marketplace publish, no automated
  extension-host test suite, no debugger integration, no incremental
  tree-sitter re-parsing, no TextMate grammar.

## Arrays — Design Spec
- source: docs/superpowers/specs/2026-07-21-arrays-design.md
- type: schema
- content:
  Adds growable arrays. `list e1, ..., en` — new keyword, no bracket
  tokens; greedily parses comma-separated expressions with no closing
  delimiter (accepted limitation: cannot be followed by a sibling call
  argument, and nested non-final `list` literals don't work — must assign
  inner arrays to variables first). New tag: **`TAG_ARRAY = 6`**, payload =
  ptr to heap `{ i64 len, i64 cap, ptr elems }`; `elems` is a separately
  `malloc`'d buffer of `%verb.value` structs. Built-ins dispatched by name
  in `gen_call` (same tier as `print`): `get(arr,i)`, `set(arr,i,v)`,
  `push(arr,v)` (grows via `malloc` + copy when `len==cap`, old buffer
  never freed — "no GC in v1" stance), `pop(arr)`, `len(arr)`. Runtime
  errors via the existing `abort_at` pattern. Array equality (`eqeq`/`neq`)
  is explicitly pointer/reference equality (existing `build_eq_fn` default
  case), not structural — called out, not a gap to fix. Out of scope: maps,
  deep/structural equality, slicing, for-each sugar.
  **NOTE — tag collision:** this spec (and its companion plan,
  `docs/superpowers/plans/2026-07-21-arrays.md`, whose Global Constraints
  section also states `TAG_ARRAY = 6`) assigns tag 6 to Array. The Maps
  design spec (below, same date) independently assigns tag 6 to Map. See
  `INGEST-CONFLICTS.md` WARNINGS — the shipped codebase (`src/value.rs`,
  `runtime/verb.h`) resolves this as `TAG_MAP = 6` / `TAG_ARRAY = 7`, so
  this spec document's literal tag value is stale relative to what was
  actually implemented.

## Maps (`import std map`) — design
- source: docs/superpowers/specs/2026-07-21-maps-design.md
- type: schema
- content:
  Adds a hash-map type via `import std map;`, mirroring `import std io`'s
  opt-in, build-only pattern exactly (new `runtime/verb_map.cpp`, gated,
  usable only with `verb build`, not `verb run`/JIT). New tag **`VERB_MAP =
  6`** in `runtime/verb.h` / `TAG_MAP` in `src/value.rs` — payload = ptr to
  heap `std::unordered_map`-backed struct, `new`'d, never freed (no GC, no
  `map_free`). Map keys restricted to nil/bool/int/float/string (closures
  and nested maps rejected as keys); numeric keys follow the same
  cross-tag equality as elsewhere (`1` int == `1.0` float as a key).
  6 builtins: `map_new/map_set/map_get/map_has/map_remove/map_len`, wired
  through the same `gen_std_io_call`-style path as `import std io`
  (generalized `MAP_FUNCS` table). Failure mode returns nil/false/0, not a
  runtime panic. No iteration/keys/values function in v1 (no arrays to
  return a key list into, at time of writing — the arrays spec above adds
  that capability in the same batch). No Windows cross-compile
  restriction (`std::unordered_map` is portable, unlike `std io`'s POSIX
  socket dependency). See tag-collision note in Arrays entry above.

## Reference-counting garbage collector (v1)
- source: docs/superpowers/specs/2026-07-21-refcounting-gc-design.md
- type: nfr
- content:
  Adds precise compile-time-inserted reference counting (chosen over a
  tracing collector: no bytecode VM/stack maps, inkwell doesn't expose
  `gc.statepoint`, and stack-scanning is fragile against optimized native
  code). Covers the 3 heap kinds existing at spec time: string buffers,
  closure structs (env always null — closures cannot yet capture anything,
  so **no reference cycle is currently constructible**, making refcounting
  exact, not heuristic), and cells (boxed locals/params). Explicitly
  out of scope: arrays/maps (not implemented yet at spec time), cycle
  collection, concurrent/compacting GC. Every heap block gets an 8-byte
  `i64` refcount header at `payload - 8`; static string literals get a
  sentinel (`INT64_MIN`) in the same header position. New runtime API:
  `verb_alloc`, `verb_retain_value`/`verb_release_value`,
  `verb_retain_cell`/`verb_release_cell` — codegen never branches on tag in
  Rust, always emits an unconditional call; the tag switch lives once in
  the C runtime. Codegen insertion rule: every `gen_expr` result is an
  owned temporary, transferred or released. `extern`/`std io` contract
  change: C++ code handing a heap string back to Verb must use
  `verb_alloc_string`, not raw `malloc`/`strdup`.
  **Superseded — see INGEST-CONFLICTS.md INFO:** the v2 spec below states
  this design's implementation (PR #11) was closed unmerged because `main`
  diverged too far for a clean rebase; v2 re-applies this design unchanged
  and extends it. This entry is preserved for historical/provenance
  purposes; v2 is the current authoritative GC design.

## Reference-counting GC v2: strings, closures, arrays, maps
- source: docs/superpowers/specs/2026-07-21-refcounting-gc-v2-design.md
- type: nfr
- content:
  Extends the v1 design (unchanged in its core) to also cover arrays
  (`TAG_ARRAY`), maps (`TAG_MAP`), and a new global-binding mechanism
  (top-level `Stmt::Assign`/`Declare` now live in module-level LLVM globals,
  not malloc'd cells) that landed on `main` after v1's PR was abandoned.
  Same 8-byte refcount header convention; same 4 runtime functions,
  extended tag dispatch (STR/CLOSURE/ARRAY/MAP). Cascade on release-to-zero:
  ARRAY releases every element then frees `elems` then the header; MAP
  calls a new `verb_map_destroy_contents` that releases every key/value
  then runs the map's destructor explicitly (placement-new requires this;
  `delete` would double-free) before the generic header free.
  **Cycles are now really possible** (arrays/maps are mutable,
  reference-stored containers with no self-reference check — e.g.
  `push(a, a)` is valid, unrejected Verb). Explicitly a known, accepted
  limitation carried to a separate later sub-project (a backup cycle
  collector) — this spec's job is only to make refcounting correct for the
  acyclic case and prove (via a fixture) that the cyclic case leaks in a
  *bounded*, confined way, not via corruption or unbounded growth. Also
  fixes a pre-existing leak in `push`'s grow path (old `elems` buffer never
  freed) as incidental work since GC already touches that code path. New
  codegen sites beyond v1: `build_array_get_fn`/`set_fn`/`push_fn`/`pop_fn`,
  `MAP_FUNCS` call path, `bind()`'s global-slot branch (release-before-
  store, including first bind — a fresh global slot zero-inits to
  `{NIL,0}`, so releasing it is a no-op), and program-exit (release every
  global's current value, since globals live outside `self.scopes`).
