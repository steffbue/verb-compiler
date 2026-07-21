# Codebase Concerns

**Analysis Date:** 2026-07-21

## Tech Debt

**Monolithic Codegen Module:**
- Issue: `src/codegen.rs` is 2283 lines, containing all LLVM IR generation, helper function builders, and test cases
- Files: `src/codegen.rs`
- Impact: Difficult to navigate and modify; changes risk breaking multiple subsystems; new contributors have steep learning curve
- Fix approach: Extract helper function builders (verb_alloc, retain/release, array ops, etc.) into separate modules; separate test cases; split by concern (memory management, arrays, operators, etc.)

**High Panic Risk in Codegen:**
- Issue: 552 unwrap()/panic!/expect() calls in `src/codegen.rs` (26 unsafe blocks, 60 unreachable!() calls)
- Files: `src/codegen.rs` (lines 100+), `src/main.rs` (line 240 expect "no main")
- Impact: LLVM API changes, unexpected None returns from LLVM functions, or runtime assertion failures could crash the compiler
- Fix approach: Replace unwrap() calls on LLVM builder operations with proper error propagation; use Option::ok_or with descriptive errors; audit LLVM FFI assumptions

**Unsafe GEP Operations Without Full Safety Justification:**
- Issue: 6 unsafe blocks using `build_in_bounds_gep()` for array access; safety depends on `verb_array_check()` actually validating bounds correctly
- Files: `src/codegen.rs` (lines 117, 179, 205, 256, 418, 961, 993, 1066, 1094, 1144, 1328, 1716, 1810, 1992, 2188)
- Impact: If index validation is bypassed or malformed, unsafe GEP could access memory outside array bounds; bounds checks use signed comparison (SLT/SGE) but GEP index is constructed from payload without explicit casting verification
- Fix approach: Add compile-time assertions that `verb_array_check` always precedes GEP use; document invariant that returned index is always < array length; consider adding debug assertions

**LLVM get_nth_param().unwrap() Pattern:**
- Issue: 55 calls to `f.get_nth_param(n).unwrap()` throughout codegen assume LLVM always returns Some for valid function params
- Files: `src/codegen.rs` (lines 730, 731, 732, 750-953, 981-985, etc.)
- Impact: If LLVM API contract changes or parameter indices are miscalculated, unwrap() panics
- Fix approach: Wrap all parameter extraction in a checked helper that returns Err if param is missing; verify parameter count matches function signature

## Known Bugs

**Integer Overflow in Array Capacity Doubling:**
- Symptoms: Extremely large arrays (capacity > i64::MAX/2) would overflow when doubled
- Files: `src/codegen.rs` line 1048
- Trigger: Create array with 2^62 or more elements, then push to trigger growth
- Workaround: Cap maximum array capacity below i64::MAX/2 in verb_array_push
- Fix approach: Check if `cap > i64::MAX/2` before doubling; return OOM error or use saturating arithmetic

**Integer Overflow in String Concatenation:**
- Symptoms: Concatenating two very large strings (total length > i64::MAX) would overflow size calculation
- Files: `src/codegen.rs` lines 742-745 (concat function)
- Trigger: Concatenate strings where `strlen(a) + strlen(b) + 1 > i64::MAX`
- Workaround: Limit string sizes in practice
- Fix approach: Use checked arithmetic or saturating add for size calculation; abort if result overflows

**Missing Arithmetic Overflow Checks:**
- Symptoms: Binary operations (Add, Mul) on int64 values can silently overflow without detection
- Files: `src/codegen.rs` lines 548, 550 (BinOp::Add/Mul codegen)
- Trigger: `9223372036854775807 add 1` (i64::MAX + 1)
- Workaround: None; users must stay within safe ranges
- Fix approach: Use LLVM `@llvm.sadd.with.overflow` or `@llvm.umul.with.overflow` intrinsics to detect overflow and abort with error message

## Security Considerations

**Unsafe C String Functions (strcpy/strcat):**
- Risk: strcpy/strcat are known to be buffer-unsafe if bounds aren't carefully managed
- Files: `src/codegen.rs` lines 88-89 (declaration), 747-748 (usage in concat)
- Current mitigation: Buffer is pre-allocated with correct size via `verb_alloc(size)` before strcpy/strcat
- Recommendations: Add compile-time verification that buffer allocation precedes strcpy/strcat; consider using `strncat`/`strncpy` as defense-in-depth; document the invariant in a comment

**expect() in LSP JSON Serialization:**
- Risk: JSON serialization could theoretically fail if VerbValue contains invalid data; expect() would panic and crash the LSP server
- Files: `src/bin/verb-lsp.rs` line 162
- Current mitigation: JSON serialization of simple structures rarely fails
- Recommendations: Wrap in map_err() to return diagnostic error instead of panicking; add test case for edge case values

**Refcount Header Format Not Validated:**
- Risk: Extern C functions must allocate through verb_alloc() to get refcount header; if C++ code uses malloc() instead, verb_retain_value/verb_release_value will read garbage as refcount
- Files: `runtime/verb.h` line 20-25 (documentation), `src/codegen.rs` line 158 (comment about header)
- Current mitigation: Documentation warning in verb.h; no runtime check
- Recommendations: Add magic number or tag after refcount to detect malloc'd pointers; add defensive check in verb_release_value

## Performance Bottlenecks

**Linear Symbol Lookup in Scopes:**
- Problem: Variable resolution uses Vec of HashMaps, requiring O(scope_depth) lookups
- Files: `src/codegen.rs` lines 22, 1402+ (scope stack used in codegen)
- Cause: Each scope is a separate HashMap; lookup walks the scope stack linearly
- Improvement path: Flatten scope stack into single flat symbol table with scope level metadata; use BTreeMap for range queries

**Monolithic AST Visitor Pattern:**
- Problem: Large match statements in compile_expr/compile_stmt walk entire AST without early termination
- Files: `src/codegen.rs` (compile_expr, compile_stmt methods)
- Cause: Exhaustive pattern matching on all possible AST nodes
- Improvement path: Consider visitor pattern with early return; profile to identify hot paths

**String Concatenation in Join Operation:**
- Problem: Uses strlen+strcpy for each concat, which scans the existing string repeatedly
- Files: `src/codegen.rs` lines 742-748
- Cause: Each strcat rescans from the start
- Improvement path: Use pointer arithmetic or single memcpy; build offsets during the copy

## Fragile Areas

**GC Refcount Accounting:**
- Files: `src/codegen.rs` lines 97-130 (gc_live global), 2180-2280 (tests)
- Why fragile: Correct refcount accounting requires every retain to be paired with a release; calls to abort_at() bypass cleanup
- Safe modification: When adding new code paths that create values, ensure verb_release_value is called on error paths; use RAII-like patterns where possible
- Test coverage: Tests in e2e.rs check zero leaks but only for successful execution; no test for error path cleanup

**Unsafe Blocks in Array Operations:**
- Files: `src/codegen.rs` (lines 961, 993, 1066, 1067, 1094, 1144, 1328, 1810)
- Why fragile: Safety of in_bounds_gep depends on index validation being correct; if verb_array_check is wrong or bypassed, UB occurs
- Safe modification: Any change to verb_array_check must be verified against all GEP call sites; add test cases for boundary conditions (0, 1, len-1, len)
- Test coverage: Existing tests cover basic operations but not edge cases with max-size arrays

**Closure Value Layout Across C++ Boundary:**
- Files: `runtime/verb.h` (line 8 comment), `src/codegen.rs` (closure_ty definition)
- Why fragile: Closures never cross C++ boundary but are passed by value internally; any change to closure struct layout requires rebuilding Verb runtime
- Safe modification: Closure struct is defined once in both codegen.rs and verb.h; keep them in sync; add static_assert checking size if possible
- Test coverage: Tests don't cover interop at this level

## Scaling Limits

**Array Capacity Near i64::MAX:**
- Current capacity: 2^63-1 elements theoretical maximum
- Limit: Can allocate at most 2^60 elements before capacity doubling would overflow; current allocation strategy has no overflow check
- Scaling path: Add check in verb_array_push; return OOM error when cap > safe threshold; consider using u64 for capacity internally (larger safe range)

**String Concatenation Size Limits:**
- Current capacity: Strings use strlen() for length; max addressable string is 2^63-1 bytes
- Limit: Concatenating two large strings can overflow i64 size calculation; no bounds check on individual string size
- Scaling path: Add maximum string length constant (e.g., 1GB limit); check in concatenation and file_read operations

**Heap Allocation Without OOM Handling:**
- Current capacity: verb_alloc calls malloc() which can fail; no check for null return
- Limit: Program will segfault if malloc returns NULL on allocation failure
- Scaling path: Check malloc result for NULL; call custom OOM handler that calls exit(1) with message; add test case for allocation failure (mocked)

## Missing Critical Features

**No Overflow Detection on Arithmetic:**
- Problem: Integer arithmetic (add, multiply) can silently overflow
- Blocks: Safe arithmetic operations; reliable computation of large numbers

**No String Escape Sequence Validation:**
- Problem: Lexer doesn't validate escape sequences; malformed escapes could cause issues in output
- Blocks: Portable string handling; cross-platform string literals

**No Max Iteration Limit on Parser Recovery:**
- Problem: parse_recovering could theoretically infinite loop on pathological input
- Files: `src/parser.rs` line 25 (parse_recovering function)
- Blocks: Guarantees parser terminates in reasonable time

## Test Coverage Gaps

**Error Path Cleanup:**
- What's not tested: When operations abort (type errors, bounds errors), refcount cleanup
- Files: `src/codegen.rs` (abort_at calls), `tests/e2e.rs` (e2e tests only check successful execution)
- Risk: Memory accounting could be wrong on errors; GC test with VERB_GC_DEBUG only checks successful runs
- Priority: High (correctness of GC system depends on this)

**Array Operations with Capacity Near Boundary:**
- What's not tested: Arrays pushed until capacity is at safe limits (2^62); capacity doubling overflow scenario
- Files: `tests/e2e.rs` (gc_arrays_regrow test only checks small arrays)
- Risk: Overflow on very large array operations
- Priority: Medium (unlikely in practice but safety-critical)

**LSP Implementation Edge Cases:**
- What's not tested: Malformed JSON input, very large documents, concurrent requests, network errors
- Files: `src/bin/verb-lsp.rs` (no automated tests)
- Risk: LSP server crashes on unexpected input; memory exhaustion on large files
- Priority: Medium (affects user experience but not correctness of compiled code)

**Unsafe Block Invariants:**
- What's not tested: All unsafe blocks assume certain invariants (valid pointers, correct struct layout, index in bounds)
- Files: `src/codegen.rs` (16 unsafe blocks)
- Risk: Undefined behavior if invariants are violated
- Priority: High (safety-critical)

**Cross-Module FFI:**
- What's not tested: Extern C functions defined by users returning heap values without proper allocation
- Files: Integration tests with mathlib, no test for mallocked pointer passed to Verb
- Risk: Verb code would read incorrect refcount, releasing invalid memory
- Priority: Medium (documentation exists but runtime safety would help)

---

*Concerns audit: 2026-07-21*
