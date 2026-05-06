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
    BlockArg, BlockEdge, BlockLabel, BlockParam, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgKeyword, CallArgPositional, ChildVisitable, HasSemanticInstrId, NameLike, Visit,
};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::{InstrTyped, TypedBlockPyModuleShape, TypedInstrExtra, ValueFacts};
use std::collections::{HashMap, HashSet};

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
pub(super) type ClifBlockRoles = HashMap<ir::Block, ClifBlockRole>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum ClifBlockRole {
    Ordinary,
    Cleanup,
    RefcountSupport,
}

impl ClifBlockRole {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Cleanup => "cleanup",
            Self::RefcountSupport => "refcount_support",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClifFunctionDisplayKind {
    RuntimeHelper,
    DirectPython,
}

#[derive(Debug, Clone)]
pub(super) struct ClifFunctionDisplayAlias {
    pub(super) display_name: String,
    kind: ClifFunctionDisplayKind,
}

impl ClifFunctionDisplayAlias {
    pub(super) fn runtime_helper(display_name: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            kind: ClifFunctionDisplayKind::RuntimeHelper,
        }
    }

    pub(super) fn direct_python(qualname: impl Into<String>) -> Self {
        Self {
            display_name: qualname.into(),
            kind: ClifFunctionDisplayKind::DirectPython,
        }
    }
}

pub(super) type ClifFunctionDisplayAliases = HashMap<u32, ClifFunctionDisplayAlias>;

#[derive(Debug, Clone, Copy)]
enum ClifInstructionPurposeConfidence {
    Exact,
    Inferred,
}

impl ClifInstructionPurposeConfidence {
    fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Inferred => "inferred",
        }
    }
}

#[derive(Debug, Clone)]
struct ClifInstructionPurpose {
    primary: &'static str,
    detail: String,
    confidence: ClifInstructionPurposeConfidence,
}

const CLIF_PURPOSE_SOURCE_LOC_BASE: u32 = 0x5a00_0000;

const CLIF_PURPOSE_NAMES: &[&str] = &[
    "codegen_plumbing",
    "control_flow",
    "counter",
    "deopt",
    "direct_call",
    "exception",
    "memory_access",
    "python_semantics",
    "refcount",
    "return",
    "runtime_helper",
    "stack_management",
];

const CLIF_BLOCK_ROLES: &[ClifBlockRole] = &[
    ClifBlockRole::Ordinary,
    ClifBlockRole::Cleanup,
    ClifBlockRole::RefcountSupport,
];

#[cfg(test)]
pub(super) fn clif_purpose_source_loc_bits(primary: &str) -> Option<u32> {
    clif_provenance_source_loc_bits(primary, ClifBlockRole::Ordinary)
}

pub(super) fn clif_provenance_source_loc_bits(
    primary: &str,
    block_role: ClifBlockRole,
) -> Option<u32> {
    let purpose_index = CLIF_PURPOSE_NAMES
        .iter()
        .position(|candidate| *candidate == primary)?;
    let role_index = CLIF_BLOCK_ROLES
        .iter()
        .position(|candidate| *candidate == block_role)?;
    Some(
        CLIF_PURPOSE_SOURCE_LOC_BASE
            + (role_index * CLIF_PURPOSE_NAMES.len() + purpose_index) as u32,
    )
}

fn clif_provenance_from_source_loc_bits(bits: u32) -> Option<(&'static str, ClifBlockRole)> {
    let provenance_index = bits.checked_sub(CLIF_PURPOSE_SOURCE_LOC_BASE)? as usize;
    let purpose_index = provenance_index % CLIF_PURPOSE_NAMES.len();
    let role_index = provenance_index / CLIF_PURPOSE_NAMES.len();
    Some((
        CLIF_PURPOSE_NAMES.get(purpose_index).copied()?,
        *CLIF_BLOCK_ROLES.get(role_index)?,
    ))
}

pub(super) fn clif_block_role_name_from_source_loc_bits(bits: u32) -> Option<&'static str> {
    clif_provenance_from_source_loc_bits(bits).map(|(_, role)| role.as_str())
}

pub(super) fn clif_purpose_name_from_source_loc_bits(bits: u32) -> Option<&'static str> {
    clif_provenance_from_source_loc_bits(bits).map(|(purpose, _)| purpose)
}

fn clif_block_role_for_block(block_roles: &ClifBlockRoles, block: ir::Block) -> ClifBlockRole {
    block_roles
        .get(&block)
        .copied()
        .unwrap_or(ClifBlockRole::Ordinary)
}

pub(super) fn register_block_role(
    block_roles: &mut ClifBlockRoles,
    block: ir::Block,
    role: ClifBlockRole,
) {
    block_roles.insert(block, role);
}

pub(super) fn annotate_clif_instruction_purpose_source_locs(
    func: &mut ir::Function,
    function_aliases: &ClifFunctionDisplayAliases,
    block_roles: &ClifBlockRoles,
) {
    for block in func.layout.blocks().collect::<Vec<_>>() {
        let in_refcount_block = block_has_inlined_refcount_shape(func, block);
        let block_role = if in_refcount_block {
            ClifBlockRole::RefcountSupport
        } else {
            clif_block_role_for_block(block_roles, block)
        };
        for inst in func.layout.block_insts(block).collect::<Vec<_>>() {
            let purpose = purpose_for_instruction(func, function_aliases, inst, in_refcount_block);
            let Some(bits) = clif_provenance_source_loc_bits(purpose.primary, block_role) else {
                continue;
            };
            func.set_srcloc(inst, ir::SourceLoc::new(bits));
        }
    }
}

impl ClifInstructionPurpose {
    fn exact(primary: &'static str, detail: impl Into<String>) -> Self {
        Self {
            primary,
            detail: detail.into(),
            confidence: ClifInstructionPurposeConfidence::Exact,
        }
    }

    fn inferred(primary: &'static str, detail: impl Into<String>) -> Self {
        Self {
            primary,
            detail: detail.into(),
            confidence: ClifInstructionPurposeConfidence::Inferred,
        }
    }

    fn render(&self) -> String {
        format!(
            "; purpose: {} | {} | {}",
            self.primary,
            self.confidence.as_str(),
            self.detail
        )
    }
}

pub(super) fn rewrite_clif_function_aliases(
    clif: &str,
    function_aliases: &ClifFunctionDisplayAliases,
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
        let Some(function_alias) = function_aliases.get(&import_id) else {
            continue;
        };
        import_aliases.insert(alias.to_string(), function_alias.display_name.clone());
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

#[derive(Debug, Clone)]
struct ClifCallTarget {
    display_name: String,
    kind: Option<ClifFunctionDisplayKind>,
}

fn function_ref_target(
    func: &ir::Function,
    function_aliases: &ClifFunctionDisplayAliases,
    func_ref: ir::FuncRef,
) -> Option<ClifCallTarget> {
    let ext_func = &func.dfg.ext_funcs[func_ref];
    let display_name = ext_func.name.display(Some(&func.params)).to_string();
    if let Some((_, import_id)) = display_name.split_once(':')
        && let Ok(import_id) = import_id.parse::<u32>()
        && let Some(alias) = function_aliases.get(&import_id)
    {
        return Some(ClifCallTarget {
            display_name: alias.display_name.clone(),
            kind: Some(alias.kind),
        });
    }
    Some(ClifCallTarget {
        display_name,
        kind: None,
    })
}

fn call_instruction_target(
    func: &ir::Function,
    function_aliases: &ClifFunctionDisplayAliases,
    inst: ir::Inst,
) -> Option<ClifCallTarget> {
    match &func.dfg.insts[inst] {
        ir::InstructionData::Call { func_ref, .. }
        | ir::InstructionData::TryCall { func_ref, .. } => {
            function_ref_target(func, function_aliases, *func_ref)
        }
        _ => None,
    }
}

fn purpose_for_helper_call(target: &ClifCallTarget) -> ClifInstructionPurpose {
    let symbol = target.display_name.as_str();
    if target.kind == Some(ClifFunctionDisplayKind::DirectPython) {
        return ClifInstructionPurpose::exact(
            "direct_call",
            format!("direct Python function call callee={symbol}"),
        );
    }
    if symbol.contains("incref") || symbol.contains("decref") {
        return ClifInstructionPurpose::exact(
            "refcount",
            format!("runtime refcount helper call helper={symbol}"),
        );
    }
    if symbol.contains("direct_compile_function_env") || symbol.contains("enter_recursive_call") {
        return ClifInstructionPurpose::exact(
            "direct_call",
            format!("direct-call helper call helper={symbol}"),
        );
    }
    if symbol.contains("deopt") {
        return ClifInstructionPurpose::exact(
            "deopt",
            format!("deoptimization helper call helper={symbol}"),
        );
    }
    if symbol.contains("counter") || symbol.contains("record_top_value") {
        return ClifInstructionPurpose::exact(
            "counter",
            format!("counter helper call helper={symbol}"),
        );
    }
    if symbol.contains("raise")
        || symbol.contains("exception")
        || symbol.contains("unbound_local")
        || symbol.contains("pop_handled")
        || symbol.contains("push_handled")
    {
        return ClifInstructionPurpose::exact(
            "exception",
            format!("exception helper call helper={symbol}"),
        );
    }
    if symbol.contains("load_global")
        || symbol.contains("store_global")
        || symbol.contains("py_call")
        || symbol.contains("py_vectorcall")
        || symbol.contains("pyobject")
        || symbol.contains("pylong")
        || symbol.contains("runtime_obj")
        || symbol.contains("tuple")
        || symbol.contains("cell")
        || symbol.contains("make_function")
    {
        return ClifInstructionPurpose::exact(
            "runtime_helper",
            format!("runtime helper call helper={symbol}"),
        );
    }
    ClifInstructionPurpose::inferred(
        "runtime_helper",
        format!("colocated/helper call helper={symbol}"),
    )
}

fn clif_inst_text(func: &ir::Function, inst: ir::Inst) -> String {
    func.dfg.display_inst(inst).to_string()
}

fn block_has_inlined_refcount_shape(func: &ir::Function, block: ir::Block) -> bool {
    let mut has_i32_refcount_load = false;
    let mut has_i32_refcount_store = false;
    let mut has_refcount_update = false;
    let mut has_immortal_or_zero_check = false;

    for inst in func.layout.block_insts(block) {
        let text = clif_inst_text(func, inst);
        let opcode = func.dfg.insts[inst].opcode().to_string();
        let data = &func.dfg.insts[inst];
        if text.contains("load.i32") {
            has_i32_refcount_load = true;
        }
        if matches!(
            data,
            ir::InstructionData::Store { .. } | ir::InstructionData::StoreNoOffset { .. }
        ) {
            has_i32_refcount_store = true;
        }
        if opcode.starts_with("iadd") || opcode.starts_with("isub") {
            has_refcount_update = true;
        }
        if text.contains("-1073741824") || text.contains("icmp slt") || text.contains("icmp uge") {
            has_immortal_or_zero_check = true;
        }
    }

    (has_i32_refcount_load
        && (has_i32_refcount_store || has_refcount_update || has_immortal_or_zero_check))
        || (has_i32_refcount_store && has_refcount_update)
}

fn purpose_for_instruction(
    func: &ir::Function,
    function_aliases: &ClifFunctionDisplayAliases,
    inst: ir::Inst,
    in_refcount_block: bool,
) -> ClifInstructionPurpose {
    if let Some(target) = call_instruction_target(func, function_aliases, inst) {
        let purpose = purpose_for_helper_call(&target);
        if in_refcount_block && purpose.primary == "runtime_helper" {
            return ClifInstructionPurpose::inferred(
                "refcount",
                format!(
                    "runtime helper reached from inlined refcount sequence helper={}",
                    target.display_name
                ),
            );
        }
        return purpose;
    }

    let data = &func.dfg.insts[inst];
    let opcode = data.opcode().to_string();
    if in_refcount_block {
        return ClifInstructionPurpose::inferred(
            "refcount",
            format!("inlined refcount helper/control-flow opcode={opcode}"),
        );
    }
    match data {
        ir::InstructionData::CallIndirect { .. } | ir::InstructionData::TryCallIndirect { .. } => {
            ClifInstructionPurpose::inferred(
                "direct_call",
                format!("indirect compiled-function call opcode={opcode}"),
            )
        }
        ir::InstructionData::Jump { .. }
        | ir::InstructionData::Brif { .. }
        | ir::InstructionData::BranchTable { .. } => ClifInstructionPurpose::inferred(
            "control_flow",
            format!("block transition opcode={opcode}"),
        ),
        ir::InstructionData::StackLoad { .. }
        | ir::InstructionData::StackStore { .. }
        | ir::InstructionData::DynamicStackLoad { .. }
        | ir::InstructionData::DynamicStackStore { .. } => ClifInstructionPurpose::inferred(
            "stack_management",
            format!("stack slot access opcode={opcode}"),
        ),
        ir::InstructionData::Load { .. }
        | ir::InstructionData::Store { .. }
        | ir::InstructionData::LoadNoOffset { .. }
        | ir::InstructionData::StoreNoOffset { .. } => ClifInstructionPurpose::inferred(
            "memory_access",
            format!("raw runtime memory access opcode={opcode}"),
        ),
        _ if opcode == "stack_addr" => ClifInstructionPurpose::inferred(
            "stack_management",
            "stack slot address materialization",
        ),
        _ if opcode == "return" => ClifInstructionPurpose::inferred("return", "function return"),
        _ if opcode.contains("trap") => ClifInstructionPurpose::inferred(
            "exception",
            format!("trap/error edge opcode={opcode}"),
        ),
        _ if opcode.contains("const")
            || opcode == "symbol_value"
            || opcode == "global_value"
            || opcode.starts_with('i')
            || opcode.starts_with('b')
            || opcode.starts_with("icmp") =>
        {
            ClifInstructionPurpose::inferred(
                "codegen_plumbing",
                format!("scalar/address computation opcode={opcode}"),
            )
        }
        _ => ClifInstructionPurpose::inferred(
            "python_semantics",
            format!("lowered operation opcode={opcode}"),
        ),
    }
}

fn collect_clif_instruction_purposes(
    func: &ir::Function,
    function_aliases: &ClifFunctionDisplayAliases,
) -> Vec<ClifInstructionPurpose> {
    let mut purposes = Vec::with_capacity(func.dfg.num_insts());
    for block in func.layout.blocks() {
        let in_refcount_block = block_has_inlined_refcount_shape(func, block);
        for inst in func.layout.block_insts(block) {
            purposes.push(purpose_for_instruction(
                func,
                function_aliases,
                inst,
                in_refcount_block,
            ));
        }
    }
    purposes
}

fn is_rendered_clif_instruction_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let is_function_decl = trimmed.split_once(" = ").is_some_and(|(_, rest)| {
        let rest = rest.strip_prefix("colocated ").unwrap_or(rest);
        rest.starts_with('u') && rest.contains(" sig")
    });
    if trimmed.is_empty()
        || trimmed.starts_with(';')
        || trimmed.starts_with("function ")
        || trimmed.starts_with("block")
        || trimmed == "}"
        || trimmed.starts_with("ss")
        || trimmed.starts_with("gv")
        || trimmed.starts_with("sig")
        || trimmed.starts_with("fn")
        || is_function_decl
    {
        return false;
    }
    trimmed.contains(" = ")
        || trimmed.starts_with("jump ")
        || trimmed.starts_with("brif")
        || trimmed.starts_with("return")
        || trimmed.starts_with("call ")
        || trimmed.starts_with("store")
        || trimmed.starts_with("trap")
}

fn clif_line_indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

pub(super) fn annotate_clif_instruction_purposes(
    func: &ir::Function,
    clif: &str,
    function_aliases: &ClifFunctionDisplayAliases,
) -> String {
    let purposes = collect_clif_instruction_purposes(func, function_aliases);
    let mut purpose_iter = purposes.into_iter();
    let mut out = String::with_capacity(clif.len() + (func.dfg.num_insts() * 48));
    for line in clif.lines() {
        out.push_str(line);
        out.push('\n');
        if is_rendered_clif_instruction_line(line)
            && let Some(purpose) = purpose_iter.next()
        {
            out.push_str(clif_line_indent(line));
            out.push_str("    ");
            out.push_str(&purpose.render());
            out.push('\n');
        }
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

pub(super) fn render_instr_typed_program(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "function {}({}):\n",
        function.names.qualname,
        function
            .params
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!("  function_id = {}\n", function.function_id));
    out.push_str(&format!(
        "  execution_mode = {:?}\n",
        function.execution_mode
    ));
    for block in &function.blocks {
        out.push_str(&format!(
            "\n  {}({}):\n",
            block.label,
            render_typed_block_params(&block.params)
        ));
        for stmt in &block.body {
            out.push_str("    ");
            out.push_str(&render_typed_expr(stmt));
            out.push('\n');
        }
        render_typed_term(&mut out, &block.term, "    ");
        if let Some(edge) = &block.exc_edge {
            out.push_str("    except ");
            out.push_str(&render_typed_edge(edge));
            out.push('\n');
        }
    }
    out
}

fn render_typed_block_params(params: &[BlockParam]) -> String {
    params
        .iter()
        .map(|param| format!("{}:{:?}", param.name, param.role))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_typed_term(out: &mut String, term: &BlockTerm<InstrTyped>, indent: &str) {
    match term {
        BlockTerm::Jump(edge) => {
            out.push_str(indent);
            out.push_str("jump ");
            out.push_str(&render_typed_edge(edge));
            out.push('\n');
        }
        BlockTerm::IfTerm(if_term) => {
            out.push_str(indent);
            out.push_str("if ");
            out.push_str(&render_typed_expr(&if_term.test));
            out.push_str(":\n");
            out.push_str(indent);
            out.push_str("  then jump ");
            out.push_str(&if_term.then_label.to_string());
            out.push('\n');
            out.push_str(indent);
            out.push_str("  else jump ");
            out.push_str(&if_term.else_label.to_string());
            out.push('\n');
        }
        BlockTerm::BranchTable(branch) => {
            out.push_str(indent);
            out.push_str("branch_table ");
            out.push_str(&render_typed_expr(&branch.index));
            out.push_str(" -> [");
            out.push_str(
                &branch
                    .targets
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push_str("] default ");
            out.push_str(&branch.default_label.to_string());
            out.push('\n');
        }
        BlockTerm::Raise(raise) => {
            out.push_str(indent);
            match &raise.exc {
                Some(exc) => {
                    out.push_str("raise ");
                    out.push_str(&render_typed_expr(exc));
                }
                None => out.push_str("raise"),
            }
            out.push('\n');
        }
        BlockTerm::Return(value) => {
            out.push_str(indent);
            out.push_str("return ");
            out.push_str(&render_typed_expr(value));
            out.push('\n');
        }
    }
}

fn render_typed_edge(edge: &BlockEdge) -> String {
    if edge.args.is_empty() {
        return edge.target.to_string();
    }
    format!(
        "{}({})",
        edge.target,
        edge.args
            .iter()
            .map(render_typed_block_arg)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_typed_block_arg(arg: &BlockArg) -> String {
    match arg {
        BlockArg::Name(name) => name.clone(),
        BlockArg::None => "None".to_string(),
        BlockArg::CurrentException => "CurrentException".to_string(),
        BlockArg::AbruptKind(kind) => format!("AbruptKind::{kind:?}"),
    }
}

fn render_typed_expr(expr: &InstrTyped) -> String {
    match expr {
        InstrTyped::Truthy(op) => render_call_like("Truthy", [render_typed_expr(&op.value)]),
        InstrTyped::Load(op) => render_typed_name(&op.name),
        InstrTyped::BinOp(op) => render_call_like(
            "BinOp",
            [
                format!("{:?}", op.kind),
                render_typed_expr(&op.left),
                render_typed_expr(&op.right),
            ],
        ),
        InstrTyped::Tuple(op) => render_call_like(
            "Tuple",
            op.values.iter().map(render_typed_expr).collect::<Vec<_>>(),
        ),
        InstrTyped::UnaryOp(op) => render_call_like(
            "UnaryOp",
            [format!("{:?}", op.kind), render_typed_expr(&op.operand)],
        ),
        InstrTyped::CalleeFunctionId(op) => {
            render_call_like("CalleeFunctionId", [render_typed_expr(&op.value)])
        }
        InstrTyped::CallTyped(op) => {
            let mut args = vec![render_typed_expr(&op.func)];
            args.extend(render_typed_call_args(&op.args, &op.keywords));
            args.extend(render_debug_annotation("access", &op.access));
            render_call_like("Call", args)
        }
        InstrTyped::GuardedCallableCallTyped(op) => {
            let mut args = vec![render_typed_expr(&op.func)];
            args.extend(render_typed_call_args(&op.args, &op.keywords));
            args.push(format!("guards={}", op.function_guards.len()));
            render_call_like("GuardedCallableCall", args)
        }
        InstrTyped::GuardedMethodCallTyped(op) => {
            let mut args = vec![render_typed_expr(&op.func)];
            args.extend(render_typed_call_args(&op.args, &op.keywords));
            args.push(format!("method={:?}", op.method_name));
            args.push(format!("guards={}", op.method_guards.len()));
            render_call_like("GuardedMethodCall", args)
        }
        InstrTyped::DirectCallableCallTyped(op) => {
            let mut args = vec![render_typed_expr(&op.func)];
            args.extend(op.args.iter().map(render_typed_positional_arg));
            args.push(format!("guard={:?}", op.guard));
            render_call_like("DirectCallableCall", args)
        }
        InstrTyped::DirectMethodCallTyped(op) => {
            let mut args = vec![render_typed_expr(&op.receiver)];
            args.extend(op.args.iter().map(render_typed_positional_arg));
            args.push(format!("method={:?}", op.method_name));
            args.push(format!("guard={:?}", op.guard));
            render_call_like("DirectMethodCall", args)
        }
        InstrTyped::DirectCallGuardTest(op) => render_call_like(
            "DirectCallGuardTest",
            [render_typed_expr(&op.value), format!("kind={:?}", op.kind)],
        ),
        InstrTyped::CallDirect(op) => {
            let mut args = vec![
                format!("function_id={}", op.function_id),
                render_typed_expr(&op.callable),
            ];
            args.extend(render_typed_call_args(&op.args, &op.keywords));
            render_call_like("CallDirect", args)
        }
        InstrTyped::GetAttrTyped(op) => {
            let mut args = vec![render_typed_expr(&op.value), render_typed_expr(&op.attr)];
            args.extend(render_debug_annotation("access", &op.access));
            render_call_like("GetAttr", args)
        }
        InstrTyped::SetAttrTyped(op) => {
            let mut args = vec![
                render_typed_expr(&op.value),
                render_typed_expr(&op.attr),
                render_typed_expr(&op.replacement),
            ];
            args.extend(render_debug_annotation("access", &op.access));
            render_call_like("SetAttr", args)
        }
        InstrTyped::GetItem(op) => render_call_like(
            "GetItem",
            [render_typed_expr(&op.value), render_typed_expr(&op.index)],
        ),
        InstrTyped::SetItem(op) => render_call_like(
            "SetItem",
            [
                render_typed_expr(&op.value),
                render_typed_expr(&op.index),
                render_typed_expr(&op.replacement),
            ],
        ),
        InstrTyped::DelItem(op) => render_call_like(
            "DelItem",
            [render_typed_expr(&op.value), render_typed_expr(&op.index)],
        ),
        InstrTyped::Store(op) => render_call_like(
            "Store",
            [render_typed_name(&op.name), render_typed_expr(&op.value)],
        ),
        InstrTyped::Del(op) => render_call_like(
            "Del",
            [
                render_typed_name(&op.name),
                format!("quietly={}", op.quietly),
            ],
        ),
        InstrTyped::MakeCell(op) => match &op.initial_value {
            Some(value) => render_call_like("MakeCell", [render_typed_expr(value)]),
            None => "MakeCell()".to_string(),
        },
        InstrTyped::IncrementCounter(op) => {
            render_call_like("IncrementCounter", [format!("{:?}", op.counter_id)])
        }
        InstrTyped::CellRef(op) => render_call_like("CellRef", [format!("{:?}", op.location)]),
        InstrTyped::MakeFunctionWithClosure(op) => render_call_like(
            "MakeFunctionWithClosure",
            [
                format!("function_id={}", op.function_id),
                format!("kind={:?}", op.kind),
                render_typed_expr(&op.captures),
                render_typed_expr(&op.param_defaults),
                render_typed_expr(&op.annotate_fn),
            ],
        ),
    }
}

fn render_typed_name(name: &soac_core::block_py::ResolvedName) -> String {
    name.id_str().to_string()
}

fn render_call_like(name: &str, args: impl IntoIterator<Item = String>) -> String {
    format!(
        "{}({})",
        name,
        args.into_iter().collect::<Vec<_>>().join(", ")
    )
}

fn render_debug_annotation<T: std::fmt::Debug>(name: &str, value: &T) -> Vec<String> {
    let rendered = format!("{value:?}");
    if rendered == "Generic" {
        Vec::new()
    } else {
        vec![format!("{name}={rendered}")]
    }
}

fn render_typed_call_args(
    args: &[CallArgPositional<InstrTyped>],
    keywords: &[CallArgKeyword<InstrTyped>],
) -> Vec<String> {
    let mut rendered = args
        .iter()
        .map(render_typed_positional_arg)
        .collect::<Vec<_>>();
    rendered.extend(keywords.iter().map(render_typed_keyword_arg));
    rendered
}

fn render_typed_positional_arg(arg: &CallArgPositional<InstrTyped>) -> String {
    match arg {
        CallArgPositional::Positional(expr) => render_typed_expr(expr),
        CallArgPositional::Starred(expr) => format!("*{}", render_typed_expr(expr)),
    }
}

fn render_typed_keyword_arg(arg: &CallArgKeyword<InstrTyped>) -> String {
    match arg {
        CallArgKeyword::Named { arg, value } => {
            format!("{}={}", arg.as_str(), render_typed_expr(value))
        }
        CallArgKeyword::Starred(value) => format!("**{}", render_typed_expr(value)),
    }
}

pub(super) fn render_instr_typed_metadata_index(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> String {
    struct MetadataRenderer<'a> {
        out: &'a mut String,
        block_label: Option<BlockLabel>,
        ordinal: usize,
    }

    impl Visit<InstrTyped> for MetadataRenderer<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let block_label = self
                .block_label
                .map(|label| label.to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            let instr_id = expr
                .try_semantic_instr_id()
                .map(|instr_id| instr_id.to_string())
                .unwrap_or_else(|| "<synthetic>".to_string());
            self.out.push_str(&format!(
                "; [{}] {} {} {} {}\n",
                self.ordinal,
                block_label,
                instr_id,
                instr_typed_variant_name(expr),
                render_typed_extra_summary(expr.typed_extra())
            ));
            self.ordinal += 1;
            expr.visit_children(self);
        }
    }

    let mut out = String::new();
    let mut renderer = MetadataRenderer {
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

fn render_typed_extra_summary(extra: Option<&TypedInstrExtra>) -> String {
    let Some(extra) = extra else {
        return "extra=<none>".to_string();
    };
    if *extra == TypedInstrExtra::default() {
        return "extra=default".to_string();
    }

    let mut parts = Vec::new();
    if let Some(result_facts) = extra.result_facts {
        parts.push(format!("result={}", render_value_facts(result_facts)));
    }
    if let Some(demand) = extra.demand {
        parts.push(format!("demand={demand:?}"));
    }
    if let Some(planned_result) = extra.planned_result {
        parts.push(format!("planned={planned_result:?}"));
    }
    if let Some(plan) = &extra.indexed_global_access {
        parts.push(format!(
            "indexed_global={}.{}@{}",
            plan.module_name, plan.name, plan.expected_index
        ));
    }
    if extra.exact_list_item_access.is_some() {
        parts.push("exact_list_item".to_string());
    }
    if extra.exact_int_branch.is_some() {
        parts.push("exact_int_branch".to_string());
    }
    if extra.exact_int_return.is_some() {
        parts.push("exact_int_return".to_string());
    }
    if let Some(plan) = extra.constructor_init_plan() {
        parts.push(format!("constructor_init={}", plan.init_function_id));
    }
    if extra.guard_miss_deopt {
        parts.push("guard_miss_deopt".to_string());
    }
    if parts.is_empty() {
        "extra=default".to_string()
    } else {
        parts.join(" ")
    }
}

fn render_value_facts(facts: ValueFacts) -> String {
    match facts {
        ValueFacts::Bool(_) => "Bool".to_string(),
        ValueFacts::I32(facts) => format!("I32(sentinel={:?})", facts.sentinel),
        ValueFacts::I64(facts) => format!("I64(sentinel={:?})", facts.sentinel),
        ValueFacts::PyObj(py) => {
            let default = soac_ir_typed::PyObjFacts::unknown();
            if py == default {
                return "PyObj(unknown)".to_string();
            }
            format!(
                "PyObj(ty={:?}, truth={:?}, none={:?}, ref={:?}, prov={:?}, callable={:?})",
                py.ty, py.truthiness, py.none, py.refcount, py.provenance, py.callable
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{render_instr_typed_metadata_index, render_instr_typed_program, render_typed_expr};
    use soac_core::block_py::{BlockPyName, Load, NameLocation, ResolvedName, Store};
    use soac_ir_typed::{InstrTyped, lower_blockpy_function_to_typed};

    #[test]
    fn instr_typed_program_renderer_uses_expression_syntax() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def classify(n):\n    if n < 0:\n        return 'neg'\n    return 'pos'\n",
        )
        .expect("source should lower");
        let function =
            lower_blockpy_function_to_typed(lowered.blockpy_module.callable_defs[0].clone());

        let rendered = render_instr_typed_program(&function);

        assert!(rendered.contains("function classify(n):"), "{rendered}");
        assert!(rendered.contains("if BinOp("), "{rendered}");
        assert!(rendered.contains("return "), "{rendered}");
        assert!(!rendered.contains("BlockPyFunction {"), "{rendered}");
    }

    #[test]
    fn instr_typed_expr_renderer_keeps_nested_calls_readable() {
        let expr = InstrTyped::Store(Store::new(
            ResolvedName {
                id: BlockPyName::new("x"),
                location: NameLocation::local(0),
            },
            InstrTyped::GetAttrTyped(soac_ir_typed::TypedGetAttr::generic(
                InstrTyped::Load(Load::new(ResolvedName {
                    id: BlockPyName::new("y"),
                    location: NameLocation::local(1),
                })),
                InstrTyped::Load(Load::new(ResolvedName {
                    id: BlockPyName::new("z"),
                    location: NameLocation::constant(0),
                })),
            )),
        ));

        assert_eq!(render_typed_expr(&expr), "Store(x, GetAttr(y, z))");
    }

    #[test]
    fn instr_typed_metadata_renderer_is_compact_index() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def classify(n):\n    return n + 1\n",
        )
        .expect("source should lower");
        let function =
            lower_blockpy_function_to_typed(lowered.blockpy_module.callable_defs[0].clone());

        let rendered = render_instr_typed_metadata_index(&function);

        assert!(rendered.contains("; [0] "), "{rendered}");
        assert!(!rendered.contains("TypedInstrExtra {"), "{rendered}");
    }
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
            let has_semantic_name = annotation
                .map(|annotation| annotation.semantic_name != token)
                .unwrap_or(false);
            if param_types.is_empty() && !has_semantic_name {
                if chunk.ends_with('\n') {
                    out.push('\n');
                }
                continue;
            }
            let param_names = annotation.map(|annotation| annotation.param_names.as_slice());
            out.push_str(" ; block ");
            out.push_str(semantic_name);
            if !param_types.is_empty() {
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
        }
        if chunk.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[derive(Debug)]
struct ClifBlockNestLayout {
    root_blocks: Vec<ir::Block>,
    child_blocks: HashMap<ir::Block, Vec<ir::Block>>,
    token_to_block: HashMap<String, ir::Block>,
}

impl ClifBlockNestLayout {
    fn new(func: &ir::Function) -> Self {
        let blocks = func.layout.blocks().collect::<Vec<_>>();
        if blocks.is_empty() {
            return Self {
                root_blocks: Vec::new(),
                child_blocks: HashMap::new(),
                token_to_block: HashMap::new(),
            };
        }

        let token_to_block = blocks
            .iter()
            .copied()
            .map(|block| (block.to_string(), block))
            .collect::<HashMap<_, _>>();
        let block_to_index = blocks
            .iter()
            .copied()
            .enumerate()
            .map(|(index, block)| (block, index))
            .collect::<HashMap<_, _>>();
        let successors = blocks
            .iter()
            .copied()
            .map(|block| collect_clif_successor_indices(func, block, &block_to_index))
            .collect::<Vec<_>>();
        let predecessors = collect_clif_predecessors(&successors);
        let entry_index = 0;
        let discovery_order = collect_clif_discovery_order(entry_index, &successors);
        let reachable = discovery_order.iter().copied().collect::<HashSet<_>>();
        let dominators =
            compute_clif_dominators(entry_index, &discovery_order, &predecessors, &reachable);
        let immediate_dominators = compute_clif_immediate_dominators(
            entry_index,
            &discovery_order,
            &dominators,
            &reachable,
        );

        let mut child_blocks: HashMap<ir::Block, Vec<ir::Block>> = HashMap::new();
        for (block_index, immediate_dominator) in immediate_dominators.iter().enumerate() {
            if let Some(parent_index) = immediate_dominator {
                child_blocks
                    .entry(blocks[*parent_index])
                    .or_default()
                    .push(blocks[block_index]);
            }
        }

        let mut root_blocks = Vec::new();
        root_blocks.push(blocks[entry_index]);
        for block_index in discovery_order {
            if block_index != entry_index && immediate_dominators[block_index].is_none() {
                root_blocks.push(blocks[block_index]);
            }
        }
        for (block_index, block) in blocks.iter().copied().enumerate() {
            if !reachable.contains(&block_index) {
                root_blocks.push(block);
            }
        }

        Self {
            root_blocks,
            child_blocks,
            token_to_block,
        }
    }
}

fn collect_clif_successor_indices(
    func: &ir::Function,
    block: ir::Block,
    block_to_index: &HashMap<ir::Block, usize>,
) -> Vec<usize> {
    let mut successors = Vec::new();
    let Some(inst) = func.layout.last_inst(block) else {
        return successors;
    };
    for destination in
        func.dfg.insts[inst].branch_destination(&func.dfg.jump_tables, &func.dfg.exception_tables)
    {
        let target = destination.block(&func.dfg.value_lists);
        let Some(target_index) = block_to_index.get(&target).copied() else {
            continue;
        };
        if !successors.contains(&target_index) {
            successors.push(target_index);
        }
    }
    successors
}

fn collect_clif_predecessors(successors: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); successors.len()];
    for (block_index, block_successors) in successors.iter().enumerate() {
        for successor in block_successors {
            if !predecessors[*successor].contains(&block_index) {
                predecessors[*successor].push(block_index);
            }
        }
    }
    predecessors
}

fn collect_clif_discovery_order(entry_index: usize, successors: &[Vec<usize>]) -> Vec<usize> {
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![entry_index];
    while let Some(block_index) = stack.pop() {
        if !seen.insert(block_index) {
            continue;
        }
        order.push(block_index);
        for successor in successors[block_index].iter().rev() {
            stack.push(*successor);
        }
    }
    order
}

fn compute_clif_dominators(
    entry_index: usize,
    discovery_order: &[usize],
    predecessors: &[Vec<usize>],
    reachable: &HashSet<usize>,
) -> Vec<HashSet<usize>> {
    let mut dominators = vec![HashSet::new(); predecessors.len()];
    for block_index in 0..predecessors.len() {
        if reachable.contains(&block_index) {
            dominators[block_index] = reachable.clone();
        } else {
            dominators[block_index].insert(block_index);
        }
    }
    dominators[entry_index].clear();
    dominators[entry_index].insert(entry_index);

    loop {
        let mut changed = false;
        for block_index in discovery_order
            .iter()
            .copied()
            .filter(|block_index| *block_index != entry_index)
        {
            let mut reachable_predecessors = predecessors[block_index]
                .iter()
                .copied()
                .filter(|predecessor| reachable.contains(predecessor));
            let Some(first_predecessor) = reachable_predecessors.next() else {
                let mut singleton = HashSet::new();
                singleton.insert(block_index);
                if dominators[block_index] != singleton {
                    dominators[block_index] = singleton;
                    changed = true;
                }
                continue;
            };

            let mut new_dominators = dominators[first_predecessor].clone();
            for predecessor in reachable_predecessors {
                new_dominators = new_dominators
                    .intersection(&dominators[predecessor])
                    .copied()
                    .collect();
            }
            new_dominators.insert(block_index);

            if dominators[block_index] != new_dominators {
                dominators[block_index] = new_dominators;
                changed = true;
            }
        }

        if !changed {
            return dominators;
        }
    }
}

fn compute_clif_immediate_dominators(
    entry_index: usize,
    discovery_order: &[usize],
    dominators: &[HashSet<usize>],
    reachable: &HashSet<usize>,
) -> Vec<Option<usize>> {
    let mut immediate_dominators = vec![None; dominators.len()];
    for block_index in discovery_order
        .iter()
        .copied()
        .filter(|block_index| *block_index != entry_index)
    {
        let strict_dominators = dominators[block_index]
            .iter()
            .copied()
            .filter(|dominator| *dominator != block_index && reachable.contains(dominator))
            .collect::<Vec<_>>();
        let immediate_dominator = strict_dominators.iter().copied().find(|candidate| {
            strict_dominators
                .iter()
                .all(|other| *other == *candidate || dominators[*candidate].contains(other))
        });
        immediate_dominators[block_index] = immediate_dominator;
    }
    immediate_dominators
}

fn parse_clif_block_header_token(line: &str) -> Option<&str> {
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
    if !matches!(bytes.get(token_end), Some(b'(' | b' ' | b':')) {
        return None;
    }
    let colon_pos = line[token_end..].find(':')?;
    let before_colon = &line[token_end..(token_end + colon_pos)];
    if before_colon.contains(';') {
        return None;
    }
    Some(&line[..token_end])
}

fn push_clif_line_with_depth(out: &mut String, line: &str, depth: usize) {
    if !line.is_empty() {
        for _ in 0..depth {
            out.push_str("    ");
        }
        out.push_str(line);
    }
    out.push('\n');
}

fn push_nested_clif_block(
    out: &mut String,
    token: &str,
    depth: usize,
    layout: &ClifBlockNestLayout,
    chunks: &HashMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
) {
    if !visited.insert(token.to_string()) {
        return;
    }
    let Some(lines) = chunks.get(token) else {
        return;
    };
    for line in lines {
        push_clif_line_with_depth(out, line, depth);
    }
    let Some(block) = layout.token_to_block.get(token) else {
        return;
    };
    if let Some(children) = layout.child_blocks.get(block) {
        for child in children {
            push_nested_clif_block(
                out,
                child.to_string().as_str(),
                depth + 1,
                layout,
                chunks,
                visited,
            );
        }
    }
}

pub(super) fn nest_clif_blocks_by_nearest_dominator(func: &ir::Function, clif: &str) -> String {
    let layout = ClifBlockNestLayout::new(func);
    if layout.root_blocks.is_empty() {
        return clif.to_string();
    }

    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut chunks: HashMap<String, Vec<String>> = HashMap::new();
    let mut block_order = Vec::new();
    let mut current_token: Option<String> = None;
    let mut current_lines = Vec::new();

    for line in clif.lines() {
        if let Some(token) = parse_clif_block_header_token(line) {
            if let Some(token) = current_token.take() {
                block_order.push(token.clone());
                chunks.insert(token, std::mem::take(&mut current_lines));
            }
            current_token = Some(token.to_string());
            current_lines.push(line.to_string());
            continue;
        }

        if line == "}" && current_token.is_some() {
            if let Some(token) = current_token.take() {
                block_order.push(token.clone());
                chunks.insert(token, std::mem::take(&mut current_lines));
            }
            suffix.push(line.to_string());
            continue;
        }

        if current_token.is_some() {
            current_lines.push(line.to_string());
        } else if suffix.is_empty() {
            prefix.push(line.to_string());
        } else {
            suffix.push(line.to_string());
        }
    }

    if let Some(token) = current_token {
        block_order.push(token.clone());
        chunks.insert(token, current_lines);
    }

    if chunks.is_empty() {
        return clif.to_string();
    }

    let mut out = String::with_capacity(clif.len());
    for line in prefix {
        out.push_str(&line);
        out.push('\n');
    }

    let mut visited = HashSet::new();
    for block in &layout.root_blocks {
        push_nested_clif_block(
            &mut out,
            block.to_string().as_str(),
            0,
            &layout,
            &chunks,
            &mut visited,
        );
    }
    for token in block_order {
        if !visited.contains(&token) {
            push_nested_clif_block(&mut out, &token, 0, &layout, &chunks, &mut visited);
        }
    }

    for line in suffix {
        out.push_str(&line);
        out.push('\n');
    }

    out
}

pub fn run_cranelift_smoke(module: &BlockPyModule<BlockPyModuleShape>) -> Result<(), String> {
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
    function_aliases: &ClifFunctionDisplayAliases,
    block_annotations: &ClifBlockDisplayAnnotations,
) -> String {
    let mut clif = String::new();
    clif.push_str("; ---- pre-inlining CLIF for inspection ----\n");
    clif.push_str(
        "; emitted after SOAC typed codegen and before runtime support CLIF inlining and Cranelift optimization\n",
    );
    clif.push_str("; instructions are annotated with inferred JIT emission purpose\n");
    let clif_display =
        rewrite_clif_function_aliases(func.display().to_string().as_str(), function_aliases);
    let annotated = rewrite_block_header_annotations(&clif_display, block_annotations);
    let annotated = annotate_clif_instruction_purposes(func, &annotated, function_aliases);
    clif.push_str(&nest_clif_blocks_by_nearest_dominator(func, &annotated));
    clif
}

pub(super) fn render_compiled_clif_and_vcode_disasm(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    mut ctx: cranelift_codegen::Context,
    function_aliases: &ClifFunctionDisplayAliases,
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
    clif.push_str("; blocks are nested by nearest dominator for inspection\n");
    clif.push_str("; instructions are annotated with inferred JIT emission purpose\n");
    let clif_display = rewrite_clif_function_aliases(
        display_func.display().to_string().as_str(),
        function_aliases,
    );
    let annotated = rewrite_block_header_annotations(&clif_display, block_annotations);
    let annotated = annotate_clif_instruction_purposes(&display_func, &annotated, function_aliases);
    clif.push_str(&nest_clif_blocks_by_nearest_dominator(
        &display_func,
        &annotated,
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
