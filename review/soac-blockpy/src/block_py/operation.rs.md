# soac-blockpy/src/block_py/operation.rs

## File Responsibilities

Defines BlockPy operation payload structs and operator enums. The file separates generic lowered
operations (`Call`, `Load`, `Store`, `MakeFunction`, etc.) from Ruff-shaped AST operations that are
kept during early lowering stages.

## Datatypes

- `BinOpKind`, `UnaryOpKind`: normalized binary/unary operation kinds, including comparisons,
  membership/identity, truthiness, and inplace variants.
- `BinOp<E>`, `UnaryOp<E>`: generic lowered operator operations.
- `Call<E>`: normal Python call with function expression, positional args, and keyword args.
- `CalleeFunctionId<E>`: operation to observe/extract a callee's function id.
- `CallDirect<E>`: direct-call IR with callable expression, expected `FunctionId`, and arguments.
- `GetAttr<E>`, `SetAttr<E>`, `GetItem<E>`, `SetItem<E>`, `DelItem<E>`: attribute and item
  operations.
- `Load<I>`, `Store<I>`, `Del<I>`: name/location load, store, and delete operations.
- `MakeCell<E>`, `CellRefForName`, `CellRef`: closure-cell construction/reference operations.
- `MakeFunction<E>`: lowered function object construction payload.
- `Await<E>`, `Yield<E>`, `YieldFrom<E>`: async/generator operations.
- `Expr*` structs: Ruff-shaped expression payloads retained in early IR, including bool ops,
  comprehensions, literals, attributes, subscripts, names, tuples/lists, slices, and IPython escape.
- `Stmt*` structs: Ruff-shaped statement payloads retained in early IR, including definitions,
  assignments, loops, match, try, imports, globals/nonlocals, and simple control statements.

## Functions

- `BinOpKind::from_ast_operator`, `from_ast_inplace_operator`, `into_ast_operator`: convert between
  Ruff operators and normalized binary operation kinds.
- `UnaryOpKind::from_ast_unary_op`, `into_ast_unary_op`: convert between Ruff unary operators and
  normalized unary operation kinds.
- `Call::new`: constructs a generic call; custom impls provide debug formatting, metadata,
  child-visiting, and child-mapping.
- `CallDirect::new`: constructs a direct-call operation; custom impls provide debug formatting,
  metadata, child-visiting, and child-mapping.
- `Load::new`, `Store::new`, `Del::new`: construct name operations; custom impls handle metadata,
  debug rendering, child traversal, and name/instruction mapping.
- `MakeFunction::function_id`, `set_function_id`: inspect and update the target function id.
- Macro-generated operation methods: all `define_operation!` / `define_ruff_operation!` payloads
  receive `new`, `meta`, `with_meta`, child traversal, and child mapping behavior.

## Context Read

- `soac-blockpy/src/block_py/operation_macro.rs`
- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/map.rs`
- `soac-blockpy/src/block_py/visit.rs`
