//! Resolve producer block-parameter roles before their declarations can be
//! optimized away. Only explicitly marked raw transport copies extend a role;
//! source assignments and lexical cells never acquire control semantics.

use crate::block_py::{
    BlockParamRole, BlockPyFunction, ChildVisitable, InstrResolved, LocalLocation, NameLocation,
    PreservedLocation, PreservedSlotStorage, ResolvedName, StorePurpose, Visit,
};
use crate::passes::ResolvedStorageModuleShape;

pub(crate) fn record_resolved_block_parameter_roles(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
) {
    let Some(layout) = function.storage_layout.as_ref() else {
        return;
    };
    let mut declared = Vec::new();
    for parameter in function.blocks.iter().flat_map(|block| &block.params) {
        if parameter.role == BlockParamRole::Value {
            continue;
        }
        let local = layout
            .stack_slots()
            .iter()
            .position(|name| name == &parameter.name)
            .unwrap_or_else(|| {
                panic!(
                    "control parameter {} in {} has no resolved local",
                    parameter.name, function.names.qualname
                )
            });
        declared.push((
            NameLocation::Local(LocalLocation(
                u32::try_from(local).expect("local slot fits u32"),
            )),
            parameter.role,
        ));
        for (index, slot) in layout.preserved_slots.iter().enumerate() {
            if slot.storage != PreservedSlotStorage::PyCellObject
                && (slot.logical_name == parameter.name || slot.storage_name == parameter.name)
            {
                declared.push((
                    NameLocation::Preserved(PreservedLocation(
                        u32::try_from(index).expect("preserved slot fits u32"),
                    )),
                    parameter.role,
                ));
            }
        }
    }
    // Capture these copies before transport_lifetime consumes their purpose.
    let copies = block_parameter_transport_copies(function);
    let layout = function.storage_layout.as_mut().unwrap();
    for (location, role) in declared {
        layout.record_block_parameter_role(location, role);
    }
    loop {
        let before = layout.block_parameter_roles.len();
        for (source, destination) in &copies {
            let roles = layout
                .block_parameter_roles_at(source.location)
                .collect::<Vec<_>>();
            for role in roles {
                layout.record_block_parameter_role(destination.location, role);
            }
        }
        if layout.block_parameter_roles.len() == before {
            break;
        }
    }
    layout
        .validate_block_parameter_roles()
        .unwrap_or_else(|error| {
            panic!(
                "invalid resolved block-parameter roles for {}: {error}",
                function.names.qualname
            )
        });
    layout
        .validate_block_parameter_declarations(
            function.blocks.iter().flat_map(|block| &block.params),
        )
        .unwrap_or_else(|error| {
            panic!(
                "missing resolved block-parameter role for {}: {error}",
                function.names.qualname
            )
        });
}

pub(crate) fn block_parameter_transport_copies(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
) -> Vec<(ResolvedName, ResolvedName)> {
    #[derive(Default)]
    struct Copies {
        depth: usize,
        values: Vec<(ResolvedName, ResolvedName)>,
    }
    impl Visit<InstrResolved> for Copies {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                if matches!(store.purpose, StorePurpose::BlockParameterTransport) {
                    assert_eq!(
                        self.depth, 0,
                        "block-parameter transport copies must be explicit statements"
                    );
                    let InstrResolved::Load(load) = store.value.as_ref() else {
                        panic!("block-parameter transport must copy a resolved raw owner")
                    };
                    assert!(
                        load.cell_binding.is_none(),
                        "lexical cells are not raw transports"
                    );
                    assert!(
                        matches!(
                            (&load.name.location, &store.name.location),
                            (NameLocation::Local(_), NameLocation::Preserved(_))
                                | (NameLocation::Preserved(_), NameLocation::Local(_))
                        ),
                        "block-parameter transport must cross local and preserved storage"
                    );
                    self.values.push((load.name.clone(), store.name.clone()));
                }
            }
            self.depth += 1;
            instr.visit_children(self);
            self.depth -= 1;
        }
    }
    let mut copies = Copies::default();
    for block in &function.blocks {
        for instr in &block.body {
            copies.visit_instr(instr);
        }
        copies.depth = 1;
        copies.visit_term(&block.term);
        copies.depth = 0;
    }
    copies.values
}
