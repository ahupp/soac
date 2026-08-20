//! Physical owners of compiler-declared exception/finally transports.
//!
//! A logical region is an identity, not a storage location. A resumed block
//! can own both an incoming local and a saved activation slot for that region.

use crate::block_py::{
    BlockParamRole, BlockPyFunction, BlockPyName, InstrResolved, LocalLocation, NameLocation,
    PreservedLocation, PreservedSlotStorage, ResolvedName, Store, StorePurpose,
};
use crate::passes::ResolvedStorageModuleShape;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TransportLocation {
    Local(u32),
    Preserved(u32),
}

impl TransportLocation {
    fn from_name(name: &ResolvedName) -> Option<Self> {
        match name.location {
            NameLocation::Local(slot) => Some(Self::Local(slot.slot())),
            NameLocation::Preserved(slot) => Some(Self::Preserved(slot.slot())),
            _ => None,
        }
    }

    pub(super) fn is_preserved(self) -> bool {
        matches!(self, Self::Preserved(_))
    }
}

pub(super) type OwnerSet = BTreeSet<TransportLocation>;

#[derive(Default)]
pub(super) struct TransportStorage {
    owners: BTreeMap<TransportLocation, ResolvedName>,
    logical: BTreeMap<String, OwnerSet>,
    locals: HashMap<String, TransportLocation>,
}

impl TransportStorage {
    pub(super) fn new(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> Self {
        let mut result = Self::default();
        let Some(layout) = &function.storage_layout else {
            return result;
        };
        for param in function.blocks.iter().flat_map(|block| &block.params) {
            if matches!(
                param.role,
                BlockParamRole::Exception
                    | BlockParamRole::EnclosingException
                    | BlockParamRole::AbruptPayload
                    | BlockParamRole::EnclosingAbruptPayload
            ) {
                result.logical.entry(param.name.clone()).or_default();
            }
        }
        for (slot, name) in layout.stack_slots.iter().enumerate() {
            let Some(owners) = result.logical.get_mut(name) else {
                continue;
            };
            let slot = u32::try_from(slot).expect("local transport location overflow");
            let key = TransportLocation::Local(slot);
            owners.insert(key);
            result.locals.insert(name.clone(), key);
            result.owners.insert(
                key,
                ResolvedName {
                    id: BlockPyName::new(name),
                    location: NameLocation::Local(LocalLocation(slot)),
                },
            );
        }
        for (slot, field) in layout.preserved_slots.iter().enumerate() {
            // Cell objects belong to the separate lexical-cell lifetime, not
            // to an incoming exception/payload object's raw pointer slot.
            if field.storage != PreservedSlotStorage::PyObjectOrNull {
                continue;
            }
            let slot = u32::try_from(slot).expect("preserved transport location overflow");
            let key = TransportLocation::Preserved(slot);
            let mut declared = false;
            for name in [&field.logical_name, &field.storage_name] {
                if let Some(owners) = result.logical.get_mut(name) {
                    owners.insert(key);
                    declared = true;
                }
            }
            if declared {
                result.owners.insert(
                    key,
                    ResolvedName {
                        id: BlockPyName::new(&field.storage_name),
                        location: NameLocation::Preserved(PreservedLocation(slot)),
                    },
                );
            }
        }
        // Name binding materializes saved block arguments into distinct local
        // owners. Only producer-marked raw copies extend a transport's owner
        // graph: an ordinary source assignment keeps its own frame lifetime.
        let copies =
            crate::passes::block_parameter_roles::block_parameter_transport_copies(function);
        loop {
            let mut changed = false;
            for (source, destination) in &copies {
                let Some(source_key) = result.key(source) else {
                    continue;
                };
                let destination_key = TransportLocation::from_name(destination)
                    .expect("validated parameter copy destination");
                if let TransportLocation::Preserved(slot) = destination_key {
                    if layout.preserved_slots[slot as usize].storage
                        != PreservedSlotStorage::PyObjectOrNull
                    {
                        continue;
                    }
                }
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    result.owners.entry(destination_key)
                {
                    entry.insert(destination.clone());
                    changed = true;
                }
                if matches!(destination_key, TransportLocation::Local(_)) {
                    result
                        .locals
                        .insert(destination.id.to_string(), destination_key);
                }
                for owners in result.logical.values_mut() {
                    if owners.contains(&source_key) {
                        changed |= owners.insert(destination_key);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        result
    }

    pub(super) fn is_empty(&self) -> bool {
        self.owners.is_empty()
    }

    pub(super) fn key(&self, name: &ResolvedName) -> Option<TransportLocation> {
        let key = TransportLocation::from_name(name)?;
        self.owners.contains_key(&key).then_some(key)
    }

    /// Explicit block arguments address the local parameter storage. Saved
    /// arguments are materialized into locals by name binding before this pass.
    pub(super) fn parameter(&self, name: &str) -> Option<TransportLocation> {
        self.locals.get(name).copied()
    }

    pub(super) fn for_logical(&self, name: &str) -> impl Iterator<Item = TransportLocation> + '_ {
        self.logical.get(name).into_iter().flatten().copied()
    }

    pub(super) fn preserved(&self) -> OwnerSet {
        self.owners
            .keys()
            .filter(|key| key.is_preserved())
            .copied()
            .collect()
    }

    pub(super) fn name(&self, key: TransportLocation) -> &ResolvedName {
        &self.owners[&key]
    }

    pub(super) fn copy_source(&self, store: &Store<InstrResolved>) -> Option<TransportLocation> {
        if !matches!(store.purpose, StorePurpose::BlockParameterTransport)
            || self.key(&store.name).is_none()
        {
            return None;
        }
        let InstrResolved::Load(load) = store.value.as_ref() else {
            return None;
        };
        self.key(&load.name)
    }
}
