# soac-blockpy/src/passes/mod.rs

## File Responsibilities
Defines the central instruction enums and module-shape types for every major compiler pipeline stage. It also provides typed-instruction adapters, legacy typed/codegen conversion, and block relabeling helpers.

## Datatypes
- `InstrRuff`: source-shaped instruction enum mirroring Ruff AST expression and statement variants.
- `RuffModuleShape`: module shape whose instruction type is `InstrRuff` with unresolved names.
- `InstrCodegenOp` / `InstrCodegen`: codegen-ready operation enum with resolved names and serializable payloads.
- `TypedTruthy`: typed wrapper for expressions already lowered to truthiness-compatible values.
- `InstrTyped` / `InstrTypedCodegen`: transitional typed instruction enum, containing typed operations plus legacy codegen operations.
- `CodegenToTyped` and `TypedToCodegen`: mappers between legacy codegen and typed instruction shapes.
- `InstrWithAwaitAndYield`, `InstrWithYield`, `InstrLow<N>`, `InstrUnresolved`, and `InstrResolved`: intermediate instruction enums for progressively lowered pipeline stages.
- Module shape structs: `CoreModuleShapeWithAwaitAndYield`, `CoreModuleShapeWithYield`, `CoreModuleShape`, `ResolvedStorageModuleShape`, `CodegenModuleShape`, `TypedCodegenModuleShape`, and `CodegenUnidentifiedModuleShape`.

## Functions
- `TypedTruthy::new`, `value`, and `into_value`: construct and access typed truthiness operands.
- Trait implementations for instruction enums provide `Instr`, `InstrWithConstantNone`, metadata, child traversal, and mapping behavior.
- `InstrTyped::is_legacy`: identifies typed instructions that still wrap legacy codegen operations.
- `lower_codegen_module_to_typed` / `lower_codegen_function_to_typed`: wrap codegen modules/functions in typed shape.
- `lower_typed_function_if_tests_to_truthy` / `lower_typed_if_tests_to_truthy`: rewrite branch conditions to typed truthiness nodes.
- `try_lower_typed_instr_to_codegen_legacy`, `try_lower_typed_term_to_codegen_legacy`, and `try_lower_typed_module_to_codegen_legacy`: lower typed IR back to legacy codegen where possible.
- `relabel_dense_bb_module`: relabels basic blocks densely across all functions.

## Context Read
- `soac-blockpy/src/block_py/mod.rs` and operation payload modules for all IR payload definitions.
- `value_facts.rs` and codegen consumers for typed-instruction use.
