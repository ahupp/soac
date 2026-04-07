use crate::block_py::{
    walk_expr_mut, walk_module_mut, BlockPyModule, ChildVisitable, NameLocation, ResolvedName,
    VisitMut,
};
use crate::passes::{InstrResolved, ResolvedStorageModuleShape};
use std::collections::HashMap;

#[derive(Default)]
struct ModuleGlobalSlots {
    slot_by_name: HashMap<String, u32>,
    names: Vec<String>,
}

impl ModuleGlobalSlots {
    fn with_preferred_names(preferred_names: impl IntoIterator<Item = String>) -> Self {
        let mut slots = Self::default();
        for name in preferred_names {
            slots.slot_for(name.as_str());
        }
        slots
    }

    fn slot_for(&mut self, name: &str) -> u32 {
        if let Some(slot) = self.slot_by_name.get(name).copied() {
            return slot;
        }
        let slot =
            u32::try_from(self.names.len()).expect("module global slot count should fit in u32");
        self.slot_by_name.insert(name.to_string(), slot);
        self.names.push(name.to_string());
        slot
    }

    fn into_names(self) -> Vec<String> {
        self.names
    }
}

struct GlobalIndexer {
    global_slots: ModuleGlobalSlots,
}

impl GlobalIndexer {
    fn index_name(&mut self, name: &mut ResolvedName) {
        if !name.location.is_global_name() {
            return;
        }
        let slot = self.global_slots.slot_for(name.id.as_str());
        name.location = NameLocation::global(slot);
    }
}

impl VisitMut<InstrResolved> for GlobalIndexer {
    fn visit_instr_mut(&mut self, expr: &mut InstrResolved)
    where
        InstrResolved: ChildVisitable<InstrResolved>,
    {
        walk_expr_mut(self, expr);

        match expr {
            InstrResolved::Load(op) => self.index_name(&mut op.name),
            InstrResolved::Store(op) => self.index_name(&mut op.name),
            InstrResolved::Del(op) => self.index_name(&mut op.name),
            _ => {}
        }
    }
}

pub fn lower_global_index_in_resolved_module(
    module: BlockPyModule<ResolvedStorageModuleShape>,
    preferred_global_names: impl IntoIterator<Item = String>,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    let mut indexer = GlobalIndexer {
        global_slots: ModuleGlobalSlots::with_preferred_names(
            module.global_names.into_iter().chain(preferred_global_names),
        ),
    };
    let mut lowered = BlockPyModule {
        global_names: Vec::new(),
        ..module
    };
    walk_module_mut(&mut indexer, &mut lowered);
    lowered.global_names = indexer.global_slots.into_names();
    lowered
}

pub fn lower_global_index_in_resolved_module_default(
    module: BlockPyModule<ResolvedStorageModuleShape>,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    lower_global_index_in_resolved_module(module, std::iter::empty())
}
