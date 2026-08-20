//! A finally payload owns a Python value until its actual control-flow
//! disposition, even when no future source expression reads that value.
//! Materialize the interleaved handler-trim/value-drop sequence in shared IR.

use super::transport_storage::TransportStorage;
use crate::block_py::{
    BlockContext, BlockEdge, BlockParam, BlockParamRole, BlockPyFunction, BlockPyName, BlockTerm,
    ChildVisitable, FunctionKind, HandledExceptionContext, InstrResolved, InstrWithConstantNone,
    Load, LocalLocation, NameLocation, RaiseDisposition, ResolvedName, ResolvedStorageBlock, Store,
    StoreLifetime, TermRaise, Visit,
};
use crate::passes::ResolvedStorageModuleShape;
use std::collections::HashMap;

#[derive(Clone)]
struct PendingOwner {
    values: Vec<ResolvedName>,
    handlers: Vec<BlockParam>,
}

fn owners(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> HashMap<String, PendingOwner> {
    let storage = TransportStorage::new(function);
    let mut owners = HashMap::<String, PendingOwner>::new();
    for block in &function.blocks {
        if block.extra.handled_exception != HandledExceptionContext::Regions
            || !block
                .params
                .iter()
                .any(|param| param.role == BlockParamRole::AbruptKind)
        {
            continue;
        }
        for param in block
            .params
            .iter()
            .filter(|param| param.role == BlockParamRole::AbruptPayload)
        {
            let values = storage
                .for_logical(&param.name)
                .map(|key| storage.name(key).clone())
                .collect::<Vec<_>>();
            assert!(
                !values.is_empty(),
                "a pending payload requires resolved owning storage"
            );
            let handlers = block
                .handled_exception_params()
                .cloned()
                .collect::<Vec<_>>();
            if let Some(previous) = owners.get(&param.name) {
                assert_eq!(
                    previous.handlers, handlers,
                    "a pending payload must have one declaring handler prefix"
                );
            } else {
                owners.insert(param.name.clone(), PendingOwner { values, handlers });
            }
        }
    }
    owners
}

fn stack(block: &ResolvedStorageBlock, owners: &HashMap<String, PendingOwner>) -> Vec<String> {
    block
        .pending_abrupt_payload_params()
        .filter(|param| owners.contains_key(&param.name))
        .map(|param| param.name.clone())
        .collect()
}

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

fn fresh_local(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
    label: &str,
    operand: bool,
) -> ResolvedName {
    let name = function.name_gen.next_tmp_name(label).to_string();
    let layout = function.storage_layout.get_or_insert_with(Default::default);
    let slot =
        LocalLocation(u32::try_from(layout.stack_slots.len()).expect("local storage overflow"));
    layout.stack_slots.push(name.clone());
    if operand {
        layout.mark_expression_temporary(slot);
    }
    ResolvedName {
        id: BlockPyName::new(name),
        location: NameLocation::Local(slot),
    }
}

fn next_operand_order(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> u64 {
    struct Maximum(u64);
    impl Visit<InstrResolved> for Maximum {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            let lifetime = match instr {
                InstrResolved::Store(store) => store.lifetime,
                _ => StoreLifetime::Frame,
            };
            if let StoreLifetime::Operand { unwind_order } = lifetime {
                self.0 = self.0.max(unwind_order);
            }
            instr.visit_children(self);
        }
    }
    let mut maximum = Maximum(0);
    for block in &function.blocks {
        for instr in &block.body {
            maximum.visit_instr(instr);
        }
        crate::block_py::walk_term(&mut maximum, &block.term);
    }
    maximum
        .0
        .checked_add(1)
        .expect("operand unwind order overflow")
}

fn keep_param(params: &mut Vec<BlockParam>, name: &str, role: BlockParamRole) {
    if !params.iter().any(|param| param.name == name) {
        params.push(BlockParam {
            name: name.into(),
            role,
        });
    }
}

/// Entry is Preserve so a newly raised exception is captured without changing
/// the source handler. Each following block trims to the payload's declaring
/// prefix, drops that value, and preserves the same pending raised-scope marker.
fn cleanup_chain(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
    source: &ResolvedStorageBlock,
    target: &BlockEdge,
    target_params: &[BlockParam],
    leaving: &[String],
    owners: &HashMap<String, PendingOwner>,
    generated: &mut Vec<ResolvedStorageBlock>,
) -> BlockEdge {
    let mut params = target_params.to_vec();
    for name in leaving {
        keep_param(&mut params, name, BlockParamRole::Value);
        for handler in &owners[name].handlers {
            keep_param(&mut params, &handler.name, BlockParamRole::Value);
        }
    }
    // Keep the original target's exception roles only on the carrier: native
    // dispatch uses that exact declaration to record the pending target.
    let mut next = BlockEdge::with_args(
        target.target,
        target_params
            .iter()
            .map(|param| crate::block_py::BlockArg::Name(param.name.clone()))
            .collect(),
    );
    for name in leaving.iter().rev() {
        let owner = &owners[name];
        let source_handlers = source
            .handled_exception_params()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        let owner_handlers = owner
            .handlers
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            source_handlers.starts_with(&owner_handlers),
            "pending payload {} is outside its declaring handler prefix",
            name
        );
        let mut drop_params = params
            .iter()
            .filter(|param| {
                !owner
                    .handlers
                    .iter()
                    .any(|handler| handler.name == param.name)
            })
            .map(|param| BlockParam {
                name: param.name.clone(),
                role: BlockParamRole::Value,
            })
            .collect::<Vec<_>>();
        // Enclosing handler roles are stored innermost-first by the IR;
        // its accessor then returns the exact outermost-first prefix.
        drop_params.extend(owner.handlers.iter().rev().cloned());
        let next_args = drop_params
            .iter()
            .map(|param| crate::block_py::BlockArg::Name(param.name.clone()))
            .collect();
        let label = function.name_gen.next_block_name();
        generated.push(ResolvedStorageBlock {
            label,
            body: owner
                .values
                .iter()
                .map(|value| {
                    InstrResolved::Store(Store::new(value.clone(), InstrResolved::constant_none()))
                })
                .collect(),
            term: BlockTerm::Jump(next),
            params: drop_params,
            exc_edge: None,
            extra: BlockContext {
                handled_exception: HandledExceptionContext::Unwind,
                ..Default::default()
            },
        });
        next = BlockEdge::with_args(label, next_args);
    }
    let label = function.name_gen.next_block_name();
    let args = params
        .iter()
        .map(|param| {
            target_params
                .iter()
                .position(|target| target.name == param.name)
                .and_then(|index| target.args.get(index))
                .cloned()
                .unwrap_or_else(|| crate::block_py::BlockArg::Name(param.name.clone()))
        })
        .collect();
    // All these fields already have ordinary local storage. Extra params
    // transport existing owners; they do not allocate a second alias slot.
    generated.push(ResolvedStorageBlock {
        label,
        body: Vec::new(),
        term: BlockTerm::Jump(next),
        params,
        exc_edge: None,
        extra: BlockContext {
            handled_exception: HandledExceptionContext::Preserve,
            ..Default::default()
        },
    });
    BlockEdge::with_args(label, args)
}

pub(super) fn materialize_pending_payload_unwinds(
    function: &mut BlockPyFunction<ResolvedStorageModuleShape>,
) {
    let owners = owners(function);
    if owners.is_empty() {
        return;
    }
    let mut target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.params.clone()))
        .collect::<HashMap<_, _>>();
    let target_stacks = function
        .blocks
        .iter()
        .map(|block| (block.label, stack(block, &owners)))
        .collect::<HashMap<_, _>>();
    let mut order = next_operand_order(function);
    let mut generated = Vec::new();
    let mut output = Vec::new();
    let mut escaping = None;
    for mut block in std::mem::take(&mut function.blocks) {
        let pending = stack(&block, &owners);
        if pending.is_empty() || block.extra.handled_exception != HandledExceptionContext::Regions {
            output.push(block);
            continue;
        }
        if function.kind == FunctionKind::Function {
            if let BlockTerm::Return(value) = &block.term {
                // Evaluate and own the overriding value before dropping any
                // pending operand or restoring a surrounding handler.
                let result = fresh_local(function, "pending_return_result", true);
                block.body.push(InstrResolved::Store(
                    Store::new(result.clone(), value.clone()).with_lifetime(
                        StoreLifetime::Operand {
                            unwind_order: order,
                        },
                    ),
                ));
                order = order.checked_add(1).expect("operand unwind order overflow");
                let label = function.name_gen.next_block_name();
                let params = vec![BlockParam {
                    name: result.id.to_string(),
                    role: BlockParamRole::Value,
                }];
                generated.push(ResolvedStorageBlock {
                    label,
                    body: Vec::new(),
                    term: BlockTerm::Return(InstrResolved::Load(Load::new(result))),
                    params: params.clone(),
                    exc_edge: None,
                    extra: Default::default(),
                });
                target_params.insert(label, params);
                block.term = BlockTerm::Jump(BlockEdge::new(label));
            }
        }
        for edge in normal_edges(&block) {
            let target_pending = target_stacks
                .get(&edge.target)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let common = pending
                .iter()
                .zip(target_pending)
                .take_while(|(left, right)| left == right)
                .count();
            let leaving = pending[common..].iter().rev().cloned().collect::<Vec<_>>();
            if leaving.is_empty() {
                continue;
            }
            let replacement = cleanup_chain(
                function,
                &block,
                &edge,
                &target_params[&edge.target],
                &leaving,
                &owners,
                &mut generated,
            );
            match &mut block.term {
                BlockTerm::Jump(target) => *target = replacement,
                other => {
                    other.replace_target(edge.target, replacement.target);
                }
            }
            // Conditional/table edges have no explicit argument list. Their
            // carrier forwards the same existing bindings by name.
        }
        let edge = if let Some(edge) = block.exc_edge.clone() {
            edge
        } else {
            let label = *escaping.get_or_insert_with(|| {
                let error = fresh_local(function, "pending_escaping_exception", false);
                let label = function.name_gen.next_block_name();
                let params = vec![BlockParam {
                    name: error.id.to_string(),
                    role: BlockParamRole::Exception,
                }];
                target_params.insert(label, params.clone());
                generated.push(ResolvedStorageBlock {
                    label,
                    body: Vec::new(),
                    term: BlockTerm::Raise(TermRaise {
                        exc: Some(InstrResolved::Load(Load::new(error))),
                        disposition: RaiseDisposition::PropagateNormalized,
                    }),
                    params,
                    exc_edge: None,
                    // An ordinary callee trims its own regions but shares its
                    // caller's item. A suspended activation must detach. Error
                    // propagation itself is a separate terminator decision.
                    extra: BlockContext {
                        handled_exception: if function.kind == FunctionKind::Function {
                            HandledExceptionContext::Unwind
                        } else {
                            HandledExceptionContext::Terminal
                        },
                        ..Default::default()
                    },
                });
                label
            });
            BlockEdge::with_args(label, vec![crate::block_py::BlockArg::CurrentException])
        };
        let target_pending = target_stacks
            .get(&edge.target)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let common = pending
            .iter()
            .zip(target_pending)
            .take_while(|(left, right)| left == right)
            .count();
        let leaving = pending[common..].iter().rev().cloned().collect::<Vec<_>>();
        if !leaving.is_empty() {
            block.exc_edge = Some(cleanup_chain(
                function,
                &block,
                &edge,
                &target_params[&edge.target],
                &leaving,
                &owners,
                &mut generated,
            ));
        }
        output.push(block);
    }
    output.extend(generated);
    function.blocks = output;
}
