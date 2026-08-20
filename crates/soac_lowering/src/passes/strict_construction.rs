//! Resolve compiler-created class construction calls before optimization.
//!
//! Neither user spelling nor a runtime helper's current value grants authority.
//! Only the source role assigned by the class rewrite may introduce this node;
//! the runtime still authenticates the actual function and module execution.

use std::collections::HashMap;

use soac_contracts::SourceIdentity;
use soac_core::block_py::{
    BlockPyModule, CallArgPositional, CallableSourceRole, ClassBindingExportKind, ConstantExpr,
    ConstructClass, HasMeta, MapFunction, MapInstr, Mappable, NameLocation, ResolvedName,
    RuntimeFunctionId, RuntimeName, WithMeta,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};

struct ResolveConstruction<'a> {
    definition: &'a SourceIdentity,
    function: RuntimeFunctionId,
    constants: &'a [ConstantExpr],
    requires_class_cell: bool,
    requires_class_dict_cell: bool,
    resolved: usize,
}

impl ResolveConstruction<'_> {
    fn runtime_name(&self, expr: &InstrBlockPy) -> Option<RuntimeName> {
        let InstrBlockPy::Load(load) = expr else {
            return None;
        };
        match load.name.location {
            NameLocation::RuntimeName(name) => Some(name),
            NameLocation::Constant(index) => match self.constants.get(index as usize) {
                Some(ConstantExpr::RuntimeName(name)) => Some(*name),
                _ => None,
            },
            _ => None,
        }
    }
}

impl MapInstr<InstrBlockPy, InstrBlockPy> for ResolveConstruction<'_> {
    fn map_instr(&mut self, instr: InstrBlockPy) -> InstrBlockPy {
        let instr = instr.map_same_children(self);
        let InstrBlockPy::Call(call) = &instr else {
            return instr;
        };
        if self.runtime_name(&call.func) != Some(RuntimeName::CreateClass) {
            return instr;
        }
        let InstrBlockPy::Call(call) = instr else {
            unreachable!()
        };
        assert!(
            call.keywords.is_empty() && matches!(call.args.len(), 7 | 8),
            "compiler-owned strict class construction has an invalid operand shape"
        );
        let meta = call.meta();
        let mut operands = call.args.into_iter().map(|arg| match arg {
            CallArgPositional::Positional(value) => value,
            CallArgPositional::Starred(_) => {
                panic!("strict construction operands cannot be expanded")
            }
        });
        self.resolved += 1;
        let name = operands.next().unwrap();
        let namespace_function = operands.next().unwrap();
        let bases = operands.next().unwrap();
        let keywords = operands.next().unwrap();
        let cell_requirement = operands.next().unwrap();
        let dictionary_requirement = operands.next().unwrap();
        for (operand, required) in [
            (&cell_requirement, self.requires_class_cell),
            (&dictionary_requirement, self.requires_class_dict_cell),
        ] {
            assert_eq!(
                self.runtime_name(operand),
                Some(if required {
                    RuntimeName::True
                } else {
                    RuntimeName::False
                }),
                "compiler class-cell declaration differs from the canonical native export recipe",
            );
        }
        ConstructClass::new(
            self.definition.clone(),
            self.function,
            name,
            namespace_function,
            bases,
            keywords,
            cell_requirement,
            dictionary_requirement,
            operands.next().unwrap(),
            operands.next().map(Box::new),
        )
        .with_meta(meta)
        .into()
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

pub(crate) fn resolve_strict_construction(module: &mut BlockPyModule<BlockPyModuleShape>) {
    if module.strict_source.is_none() {
        return;
    }
    // The native export recipe is the declaration authority. Current raw
    // carriers and escaped captures may use different physical binding names;
    // neither their spelling nor a scan of currently present CellRef uses can
    // decide whether the original class exports a cell.
    let mut namespace_cells = HashMap::new();
    for function in &module.callable_defs {
        let Some(origin) = function.scope.source_origin.as_ref() else {
            continue;
        };
        if origin.role != CallableSourceRole::ClassNamespace {
            continue;
        }
        let bindings = function
            .scope
            .class_bindings
            .as_ref()
            .expect("strict class namespace requires its canonical native binding recipe");
        assert_eq!(bindings.source, origin.definition);
        let exports = |kind| {
            bindings
                .recipe
                .exports
                .iter()
                .any(|export| export.kind == kind)
        };
        let requirements = (
            exports(ClassBindingExportKind::ClassCell),
            exports(ClassBindingExportKind::ClassDictCell),
        );
        assert!(
            namespace_cells
                .insert(origin.definition.clone(), requirements)
                .is_none(),
            "class construction must identify one actual namespace helper"
        );
    }
    for function in &mut module.callable_defs {
        let Some(origin) = function.scope.source_origin.as_ref() else {
            continue;
        };
        if origin.role != CallableSourceRole::ClassConstruction {
            continue;
        }
        let definition = origin.definition.clone();
        let (requires_class_cell, requires_class_dict_cell) = *namespace_cells
            .get(&definition)
            .expect("class construction must have its matching namespace helper");
        let mut resolve = ResolveConstruction {
            definition: &definition,
            function: function.function_id,
            constants: &module.module_constants,
            requires_class_cell,
            requires_class_dict_cell,
            resolved: 0,
        };
        *function = resolve.map_fn(function.clone());
        assert_eq!(
            resolve.resolved, 1,
            "each strict class-construction helper must resolve exactly one intrinsic"
        );
    }
}
