use crate::passes::{CodegenModuleShape, InlinePlanModule, InstrCodegen, InstrCodegenOp};
use soac_core::block_py::{
    BlockLabel, BlockPyModule, CallArgKeyword, CallArgPositional, ChildVisitable, HasMeta, InstrId,
    RuntimeFunctionId, Visit, walk_block, walk_expr,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InlineCallSiteModule {
    pub straightline_constructor_calls: Vec<StraightlineConstructorCallSite>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StraightlineConstructorCallSite {
    pub caller_function_id: RuntimeFunctionId,
    pub callee_function_id: RuntimeFunctionId,
    pub block_label: BlockLabel,
    pub instr_id: Option<InstrId>,
    pub positional_arg_count: usize,
    pub keyword_arg_count: usize,
    pub has_starred_args: bool,
    pub has_starred_keywords: bool,
}

pub fn collect_inline_call_sites(
    module: &BlockPyModule<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
) -> InlineCallSiteModule {
    let mut sites = InlineCallSiteModule::default();
    for function in &module.callable_defs {
        for block in &function.blocks {
            let mut collector = InlineCallSiteCollector {
                inline_plan,
                sites: &mut sites,
                caller_function_id: function.function_id,
                block_label: block.label,
            };
            collector.visit_block(block);
        }
    }
    sites
}

struct InlineCallSiteCollector<'a> {
    inline_plan: &'a InlinePlanModule,
    sites: &'a mut InlineCallSiteModule,
    caller_function_id: RuntimeFunctionId,
    block_label: BlockLabel,
}

impl Visit<InstrCodegen> for InlineCallSiteCollector<'_> {
    fn visit_instr(&mut self, expr: &InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        if let InstrCodegenOp::CallDirect(call) = expr {
            if self
                .inline_plan
                .straightline_constructor(call.function_id)
                .is_some()
            {
                self.sites
                    .straightline_constructor_calls
                    .push(StraightlineConstructorCallSite {
                        caller_function_id: self.caller_function_id,
                        callee_function_id: call.function_id,
                        block_label: self.block_label,
                        instr_id: call.meta().instr_id,
                        positional_arg_count: call
                            .args
                            .iter()
                            .filter(|arg| matches!(arg, CallArgPositional::Positional(_)))
                            .count(),
                        keyword_arg_count: call
                            .keywords
                            .iter()
                            .filter(|arg| matches!(arg, CallArgKeyword::Named { .. }))
                            .count(),
                        has_starred_args: call
                            .args
                            .iter()
                            .any(|arg| matches!(arg, CallArgPositional::Starred(_))),
                        has_starred_keywords: call
                            .keywords
                            .iter()
                            .any(|arg| matches!(arg, CallArgKeyword::Starred(_))),
                    });
            }
        }
        walk_expr(self, expr);
    }

    fn visit_block(&mut self, block: &soac_core::block_py::Block<InstrCodegen>)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        self.block_label = block.label;
        walk_block(self, block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::{plan_module_inlining, summarize_module_escapes};
    use soac_core::block_py::{CallDirect, Load, NameLocation, ResolvedName};
    use soac_lowering::lower_python_to_blockpy_for_testing;

    fn function_index_by_qualname(
        module: &BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> usize {
        module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("{qualname} should be present"))
    }

    fn global_load(name: &str) -> InstrCodegen {
        Load::new(ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::GlobalName,
        })
        .into()
    }

    #[test]
    fn collects_direct_calls_to_planned_straightline_constructors() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let caller_id = module.callable_defs[make_index].function_id;
        let block_label = module.callable_defs[make_index].blocks[0].label;
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::CallDirect(CallDirect::new(
                global_load("Box"),
                constructor_id,
                vec![CallArgPositional::Positional(global_load("x"))],
                Vec::new(),
            )));
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&module));

        let sites = collect_inline_call_sites(&module, &inline_plan);

        assert_eq!(sites.straightline_constructor_calls.len(), 1);
        let site = &sites.straightline_constructor_calls[0];
        assert_eq!(site.caller_function_id, caller_id);
        assert_eq!(site.callee_function_id, constructor_id);
        assert_eq!(site.block_label, block_label);
        assert_eq!(site.positional_arg_count, 1);
        assert_eq!(site.keyword_arg_count, 0);
        assert!(!site.has_starred_args);
        assert!(!site.has_starred_keywords);
    }
}
