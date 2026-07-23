//! Runtime value model: every Verb value is the LLVM struct { i8 tag, i64 payload }.
pub const TAG_NIL: u64 = 0;
pub const TAG_BOOL: u64 = 1;
pub const TAG_INT: u64 = 2;
pub const TAG_FLOAT: u64 = 3; // payload = f64 bits
pub const TAG_STR: u64 = 4;   // payload = ptr to NUL-terminated bytes
pub const TAG_CLOSURE: u64 = 5; // payload = ptr to { fn_ptr, i64 arity, env_ptr }
pub const TAG_MAP: u64 = 6;     // payload = ptr to a runtime/verb_map.cpp VerbMapImpl
pub const TAG_ARRAY: u64 = 7;   // payload = ptr to { i64 len, i64 cap, ptr elems }
pub const TAG_STRUCT: u64 = 8;  // payload = ptr to { ptr descriptor, i64 nfields, [nfields x value] }
pub const TAG_ENUM: u64 = 9;    // payload = ptr to { ptr descriptor, i64 variant_id, i64 nfields, [nfields x value] }

/// Refcount-header value that marks a string as static (a source literal,
/// never heap-allocated, never freed). Never a value a real refcount can
/// reach from 1 by increment/decrement in any real program.
pub const GC_STATIC_SENTINEL: i64 = i64::MIN;
