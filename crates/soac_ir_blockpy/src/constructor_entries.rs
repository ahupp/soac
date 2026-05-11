use crate::{BlockPyModuleShape, InstrBlockPy, assign_missing_blockpy_function_instr_ids};
use soac_core::block_py::{
    Block, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional,
    CallableScopeInfo, FunctionExecutionMode, FunctionKind, FunctionName, KeywordName, Load,
    LocalFunctionId, NameLocation, Param, ParamKind, ParamSpec, ResolvedName, RuntimeFunctionId,
    RuntimeModuleId, RuntimeName, StorageLayout,
};
use std::collections::HashSet;

pub const CONSTRUCTOR_ENTRY_FUNCTION_NAME: &str = "__soac_constructor_entry__";
pub const CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME: &str = "_dp_constructor_type";

fn class_qualname_for_init_qualname(init_qualname: &str) -> Option<&str> {
    init_qualname.strip_suffix(".__init__")
}

fn constructor_entry_qualname_for_init(
    class_qualname: &str,
    init_function_id: RuntimeFunctionId,
) -> String {
    format!("{class_qualname}.{CONSTRUCTOR_ENTRY_FUNCTION_NAME}#{init_function_id}")
}

pub fn constructor_entry_function_id_for_init(
    module: &BlockPyModule<BlockPyModuleShape>,
    init_function_id: RuntimeFunctionId,
) -> Option<RuntimeFunctionId> {
    let init_function = module
        .callable_defs
        .iter()
        .find(|function| function.function_id == init_function_id)?;
    let class_qualname = class_qualname_for_init_qualname(&init_function.names.qualname)?;
    let constructor_qualname =
        constructor_entry_qualname_for_init(class_qualname, init_function_id);
    module
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == constructor_qualname)
        .map(|function| function.function_id)
}

pub fn is_constructor_entry_function(
    function: &BlockPyFunction<impl soac_core::block_py::ModuleShape>,
) -> bool {
    function.names.fn_name == CONSTRUCTOR_ENTRY_FUNCTION_NAME
}

pub fn constructor_init_function_id_for_entry_function(
    function: &BlockPyFunction<impl soac_core::block_py::ModuleShape>,
) -> Option<RuntimeFunctionId> {
    if !is_constructor_entry_function(function) {
        return None;
    }
    let encoded = function.names.qualname.rsplit_once('#')?.1;
    let (module_id, local_id) = encoded.split_once(':')?;
    Some(RuntimeFunctionId::new(
        RuntimeModuleId::new(module_id.parse().ok()?),
        LocalFunctionId::new(local_id.parse().ok()?),
    ))
}

fn constructor_entry_bind_name(init_function_id: RuntimeFunctionId) -> String {
    format!(
        "_dp_constructor_entry_{}_{}",
        init_function_id.runtime_module_id().as_u32(),
        init_function_id.local_function_id().as_u32()
    )
}

fn constructor_entry_params(init_function: &BlockPyFunction<BlockPyModuleShape>) -> ParamSpec {
    let mut params = Vec::with_capacity(init_function.params.len());
    params.push(Param {
        name: CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME.to_string(),
        kind: ParamKind::PosOnly,
        has_default: false,
    });
    params.extend(init_function.params.params.iter().skip(1).cloned());
    ParamSpec { params }
}

fn constructor_entry_scope(names: FunctionName) -> CallableScopeInfo {
    CallableScopeInfo {
        names,
        ..CallableScopeInfo::default()
    }
}

fn local_name(name: impl Into<String>, slot: u32) -> ResolvedName {
    ResolvedName {
        id: name.into().into(),
        location: NameLocation::local(slot),
    }
}

fn runtime_name(name: RuntimeName) -> ResolvedName {
    ResolvedName {
        id: name.name().into(),
        location: NameLocation::runtime_name(name),
    }
}

fn constructor_entry_term(params: &ParamSpec) -> BlockTerm<InstrBlockPy> {
    let callable: InstrBlockPy =
        Load::<InstrBlockPy>::new(runtime_name(RuntimeName::ConstructorCall)).into();
    let mut positional_args = Vec::new();
    let mut keyword_args = Vec::new();
    for (slot, param) in params.params.iter().enumerate() {
        let value: InstrBlockPy =
            Load::<InstrBlockPy>::new(local_name(param.name.clone(), slot as u32)).into();
        match param.kind {
            ParamKind::Any | ParamKind::PosOnly => {
                positional_args.push(CallArgPositional::Positional(value));
            }
            ParamKind::KwOnly => {
                keyword_args.push(CallArgKeyword::Named {
                    arg: KeywordName::new(param.name.clone()),
                    value,
                });
            }
            ParamKind::VarArg => {
                positional_args.push(CallArgPositional::Starred(value));
            }
            ParamKind::KwArg => {
                keyword_args.push(CallArgKeyword::Starred(value));
            }
        }
    }
    BlockTerm::Return(Call::new(callable, positional_args, keyword_args).into())
}

fn constructor_entry_storage_layout(
    init_function: &BlockPyFunction<BlockPyModuleShape>,
    params: &ParamSpec,
) -> StorageLayout {
    let mut layout = StorageLayout::default();
    layout.set_stack_slots(params.names());
    if let Some(init_layout) = init_function.storage_layout.as_ref() {
        layout.freevars = init_layout.freevars.clone();
    }
    layout
}

fn build_constructor_entry_function(
    module: &BlockPyModule<BlockPyModuleShape>,
    init_function: &BlockPyFunction<BlockPyModuleShape>,
    class_qualname: &str,
) -> BlockPyFunction<BlockPyModuleShape> {
    let name_gen = module.module_name_gen.next_function_name_gen();
    let function_id = name_gen.function_id();
    let label = name_gen.next_block_name();
    let qualname = constructor_entry_qualname_for_init(class_qualname, init_function.function_id);
    let names = FunctionName::new(
        constructor_entry_bind_name(init_function.function_id),
        CONSTRUCTOR_ENTRY_FUNCTION_NAME,
        CONSTRUCTOR_ENTRY_FUNCTION_NAME,
        qualname,
    );
    let params = constructor_entry_params(init_function);
    let storage_layout = constructor_entry_storage_layout(init_function, &params);
    let term = constructor_entry_term(&params);
    BlockPyFunction {
        function_id,
        name_gen,
        names: names.clone(),
        kind: FunctionKind::Function,
        execution_mode: FunctionExecutionMode::Jit,
        params,
        body_params: None,
        public_scope: None,
        blocks: vec![Block {
            label,
            body: Vec::new(),
            term,
            params: Vec::new(),
            exc_edge: None,
            extra: Default::default(),
        }],
        doc: None,
        public_storage_layout: None,
        storage_layout: Some(storage_layout),
        scope: constructor_entry_scope(names),
    }
}

pub fn ensure_constructor_entry_functions(module: &mut BlockPyModule<BlockPyModuleShape>) -> usize {
    let mut existing_qualnames = module
        .callable_defs
        .iter()
        .map(|function| function.names.qualname.clone())
        .collect::<HashSet<_>>();
    let init_functions = module
        .callable_defs
        .iter()
        .filter_map(|function| {
            if function.names.fn_name != "__init__" {
                return None;
            }
            let class_qualname = class_qualname_for_init_qualname(&function.names.qualname)?;
            Some((function.clone(), class_qualname.to_string()))
        })
        .collect::<Vec<_>>();

    let mut inserted = 0;
    for (init_function, class_qualname) in init_functions {
        let constructor_qualname =
            constructor_entry_qualname_for_init(&class_qualname, init_function.function_id);
        if !existing_qualnames.insert(constructor_qualname) {
            continue;
        }
        let mut constructor_entry =
            build_constructor_entry_function(module, &init_function, &class_qualname);
        assign_missing_blockpy_function_instr_ids(&mut constructor_entry);
        module.callable_defs.push(constructor_entry);
        inserted += 1;
    }
    inserted
}
