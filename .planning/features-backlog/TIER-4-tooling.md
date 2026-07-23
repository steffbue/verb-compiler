# Tier 4 — Tooling / Ecosystem Implementation Plan

> Line numbers verified against branch `refcounting-gc-v2`.
> Ordered by value/effort (ROI). All independently shippable except where noted.

---

## Grounding — how the pipeline works today

- **CLI**: `parse_cli` (`src/main.rs:100-147`) builds `ParsedArgs` (`:91-98`); `main` (`:177-264`)
  lexes/parses each file, calls `cg.compile_program(...)` once (`:212`), then `match parsed.cmd`
  on `run` / `build|compile` (`:218-263`). `parse_cli` returns `None` → `usage()` exit 2 whenever
  `files` empty (`:143`) → a fileless `verb targets`/`verb repl` must dispatch **before** `parse_cli`.
- **JIT (`run`)**: `create_jit_execution_engine(OptimizationLevel::None)` (`:229-235`),
  `register_jit_runtime_symbols` (`:236`, defn `64-75`), `get_function::<...>("main")` + `call()`
  (`:238-242`). Rejects imports (`:220-228`).
- **AOT emit**: `build_aot_host` (`:310`), `build_aot_cross` (`:384`), `build_aot_all` (`:458`).
  Object emit = `tm.write_to_file(cg.module(), FileType::Object, ...)` (`:326`,`419`).
  `OptimizationLevel::Default` **hardcoded** at `:320` and `:411`.
- **Codegen**: `Codegen` struct (`src/codegen.rs:14-30`) owns module/builder/ctx/globals/externs/
  imports/cur_file. `new` (`:33`) builds runtime helper fns. Single entry `compile_program` (`:1471`).
  Module handed out read-only via `pub fn module(&self) -> &Module` (`:78`) — **no run_passes /
  mutation path, no DIBuilder field.**
- **Spans (critical for DWARF)**: `Token` has `line/col: u32` (`src/lexer.rs:16-19`). AST carries
  line/col **only on some nodes** — `Expr::Var/Binary/Unary/Call` (`src/ast.rs:14-17`),
  `Stmt::Reassign/Fn` (`:25,29`). **No `Span` type**; literals / `Assign` / control-flow have **no
  position**. `cur_file` (`codegen.rs:29`) + `stmt_files` (`main.rs:204`) give per-statement file.
- **Targets**: `src/targets.rs` — `Target{os,arch}`, `ALL:[Target;6]` (`:20`), `parse` (`:36`),
  `llvm_triple`/`zig_triple`/`label`/`is_windows`/`adjust_output`.
- **Std module wiring (4 touch-points)**: (1) parser `import_stmt` validates name vs `{io,map}`
  (`src/parser.rs:149-157`); (2) codegen arity tables `IO_FUNCS` (`codegen.rs:2114`), `MAP_FUNCS`
  (`:2133`) + resolution `:1966-1979` → `gen_std_io_call` (`:2021`); (3) `main.rs` compiles+links
  runtime `.o` (`compile_std_io_obj` `:278`, host link `337-364`, cross `422-447`); (4) `build.rs`
  compiles always-referenced units into the `verb` binary for JIT (`build.rs:29-34`).
- **Runtime ABI**: `runtime/verb.h` — `VerbValue{int8_t tag; int64_t payload}` (`:28`), ctors
  (`:39-55`), accessors (`:60-70`), templated `VERB_EXPORT` (`:77+`). `runtime/verb_std_io.cpp` is
  the module template.
- **inkwell 0.9**, feature `llvm20-1` (`Cargo.toml`) → new pass manager via
  `Module::run_passes(passes: &str, &TargetMachine, PassBuilderOptions)`.

---

## Task A — `verb targets` command + host auto-detect (smallest effort, immediate DX)

**Files**: `src/main.rs`, `src/targets.rs` (maybe), `tests/e2e.rs`.

**Approach**
1. In `main` (`:177`) **before** `parse_cli`: `if args.get(1).map(String::as_str)==Some("targets")
   { print_targets(); return; }` (sidesteps files-required `:143`).
2. `print_targets()` iterates `targets::ALL` (`targets.rs:20`) printing `label()` + `llvm_triple()`,
   marks host. Host detect: `TargetMachine::get_default_triple()` (used `:315`) `.as_str()`; add
   `Target::from_host_triple(&str) -> Option<Target>` in `targets.rs` matching x86_64/aarch64 ×
   linux/darwin/windows against `ALL`. Print `(host)` on the match.
3. Update `usage()` (`:169-175`) to list `verb targets`.

**Test**: e2e running `verb targets`, assert all 6 labels + exactly one `(host)` marker.

**Risks**: none (~30 lines). Stated non-goal but cheap.

---

## Task B — Optimizer pass + `-O0/-O1/-O2/-O3` flags (high ROI, self-contained)

**Files**: `src/main.rs`, `src/codegen.rs`, `tests/e2e.rs` (no dep change).

**Approach**
1. CLI: add `opt: u8` to `ParsedArgs` (`:91-98`); in `parse_cli` (`:112`) match `-O0..-O3` (default 0).
   Add to `usage()`.
2. Map to LLVM: JIT (`:231`) replace hardcoded `None` with `0→None,1→Less,2→Default,3→Aggressive`.
   AOT (`:320`,`411`) replace hardcoded `Default` in `create_target_machine` with same map.
3. Run pass pipeline: add `pub fn optimize(&self, tm:&TargetMachine, level:u8) -> Result<(),String>`
   on `Codegen` (near `module()` `:78`) using inkwell 0.9:
   `self.module.run_passes(&format!("default<O{level}>"), tm, PassBuilderOptions::create())`
   (skip when level==0). Call in each AOT path after `set_data_layout` (`:323`,`415`) before
   `write_to_file`. JIT has no TargetMachine → create host TM (mirror `build_aot_host:313-321`)
   just to call `run_passes` before building EE, gated on `opt>0` (for `-O2 run` parity).
4. Hand-rolled constant folding in `gen_expr` = low value (`instcombine`/`sccp` in `default<O1+>`
   already fold). Stretch, not core.

**Test**: `--emit-llvm` fixtures asserting `-O2` folds `print(2+3)` to a constant / fewer
instructions than `-O0`; `run_ok` at each level for a compute fixture (semantic invariance).

**Risks**: confirm `run_passes` signature vs installed inkwell 0.9 (`PassBuilderOptions` in
`inkwell::passes`). Verify `assert_no_leaks` fixtures pass at `-O2` (aggressive DCE must not drop
`verb_release_value` calls — they have side effects, safe, but test it).

---

## Task C — `verb repl` (high ROI; more plumbing, moderate risk)

**Files**: `src/main.rs` (dispatch + loop), maybe new `src/repl.rs` via `src/lib.rs`, `tests/e2e.rs`.

**Recommended — "declaration replay, fresh module per turn"** (lowest codegen risk):
- Maintain `Vec<String>` of history lines that produced **definitions only** (`Assign`/`Reassign`/`Fn`,
  no observable output). `print()` / bare expressions NOT retained.
- Per turn: read line → program = `history.join("\n") + "\n" + new_line` → lex/parse/`compile_program`
  into a **fresh Context + Codegen** → JIT as `:229-242` (reuse `register_jit_runtime_symbols`) →
  `call()` main. Replayed history has no printing → only new line's output appears; state (globals)
  rebuilt deterministically each turn.
- Auto-print expr results: wrap bare-expression entry as `print(<expr>);` before codegen.
- Imports: reject in REPL for v1 (mirror `run` restriction `:220`; JIT can't resolve `-l`).
- Dispatch like Task A (intercept `args[1]=="repl"` before `parse_cli`). Read `stdin().lock().lines()`,
  `verb> ` prompt, `:quit`.

**Alternative (defer)**: persistent EE + `add_module` per line, session globals promoted to External
linkage (`add_global_mapping`). Real incremental REPL but fights inkwell `Context` lifetime
(`Codegen<'ctx>` borrows `&'ctx Context`) + MCJIT symbol re-resolution. v2.

**Deps**: benefits from Task B (`-O` in REPL) but independent.

**Test**: pipe scripted session (`assign x 3;\nprint(x plus 4);\n:quit`) into `verb repl`, assert stdout.
`repl_session` fixture pair.

**Risks**: (1) replay re-runs fn side effects only if a body prints at definition — it doesn't
(bodies run on call), safe. (2) definition w/ side effect (`assign x read_line();`) re-executes each
turn → document REPL history = pure definitions, or snapshot such values as literals. (3)
`exit(main_fn.call())` (`:241`) terminates process — REPL must NOT reuse; call without `exit`.

---

## Task D — More std modules: `std net` (UDP) first

**Files**: new `runtime/verb_std_net.cpp`; `src/parser.rs:157`; `src/codegen.rs` (arity table +
resolution arm); `src/main.rs` (new `compile_*_obj` + link wiring); `build.rs` (**not** needed — JIT
rejects imports); `tests/e2e.rs`.

**Approach (UDP concretely)**
1. `runtime/verb_std_net.cpp`: copy header/`verb_string_from` preamble from `verb_std_io.cpp:10-30`;
   add `extern "C" VerbValue udp_socket()`, `udp_bind(fd,port)`, `udp_send(fd,host,port,data)`,
   `udp_recv(fd)` reusing `verb_as_int`/`verb_string`.
2. Parser: extend known-set `src/parser.rs:157` `{io,map}` → add `net` (update error message).
3. Codegen: add `const NET_FUNCS: &[(&str,usize)]` beside `IO_FUNCS` (`:2114`) + `net_func_arity`;
   resolution arm at `:1972-1976` (`std_imports … == "net"`) → `gen_std_io_call` (name-generic, just
   declares extern of N VerbValue params, `:2021`).
4. `main.rs`: `compile_net_obj` (clone `compile_std_io_obj` `:278`, new `STD_NET_CPP` const `:271`);
   wire `wants_net` into `build_aot_host` (`:329-364`) + `build_aot_cross` (`:396-447`). Apply same
   Windows-cross guard as std io (`:397-403`) — Winsock differs.

**Larger sub-items (defer, descending effort)**: non-blocking/async sockets (event-loop + Verb-level
callback/poll = real language design), TLS (OpenSSL `-lssl` — feasible via `import mod` `-l` rather
than a std module), generic containers (`set`/`deque` via another `verb_std_*.cpp` following map).

**Deps**: none.

**Test**: loopback UDP fixture built with `verb build` (like TCP tests) + standalone
`c++ -std=c++17 -Iruntime -c runtime/verb_std_net.cpp` compile check + Windows cross smoke via
`zig c++ -target x86_64-windows-gnu` guarded on zig.

**Risks**: Windows socket ABI (`#ifdef _WIN32` + Winsock) — match std-io Windows-cross rejection to
bound v1.

---

## Task E — Typed extern signatures (FFI-V2-02) (medium effort, touches grammar)

Currently every `import mod` extern declared `VerbValue(VerbValue…)`, arity checked only vs prior
call site (`gen_extern_call` `codegen.rs:2062-2097`). Typed sigs add compile-time ABI checking +
native marshalling.

**Files**: `src/lexer.rs` (maybe new tokens), `src/parser.rs` (grammar + AST), `src/ast.rs` (node +
`Ty` enum), `src/codegen.rs` (typed fn-type + box/unbox), `runtime/verb.h` (existing `wrap/unwrap`
templates `:84-96`), `tests/e2e.rs`.

**Approach**
1. Grammar: optional signature on `import_stmt` (`parser.rs:142`), e.g.
   `import mod mathlib exposing c_sqrt(float) -> float;` → per-name signature table.
2. AST: `enum Ty { Int, Float, Str, Bool }` + `ExternSig { name, params: Vec<Ty>, ret: Ty }`.
3. Codegen: in `gen_extern_call` (`:2062`) when a signature exists, build LLVM fn type from `Ty`
   (e.g. `double(double)`), **unbox** args + **rebox** result via existing tag ctors. C authors write
   natural `double c_sqrt(double)`. Enforce arity+arg-tag at compile where static; runtime tag-check
   otherwise (`abort_at` `:196`). Keep untyped path (`:2080-2096`) as default when no sig →
   backward compatible.

**Deps**: none, but most grammar-adjacent Tier-4 item → schedule after B/C/D.

**Test**: fixture linking a tiny C lib with natural `double`-typed fn under declared signature;
negative test asserting compile error on arity/type mismatch (extend `compile_err` `e2e.rs:19`).

**Risks**: keyword bikeshed (`exposing`), reserved-symbol footgun (`codegen.rs:2081-2090`).
`Str` marshalling crosses GC boundary (`verb.h:22-25`) — keep v1 scalar `Int/Float/Bool`.

---

## Task F — Debug info / DWARF (highest effort — blocked on missing spans)

**Lowest ROI** — AST lacks enough position data.

**Prerequisite F0 — thread spans**: add `line/col` (or `Span{line,col}` newtype) to every `Stmt` and
to literal `Expr` variants in `src/ast.rs`, populate in `src/parser.rs` from `Token.line/col`
(matching existing `Var/Call` pattern `ast.rs:14-17`). Mechanical but broad (most parser productions).
`cur_file` (`codegen.rs:29`) + `stmt_files` (`main.rs:204`) give the file dimension.

**DWARF proper**
1. Add `DebugInfoBuilder` (inkwell `inkwell::debug_info`, on `llvm20-1`) to `Codegen` (`:14-30`),
   created in `new` (`:33`) with a `DICompileUnit`.
2. `module.set_source_file_name` + `"Debug Info Version"`/`"Dwarf Version"` module flags (else LLVM
   strips DI).
3. `DISubprogram` per compiled Verb fn (attach in fn-lowering; `Fn` has `line` `ast.rs:29`);
   `DILocation`s per statement from F0 spans; `builder.set_current_debug_location(...)` before each stmt.
4. `finalize()` DIBuilder before `write_to_file` (`main.rs:326`,`419`).
5. Gate on `-g` CLI flag; at `-g` force `OptimizationLevel::None` (composes with Task B) so line
   tables stay usable.

**Deps**: **F0 is a hard prerequisite.** Composes with Task B.

**Test**: build fixture with `-g`, run `lldb`/`gdb` batch (`-batch -ex 'b main' -ex run -ex 'info line'`)
asserting a source line resolves (guard on debugger availability). Minimum: assert DWARF sections exist
(`llvm-dwarfdump`/`objdump --dwarf`).

**Risks**: breadth of F0 (every parser production); inkwell `debug_info` verbose + version-sensitive
(validate vs inkwell 0.9); closures/heap values need type metadata for var-level DWARF — scope v1 to
**line tables + function boundaries only** (no `DILocalVariable`) → enough for breakpoints/stepping.

---

## Recommended sequencing
1. **A (`verb targets`)** — trivial, ship first.
2. **B (optimizer / `-O`)** — high value, isolated.
3. **C (`verb repl`)** — high value, reuses JIT; after B so REPL inherits `-O`.
4. **D (`std net`/UDP)** — additive, proven pattern.
5. **E (typed externs)** — grammar work; after quick wins.
6. **F (DWARF)** — last; land **F0 span threading** first.

A, B, D, E, F0 mutually independent + parallelizable across worktrees. C depends only on B for the
`-O`-in-REPL nicety; F depends on F0.

## Critical files
- `src/main.rs` — CLI dispatch, JIT `run`, AOT emit, `-O`/`repl`/`targets` entry points
- `src/codegen.rs` — `Codegen` struct, `module()`, `optimize()` new method, extern/std-fn resolution,
  DIBuilder host
- `src/ast.rs` — span threading prerequisite (DWARF); typed-extern `Ty` node
- `src/parser.rs` — std-module name set `:157`, extern-signature grammar, span population
- `runtime/verb_std_io.cpp` — template for `runtime/verb_std_net.cpp`; ABI via `runtime/verb.h`
