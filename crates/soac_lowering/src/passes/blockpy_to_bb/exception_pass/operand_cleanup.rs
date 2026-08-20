//! Materialize operand-stack unwinding in the shared resolved CFG. This runs
//! after exception arguments have been selected, so the interpreter and JIT
//! consume the same cleanup and original exception transport.

use crate::block_py::{
    BlockArg, BlockContext, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, ChildVisitable, Del,
    HandledExceptionContext, InstrResolved, NameLocation, ResolvedName, ResolvedStorageBlock,
    StoreLifetime, Visit,
};
use crate::passes::ResolvedStorageModuleShape;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct Operand {
    name: ResolvedName,
    unwind_order: u64,
}

type LiveSet = HashSet<NameLocation>;

fn normal_targets(block: &ResolvedStorageBlock) -> Vec<BlockLabel> {
    match &block.term {
        BlockTerm::Jump(edge) => vec![edge.target],
        BlockTerm::IfTerm(branch) => vec![branch.then_label, branch.else_label],
        BlockTerm::BranchTable(branch) => {
            let mut targets = branch.targets.clone();
            targets.push(branch.default_label);
            targets
        }
        BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) | BlockTerm::Raise(_) => Vec::new(),
    }
}

struct OperandCollector(HashMap<NameLocation, Operand>);

impl Visit<InstrResolved> for OperandCollector {
    fn visit_instr(&mut self, instr: &InstrResolved) {
        let acquisition = match instr {
            InstrResolved::Store(store) => Some((&store.name, store.lifetime)),
            _ => None,
        };
        if let Some((name, StoreLifetime::Operand { unwind_order })) = acquisition {
            assert!(
                matches!(
                    name.location,
                    NameLocation::Local(_) | NameLocation::Preserved(_)
                ),
                "an expression operand must have private local or suspended storage"
            );
            if let Some(previous) = self.0.insert(
                name.location,
                Operand {
                    name: name.clone(),
                    unwind_order,
                },
            ) {
                assert_eq!(
                    previous.unwind_order, unwind_order,
                    "distinct operand producers cannot share a physical binding"
                );
            }
        }
        instr.visit_children(self);
    }
}

/// An edge reads its named arguments even if the target does not subsequently
/// read the corresponding parameter. Keep those values through the handoff.
fn edge_live(
    edge: &BlockEdge,
    live: &HashMap<BlockLabel, LiveSet>,
    operands: &[Operand],
) -> LiveSet {
    let mut result = live.get(&edge.target).cloned().unwrap_or_default();
    for arg in &edge.args {
        if let BlockArg::Name(name) = arg {
            result.extend(
                operands
                    .iter()
                    .filter(|operand| operand.name.id.as_str() == name)
                    .map(|operand| operand.name.location),
            );
        }
    }
    result
}

/// Source-generated operand deletes end a lifetime; they are not an observable
/// read that should keep the value alive on an earlier exceptional edge.
fn transfer_backwards(instr: &InstrResolved, live: &mut LiveSet, locations: &LiveSet) {
    match instr {
        InstrResolved::Load(load) => {
            if locations.contains(&load.name.location) {
                live.insert(load.name.location);
            }
        }
        InstrResolved::Store(store) => {
            live.remove(&store.name.location);
            transfer_backwards(&store.value, live, locations);
        }
        InstrResolved::Del(del) => {
            live.remove(&del.name.location);
        }
        InstrResolved::TakeOperand(op) => {
            live.remove(&op.name.location);
            if locations.contains(&op.name.location) {
                live.insert(op.name.location);
            }
        }
        InstrResolved::IteratorStep(op) => {
            if locations.contains(&op.name.location) {
                live.insert(op.name.location);
            }
        }
        InstrResolved::CallArgumentOp(op) => {
            for name in op.written_names() {
                live.remove(&name.location);
            }
            if let Some(value) = &op.value {
                transfer_backwards(value, live, locations);
            }
            for name in op.read_names() {
                if locations.contains(&name.location) {
                    live.insert(name.location);
                }
            }
        }
        InstrResolved::ComprehensionInsert(op) => {
            if locations.contains(&op.container.location) {
                live.insert(op.container.location);
            }
            transfer_backwards(&op.value, live, locations);
            if let Some(key) = &op.key {
                transfer_backwards(key, live, locations);
            }
        }
        _ => {
            struct Children<'a> {
                live: &'a mut LiveSet,
                locations: &'a LiveSet,
            }
            impl Visit<InstrResolved> for Children<'_> {
                fn visit_instr(&mut self, instr: &InstrResolved) {
                    transfer_backwards(instr, self.live, self.locations);
                }
            }
            // The only nested binding write is TakeOperand, which reads and
            // clears that same slot. These reads still commute for this
            // may-live proof; insertion handles key/value order explicitly.
            instr.visit_children(&mut Children { live, locations });
        }
    }
}

fn live_inputs(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    operands: &[Operand],
    locations: &LiveSet,
) -> HashMap<BlockLabel, LiveSet> {
    let mut inputs = HashMap::<_, LiveSet>::new();
    loop {
        let mut changed = false;
        for block in function.blocks.iter().rev() {
            let mut live = LiveSet::new();
            for target in normal_targets(block) {
                live.extend(inputs.get(&target).into_iter().flatten().copied());
            }
            if let BlockTerm::Jump(edge) = &block.term {
                live.extend(edge_live(edge, &inputs, operands));
            }
            struct TermReads<'a> {
                live: &'a mut LiveSet,
                locations: &'a LiveSet,
            }
            impl Visit<InstrResolved> for TermReads<'_> {
                fn visit_instr(&mut self, instr: &InstrResolved) {
                    transfer_backwards(instr, self.live, self.locations);
                }
            }
            crate::block_py::walk_term(
                &mut TermReads {
                    live: &mut live,
                    locations,
                },
                &block.term,
            );
            let exceptional = block
                .exc_edge
                .as_ref()
                .map(|edge| edge_live(edge, &inputs, operands))
                .unwrap_or_default();
            live.extend(exceptional.iter().copied());
            for instr in block.body.iter().rev() {
                transfer_backwards(instr, &mut live, locations);
                // The right side of a store can fail before replacing its
                // previous value. Include every throwing instruction prefix.
                live.extend(exceptional.iter().copied());
            }
            if inputs.get(&block.label) != Some(&live) {
                inputs.insert(block.label, live);
                changed = true;
            }
        }
        if !changed {
            return inputs;
        }
    }
}

fn acquired_prefixes(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    locations: &LiveSet,
) -> HashMap<BlockLabel, LiveSet> {
    let mut incoming = HashMap::<_, LiveSet>::new();
    // A resume dispatcher can enter after a suspension. A preserved operand
    // may already exist even though that edge is not a normal CFG successor
    // of the previous yield. Uninitialized preserved slots remain null.
    if let Some(entry) = function.blocks.first() {
        incoming.insert(
            entry.label,
            locations
                .iter()
                .copied()
                .filter(|location| matches!(location, NameLocation::Preserved(_)))
                .collect(),
        );
    }
    let mut failures = HashMap::new();
    loop {
        let mut changed = false;
        for block in &function.blocks {
            let mut active = incoming.get(&block.label).cloned().unwrap_or_default();
            let mut prefixes = active.clone();
            struct Acquire<'a> {
                active: &'a mut LiveSet,
                prefixes: &'a mut LiveSet,
                locations: &'a LiveSet,
            }
            impl Visit<InstrResolved> for Acquire<'_> {
                fn visit_instr(&mut self, instr: &InstrResolved) {
                    instr.visit_children(self);
                    match instr {
                        InstrResolved::Store(store)
                            if self.locations.contains(&store.name.location) =>
                        {
                            self.active.insert(store.name.location);
                            self.prefixes.insert(store.name.location);
                        }
                        InstrResolved::Del(del) => {
                            self.active.remove(&del.name.location);
                        }
                        InstrResolved::CallArgumentOp(op) => {
                            for name in op.written_names() {
                                if self.locations.contains(&name.location) {
                                    self.active.insert(name.location);
                                }
                            }
                        }
                        InstrResolved::TakeOperand(op) => {
                            self.active.remove(&op.name.location);
                        }
                        _ => {}
                    }
                }
            }
            let mut acquire = Acquire {
                active: &mut active,
                prefixes: &mut prefixes,
                locations,
            };
            for instr in &block.body {
                acquire.visit_instr(instr);
            }
            crate::block_py::walk_term(&mut acquire, &block.term);
            for target in normal_targets(block) {
                let target = incoming.entry(target).or_default();
                let previous = target.len();
                target.extend(active.iter().copied());
                changed |= previous != target.len();
            }
            if let Some(edge) = &block.exc_edge {
                let target = incoming.entry(edge.target).or_default();
                let previous = target.len();
                target.extend(prefixes.iter().copied());
                changed |= previous != target.len();
            }
            failures.insert(block.label, prefixes);
        }
        if !changed {
            return failures;
        }
    }
}

pub(super) fn materialize_operand_unwinds(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
) {
    let mut collector = OperandCollector(HashMap::new());
    for block in &function.blocks {
        for instr in &block.body {
            collector.visit_instr(instr);
        }
        crate::block_py::walk_term(&mut collector, &block.term);
    }
    if collector.0.is_empty() {
        return;
    }
    let locations = collector.0.keys().copied().collect::<LiveSet>();
    let mut operands = collector.0.into_values().collect::<Vec<_>>();
    operands.sort_by_key(|operand| std::cmp::Reverse(operand.unwind_order));
    // The lifetime producer, not a generated name, certifies both private
    // local operands and operands that already belong to a suspended payload.
    let layout = function
        .storage_layout
        .as_mut()
        .expect("resolved operand cleanup requires a physical layout");
    for operand in operands.iter().rev() {
        layout.mark_expression_temporary(
            crate::block_py::OperandLocation::from_name_location(operand.name.location)
                .expect("operand collector checked owning storage"),
        );
    }
    let live = live_inputs(function, &operands, &locations);
    let acquired = acquired_prefixes(function, &locations);
    let params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.params.clone()))
        .collect::<HashMap<_, _>>();
    let mut cleanups = Vec::new();
    for block in &mut function.blocks {
        let Some(edge) = block.exc_edge.as_ref() else {
            continue;
        };
        let target_live = edge_live(edge, &live, &operands);
        let available = &acquired[&block.label];
        let body = operands
            .iter()
            .filter(|operand| {
                available.contains(&operand.name.location)
                    && !target_live.contains(&operand.name.location)
            })
            .map(|operand| InstrResolved::Del(Del::new(operand.name.clone(), true)))
            .collect::<Vec<_>>();
        if body.is_empty() {
            continue;
        }
        let Some(target_params) = params.get(&edge.target) else {
            continue;
        };
        assert_eq!(
            edge.args.len(),
            target_params.len(),
            "operand cleanup requires resolved exception arguments"
        );
        let label = function.name_gen.next_block_name();
        let outgoing = BlockEdge::with_args(
            edge.target,
            target_params
                .iter()
                .map(|param| BlockArg::Name(param.name.clone()))
                .collect(),
        );
        cleanups.push(ResolvedStorageBlock {
            label,
            body,
            term: BlockTerm::Jump(outgoing),
            // Retain the incoming raised scope marker, but do not enter that
            // handler until after its operand values have been released.
            params: target_params.clone(),
            exc_edge: None,
            extra: BlockContext {
                handled_exception: HandledExceptionContext::Preserve,
                ..Default::default()
            },
        });
        block.exc_edge = Some(BlockEdge::with_args(label, edge.args.clone()));
    }
    function.blocks.extend(cleanups);
}
