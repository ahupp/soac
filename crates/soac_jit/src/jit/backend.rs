use super::codegen_env::JitCodegenEnv;
use super::direct_abi::SOAC_JIT_RESUME_GENERATOR_SYMBOL;
use super::inspection::{
    ClifBlockRole, ClifBlockRoles, ClifFunctionDisplayAliases,
    annotate_clif_instruction_purpose_source_locs, clif_block_role_name_from_source_loc_bits,
    clif_purpose_name_from_source_loc_bits, clif_refcount_family_name_from_source_loc_bits,
};
#[cfg(test)]
use super::inspection::{
    RefcountFamily, clif_provenance_source_loc_bits, clif_purpose_source_loc_bits,
    refcount_family_source_loc_bits,
};
use super::runtime_support::{
    inline_runtime_support_calls, load_runtime_support_clif_with_debug_symbols,
};
use super::signal_diagnostics;
use super::specialized_helpers::register_specialized_jit_symbols;
use super::symbols::{
    CpythonTypeSymbol, cpython_type_symbol_name, lookup_registered_jit_data_symbol,
    py_dealloc_symbol,
};
use super::{
    _PyDict_IndexedValueTombstone, PyFunction_Type, PyList_Type, PyLong_Type, PyMethod_Type,
    PyTuple_Type, PyType_Type, PyUnicode_Type,
};
use crate::config::CraneliftTargetConfig;
use crate::function_instantiation::{
    SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL, soac_jit_make_function_with_closure,
};
use crate::soac_jit_resume_generator;
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_codegen::ir;
use cranelift_codegen::isa::TargetIsa;
use cranelift_control::ControlPlane;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{FuncId, Module, ModuleReloc};
use soac_config::SoacEnvConfig;
use soac_core::block_py::RuntimeFunctionId;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[cfg(not(test))]
const JIT_ARENA_BYTES: usize = 256 * 1024 * 1024;

// Unit tests create many short-lived JIT modules in one process. A production-sized
// arena churns enough virtual address space to make Cranelift's PC-relative
// relocations to runtime/helper symbols exceed i32 range on some hosts.
#[cfg(test)]
const JIT_ARENA_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn new_jit_builder(env_config: &SoacEnvConfig) -> Result<JITBuilder, String> {
    let isa = CraneliftTargetConfig::runtime(env_config).build_isa()?;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    if let Ok(provider) = ArenaMemoryProvider::new_with_size(JIT_ARENA_BYTES) {
        builder.memory_provider(Box::new(provider));
    }
    register_jit_builder_symbols(&mut builder);
    Ok(builder)
}

fn register_jit_builder_symbols(builder: &mut JITBuilder) {
    builder.symbol("_Py_Dealloc", py_dealloc_symbol());
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Function),
        std::ptr::addr_of_mut!(PyFunction_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Method),
        std::ptr::addr_of_mut!(PyMethod_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Type),
        std::ptr::addr_of_mut!(PyType_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Long),
        std::ptr::addr_of_mut!(PyLong_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::List),
        std::ptr::addr_of_mut!(PyList_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Tuple),
        std::ptr::addr_of_mut!(PyTuple_Type).cast::<u8>(),
    );
    builder.symbol(
        cpython_type_symbol_name(CpythonTypeSymbol::Unicode),
        std::ptr::addr_of_mut!(PyUnicode_Type).cast::<u8>(),
    );
    builder.symbol(
        "_PyDict_IndexedValueTombstone",
        std::ptr::addr_of_mut!(_PyDict_IndexedValueTombstone).cast::<u8>(),
    );
    builder.symbol(
        SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL,
        soac_jit_make_function_with_closure as *const u8,
    );
    builder.symbol(
        SOAC_JIT_RESUME_GENERATOR_SYMBOL,
        soac_jit_resume_generator as *const u8,
    );
    builder.symbol_lookup_fn(Box::new(lookup_registered_jit_data_symbol));
    register_specialized_jit_symbols(builder);
}

pub(super) fn new_jit_module(
    compile_session: &crate::session::CompileSession,
) -> Result<JITModule, String> {
    new_jit_module_with_runtime_support_symbols(compile_session).map(|(jit_module, _)| jit_module)
}

pub(super) fn new_jit_module_with_runtime_support_symbols(
    compile_session: &crate::session::CompileSession,
) -> Result<(JITModule, HashMap<u32, String>), String> {
    let env_config = compile_session.env_config()?;
    let mut jit_module = JITModule::new(new_jit_builder(env_config)?);
    let runtime_support_symbols =
        load_runtime_support_clif_with_debug_symbols(&mut jit_module, env_config)?;
    Ok((jit_module, runtime_support_symbols))
}

#[derive(Debug)]
pub(super) struct DefinedFunctionArtifact {
    pub(super) code_size: usize,
    pub(super) code_bb_offsets: Vec<usize>,
    pub(super) code_bb_edges: Vec<(usize, usize)>,
    pub(super) code_purpose_names: Vec<String>,
    pub(super) code_purpose_bytes: Vec<usize>,
    pub(super) code_refcount_family_names: Vec<String>,
    pub(super) code_refcount_family_bytes: Vec<usize>,
    pub(super) code_unattributed_bytes: usize,
    pub(super) code_block_role_names: Vec<String>,
    pub(super) code_block_role_attributed_bytes: Vec<usize>,
    pub(super) code_block_role_unattributed_bytes: Vec<usize>,
    pub(super) code_block_role_purpose_bytes: Vec<Vec<usize>>,
    pub(super) code_bb_block_role_names: Vec<String>,
    pub(super) code_bb_purpose_bytes: Vec<Vec<usize>>,
    pub(super) code_bb_unattributed_bytes: Vec<usize>,
    pub(super) systemv_unwind_info: Option<cranelift_codegen::isa::unwind::systemv::UnwindInfo>,
}

#[derive(Clone)]
pub(super) struct CompiledFunctionBytes {
    pub(super) code: Vec<u8>,
    pub(super) alignment: u64,
    pub(super) relocs: Vec<ModuleReloc>,
}

pub(super) struct CompiledFunctionArtifact {
    pub(super) bytes: CompiledFunctionBytes,
    pub(super) artifact: DefinedFunctionArtifact,
}

#[derive(Debug)]
struct TrivialJumpBlock {
    block: ir::Block,
    target: ir::Block,
    params: Vec<ir::Value>,
    jump_args: Vec<ir::BlockArg>,
    predecessors: Vec<TrivialJumpPredecessor>,
    remove_if_unreferenced: bool,
}

#[derive(Debug, Clone, Copy)]
struct TrivialJumpPredecessor {
    block: ir::Block,
    inst: ir::Inst,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct TrivialJumpNormalizationStats {
    pub(super) removed_blocks: usize,
    pub(super) redirected_edges: usize,
}

pub(super) fn define_prepared_function(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<DefinedFunctionArtifact, String> {
    let compiled = compile_prepared_function_bytes(
        jit_module,
        env_config,
        func_id,
        ctx,
        function_name,
        err_prefix,
    )?;
    define_compiled_function_bytes(jit_module, func_id, &compiled, err_prefix)?;
    Ok(compiled.artifact)
}

pub(super) fn define_compiled_function_bytes(
    jit_module: &mut JITModule,
    func_id: FuncId,
    compiled: &CompiledFunctionArtifact,
    err_prefix: &str,
) -> Result<(), String> {
    jit_module
        .define_function_bytes(
            func_id,
            compiled.bytes.alignment,
            compiled.bytes.code.as_slice(),
            compiled.bytes.relocs.as_slice(),
        )
        .map_err(|err| format!("{err_prefix}: {err}"))?;
    Ok(())
}

pub(super) fn compile_prepared_function_bytes(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    compile_prepared_function_bytes_with_purpose_aliases(
        codegen_env,
        env_config,
        func_id,
        ctx,
        function_name,
        err_prefix,
        None,
        None,
    )
}

pub(super) fn compile_prepared_function_bytes_with_purpose_aliases(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
    purpose_aliases: Option<&ClifFunctionDisplayAliases>,
    block_roles: Option<&ClifBlockRoles>,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if env_config.jit_refcount_emission_enabled() {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(codegen_env, env_config, None, ctx, err_prefix)?;
    if let Some(purpose_aliases) = purpose_aliases {
        let empty_block_roles = ClifBlockRoles::new();
        annotate_clif_instruction_purpose_source_locs(
            &mut ctx.func,
            purpose_aliases,
            block_roles.unwrap_or(&empty_block_roles),
        );
    }
    compile_backend_prepared_function_bytes(codegen_env.codegen_isa(), func_id, ctx, err_prefix)
}

pub(super) fn compile_prepared_function_bytes_with_isa(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    isa: &dyn TargetIsa,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    function_name: &str,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let function_name = if env_config.jit_refcount_emission_enabled() {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(codegen_env, env_config, Some(isa), ctx, err_prefix)?;
    compile_backend_prepared_function_bytes(isa, func_id, ctx, err_prefix)
}

fn compile_backend_prepared_function_bytes(
    isa: &dyn TargetIsa,
    func_id: FuncId,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<CompiledFunctionArtifact, String> {
    let func_for_relocs = ctx.func.clone();
    let mut ctrl_plane = ControlPlane::default();
    let compiled_stencil = isa
        .compile_function(&ctx.func, &ctx.domtree, false, &mut ctrl_plane)
        .map_err(|err| format!("{err_prefix}: {err:?}"))?;
    let compiled = compiled_stencil.apply_params(&ctx.func.params);
    let (code_bb_offsets, code_bb_edges) = compiled.get_code_bb_layout();
    let code_purpose_provenance = collect_code_purpose_provenance(
        compiled.code_buffer().len(),
        &code_bb_offsets,
        compiled.buffer.get_srclocs_sorted(),
    );
    let alignment = compiled.buffer.alignment as u64;
    let relocs = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| ModuleReloc::from_mach_reloc(reloc, &func_for_relocs, func_id))
        .collect::<Vec<_>>();
    let systemv_unwind_info = compiled
        .create_unwind_info(isa)
        .map_err(|err| format!("{err_prefix}: failed to create unwind info: {err:?}"))?
        .and_then(|unwind_info| match unwind_info {
            cranelift_codegen::isa::unwind::UnwindInfo::SystemV(info) => Some(info),
            _ => None,
        });
    let code = compiled.code_buffer().to_vec();
    Ok(CompiledFunctionArtifact {
        bytes: CompiledFunctionBytes {
            code,
            alignment,
            relocs,
        },
        artifact: DefinedFunctionArtifact {
            code_size: compiled.code_buffer().len(),
            code_bb_offsets,
            code_bb_edges,
            code_purpose_names: code_purpose_provenance.purpose_names,
            code_purpose_bytes: code_purpose_provenance.purpose_bytes,
            code_refcount_family_names: code_purpose_provenance.refcount_family_names,
            code_refcount_family_bytes: code_purpose_provenance.refcount_family_bytes,
            code_unattributed_bytes: code_purpose_provenance.unattributed_bytes,
            code_block_role_names: code_purpose_provenance.block_role_names,
            code_block_role_attributed_bytes: code_purpose_provenance.block_role_attributed_bytes,
            code_block_role_unattributed_bytes: code_purpose_provenance
                .block_role_unattributed_bytes,
            code_block_role_purpose_bytes: code_purpose_provenance.block_role_purpose_bytes,
            code_bb_block_role_names: code_purpose_provenance.bb_block_role_names,
            code_bb_purpose_bytes: code_purpose_provenance.bb_purpose_bytes,
            code_bb_unattributed_bytes: code_purpose_provenance.bb_unattributed_bytes,
            systemv_unwind_info,
        },
    })
}

#[derive(Debug, Default)]
struct CodePurposeProvenance {
    purpose_names: Vec<String>,
    purpose_bytes: Vec<usize>,
    refcount_family_names: Vec<String>,
    refcount_family_bytes: Vec<usize>,
    unattributed_bytes: usize,
    block_role_names: Vec<String>,
    block_role_attributed_bytes: Vec<usize>,
    block_role_unattributed_bytes: Vec<usize>,
    block_role_purpose_bytes: Vec<Vec<usize>>,
    bb_block_role_names: Vec<String>,
    bb_purpose_bytes: Vec<Vec<usize>>,
    bb_unattributed_bytes: Vec<usize>,
}

fn collect_code_purpose_provenance(
    code_size: usize,
    code_bb_offsets: &[usize],
    srclocs: &[cranelift_codegen::MachSrcLoc<cranelift_codegen::Final>],
) -> CodePurposeProvenance {
    let mut bytes_by_purpose = BTreeMap::<&'static str, usize>::new();
    let mut bytes_by_refcount_family = BTreeMap::<&'static str, usize>::new();
    let mut bytes_by_block_role = BTreeMap::<&'static str, usize>::new();
    let mut bytes_by_block_role_and_purpose =
        BTreeMap::<(&'static str, &'static str), usize>::new();
    for srcloc in srclocs {
        if let Some(purpose) = clif_purpose_name_from_source_loc_bits(srcloc.loc.bits()) {
            let bytes = (srcloc.end - srcloc.start) as usize;
            *bytes_by_purpose.entry(purpose).or_default() += bytes;
            if let Some(family) = clif_refcount_family_name_from_source_loc_bits(srcloc.loc.bits())
            {
                *bytes_by_refcount_family.entry(family).or_default() += bytes;
            }
            if let Some(block_role) = clif_block_role_name_from_source_loc_bits(srcloc.loc.bits()) {
                *bytes_by_block_role.entry(block_role).or_default() += bytes;
                *bytes_by_block_role_and_purpose
                    .entry((block_role, purpose))
                    .or_default() += bytes;
            }
        }
    }
    let purpose_names = bytes_by_purpose
        .keys()
        .map(|purpose| (*purpose).to_string())
        .collect::<Vec<_>>();
    let purpose_indices = purpose_names
        .iter()
        .enumerate()
        .map(|(index, purpose)| (purpose.as_str(), index))
        .collect::<HashMap<_, _>>();
    let purpose_bytes = purpose_names
        .iter()
        .map(|purpose| {
            bytes_by_purpose
                .get(purpose.as_str())
                .copied()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let refcount_family_names = bytes_by_refcount_family
        .keys()
        .map(|family| (*family).to_string())
        .collect::<Vec<_>>();
    let refcount_family_bytes = refcount_family_names
        .iter()
        .map(|family| {
            bytes_by_refcount_family
                .get(family.as_str())
                .copied()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let mut block_role_names = bytes_by_block_role
        .keys()
        .map(|role| (*role).to_string())
        .collect::<Vec<_>>();
    if !block_role_names.iter().any(|role| role == "unknown") {
        block_role_names.push("unknown".to_string());
    }
    block_role_names.sort();
    let block_role_indices = block_role_names
        .iter()
        .enumerate()
        .map(|(index, role)| (role.as_str(), index))
        .collect::<HashMap<_, _>>();
    let block_role_attributed_bytes = block_role_names
        .iter()
        .map(|role| {
            bytes_by_block_role
                .get(role.as_str())
                .copied()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let mut block_role_unattributed_bytes = vec![0usize; block_role_names.len()];
    let block_role_purpose_bytes = block_role_names
        .iter()
        .map(|block_role| {
            purpose_names
                .iter()
                .map(|purpose| {
                    bytes_by_block_role_and_purpose
                        .get(&(block_role.as_str(), purpose.as_str()))
                        .copied()
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let attributed_bytes = purpose_bytes.iter().sum::<usize>();
    let unattributed_bytes = code_size.saturating_sub(attributed_bytes);

    let ordinary_role_index = block_role_indices
        .get(ClifBlockRole::Ordinary.as_str())
        .copied();
    let unknown_role_index = block_role_indices
        .get("unknown")
        .copied()
        .expect("unknown block role should always exist");
    let mut bb_block_role_names = Vec::with_capacity(code_bb_offsets.len());
    let mut bb_purpose_bytes = Vec::with_capacity(code_bb_offsets.len());
    let mut bb_unattributed_bytes = Vec::with_capacity(code_bb_offsets.len());
    let mut first_candidate_srcloc = 0usize;
    for (block_index, start) in code_bb_offsets.iter().copied().enumerate() {
        let end = code_bb_offsets
            .get(block_index + 1)
            .copied()
            .unwrap_or(code_size);
        while first_candidate_srcloc < srclocs.len()
            && srclocs[first_candidate_srcloc].end as usize <= start
        {
            first_candidate_srcloc += 1;
        }
        let mut block_role_bytes = vec![0usize; block_role_names.len()];
        let mut block_purpose_bytes = vec![0usize; purpose_names.len()];
        let mut attributed_block_bytes = 0usize;
        for srcloc in srclocs.iter().skip(first_candidate_srcloc) {
            let srcloc_start = srcloc.start as usize;
            let srcloc_end = srcloc.end as usize;
            if srcloc_start >= end {
                break;
            }
            let overlap_start = start.max(srcloc_start);
            let overlap_end = end.min(srcloc_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let overlap = overlap_end - overlap_start;
            let Some(purpose) = clif_purpose_name_from_source_loc_bits(srcloc.loc.bits()) else {
                continue;
            };
            let Some(purpose_index) = purpose_indices.get(purpose) else {
                continue;
            };
            block_purpose_bytes[*purpose_index] += overlap;
            if let Some(block_role) = clif_block_role_name_from_source_loc_bits(srcloc.loc.bits())
                && let Some(block_role_index) = block_role_indices.get(block_role)
            {
                block_role_bytes[*block_role_index] += overlap;
            }
            attributed_block_bytes += overlap;
        }
        let dominant_block_role_index = block_role_bytes
            .iter()
            .enumerate()
            .max_by_key(|(index, bytes)| {
                (**bytes, usize::from(Some(*index) == ordinary_role_index))
            })
            .filter(|(_, bytes)| **bytes > 0)
            .map(|(index, _)| index)
            .unwrap_or(unknown_role_index);
        let block_unattributed_bytes = (end - start).saturating_sub(attributed_block_bytes);
        block_role_unattributed_bytes[dominant_block_role_index] += block_unattributed_bytes;
        bb_block_role_names.push(block_role_names[dominant_block_role_index].clone());
        bb_purpose_bytes.push(block_purpose_bytes);
        bb_unattributed_bytes.push(block_unattributed_bytes);
    }

    CodePurposeProvenance {
        purpose_names,
        purpose_bytes,
        refcount_family_names,
        refcount_family_bytes,
        unattributed_bytes,
        block_role_names,
        block_role_attributed_bytes,
        block_role_unattributed_bytes,
        block_role_purpose_bytes,
        bb_block_role_names,
        bb_purpose_bytes,
        bb_unattributed_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CountingWriter {
        writes: Vec<Vec<u8>>,
    }

    impl std::io::Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes.push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn jit_artifact_records_are_complete_single_write_jsonl_lines() {
        let path = Path::new("jit-code-summary.jsonl");
        let first = serde_json::json!({
            "process_id": 17,
            "function_qualname": "Record.method",
            "purpose_names": ["ordinary", "refcount"],
            "block_role_purpose_bytes": [[1, 2], [3, 4]],
            "details": {"unicode": "λ", "enabled": true},
        });
        let second = serde_json::json!({
            "process_id": 23,
            "function_qualname": "Record.other",
            "purpose_names": [],
        });
        let mut writer = CountingWriter::default();

        write_jit_artifact_record(&mut writer, path, &first)
            .expect("first artifact record should serialize and write");
        assert_eq!(
            writer.writes.len(),
            1,
            "one JSONL record must use exactly one underlying write"
        );
        assert_eq!(writer.writes[0].last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&writer.writes[0])
                .expect("newline-terminated artifact should parse as JSON"),
            first
        );

        write_jit_artifact_record(&mut writer, path, &second)
            .expect("second artifact record should serialize and write");
        assert_eq!(
            writer.writes.len(),
            2,
            "appending another JSONL record must use one additional write"
        );
        let records = writer
            .writes
            .concat()
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice::<serde_json::Value>(line)
                    .expect("each appended artifact line should remain valid JSON")
            })
            .collect::<Vec<_>>();
        assert_eq!(records, vec![first, second]);
    }

    #[test]
    fn code_purpose_provenance_splits_machine_blocks_by_srcloc_spans() {
        let refcount = cranelift_codegen::MachSrcLoc::<cranelift_codegen::Final> {
            start: 2,
            end: 7,
            loc: ir::SourceLoc::new(
                clif_purpose_source_loc_bits("refcount").expect("refcount source loc should exist"),
            ),
        };
        let deopt = cranelift_codegen::MachSrcLoc::<cranelift_codegen::Final> {
            start: 7,
            end: 11,
            loc: ir::SourceLoc::new(
                clif_provenance_source_loc_bits("deopt", ClifBlockRole::Cleanup)
                    .expect("deopt source loc should exist"),
            ),
        };

        let provenance = collect_code_purpose_provenance(14, &[0, 5, 10], &[refcount, deopt]);

        assert_eq!(provenance.purpose_names, vec!["deopt", "refcount"]);
        assert_eq!(provenance.purpose_bytes, vec![4, 5]);
        assert_eq!(provenance.refcount_family_names, Vec::<String>::new());
        assert_eq!(provenance.refcount_family_bytes, Vec::<usize>::new());
        assert_eq!(provenance.unattributed_bytes, 5);
        assert_eq!(
            provenance.block_role_names,
            vec![
                "cleanup".to_string(),
                "ordinary".to_string(),
                "unknown".to_string()
            ]
        );
        assert_eq!(provenance.block_role_attributed_bytes, vec![4, 5, 0]);
        assert_eq!(provenance.block_role_unattributed_bytes, vec![3, 2, 0]);
        assert_eq!(
            provenance.block_role_purpose_bytes,
            vec![vec![4, 0], vec![0, 5], vec![0, 0]]
        );
        assert_eq!(
            provenance.bb_block_role_names,
            vec!["ordinary", "cleanup", "cleanup"]
        );
        assert_eq!(
            provenance.bb_purpose_bytes,
            vec![vec![0, 3], vec![3, 2], vec![1, 0]]
        );
        assert_eq!(provenance.bb_unattributed_bytes, vec![2, 0, 3]);
    }

    #[test]
    fn code_purpose_provenance_tracks_refcount_family_bytes() {
        let local_overwrite = cranelift_codegen::MachSrcLoc::<cranelift_codegen::Final> {
            start: 0,
            end: 5,
            loc: ir::SourceLoc::new(refcount_family_source_loc_bits(
                RefcountFamily::LocalOverwrite,
            )),
        };
        let owned_temporary = cranelift_codegen::MachSrcLoc::<cranelift_codegen::Final> {
            start: 5,
            end: 9,
            loc: ir::SourceLoc::new(refcount_family_source_loc_bits(
                RefcountFamily::OwnedTemporary,
            )),
        };

        let provenance =
            collect_code_purpose_provenance(9, &[0], &[local_overwrite, owned_temporary]);

        assert_eq!(provenance.purpose_names, vec!["refcount"]);
        assert_eq!(provenance.purpose_bytes, vec![9]);
        assert_eq!(
            provenance.refcount_family_names,
            vec!["local_overwrite", "owned_temporary"]
        );
        assert_eq!(provenance.refcount_family_bytes, vec![5, 4]);
    }
}

pub(super) fn prepare_cranelift_function_for_backend(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    isa: Option<&dyn TargetIsa>,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<(), String> {
    inline_runtime_support_calls(codegen_env, env_config, ctx, err_prefix)?;
    let isa = isa.unwrap_or_else(|| codegen_env.codegen_isa());
    let mut ctrl_plane = ControlPlane::default();
    ctx.optimize(isa, &mut ctrl_plane)
        .map_err(|err| format!("{err_prefix}: {err:?}"))?;
    ctx.compute_cfg();
    ctx.compute_domtree();
    ctx.verify_if(isa)
        .map_err(|err| format!("{err_prefix}: post-opt verifier failed: {err:?}"))?;
    Ok(())
}

pub(super) fn normalize_postopt_clif_for_inspection(
    func: &mut ir::Function,
) -> TrivialJumpNormalizationStats {
    let mut stats = TrivialJumpNormalizationStats::default();
    loop {
        let cfg = ControlFlowGraph::with_function(func);
        let value_uses = cranelift_value_use_insts(func);
        let blocks = collect_noncritical_trivial_jump_block_rewrites(func, &cfg, &value_uses);
        if blocks.is_empty() {
            break;
        }
        let redirected_edges = redirect_trivial_jump_block_predecessors(func, &blocks);
        if redirected_edges == 0 {
            break;
        }
        stats.redirected_edges += redirected_edges;
        let cfg = ControlFlowGraph::with_function(func);
        let entry_block = func.layout.blocks().next();
        for block in blocks {
            if !block.remove_if_unreferenced {
                continue;
            }
            if Some(block.block) == entry_block {
                continue;
            }
            if cfg.pred_iter(block.block).next().is_none() {
                stats.removed_blocks += 1;
                remove_block_from_layout(func, block.block);
            }
        }
    }
    stats
}

fn collect_noncritical_trivial_jump_block_rewrites(
    func: &ir::Function,
    cfg: &ControlFlowGraph,
    value_uses: &HashMap<ir::Value, Vec<ir::Inst>>,
) -> Vec<TrivialJumpBlock> {
    let mut rewrites = Vec::new();
    let mut occupied_blocks = HashSet::new();
    for block in func.layout.blocks() {
        let Some((jump_inst, target, jump_args)) = trivial_jump_block_target(func, block) else {
            continue;
        };
        if target == block {
            continue;
        }
        let predecessors = cfg
            .pred_iter(block)
            .map(|pred| TrivialJumpPredecessor {
                block: pred.block,
                inst: pred.inst,
            })
            .collect::<Vec<_>>();
        if predecessors.is_empty() {
            continue;
        }
        let params = func.dfg.block_params(block).to_vec();
        if !trivial_jump_args_are_param_forwards(&jump_args, &params) {
            continue;
        }
        if !trivial_jump_block_params_only_feed_jump(jump_inst, &params, value_uses) {
            continue;
        }
        if func.dfg.block_params(target).len() != jump_args.len() {
            continue;
        }

        if predecessors.len() == 1 && predecessors[0].block != target {
            if !trivial_jump_block_edges_are_noncritical(cfg, block, target, &predecessors) {
                continue;
            }
            if predecessors.iter().any(|pred| {
                predecessor_forward_rewrites(func, pred.inst, block, target, &params, &jump_args)
                    .is_none()
            }) {
                continue;
            }
            let involved_blocks = std::iter::once(block)
                .chain(std::iter::once(target))
                .chain(predecessors.iter().map(|pred| pred.block))
                .collect::<Vec<_>>();
            if involved_blocks
                .iter()
                .any(|block| occupied_blocks.contains(block))
            {
                continue;
            }
            occupied_blocks.extend(involved_blocks);
            rewrites.push(TrivialJumpBlock {
                block,
                target,
                params,
                jump_args,
                predecessors,
                remove_if_unreferenced: true,
            });
            continue;
        }

        let final_target_pred_count =
            trivial_jump_final_target_pred_count(cfg, block, target, &predecessors);
        let rewritable_predecessors = predecessors
            .iter()
            .filter(|pred| pred.block != target)
            .filter(|pred| func.dfg.insts[pred.inst].opcode() == ir::Opcode::Jump)
            .filter(|pred| trivial_jump_block_target(func, pred.block).is_some())
            .filter(|pred| {
                trivial_jump_predecessor_edge_is_noncritical(
                    cfg,
                    block,
                    target,
                    pred,
                    final_target_pred_count,
                )
            })
            .filter(|pred| {
                predecessor_forward_rewrites(func, pred.inst, block, target, &params, &jump_args)
                    .is_some()
            })
            .copied()
            .collect::<Vec<_>>();
        if !rewritable_predecessors.is_empty() && rewritable_predecessors.len() < predecessors.len()
        {
            rewrites.push(TrivialJumpBlock {
                block,
                target,
                params,
                jump_args,
                predecessors: rewritable_predecessors,
                remove_if_unreferenced: false,
            });
        }
    }
    rewrites
}

fn trivial_jump_args_are_param_forwards(jump_args: &[ir::BlockArg], params: &[ir::Value]) -> bool {
    let params = params.iter().copied().collect::<HashSet<_>>();
    jump_args.iter().all(|arg| match arg {
        ir::BlockArg::Value(value) => params.contains(value),
        ir::BlockArg::TryCallRet(_) | ir::BlockArg::TryCallExn(_) => false,
    })
}

fn trivial_jump_block_target(
    func: &ir::Function,
    block: ir::Block,
) -> Option<(ir::Inst, ir::Block, Vec<ir::BlockArg>)> {
    let insts = func.layout.block_insts(block).collect::<Vec<_>>();
    let (last, prefix) = insts.split_last()?;
    if prefix
        .iter()
        .any(|inst| func.dfg.insts[*inst].opcode() != ir::Opcode::Nop)
    {
        return None;
    }
    if func.dfg.insts[*last].opcode() != ir::Opcode::Jump {
        return None;
    }
    let destinations =
        func.dfg.insts[*last].branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
    let destination = destinations.first()?;
    if destinations.len() != 1 {
        return None;
    }
    Some((
        *last,
        destination.block(&func.dfg.value_lists),
        destination.args(&func.dfg.value_lists).collect(),
    ))
}

fn cranelift_value_use_insts(func: &ir::Function) -> HashMap<ir::Value, Vec<ir::Inst>> {
    let mut uses: HashMap<ir::Value, Vec<ir::Inst>> = HashMap::new();
    for block in func.layout.blocks() {
        for inst in func.layout.block_insts(block) {
            let mut inst_values = Vec::new();
            for value in func.dfg.inst_args(inst) {
                if !inst_values.contains(value) {
                    inst_values.push(*value);
                }
            }
            let destinations = func.dfg.insts[inst]
                .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
            for destination in destinations {
                for arg in destination.args(&func.dfg.value_lists) {
                    let ir::BlockArg::Value(value) = arg else {
                        continue;
                    };
                    if !inst_values.contains(&value) {
                        inst_values.push(value);
                    }
                }
            }
            for value in inst_values {
                uses.entry(value).or_default().push(inst);
            }
        }
    }
    uses
}

fn trivial_jump_block_params_only_feed_jump(
    jump_inst: ir::Inst,
    params: &[ir::Value],
    value_uses: &HashMap<ir::Value, Vec<ir::Inst>>,
) -> bool {
    params.iter().all(|param| {
        value_uses
            .get(param)
            .is_none_or(|uses| uses.iter().all(|inst| *inst == jump_inst))
    })
}

fn trivial_jump_block_edges_are_noncritical(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessors: &[TrivialJumpPredecessor],
) -> bool {
    let final_target_pred_count =
        trivial_jump_final_target_pred_count(cfg, block, target, predecessors);
    predecessors.iter().all(|pred| {
        trivial_jump_predecessor_edge_is_noncritical(
            cfg,
            block,
            target,
            pred,
            final_target_pred_count,
        )
    })
}

fn trivial_jump_final_target_pred_count(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessors: &[TrivialJumpPredecessor],
) -> usize {
    cfg.pred_iter(target)
        .map(|pred| pred.block)
        .filter(|pred| *pred != block)
        .chain(predecessors.iter().map(|pred| pred.block))
        .collect::<HashSet<_>>()
        .len()
}

fn trivial_jump_predecessor_edge_is_noncritical(
    cfg: &ControlFlowGraph,
    block: ir::Block,
    target: ir::Block,
    predecessor: &TrivialJumpPredecessor,
    final_target_pred_count: usize,
) -> bool {
    let mut final_pred_successors = cfg.succ_iter(predecessor.block).collect::<HashSet<_>>();
    final_pred_successors.remove(&block);
    final_pred_successors.insert(target);
    final_pred_successors.len() <= 1 || final_target_pred_count <= 1
}

fn predecessor_forward_rewrites(
    func: &ir::Function,
    pred_inst: ir::Inst,
    block: ir::Block,
    target: ir::Block,
    params: &[ir::Value],
    jump_args: &[ir::BlockArg],
) -> Option<Vec<(usize, Vec<ir::BlockArg>)>> {
    let mut rewrites = Vec::new();
    let destinations = func.dfg.insts[pred_inst]
        .branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables);
    for (index, destination) in destinations.iter().enumerate() {
        if destination.block(&func.dfg.value_lists) == block {
            let incoming_args = destination.args(&func.dfg.value_lists).collect::<Vec<_>>();
            let forwarded = compose_forwarded_block_args(&incoming_args, params, jump_args)?;
            if func.dfg.block_params(target).len() != forwarded.len() {
                return None;
            }
            rewrites.push((index, forwarded));
        }
    }
    (!rewrites.is_empty()).then_some(rewrites)
}

fn compose_forwarded_block_args(
    incoming_args: &[ir::BlockArg],
    params: &[ir::Value],
    jump_args: &[ir::BlockArg],
) -> Option<Vec<ir::BlockArg>> {
    if incoming_args.len() != params.len() {
        return None;
    }
    let param_args = params
        .iter()
        .copied()
        .zip(incoming_args.iter().copied())
        .collect::<HashMap<_, _>>();
    Some(
        jump_args
            .iter()
            .map(|arg| match arg {
                ir::BlockArg::Value(value) => param_args.get(value).copied().unwrap_or(*arg),
                ir::BlockArg::TryCallRet(_) | ir::BlockArg::TryCallExn(_) => *arg,
            })
            .collect(),
    )
}

fn redirect_trivial_jump_block_predecessors(
    func: &mut ir::Function,
    blocks: &[TrivialJumpBlock],
) -> usize {
    let mut changed = 0;
    for block in blocks {
        for predecessor in &block.predecessors {
            let Some(rewrites) = predecessor_forward_rewrites(
                func,
                predecessor.inst,
                block.block,
                block.target,
                &block.params,
                &block.jump_args,
            ) else {
                continue;
            };
            let new_calls = rewrites
                .into_iter()
                .map(|(index, args)| {
                    (
                        index,
                        ir::BlockCall::new(block.target, args, &mut func.dfg.value_lists),
                    )
                })
                .collect::<Vec<_>>();
            let dfg = &mut func.dfg;
            let destinations = dfg.insts[predecessor.inst]
                .branch_destination_mut(&mut dfg.jump_tables, &mut dfg.exception_tables);
            for (index, destination) in new_calls {
                if destinations[index].block(&dfg.value_lists) == block.block {
                    destinations[index] = destination;
                    changed += 1;
                }
            }
        }
    }
    changed
}

fn remove_block_from_layout(func: &mut ir::Function, block: ir::Block) {
    let insts = func.layout.block_insts(block).collect::<Vec<_>>();
    for inst in insts {
        func.layout.remove_inst(inst);
    }
    func.layout.remove_block(block);
}

#[cfg(test)]
pub(super) fn stable_cranelift_function_name(function_name: &str) -> ir::UserFuncName {
    let hash = stable_cranelift_function_hash(function_name.as_bytes());
    ir::UserFuncName::user((hash >> 32) as u32, hash as u32)
}

#[cfg(not(test))]
fn stable_cranelift_function_name(function_name: &str) -> ir::UserFuncName {
    let hash = stable_cranelift_function_hash(function_name.as_bytes());
    ir::UserFuncName::user((hash >> 32) as u32, hash as u32)
}

#[cfg(test)]
pub(super) fn stable_cranelift_function_hash(bytes: &[u8]) -> u64 {
    stable_cranelift_function_hash_impl(bytes)
}

#[cfg(not(test))]
fn stable_cranelift_function_hash(bytes: &[u8]) -> u64 {
    stable_cranelift_function_hash_impl(bytes)
}

fn stable_cranelift_function_hash_impl(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn append_jit_artifact_record(
    dir: &Path,
    path: &Path,
    record: &serde_json::Value,
    artifact_kind: &str,
) {
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        write_jit_artifact_record(&mut file, path, record)
    })();
    if let Err(err) = result {
        eprintln!("[soac {artifact_kind}] {err}");
    }
}

fn write_jit_artifact_record(
    writer: &mut impl std::io::Write,
    path: &Path,
    record: &serde_json::Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(record)
        .map_err(|err| format!("failed to serialize {}: {err}", path.display()))?;
    bytes.push(b'\n');
    writer
        .write_all(&bytes)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))
}

pub(super) fn record_jit_code_summary(
    env_config: &SoacEnvConfig,
    symbol: &str,
    code_id: u64,
    artifact: &DefinedFunctionArtifact,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    let Some(dir) = env_config.soac_work_dir() else {
        return;
    };
    let path = dir.join("jit-code-summary.jsonl");
    let record = serde_json::json!({
        "process_id": std::process::id(),
        "code_id": code_id,
        "symbol": symbol,
        "code_size": artifact.code_size,
        "machine_block_count": artifact.code_bb_offsets.len(),
        "function_id": format!("{function_id}"),
        "function_qualname": function_qualname,
        "entry_kind": entry_kind,
        "purpose_names": &artifact.code_purpose_names,
        "purpose_bytes": &artifact.code_purpose_bytes,
        "refcount_family_names": &artifact.code_refcount_family_names,
        "refcount_family_bytes": &artifact.code_refcount_family_bytes,
        "unattributed_bytes": artifact.code_unattributed_bytes,
        "block_role_names": &artifact.code_block_role_names,
        "block_role_attributed_bytes": &artifact.code_block_role_attributed_bytes,
        "block_role_unattributed_bytes": &artifact.code_block_role_unattributed_bytes,
        "block_role_purpose_bytes": &artifact.code_block_role_purpose_bytes,
    });
    append_jit_artifact_record(&dir, &path, &record, "jit code summary");
}

pub(super) fn record_jit_bb_map(
    env_config: &SoacEnvConfig,
    symbol: &str,
    code_id: u64,
    artifact: &DefinedFunctionArtifact,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    if !env_config.jit_bb_map_enabled() {
        return;
    }
    let Some(dir) = env_config.soac_work_dir() else {
        return;
    };
    let path = dir.join("jit-bb-map.jsonl");
    let record = serde_json::json!({
        "process_id": std::process::id(),
        "code_id": code_id,
        "symbol": symbol,
        "code_size": artifact.code_size,
        "function_id": format!("{function_id}"),
        "function_qualname": function_qualname,
        "entry_kind": entry_kind,
        "bb_offsets": &artifact.code_bb_offsets,
        "bb_edges": &artifact.code_bb_edges,
        "purpose_names": &artifact.code_purpose_names,
        "purpose_bytes": &artifact.code_purpose_bytes,
        "refcount_family_names": &artifact.code_refcount_family_names,
        "refcount_family_bytes": &artifact.code_refcount_family_bytes,
        "unattributed_bytes": artifact.code_unattributed_bytes,
        "block_role_names": &artifact.code_block_role_names,
        "block_role_attributed_bytes": &artifact.code_block_role_attributed_bytes,
        "block_role_unattributed_bytes": &artifact.code_block_role_unattributed_bytes,
        "block_role_purpose_bytes": &artifact.code_block_role_purpose_bytes,
        "bb_block_role_names": &artifact.code_bb_block_role_names,
        "bb_purpose_bytes": &artifact.code_bb_purpose_bytes,
        "bb_unattributed_bytes": &artifact.code_bb_unattributed_bytes,
    });
    append_jit_artifact_record(&dir, &path, &record, "jit bb map");
}

pub(super) fn register_jit_signal_diagnostics(
    symbol: &str,
    code_ptr: *const u8,
    artifact: &DefinedFunctionArtifact,
    function_id: RuntimeFunctionId,
    function_qualname: &str,
    entry_kind: &str,
) {
    signal_diagnostics::register_jit_code_range(
        symbol,
        code_ptr,
        artifact.code_size,
        function_id,
        function_qualname,
        entry_kind,
        &artifact.code_bb_offsets,
    );
}
