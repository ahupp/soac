use crate::block_py::{
    Block, BlockLabel, BlockTerm, CounterDef, CounterId, CounterScope, CounterSite, Instr,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CounterHandle {
    id: CounterId,
}

impl CounterHandle {
    pub const fn id(self) -> CounterId {
        self.id
    }
}

pub trait CounterSpec {
    fn scope(&self) -> CounterScope;
    fn kind(&self) -> &str;
    fn site(&self) -> CounterSite;
}

pub struct CounterBuilder<'a> {
    defs: &'a mut Vec<CounterDef>,
    next_id: usize,
}

impl<'a> CounterBuilder<'a> {
    pub fn new(defs: &'a mut Vec<CounterDef>) -> Self {
        let next_id = defs
            .iter()
            .map(|def| def.id.0)
            .max()
            .map(|id| id + 1)
            .unwrap_or(0);
        Self { defs, next_id }
    }

    pub fn define(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
    ) -> CounterHandle {
        let handle = CounterHandle {
            id: CounterId(self.next_id),
        };
        self.next_id += 1;
        self.defs.push(CounterDef {
            id: handle.id,
            scope,
            kind: kind.into(),
            site,
        });
        handle
    }

    pub fn define_spec(&mut self, spec: &impl CounterSpec) -> CounterHandle {
        self.define(spec.scope(), spec.kind(), spec.site())
    }

    pub fn define_if_missing(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
    ) -> CounterHandle {
        let kind = kind.into();
        if let Some(existing) = self
            .defs
            .iter()
            .find(|counter| counter.scope == scope && counter.kind == kind && counter.site == site)
        {
            return CounterHandle { id: existing.id };
        }
        self.define(scope, kind, site)
    }

    pub fn define_if_missing_spec(&mut self, spec: &impl CounterSpec) -> CounterHandle {
        self.define_if_missing(spec.scope(), spec.kind(), spec.site())
    }
}

#[derive(Debug, Clone)]
pub enum OptInstr<I: Instr> {
    Instr(I),
    Block(OptBlock<I>),
}

#[derive(Debug, Clone)]
pub struct OptBlock<I: Instr> {
    entry: Block<I>,
    dependencies: Vec<Block<I>>,
}

impl<I: Instr> OptBlock<I> {
    pub fn new(entry: Block<I>, dependencies: Vec<Block<I>>) -> Result<Self, String> {
        validate_opt_block(&entry, &dependencies)?;
        Ok(Self {
            entry,
            dependencies,
        })
    }

    pub fn entry(&self) -> &Block<I> {
        &self.entry
    }

    pub fn entry_mut(&mut self) -> &mut Block<I> {
        &mut self.entry
    }

    pub fn dependencies(&self) -> &[Block<I>] {
        &self.dependencies
    }

    pub fn dependencies_mut(&mut self) -> &mut [Block<I>] {
        &mut self.dependencies
    }

    pub fn into_parts(self) -> (Block<I>, Vec<Block<I>>) {
        (self.entry, self.dependencies)
    }

    pub fn replace_fallthrough_target(&mut self, target: BlockLabel) -> bool {
        let mut replaced = self.entry.replace_fallthrough_target(target);
        for block in &mut self.dependencies {
            replaced |= block.replace_fallthrough_target(target);
        }
        replaced
    }
}

pub trait InstrumentInstr<I: Instr> {
    type Counter;

    fn instrument_instr(&self, instr: &I) -> Option<Self::Counter>;

    fn optimize_instr(&self, counter: &Self::Counter, instr: &I) -> OptInstr<I>;
}

fn validate_opt_block<I: Instr>(entry: &Block<I>, dependencies: &[Block<I>]) -> Result<(), String> {
    let mut blocks_by_label = HashMap::new();
    blocks_by_label.insert(entry.label, entry);
    for block in dependencies {
        if blocks_by_label.insert(block.label, block).is_some() {
            return Err(format!(
                "optimization fragment reuses block label {}",
                block.label
            ));
        }
    }

    let mut reachable = HashSet::new();
    let mut memo = HashMap::new();
    if !all_paths_end_in_fallthrough(
        entry.label,
        &blocks_by_label,
        &mut reachable,
        &mut Vec::new(),
        &mut memo,
    ) {
        return Err(format!(
            "optimization fragment rooted at {} must keep every reachable path inside the fragment until {}",
            entry.label,
            BlockLabel::fallthrough()
        ));
    }

    for block in dependencies {
        if !reachable.contains(&block.label) {
            return Err(format!(
                "optimization fragment dependency {} is unreachable from {}",
                block.label, entry.label
            ));
        }
    }

    Ok(())
}

fn all_paths_end_in_fallthrough<I: Instr>(
    label: BlockLabel,
    blocks_by_label: &HashMap<BlockLabel, &Block<I>>,
    reachable: &mut HashSet<BlockLabel>,
    stack: &mut Vec<BlockLabel>,
    memo: &mut HashMap<BlockLabel, bool>,
) -> bool {
    if let Some(valid) = memo.get(&label) {
        reachable.insert(label);
        return *valid;
    }

    if stack.contains(&label) {
        return false;
    }

    let Some(block) = blocks_by_label.get(&label) else {
        return false;
    };

    reachable.insert(label);
    stack.push(label);
    let valid = match &block.term {
        BlockTerm::Jump(edge) => {
            edge.target.is_fallthrough()
                || all_paths_end_in_fallthrough(
                    edge.target,
                    blocks_by_label,
                    reachable,
                    stack,
                    memo,
                )
        }
        BlockTerm::IfTerm(if_term) => {
            [if_term.then_label, if_term.else_label]
                .into_iter()
                .all(|target| {
                    target.is_fallthrough()
                        || all_paths_end_in_fallthrough(
                            target,
                            blocks_by_label,
                            reachable,
                            stack,
                            memo,
                        )
                })
        }
        BlockTerm::BranchTable(branch) => branch
            .targets
            .iter()
            .copied()
            .chain(std::iter::once(branch.default_label))
            .all(|target| {
                target.is_fallthrough()
                    || all_paths_end_in_fallthrough(target, blocks_by_label, reachable, stack, memo)
            }),
        BlockTerm::Raise(_) | BlockTerm::Return(_) => false,
    };
    stack.pop();
    memo.insert(label, valid);
    valid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{
        BlockEdge, Call, CallArgPositional, CallDirect, CalleeFunctionId, FunctionId, HasMeta,
        HasSemanticInstrId, InstrCodegen, InstrId, InstrWithConstantNone, Load, Meta, NameLocation,
        ResolvedName, TermBranchTable, WithMeta,
    };
    use crate::passes::InstrRuff;
    use crate::py_expr;
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CallHotTargetsCounterSpec {
        function_id: FunctionId,
        instr_id: InstrId,
    }

    impl CounterSpec for CallHotTargetsCounterSpec {
        fn scope(&self) -> CounterScope {
            CounterScope::This
        }

        fn kind(&self) -> &str {
            "call_hot_targets"
        }

        fn site(&self) -> CounterSite {
            CounterSite::Runtime {
                function_id: Some(self.function_id),
                instr_id: Some(self.instr_id),
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct HotCallTargets {
        most_frequent: FunctionId,
        second_most_frequent: FunctionId,
    }

    struct ExampleCallHotTargetRule {
        function_id: FunctionId,
        entry_label: BlockLabel,
        hot0_label: BlockLabel,
        hot1_label: BlockLabel,
        generic_label: BlockLabel,
    }

    impl InstrumentInstr<InstrCodegen> for ExampleCallHotTargetRule {
        type Counter = CallHotTargetsCounterSpec;

        fn instrument_instr(&self, instr: &InstrCodegen) -> Option<Self::Counter> {
            let InstrCodegen::Call(_) = instr else {
                return None;
            };
            Some(CallHotTargetsCounterSpec {
                function_id: self.function_id,
                instr_id: instr.semantic_instr_id(),
            })
        }

        fn optimize_instr(
            &self,
            counter: &Self::Counter,
            instr: &InstrCodegen,
        ) -> OptInstr<InstrCodegen> {
            let InstrCodegen::Call(call) = instr else {
                return OptInstr::Instr(instr.clone());
            };
            let hot_targets = HotCallTargets {
                most_frequent: FunctionId::new(9, 11),
                second_most_frequent: FunctionId::new(9, 12),
            };
            let meta = call.meta();

            let entry = Block::new(
                self.entry_label,
                Vec::new(),
                BlockTerm::BranchTable(TermBranchTable {
                    index: InstrCodegen::CalleeFunctionId(
                        CalleeFunctionId::new((*call.func).clone()).with_meta(meta.clone()),
                    ),
                    targets: vec![self.hot0_label, self.hot1_label],
                    default_label: self.generic_label,
                }),
                Vec::new(),
                None,
            );
            let hot0 = Block::new(
                self.hot0_label,
                vec![InstrCodegen::CallDirect(
                    CallDirect::new(
                        (*call.func).clone(),
                        hot_targets.most_frequent,
                        call.args.clone(),
                        call.keywords.clone(),
                    )
                    .with_meta(meta.clone()),
                )],
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            );
            let hot1 = Block::new(
                self.hot1_label,
                vec![InstrCodegen::CallDirect(
                    CallDirect::new(
                        (*call.func).clone(),
                        hot_targets.second_most_frequent,
                        call.args.clone(),
                        call.keywords.clone(),
                    )
                    .with_meta(meta.clone()),
                )],
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            );
            let generic = Block::new(
                self.generic_label,
                vec![InstrCodegen::Call(
                    call.clone().with_meta(counter_instr_meta(counter, meta)),
                )],
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            );

            OptInstr::Block(
                OptBlock::new(entry, vec![hot0, hot1, generic])
                    .expect("call hot target example fragment should validate"),
            )
        }
    }

    fn counter_instr_meta(counter: &CallHotTargetsCounterSpec, mut meta: Meta) -> Meta {
        meta.instr_id = Some(counter.instr_id);
        meta
    }

    fn runtime_name_expr(name: &str, meta: Meta) -> InstrCodegen {
        Load::new(ResolvedName {
            id: name.into(),
            location: NameLocation::RuntimeName,
        })
        .with_meta(meta)
        .into()
    }

    #[test]
    fn counter_builder_allocates_sequential_ids() {
        let mut defs = Vec::new();
        let mut builder = CounterBuilder::new(&mut defs);

        let first = builder.define(
            CounterScope::This,
            "call_hot_targets",
            CounterSite::Runtime {
                function_id: Some(FunctionId::new(1, 2)),
                instr_id: Some(InstrId::new(BlockLabel::from_index(3), 4)),
            },
        );
        let second = builder.define(
            CounterScope::Global,
            "global_load_hit",
            CounterSite::Runtime {
                function_id: None,
                instr_id: None,
            },
        );

        assert_eq!(first.id(), CounterId(0));
        assert_eq!(second.id(), CounterId(1));
    }

    #[test]
    fn counter_builder_reuses_existing_definition() {
        let site = CounterSite::Runtime {
            function_id: Some(FunctionId::new(1, 2)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(0), 7)),
        };
        let mut defs = vec![CounterDef {
            id: CounterId(9),
            scope: CounterScope::Function,
            kind: "runtime_incref".to_string(),
            site: site.clone(),
        }];

        let handle = CounterBuilder::new(&mut defs).define_if_missing(
            CounterScope::Function,
            "runtime_incref",
            site,
        );

        assert_eq!(handle.id(), CounterId(9));
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn counter_builder_defines_from_counter_spec() {
        let spec = CallHotTargetsCounterSpec {
            function_id: FunctionId::new(1, 2),
            instr_id: InstrId::new(BlockLabel::from_index(3), 4),
        };
        let mut defs = Vec::new();
        let handle = CounterBuilder::new(&mut defs).define_spec(&spec);

        assert_eq!(handle.id(), CounterId(0));
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "call_hot_targets");
        assert_eq!(
            defs[0].site,
            CounterSite::Runtime {
                function_id: Some(FunctionId::new(1, 2)),
                instr_id: Some(InstrId::new(BlockLabel::from_index(3), 4)),
            }
        );
    }

    #[test]
    fn opt_block_accepts_fallthrough_fragment() {
        let entry = Block::new(
            BlockLabel::from_index(0),
            vec![crate::passes::ast_to_instr::from_ast_expr(py_expr!("x"))],
            BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
            Vec::new(),
            None,
        );

        let fragment = OptBlock::new(entry, Vec::new()).expect("fragment should validate");

        assert_eq!(fragment.entry().label, BlockLabel::from_index(0));
    }

    #[test]
    fn opt_block_rejects_non_fallthrough_cycle() {
        let entry = Block::new(
            BlockLabel::from_index(0),
            Vec::<InstrRuff>::new(),
            BlockTerm::Jump(BlockEdge::new(BlockLabel::from_index(1))),
            Vec::new(),
            None,
        );
        let dep = Block::new(
            BlockLabel::from_index(1),
            Vec::<InstrRuff>::new(),
            BlockTerm::Jump(BlockEdge::new(BlockLabel::from_index(0))),
            Vec::new(),
            None,
        );

        let err = OptBlock::new(entry, vec![dep]).expect_err("cycle should be rejected");

        assert!(err.contains("fallthrough"), "{err}");
    }

    #[test]
    fn opt_block_rejects_return_exit() {
        let entry = Block::new(
            BlockLabel::from_index(0),
            Vec::<InstrRuff>::new(),
            BlockTerm::Return(InstrRuff::constant_none()),
            Vec::new(),
            None,
        );

        let err = OptBlock::new(entry, Vec::new()).expect_err("return should be rejected");

        assert!(err.contains("fallthrough"), "{err}");
    }

    #[test]
    fn example_call_rule_matches_calls_and_builds_specialization_fragment() {
        let function_id = FunctionId::new(7, 8);
        let rule = ExampleCallHotTargetRule {
            function_id,
            entry_label: BlockLabel::from_index(10),
            hot0_label: BlockLabel::from_index(11),
            hot1_label: BlockLabel::from_index(12),
            generic_label: BlockLabel::from_index(13),
        };
        let call = InstrCodegen::Call(
            Call::new(
                runtime_name_expr("__dp_dynamic_callee", Meta::synthetic()),
                vec![CallArgPositional::Positional(runtime_name_expr(
                    "arg0",
                    Meta::synthetic(),
                ))],
                Vec::new(),
            )
            .with_meta(Meta {
                instr_id: Some(InstrId::new(BlockLabel::from_index(2), 5)),
                ..Meta::synthetic()
            }),
        );

        let counter = rule
            .instrument_instr(&call)
            .expect("call should produce instrumentation spec");
        assert_eq!(counter.function_id, function_id);
        assert_eq!(counter.instr_id, InstrId::new(BlockLabel::from_index(2), 5));

        let OptInstr::Block(fragment) = rule.optimize_instr(&counter, &call) else {
            panic!("call optimization should return an OptBlock");
        };

        assert_eq!(fragment.entry().label, BlockLabel::from_index(10));
        let BlockTerm::BranchTable(branch) = &fragment.entry().term else {
            panic!("entry fragment should dispatch with br_table");
        };
        assert!(matches!(branch.index, InstrCodegen::CalleeFunctionId(_)));
        assert_eq!(
            branch.targets,
            vec![BlockLabel::from_index(11), BlockLabel::from_index(12)]
        );
        assert_eq!(branch.default_label, BlockLabel::from_index(13));
        assert_eq!(fragment.dependencies().len(), 3);
        assert!(matches!(
            fragment.dependencies()[0].body[0],
            InstrCodegen::CallDirect(_)
        ));
        assert!(matches!(
            fragment.dependencies()[1].body[0],
            InstrCodegen::CallDirect(_)
        ));
        assert!(matches!(
            fragment.dependencies()[2].body[0],
            InstrCodegen::Call(_)
        ));
        for block in fragment.dependencies() {
            let BlockTerm::Jump(edge) = &block.term else {
                panic!("dependency block should jump to fallthrough");
            };
            assert_eq!(edge.target, BlockLabel::fallthrough());
        }
    }
}
