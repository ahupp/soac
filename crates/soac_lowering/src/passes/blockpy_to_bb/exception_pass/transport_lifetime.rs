//! Caught-object operands are not handler identities. Keep their values only
//! through semantic reads or a new handler's entry, then reset the explicit
//! storage while the native handled-state owner retains the current exception.
//! Both entry execution and native code consume these resolved stores.

use super::transport_storage::{OwnerSet, TransportStorage};
use crate::block_py::{
    BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, ChildVisitable,
    HandledExceptionContext, InstrResolved, InstrWithConstantNone, NameLike, ResolvedStorageBlock,
    Store, StorePurpose, Visit, VisitMut,
};
use crate::passes::ResolvedStorageModuleShape;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

type Names = BTreeSet<String>;
type Blocks<'a> = HashMap<BlockLabel, &'a ResolvedStorageBlock>;
type ResumeTargets = HashSet<BlockLabel>;

fn normal_edges(block: &ResolvedStorageBlock) -> Vec<BlockEdge> {
    match &block.term {
        BlockTerm::Jump(edge) => vec![edge.clone()],
        BlockTerm::IfTerm(branch) => [branch.then_label, branch.else_label]
            .into_iter()
            .map(BlockEdge::new)
            .collect(),
        BlockTerm::BranchTable(branch) => branch
            .targets
            .iter()
            .copied()
            .chain(std::iter::once(branch.default_label))
            .map(BlockEdge::new)
            .collect(),
        BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) | BlockTerm::Raise(_) => Vec::new(),
    }
}

/// A native resume dispatcher chooses one saved program counter. Its edges
/// into resume wrappers are not fresh source executions: only the explicit
/// yielding predecessor supplies the activation state for those wrappers.
fn lifetime_edges(block: &ResolvedStorageBlock, resumes: &ResumeTargets) -> Vec<(BlockEdge, bool)> {
    let mut edges = normal_edges(block)
        .into_iter()
        .filter(|edge| !resumes.contains(&edge.target))
        .map(|edge| (edge, false))
        .collect::<Vec<_>>();
    if let Some(resume) = block.extra.suspension_resume {
        edges.push((BlockEdge::new(resume), true));
    }
    edges
}

#[derive(Clone, Default, PartialEq, Eq)]
struct ScopeState {
    active: Vec<String>,
    pending: Names,
}

/// A Preserve bridge carries the predecessor's actual activation, not the
/// roles listed on its incoming payload. In particular, an operand-unwind
/// bridge must keep the pending caught value until the real handler is entered.
fn scope_states(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    blocks: &Blocks<'_>,
    resumes: &ResumeTargets,
) -> HashMap<BlockLabel, ScopeState> {
    let all = function
        .blocks
        .iter()
        .flat_map(|block| block.handled_exception_params())
        .map(|param| param.name.clone())
        .collect::<Names>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut predecessors = HashMap::<BlockLabel, Vec<(BlockLabel, bool)>>::new();
    for block in &function.blocks {
        for (edge, _) in lifetime_edges(block, resumes) {
            predecessors
                .entry(edge.target)
                .or_default()
                .push((block.label, false));
        }
        if let Some(edge) = &block.exc_edge {
            predecessors
                .entry(edge.target)
                .or_default()
                .push((block.label, true));
        }
    }
    let mut states = function
        .blocks
        .iter()
        .map(|block| {
            let active = match block.extra.handled_exception {
                HandledExceptionContext::Regions | HandledExceptionContext::Unwind => block
                    .handled_exception_params()
                    .map(|param| param.name.clone())
                    .collect(),
                HandledExceptionContext::Preserve => all.clone(),
                HandledExceptionContext::Terminal => Vec::new(),
            };
            (
                block.label,
                ScopeState {
                    active,
                    pending: Names::new(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    loop {
        let mut changed = false;
        for block in &function.blocks {
            if !matches!(
                block.extra.handled_exception,
                HandledExceptionContext::Preserve | HandledExceptionContext::Unwind
            ) {
                continue;
            }
            let mut next = ScopeState {
                active: predecessors
                    .get(&block.label)
                    .and_then(|edges| edges.first())
                    .map(|(source, _)| states[source].active.clone())
                    .unwrap_or_default(),
                pending: Names::new(),
            };
            if block.label == function.entry_block().label
                || !predecessors.contains_key(&block.label)
            {
                next.active.clear();
            }
            for (source, exceptional) in predecessors.get(&block.label).into_iter().flatten() {
                let previous = &states[source];
                let common = next
                    .active
                    .iter()
                    .zip(&previous.active)
                    .take_while(|(left, right)| left == right)
                    .count();
                next.active.truncate(common);
                if *exceptional {
                    if let Some(param) = blocks[&block.label].handled_exception_params().last() {
                        next.pending.insert(param.name.clone());
                    }
                } else {
                    next.pending.extend(previous.pending.iter().cloned());
                }
            }
            if block.extra.handled_exception == HandledExceptionContext::Unwind {
                // Trim-only blocks retain the pending incoming error while
                // selecting an existing prefix. The final Regions target is
                // the only block that can enter and consume that error.
                next.active = block
                    .handled_exception_params()
                    .map(|param| param.name.clone())
                    .collect();
            }
            if states[&block.label] != next {
                states.insert(block.label, next);
                changed = true;
            }
        }
        if !changed {
            return states;
        }
    }
}

fn edge_live(
    edge: &BlockEdge,
    source: &ScopeState,
    inputs: &HashMap<BlockLabel, OwnerSet>,
    blocks: &Blocks<'_>,
    catalog: &TransportStorage,
) -> OwnerSet {
    let target = blocks[&edge.target];
    let mut required = inputs.get(&edge.target).cloned().unwrap_or_default();
    if target.extra.handled_exception == HandledExceptionContext::Regions {
        // Entry compares ordered regions, not set membership: removing or
        // reordering an outer scope can require re-entering its former child.
        let mut common = true;
        for (index, param) in target.handled_exception_params().enumerate() {
            common &= source.active.get(index) == Some(&param.name)
                && !source.pending.contains(&param.name);
            if !common {
                required.extend(catalog.parameter(&param.name));
            }
        }
    }
    let mut result = required.clone();
    let explicit_start = target.params.len().saturating_sub(edge.args.len());
    for (param, arg) in target.params.iter().skip(explicit_start).zip(&edge.args) {
        let target_owner = catalog.parameter(&param.name);
        if let Some(owner) = target_owner {
            result.remove(&owner);
        }
        if let BlockArg::Name(name) = arg {
            if target_owner.is_none_or(|owner| required.contains(&owner)) {
                result.extend(catalog.parameter(name));
            }
        }
    }
    result
}

fn transfer_backwards(instr: &InstrResolved, live: &mut OwnerSet, catalog: &TransportStorage) {
    match instr {
        InstrResolved::Load(load) => {
            live.extend(catalog.key(&load.name));
        }
        InstrResolved::Store(store) => {
            if let Some(source) = catalog.copy_source(store) {
                let destination = catalog.key(&store.name).unwrap();
                if live.remove(&destination) {
                    live.insert(source);
                }
                return;
            }
            if let Some(owner) = catalog.key(&store.name) {
                live.remove(&owner);
            }
            transfer_backwards(&store.value, live, catalog);
        }
        InstrResolved::Del(del) => {
            if let Some(owner) = catalog.key(&del.name) {
                live.remove(&owner);
            }
        }
        _ => {
            struct Children<'a> {
                live: &'a mut OwnerSet,
                catalog: &'a TransportStorage,
            }
            impl Visit<InstrResolved> for Children<'_> {
                fn visit_instr(&mut self, instr: &InstrResolved) {
                    transfer_backwards(instr, self.live, self.catalog);
                }
            }
            instr.visit_children(&mut Children { live, catalog });
        }
    }
}

fn block_liveness(
    block: &ResolvedStorageBlock,
    inputs: &HashMap<BlockLabel, OwnerSet>,
    scopes: &HashMap<BlockLabel, ScopeState>,
    blocks: &Blocks<'_>,
    catalog: &TransportStorage,
    resumes: &ResumeTargets,
) -> Vec<OwnerSet> {
    // Pending return/raise values have a control-flow lifetime, not merely a
    // last-read lifetime. Their explicit extent ends at the ordered Unwind
    // blocks emitted by pending_cleanup.
    let pending = block
        .pending_abrupt_payload_params()
        .flat_map(|param| catalog.for_logical(&param.name))
        .collect::<OwnerSet>();
    let mut live = pending.clone();
    for (edge, suspension) in lifetime_edges(block, resumes) {
        if suspension {
            // Incoming local parameters die with this native invocation.
            // The final resume wrapper reloads only saved activation owners.
            live.extend(
                inputs
                    .get(&edge.target)
                    .into_iter()
                    .flatten()
                    .filter(|owner| owner.is_preserved())
                    .copied(),
            );
        } else {
            live.extend(edge_live(
                &edge,
                &scopes[&block.label],
                inputs,
                blocks,
                catalog,
            ));
        }
    }
    struct TermReads<'a> {
        live: &'a mut OwnerSet,
        catalog: &'a TransportStorage,
    }
    impl Visit<InstrResolved> for TermReads<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            transfer_backwards(instr, self.live, self.catalog);
        }
    }
    crate::block_py::walk_term(
        &mut TermReads {
            live: &mut live,
            catalog,
        },
        &block.term,
    );
    let exceptional = block
        .exc_edge
        .as_ref()
        .map(|edge| edge_live(edge, &scopes[&block.label], inputs, blocks, catalog))
        .unwrap_or_default();
    live.extend(exceptional.iter().cloned());
    let mut points = vec![live.clone()];
    for instr in block.body.iter().rev() {
        transfer_backwards(instr, &mut live, catalog);
        live.extend(exceptional.iter().cloned());
        live.extend(pending.iter().cloned());
        points.push(live.clone());
    }
    points.reverse();
    points
}

fn live_points(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    blocks: &Blocks<'_>,
    scopes: &HashMap<BlockLabel, ScopeState>,
    catalog: &TransportStorage,
    resumes: &ResumeTargets,
) -> HashMap<BlockLabel, Vec<OwnerSet>> {
    let mut inputs = HashMap::new();
    loop {
        let mut changed = false;
        for block in function.blocks.iter().rev() {
            let points = block_liveness(block, &inputs, scopes, blocks, catalog, resumes);
            if inputs.get(&block.label) != Some(&points[0]) {
                inputs.insert(block.label, points[0].clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                block_liveness(block, &inputs, scopes, blocks, catalog, resumes),
            )
        })
        .collect()
}

fn transfer_owned(
    instr: &InstrResolved,
    owned: &mut OwnerSet,
    catalog: &TransportStorage,
    prefixes: &mut OwnerSet,
) {
    struct Children<'a> {
        owned: &'a mut OwnerSet,
        catalog: &'a TransportStorage,
        prefixes: &'a mut OwnerSet,
    }
    impl Visit<InstrResolved> for Children<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            transfer_owned(instr, self.owned, self.catalog, self.prefixes);
        }
    }
    instr.visit_children(&mut Children {
        owned,
        catalog,
        prefixes,
    });
    match instr {
        InstrResolved::Store(store) if catalog.key(&store.name).is_some() => {
            let destination = catalog.key(&store.name).unwrap();
            let has_value = match store.value.as_ref() {
                InstrResolved::Load(load) if load.name.is_runtime_symbol("NONE") => false,
                InstrResolved::Load(load) if catalog.key(&load.name).is_some() => {
                    owned.contains(&catalog.key(&load.name).unwrap())
                }
                _ => true,
            };
            owned.remove(&destination);
            if has_value {
                owned.insert(destination);
            }
        }
        InstrResolved::Del(del) => {
            if let Some(owner) = catalog.key(&del.name) {
                owned.remove(&owner);
            }
        }
        _ => {}
    }
    prefixes.extend(owned.iter().cloned());
}

fn edge_owned(
    edge: &BlockEdge,
    owned: &OwnerSet,
    blocks: &Blocks<'_>,
    catalog: &TransportStorage,
) -> OwnerSet {
    let mut next = owned.clone();
    let params = &blocks[&edge.target].params;
    let explicit_start = params.len().saturating_sub(edge.args.len());
    for (param, arg) in params.iter().skip(explicit_start).zip(&edge.args) {
        let Some(destination) = catalog.parameter(&param.name) else {
            continue;
        };
        next.remove(&destination);
        let has_value = match arg {
            BlockArg::Name(name) if catalog.parameter(name).is_some() => {
                owned.contains(&catalog.parameter(name).unwrap())
            }
            BlockArg::Name(_) | BlockArg::CurrentException => true,
            BlockArg::None | BlockArg::AbruptKind(_) => false,
        };
        if has_value {
            next.insert(destination);
        }
    }
    next
}

/// Apply the same proposed kills during forward analysis. This prevents a
/// dead owner from being cleared repeatedly at every later block and keeps
/// the emitted stores proportional to actual transport lifetimes.
fn retire_at_point(
    block: &ResolvedStorageBlock,
    owned: &mut OwnerSet,
    live: &OwnerSet,
) -> OwnerSet {
    if block.extra.handled_exception != HandledExceptionContext::Regions {
        return OwnerSet::new();
    }
    let dead = owned.difference(live).copied().collect::<OwnerSet>();
    owned.retain(|name| live.contains(name));
    dead
}

fn owned_inputs(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    blocks: &Blocks<'_>,
    catalog: &TransportStorage,
    live: &HashMap<BlockLabel, Vec<OwnerSet>>,
    resumes: &ResumeTargets,
) -> HashMap<BlockLabel, OwnerSet> {
    let entry = function.entry_block().label;
    let mut inputs = HashMap::from([(entry, catalog.preserved())]);
    let mut work = VecDeque::from([entry]);
    while let Some(label) = work.pop_front() {
        let block = blocks[&label];
        let mut owned = inputs[&label].clone();
        let mut propagate = |edge: &BlockEdge, source: &OwnerSet| {
            let incoming = edge_owned(edge, source, blocks, catalog);
            if let Some(previous) = inputs.get_mut(&edge.target) {
                let size = previous.len();
                previous.extend(incoming);
                if previous.len() != size {
                    work.push_back(edge.target);
                }
            } else {
                inputs.insert(edge.target, incoming);
                work.push_back(edge.target);
            }
        };
        for (index, instr) in block.body.iter().enumerate() {
            retire_at_point(block, &mut owned, &live[&label][index]);
            let mut prefixes = owned.clone();
            transfer_owned(instr, &mut owned, catalog, &mut prefixes);
            if let Some(edge) = &block.exc_edge {
                propagate(edge, &prefixes);
            }
        }
        retire_at_point(block, &mut owned, live[&label].last().unwrap());
        struct TermOwned<'a> {
            owned: &'a mut OwnerSet,
            catalog: &'a TransportStorage,
            prefixes: &'a mut OwnerSet,
        }
        impl Visit<InstrResolved> for TermOwned<'_> {
            fn visit_instr(&mut self, instr: &InstrResolved) {
                transfer_owned(instr, self.owned, self.catalog, self.prefixes);
            }
        }
        let mut prefixes = owned.clone();
        crate::block_py::walk_term(
            &mut TermOwned {
                owned: &mut owned,
                catalog,
                prefixes: &mut prefixes,
            },
            &block.term,
        );
        if let Some(edge) = &block.exc_edge {
            propagate(edge, &prefixes);
        }
        for (edge, suspension) in lifetime_edges(block, resumes) {
            if suspension {
                let saved = owned
                    .iter()
                    .filter(|owner| owner.is_preserved())
                    .copied()
                    .collect();
                propagate(&edge, &saved);
            } else {
                propagate(&edge, &owned);
            }
        }
    }
    inputs
}

pub(super) fn retire_exception_transports(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
) {
    crate::block_py::cfg::validate_suspension_resumes(function).unwrap_or_else(|error| {
        panic!(
            "invalid transport resume flow in {}: {error}",
            function.names.qualname
        )
    });
    let catalog = TransportStorage::new(function);
    if catalog.is_empty() {
        for block in &mut function.blocks {
            block.extra.suspension_resume = None;
        }
        consume_copy_purposes(function);
        return;
    }
    let resumes = function
        .blocks
        .iter()
        .filter_map(|block| block.extra.suspension_resume)
        .collect::<ResumeTargets>();
    let live = {
        let blocks = function
            .blocks
            .iter()
            .map(|block| (block.label, block))
            .collect::<Blocks<'_>>();
        let scopes = scope_states(function, &blocks, &resumes);
        live_points(function, &blocks, &scopes, &catalog, &resumes)
    };
    for block in &mut function.blocks {
        for (index, instr) in block.body.iter_mut().enumerate() {
            let InstrResolved::Store(store) = instr else {
                continue;
            };
            if catalog.copy_source(store).is_some()
                && !live[&block.label][index + 1].contains(&catalog.key(&store.name).unwrap())
            {
                // A transport-only copy is not a semantic use of its old
                // caught object. Still initialize the incoming argument slot:
                // removing the store would leave a later Jump unbound.
                store.value = Box::new(InstrResolved::constant_none());
            }
        }
    }
    let blocks = function
        .blocks
        .iter()
        .map(|block| (block.label, block))
        .collect::<Blocks<'_>>();
    let inputs = owned_inputs(function, &blocks, &catalog, &live, &resumes);
    for block in &mut function.blocks {
        // This producer-owned edge has served its one resolved-lifetime
        // consumer. Optimizers see only the resulting explicit owner stores.
        block.extra.suspension_resume = None;
        let Some(mut owned) = inputs.get(&block.label).cloned() else {
            continue;
        };
        let mut body = Vec::new();
        for (index, instr) in block.body.iter().enumerate() {
            for name in retire_at_point(block, &mut owned, &live[&block.label][index]) {
                body.push(InstrResolved::Store(Store::new(
                    catalog.name(name).clone(),
                    InstrResolved::constant_none(),
                )));
            }
            transfer_owned(instr, &mut owned, &catalog, &mut OwnerSet::new());
            body.push(instr.clone());
        }
        for name in retire_at_point(block, &mut owned, live[&block.label].last().unwrap()) {
            body.push(InstrResolved::Store(Store::new(
                catalog.name(name).clone(),
                InstrResolved::constant_none(),
            )));
        }
        block.body = body;
    }
    consume_copy_purposes(function);
}

fn consume_copy_purposes(function: &mut BlockPyFunction<ResolvedStorageModuleShape>) {
    struct Consume;
    impl VisitMut<InstrResolved> for Consume {
        fn visit_instr_mut(&mut self, instr: &mut InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                if store.purpose == StorePurpose::BlockParameterTransport {
                    store.purpose = StorePurpose::Binding;
                }
            }
            crate::block_py::walk_expr_mut(self, instr);
        }
    }
    let mut consume = Consume;
    for block in &mut function.blocks {
        for instr in &mut block.body {
            consume.visit_instr_mut(instr);
        }
        consume.visit_term_mut(&mut block.term);
    }
}
