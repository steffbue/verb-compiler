; Verb scopes for nvim-treesitter's locals module (variable-reference
; highlighting, "go to definition"-style features).

[
  (block)
  (for_statement)
  (foreach_statement)
] @local.scope

(fn_statement name: (identifier) @local.definition.function)
(parameters (identifier) @local.definition.parameter)
(assign_statement name: (identifier) @local.definition.var)
(declare_statement name: (identifier) @local.definition.var)
(foreach_statement variable: (identifier) @local.definition.var)

(identifier) @local.reference
