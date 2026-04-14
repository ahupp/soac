# soac-blockpy/src/passes/instr_id.rs

## File Responsibilities
Assigns stable per-function/per-block instruction ids for codegen-stage instructions and validates that identified modules have complete, unique semantic instruction keys.

## Datatypes
- `BlockInstrIdAssigner`: visitor state for assigning monotonically increasing ids within one block.
- `CodegenInstrIdValidator`: visitor that checks id presence and duplicate `InstrKey`s within one function.

## Functions
- `BlockInstrIdAssigner::assign`: attaches the next `InstrId` to an instruction's metadata.
- `BlockInstrIdAssigner::visit_instr_mut`: assigns ids in traversal order.
- `assign_function_instr_ids`: assigns ids to every instruction in one unidentified codegen function.
- `into_identified_function`: converts an unidentified codegen function to the identified shape.
- `assign_module_instr_ids`: converts a whole unidentified module to `CodegenModuleShape`.
- `CodegenInstrIdValidator::validate_function`: validates one identified function.
- `CodegenInstrIdValidator::visit_instr`: records ids, allows synthetic counter instructions without ids, and reports missing/duplicate ids.
- `validate_codegen_instr_ids`: validates every function in a codegen module.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for identified/unidentified module shapes.
- `soac-blockpy/src/block_py/meta.rs` for instruction metadata and ids.
