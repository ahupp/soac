# crates/soac_jit/src/counter_dump.rs

## File Responsibilities

Defines the binary counter-dump format used for specialization feedback. It encodes module-level profiling records with rkyv,
parses memory-mapped dump files, exposes zero-copy record views, and collects higher-level maps for call-target, operator,
branch, block-entry, module-key, and type-key specialization consumers.

## Datatypes

- `COUNTER_DUMP_MAGIC`, `COUNTER_DUMP_VERSION`, `COUNTER_DUMP_NONE_U64`, `COUNTER_DUMP_NONE_FUNCTION_ID`: on-disk format
  constants and sentinel values.
- `CounterDumpRow`: owned logical row for one counter observation or top-value observation.
- `CounterDumpRecord`: owned per-module dump record containing rows, key layouts, and type table data.
- `CounterDumpKeyLayout`: observed module/global key-to-index layout.
- `CounterDumpTypeKeyLayout`: observed split-dict owner type key-to-index layout.
- `CounterDumpTypeKey`: stable owner-type identity by module and qualname.
- `CounterDumpTypeTableEntry`: mapping from numeric profile type id to `CounterDumpTypeKey`.
- `CounterDumpFile`: memory-mapped dump file owner.
- `CounterDumpRecordView`, `CounterDumpRowView`, `CounterDumpKeyLayoutView`, `CounterDumpTypeKeyLayoutView`,
  `CounterDumpTypeTableEntryView`: zero-copy views over archived dump data.
- `CounterDumpRecordArchive`, `CounterDumpRowArchive`, `CounterDumpKeyLayoutArchive`,
  `CounterDumpTypeKeyLayoutArchive`, `CounterDumpTypeTableEntryArchive`: rkyv-serializable on-disk payload shapes.
- `CallTargetSpecializationEntry`: internal normalized call-target observation.
- `CollectedKeyLayout`, `CollectedTypeKeyLayout`: deduplicated collected layout entries for consumers.

## Functions

- `CounterDumpRecord::encode`: serializes one record into an aligned length-delimited frame.
- `CounterDumpRecordArchive::from_record`, `CounterDumpRowArchive::from_row`,
  `CounterDumpKeyLayoutArchive::from_key_layout`, `CounterDumpTypeKeyLayoutArchive::from_type_key_layout`,
  `CounterDumpTypeTableEntryArchive::from_entry`: convert owned logical records to archive payloads.
- `CounterDumpFile::open`: opens and memory maps a dump file.
- `CounterDumpFile::records`: parses all frames from the mapped dump file.
- `CounterDumpRecordView` accessors: expose source hash, module/package names, row counts, row views, key-layout views, and
  type-table views with bounds checking.
- `parse_counter_dump_records`: validates frame headers, version, length/alignment, and rkyv payloads, returning record views.
- `call_target_specialization_entries`: extracts raw call-target observations from dump rows.
- `observed_value_entries_for_kind`: extracts `(module, function, instr, observed_value)` rows for a given top-value counter
  kind.
- `render_call_target_specializations`: renders call-target observations into the semicolon/comma text format used by older
  configuration paths.
- `collect_call_target_specializations_for_function`: returns observed direct-call target function ids per instruction.
- `read_call_target_specializations_from_file`: file-reading wrapper for call-target collection.
- `collect_operator_specializations_for_function`: returns observed operator shape values per instruction.
- `collect_branch_preferences_for_function`: aggregates branch-outcome counts and chooses the hotter boolean direction.
- `parse_block_label_text`: parses textual `bbN` labels into `BlockLabel`.
- `collect_block_entry_counts_for_function`: aggregates block-entry counts by block label.
- `collect_module_key_layouts`: deduplicates and sorts observed module key layouts by owner.
- `collect_type_key_layouts`: deduplicates and sorts observed type key layouts by owner type id.
- `collect_type_table`: builds a type-id lookup table and rejects conflicting identities.
- `read_operator_specializations_from_file`, `read_branch_preferences_from_file`, `read_block_entry_counts_from_file`: file
  wrappers around the corresponding collectors.
- `align_up`: rounds frame sizes up to the dump alignment.
- `read_le_u16`, `read_le_u64`: bounds-checked little-endian scalar readers.
- `write_counter_dump_records`: writes one or more owned records to a new dump file.

## Context Read

- `crates/soac_jit/src/module_type.rs`
- `crates/soac_jit/src/jit/mod.rs`
- `soac-blockpy/src/block_py.rs`

