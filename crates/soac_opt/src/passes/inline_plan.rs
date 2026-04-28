use crate::passes::{
    ConstructorFieldStore, ConstructorFieldValue, EscapeSummaryModule,
    FieldInitializerConstructorSummary,
};
use soac_core::block_py::PrettyPrint;
use soac_core::block_py::{LocalLocation, RuntimeFunctionId};
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

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        self.functions = std::mem::take(&mut self.functions)
            .into_iter()
            .map(|(function_id, plan)| (remap(function_id), plan))
            .collect();
    }
}

impl PrettyPrint for InlinePlanModule {
    fn fmt_pretty(&self, printer: &mut soac_core::block_py::PrettyPrinter<'_>) -> std::fmt::Result {
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
        }
        if out.is_empty() {
            std::fmt::Write::write_str(printer, "; no inline candidates\n")
        } else {
            std::fmt::Write::write_str(printer, &out)
        }
    }
}

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionInlinePlan {
    pub straightline_constructor: Option<StraightlineConstructorInlinePlan>,
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
            if straightline_constructor.is_none() {
                return None;
            }
            Some((
                *function_id,
                FunctionInlinePlan {
                    straightline_constructor,
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
    use soac_lowering::lower_python_to_blockpy_for_testing;

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
        .blockpy_module;
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
        .blockpy_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "Box.__init__")
            .expect("constructor should be present");
        let escape_summary = crate::passes::summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        assert!(
            inline_plan
                .straightline_constructor(function.function_id)
                .is_none()
        );
    }
}
