# soac-blockpy/src/block_py/mod.rs

## File Responsibilities

Primary BlockPy IR facade and core structural model. It re-exports operation payloads, literal
types, metadata, mapping/visiting helpers, scope data, counters, instruction enums from passes, and
defines names, locations, functions, blocks, terminators, edges, and call argument containers shared
across lowering stages.

## Datatypes

- `LocalLocation`, `GlobalSlot`, `CellLocation`, `NameLocation`: physical or logical storage
  locations assigned by name binding and later passes.
- `NameLike`: abstraction over unresolved/resolved name representations.
- `Instr`, `InstrWithConstantNone`: traits implemented by BlockPy instruction enums.
- `BlockPyName`: owned source/runtime identifier string.
- `UnresolvedName`: source name or explicit runtime name before name binding.
- `ResolvedName`: name plus resolved `NameLocation`.
- `FunctionKind`: normal, coroutine, generator, or async-generator function kind.
- `Block<I>`: basic block with label, instruction body, terminator, block parameters, and optional
  exception edge.
- `BlockPyModule<P>`: whole lowered module for a `ModuleShape`, including globals, callable defs,
  constants, counters, and module id generator.
- `CallArgPositional<E>`: positional or starred call argument.
- `KeywordName`: owned keyword name.
- `CallArgKeyword<E>`: named keyword or `**kwargs` call argument.
- `FunctionName`: binding/display/qualified names for a lowered callable.
- `BlockPyFunction<P>`: lowered callable definition and its scope/storage metadata.
- `ModuleShape`: type-level marker for the instruction enum used at a pipeline stage.
- `ResolvedStorageBlock`, `CodegenBlock`: aliases for common block stages.
- `BlockBuilder<I>`: helper for constructing block bodies and terminators.
- `BlockTerm<I>`: jump, if, branch table, raise, or return terminator.
- `TermIf`, `TermBranchTable`, `TermRaise`: structured terminator payloads.
- `BlockEdge`: target label plus edge arguments.
- `BlockArg`: explicit edge argument source.
- `AbruptKind`: encoded nonlocal control-flow reason.
- `BlockParamRole`, `BlockParam`: special block parameter roles and names.

## Functions

- `is_internal_symbol`: recognizes SOAC-internal names.
- `LocalLocation::slot`, `GlobalSlot::slot`: expose numeric slots.
- `CellLocation::slot`, `is_owned`, `is_closure`, `is_captured_source`: classify cell slots.
- `NameLocation` constructors/accessors/classifiers: build and inspect local/global/runtime/cell/
  constant locations and render a location-specific id.
- `NameLike::pretty_id`, `is_runtime_name`, `is_runtime_symbol`: common name rendering and runtime
  symbol checks.
- `BlockPyName::new`, `as_str`, `into_ast_name`: construct and convert names.
- `UnresolvedName::name`: converts unresolved/runtime names back to Ruff names.
- `ResolvedName::with_location`, `local_location`, `cell_location`, `resolved_pretty_id`,
  `is_runtime_name`: update and inspect resolved names.
- `Block::new`, `from_builder`: construct blocks directly or from `BlockBuilder`.
- `Block::label_str`, `ensure_param`, `set_exception_param`, `exception_param`, `param_names`,
  `param_name_vec`, `bb_params`, `bb_param_names`, `replace_fallthrough_target`: block metadata,
  special params, and fallthrough retargeting helpers.
- `core_call_expr_with_meta`, `core_runtime_name_expr_with_meta`, `runtime_name_load`,
  `core_runtime_named_call_expr_with_meta`, `core_runtime_positional_call_expr_with_meta`: construct
  common runtime-name calls/loads with metadata.
- `CallArgPositional` and `CallArgKeyword` methods: lower from Ruff args, access child exprs, and
  map child instruction types.
- `KeywordName::new`, `as_str`, `into_ast_identifier`: construct and convert keyword names.
- `FunctionName::new`: constructs function naming metadata.
- `BlockPyFunction::clone`: clones function state while sharing the name generator state.
- `BlockPyFunction::lowered_kind`, `storage_layout`, `entry_block`: inspect function metadata.
- `BlockBuilder::new`, `from_stmts`, `with_term`, `push_stmt`, `extend`, `set_term`, `finish`,
  `jump`, `ensure_fallthrough_term`: build block contents safely.
- `BlockTerm::jump_term`, `implicit_function_return`, `replace_target`: construct and retarget
  terminators.
- `BlockEdge::new`, `with_args`: construct edges.

## Context Read

- `soac-blockpy/src/block_py/operation.rs`
- `soac-blockpy/src/block_py/name_gen.rs`
- `soac-blockpy/src/block_py/scope.rs`
- `soac-blockpy/src/block_py/map.rs`
- `soac-blockpy/src/block_py/visit.rs`
