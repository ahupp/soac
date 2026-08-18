use super::backend::{compile_prepared_function_bytes_with_isa, define_prepared_function};
use super::codegen_env::JitCodegenEnv;
use super::inspection::clif_refcount_family_from_source_loc_bits;
use super::precompiled_object::{ElfSymbolBinding, ObjectFunctionDefinition};
use super::symbols::{
    SOAC_RUNTIME_DECREF_SYMBOL, SOAC_RUNTIME_INCREF_SYMBOL, SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL,
    SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL, SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL,
    SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL, SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_SYMBOL,
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL, SOAC_RUNTIME_STORE_GLOBAL_SYMBOL,
    SOAC_RUNTIME_TUPLE_NEW_SYMBOL, SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL,
};
use crate::SOAC_JIT_RUNTIME_CLIF;
use cranelift_codegen::inline::{Inline, InlineCommand};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_codegen::isa::TargetIsa;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Linkage};
use cranelift_reader::parse_functions;
use soac_config::SoacEnvConfig;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

static RUNTIME_SUPPORT_LIBRARY: OnceLock<Result<RuntimeSupportLibrary, String>> = OnceLock::new();
const RUNTIME_SUPPORT_INLINE_MAX_INSTS: usize = 128;
const SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX: &str = "soac_runtime_example_";

fn runtime_support_library() -> Result<&'static RuntimeSupportLibrary, String> {
    match RUNTIME_SUPPORT_LIBRARY.get_or_init(|| {
        if let Some(error) = runtime_support_clif_compatibility_error() {
            return Err(error.to_string());
        }
        parse_runtime_clif_functions().map(|functions| RuntimeSupportLibrary { functions })
    }) {
        Ok(library) => Ok(library),
        Err(error) => Err(error.clone()),
    }
}

#[derive(Debug)]
struct RuntimeSupportInliner {
    inlineable: HashMap<ir::UserExternalName, ir::Function>,
}

impl RuntimeSupportInliner {
    fn for_module(
        codegen_env: &mut impl JitCodegenEnv,
        env_config: &SoacEnvConfig,
    ) -> Result<Self, String> {
        let library = runtime_support_library()?;
        let local_runtime_symbols = runtime_support_local_symbols(library);
        let mut import_func_ids = HashMap::new();
        let mut import_data_ids = HashMap::new();
        let mut local_func_ids = HashMap::new();
        let mut inlineable = HashMap::new();
        for parsed in &library.functions {
            if !matches!(
                parsed.symbol.as_str(),
                SOAC_RUNTIME_INCREF_SYMBOL
                    | SOAC_RUNTIME_DECREF_SYMBOL
                    | SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_SYMBOL
                    | SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL
                    | SOAC_RUNTIME_TUPLE_NEW_SYMBOL
                    | SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL
            ) {
                continue;
            }
            let func_id = declare_runtime_clif_local_function(
                codegen_env,
                &mut local_func_ids,
                &parsed.symbol,
                &parsed.function.signature,
                "inlineable runtime CLIF function",
            )?;
            let mut function = if should_inline_refcount_as_noop(env_config, parsed.symbol.as_str())
            {
                build_noop_runtime_support_function(func_id, &parsed.function.signature)
            } else {
                parsed.function.clone()
            };
            remap_runtime_clif_extern_user_names(
                codegen_env,
                &mut function,
                &parsed.extern_symbols,
                &parsed.runtime_function_symbols,
                &local_runtime_symbols,
                &parsed.global_extern_symbols,
                &mut import_func_ids,
                &mut local_func_ids,
                &mut import_data_ids,
            )?;
            if function.dfg.num_insts() > RUNTIME_SUPPORT_INLINE_MAX_INSTS {
                continue;
            }
            inlineable.insert(ir::UserExternalName::new(0, func_id.as_u32()), function);
        }
        Ok(Self { inlineable })
    }
}

fn should_inline_refcount_as_noop(env_config: &SoacEnvConfig, symbol: &str) -> bool {
    !env_config.jit_refcount_emission_enabled()
        && matches!(
            symbol,
            SOAC_RUNTIME_INCREF_SYMBOL | SOAC_RUNTIME_DECREF_SYMBOL
        )
}

fn build_noop_runtime_support_function(func_id: FuncId, signature: &ir::Signature) -> ir::Function {
    let mut function = ir::Function::with_name_signature(
        ir::UserFuncName::user(0, func_id.as_u32()),
        signature.clone(),
    );
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);
        fb.ins().return_(&[]);
        fb.finalize();
    }
    function
}

impl Inline for RuntimeSupportInliner {
    fn inline(
        &mut self,
        caller: &ir::Function,
        call_inst: ir::Inst,
        _call_opcode: ir::Opcode,
        callee: ir::FuncRef,
        _call_args: &[ir::Value],
    ) -> InlineCommand<'_> {
        let ext_func = &caller.dfg.ext_funcs[callee];
        let ir::ExternalName::User(name_ref) = &ext_func.name else {
            return InlineCommand::KeepCall;
        };
        let user_name = caller.params.user_named_funcs()[*name_ref].clone();
        let Some(callee_func) = self.inlineable.get(&user_name) else {
            return InlineCommand::KeepCall;
        };
        let call_srcloc = caller.srcloc(call_inst);
        let callee = if clif_refcount_family_from_source_loc_bits(call_srcloc.bits()).is_some() {
            let mut callee = callee_func.clone();
            for block in callee.layout.blocks().collect::<Vec<_>>() {
                for inst in callee.layout.block_insts(block).collect::<Vec<_>>() {
                    callee.set_srcloc(inst, call_srcloc);
                }
            }
            Cow::Owned(callee)
        } else {
            Cow::Borrowed(callee_func)
        };
        InlineCommand::Inline {
            callee,
            // We only want to splice these tiny refcount helpers into the caller.
            visit_callee: false,
        }
    }
}

pub(super) fn inline_runtime_support_calls(
    codegen_env: &mut impl JitCodegenEnv,
    env_config: &SoacEnvConfig,
    ctx: &mut cranelift_codegen::Context,
    err_prefix: &str,
) -> Result<bool, String> {
    let mut inliner = RuntimeSupportInliner::for_module(codegen_env, env_config)?;
    ctx.inline(&mut inliner)
        .map_err(|err| format!("{err_prefix}: failed to inline runtime support calls: {err:?}"))
}

fn runtime_support_clif_compatibility_error() -> Option<&'static str> {
    if cfg!(Py_GIL_DISABLED) {
        return Some("runtime CLIF support does not support free-threaded CPython builds");
    }
    if cfg!(py_sys_config = "Py_REF_DEBUG") {
        return Some("runtime CLIF support does not support Py_REF_DEBUG CPython builds");
    }
    if cfg!(py_sys_config = "Py_TRACE_REFS") {
        return Some("runtime CLIF support does not support Py_TRACE_REFS CPython builds");
    }
    None
}

#[derive(Debug)]
struct RuntimeSupportLibrary {
    functions: Vec<ParsedRuntimeClifFunction>,
}

#[derive(Clone, Debug)]
pub(super) struct ParsedRuntimeClifFunction {
    pub(super) symbol: String,
    pub(super) function: ir::Function,
    extern_symbols: HashMap<ir::UserExternalName, String>,
    runtime_function_symbols: HashMap<ir::UserExternalName, String>,
    global_extern_symbols: HashMap<u32, String>,
}

pub(super) fn parse_runtime_clif_functions() -> Result<Vec<ParsedRuntimeClifFunction>, String> {
    let mut parsed_functions = Vec::new();
    for (symbol, clif_text) in SOAC_JIT_RUNTIME_CLIF {
        let reader_clif = runtime_clif_for_reader(clif_text);
        let mut functions = parse_functions(&reader_clif)
            .map_err(|err| format!("failed to parse runtime CLIF for {symbol}: {err}"))?;
        if functions.len() != 1 {
            return Err(format!(
                "expected exactly one runtime CLIF function for {symbol}, found {}",
                functions.len()
            ));
        }
        let function = functions
            .pop()
            .ok_or_else(|| format!("missing parsed runtime CLIF function for {symbol}"))?;
        parsed_functions.push(ParsedRuntimeClifFunction {
            symbol: (*symbol).to_string(),
            function,
            extern_symbols: parse_runtime_clif_extern_symbols(clif_text)?,
            runtime_function_symbols: parse_runtime_clif_runtime_function_symbols(clif_text)?,
            global_extern_symbols: parse_runtime_clif_global_extern_symbols(clif_text)?,
        });
    }
    Ok(parsed_functions)
}

fn runtime_clif_for_reader(clif_text: &str) -> Cow<'_, str> {
    const DISABLED_COMPACT_UNWIND: &str = "set enable_compact_unwind_abi=0";

    if !clif_text.lines().any(|line| {
        line.trim() == DISABLED_COMPACT_UNWIND
            || line
                .trim_start()
                .strip_prefix("target ")
                .is_some_and(|target| target.split_whitespace().count() > 1)
    }) {
        return Cow::Borrowed(clif_text);
    }

    // rustc-codegen-cranelift from newer nightlies can emit disabled global
    // settings and target ISA flags before the crates.io Cranelift reader
    // exposes them. Neither affects the function bodies that SOAC imports and
    // recompiles with its own target ISA.
    Cow::Owned(
        clif_text
            .lines()
            .filter(|line| line.trim() != DISABLED_COMPACT_UNWIND)
            .map(|line| {
                let Some(target) = line.trim_start().strip_prefix("target ") else {
                    return line.to_owned();
                };
                let Some(isa) = target.split_whitespace().next() else {
                    return line.to_owned();
                };
                format!("target {isa}")
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod runtime_clif_reader_tests {
    use super::runtime_clif_for_reader;

    #[test]
    fn parses_runtime_clif_with_unknown_target_isa_settings() {
        let clif_text = format!(
            "target {} unknown_future_isa_setting=0\nfunction u0:0() system_v {{\nblock0:\n    return\n}}\n",
            std::env::consts::ARCH
        );

        let functions = cranelift_reader::parse_functions(&runtime_clif_for_reader(&clif_text))
            .expect("runtime CLIF should ignore settings from a newer target ISA");

        assert_eq!(functions.len(), 1);
    }
}

fn parse_runtime_clif_extern_symbols(
    clif_text: &str,
) -> Result<HashMap<ir::UserExternalName, String>, String> {
    let mut extern_symbols = HashMap::new();
    for line in clif_text.lines() {
        if line.trim_start().starts_with(';') {
            continue;
        }
        if !line.contains("::{extern#") {
            continue;
        }
        if !line.contains("Instance {") {
            continue;
        }
        let Some(user_name) = parse_runtime_clif_user_name(line) else {
            return Err(format!(
                "failed to parse user function name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_extern_symbol(line) else {
            return Err(format!(
                "failed to parse extern symbol from runtime CLIF line: {line}"
            ));
        };
        extern_symbols.insert(user_name, symbol);
    }
    Ok(extern_symbols)
}

fn parse_runtime_clif_runtime_function_symbols(
    clif_text: &str,
) -> Result<HashMap<ir::UserExternalName, String>, String> {
    let mut runtime_symbols = HashMap::new();
    for line in clif_text.lines() {
        if line.trim_start().starts_with(';') {
            continue;
        }
        if !line.contains("Instance {") {
            continue;
        }
        let Some(user_name) = parse_runtime_clif_user_name(line) else {
            return Err(format!(
                "failed to parse user function name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_instance_symbol(line) else {
            continue;
        };
        if symbol.starts_with("soac_runtime_") {
            runtime_symbols.insert(user_name, symbol);
        }
    }
    Ok(runtime_symbols)
}

fn parse_runtime_clif_global_extern_symbols(
    clif_text: &str,
) -> Result<HashMap<u32, String>, String> {
    let mut extern_symbols = HashMap::new();
    for line in clif_text.lines() {
        if !line.contains("::{extern#") || !line.contains(" = symbol userextname") {
            continue;
        }
        let Some(alias_pos) = line.find("userextname") else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let alias = &line[(alias_pos + "userextname".len())..];
        let alias_end = alias
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(alias.len());
        let Some(alias) = alias.get(..alias_end) else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let Ok(alias) = alias.parse::<u32>() else {
            return Err(format!(
                "failed to parse user global name from runtime CLIF line: {line}"
            ));
        };
        let Some(symbol) = parse_runtime_clif_extern_symbol(line) else {
            return Err(format!(
                "failed to parse extern symbol from runtime CLIF line: {line}"
            ));
        };
        extern_symbols.insert(alias, symbol);
    }
    Ok(extern_symbols)
}

fn parse_runtime_clif_user_name(line: &str) -> Option<ir::UserExternalName> {
    let token = line
        .split_whitespace()
        .find(|token| token.starts_with('u') && token.contains(':'))?;
    let rest = token.strip_prefix('u')?;
    let colon = rest.find(':')?;
    let namespace = rest.get(..colon)?.parse().ok()?;
    let rest = rest.get(colon + 1..)?;
    let index_end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    let index = rest.get(..index_end)?.parse().ok()?;
    Some(ir::UserExternalName::new(namespace, index))
}

fn parse_runtime_clif_extern_symbol(line: &str) -> Option<String> {
    let extern_pos = line.find("::{extern#")?;
    let rest = line.get(extern_pos..)?;
    parse_runtime_clif_instance_symbol(rest)
}

fn parse_runtime_clif_instance_symbol(line: &str) -> Option<String> {
    let symbol = line.rsplit("::").next()?;
    let symbol_end = symbol
        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .unwrap_or(symbol.len());
    let symbol = symbol.get(..symbol_end)?;
    if symbol.is_empty() {
        return None;
    }
    Some(symbol.to_string())
}

fn runtime_support_local_symbols(library: &RuntimeSupportLibrary) -> HashSet<String> {
    library
        .functions
        .iter()
        .filter(|parsed| {
            !parsed
                .symbol
                .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        })
        .map(|parsed| parsed.symbol.clone())
        .collect()
}

fn declare_runtime_clif_local_function(
    codegen_env: &mut impl JitCodegenEnv,
    local_func_ids: &mut HashMap<String, FuncId>,
    symbol: &str,
    signature: &ir::Signature,
    description: &str,
) -> Result<FuncId, String> {
    if let Some(func_id) = local_func_ids.get(symbol) {
        return Ok(*func_id);
    }
    let func_id = codegen_env
        .codegen_declare_function(symbol, Linkage::Local, signature)
        .map_err(|err| format!("failed to declare {description} {symbol}: {err}"))?;
    local_func_ids.insert(symbol.to_string(), func_id);
    Ok(func_id)
}

fn remap_runtime_clif_extern_user_names(
    codegen_env: &mut impl JitCodegenEnv,
    function: &mut ir::Function,
    extern_symbols: &HashMap<ir::UserExternalName, String>,
    runtime_function_symbols: &HashMap<ir::UserExternalName, String>,
    local_runtime_symbols: &HashSet<String>,
    global_extern_symbols: &HashMap<u32, String>,
    import_func_ids: &mut HashMap<String, FuncId>,
    local_func_ids: &mut HashMap<String, FuncId>,
    import_data_ids: &mut HashMap<String, cranelift_module::DataId>,
) -> Result<(), String> {
    let remaps = function
        .dfg
        .ext_funcs
        .iter()
        .filter_map(|(_, ext_func)| {
            let ir::ExternalName::User(name_ref) = ext_func.name else {
                return None;
            };
            let original_name = function.params.user_named_funcs()[name_ref].clone();
            Some((name_ref, original_name, ext_func.signature))
        })
        .collect::<Vec<_>>();

    for (name_ref, original_name, sig_ref) in remaps {
        let mapped_name = if let Some(symbol) = runtime_function_symbols
            .get(&original_name)
            .filter(|symbol| local_runtime_symbols.contains(*symbol))
        {
            let sig = function.dfg.signatures[sig_ref].clone();
            let local_id = declare_runtime_clif_local_function(
                codegen_env,
                local_func_ids,
                symbol,
                &sig,
                "runtime CLIF local symbol",
            )?;
            ir::UserExternalName::new(0, local_id.as_u32())
        } else if let Some(symbol) = extern_symbols.get(&original_name) {
            let import_id = if let Some(import_id) = import_func_ids.get(symbol) {
                *import_id
            } else {
                let sig = function.dfg.signatures[sig_ref].clone();
                let import_id = codegen_env
                    .codegen_declare_function(symbol, Linkage::Import, &sig)
                    .map_err(|err| {
                        format!("failed to declare runtime CLIF extern symbol {symbol}: {err}")
                    })?;
                import_func_ids.insert(symbol.clone(), import_id);
                import_id
            };
            ir::UserExternalName::new(0, import_id.as_u32())
        } else {
            return Err(format!(
                "unresolved non-extern runtime CLIF user function name {} while loading {}",
                original_name, function.name
            ));
        };
        function.params.reset_user_func_name(name_ref, mapped_name);
    }

    let global_symbol_remaps = function
        .global_values
        .iter()
        .filter_map(|(gv, data)| {
            let ir::GlobalValueData::Symbol {
                name: ir::ExternalName::User(name_ref),
                ..
            } = data
            else {
                return None;
            };
            Some((gv, *name_ref))
        })
        .collect::<Vec<_>>();
    for (gv, name_ref) in global_symbol_remaps {
        let Some(symbol) = global_extern_symbols.get(&name_ref.as_u32()) else {
            continue;
        };
        let import_id = if let Some(import_id) = import_data_ids.get(symbol) {
            *import_id
        } else {
            let import_id = codegen_env
                .codegen_declare_data(symbol, Linkage::Import, false, false)
                .map_err(|err| {
                    format!("failed to declare runtime CLIF extern data symbol {symbol}: {err}")
                })?;
            import_data_ids.insert(symbol.clone(), import_id);
            import_id
        };
        let mapped_name_ref = function
            .declare_imported_user_function(ir::UserExternalName::new(1, import_id.as_u32()));
        if let ir::GlobalValueData::Symbol { name, .. } = &mut function.global_values[gv] {
            *name = ir::ExternalName::User(mapped_name_ref);
        }
    }
    Ok(())
}

pub(super) fn load_runtime_support_clif_with_debug_symbols(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
) -> Result<HashMap<u32, String>, String> {
    let library = runtime_support_library()?;
    let local_runtime_symbols = runtime_support_local_symbols(library);
    let mut import_func_ids = HashMap::new();
    let mut import_data_ids = HashMap::new();
    let mut local_func_ids = HashMap::new();
    let mut debug_symbols = HashMap::new();
    for parsed in library.functions.iter().cloned() {
        if parsed
            .symbol
            .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        {
            continue;
        }
        let func_id = declare_runtime_clif_local_function(
            jit_module,
            &mut local_func_ids,
            &parsed.symbol,
            &parsed.function.signature,
            "runtime CLIF function",
        )?;
        debug_symbols.insert(func_id.as_u32(), parsed.symbol.clone());
        let mut function = parsed.function;
        remap_runtime_clif_extern_user_names(
            jit_module,
            &mut function,
            &parsed.extern_symbols,
            &parsed.runtime_function_symbols,
            &local_runtime_symbols,
            &parsed.global_extern_symbols,
            &mut import_func_ids,
            &mut local_func_ids,
            &mut import_data_ids,
        )?;
        for (symbol, func_id) in &local_func_ids {
            debug_symbols
                .entry(func_id.as_u32())
                .or_insert_with(|| symbol.clone());
        }
        for (symbol, func_id) in &import_func_ids {
            debug_symbols
                .entry(func_id.as_u32())
                .or_insert_with(|| symbol.clone());
        }
        let mut ctx = jit_module.codegen_make_context();
        ctx.func = function;
        let _ = define_prepared_function(
            jit_module,
            env_config,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!("failed to define runtime CLIF function {}", parsed.symbol),
        )?;
        jit_module.codegen_clear_context(&mut ctx);
    }
    Ok(debug_symbols)
}

pub(super) fn compile_runtime_support_clif_for_object(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    object_isa: &dyn TargetIsa,
) -> Result<Vec<ObjectFunctionDefinition>, String> {
    let library = runtime_support_library()?;
    let local_runtime_symbols = runtime_support_local_symbols(library);
    let mut import_func_ids = HashMap::new();
    let mut import_data_ids = HashMap::new();
    let mut local_func_ids = HashMap::new();
    let mut out = Vec::new();
    for parsed in library.functions.iter().cloned() {
        if parsed
            .symbol
            .starts_with(SOAC_RUNTIME_EXAMPLE_SYMBOL_PREFIX)
        {
            continue;
        }
        let func_id = declare_runtime_clif_local_function(
            jit_module,
            &mut local_func_ids,
            &parsed.symbol,
            &parsed.function.signature,
            "runtime CLIF function",
        )?;
        let mut function = parsed.function;
        remap_runtime_clif_extern_user_names(
            jit_module,
            &mut function,
            &parsed.extern_symbols,
            &parsed.runtime_function_symbols,
            &local_runtime_symbols,
            &parsed.global_extern_symbols,
            &mut import_func_ids,
            &mut local_func_ids,
            &mut import_data_ids,
        )?;
        let mut ctx = jit_module.codegen_make_context();
        ctx.func = function;
        let compiled = compile_prepared_function_bytes_with_isa(
            jit_module,
            env_config,
            object_isa,
            func_id,
            &mut ctx,
            &parsed.symbol,
            &format!(
                "failed to compile runtime CLIF function {} to object",
                parsed.symbol
            ),
        )?;
        jit_module.codegen_clear_context(&mut ctx);
        out.push(ObjectFunctionDefinition {
            func_id,
            symbol: parsed.symbol,
            binding: ElfSymbolBinding::Local,
            bytes: compiled.bytes,
            systemv_unwind_info: compiled.artifact.systemv_unwind_info,
        });
    }
    Ok(out)
}
