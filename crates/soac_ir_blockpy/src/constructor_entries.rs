use crate::{CodegenModuleShape, InstrCodegen, assign_missing_codegen_function_instr_ids};
use soac_core::block_py::{
    Block, BlockPyFunction, BlockPyModule, BlockTerm, CallableScopeInfo, FunctionExecutionMode,
    FunctionKind, FunctionName, ParamSpec, RuntimeFunctionId,
};
use std::collections::HashSet;

pub const CONSTRUCTOR_ENTRY_FUNCTION_NAME: &str = "__soac_constructor_entry__";

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
    module: &BlockPyModule<CodegenModuleShape>,
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

fn constructor_entry_bind_name(init_function_id: RuntimeFunctionId) -> String {
    format!(
        "_dp_constructor_entry_{}_{}",
        init_function_id.runtime_module_id().as_u32(),
        init_function_id.local_function_id().as_u32()
    )
}

fn constructor_entry_params(init_function: &BlockPyFunction<CodegenModuleShape>) -> ParamSpec {
    ParamSpec {
        params: init_function
            .params
            .params
            .iter()
            .skip(1)
            .cloned()
            .collect(),
    }
}

fn constructor_entry_scope(names: FunctionName) -> CallableScopeInfo {
    CallableScopeInfo {
        names,
        ..CallableScopeInfo::default()
    }
}

fn build_constructor_entry_function(
    module: &BlockPyModule<CodegenModuleShape>,
    init_function: &BlockPyFunction<CodegenModuleShape>,
    class_qualname: &str,
) -> BlockPyFunction<CodegenModuleShape> {
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
    BlockPyFunction {
        function_id,
        name_gen,
        names: names.clone(),
        kind: FunctionKind::Function,
        execution_mode: FunctionExecutionMode::Interpreted,
        params: constructor_entry_params(init_function),
        blocks: vec![Block {
            label,
            body: Vec::new(),
            term: BlockTerm::<InstrCodegen>::implicit_function_return(),
            params: Vec::new(),
            exc_edge: None,
            extra: Default::default(),
        }],
        doc: None,
        storage_layout: None,
        scope: constructor_entry_scope(names),
    }
}

pub fn ensure_constructor_entry_functions(module: &mut BlockPyModule<CodegenModuleShape>) -> usize {
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
        assign_missing_codegen_function_instr_ids(&mut constructor_entry);
        module.callable_defs.push(constructor_entry);
        inserted += 1;
    }
    inserted
}
