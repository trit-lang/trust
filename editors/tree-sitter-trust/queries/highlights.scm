; Highlighting for Trust.
;
; Later patterns win, so the order here is general first and specific after:
; an identifier is a variable until something says it is a type or a call.

; ----------------------------------------------------------------- names

(identifier) @variable

(parameter name: (identifier) @variable.parameter)
(closure_parameter (identifier) @variable.parameter)
(self_parameter) @variable.special
(self_expression) @variable.special

(field_declaration name: (identifier) @property)
(field_expression field: (identifier) @property)
(field_initializer name: (identifier) @property)
(field_pattern (identifier) @property)
(associated_binding (identifier) @property)

(function_item name: (identifier) @function)
(call_expression function: (identifier) @function)
(method_call_expression method: (identifier) @function.method)

(struct_item name: (identifier) @type)
(enum_item name: (identifier) @type)
(trait_item name: (identifier) @type)
(type_parameter (identifier) @type)
(const_parameter (identifier) @constant)
(bound (identifier) @type)
(generic_type name: (identifier) @type)
(struct_expression name: (identifier) @constructor)
(variant name: (identifier) @constructor)
(attribute (identifier) @attribute)
(mod_item name: (identifier) @module)
(module_path (identifier) @module)

(const_item name: (identifier) @constant)

(lifetime) @label

; A type position is a type even when it is a bare name.
(parameter type: (identifier) @type)
(field_declaration type: (identifier) @type)
(function_item return_type: (identifier) @type)
(reference_type type: (identifier) @type)
(impl_item trait: (identifier) @type)
(impl_item type: (identifier) @type)
(qualified_type (identifier) @type)
(dynamic_type (identifier) @type)
(impl_fn_type trait: (identifier) @type)

(primitive_type) @type.builtin
(self_type) @type.builtin
(never_type) @type.builtin

; ------------------------------------------------------------- literals

(integer_literal) @number
(boolean_literal) @constant.builtin
(char_literal) @string.special.symbol
(string_literal) @string
(line_comment) @comment
(block_comment) @comment

; ------------------------------------------------------------- keywords

[
  "as"
  "break"
  "const"
  "dyn"
  "else"
  "enum"
  "fn"
  "for"
  "if"
  "impl"
  "in"
  "let"
  "loop"
  "match"
  "mod"
  "mut"
  "pub"
  "return"
  "struct"
  "trait"
  "type"
  "use"
  "where"
  "while"
] @keyword

; `continue` is a whole expression on its own, so the keyword is the node.
(continue_expression) @keyword

; ------------------------------------------------------- operators, marks

[
  "+"
  "-"
  "*"
  "/"
  "%"
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "<=>"
  "&&"
  "||"
  "!"
  "&"
  "<<"
  ">>"
  "="
  "+="
  "-="
  "*="
  "/="
  "%="
  "<<="
  ">>="
  "->"
  "=>"
  ".."
  "@"
  "?"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "::"
  "."
] @punctuation.delimiter

(wildcard_pattern) @variable.special
