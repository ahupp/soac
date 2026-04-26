use super::backend::{
    define_prepared_function, new_jit_module, normalize_postopt_clif_for_inspection,
    prepare_cranelift_function_for_backend,
};
use super::codegen_env::{JitCodegenEnv, declare_local_fn};
use super::symbols::is_clif_ident_byte;
use cranelift_codegen::cfg_printer::CFGPrinter;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_control::ControlPlane;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::JITModule;
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, ChildVisitable, HasSemanticInstrId, Visit,
};
use soac_ir_blockpy::CodegenModuleShape;
use soac_ir_typed::{InstrTyped, TypedCodegenModuleShape};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RenderedSpecializedClif {
    pub pre_inline_clif: String,
    pub clif: String,
    pub cfg_dot: String,
    pub vcode_disasm: String,
}

#[derive(Debug, Clone)]
pub(super) struct ClifBlockDisplayAnnotation {
    pub(super) semantic_name: String,
    pub(super) param_names: Vec<String>,
}

pub(super) type ClifBlockDisplayAnnotations = HashMap<String, ClifBlockDisplayAnnotation>;

fn rewrite_import_fn_aliases(
    clif: &str,
    import_id_to_symbol: &HashMap<u32, &'static str>,
) -> String {
    let mut import_aliases: HashMap<String, String> = HashMap::new();
    for raw_line in clif.lines() {
        let line = raw_line.trim_start();
        let Some(eq_pos) = line.find(" = ") else {
            continue;
        };
        let alias = &line[..eq_pos];
        if alias.is_empty() {
            continue;
        }
        let rest = &line[(eq_pos + 3)..];
        let rest = rest.strip_prefix("colocated ").unwrap_or(rest);
        let Some(first_token) = rest.split_whitespace().next() else {
            continue;
        };
        let Some(colon_pos) = first_token.find(':') else {
            continue;
        };
        let import_id = &first_token[(colon_pos + 1)..];
        if import_id.is_empty() || !import_id.as_bytes().iter().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(import_id) = import_id.parse::<u32>() else {
            continue;
        };
        let Some(symbol) = import_id_to_symbol.get(&import_id) else {
            continue;
        };
        import_aliases.insert(alias.to_string(), (*symbol).to_string());
    }

    let bytes = clif.as_bytes();
    let mut out = String::with_capacity(clif.len() + 128);
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'f' && index + 2 < bytes.len() && bytes[index + 1] == b'n' {
            let start = index;
            let mut end = index + 2;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            let has_digits = end > start + 2;
            let left_boundary = start == 0 || !is_clif_ident_byte(bytes[start - 1]);
            let right_boundary = end >= bytes.len() || !is_clif_ident_byte(bytes[end]);
            if has_digits && left_boundary && right_boundary {
                let token = &clif[start..end];
                if let Some(alias) = import_aliases.get(token) {
                    out.push_str(alias);
                    index = end;
                    continue;
                }
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

pub(super) fn register_block_display_annotation(
    annotations: &mut ClifBlockDisplayAnnotations,
    block: ir::Block,
    semantic_name: impl Into<String>,
    param_names: Vec<String>,
) {
    annotations.insert(
        block.to_string(),
        ClifBlockDisplayAnnotation {
            semantic_name: semantic_name.into(),
            param_names,
        },
    );
}

fn instr_typed_variant_name(expr: &InstrTyped) -> &'static str {
    match expr {
        InstrTyped::Truthy(_) => "Truthy",
        InstrTyped::Load(_) => "Load",
        InstrTyped::BinOp(_) => "BinOp",
        InstrTyped::Tuple(_) => "Tuple",
        InstrTyped::UnaryOp(_) => "UnaryOp",
        InstrTyped::CalleeFunctionId(_) => "CalleeFunctionId",
        InstrTyped::CallTyped(_) => "CallTyped",
        InstrTyped::GuardedCallableCallTyped(_) => "GuardedCallableCallTyped",
        InstrTyped::GuardedMethodCallTyped(_) => "GuardedMethodCallTyped",
        InstrTyped::DirectCallableCallTyped(_) => "DirectCallableCallTyped",
        InstrTyped::DirectMethodCallTyped(_) => "DirectMethodCallTyped",
        InstrTyped::DirectCallGuardTest(_) => "DirectCallGuardTest",
        InstrTyped::CallDirect(_) => "CallDirect",
        InstrTyped::GetAttrTyped(_) => "GetAttrTyped",
        InstrTyped::SetAttrTyped(_) => "SetAttrTyped",
        InstrTyped::GetItem(_) => "GetItem",
        InstrTyped::SetItem(_) => "SetItem",
        InstrTyped::DelItem(_) => "DelItem",
        InstrTyped::Store(_) => "Store",
        InstrTyped::Del(_) => "Del",
        InstrTyped::MakeCell(_) => "MakeCell",
        InstrTyped::IncrementCounter(_) => "IncrementCounter",
        InstrTyped::CellRef(_) => "CellRef",
        InstrTyped::MakeFunctionWithClosure(_) => "MakeFunctionWithClosure",
    }
}

pub(super) fn render_instr_typed_preorder_extras(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> String {
    struct ExtraRenderer<'a> {
        out: &'a mut String,
        block_label: Option<BlockLabel>,
        ordinal: usize,
    }

    impl Visit<InstrTyped> for ExtraRenderer<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let block_label = self
                .block_label
                .map(|label| label.to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            let instr_id = expr
                .try_semantic_instr_id()
                .map(|instr_id| instr_id.to_string())
                .unwrap_or_else(|| "<synthetic>".to_string());
            match expr.typed_extra() {
                Some(extra) => {
                    self.out.push_str(&format!(
                        "; typed_expr[{}] block={} instr_id={} kind={} extra={:?}\n",
                        self.ordinal,
                        block_label,
                        instr_id,
                        instr_typed_variant_name(expr),
                        extra
                    ));
                }
                None => {
                    self.out.push_str(&format!(
                        "; typed_expr[{}] block={} instr_id={} kind={} extra=<none>\n",
                        self.ordinal,
                        block_label,
                        instr_id,
                        instr_typed_variant_name(expr)
                    ));
                }
            }
            self.ordinal += 1;
            expr.visit_children(self);
        }
    }

    let mut out = String::new();
    let mut renderer = ExtraRenderer {
        out: &mut out,
        block_label: None,
        ordinal: 0,
    };
    for block in &function.blocks {
        renderer.block_label = Some(block.label);
        renderer.visit_block(block);
    }
    out
}

fn parse_block_header_for_display(line: &str) -> Option<(&str, Vec<&str>)> {
    if line.trim_start().len() != line.len() || !line.starts_with("block") {
        return None;
    }
    let bytes = line.as_bytes();
    let mut token_end = "block".len();
    while token_end < bytes.len() && bytes[token_end].is_ascii_digit() {
        token_end += 1;
    }
    if token_end == "block".len() {
        return None;
    }
    let token = &line[..token_end];
    let mut cursor = token_end;
    let mut param_types = Vec::new();
    if cursor < bytes.len() && bytes[cursor] == b'(' {
        let params_start = cursor + 1;
        let params_end = params_start + line[params_start..].find(')')?;
        let params_text = &line[params_start..params_end];
        if !params_text.trim().is_empty() {
            for param in params_text.split(", ") {
                let (_, ty) = param.split_once(':')?;
                param_types.push(ty.trim());
            }
        }
        cursor = params_end + 1;
    }
    if !line[cursor..].trim_end().ends_with(':') {
        return None;
    }
    Some((token, param_types))
}

fn rewrite_block_header_annotations(
    clif: &str,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> String {
    let mut out = String::with_capacity(clif.len() + (block_annotations.len() * 48));
    for chunk in clif.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        out.push_str(line);
        if let Some((token, param_types)) = parse_block_header_for_display(line) {
            let annotation = block_annotations.get(token);
            let semantic_name = annotation
                .map(|annotation| annotation.semantic_name.as_str())
                .unwrap_or(token);
            let param_names = annotation.map(|annotation| annotation.param_names.as_slice());
            out.push_str(" ; block ");
            out.push_str(semantic_name);
            out.push('(');
            for (index, ty) in param_types.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                let fallback_name = format!("param{index}");
                let param_name = param_names
                    .and_then(|names| names.get(index))
                    .map(String::as_str)
                    .unwrap_or(fallback_name.as_str());
                out.push_str(param_name);
                out.push_str(": ");
                out.push_str(ty);
            }
            out.push(')');
        }
        if chunk.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

pub fn run_cranelift_smoke(module: &BlockPyModule<CodegenModuleShape>) -> Result<(), String> {
    let function_count = module.callable_defs.len() as i64;
    let block_count = module
        .callable_defs
        .iter()
        .map(|f| f.blocks.len() as i64)
        .sum::<i64>();
    let sentinel = (function_count << 32) ^ block_count;

    let compile_session = crate::session::CompileSession::new();
    let mut jit_module = new_jit_module(&compile_session)?;
    let env_config = compile_session.env_config()?;
    let mut ctx = jit_module.codegen_make_context();
    ctx.func
        .signature
        .returns
        .push(ir::AbiParam::new(ir::types::I64));
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = builder.create_block();
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let value = builder.ins().iconst(ir::types::I64, sentinel);
        builder.ins().return_(&[value]);
        builder.finalize();
    }

    let function_id = declare_local_fn(&mut jit_module, "dp_jit_smoke", &ctx.func.signature)?;
    let _ = define_prepared_function(
        &mut jit_module,
        env_config,
        function_id,
        &mut ctx,
        "jit-smoke",
        "failed to define Cranelift function",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize Cranelift definitions: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(function_id);
    let compiled: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code_ptr) };
    let got = compiled();
    if got != sentinel {
        return Err(format!(
            "Cranelift JIT smoke mismatch: expected {sentinel}, got {got}"
        ));
    }
    Ok(())
}

pub(super) fn render_pre_inline_clif_for_inspection(
    func: &ir::Function,
    import_id_to_symbol: &HashMap<u32, &'static str>,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> String {
    let mut clif = String::new();
    clif.push_str("; ---- pre-inlining CLIF for inspection ----\n");
    clif.push_str(
        "; emitted after SOAC typed codegen and before runtime support CLIF inlining and Cranelift optimization\n",
    );
    let clif_display =
        rewrite_import_fn_aliases(func.display().to_string().as_str(), import_id_to_symbol);
    clif.push_str(&rewrite_block_header_annotations(
        &clif_display,
        block_annotations,
    ));
    clif
}

pub(super) fn render_compiled_clif_and_vcode_disasm(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    mut ctx: cranelift_codegen::Context,
    import_id_to_symbol: &HashMap<u32, &'static str>,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> Result<(String, String, String), String> {
    prepare_cranelift_function_for_backend(
        jit_module,
        env_config,
        None,
        &mut ctx,
        "failed to render specialized jit run_bb function",
    )?;

    let mut display_func = ctx.func.clone();
    let normalize_stats = normalize_postopt_clif_for_inspection(&mut display_func);
    let cfg_dot = CFGPrinter::new(&display_func).to_string();

    let mut clif = String::new();
    clif.push_str("; ---- normalized post-opt CLIF for inspection ----\n");
    clif.push_str(
        "; trivial jump-only blocks are collapsed here for readability; production codegen uses the unnormalized post-opt CLIF\n",
    );
    clif.push_str(&format!(
        "; normalized trivial jumps: redirected_edges={}, removed_blocks={}\n",
        normalize_stats.redirected_edges, normalize_stats.removed_blocks
    ));
    let clif_display = rewrite_import_fn_aliases(
        display_func.display().to_string().as_str(),
        import_id_to_symbol,
    );
    clif.push_str(&rewrite_block_header_annotations(
        &clif_display,
        block_annotations,
    ));

    let mut ctrl_plane = ControlPlane::default();
    let compiled = jit_module
        .codegen_isa()
        .compile_function(&ctx.func, &ctx.domtree, true, &mut ctrl_plane)
        .map_err(|err| format!("failed to compile specialized jit run_bb function: {err:?}"))?;

    let mut vcode_disasm = String::new();
    vcode_disasm.push_str("; ---- emitted VCode disassembly ----\n");
    match compiled.vcode {
        Some(disasm) if !disasm.trim().is_empty() => vcode_disasm.push_str(&disasm),
        _ => vcode_disasm.push_str("; emitted disassembly unavailable for this backend\n"),
    }

    Ok((clif, cfg_dot, vcode_disasm))
}
