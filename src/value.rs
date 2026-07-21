//! Runtime value model: every Verb value is the LLVM struct { i8 tag, i64 payload }.
pub const TAG_NIL: u64 = 0;
pub const TAG_BOOL: u64 = 1;
pub const TAG_INT: u64 = 2;
pub const TAG_FLOAT: u64 = 3; // payload = f64 bits
pub const TAG_STR: u64 = 4;   // payload = ptr to NUL-terminated bytes
pub const TAG_CLOSURE: u64 = 5; // payload = ptr to { fn_ptr, i64 arity, env_ptr }
pub const TAG_MAP: u64 = 6;     // payload = ptr to a runtime/verb_map.cpp VerbMapImpl
pub const TAG_ARRAY: u64 = 7;   // payload = ptr to { i64 len, i64 cap, ptr elems }
