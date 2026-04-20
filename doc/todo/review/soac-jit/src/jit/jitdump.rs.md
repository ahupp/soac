# soac-jit/src/jit/jitdump.rs

## File Responsibilities

Emits Linux perf-compatible JIT dump files for generated code. On Linux it records jitdump headers, code-load records, raw
machine code bytes, and optional System V unwind info. On non-Linux targets it provides a no-op `record_code_load` shim.

## Datatypes

- `JITDUMP_MAGIC`, `JITDUMP_VERSION`, `PERF_JIT_CODE_LOAD`, `PERF_JIT_CODE_UNWINDING_INFO`: jitdump format constants.
- `DWARF_*`: encodings used in generated `.eh_frame_hdr` records.
- `Header`, `BaseEvent`, `CodeLoadEvent`, `CodeUnwindingInfoEvent`, `EhFrameHeader`: C-layout records written to the
  jitdump file.
- `SerializedUnwindInfo`: serialized `.eh_frame` bytes plus the generated header.
- `JitDumpSession`: owns the jitdump file, marker mapping, and monotonically increasing code ids.
- `JITDUMP_SESSION`: process-wide lazily initialized session protected by a mutex.

## Functions

- `JitDumpSession::new`, `new_in_dir`: create the jitdump file, marker mmap, and initial header.
- `JitDumpSession::record_code_load`: optionally records unwind info, then writes a code-load event, symbol name, and code
  bytes.
- `JitDumpSession::record_serialized_unwind_info`: writes a perf unwind-info event plus `.eh_frame` payload and padding.
- `JitDumpSession::drop`: flushes the file and unmaps the marker page.
- `record_code_load` on Linux: public entry that initializes/locks the singleton and delegates to the session.
- `record_code_load` on non-Linux: no-op compatibility entry.
- `serialize_unwind_info`: converts Cranelift System V unwind info into jitdump unwind records.
- `dwarf_record_size`, `checked_i32`, `round_up`: validate and shape encoded DWARF/jitdump fields.
- `current_monotonic_ticks`, `current_time_microseconds`, `current_thread_id`, `elf_machine_architecture`: collect host
  metadata for jitdump records.
- `write_plain`: writes C-layout records as raw bytes.

Tests validate header/code-load serialization and unwind-info ordering in a temporary jitdump file.

## Context Read

- `soac-jit/src/jit/mod.rs`: calls `record_code_load` after generated code is compiled/finalized.
- `crate::config::soac_work_dir_from_env`: chooses the jitdump output directory.
- Cranelift `TargetIsa` and System V unwind info APIs: source of codegen unwind metadata.
