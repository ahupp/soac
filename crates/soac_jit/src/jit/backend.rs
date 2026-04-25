use super::runtime_support::{inline_runtime_support_calls, load_runtime_support_clif};
use super::specialized_helpers::register_specialized_jit_symbols;
use super::symbols::{
    cpython_type_symbol_name, lookup_registered_jit_data_symbol, py_dealloc_symbol,
};
use super::*;
use crate::function_instantiation::{
    SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL, soac_jit_make_function_with_closure,
};
use cranelift_codegen::flowgraph::ControlFlowGraph;
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::ModuleReloc;

const JIT_ARENA_BYTES: usize = 256 * 1024 * 1024;

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
        "_PyDict_IndexedValueTombstone",
        std::ptr::addr_of_mut!(_PyDict_IndexedValueTombstone).cast::<u8>(),
    );
    builder.symbol(
        SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL,
        soac_jit_make_function_with_closure as *const u8,
    );
    builder.symbol_lookup_fn(Box::new(lookup_registered_jit_data_symbol));
    register_specialized_jit_symbols(builder);
}

pub(super) fn new_jit_module(
    compile_session: &crate::session::CompileSession,
) -> Result<JITModule, String> {
    let env_config = compile_session.env_config()?;
    let mut jit_module = JITModule::new(new_jit_builder(env_config)?);
    load_runtime_support_clif(&mut jit_module, env_config)?;
    Ok(jit_module)
}

#[derive(Debug)]
pub(super) struct DefinedFunctionArtifact {
    pub(super) code_size: usize,
    pub(super) code_bb_offsets: Vec<usize>,
    pub(super) code_bb_edges: Vec<(usize, usize)>,
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
    let function_name = if env_config.jit_refcount_emission_enabled() {
        Cow::Borrowed(function_name)
    } else {
        Cow::Owned(format!("{function_name}:refcounts=off"))
    };
    ctx.func.name = stable_cranelift_function_name(function_name.as_ref());
    prepare_cranelift_function_for_backend(codegen_env, env_config, None, ctx, err_prefix)?;
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
            systemv_unwind_info,
        },
    })
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

pub(super) fn record_jit_bb_map(
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
    });
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
        use std::io::Write;
        serde_json::to_writer(&mut file, &record)
            .map_err(|err| format!("failed to serialize {}: {err}", path.display()))?;
        file.write_all(b"\n")
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
        Ok(())
    })();
    if let Err(err) = result {
        eprintln!("[soac jit bb map] {err}");
    }
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
