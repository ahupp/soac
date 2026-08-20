//! Validate private-cell transport against exact creation edges and signed
//! field declarations before the native catalogue grants template admission.

use super::*;
use soac_core::block_py::{
    BindingKind, CellBindingKind, CellLocation, LexicalCellBinding, LexicalCellCapture,
    MakeFunctionWithClosure, NameLocation, PreservedSlotStorage,
};
use std::collections::BTreeSet;

fn demanded_cells(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> BTreeSet<(LexicalCellBinding, u32)> {
    struct Collect<'a> {
        module: &'a BlockPyModule<BlockPyModuleShape>,
        cells: BTreeSet<(LexicalCellBinding, u32)>,
    }
    impl Visit<InstrBlockPy> for Collect<'_> {
        fn visit_instr(&mut self, node: &InstrBlockPy) {
            if let InstrBlockPy::MakeFunctionWithClosure(op) = node
                && let Some(target) = self
                    .module
                    .callable_defs
                    .iter()
                    .find(|function| function.function_id == op.function_id)
            {
                let captures: Vec<_> = if let Some(scope) = &target.scope.class_construction {
                    scope.captures.iter().collect()
                } else {
                    target
                        .scope
                        .private_lexical
                        .as_ref()
                        .map_or_else(Vec::new, |scope| scope.private_captures().collect())
                };
                for capture in captures {
                    self.cells.extend(
                        capture
                            .nominal_binding_indices
                            .iter()
                            .map(|index| (capture.binding.clone(), *index)),
                    );
                }
            }
            node.visit_children(self);
        }
    }
    let mut collect = Collect {
        module,
        cells: BTreeSet::new(),
    };
    collect.visit_fn(function);
    collect.cells
}

struct Validate<'a> {
    module: &'a BlockPyModule<BlockPyModuleShape>,
    producer: &'a BlockPyFunction<BlockPyModuleShape>,
    facts: &'a soac_contracts::ModuleTypeFacts,
    invalid: bool,
}

impl Validate<'_> {
    fn selected(&self, capture: &LexicalCellCapture) -> bool {
        let mut seen = BTreeSet::new();
        !capture.nominal_binding_indices.is_empty()
            && capture.nominal_binding_indices.iter().all(|index| {
                seen.insert(index)
                    && crate::strict_function::required_lexical_field_leaf(
                        self.facts,
                        *index,
                        &capture.binding,
                    )
                    .is_some()
            })
    }

    fn operand_matches(&self, capture: &LexicalCellCapture, operand: &InstrBlockPy) -> bool {
        let InstrBlockPy::CellRef(cell) = operand else {
            return false;
        };
        let Some(layout) = self.producer.storage_layout.as_ref() else {
            return false;
        };
        match cell.location {
            CellLocation::Owned(index) | CellLocation::Preserved(index) => {
                let name = if matches!(cell.location, CellLocation::Owned(_)) {
                    layout
                        .owned_slot(index)
                        .map(|slot| slot.logical_name.as_str())
                } else {
                    layout
                        .preserved_slot(index)
                        .filter(|slot| slot.storage == PreservedSlotStorage::PyCellObject)
                        .map(|slot| slot.logical_name.as_str())
                };
                name == Some(capture.binding.name.as_str())
                    && self
                        .producer
                        .scope
                        .source_origin
                        .as_ref()
                        .is_some_and(|origin| {
                            origin.role == CallableSourceRole::SourceFunction
                                && origin.definition == capture.binding.scope
                        })
            }
            CellLocation::Closure(index) | CellLocation::CapturedSource(index) => {
                let Some(scope) = &self.producer.scope.private_lexical else {
                    return false;
                };
                scope.captures.iter().any(|projection| {
                    projection.cell.binding == capture.binding
                        && projection.native_closure.as_deref().is_some_and(|name| {
                            self.producer
                                .public_storage_layout()
                                .and_then(|layout| layout.freevar_slot(index))
                                .is_some_and(|slot| slot.logical_name == name)
                        })
                        && capture
                            .nominal_binding_indices
                            .iter()
                            .all(|index| projection.cell.nominal_binding_indices.contains(index))
                }) && self
                    .producer
                    .scope
                    .source_origin
                    .as_ref()
                    .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
            }
            CellLocation::Private(index) => self
                .producer
                .scope
                .private_lexical
                .as_ref()
                .and_then(|scope| scope.private_captures().nth(index as usize))
                .is_some_and(|actual| {
                    actual.binding == capture.binding
                        && capture
                            .nominal_binding_indices
                            .iter()
                            .all(|index| actual.nominal_binding_indices.contains(index))
                }),
        }
    }

    fn matches(&self, op: &MakeFunctionWithClosure<InstrBlockPy>) -> bool {
        let Some(target) = self
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == op.function_id)
        else {
            return op.class_namespace.is_none() && op.creation_cells.is_empty();
        };
        if let Some(scope) = &target.scope.private_lexical {
            if target.scope.class_construction.is_some()
                || self.producer.scope.source_origin.as_ref() != Some(&scope.creator)
                || op.class_namespace.is_some()
                || scope.captures.is_empty()
            {
                return false;
            }
            let Some(origin) = &target.scope.source_origin else {
                return false;
            };
            if !matches!(
                origin.role,
                CallableSourceRole::SourceFunction | CallableSourceRole::ClassNamespace
            ) {
                return false;
            }
            let demand = demanded_cells(self.module, target);
            let mut seen = BTreeSet::new();
            for projection in &scope.captures {
                if !self.selected(&projection.cell)
                    || !seen.insert(&projection.cell.binding)
                    || !projection
                        .cell
                        .nominal_binding_indices
                        .iter()
                        .all(|index| demand.contains(&(projection.cell.binding.clone(), *index)))
                {
                    return false;
                }
                if let Some(name) = &projection.native_closure {
                    if origin.role != CallableSourceRole::SourceFunction
                        || name != &projection.cell.binding.name
                        || target.public_scope().binding_kind(name)
                            != Some(BindingKind::Cell(CellBindingKind::Capture))
                        || !target.public_storage_layout().is_some_and(|layout| {
                            layout
                                .freevars
                                .iter()
                                .any(|slot| &slot.logical_name == name)
                        })
                    {
                        return false;
                    }
                }
            }
            if origin.role == CallableSourceRole::ClassNamespace {
                if !op.creation_cells.is_empty()
                    || scope
                        .captures
                        .iter()
                        .any(|capture| capture.native_closure.is_some())
                {
                    return false;
                }
                return self.module.callable_defs.iter().any(|function| {
                    function
                        .scope
                        .class_construction
                        .as_ref()
                        .is_some_and(|construction| {
                            construction.namespace_function == target.function_id
                                && scope.private_captures().all(|capture| {
                                    construction.captures.iter().any(|provided| {
                                        provided.binding == capture.binding
                                            && capture.nominal_binding_indices.iter().all(|index| {
                                                provided.nominal_binding_indices.contains(index)
                                            })
                                    })
                                })
                        })
                });
            }
            return scope.private_captures().count() == op.creation_cells.len()
                && scope
                    .private_captures()
                    .zip(&op.creation_cells)
                    .all(|(capture, operand)| self.operand_matches(capture, operand));
        }
        let Some(plan) = &target.scope.class_construction else {
            return op.class_namespace.is_none() && op.creation_cells.is_empty();
        };
        let Some(class) = target
            .scope
            .source_origin
            .as_ref()
            .filter(|origin| origin.role == CallableSourceRole::ClassConstruction)
        else {
            return false;
        };
        if self.producer.scope.source_origin.as_ref() != Some(&plan.producer)
            || !matches!(
                plan.producer.role,
                CallableSourceRole::SourceFunction | CallableSourceRole::ClassNamespace
            )
            || plan.captures.is_empty()
            || op.creation_cells.len() != plan.captures.len()
        {
            return false;
        }
        let Some(namespace) = self
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == plan.namespace_function)
        else {
            return false;
        };
        if namespace.scope.source_origin.as_ref().is_none_or(|origin| {
            origin.role != CallableSourceRole::ClassNamespace
                || origin.definition != class.definition
        }) {
            return false;
        }
        let Some(InstrBlockPy::Load(load)) = op.class_namespace.as_deref() else {
            return false;
        };
        if load.name.id.as_str() != namespace.names.bind_name
            || !matches!(
                load.name.location,
                NameLocation::Local(_) | NameLocation::Preserved(_)
            )
        {
            return false;
        }
        let mut seen = BTreeSet::new();
        for (capture, operand) in plan.captures.iter().zip(&op.creation_cells) {
            if !self.selected(capture)
                || !seen.insert(&capture.binding)
                || !self.operand_matches(capture, operand)
            {
                return false;
            }
            for &index in &capture.nominal_binding_indices {
                let leaf = &self.facts.nominal_bindings[index as usize];
                let own = matches!(&leaf.owner, soac_contracts::NominalBindingOwner::Field { field } if field.declaring_class.definition == class.definition);
                let forwarded = namespace
                    .scope
                    .private_lexical
                    .as_ref()
                    .is_some_and(|scope| {
                        scope.private_captures().any(|projection| {
                            projection.binding == capture.binding
                                && projection.nominal_binding_indices.contains(&index)
                        })
                    });
                if !own && !forwarded {
                    return false;
                }
            }
        }
        true
    }
}

impl Visit<InstrBlockPy> for Validate<'_> {
    fn visit_instr(&mut self, node: &InstrBlockPy) {
        if let InstrBlockPy::MakeFunctionWithClosure(op) = node {
            self.invalid |= !self.matches(op);
        }
        node.visit_children(self);
    }
}

pub(super) fn validate_creations(
    module: &BlockPyModule<BlockPyModuleShape>,
    facts: &soac_contracts::ModuleTypeFacts,
) -> PyResult<()> {
    for producer in &module.callable_defs {
        let mut validate = Validate {
            module,
            producer,
            facts,
            invalid: false,
        };
        validate.visit_fn(producer);
        if validate.invalid {
            return Err(pyo3::exceptions::PyRuntimeError::new_err(
                "private lexical captures changed their explicit source/cell/namespace projection",
            ));
        }
    }
    Ok(())
}
