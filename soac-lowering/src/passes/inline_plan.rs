use crate::block_py::BlockPyPrettyPrint;
use crate::block_py::{LocalLocation, RuntimeFunctionId};
use crate::passes::{
    ConstructorFieldStore, ConstructorFieldValue, EscapeSummaryModule,
    FieldInitializerConstructorSummary, NonEscapingConstructorAllocationSummary,
};
use std::collections::HashMap;

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct InlinePlanModule {
    pub functions: HashMap<RuntimeFunctionId, FunctionInlinePlan>,
}

impl InlinePlanModule {
    pub fn function(&self, function_id: RuntimeFunctionId) -> Option<&FunctionInlinePlan> {
        self.functions.get(&function_id)
    }

    pub fn straightline_constructor(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<&StraightlineConstructorInlinePlan> {
        self.function(function_id)
            .and_then(|plan| plan.straightline_constructor.as_ref())
    }

    pub fn non_escaping_constructor_allocations(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<&[NonEscapingConstructorAllocationSummary]> {
        self.function(function_id)
            .map(|plan| plan.non_escaping_constructor_allocations.as_slice())
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        self.functions = std::mem::take(&mut self.functions)
            .into_iter()
            .map(|(function_id, mut plan)| {
                plan.remap_function_ids(remap);
                (remap(function_id), plan)
            })
            .collect();
    }
}

impl BlockPyPrettyPrint for InlinePlanModule {
    fn pretty_print(&self) -> String {
        let mut function_ids = self.functions.keys().copied().collect::<Vec<_>>();
        function_ids.sort_by_key(|function_id| function_id.to_packed_runtime_u64());
        let mut out = String::new();
        for function_id in function_ids {
            let plan = self
                .functions
                .get(&function_id)
                .expect("function id was collected from this inline plan map");
            if let Some(constructor) = &plan.straightline_constructor {
                out.push_str(&format!(
                    "{function_id}: straightline_constructor self={} fields={}\n",
                    constructor.self_name,
                    render_field_stores(&constructor.field_stores),
                ));
            }
            if !plan.non_escaping_constructor_allocations.is_empty() {
                out.push_str(&format!(
                    "{function_id}: non_escaping_constructor_allocations count={}\n",
                    plan.non_escaping_constructor_allocations.len(),
                ));
            }
        }
        if out.is_empty() {
            "; no inline candidates\n".to_string()
        } else {
            out
        }
    }
}

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionInlinePlan {
    pub straightline_constructor: Option<StraightlineConstructorInlinePlan>,
    pub non_escaping_constructor_allocations: Vec<NonEscapingConstructorAllocationSummary>,
}

impl FunctionInlinePlan {
    fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        for allocation in &mut self.non_escaping_constructor_allocations {
            allocation.constructor_function_id = remap(allocation.constructor_function_id);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StraightlineConstructorInlinePlan {
    pub self_name: String,
    pub self_location: Option<LocalLocation>,
    pub field_stores: Vec<ConstructorFieldStore>,
}

pub fn plan_module_inlining(escape_summary: &EscapeSummaryModule) -> InlinePlanModule {
    let functions = escape_summary
        .functions
        .iter()
        .filter_map(|(function_id, summary)| {
            let straightline_constructor = summary
                .straightline_field_initializer
                .as_ref()
                .map(straightline_constructor_plan);
            if straightline_constructor.is_none()
                && summary.non_escaping_constructor_allocations.is_empty()
            {
                return None;
            }
            Some((
                *function_id,
                FunctionInlinePlan {
                    straightline_constructor,
                    non_escaping_constructor_allocations: summary
                        .non_escaping_constructor_allocations
                        .clone(),
                },
            ))
        })
        .collect();
    InlinePlanModule { functions }
}

fn straightline_constructor_plan(
    summary: &FieldInitializerConstructorSummary,
) -> StraightlineConstructorInlinePlan {
    StraightlineConstructorInlinePlan {
        self_name: summary.self_name.clone(),
        self_location: summary.self_location,
        field_stores: summary.field_stores.clone(),
    }
}

fn render_field_stores(stores: &[ConstructorFieldStore]) -> String {
    stores
        .iter()
        .map(|store| format!("{}={}", store.field_name, render_field_value(&store.value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_field_value(value: &ConstructorFieldValue) -> String {
    match value {
        ConstructorFieldValue::Param { name, index, .. } => format!("param#{index}:{name}"),
        ConstructorFieldValue::Local { name, .. } => format!("local:{name}"),
        ConstructorFieldValue::Constant { description } => format!("const:{description}"),
        ConstructorFieldValue::Other => "other".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower_python_to_blockpy_for_testing;

    #[test]
    fn plans_straightline_constructor_from_escape_summary() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
class IterRange:
    def __init__(self, start, stop, step, /):
        self.current = start
        self.stop = stop
        self.step = step
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "IterRange.__init__")
            .expect("constructor should be present");
        let escape_summary = crate::passes::summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        let constructor = inline_plan
            .straightline_constructor(function.function_id)
            .expect("field-only constructor should be an inline candidate");
        let fields = constructor
            .field_stores
            .iter()
            .map(|store| store.field_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(fields, ["current", "stop", "step"]);
    }

    #[test]
    fn omits_constructor_with_control_flow() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        if value is None:
            value = 0
        self.value = value
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "Box.__init__")
            .expect("constructor should be present");
        let escape_summary = crate::passes::summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        assert!(inline_plan
            .straightline_constructor(function.function_id)
            .is_none());
    }

    #[test]
    fn carries_non_escaping_constructor_allocations() {
        let caller_id = RuntimeFunctionId::from_raw_parts(1, 10);
        let constructor_id = RuntimeFunctionId::from_raw_parts(1, 20);
        let escape_summary = EscapeSummaryModule {
            functions: HashMap::from([(
                caller_id,
                crate::passes::FunctionEscapeSummary {
                    non_escaping_constructor: None,
                    straightline_field_initializer: None,
                    non_escaping_constructor_allocations: vec![
                        NonEscapingConstructorAllocationSummary {
                            local_name: "box".to_string(),
                            local_location: LocalLocation(0),
                            constructor_function_id: constructor_id,
                            call_instr_id: None,
                            field_reads: vec![crate::passes::ConstructorFieldAccess {
                                field_name: "value".to_string(),
                            }],
                            field_writes: Vec::new(),
                        },
                    ],
                },
            )]),
        };

        let inline_plan = plan_module_inlining(&escape_summary);
        let allocations = inline_plan
            .non_escaping_constructor_allocations(caller_id)
            .expect("caller should have an inline plan");

        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations[0].constructor_function_id, constructor_id);
        assert_eq!(allocations[0].field_reads[0].field_name, "value");
    }
}
