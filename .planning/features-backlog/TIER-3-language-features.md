# Tier 3 — Net-New Language Features Implementation Plan

> Line numbers verified against branch `refcounting-gc-v2`.
> Sequenced: structs first (unlocks heap-record + descriptor + GC-cascade pattern),
> then closures (independent, highest conceptual payoff), then enums/match, result
> errors, string methods.

---

## Grounding — conventions every feature must obey

- **Tagged value** `value_ty = { i8 tag, i64 payload }` (`src/codegen.rs:37`); built via
  `make_val(tag,payload)` (`:133`), decomposed `tag_of` (`:144`)/`payload_of` (`:148`).
  Tags in `src/value.rs:2-9`; **tags 0–7 taken, next free = 8.**
- **Heap alloc + GC header** — `verb_alloc(i64 n)` (`:246`) wraps `malloc`, prefixes an
  **8-byte refcount header = 1**. `malloc_bytes(n)` (`:264`). Real malloc addr = `p-8`
  (`header_ptr` `:115`); **`free(header_ptr(p))`, never `free(p)`** (`:1339-1345`).
- **Retain/release** — `verb_retain_value` (`:1154`) bumps header for STR/CLOSURE/ARRAY/MAP;
  `verb_release_value` (`:1220`) decrements, cascades at zero (array cascade `:1296-1364`).
  `Expr::Var` load **retains** on read (`:1774`); owned temporaries released after use (`:1880`).
- **Variable storage** — locals are 16-byte heap **cells** holding one `value_ty`, refcounted
  via `verb_retain_cell`/`verb_release_cell` (`:1395`,`:1413`); `scopes: Vec<HashMap<String,PointerValue>>`
  (`:22`). Top-level names are LLVM module globals via `global_slot` (`:1529`). `lookup` (`:1518`)
  walks scopes then globals; `bind` (`:1542`) creates a cell or writes a global.
- **Functions/closures** — every `fn` lowers to `value_ty (ptr env, ptr argv)` (`:1684`).
  `make_closure` (`:1443`) mallocs 24-byte `closure_ty = {ptr fn, i64 arity, ptr env}` (`:39`)
  and **hardcodes env = null** (`:1450`). Call path **already loads env from closure and passes
  it as arg 0** of the indirect call (`:2002-2008`) — plumbing exists; body never reads it,
  release never frees it.
- **No-capture enforced in one place**: `Stmt::Fn` does `std::mem::take(&mut self.scopes)` (`:1698`)
  before compiling body → nested `make` sees only its own frames + globals. `err_closure_no_capture*`
  fixtures depend on `lookup` failing → `undefined variable`.
- **Dispatch** — `gen_call` (`:1891`) special-cases builtins by name (print/len/get/set/push/pop
  `:1896-1965`), then std-io/std-map (`:1967-1976`), then externs (`:1977`), then generic closure
  call (`:1981`). New builtin = another `if name == …` arm.
- **Print** — `verb_print_value` tag switch (`:324-437`); every new tag needs a branch.
  Keep 4 switches in lockstep: `verb_print_value` (`:324`), `verb_type_name` (`:214`),
  `verb_retain_value` (`:1154`), `verb_release_value` (`:1220`).
- **Syntax surface** — keyword→token `src/lexer.rs:160-169`; `TokenKind` `:4-9`; statement
  dispatch `parser.rs:182-207`; expression chain `or_expr→…→call→primary` `parser.rs:315-457`.
  **No `.` token / member operator today** — field access is net-new.

---

## Feature 1 — Structs / records (do first)

### Syntax (word-based, no new punctuation)
```
record Point begin x, y end     %% declaration
assign p Point(3, 4);           %% construction (constructor call)
print(x of p);                  %% field get:  <field> of <expr>
x of p be 10;                   %% field set:  <field> of <expr> be <value>;
```
`record` + `of` new keywords (`lexer.rs:160-169` / `TokenKind` `:6`).

### AST (`src/ast.rs`)
- `Stmt::Record { name, fields: Vec<String>, line, col }`
- `Expr::FieldGet { obj: Box<Expr>, field: String, line, col }`
- `Stmt::FieldSet { obj: Expr, field: String, value: Expr, line, col }`
- Construction reuses `Expr::Call` — resolved by name in codegen (like `print`).

### Parser (`src/parser.rs`)
- `statement()` (`:182`): `TokenKind::Record => record_stmt()` — name, `begin`, comma idents, `end`.
- Field-get postfix `of`: new `postfix` layer after `primary`, or handle in `primary`/`call`
  when `Ident` followed by `of` → parse `IDENT of EXPR` (field name = left operand).
- Field-set statement: when bare `Ident` followed by `of` → `FieldSet` (mirrors `Ident … Be`
  reassign detection `parser.rs:200`).

### Codegen (`src/codegen.rs`)
- New tag `TAG_STRUCT = 8` in `value.rs`.
- Heap layout `malloc_bytes(16 + n*16)` → `[ptr descriptor][i64 nfields][field0 value]…`
  (each field `value_ty` = 16B). Static **descriptor** global `{i8* type_name, i64 nfields,
  [i8* field_names]}` built once per `Stmt::Record` → runtime print/GC work generically.
- `Stmt::Record`: register `name → (descriptor global, field-index map)` in new
  `records: HashMap<String, RecordInfo>`. No IR except descriptor global.
- Construction in `gen_call` (`:1891`): before generic path, `if let Some(info)=self.records.get(name)`
  → eval args (arity-check vs field count), alloc, store descriptor+nfields+each field (fields
  moved in; `Expr::Var` retain balances; literals already owned). Return `make_val(TAG_STRUCT, ptr)`.
- `Expr::FieldGet`: eval obj (abort if tag≠STRUCT via `verb_check_struct` modeled on
  `verb_check_call` `:801`), compile-time field index from `records`, GEP `16 + idx*16`, load,
  **retain** (mirror `Expr::Var` `:1774`, `verb_array_get` `:965`), release obj temp.
- `Stmt::FieldSet`: eval obj+value; load old, `verb_release_value` old, store new,
  `verb_retain_value` new (array-set discipline `:996-999`).
- GC: `TAG_STRUCT` arm in retain (`:1190`, bump header) and release (`:1296`): read `nfields`
  at offset 8, loop-release fields (array cascade `:1321-1335`), `free(header_ptr(ptr))`.
  Only one block per struct → one `dec_live_counter` + one `free`.
- Print: `TAG_STRUCT` branch in `verb_print_value` (`:352`): read descriptor, print
  `Point{x: <v>, y: <v>}` recursing. Add name to `verb_type_name` (`:223`).

### Tests
`structs_basic.verb`, `structs_nested.verb` (struct field holding struct/array → GC cascade),
`gc_structs.verb` (reassign struct-holding global → zero leaks via `emit_gc_debug_dump` `:295`).
Errors: `err_struct_field_unknown.verb`, `err_struct_arity.verb`, `err_field_of_nonstruct.verb`.

### Risks / tension
- `of` = first member-access; watch precedence vs `call` so `get(x of p, 0)` parses intuitively.
- Descriptor = second static-global kind alongside strings; keeps runtime generic.
- Overlaps `map` std module — justify as typed positional field-named records vs dynamic maps.

---

## Feature 2 — Real closures (capture) (second)

### Key realization
Env-passing plumbing **already exists** (`:2002-2008`). Missing: (1) which vars to capture,
(2) alloc+populate env at creation, (3) read captures in body, (4) env GC.

### Recommendation — capture **by value** (snapshot)
At creation, snapshot each captured var's `value_ty` into the env block + `verb_retain_value`.
Inner mutations don't propagate outward. By-ref (shared cells) needs env to co-own cells /
extend lifetime past frame → more machinery + cycle risk; defer to v2. State the trade-off.

### Free-variable analysis
At `Stmt::Fn` (`:1681`), **before** `std::mem::take` (`:1698`), walk `body` collecting referenced
`Expr::Var` names minus locally-bound (params + Assign/Declare targets, respecting nested-fn
boundaries). For each free name, `self.lookup`: resolves in `self.scopes` (enclosing local) →
**capture**; resolves in `globals` → stays global (no capture). Small AST walk helper.

### Codegen
- Env layout `env_ty = { i64 n_captures, [n x value] inline }` via `malloc_bytes(8 + n*16)`.
  At creation (inside enclosing fn, captures live): load from cell, store env slot, `verb_retain_value`.
- `make_closure` (`:1443`): accept `env: PointerValue`, store at field 2 (`:1450`) instead of
  null. Empty-capture → null (non-capturing fns unchanged → most fixtures untouched).
- Body threading (after `:1701`): env param = `fnv.get_nth_param(0)` (currently ignored). For each
  capture: fresh local **cell** (like params `:1722`), init by loading env slot i + `verb_retain_value`,
  insert into scope. `lookup` now finds captures as ordinary locals — **`std::mem::take` (`:1698`)
  can stay**; captures re-materialized as locals preserves isolation invariant.
- **GC (critical fix)**: `verb_release_value` CLOSURE branch (`:1280-1294`) currently only frees the
  24-byte closure block, never env. Extend: at refcount zero, if `env != null` read `n_captures` at
  env offset 0, loop-release captured values, `free(header_ptr(env))`, then free closure block.
  Retain branch (`:1190`) already bumps closure header; env owned solely by closure → no separate retain.
- `self_clos` recursion cell (`:1709`) keeps working (env=null for self ref fine; body has captures
  as locals).

### Fixtures that must flip
`err_closure_no_capture.verb` + `err_closure_no_capture_param.verb` currently **assert rejection** →
become **passing** `closures_capture_local.verb` / `closures_capture_param.verb` (`.expected`:
`outer(5)`→6 and 5). Existing `closures.verb` (nested fn sees only globals) still passes.

### Tests
`closures_capture_local.verb`, `closures_capture_param.verb`, `closures_counter.verb` (returns a
closure capturing a param, called after `outer` returns → proves env outlives frame),
`gc_closures_capture.verb` (capture heap string/array, reassign → zero leaks). By-value test:
mutate captured var in inner, confirm outer unchanged.

### Risks / tension
- Env must outlive enclosing frame — by-value snapshot + refcounted env handles cleanly.
- Cycles (closure captures struct/array transitively holding the closure) leak — same accepted
  limitation as `gc_cyclic_array_leaks_confined`. Note, don't solve.
- Retain/release balance on env is the main hazard; array cascade `:1296-1364` is the template.

---

## Feature 3 — Enums + pattern matching (depends on struct machinery)

### Syntax
```
choice Shape begin
  Circle(r) or
  Square(s)
end
assign sh Circle(5);
match sh begin
  when Circle(r) begin print(r); end
  when Square(s) begin print(s); end
end
```
New keywords `choice`, `match`, `when` (`lexer.rs:160`); reuse `or` as variant separator.

### AST / parser
- `Stmt::Choice { name, variants: Vec<(String, Vec<String>)> }`
- `Stmt::Match { scrutinee: Expr, arms: Vec<MatchArm> }`, `MatchArm { variant, bindings, body }`
- Variant construction reuses `Expr::Call`. Parser `choice_stmt`/`match_stmt` reuse `block()`
  (`parser.rs:302`); `match` desugars like `if_stmt` (`:261`).

### Codegen
- Variant value = struct layout (Feature 1) + `variant_id`:
  `[ptr choice_descriptor][i64 variant_id][i64 nfields][fields…]`, tag `TAG_ENUM = 9` (or reuse
  TAG_STRUCT with variant_id in descriptor). Reuses struct alloc/GC/print wholesale — **why enums
  come after structs.**
- `Stmt::Match`: eval scrutinee, load variant_id, switch/if-chain over arms (model `verb_print_value`
  switch `:344` or `Stmt::If` chain `:1616`), each arm binds fields into a pushed scope (like params
  `:1715`) then runs body. Require `otherwise` arm or abort on no-match (`abort_at` `:196`).
- GC: identical cascade to structs.

### Risks
Exhaustiveness is the main semantic-check addition. v1: require `otherwise` or runtime abort.

---

## Feature 4 — Result-style error handling (depends on structs OR new tag)

### Recommended — Option A (leverages Feature 1)
Built-in `Err(kind, msg)` record + predicate builtins `is_err(v)` / `err_kind(v)` / `err_msg(v)`
as `gen_call` arms (`:1896`-style). `std io` C++ fns return an `Err` struct instead of nil on
failure. Reuses struct infra, no new tag, pattern-matches via Feature 3
(`match r begin when Ok(v)… when Err(k)… end`). `runtime/verb_std_io.cpp` changes to emit error struct.

(Option B — dedicated `TAG_ERR=10` + static-string payload — lighter but adds another tag to every
switch. Reject in favor of A.)

### Tests
`std_io_err_file_missing.verb` (read missing file → `Err`, `is_err` true, `err_msg` printed);
`match`-based dispatch fixture once Feature 3 lands.

### Risk
Changing std-io return contract touches `tests/fixtures/std_io_*` + `gc_std_io_*`. Keep success
paths returning plain value; only failure changes nil → `Err`.

---

## Feature 5 — String methods (independent, lowest coupling)

Strings = `TAG_STR`, payload = ptr to NUL-terminated bytes w/ refcount header. **Literals use
`GC_STATIC_SENTINEL`** (`value.rs:14`, `static_string_ptr` `:162`) → never freed. New strings from
methods are real `verb_alloc` blocks (refcount 1).

### API (builtins via `gen_call` arms, like `len`/`get`)
- `str_len(s)` → int (`strlen`, declared `:87`)
- `str_slice(s,start,end)` → new string (`verb_alloc` + copy; bounds → `abort_at`)
- `str_index(s,sub)` → int position or −1
- `str_split(s,sep)` → **array of strings** (alloc `array_ty` block `:41` + elems, each fresh
  `TAG_STR`; GC already cascades arrays of strings `:1296`)

### Codegen
Inline IR for trivial ones (`str_len`/`str_index`); C++ runtime helper `runtime/verb_str.cpp`
(mirror `verb_map.cpp`) for `str_split` (alloc-heavy, returns array). All allocations via
`verb_alloc` → automatic refcounting.

### Tests
`strings_methods.verb` (len/slice/index/split); `gc_string_split.verb` (split array reassigned/dropped
→ zero leaks); `err_str_slice_bounds.verb`.

### Risk
Slicing a literal must produce a **new heap** string (never alias literal buffer) or release frees a
static block. Follow `verb_concat` (`:716-750`) which correctly allocs a fresh result.

---

## Suggested sequencing
1. **Structs** — unlocks heap-record + descriptor + GC-cascade pattern.
2. **Closures** — independent of structs, can parallel; highest payoff, plumbing exists.
3. **Enums + match** — reuses struct machinery.
4. **Result errors** — builds on structs + match.
5. **String methods** — fully independent, land any time.

Each feature keeps the invariant that **codegen doubles as semantic checker** (arity/type/bounds
abort via `abort_at` `:196` or `CompileError`) and extends the 4 tag-switches in lockstep.

## Critical files
- `src/codegen.rs` — heap alloc, tag switches, `Stmt::Fn` `:1681`, `make_closure` `:1443`,
  `gen_call` `:1891`, GC branches `:1220`
- `src/ast.rs` — new Stmt/Expr nodes
- `src/parser.rs` — new statements/operators; `statement()` `:182`, expr chain `:315`
- `src/lexer.rs` — new keywords/tokens `:160`, `:4`
- `src/value.rs` — new tags `TAG_STRUCT`/`TAG_ENUM` (next free = 8)
