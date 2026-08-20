//! Source-shaped expansion barriers around child expression control flow.
//!
//! The same phase producer serves Ruff child CFGs and core suspension ordering.
//! A sole star intentionally waits until keyword preparation before iteration.

use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    BuildCollection, BuildCollectionKind, Call, CallArgKeyword, CallArgPositional, CallArgumentOp,
    CallArgumentOpKind, ComprehensionInsert, ComprehensionInsertKind, ExprTuple, HasMeta, Instr,
    Meta, PreparedCall, Store, StoreLifetime, TakeOperand, WithMeta,
};
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;
use crate::template::py_expr;
use ruff_python_ast as ast;

// Include/internal/pycore_compile.h in the pinned native compiler. This selects
// source stack shape, not a profile-dependent runtime prediction.
const NATIVE_STACK_USE_GUIDELINE: usize = 30;

/// The adapters supply expression lowering and physical Operand acquisition.
/// All expansion, grouping and normalization decisions live in the producer
/// below, including the singleton-star ERROR_NO_POP boundary.
pub(crate) trait SourceCallPhaseBuilder<I: Instr> {
    fn lower_input(&mut self, input: I) -> Result<I, String>;
    fn capture(&mut self, value: I) -> I::Name;
    fn emit(&mut self, statement: I);
    fn tuple(&mut self, values: Vec<I>, meta: &Meta) -> I;
    fn keyword_literal(&mut self, name: &str, meta: &Meta) -> I;
}

fn take<I>(name: &I::Name) -> I
where
    I: Instr + From<TakeOperand<I>>,
{
    TakeOperand::<I>::new(name.clone())
        .with_meta(Meta::synthetic())
        .into()
}

fn build<I, B>(builder: &mut B, kind: BuildCollectionKind, values: Vec<I>, meta: &Meta) -> I::Name
where
    I: Instr + From<BuildCollection<I>>,
    B: SourceCallPhaseBuilder<I>,
{
    builder.capture(
        BuildCollection::new(kind, values)
            .with_meta(meta.clone())
            .into(),
    )
}

fn phase<I, B>(
    builder: &mut B,
    kind: CallArgumentOpKind,
    callable: &I::Name,
    buffer: &I::Name,
    value: Option<I>,
    meta: &Meta,
) where
    I: Instr + From<CallArgumentOp<I>>,
    B: SourceCallPhaseBuilder<I>,
{
    builder.emit(
        CallArgumentOp::<I>::new(kind, callable.clone(), buffer.clone(), value.map(Box::new))
            .with_meta(meta.clone())
            .into(),
    );
}

fn insert<I, B>(
    builder: &mut B,
    kind: ComprehensionInsertKind,
    buffer: &I::Name,
    key: Option<I>,
    value: I,
    meta: &Meta,
) where
    I: Instr + From<ComprehensionInsert<I>>,
    B: SourceCallPhaseBuilder<I>,
{
    builder.emit(
        ComprehensionInsert::<I>::new(kind, buffer.clone(), key.map(Box::new), Box::new(value))
            .with_meta(meta.clone())
            .into(),
    );
}

/// Return both the child-rewritten ordinary call and the fully prepared call.
/// The adapter can discard its private phase output when no child needed setup;
/// no original child is lowered twice and no partial phase bundle is committed.
pub(crate) fn lower_source_call_phases<I, B>(
    mut call: Call<I>,
    builder: &mut B,
) -> Result<(Call<I>, PreparedCall<I>), String>
where
    I: Instr
        + From<TakeOperand<I>>
        + From<BuildCollection<I>>
        + From<CallArgumentOp<I>>
        + From<ComprehensionInsert<I>>,
    B: SourceCallPhaseBuilder<I>,
{
    let meta = call.meta();
    let callable_value = builder.lower_input(*call.func)?;
    call.func = Box::new(callable_value.clone());
    let callable = builder.capture(callable_value);

    let singleton_star = matches!(call.args.as_slice(), [CallArgPositional::Starred(_)]);
    let big_positional = call.args.len() > NATIVE_STACK_USE_GUIDELINE;
    let list_positional = !singleton_star
        && (big_positional
            || call
                .args
                .iter()
                .any(|arg| matches!(arg, CallArgPositional::Starred(_))));
    let mut arguments = if list_positional && big_positional {
        Some(build::<I, B>(
            builder,
            BuildCollectionKind::List,
            Vec::new(),
            &meta,
        ))
    } else {
        None
    };
    let mut prefix = Vec::new();
    let mut original_args = Vec::with_capacity(call.args.len());
    for argument in call.args {
        let (starred, value) = match argument {
            CallArgPositional::Positional(value) => (false, value),
            CallArgPositional::Starred(value) => (true, value),
        };
        // BUILD_LIST runs before evaluating the first star expression.
        if starred && !singleton_star && arguments.is_none() {
            arguments = Some(build::<I, B>(
                builder,
                BuildCollectionKind::List,
                std::mem::take(&mut prefix),
                &meta,
            ));
        }
        let value = builder.lower_input(value)?;
        original_args.push(if starred {
            CallArgPositional::Starred(value.clone())
        } else {
            CallArgPositional::Positional(value.clone())
        });
        let operand = builder.capture(value);
        if singleton_star {
            arguments = Some(operand);
        } else if let Some(buffer) = &arguments {
            if starred {
                phase::<I, B>(
                    builder,
                    CallArgumentOpKind::ExtendPositional,
                    &callable,
                    buffer,
                    Some(take::<I>(&operand)),
                    &meta,
                );
            } else {
                insert::<I, B>(
                    builder,
                    ComprehensionInsertKind::ListAppend,
                    buffer,
                    None,
                    take::<I>(&operand),
                    &meta,
                );
            }
        } else {
            prefix.push(take::<I>(&operand));
        }
    }
    call.args = original_args;
    let arguments = match arguments {
        Some(arguments) => {
            if list_positional {
                phase::<I, B>(
                    builder,
                    CallArgumentOpKind::FinishPositionalList,
                    &callable,
                    &arguments,
                    None,
                    &meta,
                );
            }
            arguments
        }
        None => {
            let tuple = builder.tuple(prefix, &meta);
            builder.capture(tuple)
        }
    };

    let mut keywords = None;
    let mut original_keywords = Vec::with_capacity(call.keywords.len());
    let mut pending = call.keywords.into_iter().peekable();
    while let Some(keyword) = pending.next() {
        let CallArgKeyword::Named { arg, value } = keyword else {
            let CallArgKeyword::Starred(value) = keyword else {
                unreachable!()
            };
            // Empty kwargs allocation precedes evaluation of the first ** value.
            if keywords.is_none() {
                keywords = Some(build::<I, B>(
                    builder,
                    BuildCollectionKind::Dict,
                    Vec::new(),
                    &meta,
                ));
            }
            let value = builder.lower_input(value)?;
            original_keywords.push(CallArgKeyword::Starred(value.clone()));
            let update = builder.capture(value);
            phase::<I, B>(
                builder,
                CallArgumentOpKind::MergeKeywords,
                &callable,
                keywords.as_ref().expect("keyword buffer"),
                Some(take::<I>(&update)),
                &meta,
            );
            continue;
        };
        let mut group_values = vec![(arg, value)];
        while matches!(pending.peek(), Some(CallArgKeyword::Named { .. })) {
            let Some(CallArgKeyword::Named { arg, value }) = pending.next() else {
                unreachable!()
            };
            group_values.push((arg, value));
        }
        let big = group_values.len() > NATIVE_STACK_USE_GUIDELINE / 2;
        let group = if big {
            Some(build::<I, B>(
                builder,
                BuildCollectionKind::Dict,
                Vec::new(),
                &meta,
            ))
        } else {
            None
        };
        let mut pairs = Vec::new();
        for (arg, value) in group_values {
            // Pinned codegen_subkwargs loads each key before its value, even
            // for the small BUILD_MAP group (not BUILD_CONST_KEY_MAP).
            let key = builder.keyword_literal(arg.as_str(), &meta);
            let key = builder.capture(key);
            let value = builder.lower_input(value)?;
            original_keywords.push(CallArgKeyword::Named {
                arg,
                value: value.clone(),
            });
            let value = builder.capture(value);
            if let Some(group) = &group {
                insert::<I, B>(
                    builder,
                    ComprehensionInsertKind::DictSetItem,
                    group,
                    Some(take::<I>(&key)),
                    take::<I>(&value),
                    &meta,
                );
            } else {
                pairs.push(take::<I>(&key));
                pairs.push(take::<I>(&value));
            }
        }
        let group = group
            .unwrap_or_else(|| build::<I, B>(builder, BuildCollectionKind::Dict, pairs, &meta));
        if let Some(keywords) = &keywords {
            // A whole contiguous named group precedes this duplicate-checking merge.
            phase::<I, B>(
                builder,
                CallArgumentOpKind::MergeKeywords,
                &callable,
                keywords,
                Some(take::<I>(&group)),
                &meta,
            );
        } else {
            keywords = Some(group);
        }
    }
    call.keywords = original_keywords;
    if singleton_star {
        phase::<I, B>(
            builder,
            CallArgumentOpKind::NormalizeSingletonStar,
            &callable,
            &arguments,
            None,
            &meta,
        );
    }
    // Namespace metadata remains an unevaluated frame coordinate.
    let prepared = PreparedCall::new(
        Box::new(take::<I>(&callable)),
        Box::new(take::<I>(&arguments)),
        keywords.map(|name| Box::new(take::<I>(&name))),
        call.frame_namespace.clone(),
    )
    .with_meta(meta)
    .with_extra(call.extra.clone());
    Ok((call, prepared))
}

struct RuffSourceCallPhaseBuilder<'a, L: ?Sized, E: RuffToBlockPyExpr> {
    lowerer: &'a L,
    prepared: BlockPyStmtBuilder<E>,
    loop_ctx: Option<&'a LoopContext>,
    has_setup: bool,
}

impl<L, E> SourceCallPhaseBuilder<InstrRuff> for RuffSourceCallPhaseBuilder<'_, L, E>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    fn lower_input(&mut self, input: InstrRuff) -> Result<InstrRuff, String> {
        let mut setup = BlockPyStmtBuilder::<E>::new(self.prepared.name_gen());
        let value = self
            .lowerer
            .lower_expr_instr_into(input, &mut setup, self.loop_ctx)?;
        if !setup.is_empty() {
            self.has_setup = true;
            self.prepared.append_fragment(setup.finish_fallthrough());
        }
        Ok(value)
    }

    fn capture(&mut self, value: InstrRuff) -> <InstrRuff as Instr>::Name {
        let name = self.lowerer.fresh_operand_binding();
        let meta = value.meta();
        // Reserve here, before lowering the next child's CFG.
        let unwind_order = self.prepared.name_gen().next_temporary_sequence();
        self.prepared.push_stmt(
            Store::new(name.clone(), E::from_lowered_expr(value))
                .with_lifetime(StoreLifetime::Operand { unwind_order })
                .with_meta(meta)
                .into(),
        );
        name.into()
    }

    fn emit(&mut self, statement: InstrRuff) {
        self.prepared.push_stmt(E::from_lowered_expr(statement));
    }

    fn tuple(&mut self, values: Vec<InstrRuff>, meta: &Meta) -> InstrRuff {
        ExprTuple::new(values, ast::ExprContext::Load, false)
            .with_meta(meta.clone())
            .into()
    }

    fn keyword_literal(&mut self, name: &str, meta: &Meta) -> InstrRuff {
        crate::passes::ast_to_instr::from_ast_expr(py_expr!("{name:literal}", name = name))
            .with_meta(meta.clone())
    }
}

/// Only unregistered source calls enter this path. Keep the ordinary Call when
/// no child emitted setup, and commit the complete prepared bundle otherwise.
pub(super) fn lower_call_with_setup<L, E>(
    lowerer: &L,
    call: Call<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    if lowerer.recorded_call_runtime_start(&call).is_some() {
        return Err("recorded operation cannot use ordinary expanded-call preparation".into());
    }
    let mut builder = RuffSourceCallPhaseBuilder {
        lowerer,
        prepared: BlockPyStmtBuilder::<E>::new(out.name_gen()),
        loop_ctx,
        has_setup: false,
    };
    let (ordinary, prepared) = lower_source_call_phases(call, &mut builder)?;
    if !builder.has_setup {
        return Ok(ordinary.into());
    }
    out.append_fragment(builder.prepared.finish_fallthrough());
    Ok(prepared.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{BlockTerm, InstrWithAwaitAndYield, NameLike};
    use crate::passes::ruff_to_blockpy::expr_lowering::AstSetupExprLowerer;
    use crate::passes::ruff_to_blockpy::{test_name_gen, InlineFragment};
    use ruff_python_parser::parse_expression;
    use std::collections::HashSet;

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Call(String),
        Branch,
        Phase(CallArgumentOpKind),
        Build(BuildCollectionKind, usize),
        Insert(ComprehensionInsertKind),
    }

    fn lower(
        source: &str,
    ) -> (
        InstrWithAwaitAndYield,
        InlineFragment<InstrWithAwaitAndYield>,
    ) {
        let InstrRuff::Call(call) = crate::passes::ast_to_instr::from_ast_expr(
            *parse_expression(source).unwrap().into_syntax().body,
        ) else {
            panic!("source must be one call")
        };
        let names = test_name_gen();
        let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&names);
        let value = lower_call_with_setup(&AstSetupExprLowerer, call, &mut out, None).unwrap();
        (
            InstrWithAwaitAndYield::from_lowered_expr(value),
            out.finish_fallthrough(),
        )
    }

    fn events(fragment: &InlineFragment<InstrWithAwaitAndYield>) -> Vec<Event> {
        fn expression(expr: &InstrWithAwaitAndYield, events: &mut Vec<Event>) {
            match expr {
                InstrWithAwaitAndYield::Store(op) => expression(&op.value, events),
                InstrWithAwaitAndYield::Call(op) => {
                    if let InstrWithAwaitAndYield::Load(name) = op.func.as_ref() {
                        events.push(Event::Call(name.name.id_str().to_owned()));
                    }
                }
                InstrWithAwaitAndYield::BuildCollection(op) => {
                    events.push(Event::Build(op.kind, op.values.len()))
                }
                InstrWithAwaitAndYield::CallArgumentOp(op) => events.push(Event::Phase(op.kind)),
                InstrWithAwaitAndYield::ComprehensionInsert(op) => {
                    events.push(Event::Insert(op.kind))
                }
                _ => {}
            }
        }
        let mut events = Vec::new();
        let mut block = &fragment.entry;
        let mut visited = HashSet::new();
        loop {
            assert!(visited.insert(block.label));
            for statement in &block.body {
                expression(statement, &mut events);
            }
            let target = match &block.term {
                BlockTerm::Jump(edge) => edge.target,
                BlockTerm::IfTerm(branch) => {
                    expression(&branch.test, &mut events);
                    events.push(Event::Branch);
                    branch.then_label
                }
                _ => break,
            };
            let Some(next) = fragment
                .deps
                .iter()
                .find(|candidate| candidate.label == target)
            else {
                break;
            };
            block = next;
        }
        events
    }

    #[test]
    fn call_argument_setup_preserves_ordinary_call_without_child_cfg() {
        let (value, fragment) = lower("callee(*values, **keywords)");
        assert!(matches!(value, InstrWithAwaitAndYield::Call(_)));
        assert!(fragment.entry.body.is_empty());
        assert!(fragment.deps.is_empty());
    }

    #[test]
    fn call_argument_setup_expands_prefix_but_defers_singleton_star() {
        for (source, expected_before_branch) in [
            ("callee(*values, left() if predicate() else right())", true),
            (
                "callee(*values, tail=left() if predicate() else right())",
                false,
            ),
        ] {
            let (value, fragment) = lower(source);
            assert!(matches!(value, InstrWithAwaitAndYield::PreparedCall(_)));
            let events = events(&fragment);
            let branch = events
                .iter()
                .position(|event| *event == Event::Branch)
                .unwrap();
            assert_eq!(
                events[..branch].contains(&Event::Phase(CallArgumentOpKind::ExtendPositional)),
                expected_before_branch
            );
            if !expected_before_branch {
                let normalization = events
                    .iter()
                    .position(|event| {
                        *event == Event::Phase(CallArgumentOpKind::NormalizeSingletonStar)
                    })
                    .unwrap();
                assert!(normalization > branch);
                assert!(events[branch..normalization]
                    .contains(&Event::Build(BuildCollectionKind::Dict, 2)));
            }
        }
    }

    #[test]
    fn call_argument_setup_merges_mapping_before_branch_and_named_group_after_all_values() {
        let (_, fragment) =
            lower("callee(**mapping, a=first(), b=left() if predicate() else right())");
        let events = events(&fragment);
        let merges: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(index, event)| {
                (*event == Event::Phase(CallArgumentOpKind::MergeKeywords)).then_some(index)
            })
            .collect();
        assert_eq!(merges.len(), 2);
        let first = events
            .iter()
            .position(|event| *event == Event::Call("first".into()))
            .unwrap();
        let branch = events
            .iter()
            .position(|event| *event == Event::Branch)
            .unwrap();
        assert!(merges[0] < first && first < branch && branch < merges[1]);
        assert!(events[branch..merges[1]].contains(&Event::Build(BuildCollectionKind::Dict, 4)));
    }

    #[test]
    fn call_argument_setup_large_keyword_group_uses_native_incremental_map_shape() {
        let mut arguments: Vec<_> = (0..15)
            .map(|index| format!("k{index}=value{index}()"))
            .collect();
        arguments.push("last=left() if predicate() else right()".into());
        let (_, fragment) = lower(&format!("callee(*values, {})", arguments.join(", ")));
        let events = events(&fragment);
        assert_eq!(
            events
                .iter()
                .filter(|event| **event == Event::Insert(ComprehensionInsertKind::DictSetItem))
                .count(),
            16
        );
        assert!(!events.contains(&Event::Build(BuildCollectionKind::Dict, 32)));
        let empty = events
            .iter()
            .position(|event| *event == Event::Build(BuildCollectionKind::Dict, 0))
            .unwrap();
        let first = events
            .iter()
            .position(|event| *event == Event::Call("value0".into()))
            .unwrap();
        assert!(empty < first);
    }

    #[test]
    fn call_argument_setup_singleton_raw_unwinds_after_keywords_and_branch_temporaries() {
        let (value, fragment) = lower("callee(*values, tail=left() if predicate() else right())");
        let InstrWithAwaitAndYield::PreparedCall(call) = value else {
            panic!("prepared call")
        };
        let InstrWithAwaitAndYield::TakeOperand(arguments) = call.arguments.as_ref() else {
            panic!("argument move")
        };
        let InstrWithAwaitAndYield::TakeOperand(keywords) = call.keywords.as_deref().unwrap()
        else {
            panic!("keyword move")
        };
        let stores: Vec<_> = std::iter::once(&fragment.entry)
            .chain(&fragment.deps)
            .flat_map(|block| &block.body)
            .filter_map(|statement| {
                let InstrWithAwaitAndYield::Store(op) = statement else {
                    return None;
                };
                let StoreLifetime::Operand { unwind_order } = op.lifetime else {
                    return None;
                };
                Some((op.name.id_str(), unwind_order))
            })
            .collect();
        let rank = |name: &str| {
            stores
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .unwrap()
                .1
        };
        assert!(rank(keywords.name.id_str()) > rank(arguments.name.id_str()));
        assert_eq!(
            events(&fragment).last(),
            Some(&Event::Phase(CallArgumentOpKind::NormalizeSingletonStar))
        );
    }
}
