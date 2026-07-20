; Verb highlight queries — capture names follow the nvim-treesitter
; standard groups (see :h treesitter-highlight-groups).

; ----- literals -----

(int) @number
(float) @number.float
(string) @string
(escape_sequence) @string.escape
(true) @boolean
(false) @boolean
(nil) @constant.builtin

; ----- comments -----

(line_comment) @comment
(block_comment) @comment

; ----- statement keywords -----

"assign" @keyword
"declare" @keyword
"be" @keyword.operator
"make" @keyword.function
"return" @keyword.return
"check" @keyword.conditional
"orelse" @keyword.conditional
"repeat" @keyword.repeat
"loop" @keyword.repeat
"begin" @keyword
"end" @keyword

; ----- expression operators -----

["add" "sub" "times" "div" "mod"] @operator
["equals" "differs" "trails" "beats" "atmost" "atleast"] @operator
["and" "or" "not" "neg" "join"] @operator

; ----- functions -----

(fn_statement name: (identifier) @function)
(call_expression function: (identifier) @function.call)
(parameters (identifier) @variable.parameter)

; ----- variables -----

(assign_statement name: (identifier) @variable)
(declare_statement name: (identifier) @variable)
(reassign_statement name: (identifier) @variable)
(identifier) @variable

; ----- punctuation -----

["(" ")"] @punctuation.bracket
[";" ","] @punctuation.delimiter
